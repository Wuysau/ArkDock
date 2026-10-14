import type { AppSnapshot, Candidate } from '../ipc/model.ts';
import { candidateCategoryNames, candidateCategoryOrder, candidateView } from './candidates.ts';
import type { CandidateCategory } from './candidates.ts';
import { Icon } from '../Icon.tsx';

/**
 * Candidate groups. The row only renders what `candidateView` decided: a single
 * reliable candidate is marked, a suggestion stays explicitly confirmable, a
 * history entry never looks like the current workspace, and an invalid or
 * unsupported project can never be associated from here.
 */
export function CandidateList({ groups, snapshot, now, busy, select }: {
  groups: Record<CandidateCategory, Candidate[]>; snapshot: AppSnapshot | null; now: number; busy: boolean; select: (id: string) => void;
}) {
  return <>{candidateCategoryOrder.filter((category) => groups[category].length > 0).map((category) => (
    <section className="candidate-group" key={category} aria-label={candidateCategoryNames[category]}>
      <h3>{candidateCategoryNames[category]} · {groups[category].length}</h3>
      <ul className="candidate-list">{groups[category].map((candidate) =>
        <CandidateItem key={candidate.id} candidate={candidate} snapshot={snapshot} now={now} busy={busy} select={select} />)}</ul>
    </section>
  ))}</>;
}

function CandidateItem({ candidate, snapshot, now, busy, select }: {
  candidate: Candidate; snapshot: AppSnapshot | null; now: number; busy: boolean; select: (id: string) => void;
}) {
  const view = candidateView(candidate, snapshot, now);
  return <li className={`candidate ${view.reliable ? 'reliable' : ''}`}><button data-candidate-id={candidate.id} disabled={busy || !view.selectable} onClick={() => select(candidate.id)}>
    <span className="candidate-heading"><span className="candidate-icon"><Icon name="folder" /></span><strong>{candidate.project.name}</strong><span className={`badge ${view.selectable ? '' : 'invalid'}`}>{view.badge}</span><Icon name="chevron" size={16} /></span>
    <code className="path">{view.appRoot}</code><span>{view.environment} · {candidate.project.module} · {candidate.project.variant}</span><span>工作区：{view.workspaceRoot}</span>
    <span className="evidence-tags">{view.sources.map((source) => <span key={source}>{source}</span>)}</span><span className="candidate-reason">{view.reason}</span>
  </button></li>;
}
