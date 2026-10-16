import type { AppSnapshot, Candidate, CommandArgs, NativeFrame, PairLayoutState, RequestScope } from '../../src/ipc/model.ts';
import type { UiBridge } from '../../src/ipc/client.ts';

export const scenarios = {
  welcome: '独立悬浮', hint: '目标高亮', discovering: '识别中', reliable: '唯一可靠工程',
  multi: '多候选', weak: '单个弱候选', none: '无候选', trust: '等待信任', sdkMissing: 'SDK 缺失',
  building: '构建中', waiting: '等待首帧', running: '测试画面运行', stale: '旧画面 / 构建失败',
  recovery: '错误恢复', diagnostics: '诊断记录',
  pairUnconfirmed: '共同布局未确认', pairMaximizing: '共同最大化待确认',
  pairMaximized: '共同最大化已确认', pairRestoring: '共同布局恢复中', pairBlocked: '共同布局受限',
} as const;
export type Scenario = keyof typeof scenarios;
const stamp = Date.parse('2026-10-09T02:24:36Z');
const project = {
  id: 'demo-project', name: 'native-counter', environment: 'windowsLocal' as const,
  workspaceRoot: 'D:\\Projects\\Harmony\\native-counter', appRoot: 'D:\\Projects\\Harmony\\native-counter',
  module: 'entry', variant: 'default', sdkId: null,
};
const target = { id: 'demo-target', title: 'native-counter — Windows Terminal', app: '终端', process: { pid: 4200, startedAt: 'demo-process-lifecycle' } };
function candidate(index: number, reliable = false): Candidate {
  const names = ['native-counter', 'harmony-store', 'quick-action', 'feature-lab', '中文长名称组件工程', 'terminal-preview'];
  const name = names[index % names.length];
  // Mirrors `discovery::classification`: only lifecycle-verified active-scope
  // evidence makes a candidate `current`, and history evidence can never do so.
  // The backend owns this field, so the fixture has to mirror the same rules.
  const source = reliable ? 'registration' : index === 1 ? 'history' : 'title';
  const category = reliable ? 'current' : source === 'history' ? 'history' : 'suggested';
  // `categoryExpiresAt` only schedules a change where the category really is time
  // bound: the verified current candidate stops being `current` when its evidence
  // expires. Suggested and history rows have no scheduled category change.
  const evidenceExpiresAt = stamp + 60_000;
  return {
    id: `demo-candidate-${index}`, project: { ...project, id: `demo-project-${index}`, name, workspaceRoot: `D:\\Projects\\Harmony\\${name}`, appRoot: `D:\\Projects\\Harmony\\${name}`, module: index === 4 ? 'feature' : 'entry' },
    evidence: [{ source, targetId: target.id, process: target.process, sessionId: reliable ? 'demo-source-session' : null, observedAt: stamp, expiresAt: evidenceExpiresAt, environment: 'windowsLocal', rawPath: `D:\\Projects\\Harmony\\${name}`, verifiable: reliable, activeScope: reliable }],
    valid: index !== 5, autoEligible: reliable, category,
    categoryExpiresAt: category === 'current' ? evidenceExpiresAt : 0,
    reasons: [reliable ? '来自当前会话；生命周期与工程结构已核验' : index === 5 ? '目录不可访问；需要重新选择有效工程' : index === 1 ? '曾关联此窗口；当前活动工程仍需确认' : '路径待确认；窗口标题不能证明当前工程'],
  };
}
function base(revision: number): AppSnapshot {
  return {
    revision, phase: 'idle', target: null, project: null, candidates: [], workspaceObservation: null, trusted: false, sessionId: null,
    backend: { id: 'ui-fixture', mode: 'UI 演示 · 非原生渲染', available: false, reason: 'UI 演示数据，不能证明 Windows 贴边或 HarmonyOS 原生预览。', sdkPath: null, sdkVersion: null, canClick: false, canSwipe: false, canText: false, canRotate: false },
    build: { generation: 0, displayedGeneration: null, buildId: null, durationMs: null, error: null, stale: false },
    diagnostics: [], dockHint: null, layoutRequired: false, sessionRef: null, pairLayout: { state: 'unbound', preferredWidthDip: 440, reason: null },
    display: { mode: 'expanded', requestId: 0, orbAutoHide: false, orbOpacity: 100, trayAvailable: false, reason: 'UI 演示不创建悬浮球或系统托盘。' },
    motion: { reduced: false, intensity: 'standard' }, dockConfirmation: null,
  };
}
function snapshot(scenario: Scenario, revision: number): AppSnapshot {
  const value = base(revision);
  if (scenario === 'welcome') return value;
  if (scenario === 'hint') return { ...value, dockHint: 'Release to bind Windows Terminal' };
  value.target = { ...target }; value.pairLayout = { state: 'paired', preferredWidthDip: 440, reason: null };
  if (scenario === 'discovering') return { ...value, phase: 'discovering' };
  if (scenario === 'multi' || scenario === 'weak' || scenario === 'none') return { ...value, phase: 'selectProject', candidates: scenario === 'none' ? [] : scenario === 'weak' ? [candidate(0)] : Array.from({ length: 6 }, (_, index) => candidate(index)) };
  value.project = { ...project };
  value.sessionId = `demo-session-${revision}`;
  value.sessionRef = { id: value.sessionId, project: value.project, target: value.target, bindingEvidence: [], sdkId: null };
  if (scenario.startsWith('pair')) {
    const pairStates: Record<string, PairLayoutState> = { pairMaximizing: 'maximizing', pairMaximized: 'maximized', pairRestoring: 'restoring', pairBlocked: 'blocked' };
    return { ...value, trusted: true, phase: 'sdkMissing', pairLayout: scenario === 'pairUnconfirmed' ? null : {
      state: pairStates[scenario], preferredWidthDip: 440,
      reason: scenario === 'pairBlocked' ? 'UI 测试：目标窗口权限高于 ArkDock，不能调整共同布局。' : 'UI 演示只验证布局状态；不移动或缩放任何系统窗口。',
    } };
  }
  if (scenario === 'trust' || scenario === 'reliable') return { ...value, phase: 'untrusted', candidates: scenario === 'reliable' ? [candidate(0, true)] : [] };
  value.trusted = true;
  if (scenario === 'sdkMissing') return { ...value, phase: 'sdkMissing', backend: { ...value.backend, reason: '未找到合法 HarmonyOS SDK；请在桌面环境指定 SDK 目录。' }, diagnostics: [{ code: 'SDK_MISSING', message: '合法 SDK 未配置；原生门禁 BLOCKED。此场景为 UI 测试数据。', status: 'BLOCKED', observedAt: stamp, workspaceId: project.id, sessionId: value.sessionId, buildId: null, backendId: 'ui-fixture' }] };
  value.backend = { ...value.backend, available: true, canClick: true, reason: 'UI 演示 · 仅测试点击坐标与状态；此画面不是 ArkUI。', sdkVersion: 'UI fixture', sdkPath: null };
  value.build = { generation: 1, displayedGeneration: null, buildId: 'demo-build-1', durationMs: scenario === 'building' ? null : 1380, error: null, stale: false };
  value.phase = scenario === 'building' ? 'building' : 'running';
  if (scenario === 'running' || scenario === 'recovery') value.build.displayedGeneration = 1;
  if (scenario === 'recovery') value.diagnostics = [{ code: 'BUILD_RECOVERED', message: '测试事件：修复后当前构建重新收到有效画面。', status: 'PASS', observedAt: stamp, workspaceId: project.id, sessionId: value.sessionId, buildId: value.build.buildId, backendId: 'ui-fixture' }];
  if (scenario === 'stale' || scenario === 'diagnostics') {
    value.phase = 'stale';
    value.build = { ...value.build, generation: 2, displayedGeneration: 1, buildId: 'demo-build-2', error: '测试构建失败：组件引用不存在。修复后可重新刷新。', stale: true, durationMs: 760 };
    value.diagnostics = [{ code: 'BUILD_FAILED', message: value.build.error!, status: 'FAIL', observedAt: stamp, workspaceId: project.id, sessionId: value.sessionId, buildId: value.build.buildId, backendId: 'ui-fixture' }, { code: 'INPUT_BLOCKED', message: '已保留最后成功测试画面；当前失败画面禁止输入。', status: 'BLOCKED', observedAt: stamp, workspaceId: project.id, sessionId: value.sessionId, buildId: value.build.buildId, backendId: 'ui-fixture' }];
  }
  return value;
}

// A labelled calibration image, intentionally unrelated to the example accounting app.
function calibrationPng(): string {
  const canvas = document.createElement('canvas'); canvas.width = 360; canvas.height = 640;
  const context = canvas.getContext('2d');
  if (!context) throw new Error('测试画面无法创建 Canvas 2D；没有原生回退。');
  context.fillStyle = '#0b1220'; context.fillRect(0, 0, 360, 640);
  context.strokeStyle = '#223653'; context.lineWidth = 1;
  for (let x = 0; x <= 360; x += 40) { context.beginPath(); context.moveTo(x, 0); context.lineTo(x, 640); context.stroke(); }
  for (let y = 0; y <= 640; y += 40) { context.beginPath(); context.moveTo(0, y); context.lineTo(360, y); context.stroke(); }
  context.fillStyle = '#4b82ff'; context.fillRect(20, 20, 320, 58);
  context.textAlign = 'center'; context.font = 'bold 20px Segoe UI, Microsoft YaHei UI'; context.fillStyle = '#ffffff'; context.fillText('UI 测试画面', 180, 55);
  context.font = '16px Segoe UI, Microsoft YaHei UI'; context.fillStyle = '#a5b4c9'; context.fillText('非 ArkUI · 非原生渲染', 180, 116);
  context.strokeStyle = '#78b8ff'; context.lineWidth = 2;
  context.beginPath(); context.arc(180, 320, 96, 0, Math.PI * 2); context.stroke();
  context.beginPath(); context.moveTo(60, 320); context.lineTo(300, 320); context.moveTo(180, 200); context.lineTo(180, 440); context.stroke();
  context.fillStyle = '#eaf1fb'; context.font = '18px Consolas, monospace'; context.fillText('360 × 640', 180, 484);
  context.font = '15px Segoe UI, Microsoft YaHei UI'; context.fillStyle = '#a5b4c9'; context.fillText('坐标、缩放与状态校准', 180, 530);
  context.fillText('原生验收需合法 SDK 和真实工程', 180, 588);
  return canvas.toDataURL('image/png').split(',')[1];
}

export interface FixtureDriver {
  bridge: UiBridge;
  calls: Array<{ command: keyof CommandArgs; args: object; scope: RequestScope | undefined }>;
  setScenario(scenario: Scenario): void;
  emitCurrentFrame(): void;
  emitOldState(): void;
  emitOldFrame(): void;
  switchTarget(): void;
  current(): AppSnapshot;
  deferNext(command: keyof CommandArgs, mode: 'command' | 'response'): void;
  releaseDeferred(): void;
  hasDeferred(): boolean;
  emitPairLayout(state: PairLayoutState | null, reason?: string): void;
  emitDockConfirmation(bindingId: string | null, side?: 'left' | 'right'): void;
}
export function createFixtureDriver(initial: Scenario): FixtureDriver {
  let revision = 1; let current = snapshot(initial, revision); let previous: AppSnapshot | null = null; let oldFrame: NativeFrame | null = null; let initialPending = true;
  const png = calibrationPng(); let sequence = 0;
  const subscriptions = new Set<{ state: (state: AppSnapshot) => void; frame: (frame: NativeFrame) => void }>();
  const calls: FixtureDriver['calls'] = [];
  let queued: { command: keyof CommandArgs; mode: 'command' | 'response' } | null = null;
  let deferred: (() => void) | null = null;
  const copy = (value: AppSnapshot) => structuredClone(value);
  const emitState = () => { for (const listener of subscriptions) listener.state(copy(current)); };
  const frameFor = (state: AppSnapshot): NativeFrame => ({ sessionId: state.sessionId!, buildId: state.build.buildId!, generation: state.build.generation, sequence: ++sequence, width: 360, height: 640, capturedAt: stamp, rotation: 0, pngBase64: png });
  const emitFrame = () => { if (!current.sessionId || !current.build.buildId) return; const frame = frameFor(current); oldFrame = frame; for (const listener of subscriptions) listener.frame(frame); };
  const publish = (next: AppSnapshot) => { previous = current; current = next; emitState(); };
  function load(scenario: Scenario) {
    const next = snapshot(scenario, ++revision);
    if (scenario === 'stale' || scenario === 'diagnostics') {
      const good = { ...next, phase: 'running' as const, build: { ...next.build, buildId: 'demo-build-1', generation: 1, error: null, stale: false }, diagnostics: [] };
      publish(good);
      // Preserve a real decoded/displayed fixture bitmap before emitting build failure.
      // A synchronous good/stale burst can invalidate the frame while createImageBitmap awaits.
      const presented = (event: Event) => {
        const frame = (event as CustomEvent<{ sessionId: string; buildId: string }>).detail;
        if (frame?.sessionId !== good.sessionId || frame.buildId !== good.build.buildId) return;
        window.removeEventListener('arkdock:frame-presented', presented);
        if (current.target?.id !== good.target?.id || current.sessionId !== good.sessionId || current.build.buildId !== good.build.buildId || current.phase !== 'running') return;
        current = { ...current, phase: next.phase, build: next.build, diagnostics: next.diagnostics, revision: ++revision }; emitState();
      };
      window.addEventListener('arkdock:frame-presented', presented);
      emitFrame();
    } else { publish(next); if (scenario === 'running' || scenario === 'recovery') emitFrame(); }
  }
  async function perform<K extends keyof CommandArgs>(command: K, args: Omit<CommandArgs[K], 'scope'>, scope?: RequestScope): Promise<AppSnapshot> {
      if (command === 'snapshot') {
        if (initialPending && (initial === 'stale' || initial === 'diagnostics')) load(initial);
        else { emitState(); if (initialPending && (initial === 'running' || initial === 'recovery')) emitFrame(); }
        initialPending = false; return copy(current);
      }
      if (scope?.targetId !== (current.target?.id ?? null) || scope?.sessionId !== current.sessionId) throw new Error('STALE_REQUEST: UI 测试请求属于已失效的绑定会话。');
      const name: string = command;
      if (command === 'set_motion_preferences') {
        publish({ ...current, revision: ++revision, motion: (args as Omit<CommandArgs['set_motion_preferences'], 'scope'>).preferences });
        return copy(current);
      }
      if (command === 'get_display_mode') return copy(current);
      if (command === 'set_orb_auto_hide') {
        publish({ ...current, revision: ++revision, display: { ...current.display, orbAutoHide: (args as Omit<CommandArgs['set_orb_auto_hide'], 'scope'>).enabled } });
        return copy(current);
      }
      if (command === 'set_orb_opacity') {
        publish({ ...current, revision: ++revision, display: { ...current.display, orbOpacity: (args as Omit<CommandArgs['set_orb_opacity'], 'scope'>).opacity } });
        return copy(current);
      }
      if (['collapse_to_orb', 'expand_from_orb', 'hide_to_tray', 'show_from_tray'].includes(command)) throw new Error('UI 演示不能创建悬浮球或托盘；请在 Windows 桌面验证。');
      if (name === 'minimize_window' || name === 'close_window') throw new Error('UI 演示不能操作系统窗口；请使用 Windows 桌面运行验证。');
      if (name === 'click_preview') {
        if (current.phase !== 'running' || current.build.stale) throw new Error('UI 测试：过期画面禁止输入。');
        return copy(current);
      }
      if (name === 'maximize_pair' || name === 'restore_pair') publish({ ...current, revision: ++revision, pairLayout: { state: name === 'maximize_pair' ? 'maximized' : 'paired', preferredWidthDip: 440, reason: 'UI 演示仅验证共同布局状态，不操作系统窗口。' } });
      else if (name === 'detach') publish(base(++revision));
      else if (name === 'rediscover') publish({ ...current, revision: ++revision, phase: 'discovering', candidates: [], project: null, sessionId: null, trusted: false, sessionRef: null });
      else if (name === 'begin_drag') publish({ ...current, revision: ++revision, dockHint: 'Release to bind Windows Terminal' });
      else if (name === 'select_candidate') {
        const chosen = current.candidates.find((item) => item.id === (args as unknown as { candidateId: string }).candidateId);
        if (!chosen?.valid) throw new Error('UI 测试：候选不可访问。');
        const sessionId = `demo-session-${++revision}`;
        publish({ ...current, revision, phase: 'untrusted', project: chosen.project, sessionId, trusted: false, sessionRef: { id: sessionId, project: chosen.project, target: current.target!, bindingEvidence: chosen.evidence, sdkId: null } });
      } else if (name === 'set_trust') {
        const trusted = (args as unknown as { trusted: boolean }).trusted;
        publish({ ...current, revision: ++revision, trusted, phase: trusted ? 'sdkMissing' : 'untrusted', backend: { ...current.backend, available: false, reason: '演示无合法 SDK；确认信任后仍需真实工具链。' } });
      } else if (name === 'refresh_preview') publish({ ...current, revision: ++revision, phase: 'building', build: { ...current.build, generation: current.build.generation + 1, buildId: `demo-build-${current.build.generation + 1}`, stale: false, error: null, durationMs: null } });
      else if (name === 'stop_preview') publish({ ...current, revision: ++revision, phase: 'stopped', backend: { ...current.backend, canClick: false } });
      else if (name === 'configure_sdk' || name === 'select_directory') throw new Error('UI 演示不打开系统目录对话框；请在桌面环境选择真实路径。');
      else if (name === 'arrange_side_by_side') throw new Error('UI 演示不能改变目标窗口布局。');
      else throw new Error(`UI 演示没有实现 ${name}；不会伪装执行成功。`);
    return copy(current);
  }
  function hold(complete: () => Promise<AppSnapshot>): Promise<AppSnapshot> {
    return new Promise((resolve, reject) => { deferred = () => { void complete().then(resolve, reject); }; });
  }
  const bridge: UiBridge = {
    available: true,
    async request<K extends keyof CommandArgs>(command: K, args: Omit<CommandArgs[K], 'scope'>, scope?: RequestScope) {
      calls.push({ command, args: structuredClone(args), scope });
      const mode = queued?.command === command ? queued.mode : null;
      if (mode) queued = null;
      if (mode === 'command') return hold(() => perform(command, args, scope));
      const result = await perform(command, args, scope);
      if (mode === 'response') return hold(async () => result);
      return result;
    },
    async observe(state, frame) { const listener = { state, frame }; subscriptions.add(listener); return () => { subscriptions.delete(listener); }; },
  };
  return {
    bridge, calls, setScenario: load, current: () => copy(current), emitCurrentFrame: emitFrame,
    emitOldState: () => { if (previous) for (const listener of subscriptions) listener.state(copy(previous)); },
    emitOldFrame: () => { if (oldFrame) for (const listener of subscriptions) listener.frame({ ...oldFrame, sequence: ++sequence }); },
    switchTarget: () => publish({ ...base(++revision), phase: 'discovering', target: { ...target, id: `demo-target-${revision}`, title: '第二个测试目标 — 终端' } }),
    deferNext: (command, mode) => { if (queued || deferred) throw new Error('测试已有待释放请求。'); queued = { command, mode }; },
    hasDeferred: () => deferred !== null,
    releaseDeferred: () => { if (!deferred) throw new Error('没有待释放测试请求。'); const release = deferred; deferred = null; release(); },
    emitPairLayout: (state, reason = 'UI 演示布局事件；不操作系统窗口。') => publish({ ...current, revision: ++revision, pairLayout: state === null ? null : { state, preferredWidthDip: 440, reason } }),
    emitDockConfirmation: (bindingId, side = 'right') => publish({ ...current, revision: ++revision, dockConfirmation: bindingId ? { bindingId, side } : null }),
  };
}
