import type { AppSnapshot, RequestScope } from './ipc/model.ts';
import { createScopedPreferenceSync } from './motion.ts';
import { sameTarget } from './request-state.ts';
import { targetLifecycleKey } from './dock-ripple.ts';

type OrbPreferenceSnapshot = Pick<AppSnapshot, 'target' | 'sessionId' | 'display'>;
export function nativeOrbPreferenceChanged(previous: OrbPreferenceSnapshot | null, incoming: OrbPreferenceSnapshot): boolean {
  return Boolean(previous && sameTarget(previous.target, incoming.target) && previous.sessionId === incoming.sessionId
    && previous.display.orbAutoHide !== incoming.display.orbAutoHide);
}

/** Opacity is a percent where 100 keeps the Orb fully drawn; the floor stays locatable. */
export const orbOpacityFloor = 20;
export const orbOpacityTransparencyCeiling = 100 - orbOpacityFloor;

export function parseOrbOpacity(value: string | null): number | null {
  if (value === null) return null;
  const opacity = Number(value);
  return Number.isInteger(opacity) && opacity >= orbOpacityFloor && opacity <= 100 && String(opacity) === value ? opacity : null;
}

export function readOrbOpacity(): number | null {
  try { return parseOrbOpacity(localStorage.getItem('arkdock:orb-opacity')); }
  catch { return null; }
}

export function saveOrbOpacity(opacity: number): void {
  try { localStorage.setItem('arkdock:orb-opacity', String(opacity)); }
  catch { /* The native preference still works without storage. */ }
}

export function parseOrbAutoHide(value: string | null): boolean | null {
  return value === 'true' ? true : value === 'false' ? false : null;
}

export function readOrbAutoHide(): boolean | null {
  try { return parseOrbAutoHide(localStorage.getItem('arkdock:orb-auto-hide')); }
  catch { return null; }
}

export function saveOrbAutoHide(enabled: boolean): void {
  try { localStorage.setItem('arkdock:orb-auto-hide', enabled ? 'true' : 'false'); }
  catch { /* The native preference still works without storage. */ }
}

export interface OrbAutoHideSyncJob { scope: RequestScope; targetKey: string; enabled: boolean }

/** A successful current write may be overtaken by a newer native menu confirmation. */
export function settledOrbChoice(job: OrbAutoHideSyncJob, result: OrbPreferenceSnapshot & Pick<AppSnapshot, 'revision'>,
  current: (OrbPreferenceSnapshot & Pick<AppSnapshot, 'revision'>) | null): boolean | null {
  if (!current || current.revision < result.revision || targetLifecycleKey(current) !== job.targetKey
    || (current.target?.id ?? null) !== job.scope.targetId || current.sessionId !== job.scope.sessionId
    || result.sessionId !== job.scope.sessionId || !sameTarget(current.target, result.target)) return null;
  return current.display.orbAutoHide;
}

export function createOrbAutoHidePreferenceSync<Result>(callbacks: {
  send: (job: OrbAutoHideSyncJob) => Promise<Result>;
  accept: (job: OrbAutoHideSyncJob, result: Result) => void;
  fail: (job: OrbAutoHideSyncJob, error: unknown) => void;
}) {
  return createScopedPreferenceSync<OrbAutoHideSyncJob, Result>({
    ...callbacks,
    key: (job) => JSON.stringify([job.targetKey, job.scope.sessionId, job.enabled]),
  });
}

export interface OrbOpacitySyncJob { scope: RequestScope; targetKey: string; opacity: number }

/** Serializes opacity writes so a drag release cannot queue a burst of redraws. */
export function createOrbOpacityPreferenceSync<Result>(callbacks: {
  send: (job: OrbOpacitySyncJob) => Promise<Result>;
  accept: (job: OrbOpacitySyncJob, result: Result) => void;
  fail: (job: OrbOpacitySyncJob, error: unknown) => void;
}) {
  return createScopedPreferenceSync<OrbOpacitySyncJob, Result>({
    ...callbacks,
    key: (job) => JSON.stringify([job.targetKey, job.scope.sessionId, job.opacity]),
  });
}
