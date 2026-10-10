import type { PreviewFrame } from './frame-types.ts';
import type { DecodedImage } from './frame-decoder.ts';

export interface PresenterOptions<T extends DecodedImage> {
  decode: (frame: PreviewFrame) => Promise<T>;
  current: (frame: PreviewFrame) => boolean;
  draw: (image: T, frame: PreviewFrame, decodeMs: number) => void;
  clear: () => void;
  error?: (error: unknown) => void;
  now?: () => number;
}

/** At most one decode plus one pending latest frame and one retained displayed bitmap. */
export class FramePresenter<T extends DecodedImage> {
  private pending: PreviewFrame | null = null;
  private displayed: { image: T; frame: PreviewFrame } | null = null;
  private inFlight: PreviewFrame | null = null;
  private decoding = false;
  private disposed = false;
  private paused = false;
  private epoch = 0;
  private options: PresenterOptions<T>;
  readonly metrics = { decoded: 0, discarded: 0, failures: 0, presented: 0 };
  constructor(options: PresenterOptions<T>) { this.options = options; }
  submit(frame: PreviewFrame | null) {
    if (this.disposed) return;
    if (!frame) { this.reset(); return; }
    if (frame === this.pending || frame === this.inFlight || frame === this.displayed?.frame) return;
    if (this.pending) this.metrics.discarded++;
    this.pending = frame;
    if (!this.paused) void this.pump();
  }
  setPaused(paused: boolean) { this.paused = paused; if (!paused) void this.pump(); }
  replay() { if (!this.disposed && !this.paused && this.displayed) this.options.draw(this.displayed.image, this.displayed.frame, 0); }
  private reset() {
    this.epoch++;
    this.pending = null;
    this.displayed?.image.close();
    this.displayed = null;
    this.options.clear();
  }
  dispose() { if (this.disposed) return; this.disposed = true; this.reset(); }
  private async pump() {
    if (this.decoding || this.disposed || this.paused) return;
    this.decoding = true;
    try {
      while (this.pending && !this.disposed && !this.paused) {
        const frame = this.pending, epoch = this.epoch;
        this.pending = null;
        if (!this.options.current(frame)) { this.metrics.discarded++; continue; }
        const start = (this.options.now ?? performance.now.bind(performance))();
        let image: T;
        this.inFlight = frame;
        try { image = await this.options.decode(frame); this.metrics.decoded++; }
        catch (error) { this.inFlight = null; this.metrics.failures++; this.options.error?.(error); continue; }
        this.inFlight = null;
        if (this.disposed || this.paused || epoch !== this.epoch || !this.options.current(frame)) { image.close(); this.metrics.discarded++; continue; }
        try {
          this.options.draw(image, frame, (this.options.now ?? performance.now.bind(performance))() - start);
          const previous = this.displayed;
          this.displayed = { image, frame };
          previous?.image.close();
          this.metrics.presented++;
        } catch (error) { image.close(); this.metrics.failures++; this.options.error?.(error); }
      }
    } finally { this.decoding = false; }
  }
}
