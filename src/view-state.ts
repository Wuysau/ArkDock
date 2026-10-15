import type { AppSnapshot, Candidate, DeviceRuntimeState, NativeControlState, NativeFrame, Phase } from './ipc/model.ts';
import type { PreviewFrame } from './preview/frame-types.ts';
import { isBinaryFrame } from './preview/frame-types.ts';
import { hasDockCandidate } from './request-state.ts';
import { displayPath } from './path-display.ts';

export function applySnapshot(current: AppSnapshot | null, incoming: AppSnapshot): AppSnapshot {
  return current && incoming.revision <= current.revision ? current : incoming;
}

export function acceptFrame(state: AppSnapshot, current: PreviewFrame | null, frame: PreviewFrame): PreviewFrame | null {
  if ((isBinaryFrame(frame) && frame.workspaceId !== state.project?.id)
    || frame.sessionId !== state.sessionId || frame.generation !== state.build.generation || frame.buildId !== state.build.buildId
    || (current?.sessionId === frame.sessionId && current.buildId === frame.buildId
      && current.generation === frame.generation && frame.sequence <= current.sequence)) return current;
  return frame;
}

export function filterCandidates(items: Candidate[], query: string): Candidate[] {
  const search = query.trim().toLocaleLowerCase();
  return search ? items.filter(({ project, reasons }) => [project.name, project.appRoot, project.workspaceRoot,
    displayPath(project.appRoot), displayPath(project.workspaceRoot),
    project.module, project.environment, ...reasons].join(' ').toLocaleLowerCase().includes(search)) : items;
}

export function needsDirectoryFallback(items: Candidate[]): boolean {
  return !items.some((candidate) => candidate.valid && candidate.project.environment === 'windowsLocal');
}

export function phaseText(phase: Phase): string {
  const labels: Record<Phase, string> = {
    idle: '拖到应用窗口右侧，关联当前工程', discovering: '正在读取与目标相关的工程线索',
    selectProject: '请选择当前工程', untrusted: '工程未信任：仅进行只读检查',
    sdkMissing: '未找到 SDK，请指定合法安装的工具链', protocolBlocked: '已安装的 SDK 暂不受支持，请指定兼容的工具链',
    building: '正在构建实际工程', running: '原生预览运行中', stale: '画面来自旧代码',
    crashed: '后端已退出，请查看诊断', stopped: '预览已停止', unsupported: '桌面贴边需要 Windows 11 本地环境',
  };
  return labels[phase];
}

export type BindingState = 'unbound' | 'candidate' | 'binding' | 'bound' | 'unavailable';
export type WorkspaceState = 'unidentified' | 'discovering' | 'selecting' | 'untrusted' | 'ready' | 'unavailable';
export type PreviewState = 'unavailable' | 'idle' | 'building' | 'waitingFrame' | 'running' | 'stale' | 'stopped' | 'error';
export type CapabilityStatus = 'available' | 'unsupported' | 'unavailable' | 'pending' | 'experimental' | 'blocked';
export interface UiCapability { status: CapabilityStatus; reason: string }
export interface UiState {
  binding: BindingState;
  workspace: WorkspaceState;
  preview: PreviewState;
  bindingLabel: string;
  workspaceLabel: string;
  previewLabel: string;
  frameCurrent: boolean;
  inputAllowed: boolean;
  capabilities: Record<'click' | 'swipe' | 'text' | 'rotate' | 'refresh' | 'stop' | 'page' | 'device' | 'appTheme' | 'emulator' | 'physicalDevice', UiCapability>;
}

/** Frame metadata is validated again before display; pixels remain owned by the native transport. */
export function frameMetadataValid(frame: PreviewFrame): boolean {
  return Number.isSafeInteger(frame.width) && frame.width > 0 && frame.width <= 8192
    && Number.isSafeInteger(frame.height) && frame.height > 0 && frame.height <= 8192
    && frame.width * frame.height <= 16 * 1024 * 1024
    && Number.isSafeInteger(frame.generation) && frame.generation > 0
    && Number.isSafeInteger(frame.sequence) && frame.sequence >= 0
    && Number.isFinite(frame.capturedAt) && frame.capturedAt >= 0
    && [0, 90, 180, 270].includes(frame.rotation)
    && (isBinaryFrame(frame) ? frame.protocolVersion === 1 && frame.format === 'jpeg'
      && frame.jpeg instanceof ArrayBuffer && frame.jpeg.byteLength === frame.payloadLength
      && frame.payloadLength > 0 && frame.payloadLength <= 1024 * 1024 - 40
      : typeof frame.pngBase64 === 'string' && frame.pngBase64.length > 0 && frame.pngBase64.length <= 16 * 1024 * 1024);
}

export function frameBelongsToCurrentBuild(state: AppSnapshot | null, frame: PreviewFrame | null): boolean {
  return Boolean(state?.sessionId && state.target && state.project && frame && frameMetadataValid(frame)
    && (!isBinaryFrame(frame) || frame.workspaceId === state.project?.id)
    && frame.sessionId === state.sessionId && frame.buildId === state.build.buildId
    && frame.generation === state.build.generation);
}

/** Bounds are the rendered image rectangle, excluding shell borders and fit letterboxing. */
export function mapFramePoint(frame: Pick<NativeFrame, 'width' | 'height'>,
  bounds: { left: number; top: number; width: number; height: number }, clientX: number, clientY: number): { x: number; y: number } | null {
  if (bounds.width <= 0 || bounds.height <= 0 || !Number.isFinite(clientX) || !Number.isFinite(clientY)) return null;
  const x = Math.floor((clientX - bounds.left) * frame.width / bounds.width);
  const y = Math.floor((clientY - bounds.top) * frame.height / bounds.height);
  return x >= 0 && x < frame.width && y >= 0 && y < frame.height ? { x, y } : null;
}

/** These dimensions describe confirmed backend state, never a timer or a predicted command result. */
export function deriveUiState(state: AppSnapshot | null, frame: PreviewFrame | null, control?: NativeControlState | null, device?: DeviceRuntimeState | null): UiState {
  const binding: BindingState = state?.phase === 'unsupported' ? 'unavailable'
    : hasDockCandidate(state?.dockHint) ? 'candidate' : state?.target ? 'bound' : 'unbound';
  const workspace: WorkspaceState = state?.phase === 'discovering' ? 'discovering'
    : state?.phase === 'selectProject' ? 'selecting'
      : state?.project ? state.project.environment !== 'windowsLocal' ? 'unavailable'
        : state.trusted ? 'ready' : 'untrusted' : 'unidentified';
  const sameSessionFrame = Boolean(frame && state?.target && state?.project && state.sessionId === frame.sessionId);
  const frameCurrent = frameBelongsToCurrentBuild(state, frame) && !state?.build.stale && !state?.build.error;
  const preview: PreviewState = !state ? 'unavailable'
    : state.phase === 'stopped' ? 'stopped'
      : state.build.error || state.phase === 'crashed' || state.phase === 'stale' ? sameSessionFrame ? 'stale' : 'error'
        : !state.project || !state.trusted || !state.backend.available || state.phase === 'unsupported' ? 'unavailable'
          : state.phase === 'building' ? state.build.durationMs === null ? 'building' : 'waitingFrame'
            : state.phase === 'running' ? frameCurrent ? 'running' : 'waitingFrame' : 'idle';
  const inputAllowed = Boolean(frameCurrent && state?.phase === 'running' && state.trusted && state.backend.available && state.backend.canClick);
  const capability = (status: CapabilityStatus, reason: string): UiCapability => ({ status, reason });
  const gate: UiCapability | null = !state ? capability('pending', '等待 Windows 桌面后端连接')
    : !state.target || !state.project ? capability('pending', '先连接目标窗口并关联实际工程')
      : !state.trusted ? capability('pending', '工程执行信任尚未确认')
        : !state.backend.available ? capability('unavailable', state.backend.reason || '原生后端不可用') : null;
  const click = gate ?? (!state?.backend.canClick ? capability('unsupported', '当前原生后端未提供点击输入')
    : inputAllowed ? capability('available', '点击当前有效原生画面')
      : capability('pending', preview === 'stale' || preview === 'error' ? '旧画面不可交互，等待当前构建恢复' : '等待当前构建的有效首帧'));
  const nativeCapability = (name: string, reason: string): UiCapability => {
    if (gate) return gate;
    const actual = control?.capabilities.find((item) => item.name === name);
    return actual ? capability(actual.status, actual.reason) : capability('unavailable', control ? reason : '等待原生控制能力状态');
  };
  const deviceCapability = (kind: 'emulator' | 'physicalDevice'): UiCapability => gate ?? capability('blocked', device?.reason
    || (kind === 'emulator' ? '未接入：暂不支持经由 ArkDock 启动鸿蒙虚拟设备' : '未接入：暂不支持经由 ArkDock 连接真机并显示画面'));
  const bindingLabels: Record<BindingState, string> = {
    unbound: '未绑定', candidate: '松开以连接', binding: '正在连接',
    bound: state?.target ? `已连接到 ${state.target.app}` : '已连接', unavailable: '窗口联动不可用',
  };
  const workspaceLabels: Record<WorkspaceState, string> = {
    unidentified: '工程未识别', discovering: '正在识别工程', selecting: '请确认当前工程',
    untrusted: '等待工程信任', ready: '工程已就绪', unavailable: '当前工程环境不支持原生预览',
  };
  const previewLabels: Record<PreviewState, string> = {
    unavailable: state?.backend.reason || '等待原生预览', idle: '预览待启动', building: '正在构建实际工程',
    waitingFrame: '等待当前构建的原生首帧', running: state?.backend.canClick ? '原生预览可交互' : '原生预览运行中',
    stale: state?.build.error ? '旧画面 · 当前代码构建失败' : '旧画面 · 原生后端不可用',
    stopped: '预览已停止', error: '预览失败，请查看诊断',
  };
  return {
    binding, workspace, preview, bindingLabel: bindingLabels[binding], workspaceLabel: workspaceLabels[workspace],
    previewLabel: previewLabels[preview], frameCurrent, inputAllowed,
    capabilities: {
      click,
      refresh: gate ?? capability('available', '重新构建当前受信任工程'),
      stop: !state ? capability('pending', '等待桌面后端') : state.sessionId && state.phase !== 'stopped'
        ? capability('available', '停止此工具拥有的原生预览资源') : capability('unavailable', '当前没有可停止的预览会话'),
      swipe: nativeCapability('touch', '未接入：暂不支持经由 ArkDock 拖拽或滑动'),
      text: nativeCapability('text', '未接入：暂不支持经由 ArkDock 发送文字输入'),
      rotate: nativeCapability('orientation', '未接入：暂不支持经由 ArkDock 切换设备方向'),
      page: nativeCapability('page', '未接入：暂不支持经由 ArkDock 切换页面'),
      device: nativeCapability('profile', '未接入：暂不支持经由 ArkDock 切换设备配置'),
      appTheme: nativeCapability('colorMode', '未接入：暂不支持经由 ArkDock 切换应用主题'),
      emulator: deviceCapability('emulator'),
      physicalDevice: deviceCapability('physicalDevice'),
    },
  };
}
