import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { AppSnapshot, InteractionMode } from './ipc/model.ts';
import { nativeBridge } from './ipc/client.ts';
import type { UiBridge } from './ipc/client.ts';
import { describeError, errorCategoryLabel } from './ipc/errors.ts';
import { deriveUiState } from './view-state.ts';
import type { PreviewFrame } from './preview/frame-types.ts';
import { InspectorActionsBar, InspectorToolbar } from './inspector/InspectorToolbar.tsx';
import { InspectorPreview } from './inspector/InspectorPreview.tsx';
import type { InspectorActions } from './inspector/InspectorPreview.tsx';
import { PreviewCapabilities } from './PreviewCapabilities.tsx';
import { Icon } from './Icon.tsx';
import { Sheet } from './Sheet.tsx';
import { applyTheme, readTheme } from './theme.ts';
import type { Theme } from './theme.ts';
import { RuntimeLogs } from './RuntimeLogs.tsx';
import { PreviewChrome } from './PreviewChrome.tsx';
import { WorkspaceStrip } from './WorkspaceStrip.tsx';
import { currentScope, hasDockCandidate, scopeStillCurrent, sameTarget } from './request-state.ts';
import { pairControl } from './pair-layout.ts';
import { DockRipple } from './DockRipple.tsx';
import { displayTransitioning, nextDisplayRequestId } from './display-state.ts';
import { useNativeControls } from './preview/useNativeControls.ts';
import type { NativeStageInput } from './PreviewStage.tsx';
import { PreviewControlToolbar, PreviewPageSelect } from './preview/PreviewControlToolbar.tsx';
import { DeviceRuntimePanel } from './preview/DeviceRuntimePanel.tsx';
import { useNotices } from './app/notices.ts';
import { useNavigation } from './app/useNavigation.ts';
import { panelNames } from './app/navigation.ts';
import type { Panel } from './app/navigation.ts';
import { nextDisplayDeadline } from './app/candidates.ts';
import { recoveryOffer, recoveryText } from './app/recovery.ts';
import type { RecoveryIntent } from './app/recovery.ts';
import { useDesktopSession } from './app/useDesktopSession.ts';
import type { SnapshotTransition } from './app/useDesktopSession.ts';
import { usePreferences } from './app/usePreferences.ts';
import { useIpcCommands } from './app/useIpcCommands.ts';
import { BuildStatusBar, EmptyState } from './app/PreviewStatus.tsx';
import { ProjectsPanel } from './app/ProjectsPanel.tsx';
import { HelpPanel, MenuPanel, PathPanel, SdkPanel, SettingsPanel, TrustPanel } from './app/SidePanels.tsx';
import type { MenuIntents, ShellControls } from './app/SidePanels.tsx';
import './preview/preview-controls.css';

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

/**
 * Composition root. Cross-cutting state lives in `src/app/` hooks (desktop
 * session, request lifecycle, navigation, native preferences, notices); status
 * and panel surfaces live in `src/app/` components. What remains here is the
 * wiring between them plus the top-level window/header interactions.
 */
export function App({ bridge = nativeBridge }: { bridge?: UiBridge } = {}) {
  const [theme, setTheme] = useState<Theme>(readTheme);
  const [search, setSearch] = useState('');
  const [directoryPath, setDirectoryPath] = useState('');
  const [layoutConsent, setLayoutConsent] = useState(false);
  const [zoom, setZoom] = useState<number | 'fit'>('fit');
  const [interactionMode, setInteractionMode] = useState<InteractionMode>('interact');
  const [runtimeKind, setRuntimeKind] = useState<'native' | 'emulator' | 'physicalDevice'>('native');
  const [inspectorProblem, setInspectorProblem] = useState<string | null>(null);
  const [displayClock, setDisplayClock] = useState(Date.now);
  const inspectorActions = useRef<InspectorActions | null>(null);
  const displayRequestId = useRef(0);
  const inputPending = useRef(false);

  const notices = useNotices();
  const navigation = useNavigation();
  // Snapshot transition side effects are dispatched through this slot so the
  // session hook can be created before the concerns it must notify.
  const transition = useRef<(change: SnapshotTransition) => void>(() => { });
  const session = useDesktopSession({
    bridge, onError: notices.report, onTransition: (change) => transition.current(change),
  });
  const preferences = usePreferences({ bridge, session, notices });
  const commands = useIpcCommands({
    bridge, session, notices, navigationRevision: navigation.revision, openPanel: navigation.open,
  });

  const { state, stateRef, store, frameController, receive, connected, nativeReady } = session;
  const { panel, anchor, navigate: navigateTo, open: openPanel, openAutomatic, resetAutomatic } = navigation;
  const { act, cancelAssociation, pending, cancelling, pendingRef, activeCommand, cancelRequested } = commands;
  const { report, notify, clear } = notices;
  const { motion, intensity, effectiveMotion, adoptNativeOrbChange, setMotion, setIntensity, chooseOrbAutoHide } = preferences;

  const ui = deriveUiState(state, null);
  const pair = pairControl(state);
  const busy = pending !== null;
  const displayBusy = busy || displayTransitioning(state?.display);
  const trayReason = !connected ? '桌面桥接未连接' : state?.display.reason ?? '系统托盘尚未就绪';
  const trayAvailable = Boolean(state?.display.trayAvailable);
  const statusText = !connected ? '桌面未连接' : pending === 'begin_drag' ? '正在连接' : ui.bindingLabel;
  const previewVisible = session.previewVisible && runtimeKind === 'native';

  useEffect(() => { applyTheme(theme); }, [theme]);

  // Display semantics are time bound, and the backend reports when they may move.
  // Re-reading `snapshot` at that deadline converges the view without recomputing a
  // category here; the reply is revision gated, so a late answer cannot regress it.
  useEffect(() => {
    if (!connected || !state) return;
    const now = Date.now();
    const next = nextDisplayDeadline(state, now);
    if (next === null) return;
    const timeout = window.setTimeout(() => {
      setDisplayClock(Date.now());
      void bridge.request('snapshot', {}).then(receive).catch(() => undefined);
    }, next - now + 1);
    return () => window.clearTimeout(timeout);
  }, [connected, bridge, receive, state]);

  // A committed snapshot is the only place that changes UI ownership. Native orb
  // choices are adopted once queued writes settled, a target lifecycle change
  // resets panel-scoped state, and an automatic panel is consumed at most once
  // per lifecycle so it cannot re-open in a loop.
  transition.current = ({ previous, updated }: SnapshotTransition) => {
    adoptNativeOrbChange(previous, updated);
    if (!sameTarget(previous?.target, updated.target)) {
      const lost = Boolean(previous?.target && !updated.target);
      resetAutomatic();
      openPanel(null);
      setSearch(''); setDirectoryPath(''); setLayoutConsent(false);
      clear();
      if (lost && !cancelRequested.current && activeCommand.current !== 'detach') notify('目标窗口不可用，已解除绑定。');
    }
    if (updated.display.mode !== 'expanded') openPanel(null);
    openAutomatic(updated, cancelRequested.current);
  };

  const controls = useNativeControls({ bridge, snapshot: state, store,
    visible: previewVisible && !panel && interactionMode === 'interact',
    windowVisible: previewVisible, wake: () => frameController.current?.wake() });
  const controlsRef = useRef(controls); controlsRef.current = controls;
  // Keep stage input identity stable across snapshots while dispatching through current controls.
  const nativeInput = useMemo<NativeStageInput>(() => ({
    start: (frame, x, y) => controlsRef.current.start(frame, x, y),
    move: controls.input.move.bind(controls.input), end: controls.input.end.bind(controls.input),
    cancel: () => controlsRef.current.cancelInput(), text: (text) => controlsRef.current.text(text), key: (key) => controlsRef.current.key(key),
  }), [controls.input]);

  function navigate(next: Panel | null) { navigateTo(next); notify(null); }
  async function inspectPath() {
    const path = directoryPath.trim();
    if (!path) return;
    if (await act('inspect_directory_path', { path })) {
      setSearch('');
      notify('目录只读验证已完成。请确认对应候选工程后，点击关联。');
    }
  }
  async function chooseDirectory() {
    if (await act('select_directory', {})) navigate(stateRef.current?.phase === 'untrusted' ? 'trust' : null);
  }
  async function select(id: string) {
    const target = stateRef.current?.target?.id;
    if (await act('select_candidate', { candidateId: id }) && stateRef.current?.target?.id === target) openPanel(stateRef.current?.phase === 'untrusted' ? 'trust' : null);
  }
  async function changePair() { if (await act(pair.command, {})) navigate(null); }
  async function detach() { if (await act('detach', {})) navigate(null); }
  async function trustCurrentProject() {
    const project = stateRef.current?.project;
    if (project && await act('set_trust', { projectId: project.id, trusted: true })) navigate(null);
  }
  async function untrustCurrentProject() {
    const project = stateRef.current?.project;
    if (project) await act('set_trust', { projectId: project.id, trusted: false });
  }
  async function changeDisplay(command: 'collapse_to_orb' | 'expand_from_orb' | 'hide_to_tray' | 'show_from_tray') {
    try {
      displayRequestId.current = nextDisplayRequestId(displayRequestId.current, stateRef.current?.display);
      if (await act(command, { requestId: displayRequestId.current })) navigate(null);
    } catch (reason) { report(reason); }
  }
  async function copy(text: string, label: string) {
    try { await navigator.clipboard.writeText(text); notify(`${label}已复制`); }
    catch { report('当前环境无法访问剪贴板。可在路径或诊断中选中文字复制。'); }
  }
  // The backend's recovery action, narrowed to what this snapshot can actually do.
  // Every branch re-enters the Rust backend, which re-validates before acting; none
  // of them re-issues an Agent delivery, a credential change or a trust change.
  function runRecovery(intent: RecoveryIntent) {
    switch (intent) {
      case 'refreshSnapshot': void act('snapshot', {}); return;
      case 'rediscover': void act('rediscover', {}); return;
      case 'openProjects': clear(); navigate('projects'); return;
      case 'openTrust': clear(); navigate('trust'); return;
      case 'configureSdk': void act('configure_sdk', {}); return;
      case 'restartPreview': void act('refresh_preview', {}); return;
      case 'openDiagnostics': clear(); navigate('diagnostics'); return;
    }
  }
  const recovery = recoveryOffer(notices.error, {
    connected, target: Boolean(state?.target), project: Boolean(state?.project),
    trusted: Boolean(state?.trusted), refreshAvailable: ui.capabilities.refresh.status === 'available',
  });
  const recoveryHint = recoveryText(notices.error);
  const exitInspection = useCallback(() => setInteractionMode('interact'), []);
  const clickNative = useCallback(async (frame: PreviewFrame, x: number, y: number) => {
    if (inputPending.current || pendingRef.current || interactionMode !== 'interact') return;
    const actual = stateRef.current;
    if (!deriveUiState(actual, frame).inputAllowed) return;
    const scope = currentScope(actual);
    inputPending.current = true;
    try { frameController.current?.wake(); receive(await bridge.request('click_preview', { buildId: frame.buildId, generation: frame.generation, x, y }, scope)); }
    catch (reason) { if (scopeStillCurrent(stateRef.current, scope)) report(reason); }
    finally { inputPending.current = false; }
  }, [bridge, frameController, interactionMode, pendingRef, receive, report, stateRef]);

  const shell: ShellControls = {
    connected, nativeReady, busy, displayBusy, trayAvailable, trayReason,
    collapseToOrb: () => void changeDisplay('collapse_to_orb'), hideToTray: () => void changeDisplay('hide_to_tray'),
  };
  const menuIntents: MenuIntents = {
    open: (next) => navigate(next),
    rediscover: () => { navigate(null); void act('rediscover', {}); },
    chooseDirectory: () => void chooseDirectory(),
    pair: () => void changePair(),
    minimize: () => void act('minimize_window', {}),
    detach: () => void detach(),
  };

  return <main className={`app ${hasDockCandidate(state?.dockHint) ? 'target-near' : ''}`} data-binding={ui.binding} data-workspace={ui.workspace} data-pair-layout={pair.state} data-display={state?.display.mode ?? 'expanded'}>
    <div className="dock-body" {...(panel || state && state.display.mode !== 'expanded' ? { inert: '' } : {})}>
      <header className="header"><button className="drag-zone drag" aria-label="拖动 ArkDock 到目标应用窗口外侧" title="按住顶部，拖到目标窗口右侧" onPointerDown={(event) => { if (event.button === 0 && connected) void act('begin_drag', {}); }} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); notify('键盘移动：Alt+空格打开系统窗口菜单，选择移动。'); } }}><Icon name="grip" size={15} /><span className="mark" aria-hidden="true"><span /><span /></span><strong>ArkDock</strong></button><span className={`header-status ${ui.binding === 'bound' ? 'connected' : ''}`} title={state?.target?.title ?? statusText}><span className="dot" />{statusText}</span><div className="header-actions"><button className="collapse-orb" disabled={!connected || !nativeReady || displayBusy || state?.display.mode !== 'expanded'} aria-label="收起为悬浮球" title={connected ? '收起为悬浮球' : '桌面桥接未连接'} onClick={() => void changeDisplay('collapse_to_orb')}><span className="orb-symbol" aria-hidden="true" /><span className="collapse-label">收起</span></button><button className="icon-button" aria-label="更多操作" title="更多操作" onClick={() => navigate('menu')}><span className="ellipsis" aria-hidden="true">···</span></button><button className="icon-button minimize" disabled={!connected || busy} aria-label="最小化窗口" title={connected ? '最小化窗口' : '桌面桥接未连接'} onClick={() => void act('minimize_window', {})}><span aria-hidden="true">−</span></button><button className="icon-button maximize" disabled={!connected || busy || !pair.enabled} aria-label={pair.action} title={pair.reason} onClick={() => void changePair()}><Icon name={pair.state === 'maximized' ? 'restore' : 'maximize'} size={16} /></button><button className="icon-button close-window" disabled={!connected || !nativeReady || displayBusy || state?.display.mode !== 'expanded' || !trayAvailable} aria-label="隐藏到系统托盘" title={trayAvailable ? '隐藏到系统托盘，可从托盘图标恢复' : trayReason} onClick={() => void changeDisplay('hide_to_tray')}><Icon name="close" size={17} /></button></div></header>
      <WorkspaceStrip snapshot={state} busy={busy || !connected} cancelling={cancelling}
        openProject={() => navigate(state?.project ? 'path' : 'projects')} openCandidates={() => navigate('projects')}
        manual={() => void chooseDirectory()} rediscover={() => void act('rediscover', {})} cancel={() => void cancelAssociation()} />
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
      </section><div className="stage-caption"><span><span className={`tiny-dot ${runtimeKind === 'native' && state?.phase === 'running' ? 'live' : ''}`} />{runtimeKind === 'native' ? state?.backend.available ? 'NATIVE PREVIEW' : 'PREVIEW WORKSPACE' : 'DEVICE RUNTIME'}</span><span>{state?.backend.sdkVersion ? `SDK ${state.backend.sdkVersion}` : '等待工具链'}</span></div></>}
          status={<BuildStatusBar snapshot={state} store={store} pending={pending} open={() => navigate('diagnostics')} />}
          statusIndicator={<BuildStatusBar snapshot={state} store={store} pending={pending} compact />}>
          <div className="native-preview-host" hidden={runtimeKind !== 'native'}>
          <StableInspectorPreview visible={previewVisible} bridge={bridge} interactionMode={interactionMode} exit={exitInspection}
            actions={inspectorActions} onProblem={setInspectorProblem} onError={report}
            controlState={controls.state} nativeInput={nativeInput} inputPaused={Boolean(controls.pending || panel || runtimeKind !== 'native')} inputError={describeError(controls.error)}
            snapshot={state} store={store} zoom={zoom} busy={busy} onClick={clickNative} />
          <EmptyState snapshot={state} store={store} connected={connected} navigate={navigate} />
          </div>
          {runtimeKind !== 'native' && <DeviceRuntimePanel snapshot={state} bridge={bridge} kind={runtimeKind} />}
        </PreviewChrome>
      </section>
      {state?.layoutRequired && <aside className="layout-notice"><Icon name="monitor" size={18} /><div><p>右侧空间不足。并排布局会调整目标窗口。</p><label><input type="checkbox" checked={layoutConsent} onChange={(event) => setLayoutConsent(event.target.checked)} />同意调整目标窗口</label></div><button disabled={!layoutConsent || busy} onClick={() => { void act('arrange_side_by_side', { consent: true }); setLayoutConsent(false); }}>应用布局</button></aside>}
    </div>
    {panel && <Sheet key={panel} returnFocus={anchor.current} title={panelNames[panel]} kind={panel === 'diagnostics' ? 'diagnostics' : panel} close={() => navigate(null)}>
      {panel === 'projects' && <ProjectsPanel snapshot={state} connected={connected} busy={busy} now={Math.max(displayClock, Date.now())} search={search} onSearch={setSearch}
        directoryPath={directoryPath} onDirectoryPath={setDirectoryPath}
        inspectDirectory={() => void inspectPath()} chooseDirectory={() => void chooseDirectory()}
        rediscover={() => void act('rediscover', {})} select={(id) => void select(id)} />}
      {panel === 'trust' && <TrustPanel snapshot={state} busy={busy} trust={() => void trustCurrentProject()} browse={() => navigate('projects')} />}
      {panel === 'sdk' && <SdkPanel snapshot={state} connected={connected} busy={busy} configure={() => void act('configure_sdk', {})} />}
      {panel === 'path' && <PathPanel snapshot={state} busy={busy} copy={(text, label) => void copy(text, label)} browse={() => navigate('projects')} untrust={() => void untrustCurrentProject()} />}
      {panel === 'settings' && <SettingsPanel snapshot={state} shell={shell} theme={theme} setTheme={setTheme}
        motion={motion} setMotion={setMotion} intensity={intensity} setIntensity={setIntensity}
        chooseOrbAutoHide={chooseOrbAutoHide} changeDisplay={(command) => void changeDisplay(command)} pair={pair} detach={() => void detach()} open={(next) => navigate(next)} />}
      {panel === 'menu' && <MenuPanel snapshot={state} shell={shell} pair={pair} intents={menuIntents} />}
      {panel === 'controls' && <PreviewCapabilities snapshot={state} store={store} busy={busy} controls={controls} refresh={async () => { if (await act('refresh_preview', {})) navigate(null); }} />}
      {panel === 'diagnostics' && <RuntimeLogs snapshot={state} />}
      {panel === 'help' && <HelpPanel />}
    </Sheet>}
    {(notices.error || notices.feedback) && <div className={`toast ${notices.error ? 'error' : ''}`} role={notices.error ? 'alert' : 'status'} title={notices.error && !notices.error.legacy ? `${notices.error.code} · ${errorCategoryLabel(notices.error)}${notices.error.retryable ? ' · 可重试' : ''}` : undefined}><span>{notices.error ? `${describeError(notices.error)}${recoveryHint ? ` · ${recoveryHint}` : ''}` : notices.feedback}</span>{recovery && <button type="button" className="toast-action" disabled={Boolean(panel)} onClick={() => runRecovery(recovery.intent)}>{recovery.label}</button>}<button className="icon-button" disabled={Boolean(panel)} aria-label="关闭提示" onClick={clear}><Icon name="close" size={16} /></button></div>}
    <DockRipple snapshot={state} motion={effectiveMotion} />
  </main>;
}
