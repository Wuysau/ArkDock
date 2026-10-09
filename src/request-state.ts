import type { AppSnapshot, TargetInfo } from './ipc/model.ts';

export function sameTarget(a: TargetInfo | null | undefined, b: TargetInfo | null | undefined): boolean {
  return !a || !b ? !a && !b : a.id === b.id && a.process.pid === b.process.pid && a.process.startedAt === b.process.startedAt;
}

export function responseStillCurrent(result: AppSnapshot, current: AppSnapshot | null): boolean {
  return Boolean(current && sameTarget(result.target, current.target) && result.sessionId === current.sessionId
    && result.project?.id === current.project?.id);
}

export function hasDockCandidate(hint: string | null | undefined): boolean {
  return Boolean(hint?.startsWith('Release to bind '));
}
