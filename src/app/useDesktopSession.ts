import { useCallback, useEffect, useRef, useState } from 'react';
import type { MutableRefObject } from 'react';
import type { AppSnapshot } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';
import { applySnapshot } from '../view-state.ts';
import { createFrameStore } from '../frame-store.ts';
import type { FrameStore } from '../frame-store.ts';
import { BinaryFrameController } from '../preview/BinaryFrameController.ts';

export interface SnapshotTransition {
  previous: AppSnapshot | null;
  updated: AppSnapshot;
}

export interface DesktopSession {
  state: AppSnapshot | null;
  stateRef: MutableRefObject<AppSnapshot | null>;
  /** Single latest-frame mailbox shared by every preview surface. */
  store: FrameStore;
  frameController: MutableRefObject<BinaryFrameController | null>;
  /** Commits a snapshot: revision gate, transition side effects, store sync. */
  receive: (value: AppSnapshot) => void;
  connected: boolean;
  nativeReady: boolean;
  nativeReadyRef: MutableRefObject<boolean>;
  /** True between the first received snapshot and the first preference write. */
  initialSnapshotApplied: MutableRefObject<boolean>;
  /** Native observation of the owned main window visibility. */
  previewVisible: boolean;
}

/**
 * Owns the desktop subscription, the binary frame controller and the committed
 * snapshot. Lifecycle rules stay where they were: an older revision never
 * replaces a newer one, a target change is reported once to the caller, and the
 * subscription, frame lease and visibility listener are released together.
 */
export function useDesktopSession({ bridge, onTransition, onError }: {
  bridge: UiBridge;
  onTransition: (change: SnapshotTransition) => void;
  onError: (reason: unknown) => void;
}): DesktopSession {
  const [state, setState] = useState<AppSnapshot | null>(null);
  const stateRef = useRef<AppSnapshot | null>(null);
  const [store] = useState(createFrameStore);
  const frameController = useRef<BinaryFrameController | null>(null);
  const [previewVisible, setPreviewVisible] = useState(true);
  const previewVisibleRef = useRef(true);
  const [nativeReady, setNativeReady] = useState(false);
  const nativeReadyRef = useRef(false);
  const initialSnapshotApplied = useRef(false);
  const handlers = useRef({ onTransition, onError });
  handlers.current = { onTransition, onError };
  const connected = bridge.available;

  const receive = useCallback((value: AppSnapshot) => {
    const previous = stateRef.current;
    const updated = applySnapshot(previous, value);
    if (updated === previous) return;
    handlers.current.onTransition({ previous, updated });
    stateRef.current = updated;
    store.sync(updated);
    setState(updated);
    frameController.current?.update(updated);
  }, [store]);

  useEffect(() => {
    let live = true;
    let unsubscribe: (() => void) | undefined;
    nativeReadyRef.current = false;
    initialSnapshotApplied.current = false;
    setNativeReady(false);
    const controller = bridge.readFrame ? new BinaryFrameController({
      bridge: { request: bridge.request, readFrame: bridge.readFrame }, store,
      getState: () => stateRef.current, onSnapshot: receive,
      onMode: (_mode, reason) => { if (live && reason) handlers.current.onError(reason); },
    }) : null;
    frameController.current = controller;
    controller?.update(stateRef.current);
    // WebView2 document.hidden can disagree with the visible owned main window.
    // The native visibility observation controls frame delivery; keyboard input
    // additionally requires local document focus and the current native scope.
    const visibility = () => controller?.setVisible(previewVisibleRef.current);
    visibility();
    document.addEventListener('visibilitychange', visibility);
    if (connected) bridge.observe(
      (value) => { if (live) receive(value); },
      (frame) => { if (live && stateRef.current) { if (controller) controller.acceptLegacy(frame); else store.push(stateRef.current, frame); } },
      (visible) => { if (live && previewVisibleRef.current !== visible) { previewVisibleRef.current = visible; setPreviewVisible(visible); visibility(); } },
    ).then(async (off) => {
      if (!live) { off(); return; }
      unsubscribe = off;
      const snapshot = await bridge.request('snapshot', {});
      if (!live) return;
      receive(snapshot);
      initialSnapshotApplied.current = true;
      nativeReadyRef.current = true;
      setNativeReady(true);
    }).catch((reason: unknown) => { if (live) handlers.current.onError(reason); });
    return () => {
      live = false;
      unsubscribe?.();
      document.removeEventListener('visibilitychange', visibility);
      controller?.dispose();
      if (frameController.current === controller) frameController.current = null;
      store.clear();
    };
  }, [bridge, connected, store, receive]);

  return { state, stateRef, store, frameController, receive, connected, nativeReady, nativeReadyRef, initialSnapshotApplied, previewVisible };
}
