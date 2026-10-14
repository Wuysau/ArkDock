import type { AppSnapshot, Candidate, Evidence, EvidenceSource, ProjectRef } from '../ipc/model.ts';
import { displayPath } from '../path-display.ts';

export const environments: Record<ProjectRef['environment'], string> = {
  windowsLocal: 'Windows 本地', wsl: 'WSL · 预览未支持', remote: '远程 · 预览未支持',
};

/** Unknown future environment tokens fall back to the raw backend token. */
export function environmentLabel(environment: ProjectRef['environment']): string {
  return environments[environment] ?? String(environment);
}

export type CandidateCategory = 'current' | 'suggested' | 'manual' | 'history';
export const candidateCategoryOrder = ['current', 'suggested', 'manual', 'history'] as const;
export const candidateCategoryNames: Record<CandidateCategory, string> = {
  current: '当前工作区工程候选', suggested: '建议候选 · 需要确认',
  manual: '指定目录 · 验证结果', history: '历史工程 · 不代表当前工作区',
};
export const evidenceSourceNames: Record<EvidenceSource, string> = {
  hostWorkspace: '宿主工作区接口', codexProjectMetadata: 'Codex 项目元数据', registration: '已登记工程线索',
  uia: '窗口标签线索', processArguments: '进程启动参数', history: '历史关联记录',
  title: '窗口标题 · 路径待确认', manual: '手动指定目录',
};

const workspaceSources: ReadonlyArray<EvidenceSource> = ['hostWorkspace', 'codexProjectMetadata', 'registration', 'uia', 'processArguments'];
export const WORKSPACE_OBSERVATION_MS = 15_000;
const EVIDENCE_MAX_WINDOW_MS = 120_000;

/** A workspace observation is only displayable inside its short validity window. */
export function observationAge(snapshot: AppSnapshot | null, now: number): boolean {
  const observation = snapshot?.workspaceObservation;
  return Boolean(observation && observation.observedAt <= now && now - observation.observedAt < WORKSPACE_OBSERVATION_MS);
}

/**
 * Temporary frontend judgement: the backend does not yet report candidate
 * display semantics, so evidence freshness is still evaluated here. This is the
 * only place allowed to do it — the panel renders `candidateView` output and
 * never re-derives trust level itself. P3-04 replaces these two functions with
 * the backend-provided level without touching the panel.
 */
function currentEvidence(evidence: Evidence, snapshot: AppSnapshot | null, now: number): boolean {
  const target = snapshot?.target;
  return Boolean(target && evidence.source !== 'history' && evidence.source !== 'manual'
    && evidence.targetId === target.id && evidence.process?.pid === target.process.pid
    && evidence.process.startedAt === target.process.startedAt
    && evidence.observedAt <= now && now < evidence.expiresAt
    && evidence.expiresAt > evidence.observedAt && evidence.expiresAt - evidence.observedAt <= EVIDENCE_MAX_WINDOW_MS);
}

function categoryOf(candidate: Candidate, snapshot: AppSnapshot | null, now: number): CandidateCategory {
  const observation = snapshot?.workspaceObservation;
  const fresh = !observation || (observation.status === 'verified' && observationAge(snapshot, now));
  const current = candidate.evidence.filter((evidence) => currentEvidence(evidence, snapshot, now));
  if (fresh && current.some((evidence) => workspaceSources.includes(evidence.source)
    && evidence.verifiable && evidence.activeScope && evidence.environment === 'windowsLocal'
    && candidate.project.environment === 'windowsLocal'
    && Boolean(evidence.sessionId && evidence.sessionId.length <= 128))) return 'current';
  if (current.length) return 'suggested';
  if (candidate.evidence.some((evidence) => evidence.source === 'manual')) return 'manual';
  return candidate.evidence.some((evidence) => evidence.source === 'history') ? 'history' : 'suggested';
}

export interface CandidateView {
  category: CandidateCategory;
  /** A single reliable candidate may bypass explicit confirmation. */
  reliable: boolean;
  /** Whether the row can actually be associated at all. */
  selectable: boolean;
  badge: string;
  reason: string;
  sources: string[];
  environment: string;
  appRoot: string;
  workspaceRoot: string;
}

export function candidateView(candidate: Candidate, snapshot: AppSnapshot | null, now: number): CandidateView {
  const category = categoryOf(candidate, snapshot, now);
  const supported = candidate.project.environment === 'windowsLocal';
  const reliable = category === 'current' && candidate.autoEligible && candidate.valid && supported;
  const badge = !candidate.valid ? '目录不可用' : !supported ? '环境不支持'
    : category === 'history' ? '历史工程' : reliable ? '证据可靠' : '需要确认';
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

/**
 * Grouping only. Order stays the backend order inside each category; the panel
 * owns nothing but labels, descriptions, filtering and the user's click.
 */
export function groupCandidates(candidates: Candidate[], snapshot: AppSnapshot | null, now: number): Record<CandidateCategory, Candidate[]> {
  const groups: Record<CandidateCategory, Candidate[]> = { current: [], suggested: [], manual: [], history: [] };
  for (const candidate of candidates) groups[categoryOf(candidate, snapshot, now)].push(candidate);
  return groups;
}
