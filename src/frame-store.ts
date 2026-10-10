import type { AppSnapshot } from './ipc/model.ts';
import type { PreviewFrame } from './preview/frame-types.ts';
import { acceptFrame, frameBelongsToCurrentBuild, frameMetadataValid } from './view-state.ts';

export interface FrameStore {
  getSnapshot: () => PreviewFrame | null;
  getDisplayed: () => PreviewFrame | null;
  subscribeDisplayed: (listener: () => void) => () => void;
  markDisplayed: (frame: PreviewFrame) => boolean;
  isPresentable: (frame: PreviewFrame) => boolean;
  subscribe: (listener: () => void) => () => void;
  push: (state: AppSnapshot, frame: PreviewFrame) => void;
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
  let current: PreviewFrame | null = null;
  let displayed: PreviewFrame | null = null;
  let scheduled: number | null = null;
  let disposed = false;
  const listeners = new Set<() => void>();
  const displayedListeners = new Set<() => void>();
  function notifyDisplayed() { for (const listener of displayedListeners) listener(); }
  function notify() {
    if (disposed || scheduled !== null || listeners.size === 0) return;
    scheduled = scheduler.requestFrame(() => {
      scheduled = null;
      if (!disposed) for (const listener of listeners) listener();
    });
  }
  function clear() {
    if (current === null && displayed === null) return;
    current = null;
    displayed = null;
    notifyDisplayed();
    notify();
  }
  function sync(state: AppSnapshot | null) {
    if (disposed || (state && snapshot && state.revision < snapshot.revision)) return;
    const changed = identity(snapshot) !== identity(state);
    snapshot = state;
    if (changed || !state?.target || !state.sessionId || !state.project || !state.trusted || state.phase === 'stopped' || state.phase === 'crashed') clear();
  }
  function isPresentable(frame: PreviewFrame) {
    return !disposed && current === frame && frameBelongsToCurrentBuild(snapshot, frame) && !snapshot?.build.error
      && (snapshot?.phase === 'running' || (snapshot?.phase === 'building' && snapshot.build.durationMs !== null));
  }
  return {
    getSnapshot: () => current,
    getDisplayed: () => displayed,
    subscribeDisplayed: (listener) => { if (disposed) return () => undefined; displayedListeners.add(listener); return () => { displayedListeners.delete(listener); }; },
    isPresentable,
    markDisplayed: (frame) => {
      if (!isPresentable(frame)) return false;
      if (displayed === frame) return true;
      displayed = frame;
      notifyDisplayed();
      return true;
    },
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
      displayed = null;
      snapshot = null;
      listeners.clear();
      displayedListeners.clear();
    },
  };
}
