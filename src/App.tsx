import { memo, useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import type { AppSnapshot, Candidate, CommandArgs, Evidence, InteractionMode } from './ipc/model.ts';
import { nativeBridge } from './ipc/client.ts';
import type { UiBridge } from './ipc/client.ts';
import { applySnapshot, deriveUiState, filterCandidates } from './view-state.ts';
import { createFrameStore } from './frame-store.ts';
import type { FrameStore } from './frame-store.ts';
import { BinaryFrameController } from './preview/BinaryFrameController.ts';
import type { PreviewFrame } from './preview/frame-types.ts';
import { InspectorActionsBar, InspectorToolbar } from './inspector/InspectorToolbar.tsx';
import { InspectorPreview } from './inspector/InspectorPreview.tsx';
import type { InspectorActions } from './inspector/InspectorPreview.tsx';
import { PreviewCapabilities } from './PreviewCapabilities.tsx';
import { Icon } from './Icon.tsx';
import { Sheet } from './Sheet.tsx';
import { applyTheme, readTheme } from './theme.ts';
import type { Theme } from './theme.ts';
import { applyMotion, createMotionPreferenceSync, motionSettings, readMotion, readMotionIntensity } from './motion.ts';
import type { MotionPreference } from './motion.ts';
import { RuntimeLogs } from './RuntimeLogs.tsx';
import { PreviewChrome } from './PreviewChrome.tsx';
import { WorkspaceStrip } from './WorkspaceStrip.tsx';
import { displayPath } from './path-display.ts';
import { hasDockCandidate, responseStillCurrent, sameTarget } from './request-state.ts';
import { pairControl } from './pair-layout.ts';
import { DockRipple } from './DockRipple.tsx';
import { targetLifecycleKey } from './dock-ripple.ts';
import { displayTransitioning, nextDisplayRequestId } from './display-state.ts';
import { createOrbAutoHidePreferenceSync, nativeOrbPreferenceChanged, readOrbAutoHide, saveOrbAutoHide, settledOrbChoice } from './orb-preferences.ts';
import { useNativeControls } from './preview/useNativeControls.ts';
import { PreviewControlToolbar, PreviewPageSelect } from './preview/PreviewControlToolbar.tsx';
import { DeviceRuntimePanel } from './preview/DeviceRuntimePanel.tsx';
import './preview/preview-controls.css';

type Panel = 'projects' | 'sdk' | 'trust' | 'path' | 'settings' | 'menu' | 'controls' | 'diagnostics' | 'help' | null;
const environments = { windowsLocal: 'Windows 本地', wsl: 'WSL · 预览未支持', remote: '远程 · 预览未支持' };
const sources = { hostWorkspace: '宿主工作区接口', codexProjectMetadata: 'Codex 项目元数据', registration: '已登记工程线索', uia: '窗口标签线索', processArguments: '进程启动参数', history: '历史关联记录', title: '窗口标题 · 路径待确认', manual: '手动指定目录' };
const panelNames = { projects: '选择关联工程', sdk: '设备与原生工具链', trust: '工程信任', path: '工程身份与路径', settings: '偏好设置', menu: '更多操作', controls: '预览能力', diagnostics: '运行日志', help: '使用帮助' };
type CandidateCategory = 'current' | 'suggested' | 'manual' | 'history';
const categoryNames: Record<CandidateCategory, string> = { current: '当前工作区工程候选', suggested: '建议候选 · 需要确认', manual: '指定目录 · 验证结果', history: '历史工程 · 不代表当前工作区' };


function previewStateKey(state: AppSnapshot | null): string {
  // Diagnostic/revision-only snapshots do not redraw the preview component tree.
  // Every field used by its scope, trust, capabilities and stale-frame gates remains included.
  return JSON.stringify(state ? [state.phase, state.sessionId, state.target, state.project, state.trusted, state.build, state.backend] : null);
}
const StableInspectorPreview = memo(InspectorPreview, (previous, next) => previous.bridge === next.bridge
  && previous.store === next.store && previous.visible === next.visible && previous.zoom === next.zoom
  && previous.busy === next.busy && previous.interactionMode === next.interactionMode
  && previous.controlState === next.controlState && previous.nativeInput === next.nativeInput && previous.inputPaused === next.inputPaused
  && previous.inputError === next.inputError
  && previous.exit === next.exit && previous.onClick === next.onClick
  && previous.actions === next.actions && previous.onProblem === next.onProblem && previous.onError === next.onError
  && previewStateKey(previous.snapshot) === previewStateKey(next.snapshot));

function currentEvidence(evidence: Evidence, snapshot: AppSnapshot | null, now: number): boolean {
  const target = snapshot?.target;
  return Boolean(target && evidence.source !== 'history' && evidence.source !== 'manual'
    && evidence.targetId === target.id && evidence.process?.pid === target.process.pid
    && evidence.process.startedAt === target.process.startedAt
    && evidence.observedAt <= now && now < evidence.expiresAt
    && evidence.expiresAt > evidence.observedAt && evidence.expiresAt - evidence.observedAt <= 120_000);
}

function candidateCategory(candidate: Candidate, snapshot: AppSnapshot | null, now: number): CandidateCategory {
  const current = candidate.evidence.filter((evidence) => currentEvidence(evidence, snapshot, now));
  if ((!snapshot?.workspaceObservation || (snapshot.workspaceObservation.status === 'verified'
    && snapshot.workspaceObservation.observedAt <= now && now - snapshot.workspaceObservation.observedAt < 15_000))
    && current.some((evidence) => ['hostWorkspace', 'codexProjectMetadata', 'registration', 'uia', 'processArguments'].includes(evidence.source)
    && evidence.verifiable && evidence.activeScope && evidence.environment === 'windowsLocal'
    && candidate.project.environment === 'windowsLocal'
    && Boolean(evidence.sessionId && evidence.sessionId.length <= 128))) return 'current';
  if (current.length) return 'suggested';
  if (candidate.evidence.some((evidence) => evidence.source === 'manual')) return 'manual';
  return candidate.evidence.some((evidence) => evidence.source === 'history') ? 'history' : 'suggested';
}

function CandidateItem({ candidate, category, select, busy }: { candidate: Candidate; category: CandidateCategory; select: (id: string) => void; busy: boolean }) {
  const reliable = category === 'current' && candidate.autoEligible;
  return <li className={`candidate ${reliable ? 'reliable' : ''}`}><button data-candidate-id={candidate.id} disabled={busy || !candidate.valid || candidate.project.environment !== 'windowsLocal'} onClick={() => select(candidate.id)}>
    <span className="candidate-heading"><span className="candidate-icon"><Icon name="folder" /></span><strong>{candidate.project.name}</strong><span className={`badge ${candidate.valid ? '' : 'invalid'}`}>{candidate.valid ? category === 'history' ? '历史工程' : reliable ? '证据可靠' : '需要确认' : '目录不可用'}</span><Icon name="chevron" size={16} /></span>
    <code className="path">{displayPath(candidate.project.appRoot)}</code><span>{environments[candidate.project.environment]} · {candidate.project.module} · {candidate.project.variant}</span><span>工作区：{displayPath(candidate.project.workspaceRoot)}</span>
    <span className="evidence-tags">{Array.from(new Set(candidate.evidence.map((item) => sources[item.source]))).map((source) => <span key={source}>{source}</span>)}</span><span className="candidate-reason">{category === 'history' ? '这是之前关联过的工程；当前窗口的活动工作区尚需另行核验。' : category === 'manual' && candidate.valid ? '指定目录已执行只读检查。确认这是当前工程后，点击此项关联。' : candidate.reasons.join('；') || '请选择与目标窗口对应的实际工程。'}</span>
  </button></li>;
}

function EmptyState({ snapshot, store, connected, navigate }: { snapshot: AppSnapshot | null; store: FrameStore; connected: boolean; navigate: (panel: Panel) => void }) {
  const frame = useSyncExternalStore(store.subscribe, store.getSnapshot);
  if (frame) return null;
  const ui = deriveUiState(snapshot, null);
  const loading = ui.workspace === 'discovering' || ui.preview === 'building' || ui.preview === 'waitingFrame';
  const welcome = !snapshot?.target;
  const title = welcome ? hasDockCandidate(snapshot?.dockHint) ? '松开以连接' : '拖到开发窗口旁开始' : ui.workspace === 'discovering' ? '正在识别当前工程' : ui.workspace === 'untrusted' ? '让信任先于构建' : ui.preview === 'building' ? '正在构建实际工程' : ui.preview === 'waitingFrame' ? '等待原生首帧' : snapshot?.phase === 'crashed' ? '预览进程已退出' : snapshot?.phase === 'stopped' ? '预览已停止' : !snapshot?.project ? '选择当前鸿蒙工程' : '原生预览暂不可用';
  return <div className={`empty ${welcome ? 'welcome-state' : ''} ${loading ? 'loading-state' : ''}`}>
    {ui.workspace === 'discovering' ? <div className="scanner-orbit" aria-hidden="true"><span /><span /><span /><div><Icon name="folder" size={34} /></div></div> : <div className="welcome-art" aria-hidden="true"><div className="art-window"><span /><span /><span /><div /><div /><div /></div><div className="art-device"><span className="mark"><span /><span /></span></div><div className="art-link"><Icon name="link" size={18} /></div></div>}
    {loading && <span className="task-spinner" aria-label="任务进行中" />}<span className="eyebrow">{welcome ? '你的鸿蒙预览伴侣' : '实际工程 · 原生运行时'}</span><h1>{title}</h1>
    <p>{welcome ? '按住顶部拖动区，靠近目标窗口右侧。\n松开后关联工程，预览随代码更新。' : ui.preview === 'waitingFrame' ? '构建已完成，原生运行时尚未返回有效画面。' : ui.preview === 'building' ? '构建工具正在处理当前代码；完成后等待有效首帧。' : ui.workspace === 'discovering' ? '只读检查当前窗口的工程线索。\n结果就绪后立即继续。' : snapshot?.project ? ui.workspace === 'untrusted' ? '关联只读；你确认后才允许执行工程构建逻辑。' : snapshot.backend.reason : '工程线索不足时需要确认，路径不会根据应用名称生成。'}</p>
    {!connected && <div className="bridge-notice"><Icon name="monitor" size={16} /><div><strong>桌面桥接未连接</strong><span>窗口联动与原生预览需在 Windows 桌面运行</span><code>pnpm dev</code></div></div>}
    {connected && !welcome && !loading && <button className="primary-button" onClick={() => navigate(ui.workspace === 'untrusted' ? 'trust' : !snapshot?.project ? 'projects' : snapshot?.phase === 'stopped' ? 'controls' : 'sdk')}>{ui.workspace === 'untrusted' ? '确认工程信任' : !snapshot?.project ? '查看工程候选' : snapshot.phase === 'stopped' ? '查看预览控制' : '检查原生工具链'}<Icon name="arrow" size={16} /></button>}
    {welcome && <button className="text-button" onClick={() => navigate('help')}>了解拖动与工程识别<Icon name="chevron" size={14} /></button>}{welcome ? <div className="welcome-features"><span><Icon name="folder" size={15} />只读发现工程线索</span><span><Icon name="shield" size={15} />执行前确认工程信任</span><span><Icon name="preview" size={15} />接入真实原生画面</span></div> : <div className="welcome-footnote"><Icon name="shield" size={14} /><span>独立窗口 · 只读发现 · 执行前确认信任</span></div>}
  </div>;
}

function BuildStatusBar({ snapshot, store, pending, open, compact = false }: { snapshot: AppSnapshot | null; store: FrameStore; pending: string | null; open?: () => void; compact?: boolean }) {
  const frame = useSyncExternalStore(store.subscribeDisplayed, store.getDisplayed);
  const ui = deriveUiState(snapshot, frame);
  const failures = snapshot?.diagnostics.filter((item) => item.status === 'FAIL').length;
  const blocked = snapshot?.diagnostics.filter((item) => item.status === 'BLOCKED').length;
  const failed = ui.preview === 'error' || Boolean(snapshot?.build.error) || snapshot?.phase === 'crashed';
  const warning = !failed && (ui.preview === 'stale' || ui.preview === 'building' || ui.preview === 'waitingFrame' || Boolean(snapshot?.project && (!snapshot.trusted || !snapshot.backend.available)));
  const indicator = failed ? 'error' : warning ? 'warning' : ui.preview === 'running' ? 'live' : '';
  const label = pending ? '请求处理中…' : ui.previewLabel;
  if (compact) return <span className={`preview-status-indicator ${indicator}`} title={snapshot?.build.error ?? label}>
    <span className={`dot ${indicator}`} aria-hidden="true" /><span aria-live="polite">{ui.preview === 'running' && !pending ? '预览运行' : label}</span>
  </span>;
  return <footer className="footer" data-preview={ui.preview}><button className="status-summary" onClick={open} aria-label="打开运行日志" title="查看构建、窗口与工程诊断"><span className={`dot ${indicator}`} /><span className="status-copy"><strong aria-live="polite">{label}</strong><span>{frame ? `画面 v${frame.generation} · ${new Date(frame.capturedAt).toLocaleTimeString('zh-CN')}` : snapshot?.build.buildId ? `代码 v${snapshot.build.generation} · ${snapshot.build.durationMs == null ? '耗时未知' : `${snapshot.build.durationMs} ms`}` : '尚无原生画面'}</span></span><span className="diagnostic-count">{failures == null ? '诊断未知' : `历史 ${failures} 失败 · ${blocked} 阻塞`}</span><Icon name="log" size={18} /></button></footer>;
}

export function App({ bridge = nativeBridge }: { bridge?: UiBridge } = {}) {
  const [state, setState] = useState<AppSnapshot | null>(null);
  const stateRef = useRef<AppSnapshot | null>(null);
  const [store] = useState(createFrameStore);
  const frameController = useRef<BinaryFrameController | null>(null);
  const [interactionMode, setInteractionMode] = useState<InteractionMode>('interact');
  const inspectorActions = useRef<InspectorActions | null>(null);
  const [inspectorProblem, setInspectorProblem] = useState<string | null>(null);
  const [runtimeKind, setRuntimeKind] = useState<'native' | 'emulator' | 'physicalDevice'>('native');
  const inputPending = useRef(false);
  const [previewVisible, setPreviewVisible] = useState(true);
  const previewVisibleRef = useRef(true);
  const [panel, setPanel] = useState<Panel>(null);
  const panelAnchor = useRef<HTMLElement | null>(null);
  const navigationRevision = useRef(0);
  const automaticPanels = useRef<{ projects: string | null; trust: string | null }>({ projects: null, trust: null });
  const [search, setSearch] = useState('');
  const [directoryPath, setDirectoryPath] = useState('');
  const [observationTime, setObservationTime] = useState(Date.now);
  const [error, setError] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<string | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  const activeCommand = useRef<string | null>(null);
  const pendingRef = useRef(false);
  const cancelRequested = useRef(false);
  const [cancelling, setCancelling] = useState(false);
  const [layoutConsent, setLayoutConsent] = useState(false);
  const [zoom, setZoom] = useState<number | 'fit'>('fit');
  const [theme, setTheme] = useState<Theme>(readTheme);
  const [motion, setMotion] = useState<MotionPreference>(readMotion);
  const [intensity, setIntensity] = useState(readMotionIntensity);
  const [systemReduced, setSystemReduced] = useState(() => window.matchMedia('(prefers-reduced-motion: reduce)').matches);
  const [nativeReady, setNativeReady] = useState(false);
  const [orbAutoHide, setOrbAutoHide] = useState(readOrbAutoHide);
  const orbChoice = useRef(orbAutoHide);
  const orbBootstrap = useRef(false);
  const nativeReadyRef = useRef(false);
  const preferenceSync = useRef<ReturnType<typeof createMotionPreferenceSync<AppSnapshot>> | null>(null);
  const orbPreferenceSync = useRef<ReturnType<typeof createOrbAutoHidePreferenceSync<AppSnapshot>> | null>(null);
  const displayRequestId = useRef(0);
  const connected = bridge.available;
  const effectiveMotion = motionSettings(motion, intensity, systemReduced);
  const motionTargetKey = targetLifecycleKey(state);
  const ui = deriveUiState(state, null);
  const pair = pairControl(state);
  const controls = useNativeControls({ bridge, snapshot: state, store,
    visible: previewVisible && !panel && runtimeKind === 'native' && interactionMode === 'interact',
    windowVisible: previewVisible && runtimeKind === 'native', wake: () => frameController.current?.wake() });
  const nativeInput = useMemo(() => ({ start: controls.start, move: controls.input.move.bind(controls.input),
    end: controls.input.end.bind(controls.input), cancel: controls.cancelInput, text: controls.text, key: controls.key }), [controls.input]);
  const receive = useCallback((value: AppSnapshot) => {
    const updated = applySnapshot(stateRef.current, value);
    if (updated === stateRef.current) return;
    if (nativeReadyRef.current && !orbBootstrap.current && !orbPreferenceSync.current?.pending()
      && nativeOrbPreferenceChanged(stateRef.current, updated)
      && orbChoice.current !== updated.display.orbAutoHide) {
      // Native context-menu changes are user choices too, once queued UI writes have settled.
      orbChoice.current = updated.display.orbAutoHide;
      setOrbAutoHide(updated.display.orbAutoHide);
    }
    if (!sameTarget(stateRef.current?.target, updated.target)) {
      const lost = Boolean(stateRef.current?.target && !updated.target);
      automaticPanels.current = { projects: null, trust: null };
      setPanel(null); setSearch(''); setDirectoryPath(''); setError(null); setFeedback(null); setLayoutConsent(false);
      if (lost && !cancelRequested.current && activeCommand.current !== 'detach') setFeedback('目标窗口不可用，已解除绑定。');
    }
    if (updated.display.mode !== 'expanded') setPanel(null);
    stateRef.current = updated; store.sync(updated); setState(updated);
    frameController.current?.update(updated);
  }, [store]);
  useEffect(() => {
    let live = true;
    let unsubscribe: (() => void) | undefined;
    nativeReadyRef.current = false;
    setNativeReady(false);
    const controller = bridge.readFrame ? new BinaryFrameController({ bridge: { request: bridge.request, readFrame: bridge.readFrame }, store, getState: () => stateRef.current, onSnapshot: receive, onMode: (_mode, reason) => { if (live && reason) setError(reason); } }) : null;
    frameController.current = controller;
    controller?.update(stateRef.current);
    // WebView2 document.hidden can disagree with the visible owned main window.
    // The native visibility observation controls frame delivery; keyboard input
    // additionally requires local document focus and the current native scope.
    const visibility = () => controller?.setVisible(previewVisibleRef.current);
    visibility(); document.addEventListener('visibilitychange', visibility);
    if (connected) bridge.observe((value) => { if (live) receive(value); }, (frame) => { if (live && stateRef.current) { if (controller) controller.acceptLegacy(frame); else store.push(stateRef.current, frame); } }, (visible) => { if (live && previewVisibleRef.current !== visible) { previewVisibleRef.current = visible; setPreviewVisible(visible); visibility(); } }).then(async (off) => {
      if (!live) { off(); return; }
      unsubscribe = off;
      const snapshot = await bridge.request('snapshot', {});
      if (live) { receive(snapshot); orbBootstrap.current = true; nativeReadyRef.current = true; setNativeReady(true); }
    }).catch((reason: unknown) => { if (live) setError(String(reason)); });
    return () => { live = false; unsubscribe?.(); document.removeEventListener('visibilitychange', visibility); controller?.dispose(); if (frameController.current === controller) frameController.current = null; store.clear(); };
  }, [bridge, connected, store, receive]);
  useEffect(() => { applyTheme(theme); }, [theme]);
  useEffect(() => { applyMotion(motion, intensity, systemReduced); }, [motion, intensity, systemReduced]);
  useEffect(() => {
    const media = window.matchMedia('(prefers-reduced-motion: reduce)');
    const changed = () => setSystemReduced(media.matches);
    media.addEventListener('change', changed);
    return () => media.removeEventListener('change', changed);
  }, []);
  useEffect(() => {
    const sync = createMotionPreferenceSync<AppSnapshot>({
      send: (job) => bridge.request('set_motion_preferences', { preferences: job.preferences }, job.scope),
      accept: (job, result) => {
        if (job.targetKey === targetLifecycleKey(stateRef.current) && job.scope.sessionId === (stateRef.current?.sessionId ?? null)) receive(result);
      },
      fail: (job, reason) => {
        if (job.targetKey === targetLifecycleKey(stateRef.current) && job.scope.sessionId === (stateRef.current?.sessionId ?? null)) setError(String(reason));
      },
    });
    preferenceSync.current = sync;
    return () => { sync.dispose(); if (preferenceSync.current === sync) preferenceSync.current = null; };
  }, [bridge, receive]);
  useEffect(() => {
    if (!connected || !nativeReady) return;
    preferenceSync.current?.enqueue({
      scope: { targetId: stateRef.current?.target?.id ?? null, sessionId: stateRef.current?.sessionId ?? null },
      targetKey: motionTargetKey,
      preferences: motionSettings(motion, intensity, systemReduced),
    });
  }, [connected, nativeReady, motionTargetKey, state?.sessionId, motion, intensity, systemReduced]);
  useEffect(() => {
    const sync = createOrbAutoHidePreferenceSync<AppSnapshot>({
      send: (job) => bridge.request('set_orb_auto_hide', { enabled: job.enabled }, job.scope),
      accept: (job, result) => {
        if (job.targetKey !== targetLifecycleKey(stateRef.current) || job.scope.sessionId !== (stateRef.current?.sessionId ?? null)) return;
        receive(result);
        const confirmed = settledOrbChoice(job, result, stateRef.current);
        if (confirmed !== null && orbChoice.current !== confirmed) {
          // receive may already have accepted a newer native event while this invoke was pending.
          orbChoice.current = confirmed;
          setOrbAutoHide(confirmed);
        }
      },
      fail: (job, reason) => {
        if (job.targetKey === targetLifecycleKey(stateRef.current) && job.scope.sessionId === (stateRef.current?.sessionId ?? null)) setError(String(reason));
      },
    });
    orbPreferenceSync.current = sync;
    return () => { sync.dispose(); if (orbPreferenceSync.current === sync) orbPreferenceSync.current = null; };
  }, [bridge, receive]);
  useEffect(() => {
    if (orbAutoHide !== null) saveOrbAutoHide(orbAutoHide);
  }, [orbAutoHide]);
  useEffect(() => {
    if (!connected || !nativeReady || !stateRef.current) return;
    const enabled = orbAutoHide ?? stateRef.current.display.orbAutoHide;
    orbChoice.current = enabled;
    if (orbAutoHide === null) setOrbAutoHide(enabled);
    orbPreferenceSync.current?.enqueue({
      scope: { targetId: stateRef.current.target?.id ?? null, sessionId: stateRef.current.sessionId },
      targetKey: motionTargetKey,
      enabled,
    });
    orbBootstrap.current = false;
  }, [connected, nativeReady, motionTargetKey, state?.sessionId, orbAutoHide]);
  useEffect(() => {
    if (cancelRequested.current) return;
    const target = state?.target;
    if (!state || !target || state.display.mode !== 'expanded') return;
    const lifecycle = `${target.id}:${target.process.pid}:${target.process.startedAt}`;
    if (state.phase === 'selectProject' && automaticPanels.current.projects !== lifecycle) {
      automaticPanels.current.projects = lifecycle; setPanel('projects');
    } else if (state.phase === 'untrusted' && state.sessionId && automaticPanels.current.trust !== state.sessionId) {
      automaticPanels.current.trust = state.sessionId; setPanel('trust');
    }
  }, [state?.phase, state?.sessionId, state?.target, state?.display.mode]);
  useEffect(() => {
    const now = Date.now();
    const deadlines = (state?.candidates ?? []).flatMap((candidate) => candidate.evidence.map((evidence) => evidence.expiresAt));
    if (state?.workspaceObservation) deadlines.push(state.workspaceObservation.observedAt + 15_000);
    const next = Math.min(...deadlines.filter((deadline) => Number.isFinite(deadline) && deadline > now));
    if (!Number.isFinite(next)) return;
    const timeout = window.setTimeout(() => setObservationTime(Date.now()), next - now + 1);
    return () => window.clearTimeout(timeout);
  }, [state, observationTime]);
  async function act<K extends keyof CommandArgs>(command: K, args: Omit<CommandArgs[K], 'scope'>): Promise<boolean> {
    if (pendingRef.current) return false;
    pendingRef.current = true; activeCommand.current = command; setPending(command); setError(null); setFeedback(null);
    const scope = { targetId: stateRef.current?.target?.id ?? null, sessionId: stateRef.current?.sessionId ?? null };
    const navigation = navigationRevision.current;
    try {
      const result = await bridge.request(command, args, scope);
      receive(result);
      return navigation === navigationRevision.current && responseStillCurrent(result, stateRef.current);
    }
    catch (reason) { if (scope.targetId === (stateRef.current?.target?.id ?? null) && scope.sessionId === (stateRef.current?.sessionId ?? null)) setError(String(reason)); return false; }
    finally { pendingRef.current = false; activeCommand.current = null; setPending(null); }
  }
  function navigate(next: Panel) {
    navigationRevision.current += 1;
    if (!panel && next) panelAnchor.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setPanel(next); setFeedback(null);
  }
  async function cancelAssociation() {
    if (cancelRequested.current) return;
    cancelRequested.current = true; setCancelling(true); setPanel(null); setError(null);
    const target = stateRef.current?.target;
    const scope = { targetId: target?.id ?? null, sessionId: stateRef.current?.sessionId ?? null };
    try {
      let result: AppSnapshot;
      try { result = await bridge.request('detach', {}, scope); }
      catch (reason) {
        // A completed discovery may create a session before queued detach. Retry only this exact target lifecycle.
        if (!String(reason).includes('STALE_REQUEST')) throw reason;
        const latest = await bridge.request('snapshot', {});
        receive(latest);
        if (!sameTarget(target, latest.target)) return;
        result = await bridge.request('detach', {}, { targetId: latest.target?.id ?? null, sessionId: latest.sessionId });
      }
      receive(result);
      if (responseStillCurrent(result, stateRef.current)) setFeedback('关联已取消，窗口已解除绑定。');
    } catch (reason) { if (sameTarget(target, stateRef.current?.target)) setError(String(reason)); }
    finally { cancelRequested.current = false; setCancelling(false); }
  }
  async function select(id: string) {
    const target = stateRef.current?.target?.id;
    if (await act('select_candidate', { candidateId: id }) && stateRef.current?.target?.id === target) setPanel(stateRef.current?.phase === 'untrusted' ? 'trust' : null);
  }
  async function inspectPath() {
    const path = directoryPath.trim();
    if (!path) return;
    if (await act('inspect_directory_path', { path })) {
      setSearch('');
      setFeedback('目录只读验证已完成。请确认对应候选工程后，点击关联。');
    }
  }
  async function changeDisplay(command: 'collapse_to_orb' | 'expand_from_orb' | 'hide_to_tray' | 'show_from_tray') {
    try {
      displayRequestId.current = nextDisplayRequestId(displayRequestId.current, stateRef.current?.display);
      if (await act(command, { requestId: displayRequestId.current })) navigate(null);
    } catch (reason) { setError(String(reason)); }
  }
  function chooseOrbAutoHide(enabled: boolean) {
    orbChoice.current = enabled;
    setOrbAutoHide(enabled);
    if (!connected || !nativeReady || !stateRef.current) return;
    orbPreferenceSync.current?.enqueue({
      scope: { targetId: stateRef.current.target?.id ?? null, sessionId: stateRef.current.sessionId },
      targetKey: targetLifecycleKey(stateRef.current), enabled,
    }, true);
  }
  async function copy(text: string, label: string) {
    try { await navigator.clipboard.writeText(text); setFeedback(`${label}已复制`); }
    catch { setError('当前环境无法访问剪贴板。可在路径或诊断中选中文字复制。'); }
  }
  const exitInspection = useCallback(() => setInteractionMode('interact'), []);
  const clickNative = useCallback(async (frame: PreviewFrame, x: number, y: number) => {
    if (inputPending.current || pendingRef.current || interactionMode !== 'interact') return;
    const actual = stateRef.current;
    if (!deriveUiState(actual, frame).inputAllowed) return;
    const scope = { targetId: actual?.target?.id ?? null, sessionId: actual?.sessionId ?? null };
    inputPending.current = true;
    try { frameController.current?.wake(); receive(await bridge.request('click_preview', { buildId: frame.buildId, generation: frame.generation, x, y }, scope)); }
    catch (reason) { if (scope.targetId === stateRef.current?.target?.id && scope.sessionId === stateRef.current?.sessionId) setError(String(reason)); }
    finally { inputPending.current = false; }
  }, [bridge, receive, interactionMode]);
  const candidates = filterCandidates(state?.candidates ?? [], search);
  const now = Math.max(observationTime, Date.now());
  const grouped = Object.fromEntries((['current', 'suggested', 'manual', 'history'] as const).map((category) => [category,
    candidates.filter((candidate) => candidateCategory(candidate, state, now) === category)])) as Record<CandidateCategory, Candidate[]>;
  const observation = state?.workspaceObservation;
  const observationFresh = Boolean(state?.target && observation && observation.observedAt <= now && now - observation.observedAt < 15_000);
  const activePath = observationFresh && observation?.status === 'verified' ? observation.activePath : null;
  const suggestedName = observationFresh && observation?.status === 'suggested' ? observation.projectName : null;

  const busy = pending !== null;
  const displayBusy = busy || displayTransitioning(state?.display);
  const trayReason = !connected ? '桌面桥接未连接' : state?.display.reason ?? '系统托盘尚未就绪';
  const statusText = !connected ? '桌面未连接' : pending === 'begin_drag' ? '正在连接' : ui.bindingLabel;
  return <main className={`app ${hasDockCandidate(state?.dockHint) ? 'target-near' : ''}`} data-binding={ui.binding} data-workspace={ui.workspace} data-pair-layout={pair.state} data-display={state?.display.mode ?? 'expanded'}>
    <div className="dock-body" {...(panel || state && state.display.mode !== 'expanded' ? { inert: '' } : {})}>
      <header className="header"><button className="drag-zone drag" aria-label="拖动 ArkDock 到目标应用窗口外侧" title="按住顶部，拖到目标窗口右侧" onPointerDown={(event) => { if (event.button === 0 && connected) void act('begin_drag', {}); }} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); setFeedback('键盘移动：Alt+空格打开系统窗口菜单，选择移动。'); } }}><Icon name="grip" size={15} /><span className="mark" aria-hidden="true"><span /><span /></span><strong>ArkDock</strong></button><span className={`header-status ${ui.binding === 'bound' ? 'connected' : ''}`} title={state?.target?.title ?? statusText}><span className="dot" />{statusText}</span><div className="header-actions"><button className="collapse-orb" disabled={!connected || !nativeReady || displayBusy || state?.display.mode !== 'expanded'} aria-label="收起为悬浮球" title={connected ? '收起为悬浮球' : '桌面桥接未连接'} onClick={() => void changeDisplay('collapse_to_orb')}><span className="orb-symbol" aria-hidden="true" /><span className="collapse-label">收起</span></button><button className="icon-button" aria-label="更多操作" title="更多操作" onClick={() => navigate('menu')}><span className="ellipsis" aria-hidden="true">···</span></button><button className="icon-button minimize" disabled={!connected || busy} aria-label="最小化窗口" title={connected ? '最小化窗口' : '桌面桥接未连接'} onClick={() => void act('minimize_window', {})}><span aria-hidden="true">−</span></button><button className="icon-button maximize" disabled={!connected || busy || !pair.enabled} aria-label={pair.action} title={pair.reason} onClick={() => void act(pair.command, {})}><Icon name={pair.state === 'maximized' ? 'restore' : 'maximize'} size={16} /></button><button className="icon-button close-window" disabled={!connected || busy} aria-label="关闭 ArkDock" title={connected ? '关闭 ArkDock' : '桌面桥接未连接'} onClick={() => void act('close_window', {})}><Icon name="close" size={17} /></button></div></header>
      <WorkspaceStrip snapshot={state} busy={busy || !connected} cancelling={cancelling}
        openProject={() => navigate(state?.project ? 'path' : 'projects')} openCandidates={() => navigate('projects')}
        manual={async () => { if (await act('select_directory', {})) navigate(stateRef.current?.phase === 'untrusted' ? 'trust' : null); }}
        rediscover={() => void act('rediscover', {})} cancel={() => void cancelAssociation()} />
      <section className="preview" aria-label={runtimeKind === 'native' ? '原生预览画面' : '设备运行接入'}>
        <PreviewChrome enabled={Boolean(state?.target)} inspecting={runtimeKind === 'native' && interactionMode === 'inspect'} modalOpen={Boolean(panel)}
          toolbar={<><section className="preview-toolbar" aria-label="预览控制">
        <div className="toolbar-selects"><InspectorToolbar mode={interactionMode} enabled={Boolean(runtimeKind === 'native' && state?.phase === 'running' && state.trusted && state.backend.available && !controls.pending && (!controls.state?.experimentalEnabled || controls.state.inputReady))} change={setInteractionMode} />
          <label><span className="sr-only">运行方式</span><select aria-label="运行方式" value={runtimeKind} disabled={busy} title="设备运行与 Previewer 使用独立通道"
            onChange={(event) => { void controls.input.cancel(); setInteractionMode('interact'); setRuntimeKind(event.target.value as typeof runtimeKind); }}>
            <option value="native">原生 Previewer{state?.backend.available ? '' : ' · 待就绪'}</option><option value="emulator">模拟器 · 设备接入</option><option value="physicalDevice">真机 · 设备接入</option></select></label>
          <PreviewPageSelect snapshot={state} store={store} controls={controls} busy={busy || runtimeKind !== 'native'} />
          <button className={`icon-button refresh-button ${pending === 'refresh_preview' ? 'pending' : ''}`} disabled={runtimeKind !== 'native' || ui.capabilities.refresh.status !== 'available' || busy} aria-label="手动刷新" title={ui.capabilities.refresh.reason} onClick={() => void act('refresh_preview', {})}><Icon name="refresh" size={18} /></button></div>
        <div className="toolbar-actions"><button className="stop-button" disabled={runtimeKind !== 'native' || ui.capabilities.stop.status !== 'available' || busy} title={ui.capabilities.stop.reason} aria-label="停止原生预览" onClick={() => void act('stop_preview', {})}><Icon name="stop" size={14} /><span>停止</span></button>
          <label className="zoom-control"><select aria-label="画面缩放" value={zoom} disabled={runtimeKind !== 'native'} onChange={(event) => setZoom(event.target.value === 'fit' ? 'fit' : Number(event.target.value))}><option value="fit">适应窗口</option><option value="0.5">50%</option><option value="0.75">75%</option><option value="1">100%</option><option value="1.5">150%</option></select></label>
          <PreviewControlToolbar snapshot={state} store={store} controls={controls} busy={busy || runtimeKind !== 'native'} /><span className="toolbar-divider" /><span className="mode-caption"><Icon name="device" size={14} />{runtimeKind === 'native' ? '原生' : '设备'}</span>
          <button className="icon-button" aria-label="预览能力" title="查看方向、应用主题和输入能力" onClick={() => navigate('controls')}><Icon name="settings" size={17} /></button></div>
        {runtimeKind === 'native' && interactionMode === 'inspect' && <InspectorActionsBar enabled={Boolean(state?.phase === 'running' && state.trusted && state.backend.available) && !busy && !controls.pending && (!controls.state?.experimentalEnabled || controls.state.inputReady)}
            region={() => inspectorActions.current?.selectRegion()} refresh={() => inspectorActions.current?.refreshTree()} problem={inspectorProblem} />}
      </section><div className="stage-caption"><span><span className={`tiny-dot ${runtimeKind === 'native' && state?.phase === 'running' ? 'live' : ''}`} />{runtimeKind === 'native' ? state?.backend.available ? 'NATIVE PREVIEW' : 'PREVIEW WORKSPACE' : 'DEVICE RUNTIME · BLOCKED'}</span><span>{state?.backend.sdkVersion ? `SDK ${state.backend.sdkVersion}` : '等待工具链'}</span></div></>}
          status={<BuildStatusBar snapshot={state} store={store} pending={pending} open={() => navigate('diagnostics')} />}
          statusIndicator={<BuildStatusBar snapshot={state} store={store} pending={pending} compact />}>
          <div className="native-preview-host" hidden={runtimeKind !== 'native'}>
          <StableInspectorPreview visible={previewVisible && runtimeKind === 'native'} bridge={bridge} interactionMode={interactionMode} exit={exitInspection}
            actions={inspectorActions} onProblem={setInspectorProblem} onError={setError}
            controlState={controls.state} nativeInput={nativeInput} inputPaused={Boolean(controls.pending || panel || runtimeKind !== 'native')} inputError={controls.error}
            snapshot={state} store={store} zoom={zoom} busy={busy} onClick={clickNative} />
          <EmptyState snapshot={state} store={store} connected={connected} navigate={navigate} />
          </div>
          {runtimeKind !== 'native' && <DeviceRuntimePanel snapshot={state} bridge={bridge} kind={runtimeKind} />}
        </PreviewChrome>
      </section>
      {state?.layoutRequired && <aside className="layout-notice"><Icon name="monitor" size={18} /><div><p>右侧空间不足。并排布局会调整目标窗口。</p><label><input type="checkbox" checked={layoutConsent} onChange={(event) => setLayoutConsent(event.target.checked)} />同意调整目标窗口</label></div><button disabled={!layoutConsent || busy} onClick={() => { void act('arrange_side_by_side', { consent: true }); setLayoutConsent(false); }}>应用布局</button></aside>}
    </div>
    {panel && <Sheet key={panel} returnFocus={panelAnchor.current} title={panelNames[panel]} kind={panel === 'diagnostics' ? 'diagnostics' : panel} close={() => navigate(null)}>
      {panel === 'projects' && <>
        <p className="muted">确认与目标窗口对应的实际工程。关联不授予构建权限。</p>
        <section className="workspace-observation" aria-label="当前活动工作区">
          <div className="results-summary"><h3>当前活动工作区</h3><button disabled={!state?.target || busy} onClick={() => void act('rediscover', {})}>{state?.phase === 'discovering' ? '正在识别…' : '重新识别'}</button></div>
          <strong>{activePath ? observation?.projectName ?? '已可靠识别' : '尚未可靠识别'}</strong>
          {activePath && <code className="path">{displayPath(activePath)}</code>}
          {suggestedName && <p className="muted">项目名称线索：{suggestedName}。名称不能证明当前路径或活动归属。</p>}
          <p className="muted">{observationFresh ? observation?.reason : observation ? '活动工作区证据已过期，请重新识别。' : '目标窗口尚未提供可靠的当前工作区路径。'}</p>
          {!activePath && <p className="muted">建议操作：重新识别或选择工程目录。Codex 用户主动打开项目信息后，可再次识别其中可访问的项目元数据。</p>}
        </section>
        <div className="directory-fallback">
          <button className="primary-button" disabled={!connected || busy} onClick={async () => { if (await act('select_directory', {})) navigate(stateRef.current?.phase === 'untrusted' ? 'trust' : null); }}><Icon name="folder" size={16} />手动选择工程目录</button>
          <form className="directory-input" onSubmit={(event) => { event.preventDefault(); void inspectPath(); }}>
            <label htmlFor="project-directory">或输入绝对工程目录</label><input id="project-directory" aria-label="绝对工程目录" value={directoryPath} onChange={(event) => setDirectoryPath(event.target.value)} placeholder="输入完整目录路径" autoComplete="off" spellCheck={false} />
            <button disabled={!connected || busy || !directoryPath.trim()} type="submit">只读验证目录</button><p className="muted">验证只读取指定目录；验证结果需另行选择后才关联。</p>
          </form>
        </div>
        <label className="search-box"><Icon name="search" size={17} /><input data-autofocus aria-label="筛选已列出的工程候选" value={search} onChange={(event) => setSearch(event.target.value)} placeholder="筛选候选：工程、路径或模块" /></label>
        <div className="results-summary"><span>工作区候选 {grouped.current.length} · 建议 {grouped.suggested.length} · 指定目录 {grouped.manual.length} · 历史 {grouped.history.length}{search ? '（筛选后）' : ''}</span></div>
        {(['current', 'suggested', 'manual', 'history'] as const).filter((category) => grouped[category].length > 0).map((category) => <section className="candidate-group" key={category} aria-label={categoryNames[category]}><h3>{categoryNames[category]} · {grouped[category].length}</h3><ul className="candidate-list">{grouped[category].map((candidate) => <CandidateItem key={candidate.id} candidate={candidate} category={category} busy={busy} select={(id) => { void select(id); }} />)}</ul></section>)}
        {!candidates.length && <div className="no-results"><Icon name="folder" size={30} /><h3>{search ? '没有匹配的工程候选' : '尚无可选择的鸿蒙工程'}</h3><p>{search ? '筛选仅作用于已列出的候选。指定目录请使用上方独立输入框。' : activePath ? '已取得工作区路径，但尚未确认可预览的实际鸿蒙工程。可指定应用根目录执行只读验证。' : '当前窗口没有提供可靠工程路径。可重新识别或手动指定目录。'}</p></div>}
      </>}
      {panel === 'trust' && (state?.project ? <div className="settings-card"><span className="section-icon"><Icon name="shield" size={27} /></span><h3>允许这个工程执行构建？</h3><p>发现和关联只读取工程信息。信任后才允许工程的构建逻辑运行。</p><code className="path">{displayPath(state.project.appRoot)}</code><p className="muted">构建配置改变时将再次确认。可随时撤销信任。</p><div className="panel-actions"><button className="primary-button" disabled={busy} onClick={async () => { if (await act('set_trust', { projectId: state.project!.id, trusted: true })) navigate(null); }}>信任此工程</button><button onClick={() => navigate('projects')}>换一个工程</button></div></div> : <p>当前没有待信任工程。</p>)}
      {panel === 'sdk' && <div className="settings-card"><span className="section-icon"><Icon name="device" size={27} /></span><h3>连接真实原生运行时</h3><p>使用合法安装的 HarmonyOS SDK 与构建工具，无需启动 DevEco GUI。</p><dl className="details-list"><div><dt>SDK 目录</dt><dd className="path">{state?.backend.sdkPath ? displayPath(state.backend.sdkPath) : '未配置'}</dd></div><div><dt>工具链版本</dt><dd>{state?.backend.sdkVersion ?? '尚未检测'}</dd></div></dl><p className="info-note">{state?.backend.reason ?? '桌面桥接未连接，浏览器页面无法探测 SDK。'}</p><button className="primary-button" disabled={!connected || busy} onClick={() => void act('configure_sdk', {})}><Icon name="folder" size={16} />指定 SDK 目录</button><p className="muted">原生基线为 Stage 工程的 Phone 画面。新增控制在预览能力中显式启用，实际 SDK 行为等待用户核验；其他设备 Profile 保留具体阻塞原因。</p></div>}
      {panel === 'path' && (state?.project ? <div className="settings-card"><h3>{state.project.name}</h3><dl className="details-list"><div><dt>应用根</dt><dd className="path">{displayPath(state.project.appRoot)}</dd></div><div><dt>工作区根</dt><dd className="path">{displayPath(state.project.workspaceRoot)}</dd></div><div><dt>环境</dt><dd>{environments[state.project.environment]}</dd></div><div><dt>模块 · 变体</dt><dd>{state.project.module} · {state.project.variant}</dd></div><div><dt>执行信任</dt><dd>{state.trusted ? '已信任当前构建配置' : '未信任 · 只读'}</dd></div></dl><div className="panel-actions"><button onClick={() => void copy(displayPath(state.project!.appRoot), '工程路径')}>复制完整路径</button><button onClick={() => navigate('projects')}>切换工程</button><button disabled={!state.trusted || busy} onClick={() => void act('set_trust', { projectId: state.project!.id, trusted: false })}>撤销工程信任</button></div></div> : <button onClick={() => navigate('projects')}>查看工程候选</button>)}
      {panel === 'settings' && <>
        <div className="settings-card"><h3>工具外观</h3><p className="muted">仅改变 ArkDock 外壳，不改变鸿蒙应用主题。</p><div className="theme-options" aria-label="外观主题">{(['dark', 'light', 'system'] as const).map((option) => <button key={option} className={theme === option ? 'selected' : ''} aria-pressed={theme === option} onClick={() => setTheme(option)}><Icon name={option === 'light' ? 'sun' : option === 'dark' ? 'moon' : 'monitor'} size={20} />{option === 'light' ? '浅色' : option === 'dark' ? '深色' : '跟随系统'}</button>)}</div></div>
        <div className="settings-card"><h3>动画偏好</h3><p className="muted">系统减少动画优先生效；关闭或轻量时不播放贴边涟漪与悬浮球装饰。</p><div className="motion-options" aria-label="减少动画偏好">{(['system', 'reduce'] as const).map((option) => <button key={option} aria-pressed={motion === option} className={motion === option ? 'selected' : ''} onClick={() => setMotion(option)}>{option === 'system' ? '跟随系统动画' : '减少动画'}</button>)}</div><div className="intensity-options" aria-label="动效强度">{(['off', 'light', 'standard', 'enhanced'] as const).map((option) => <button key={option} aria-pressed={intensity === option} className={intensity === option ? 'selected' : ''} onClick={() => setIntensity(option)}>{({ off: '关闭', light: '轻量', standard: '标准', enhanced: '增强' })[option]}</button>)}</div><output className="motion-status">{!connected ? '桌面桥接未连接，原生动效偏好尚未同步。' : state?.motion.reduced ? '原生窗口已减少动画。' : state?.motion.intensity === 'off' ? '原生窗口动效已关闭。' : state?.motion.intensity === 'light' ? '原生窗口使用轻量动效。' : state?.motion.intensity === 'enhanced' ? '原生窗口使用增强动效。' : '原生窗口使用标准动效。'}</output></div>
        <div className="settings-card"><h3>悬浮球</h3><label className="setting-toggle"><span><strong>贴边自动隐藏</strong><small>悬浮球闲置时沿屏幕边缘收起，靠近后恢复显示。</small></span><input type="checkbox" role="switch" aria-checked={state?.display.orbAutoHide ?? false} aria-label="悬浮球贴边自动隐藏" checked={state?.display.orbAutoHide ?? false} disabled={!connected || !nativeReady || displayBusy} onChange={(event) => chooseOrbAutoHide(event.target.checked)} /></label><button disabled={!connected || !nativeReady || displayBusy || state?.display.mode !== 'expanded'} onClick={() => void changeDisplay('collapse_to_orb')}><span className="orb-symbol" aria-hidden="true" />收起为悬浮球</button></div>
        <div className="settings-card"><h3>窗口与工程</h3><p className="pair-mode-description">{pair.label}{state?.pairLayout?.reason && ` · ${state.pairLayout.reason}`}</p><p className="muted">两个独立窗口全贴合，尺寸联动；共同最大化时 ArkDock 保持右侧。解除绑定会停止自有预览资源。</p><button disabled={!state?.target || busy} onClick={async () => { if (await act('detach', {})) navigate(null); }}>解除当前绑定</button>{state?.project && <button onClick={() => navigate('path')}>管理工程信任</button>}</div>
      </>}
      {panel === 'menu' && <div className="menu-list"><button disabled={!connected || !nativeReady || displayBusy || state?.display.mode !== 'expanded'} onClick={() => void changeDisplay('collapse_to_orb')}><span className="orb-symbol" aria-hidden="true" />收起为悬浮球</button><button disabled={!connected || !nativeReady || displayBusy || !state?.display.trayAvailable} title={state?.display.trayAvailable ? '隐藏窗口，由系统托盘恢复' : trayReason} onClick={() => void changeDisplay('hide_to_tray')}><Icon name="monitor" />隐藏到系统托盘</button>{!state?.display.trayAvailable && <p className="menu-note">{trayReason}</p>}<button onClick={() => navigate('projects')}><Icon name="folder" />选择当前工程<Icon name="chevron" size={16} /></button><button disabled={!state?.target || busy} onClick={async () => { navigate(null); await act('rediscover', {}); }}><Icon name="refresh" />重新识别</button><button disabled={!connected || busy} onClick={async () => { if (await act('select_directory', {})) navigate(stateRef.current?.phase === 'untrusted' ? 'trust' : null); }}><Icon name="folder" />手动选择目录</button><button onClick={() => navigate('controls')}><Icon name="preview" />预览能力</button><button onClick={() => navigate('sdk')}><Icon name="device" />设备与原生工具链</button><button onClick={() => navigate('diagnostics')}><Icon name="log" />运行日志</button><button onClick={() => navigate('settings')}><Icon name="settings" />偏好设置</button><button onClick={() => navigate('help')}><Icon name="link" />使用帮助</button><button disabled={!connected || busy || !pair.enabled} title={pair.reason} onClick={async () => { if (await act(pair.command, {})) navigate(null); }}><Icon name="maximize" />{pair.action}</button><button disabled={!connected || busy} onClick={() => void act('minimize_window', {})}><Icon name="monitor" />最小化窗口</button><button className="danger-button" disabled={!state?.target || busy} onClick={async () => { if (await act('detach', {})) navigate(null); }}><Icon name="close" />解除当前绑定</button></div>}
      {panel === 'controls' && <PreviewCapabilities snapshot={state} store={store} busy={busy} controls={controls} refresh={async () => { if (await act('refresh_preview', {})) navigate(null); }} />}
      {panel === 'diagnostics' && <RuntimeLogs snapshot={state} />}
      {panel === 'help' && <div className="settings-card"><h3>从正在开发的窗口开始</h3><ol className="help-steps"><li>按住顶部标识或拖动区，靠近开发窗口右侧。</li><li>收到目标提示后松开；独立窗口由原生层贴边。</li><li>可靠工程直接关联；线索不足时确认候选，未发现时选择目录。</li><li>确认工程信任，再检查工具链，等待真实原生首帧。</li></ol><p className="muted">可随时重新识别、切换工程或解除绑定。只读识别目前不能中途取消；解除绑定使会话失效。</p></div>}
    </Sheet>}
    {(error || feedback) && <div className={`toast ${error ? 'error' : ''}`} role={error ? 'alert' : 'status'}><span>{error ?? feedback}</span><button className="icon-button" disabled={Boolean(panel)} aria-label="关闭提示" onClick={() => { setError(null); setFeedback(null); }}><Icon name="close" size={16} /></button></div>}
    <DockRipple snapshot={state} motion={effectiveMotion} />
  </main>;
}
