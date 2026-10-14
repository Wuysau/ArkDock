import type { AppSnapshot } from '../ipc/model.ts';
import { filterCandidates } from '../view-state.ts';
import { displayPath } from '../path-display.ts';
import { groupCandidates, observationAge } from './candidates.ts';
import { CandidateList } from './CandidateList.tsx';
import { Icon } from '../Icon.tsx';

/**
 * Project association. Discovery stays read-only: the panel only shows what the
 * backend already verified, offers an explicit directory fallback, and asks the
 * user to confirm which candidate is the current workspace.
 */
export function ProjectsPanel({ snapshot, connected, busy, now, search, onSearch, directoryPath, onDirectoryPath, inspectDirectory, chooseDirectory, rediscover, select }: {
  snapshot: AppSnapshot | null; connected: boolean; busy: boolean; now: number;
  search: string; onSearch: (value: string) => void;
  directoryPath: string; onDirectoryPath: (value: string) => void;
  inspectDirectory: () => void; chooseDirectory: () => void; rediscover: () => void; select: (id: string) => void;
}) {
  // Candidate reliability is time bound: the display clock is armed by the caller
  // from the backend's own deadlines, and the panel re-labels from `now`.
  const candidates = filterCandidates(snapshot?.candidates ?? [], search);
  const grouped = groupCandidates(candidates);
  const observation = snapshot?.workspaceObservation;
  const observationFresh = Boolean(snapshot?.target) && observationAge(snapshot, now);
  const activePath = observationFresh && observation?.status === 'verified' ? observation.activePath : null;
  const suggestedName = observationFresh && observation?.status === 'suggested' ? observation.projectName : null;
  return <>
    <p className="muted">确认与目标窗口对应的实际工程。关联不授予构建权限。</p>
    <section className="workspace-observation" aria-label="当前活动工作区">
      <div className="results-summary"><h3>当前活动工作区</h3><button disabled={!snapshot?.target || busy} onClick={rediscover}>{snapshot?.phase === 'discovering' ? '正在识别…' : '重新识别'}</button></div>
      <strong>{activePath ? observation?.projectName ?? '已可靠识别' : '尚未可靠识别'}</strong>
      {activePath && <code className="path">{displayPath(activePath)}</code>}
      {suggestedName && <p className="muted">项目名称线索：{suggestedName}。名称不能证明当前路径或活动归属。</p>}
      <p className="muted">{observationFresh ? observation?.reason : observation ? '活动工作区证据已过期，请重新识别。' : '目标窗口尚未提供可靠的当前工作区路径。'}</p>
      {!activePath && <p className="muted">建议操作：重新识别或选择工程目录。Codex 用户主动打开项目信息后，可再次识别其中可访问的项目元数据。</p>}
    </section>
    <div className="directory-fallback">
      <button className="primary-button" disabled={!connected || busy} onClick={chooseDirectory}><Icon name="folder" size={16} />手动选择工程目录</button>
      <form className="directory-input" onSubmit={(event) => { event.preventDefault(); inspectDirectory(); }}>
        <label htmlFor="project-directory">或输入绝对工程目录</label><input id="project-directory" aria-label="绝对工程目录" value={directoryPath} onChange={(event) => onDirectoryPath(event.target.value)} placeholder="输入完整目录路径" autoComplete="off" spellCheck={false} />
        <button disabled={!connected || busy || !directoryPath.trim()} type="submit">只读验证目录</button><p className="muted">验证只读取指定目录；验证结果需另行选择后才关联。</p>
      </form>
    </div>
    <label className="search-box"><Icon name="search" size={17} /><input data-autofocus aria-label="筛选已列出的工程候选" value={search} onChange={(event) => onSearch(event.target.value)} placeholder="筛选候选：工程、路径或模块" /></label>
    <div className="results-summary"><span>工作区候选 {grouped.current.length} · 建议 {grouped.suggested.length} · 指定目录 {grouped.manual.length} · 历史 {grouped.history.length}{search ? '（筛选后）' : ''}</span></div>
    <CandidateList groups={grouped} busy={busy} select={select} />
    {!candidates.length && <div className="no-results"><Icon name="folder" size={30} /><h3>{search ? '没有匹配的工程候选' : '尚无可选择的鸿蒙工程'}</h3><p>{search ? '筛选仅作用于已列出的候选。指定目录请使用上方独立输入框。' : activePath ? '已取得工作区路径，但尚未确认可预览的实际鸿蒙工程。可指定应用根目录执行只读验证。' : '当前窗口没有提供可靠工程路径。可重新识别或手动指定目录。'}</p></div>}
  </>;
}
