import type { MotionIntensity, MotionSettings, RequestScope } from './ipc/model.ts';

export type MotionPreference = 'system' | 'reduce';
export function readMotion(): MotionPreference {
  try { return localStorage.getItem('arkdock:motion') === 'reduce' ? 'reduce' : 'system'; }
  catch { return 'system'; }
}
export function readMotionIntensity(): MotionIntensity {
  try {
    const value = localStorage.getItem('arkdock:motion-intensity');
    return value === 'off' || value === 'light' || value === 'enhanced' ? value : 'standard';
  } catch { return 'standard'; }
}

export function motionSettings(preference: MotionPreference, intensity: MotionIntensity, systemReduced: boolean): MotionSettings {
  return { reduced: preference === 'reduce' || systemReduced, intensity };
}

export function applyMotion(preference: MotionPreference, intensity: MotionIntensity, systemReduced: boolean): void {
  const settings = motionSettings(preference, intensity, systemReduced);
  document.documentElement.dataset.motion = settings.reduced ? 'reduce' : 'system';
  document.documentElement.dataset.motionIntensity = intensity;
  if (settings.reduced || intensity === 'off' || intensity === 'light') cancelMotion();
  try {
    localStorage.setItem('arkdock:motion', preference);
    localStorage.setItem('arkdock:motion-intensity', intensity);
  } catch { /* Works without storage. */ }
}

export function cancelMotion(): void {
  // Changing a duration token does not interrupt a CSS transition already in flight.
  for (const animation of document.querySelector('.app')?.getAnimations({ subtree: true }) ?? []) animation.cancel();
}

export interface MotionSyncJob { scope: RequestScope; targetKey: string; preferences: MotionSettings }
function syncKey(job: MotionSyncJob): string {
  return JSON.stringify([job.targetKey, job.scope.sessionId, job.preferences.reduced, job.preferences.intensity]);
}

/** Serialize real native writes; a rapid preference change replaces the queued value. */
export function createMotionPreferenceSync<Result>(callbacks: {
  send: (job: MotionSyncJob) => Promise<Result>;
  accept: (job: MotionSyncJob, result: Result) => void;
  fail: (job: MotionSyncJob, error: unknown) => void;
}): { enqueue: (job: MotionSyncJob, retry?: boolean) => void; dispose: () => void; pending: () => boolean } {
  return createScopedPreferenceSync<MotionSyncJob, Result>({ ...callbacks, key: syncKey });
}

export function createScopedPreferenceSync<Job, Result>(callbacks: {
  key: (job: Job) => string;
  send: (job: Job) => Promise<Result>;
  accept: (job: Job, result: Result) => void;
  fail: (job: Job, error: unknown) => void;
}): { enqueue: (job: Job, retry?: boolean) => void; dispose: () => void; pending: () => boolean } {
  let desired: Job | null = null;
  let completed: string | null = null;
  let running = false;
  let disposed = false;
  async function drain() {
    if (running || disposed) return;
    running = true;
    try {
      while (!disposed && desired && callbacks.key(desired) !== completed) {
        const job = desired;
        const key = callbacks.key(job);
        try {
          const result = await callbacks.send(job);
          if (!disposed && desired && callbacks.key(desired) === key) callbacks.accept(job, result);
        } catch (error) {
          if (!disposed && desired && callbacks.key(desired) === key) callbacks.fail(job, error);
        }
        // A failed write waits for a new preference/scope rather than looping requests.
        completed = key;
      }
    } finally { running = false; }
  }
  return {
    enqueue(job, retry = false) {
      if (!disposed) {
        if (retry && !running) completed = null;
        desired = job; void drain();
      }
    },
    dispose() { disposed = true; desired = null; },
    pending() { return Boolean(!disposed && desired && callbacks.key(desired) !== completed); },
  };
}
