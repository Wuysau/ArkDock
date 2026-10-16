import type { AppSnapshot, CommandArgs, CommandResults, DeviceRunIdentity, NativeControlReceipt, NativeFrame, PreviewVisibility, RequestScope } from './model.ts';
import { normalizeError } from './errors.ts';

interface DesktopBridge {
  core: { invoke<T>(command: string, args?: object): Promise<T> };
  event: { listen<T>(name: string, listener: (event: { payload: T }) => void): Promise<() => void> };
}
declare global { interface Window { __TAURI__?: DesktopBridge } }

export interface UiBridge {
  available: boolean;
  request: typeof request;
  observe: typeof observe;
  readFrame?: typeof readPreviewFrame;
  readDeviceFrame?: typeof readDeviceFrame;
  transfer?: typeof transfer;
  mcpStatus?: typeof mcpStatus;
  mcpRevokeContext?: typeof mcpRevokeContext;
  observeControlReceipts?: typeof observeControlReceipts;
  observeFrameReady?: typeof observeFrameReady;
}

export const nativeBridge: UiBridge = {
  get available() { return desktopAvailable(); },
  request,
  observe,
  readFrame: readPreviewFrame,
  readDeviceFrame,
  transfer,
  mcpStatus,
  mcpRevokeContext,
  observeControlReceipts,
  observeFrameReady,
};

export interface PreviewFrameReady { sessionId: string; buildId: string; generation: number; sequence: number }

/** Notifications carry no pixels and never replace scoped reads or receipt validation. */
export async function observeControlReceipts(listener: (receipt: NativeControlReceipt) => void): Promise<() => void> {
  const desktop = window.__TAURI__;
  if (!desktop) throw new Error('缺少桌面桥接。');
  return desktop.event.listen<NativeControlReceipt>('arkdock:control-receipt', (event) => listener(event.payload));
}

export async function observeFrameReady(listener: (frame: PreviewFrameReady) => void): Promise<() => void> {
  const desktop = window.__TAURI__;
  if (!desktop) throw new Error('缺少桌面桥接。');
  return desktop.event.listen<PreviewFrameReady>('arkdock:frame-ready', (event) => listener(event.payload));
}

export function desktopAvailable(): boolean { return Boolean(window.__TAURI__); }

/**
 * The single product entry to native commands. Every reply is an `AppSnapshot`;
 * every native failure is normalized here so no display layer has to interpret a
 * thrown value. Normalization never retries: Agent delivery, credential and
 * project-trust commands keep exactly one attempt.
 */
export async function request<K extends keyof CommandArgs>(command: K, args: Omit<CommandArgs[K], 'scope'>, scope?: RequestScope): Promise<AppSnapshot> {
  const desktop = window.__TAURI__;
  if (!desktop) throw new Error('缺少桌面桥接。请在 Windows 11 使用 pnpm dev 启动 ArkDock 独立应用。');
  if (command !== 'snapshot' && !scope) throw new Error('缺少当前目标/预览会话，请等待后端状态同步。');
  if (command === 'read_preview_frame') throw new Error('二进制帧需要使用 readPreviewFrame；不能按状态快照读取。');
  if (command === 'read_device_frame') throw new Error('设备二进制帧需要使用 readDeviceFrame；不能按状态快照读取。');
  if (command === 'get_motion_metrics') throw new Error('动效计数需要使用 readMotionMetrics；不能按业务快照读取。');
  try { return await desktop.core.invoke<AppSnapshot>(command, command === 'snapshot' ? args : { ...args, scope }); }
  catch (reason) { throw normalizeError(reason); }
}

export async function readMotionMetrics(scope: RequestScope): Promise<CommandResults['get_motion_metrics']> {
  const desktop = window.__TAURI__;
  if (!desktop) throw new Error('缺少桌面桥接。');
  return desktop.core.invoke<CommandResults['get_motion_metrics']>('get_motion_metrics', { scope });
}

/**
 * The binary transport keeps its own structured `PreviewProtocolError` identity:
 * the frame controller reads that exact value, and a normalized copy would drop
 * the identity that the IPC contract pins. The controller normalizes at use.
 */
export async function readPreviewFrame(scope: RequestScope, protocolVersion: number, consumerId: string): Promise<CommandResults['read_preview_frame']> {
  const desktop = window.__TAURI__;
  if (!desktop) throw new Error('缺少桌面桥接。');
  const args: CommandArgs['read_preview_frame'] = { scope, protocolVersion, consumerId };
  const bytes = await desktop.core.invoke<CommandResults['read_preview_frame']>('read_preview_frame', args);
  if (!(bytes instanceof ArrayBuffer)) throw new Error('二进制帧响应类型错误。');
  return bytes;
}

/** Device frames belong to their own run; a Previewer session is not required. */
export async function readDeviceFrame(scope: RequestScope, identity: DeviceRunIdentity, afterSequence: number): Promise<ArrayBuffer> {
  const desktop = window.__TAURI__;
  if (!desktop) throw new Error('缺少桌面桥接。');
  if (!scope.targetId) throw new Error('需要当前工程目标。');
  if (!Number.isSafeInteger(afterSequence) || afterSequence < 0) throw new Error('设备帧序号无效。');
  const args: CommandArgs['read_device_frame'] = { scope, identity, afterSequence };
  const bytes = await desktop.core.invoke<CommandResults['read_device_frame']>('read_device_frame', args);
  if (!(bytes instanceof ArrayBuffer)) throw new Error('设备二进制帧响应类型错误。');
  return bytes;
}

export type DataCommand = Exclude<keyof CommandArgs, 'mcp_status' | 'mcp_revoke_context' | 'read_preview_frame' | 'read_device_frame' | 'enable_binary_preview_frames' | 'disable_binary_preview_frames' | 'click_preview' | 'snapshot' | 'minimize_window' | 'close_window' | 'maximize_pair' | 'restore_pair' | 'begin_drag' | 'rediscover' | 'select_candidate' | 'select_directory' | 'inspect_directory_path' | 'detach' | 'set_trust' | 'configure_sdk' | 'refresh_preview' | 'stop_preview' | 'arrange_side_by_side'>;

/** Typed data replies use the same validated desktop entry, outside AppSnapshot. */
export async function transfer<K extends DataCommand>(command: K, args: Omit<CommandArgs[K], 'scope'>, scope: RequestScope): Promise<CommandResults[K]> {
  const desktop = window.__TAURI__;
  if (!desktop) throw new Error('缺少桌面桥接。');
  const deviceCommand = command === 'device_runtime_state' || command === 'device_runtime_action';
  if (!scope.targetId || !deviceCommand && !scope.sessionId) throw new Error(deviceCommand ? '需要当前工程目标。' : '需要当前目标与原生预览会话。');
  try { return await desktop.core.invoke<CommandResults[K]>(command, { ...args, scope }); }
  catch (reason) { throw normalizeError(reason); }
}

export async function observe(state: (value: AppSnapshot) => void, frame: (value: NativeFrame) => void, visibility?: (visible: boolean) => void): Promise<() => void> {
  const desktop = window.__TAURI__;
  if (!desktop) throw new Error('浏览器调试不具备窗口和原生预览能力。');
  const offState = await desktop.event.listen<AppSnapshot>('arkdock:state', (event) => state(event.payload));
  let offFrame: () => void;
  try { offFrame = await desktop.event.listen<NativeFrame>('arkdock:frame', (event) => frame(event.payload)); }
  catch (error) { offState(); throw error; }
  let offVisibility: () => void;
  try { offVisibility = await desktop.event.listen<PreviewVisibility>('arkdock:visibility', (event) => visibility?.(event.payload.visible)); }
  catch (error) { offState(); offFrame(); throw error; }
  return () => { offState(); offFrame(); offVisibility(); };
}

/**
 * MCP setup is available before docking. The backend validates the calling window;
 * task data continues to use the scoped transfer gate.
 */
export async function mcpStatus(): Promise<CommandResults['mcp_status']> {
  const desktop = window.__TAURI__;
  if (!desktop) throw new Error('缺少桌面桥接。请在 Windows 11 使用 pnpm dev 启动 ArkDock 独立应用。');
  try { return await desktop.core.invoke<CommandResults['mcp_status']>('mcp_status', {}); }
  catch (reason) { throw normalizeError(reason); }
}

/** Revokes only the selected MCP context. */
export async function mcpRevokeContext(contextId: string): Promise<boolean> {
  const desktop = window.__TAURI__;
  if (!desktop) throw new Error('缺少桌面桥接。');
  try { return await desktop.core.invoke<boolean>('mcp_revoke_context', { contextId }); }
  catch (reason) { throw normalizeError(reason); }
}
