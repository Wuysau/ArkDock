import type { AppSnapshot, DeviceRuntimeState } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';

export function deviceCapabilityContext(state: AppSnapshot | null): string | null {
  return state?.target && state.project && state.trusted ? JSON.stringify([state.target.id, state.project.id,
    state.project.sdkId, state.backend.sdkPath, state.trusted]) : null;
}

interface Options {
  bridge: Pick<UiBridge, 'transfer'>;
  getSnapshot: () => AppSnapshot | null;
  change: (state: DeviceRuntimeState | null) => void;
  gate?: { pending: boolean };
  clock?: { schedule: (callback: () => void, delay: number) => ReturnType<typeof setTimeout>; cancel: (handle: ReturnType<typeof setTimeout>) => void };
}

/** Read-only device capability polling. One request at a time, including across visibility restarts. */
export function observeDeviceCapabilities(options: Options): () => void {
  const key = deviceCapabilityContext(options.getSnapshot());
  if (!key || !options.bridge.transfer) return () => {};
  const gate = options.gate ?? { pending: false };
  const clock = options.clock ?? { schedule: (callback, delay) => setTimeout(callback, delay), cancel: (handle) => clearTimeout(handle) };
  let live = true, timer: ReturnType<typeof setTimeout> | null = null;
  const current = () => live && key === deviceCapabilityContext(options.getSnapshot());
  const schedule = () => { if (current()) timer = clock.schedule(() => { timer = null; void poll(); }, 1000); };
  async function poll() {
    if (!current()) return;
    if (gate.pending) { schedule(); return; }
    gate.pending = true;
    const snapshot = options.getSnapshot()!;
    try {
      const result = await options.bridge.transfer!('device_runtime_state', {}, { targetId: snapshot.target!.id, sessionId: snapshot.sessionId });
      if (current()) options.change(result.identity?.workspaceId === snapshot.project!.id ? result : null);
    } catch { if (current()) options.change(null); }
    finally { gate.pending = false; schedule(); }
  }
  void poll();
  return () => { live = false; if (timer !== null) clock.cancel(timer); };
}
