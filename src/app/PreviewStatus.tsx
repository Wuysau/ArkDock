import { useSyncExternalStore } from 'react';
import type { AppSnapshot } from '../ipc/model.ts';
import type { FrameStore } from '../frame-store.ts';
import { deriveUiState } from '../view-state.ts';
import { hasDockCandidate } from '../request-state.ts';
import { Icon } from '../Icon.tsx';
import type { Panel } from './navigation.ts';

/**
 * Build and runtime status. Frame identity is read from the displayed mailbox, so
 * a new-but-not-yet-drawn frame cannot change this footer, and the compact
 * variant is the only status the chrome shows while collapsed.
 */
export function BuildStatusBar({ snapshot, store, pending, open, compact = false }: {
  snapshot: AppSnapshot | null; store: FrameStore; pending: string | null; open?: () => void; compact?: boolean;
}) {
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

/** Shown only while no current frame exists; it never covers a real native frame. */
export function EmptyState({ snapshot, store, connected, navigate }: {
  snapshot: AppSnapshot | null; store: FrameStore; connected: boolean; navigate: (panel: Panel) => void;
}) {
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
