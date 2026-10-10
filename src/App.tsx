import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { AppSnapshot, Candidate, CommandArgs, NativeFrame } from './ipc/model.ts';
import { nativeBridge } from './ipc/client.ts';
import type { UiBridge } from './ipc/client.ts';
import { applySnapshot, deriveUiState, filterCandidates, needsDirectoryFallback } from './view-state.ts';
import { createFrameStore } from './frame-store.ts';
import type { FrameStore } from './frame-store.ts';
import { PreviewStage } from './PreviewStage.tsx';
import { PreviewCapabilities } from './PreviewCapabilities.tsx';
import { Icon } from './Icon.tsx';
import { Sheet } from './Sheet.tsx';
import { applyTheme, readTheme } from './theme.ts';
import type { Theme } from './theme.ts';
import { applyMotion, cancelMotion, readMotion } from './motion.ts';
import type { MotionPreference } from './motion.ts';
import { diagnosticText } from './diagnostics.ts';
import { hasDockCandidate, responseStillCurrent, sameTarget } from './request-state.ts';
import { pairControl } from './pair-layout.ts';

type Panel = 'projects' | 'sdk' | 'trust' | 'path' | 'settings' | 'menu' | 'controls' | 'diagnostics' | 'help' | null;
const environments = { windowsLocal: 'Windows 本地', wsl: 'WSL · 预览未支持', remote: '远程 · 预览未支持' };
const sources = { registration: '来自当前会话', uia: '窗口选中标签', processArguments: '进程启动参数', history: '曾关联此窗口', title: '窗口标题 · 路径待确认', manual: '手动目录选择' };
const panelNames = { projects: '选择当前工程', sdk: '设备与原生工具链', trust: '工程信任', path: '工程身份与路径', settings: '偏好设置', menu: '更多操作', controls: '预览能力', diagnostics: '运行日志', help: '使用帮助' };

function CandidateItem({ candidate, select, busy }: { candidate: Candidate; select: (id: string) => void; busy: boolean }) {
  return <li className={`candidate ${candidate.autoEligible ? 'reliable' : ''}`}><button disabled={busy || !candidate.valid || candidate.project.environment !== 'windowsLocal'} onClick={() => select(candidate.id)}>
    <span className="candidate-heading"><span className="candidate-icon"><Icon name="folder" /></span><strong>{candidate.project.name}</strong><span className={`badge ${candidate.valid ? '' : 'invalid'}`}>{candidate.valid ? candidate.autoEligible ? '证据可靠' : '需要确认' : '目录不可用'}</span><Icon name="chevron" size={16} /></span>
    <code className="path">{candidate.project.appRoot}</code><span>{environments[candidate.project.environment]} · {candidate.project.module} · {candidate.project.variant}</span><span>工作区：{candidate.project.workspaceRoot}</span>
    <span className="evidence-tags">{Array.from(new Set(candidate.evidence.map((item) => sources[item.source]))).map((source) => <span key={source}>{source}</span>)}</span><span className="candidate-reason">{candidate.reasons.join('；') || '请选择与目标窗口对应的实际工程。'}</span>
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

function BuildStatusBar({ snapshot, store, pending, open }: { snapshot: AppSnapshot | null; store: FrameStore; pending: string | null; open: () => void }) {
  const frame = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const ui = deriveUiState(snapshot, frame);
  const failures = snapshot?.diagnostics.filter((item) => item.status === 'FAIL').length;
  const blocked = snapshot?.diagnostics.filter((item) => item.status === 'BLOCKED').length;
  return <footer className="footer" data-preview={ui.preview}><button className="status-summary" onClick={open} aria-label="打开诊断" title="查看构建、窗口与工程诊断"><span className={`dot ${ui.preview === 'running' ? 'live' : ui.preview === 'stale' || ui.preview === 'error' ? 'warning' : ''}`} /><span className="status-copy"><strong aria-live="polite">{pending ? '请求处理中…' : ui.previewLabel}</strong><span>{frame ? `画面 v${frame.generation} · ${new Date(frame.capturedAt).toLocaleTimeString('zh-CN')}` : snapshot?.build.buildId ? `代码 v${snapshot.build.generation} · ${snapshot.build.durationMs == null ? '耗时未知' : `${snapshot.build.durationMs} ms`}` : '尚无原生画面'}</span></span><span className="diagnostic-count">{failures == null ? '诊断未知' : `${failures} 失败 · ${blocked} 阻塞`}</span><Icon name="log" size={18} /></button></footer>;
}

export function App({ bridge = nativeBridge }: { bridge?: UiBridge } = {}) {
  const [state, setState] = useState<AppSnapshot | null>(null);
  const stateRef = useRef<AppSnapshot | null>(null);
  const [store] = useState(createFrameStore);
  const [panel, setPanel] = useState<Panel>(null);
  const panelAnchor = useRef<HTMLElement | null>(null);
  const navigationRevision = useRef(0);
  const [search, setSearch] = useState('');
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
  const connected = bridge.available;
  const ui = deriveUiState(state, null);
  const pair = pairControl(state);
  const receive = useCallback((value: AppSnapshot) => {
    const updated = applySnapshot(stateRef.current, value);
    if (updated === stateRef.current) return;
    if (!sameTarget(stateRef.current?.target, updated.target)) {
      const lost = Boolean(stateRef.current?.target && !updated.target);
      setPanel(null); setSearch(''); setError(null); setFeedback(null); setLayoutConsent(false);
      if (lost && !cancelRequested.current && activeCommand.current !== 'detach') setFeedback('目标窗口不可用，已解除绑定。');
    }
    stateRef.current = updated; store.sync(updated); setState(updated);
  }, [store]);
  useEffect(() => {
    let live = true;
    let unsubscribe: (() => void) | undefined;
    if (connected) bridge.observe((value) => { if (live) receive(value); }, (frame) => { if (live && stateRef.current) store.push(stateRef.current, frame); }).then(async (off) => {
      if (!live) { off(); return; }
      unsubscribe = off;
      const snapshot = await bridge.request('snapshot', {});
      if (live) receive(snapshot);
    }).catch((reason: unknown) => { if (live) setError(String(reason)); });
    return () => { live = false; unsubscribe?.(); store.clear(); };
  }, [bridge, connected, store, receive]);
  useEffect(() => { applyTheme(theme); }, [theme]);
  useEffect(() => { applyMotion(motion); }, [motion]);
  useEffect(() => {
    const media = window.matchMedia('(prefers-reduced-motion: reduce)');
    const changed = () => { if (media.matches) cancelMotion(); };
    media.addEventListener('change', changed);
    return () => media.removeEventListener('change', changed);
  }, []);
  useEffect(() => {
    if (cancelRequested.current) return;
    if (state?.phase === 'selectProject') setPanel('projects');
    else if (state?.phase === 'untrusted') setPanel('trust');
  }, [state?.phase, state?.sessionId]);
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
  async function copy(text: string, label: string) {
    try { await navigator.clipboard.writeText(text); setFeedback(`${label}已复制`); }
    catch { setError('当前环境无法访问剪贴板。可在路径或诊断中选中文字复制。'); }
  }
  const candidates = filterCandidates(state?.candidates ?? [], search);
  const busy = pending !== null;
  const working = state?.phase === 'discovering' || state?.phase === 'building';
  const statusText = !connected ? '桌面未连接' : pending === 'begin_drag' ? '正在连接' : ui.bindingLabel;
  return <main className={`app ${hasDockCandidate(state?.dockHint) ? 'target-near' : ''}`} data-binding={ui.binding} data-workspace={ui.workspace} data-pair-layout={pair.state}>
    <div className="dock-body" {...(panel ? { inert: '' } : {})}>
      <header className="header"><button className="drag-zone drag" aria-label="拖动 ArkDock 到目标应用窗口外侧" title="按住顶部，拖到目标窗口右侧" onPointerDown={(event) => { if (event.button === 0 && connected) void act('begin_drag', {}); }} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); setFeedback('键盘移动：Alt+空格打开系统窗口菜单，选择移动。'); } }}><Icon name="grip" size={15} /><span className="mark" aria-hidden="true"><span /><span /></span><strong>ArkDock</strong></button><span className={`header-status ${ui.binding === 'bound' ? 'connected' : ''}`} title={state?.target?.title ?? statusText}><span className="dot" />{statusText}</span><div className="header-actions"><button className="icon-button" aria-label="更多操作" title="更多操作" onClick={() => navigate('menu')}><span className="ellipsis" aria-hidden="true">···</span></button><button className="icon-button minimize" disabled={!connected || busy} aria-label="最小化窗口" title={connected ? '最小化窗口' : '桌面桥接未连接'} onClick={() => void act('minimize_window', {})}><span aria-hidden="true">−</span></button><button className="icon-button maximize" disabled={!connected || busy || !pair.enabled} aria-label={pair.action} title={pair.reason} onClick={() => void act(pair.command, {})}><Icon name={pair.state === 'maximized' ? 'restore' : 'maximize'} size={16} /></button><button className="icon-button close-window" disabled={!connected || busy} aria-label="关闭 ArkDock" title={connected ? '关闭 ArkDock' : '桌面桥接未连接'} onClick={() => void act('close_window', {})}><Icon name="close" size={17} /></button></div></header>
      <section className="workspace-strip" aria-label="当前工程"><button className="workspace-main" aria-label="工程" onClick={() => navigate(state?.project ? 'path' : 'projects')} title={state?.project?.appRoot ?? '查看工程候选'}><span className="workspace-icon"><Icon name="folder" size={18} /></span><span><strong>{state?.project?.name ?? '尚未关联工程'}</strong><span className="workspace-path">{state?.project ? `${environments[state.project.environment]} · ${state.project.module}` : '拖动贴边后，只读识别当前工程'}</span></span><Icon name="chevron" size={15} /></button><button className="icon-button" disabled={!state?.target || busy} aria-label="重新识别" title={!state?.target ? '先连接开发窗口' : '重新读取目标工程线索'} onClick={() => void act('rediscover', {})}><Icon name="refresh" size={17} /></button></section>
      <output key={`${state?.phase}-${ui.workspace}-${state?.dockHint ?? ''}`} className={`state-strip ${working ? 'working' : ''}`}><Icon name={state?.trusted ? 'shield' : 'link'} size={14} /><span>{hasDockCandidate(state?.dockHint) ? '松开以连接 · 靠近目标外侧' : state?.dockHint ?? (state?.target ? ui.workspaceLabel : '独立悬浮 · 拖到开发窗口旁开始')}</span>{state?.target && <span className={`pair-badge ${pair.pending ? 'pending' : ''}`} title={pair.reason}>{pair.state === 'maximized' ? '共同最大化' : pair.pending ? '布局调整中' : pair.state === 'blocked' ? '布局受限' : pair.state === 'paired' ? '全贴合' : '等待布局'}</span>}{working && <span className="task-spinner" />}{state?.phase === 'discovering' && <button className="cancel-discovery" disabled={cancelling} title="解除绑定，使当前关联会话失效；正在进行的只读检查等待后端返回" onClick={() => void cancelAssociation()}>{cancelling ? '取消中…' : '取消关联'}</button>}</output>
      <section className="preview-toolbar" aria-label="预览控制"><div className="toolbar-selects"><label><span className="sr-only">运行方式</span><select aria-label="运行方式" value="native" title={state?.backend.reason ?? '等待桌面后端'} onChange={() => {}}><option value="native">原生 Previewer{state?.backend.available ? '' : ' · 待就绪'}</option><option disabled>模拟器 · 未支持</option><option disabled>真机 · 未支持</option></select></label><label><span className="sr-only">页面或组件</span><select aria-label="页面或组件" disabled title="当前适配器仅支持默认 pages/Index；页面切换未接入"><option>Index · 默认页面</option></select></label><button className={`icon-button refresh-button ${pending === 'refresh_preview' ? 'pending' : ''}`} disabled={ui.capabilities.refresh.status !== 'available' || busy} aria-label="手动刷新" title={ui.capabilities.refresh.reason} onClick={() => void act('refresh_preview', {})}><Icon name="refresh" size={18} /></button></div><div className="toolbar-actions"><button className="stop-button" disabled={ui.capabilities.stop.status !== 'available' || busy} title={ui.capabilities.stop.reason} aria-label="停止预览" onClick={() => void act('stop_preview', {})}><Icon name="stop" size={14} /><span>停止</span></button><label className="zoom-control"><select aria-label="画面缩放" value={zoom} onChange={(event) => setZoom(event.target.value === 'fit' ? 'fit' : Number(event.target.value))}><option value="fit">适应窗口</option><option value="0.5">50%</option><option value="0.75">75%</option><option value="1">100%</option><option value="1.5">150%</option></select></label><span className="toolbar-divider" /><span className="mode-caption"><Icon name="device" size={14} />原生画面</span><button className="icon-button" aria-label="预览能力" title="查看方向、应用主题和输入能力" onClick={() => navigate('controls')}><Icon name="settings" size={17} /></button></div></section>
      <section className="preview" aria-label="原生预览画面"><div className="stage-caption"><span><span className={`tiny-dot ${state?.phase === 'running' ? 'live' : ''}`} />{state?.backend.available ? 'NATIVE PREVIEW' : 'PREVIEW WORKSPACE'}</span><span>{state?.backend.sdkVersion ? `SDK ${state.backend.sdkVersion}` : '等待工具链'}</span></div><PreviewStage snapshot={state} store={store} zoom={zoom} busy={busy} onClick={(frame: NativeFrame, x: number, y: number) => { void act('click_preview', { buildId: frame.buildId, generation: frame.generation, x, y }); }} /><EmptyState snapshot={state} store={store} connected={connected} navigate={navigate} /></section>
      {state?.layoutRequired && <aside className="layout-notice"><Icon name="monitor" size={18} /><div><p>右侧空间不足。并排布局会调整目标窗口。</p><label><input type="checkbox" checked={layoutConsent} onChange={(event) => setLayoutConsent(event.target.checked)} />同意调整目标窗口</label></div><button disabled={!layoutConsent || busy} onClick={() => { void act('arrange_side_by_side', { consent: true }); setLayoutConsent(false); }}>应用布局</button></aside>}
      <BuildStatusBar snapshot={state} store={store} pending={pending} open={() => navigate('diagnostics')} />
    </div>
    {panel && <Sheet key={panel} returnFocus={panelAnchor.current} title={panelNames[panel]} kind={panel === 'diagnostics' ? 'diagnostics' : panel} close={() => navigate(null)}>
      {panel === 'projects' && <><p className="muted">确认与当前窗口对应的工程。关联不授予构建权限。</p><label className="search-box"><Icon name="search" size={17} /><input data-autofocus aria-label="搜索工程" value={search} onChange={(event) => setSearch(event.target.value)} placeholder="搜索工程、路径或模块" /></label><div className="results-summary"><span>发现 {state?.candidates.length ?? 0} 项 · 匹配 {candidates.length} 项</span><button disabled={!state?.target || busy} onClick={() => void act('rediscover', {})}>重新识别</button></div><ul className="candidate-list">{candidates.map((candidate) => <CandidateItem key={candidate.id} candidate={candidate} busy={busy} select={(id) => { void select(id); }} />)}</ul>{!candidates.length && <div className="no-results"><Icon name="folder" size={30} /><h3>{search ? '没有匹配的工程' : '未发现可确认的工程'}</h3><p>{search ? '试试完整路径或模块名。' : '当前窗口没有提供可靠路径。请选择实际工程目录。'}</p></div>}{needsDirectoryFallback(state?.candidates ?? []) && <div className="fallback"><button className="primary-button" disabled={!connected || busy} onClick={async () => { if (await act('select_directory', {})) navigate(stateRef.current?.phase === 'untrusted' ? 'trust' : null); }}><Icon name="folder" size={16} />选择工程目录</button></div>}</>}
      {panel === 'trust' && (state?.project ? <div className="settings-card"><span className="section-icon"><Icon name="shield" size={27} /></span><h3>允许这个工程执行构建？</h3><p>发现和关联只读取工程信息。信任后才允许工程的构建逻辑运行。</p><code className="path">{state.project.appRoot}</code><p className="muted">构建配置改变时将再次确认。可随时撤销信任。</p><div className="panel-actions"><button className="primary-button" disabled={busy} onClick={async () => { if (await act('set_trust', { projectId: state.project!.id, trusted: true })) navigate(null); }}>信任此工程</button><button onClick={() => navigate('projects')}>换一个工程</button></div></div> : <p>当前没有待信任工程。</p>)}
      {panel === 'sdk' && <div className="settings-card"><span className="section-icon"><Icon name="device" size={27} /></span><h3>连接真实原生运行时</h3><p>使用合法安装的 HarmonyOS SDK 与构建工具，无需启动 DevEco GUI。</p><dl className="details-list"><div><dt>SDK 目录</dt><dd className="path">{state?.backend.sdkPath ?? '未配置'}</dd></div><div><dt>工具链版本</dt><dd>{state?.backend.sdkVersion ?? '尚未检测'}</dd></div></dl><p className="info-note">{state?.backend.reason ?? '桌面桥接未连接，浏览器页面无法探测 SDK。'}</p><button className="primary-button" disabled={!connected || busy} onClick={() => void act('configure_sdk', {})}><Icon name="folder" size={16} />指定 SDK 目录</button><p className="muted">当前适配器限定默认 Stage、pages/Index 与 360×640；其他 SDK 版本需要单独核验。</p></div>}
      {panel === 'path' && (state?.project ? <div className="settings-card"><h3>{state.project.name}</h3><dl className="details-list"><div><dt>应用根</dt><dd className="path">{state.project.appRoot}</dd></div><div><dt>工作区根</dt><dd className="path">{state.project.workspaceRoot}</dd></div><div><dt>环境</dt><dd>{environments[state.project.environment]}</dd></div><div><dt>模块 · 变体</dt><dd>{state.project.module} · {state.project.variant}</dd></div><div><dt>执行信任</dt><dd>{state.trusted ? '已信任当前构建配置' : '未信任 · 只读'}</dd></div></dl><div className="panel-actions"><button onClick={() => void copy(state.project!.appRoot, '工程路径')}>复制完整路径</button><button onClick={() => navigate('projects')}>切换工程</button><button disabled={!state.trusted || busy} onClick={() => void act('set_trust', { projectId: state.project!.id, trusted: false })}>撤销工程信任</button></div></div> : <button onClick={() => navigate('projects')}>查看工程候选</button>)}
      {panel === 'settings' && <><div className="settings-card"><h3>工具外观</h3><p className="muted">仅改变 ArkDock 外壳，不改变鸿蒙应用主题。</p><div className="theme-options" aria-label="外观主题">{(['dark', 'light', 'system'] as const).map((option) => <button key={option} className={theme === option ? 'selected' : ''} aria-pressed={theme === option} onClick={() => setTheme(option)}><Icon name={option === 'light' ? 'sun' : option === 'dark' ? 'moon' : 'monitor'} size={20} />{option === 'light' ? '浅色' : option === 'dark' ? '深色' : '跟随系统'}</button>)}</div></div><div className="settings-card"><h3>动画偏好</h3><p className="muted">减少动画时取消位移与循环装饰，保留状态反馈。</p><div className="motion-options">{(['system', 'reduce'] as const).map((option) => <button key={option} aria-pressed={motion === option} className={motion === option ? 'selected' : ''} onClick={() => setMotion(option)}>{option === 'system' ? '跟随系统动画' : '减少动画'}</button>)}</div></div><div className="settings-card"><h3>窗口与工程</h3><p className="pair-mode-description">{pair.label}{state?.pairLayout?.reason && ` · ${state.pairLayout.reason}`}</p><p className="muted">两个独立窗口全贴合，尺寸联动；共同最大化时 ArkDock 保持右侧。解除绑定会停止自有预览资源。</p><button disabled={!state?.target || busy} onClick={async () => { if (await act('detach', {})) navigate(null); }}>解除当前绑定</button>{state?.project && <button onClick={() => navigate('path')}>管理工程信任</button>}</div></>}
      {panel === 'menu' && <div className="menu-list"><button onClick={() => navigate('projects')}><Icon name="folder" />选择当前工程<Icon name="chevron" size={16} /></button><button disabled={!state?.target || busy} onClick={async () => { navigate(null); await act('rediscover', {}); }}><Icon name="refresh" />重新识别</button><button disabled={!connected || busy} onClick={async () => { if (await act('select_directory', {})) navigate(stateRef.current?.phase === 'untrusted' ? 'trust' : null); }}><Icon name="folder" />手动选择目录</button><button onClick={() => navigate('controls')}><Icon name="preview" />预览能力</button><button onClick={() => navigate('sdk')}><Icon name="device" />设备与原生工具链</button><button onClick={() => navigate('diagnostics')}><Icon name="log" />运行日志</button><button onClick={() => navigate('settings')}><Icon name="settings" />偏好设置</button><button onClick={() => navigate('help')}><Icon name="link" />使用帮助</button><button disabled={!connected || busy || !pair.enabled} title={pair.reason} onClick={async () => { if (await act(pair.command, {})) navigate(null); }}><Icon name="maximize" />{pair.action}</button><button disabled={!connected || busy} onClick={() => void act('minimize_window', {})}><Icon name="monitor" />最小化窗口</button><button className="danger-button" disabled={!state?.target || busy} onClick={async () => { if (await act('detach', {})) navigate(null); }}><Icon name="close" />解除当前绑定</button></div>}
      {panel === 'controls' && <PreviewCapabilities snapshot={state} store={store} busy={busy} refresh={async () => { if (await act('refresh_preview', {})) navigate(null); }} />}
      {panel === 'diagnostics' && <><div className="diagnostic-actions"><span className="muted">最近 {state?.diagnostics.length ?? 0} 条记录</span><button onClick={() => void copy(diagnosticText(state), '脱敏诊断')}>复制脱敏诊断</button></div><p className="info-note">{state?.backend.reason ?? '桌面后端未连接。'}</p>{state?.build.error && <p className="build-error">{state.build.error}</p>}{state?.diagnostics.length ? state.diagnostics.map((item, index) => <article key={`${item.observedAt}-${index}`}><div><span className={`log-status ${item.status.toLowerCase()}`}>{item.status}</span><strong>{item.code}</strong><time>{new Date(item.observedAt).toLocaleTimeString('zh-CN')}</time></div><p>{item.message}</p><small>{item.workspaceId ?? '—'} / {item.buildId ?? '—'}</small></article>) : <div className="no-results"><Icon name="log" size={30} /><p>暂无诊断记录，编译错误总数未知。</p></div>}</>}
      {panel === 'help' && <div className="settings-card"><h3>从正在开发的窗口开始</h3><ol className="help-steps"><li>按住顶部标识或拖动区，靠近开发窗口右侧。</li><li>收到目标提示后松开；独立窗口由原生层贴边。</li><li>可靠工程直接关联；线索不足时确认候选，未发现时选择目录。</li><li>确认工程信任，再检查工具链，等待真实原生首帧。</li></ol><p className="muted">可随时重新识别、切换工程或解除绑定。只读识别目前不能中途取消；解除绑定使会话失效。</p></div>}
    </Sheet>}
    {(error || feedback) && <div className={`toast ${error ? 'error' : ''}`} role={error ? 'alert' : 'status'}><span>{error ?? feedback}</span><button className="icon-button" disabled={Boolean(panel)} aria-label="关闭提示" onClick={() => { setError(null); setFeedback(null); }}><Icon name="close" size={16} /></button></div>}
  </main>;
}
