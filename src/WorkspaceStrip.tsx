import { useId, useState } from 'react';
import type { AppSnapshot } from './ipc/model.ts';
import { deriveUiState } from './view-state.ts';
import { pairControl } from './pair-layout.ts';
import { dockingStatus, hasDockCandidate } from './request-state.ts';
import { Icon } from './Icon.tsx';
import { displayPath } from './path-display.ts';

const environments = { windowsLocal: 'Windows 本地', wsl: 'WSL · 预览未支持', remote: '远程 · 预览未支持' };
export function WorkspaceStrip({ snapshot, busy, cancelling, openProject, openCandidates, manual, rediscover, cancel, pairContext, revokeContext, selectContext }: {
  snapshot: AppSnapshot | null; busy: boolean; cancelling: boolean;
  openProject: () => void; openCandidates: () => void; manual: () => void; rediscover: () => void; cancel: () => void;
  pairContext: () => void; revokeContext: () => void;
  selectContext: (registrationId: string) => void;
}) {
  // A view preference belongs to this running UI, not to a workspace/build/session key.
  const [expanded, setExpanded] = useState(false);
  const id = useId();
  const ui = deriveUiState(snapshot, null);
  const project = snapshot?.project;
  const context = snapshot?.contextSource;
  const pair = pairControl(snapshot);
  const working = snapshot?.phase === 'discovering' || snapshot?.phase === 'building';
  const failed = Boolean(snapshot?.build.error || snapshot?.phase === 'crashed');
  const unavailable = Boolean(project && (!snapshot?.trusted || !snapshot.backend.available || project.environment !== 'windowsLocal'));
  const baseStatus = snapshot?.build.error ? '当前构建失败' : snapshot?.phase === 'crashed' ? '后端已退出'
    : snapshot?.phase === 'sdkMissing' ? 'SDK 不可用' : snapshot?.phase === 'protocolBlocked' ? '已安装的 SDK 暂不受支持，请指定兼容的工具链'
      : project?.environment !== undefined && project.environment !== 'windowsLocal' ? '工程环境不支持'
        : project && !snapshot?.backend.available ? '工具链不可用' : snapshot?.phase === 'building' ? '正在构建' : ui.workspaceLabel;
  const status = project && !snapshot?.trusted && ui.workspaceLabel !== baseStatus ? `${baseStatus} · 等待信任` : baseStatus;
  const reason = snapshot?.build.error || (project && !snapshot?.backend.available ? snapshot?.backend.reason : null) || status;
  const docking = dockingStatus(snapshot?.dockHint);
  if (!snapshot?.target && docking) return <output className={`state-strip docking-status${hasDockCandidate(snapshot?.dockHint) ? ' working' : ''}`} aria-label="贴附状态">
    <Icon name="link" size={14} /><span title={docking.detail}>{docking.label}</span>
  </output>;
  return <section className="workspace-card" aria-label="当前工程" data-expanded={expanded}>
    <div className="workspace-compact">
      <button className="workspace-main" aria-label="工程" onClick={openProject} title={project ? `${project.name}\n${displayPath(project.appRoot)}` : '查看工程候选'}>
        <span className="workspace-icon"><Icon name="folder" size={17} /></span>
        <span><strong>{project?.name ?? '尚未关联工程'}</strong><span className={`workspace-status${failed ? ' is-error' : unavailable ? ' is-warning' : ''}`} title={reason}>
          {working ? <span className="task-spinner" aria-hidden="true" /> : <span className="dot" aria-hidden="true" />}{status}
        </span></span>
      </button>
      <button className="icon-button workspace-expand" aria-label={expanded ? '折叠工程信息' : '展开工程信息'} aria-expanded={expanded} aria-controls={`${id}-details`}
        title={expanded ? '折叠工程信息' : '展开工程信息与操作'} onClick={() => setExpanded(value => !value)}><Icon name="chevron" size={16} /></button>
    </div>
    <div id={`${id}-details`} className="workspace-details" aria-hidden={!expanded} {...(!expanded ? { inert: '' } : {})}>
      <div className="workspace-details-clip"><div className="workspace-details-content">
        {project && <><span className="workspace-path">{environments[project.environment]} · {project.module} · {project.variant}</span>
          <code className="path" title={displayPath(project.appRoot)}>{displayPath(project.appRoot)}</code></>}
        <output className={`state-strip ${working ? 'working' : ''}`}><Icon name={snapshot?.trusted ? 'shield' : 'link'} size={14} />
          <span title={reason}>{hasDockCandidate(snapshot?.dockHint) ? '松开以连接 · 靠近目标外侧' : snapshot?.dockHint ?? (snapshot?.target ? ui.workspaceLabel : '独立悬浮 · 拖到开发窗口旁开始')}</span>
          {snapshot?.target && <span className={`pair-badge ${pair.pending ? 'pending' : ''}`} title={pair.reason}>
            {pair.state === 'maximized' ? '共同最大化' : pair.pending ? '布局调整中' : pair.state === 'blocked' ? '布局受限' : pair.state === 'paired' ? '全贴合' : '等待布局'}
          </span>}
        </output>
        {context && <output className={`state-strip${context.paired ? '' : ' idle'}`} aria-label="Agent 会话源">
          <Icon name="link" size={14} />
          <span>{context.paired ? `Agent 已配对 · ${context.activeSessions} 个连接` : '连接 Agent 后可从上报的工程中选择'}</span>
        </output>}
        {context?.paired && <p className="workspace-path">当前接入需要确认工程，不会自动跟随对话切换。</p>}
        <div className="context-projects">{context?.registrations.map(entry => <div key={entry.id} className="context-project" aria-label="Agent 上报的工程">
          <span className="workspace-path">{context.boundRegistrationId === entry.id ? '已连接' : '待确认'} · {entry.agent ?? 'Agent'} · 会话 {entry.hostSession ?? entry.session}</span>
          <span className="workspace-path">原始项目</span><code className="path" title={entry.originalProject ?? undefined}>{entry.originalProject ? displayPath(entry.originalProject) : '宿主未提供'}</code>
          <span className="workspace-path">{entry.executionDirectory ? '声明的实际构建目录' : entry.hintSource === 'roots' ? '宿主建议目录（不代表当前对话）' : '启动目录提示（不代表当前对话）'} · {environments[entry.environment]}</span>
          <code className="path" title={entry.executionDirectory ?? entry.suggestedDirectory ?? undefined}>{entry.executionDirectory || entry.suggestedDirectory ? displayPath(entry.executionDirectory ?? entry.suggestedDirectory!) : '未指定唯一目录，请在对话中连接当前工程'}</code>
          <button disabled={busy || !snapshot?.target || !(entry.executionDirectory || entry.suggestedDirectory) || entry.environment !== 'windowsLocal' || context.boundRegistrationId === entry.id}
            onClick={() => selectContext(entry.id)} title={!snapshot?.target ? '先贴附到目标窗口' : '核对目录后用于当前预览；执行仍需工程信任'}>{context.boundRegistrationId === entry.id ? '已连接此工程' : entry.executionDirectory ? '连接此工程' : '确认使用此建议'}</button>
        </div>)}</div>
        {unavailable && <p className="workspace-problem">{reason}</p>}
        <div className="workspace-detail-actions">
          <button disabled={!snapshot?.target || busy} onClick={rediscover} title={!snapshot?.target ? '先连接开发窗口' : '重新读取目标工程线索'}><Icon name="refresh" size={14} />重新识别</button>
          <button onClick={openCandidates}>切换工程</button><button disabled={!snapshot || busy} onClick={manual}>手动选择</button>
          {context?.paired
            ? <button disabled={busy} onClick={revokeContext} title="撤销配对并清除全部已登记会话；已连接的 Agent 下一次心跳即失效">撤销会话源</button>
            : <button disabled={busy} onClick={pairContext} title="生成配对文件，按接入说明配置 Agent 后可上报工程">配对会话源</button>}
          {snapshot?.phase === 'discovering' && <button disabled={cancelling} title="解除绑定，使当前关联会话失效；只读检查等待后端返回" onClick={cancel}>{cancelling ? '取消中…' : '取消关联'}</button>}
        </div>
      </div></div>
    </div>
  </section>;
}
