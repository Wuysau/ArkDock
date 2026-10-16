import type { FrameStore } from '../frame-store.ts';
import type { BinaryFrameController, FrameTransportMode } from './BinaryFrameController.ts';

export interface InputPerformanceMetrics {
  receiptDurationMs: readonly number[];
  dispatched: number;
  coalescedMoves: number;
}
export interface DurationSummary { count: number; p50: number | null; p95: number | null }
export interface PreviewPerformanceSample {
  active: boolean;
  pointer: DurationSummary;
  binaryRead: DurationSummary;
  presentedFps: number | null;
  coalescedMoves: number | null;
  discardedReads: number | null;
  transportMode: FrameTransportMode | null;
}
export type PerformanceFrameController = Pick<BinaryFrameController, 'metrics' | 'readDurationMs' | 'getMode'>;

/** Nearest-rank percentiles of at most 120 recent valid samples; no invented zero. */
export function summarizeDurations(samples: readonly number[]): DurationSummary {
  const sorted = samples.slice(-120).filter((value) => Number.isFinite(value) && value >= 0).sort((a, b) => a - b);
  return { count: sorted.length, p50: sorted.length ? sorted[Math.ceil(sorted.length * 0.5) - 1] : null,
    p95: sorted.length ? sorted[Math.ceil(sorted.length * 0.95) - 1] : null };
}

interface MonitorOptions {
  store: Pick<FrameStore, 'subscribeDisplayed' | 'getDisplayed'>;
  getInput: () => InputPerformanceMetrics | null;
  getTransport: () => PerformanceFrameController | null;
  sample: (value: PreviewPerformanceSample) => void;
  clock?: { now: () => number; schedule: (callback: () => void, delay: number) => ReturnType<typeof setTimeout>; cancel: (handle: ReturnType<typeof setTimeout>) => void };
}

/** Frame notifications only increment a scalar. React receives one sample per second. */
export function observePreviewPerformance(options: MonitorOptions): () => void {
  const clock = options.clock ?? { now: () => performance.now(), schedule: (callback, delay) => setTimeout(callback, delay), cancel: (handle) => clearTimeout(handle) };
  let previousFrame = options.store.getDisplayed(), presented = 0, started = clock.now(), live = true;
  let timer: ReturnType<typeof setTimeout>;
  const off = options.store.subscribeDisplayed(() => {
    if (!live) return;
    const frame = options.store.getDisplayed();
    if (frame && frame !== previousFrame) presented++;
    previousFrame = frame;
  });
  function publish(initial: boolean) {
    const input = options.getInput(), transport = options.getTransport(), now = clock.now();
    const elapsed = now - started;
    options.sample({ active: true, pointer: summarizeDurations(input?.receiptDurationMs ?? []),
      binaryRead: summarizeDurations(transport?.readDurationMs ?? []),
      presentedFps: !initial && elapsed > 0 && options.store.getDisplayed() ? presented * 1000 / elapsed : null,
      coalescedMoves: input?.coalescedMoves ?? null,
      discardedReads: transport ? transport.metrics.discarded + transport.metrics.rejected : null,
      transportMode: transport?.getMode() ?? null });
    presented = 0; started = now;
  }
  function tick() { if (!live) return; publish(false); timer = clock.schedule(tick, 1000); }
  publish(true); timer = clock.schedule(tick, 1000);
  return () => { live = false; off(); clock.cancel(timer); };
}
