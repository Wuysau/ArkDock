import type { AppSnapshot, CommandArgs, CommandResults, NativeFrame, PreviewVisibility, RequestScope } from './model.ts';
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
  transfer?: typeof transfer;
}

export const nativeBridge: UiBridge = {
  get available() { return desktopAvailable(); },
  request,
  observe,
  readFrame: readPreviewFrame,
  transfer,
};

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

export type DataCommand = Exclude<keyof CommandArgs, 'read_preview_frame' | 'enable_binary_preview_frames' | 'disable_binary_preview_frames' | 'click_preview' | 'snapshot' | 'minimize_window' | 'close_window' | 'maximize_pair' | 'restore_pair' | 'begin_drag' | 'rediscover' | 'select_candidate' | 'select_directory' | 'inspect_directory_path' | 'detach' | 'set_trust' | 'configure_sdk' | 'refresh_preview' | 'stop_preview' | 'arrange_side_by_side'>;

/** Typed data replies use the same validated desktop entry, outside AppSnapshot. */
export async function transfer<K extends DataCommand>(command: K, args: Omit<CommandArgs[K], 'scope'>, scope: RequestScope): Promise<CommandResults[K]> {
  const desktop = window.__TAURI__;
  if (!desktop) throw new Error('缺少桌面桥接。');
  if (!scope.targetId || !scope.sessionId) throw new Error('需要当前目标与原生预览会话。');
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
 * Context-source pairing is a pre-docking capability: it must work before any
 * project or preview session exists, so it bypasses the session-scoped `request`
 * gate on purpose. The backend still validates the calling window itself.
 */
export async function contextPairCreate(): Promise<CommandResults['context_pair_create']> {
  const desktop = window.__TAURI__;
  if (!desktop) throw new Error('缺少桌面桥接。请在 Windows 11 使用 pnpm dev 启动 ArkDock 独立应用。');
  return desktop.core.invoke<CommandResults['context_pair_create']>('context_pair_create', {});
}

/** Revokes the context-source pairing and every live registration. */
export async function contextPairRevoke(): Promise<boolean> {
  const desktop = window.__TAURI__;
  if (!desktop) throw new Error('缺少桌面桥接。');
  return desktop.core.invoke<boolean>('context_pair_revoke', {});
}
