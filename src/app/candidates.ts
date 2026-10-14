import type { AppSnapshot, Candidate, CandidateCategory, EvidenceSource, ProjectRef } from '../ipc/model.ts';
import { displayPath } from '../path-display.ts';

export type { CandidateCategory } from '../ipc/model.ts';

export const environments: Record<ProjectRef['environment'], string> = {
  windowsLocal: 'Windows 本地', wsl: 'WSL · 预览未支持', remote: '远程 · 预览未支持',
};

/** Unknown future environment tokens fall back to the raw backend token. */
export function environmentLabel(environment: ProjectRef['environment']): string {
  return environments[environment] ?? String(environment);
}

export const candidateCategoryOrder: ReadonlyArray<CandidateCategory> = ['current', 'suggested', 'manual', 'history'];
export const candidateCategoryNames: Record<CandidateCategory, string> = {
  current: '当前工作区工程候选', suggested: '建议候选 · 需要确认',
  manual: '指定目录 · 验证结果', history: '历史工程 · 不代表当前工作区',
};
/** Labels only: the evidence sources stay whatever the backend reported. */
export const evidenceSourceNames: Record<EvidenceSource, string> = {
  hostWorkspace: '宿主工作区接口', codexProjectMetadata: 'Codex 项目元数据', registration: '已登记工程线索',
  uia: '窗口标签线索', processArguments: '进程启动参数', history: '历史关联记录',
  title: '窗口标题 · 路径待确认', manual: '手动指定目录',
};

/** Mirrors `discovery::WORKSPACE_OBSERVATION_TTL_MS`; used for the observation sentence only. */
export const WORKSPACE_OBSERVATION_MS = 15_000;
const categories = new Set<string>(candidateCategoryOrder);

/**
 * The backend owns candidate display semantics (`classification.rs` assigns
 * `category` and `categoryExpiresAt` on every published snapshot). The frontend
 * only labels, groups and orders what it received: it never inspects evidence
 * freshness, target identity or process lifetime to decide a trust level. A
 * candidate without the contract field is downgraded to `suggested`, so a
 * partially built or older payload can never look like a verified workspace.
 */
function categoryOf(candidate: Candidate): CandidateCategory {
  return categories.has(String(candidate.category)) ? candidate.category : 'suggested';
}

/** A workspace observation is only displayable inside its short validity window. */
export function observationAge(snapshot: AppSnapshot | null, now: number): boolean {
  const observation = snapshot?.workspaceObservation;
  return Boolean(observation && observation.observedAt <= now && now - observation.observedAt < WORKSPACE_OBSERVATION_MS);
}

/**
 * The earliest instant at which the display may move. `categoryExpiresAt` is the
 * backend's own report, and the backend republishes as soon as a category really
 * changes; this only arms a re-render for the observation sentence so a stale
 * path is never presented as the current one.
 */
export function nextDisplayDeadline(snapshot: AppSnapshot | null, now: number): number | null {
  if (!snapshot) return null;
  const deadlines = snapshot.candidates.map((candidate) => candidate.categoryExpiresAt).filter((deadline) => deadline > now);
  if (snapshot.workspaceObservation) deadlines.push(snapshot.workspaceObservation.observedAt + WORKSPACE_OBSERVATION_MS);
  const next = Math.min(...deadlines);
  return Number.isFinite(next) ? next : null;
}

export interface CandidateView {
  category: CandidateCategory;
  /** The backend's unique auto-associable candidate; the only "reliable" row. */
  reliable: boolean;
  /** Whether the row can be associated at all; trust still lives in the backend. */
  selectable: boolean;
  badge: string;
  reason: string;
  sources: string[];
  environment: string;
  appRoot: string;
  workspaceRoot: string;
}

export function candidateView(candidate: Candidate): CandidateView {
  const category = categoryOf(candidate);
  const supported = candidate.project.environment === 'windowsLocal';
  const reliable = category === 'current' && candidate.autoEligible && candidate.valid && supported;
  const badge = !candidate.valid ? '目录不可用' : !supported ? '环境不支持'
    : category === 'history' ? '历史工程' : reliable ? '证据可靠' : category === 'current' ? '当前工作区 · 需要确认' : '需要确认';
  const reason = !candidate.valid ? candidate.reasons.join('；') || '目录不可访问；需要重新选择有效工程。'
    : !supported ? `${environmentLabel(candidate.project.environment)}；当前不提供原生预览。`
      : category === 'history' ? '这是之前关联过的工程；当前窗口的活动工作区尚需另行核验。'
        : category === 'manual' ? '指定目录已执行只读检查。确认这是当前工程后，点击此项关联。'
          : candidate.reasons.join('；') || '请选择与目标窗口对应的实际工程。';
  return {
    category, reliable, selectable: candidate.valid && supported, badge, reason,
    // Missing or future label fields degrade to nothing rather than rendering blanks.
    sources: Array.from(new Set(candidate.evidence.map((item) => evidenceSourceNames[item.source]).filter((label): label is string => Boolean(label)))),
    environment: environmentLabel(candidate.project.environment),
    appRoot: displayPath(candidate.project.appRoot),
    workspaceRoot: displayPath(candidate.project.workspaceRoot),
  };
}

/** Grouping only, in the backend's own order: it already ranks eligibility and validity. */
export function groupCandidates(candidates: Candidate[]): Record<CandidateCategory, Candidate[]> {
  const groups: Record<CandidateCategory, Candidate[]> = { current: [], suggested: [], manual: [], history: [] };
  for (const candidate of candidates) groups[categoryOf(candidate)].push(candidate);
  return groups;
}
