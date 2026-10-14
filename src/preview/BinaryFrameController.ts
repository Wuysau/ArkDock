import type { AppSnapshot, NativeFrame, RequestScope } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';
import { describeError, normalizeError } from '../ipc/errors.ts';
import type { FrameStore } from '../frame-store.ts';
import { FRAME_PROTOCOL_VERSION, parseBinaryFrame } from './frame-decoder.ts';

export type FrameTransportMode = 'idle' | 'negotiating' | 'binary' | 'legacy' | 'blocked';
interface Clock {
  schedule: (callback: () => void, delay: number) => ReturnType<typeof setTimeout>;
  cancel: (handle: ReturnType<typeof setTimeout>) => void;
}
interface Options {
  bridge: Pick<UiBridge, 'request' | 'readFrame'>;
  store: FrameStore;
  getState: () => AppSnapshot | null;
  onSnapshot?: (state: AppSnapshot) => void;
  onMode?: (mode: FrameTransportMode, reason?: string) => void;
  clock?: Clock;
  consumerId?: string;
}
interface Lease { scope: RequestScope; identity: string; consumerId: string; acknowledged: boolean; closed: boolean }
const unsupported = (error: unknown) => normalizeError(error).kind === 'unsupported';
const identity = (state: AppSnapshot | null) => state?.target && state.project && state.sessionId && state.trusted && state.backend.available
  && state.phase !== 'stopped' && state.phase !== 'crashed' ? JSON.stringify([state.target.id, state.target.process.pid,
    state.target.process.startedAt, state.project.id, state.project.environment, state.project.workspaceRoot, state.project.appRoot,
    state.project.module, state.project.variant, state.sessionId]) : null;
const readable = (state: AppSnapshot | null) => Boolean(state && (state.phase === 'running'
  || (state.phase === 'building' && state.build.durationMs !== null)) && !state.build.error);

/** One acknowledged mailbox owner; serial pulls at <= 30 Hz, idle backoff to 250 ms. */
export class BinaryFrameController {
  private lease: Lease | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private disposed = false;
  private visible = true;
  private pulling = false;
  private emptyReads = 0;
  private mode: FrameTransportMode = 'idle';
  private clock: Clock;
  private token: string;
  private leaseSerial = 0;
  private options: Options;
  readonly metrics = { reads: 0, emptyReads: 0, accepted: 0, discarded: 0, rejected: 0, errors: 0 };
  constructor(options: Options) {
    this.options = options;
    // WebView2 timer functions require their Window receiver; do not call them as clock methods.
    this.clock = options.clock ?? { schedule: (callback, delay) => setTimeout(callback, delay), cancel: (handle) => clearTimeout(handle) };
    this.token = options.consumerId ?? crypto.randomUUID();
  }
  getMode() { return this.mode; }
  private change(mode: FrameTransportMode, reason?: string) { this.mode = mode; this.options.onMode?.(mode, reason); }
  update(state: AppSnapshot | null) {
    if (this.disposed) return;
    const next = identity(state);
    if (next === this.lease?.identity) { if (!readable(state)) this.cancelTimer(); else this.schedule(); return; }
    const previous = this.lease;
    this.cancelTimer(); this.lease = null;
    if (previous) { previous.closed = true; if (previous.acknowledged) void this.disable(previous); }
    this.emptyReads = 0;
    if (!next || !state) { this.change('idle'); return; }
    const lease: Lease = { scope: { targetId: state.target!.id, sessionId: state.sessionId }, identity: next,
      consumerId: `${this.token.slice(0, 96)}:${++this.leaseSerial}`, acknowledged: false, closed: false };
    this.lease = lease;
    if (!this.options.bridge.readFrame) { this.change('legacy', '桌面桥接未提供二进制读取能力'); return; }
    this.change('negotiating');
    void this.enable(lease);
  }
  acceptLegacy(frame: NativeFrame) {
    const state = this.options.getState();
    if (!this.disposed && this.mode === 'legacy' && state && this.lease?.identity === identity(state)) this.options.store.push(state, frame);
  }
  setVisible(visible: boolean) { this.visible = visible; if (!visible) this.cancelTimer(); else this.wake(); }
  /** A confirmed user input can shorten idle backoff without creating a competing reader. */
  wake() { this.emptyReads = 0; this.cancelTimer(); this.schedule(); }
  dispose() {
    if (this.disposed) return;
    this.disposed = true; this.cancelTimer();
    const lease = this.lease; this.lease = null;
    if (lease) { lease.closed = true; if (lease.acknowledged) void this.disable(lease); }
    this.change('idle');
  }
  private async enable(lease: Lease) {
    try {
      const state = await this.options.bridge.request('enable_binary_preview_frames', { protocolVersion: FRAME_PROTOCOL_VERSION, consumerId: lease.consumerId }, lease.scope);
      lease.acknowledged = true;
      if (this.disposed || lease.closed || this.lease !== lease) { await this.disable(lease); return; }
      this.options.onSnapshot?.(state);
      if (this.lease !== lease || lease.closed) return;
      this.change('binary'); this.schedule();
    } catch (error) {
      if (this.disposed || this.lease !== lease || lease.closed) return;
      this.metrics.errors++;
      this.change(unsupported(error) ? 'legacy' : 'blocked', describeError(error));
    }
  }
  private async disable(lease: Lease) {
    try { await this.options.bridge.request('disable_binary_preview_frames', { protocolVersion: FRAME_PROTOCOL_VERSION, consumerId: lease.consumerId }, lease.scope); }
    catch { /* Scope and lease may already have been replaced. Never disable a successor. */ }
  }
  private cancelTimer() { if (this.timer !== null) this.clock.cancel(this.timer); this.timer = null; }
  private schedule() {
    if (this.disposed || !this.visible || this.mode !== 'binary' || this.timer !== null || this.pulling || !this.lease) return;
    if (!readable(this.options.getState())) return;
    const delay = Math.min(250, (1000 / 30) * 1.6 ** Math.min(this.emptyReads, 5));
    this.timer = this.clock.schedule(() => { this.timer = null; void this.pull(); }, delay);
  }
  private async pull() {
    const lease = this.lease;
    if (this.disposed || !this.visible || !lease || this.mode !== 'binary' || !this.options.bridge.readFrame || !readable(this.options.getState())) return;
    this.pulling = true;
    try {
      this.metrics.reads++;
      const packet = await this.options.bridge.readFrame(lease.scope, FRAME_PROTOCOL_VERSION, lease.consumerId);
      const state = this.options.getState();
      if (this.disposed || !this.visible || this.lease !== lease || lease.closed || !state || identity(state) !== lease.identity) { this.metrics.discarded++; return; }
      if (!packet.byteLength) { this.emptyReads++; this.metrics.emptyReads++; return; }
      this.emptyReads = 0;
      let frame;
      try { frame = parseBinaryFrame(packet, state, this.options.store.getSnapshot()); }
      catch { this.metrics.rejected++; return; }
      if (!frame) { this.metrics.discarded++; return; }
      this.options.store.push(state, frame);
      if (this.options.store.getSnapshot() === frame) this.metrics.accepted++; else this.metrics.discarded++;
    } catch { this.emptyReads = 5; this.metrics.errors++; }
    finally { this.pulling = false; this.schedule(); }
  }
}
