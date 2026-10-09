import type { AppSnapshot, NativeFrame } from './ipc/model.ts';
import { acceptFrame, frameMetadataValid } from './view-state.ts';

export interface FrameStore {
  getSnapshot: () => NativeFrame | null;
  subscribe: (listener: () => void) => () => void;
  push: (state: AppSnapshot, frame: NativeFrame) => void;
  sync: (state: AppSnapshot | null) => void;
  clear: () => void;
  dispose: () => void;
}
export interface FrameScheduler {
  requestFrame: (callback: () => void) => number;
  cancelFrame: (handle: number) => void;
}

const browserScheduler: FrameScheduler = {
  requestFrame: (callback) => typeof requestAnimationFrame === 'function'
    ? requestAnimationFrame(callback) : Number(globalThis.setTimeout(callback, 16)),
  cancelFrame: (handle) => typeof cancelAnimationFrame === 'function'
    ? cancelAnimationFrame(handle) : globalThis.clearTimeout(handle),
};

function identity(state: AppSnapshot | null): string | null {
  if (!state?.target || !state.project || !state.sessionId) return null;
  return JSON.stringify([state.sessionId, state.target.id, state.target.process.pid, state.target.process.startedAt,
    state.project.id, state.project.environment, state.project.workspaceRoot, state.project.appRoot, state.project.module, state.project.variant]);
}

/** One latest-frame mailbox; only PreviewStage subscribes to pixel changes. */
export function createFrameStore(scheduler: FrameScheduler = browserScheduler): FrameStore {
  let snapshot: AppSnapshot | null = null;
  let current: NativeFrame | null = null;
  let scheduled: number | null = null;
  let disposed = false;
  const listeners = new Set<() => void>();
  function notify() {
    if (disposed || scheduled !== null || listeners.size === 0) return;
    scheduled = scheduler.requestFrame(() => {
      scheduled = null;
      if (!disposed) for (const listener of listeners) listener();
    });
  }
  function clear() {
    if (current === null) return;
    current = null;
    notify();
  }
  function sync(state: AppSnapshot | null) {
    if (disposed || (state && snapshot && state.revision < snapshot.revision)) return;
    const changed = identity(snapshot) !== identity(state);
    snapshot = state;
    if (changed || !state?.target || !state.sessionId || !state.project || !state.trusted || state.phase === 'stopped') clear();
  }
  return {
    getSnapshot: () => current,
    subscribe: (listener) => {
      if (disposed) return () => undefined;
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
        if (listeners.size === 0 && scheduled !== null) { scheduler.cancelFrame(scheduled); scheduled = null; }
      };
    },
    push: (state, frame) => {
      if (disposed || (snapshot && state.revision < snapshot.revision)) return;
      sync(state);
      if (!snapshot?.target || !snapshot.project || !snapshot.sessionId || !snapshot.trusted || !snapshot.backend.available
        || (snapshot.phase !== 'running' && !(snapshot.phase === 'building' && snapshot.build.durationMs !== null))
        || snapshot.build.error || !frameMetadataValid(frame)) return;
      const accepted = acceptFrame(snapshot, current, frame);
      if (accepted === current) return;
      current = accepted;
      notify();
    },
    sync,
    clear,
    dispose: () => {
      disposed = true;
      if (scheduled !== null) scheduler.cancelFrame(scheduled);
      scheduled = null;
      current = null;
      snapshot = null;
      listeners.clear();
    },
  };
}
