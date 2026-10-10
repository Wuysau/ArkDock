import { memo, useEffect, useRef } from 'react';
import type { FrameStore } from '../frame-store.ts';
import { decodePreviewFrame } from './frame-decoder.ts';
import { FramePresenter } from './frame-presenter.ts';
import { isBinaryFrame } from './frame-types.ts';

/** A persistent native pixel surface. Decode completes before replacing the old image. */
export const NativeCanvas = memo(function NativeCanvas({ store, visible = true }: { store: FrameStore; visible?: boolean }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const presenterRef = useRef<FramePresenter<ImageBitmap> | null>(null);
  const visibleRef = useRef(visible);
  visibleRef.current = visible;
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    const context = canvas.getContext('2d', { alpha: false });
    if (!context) return;
    function dimensions() {
      const bounds = canvas!.getBoundingClientRect();
      const ratio = Math.max(1, window.devicePixelRatio || 1);
      const width = Math.max(1, Math.round(bounds.width * ratio)), height = Math.max(1, Math.round(bounds.height * ratio));
      const reduction = Math.min(1, 8192 / width, 8192 / height, Math.sqrt((16 * 1024 * 1024) / (width * height)));
      return { width: Math.max(1, Math.floor(width * reduction)), height: Math.max(1, Math.floor(height * reduction)) };
    }
    const presenter = new FramePresenter<ImageBitmap>({
      decode: decodePreviewFrame,
      current: (frame) => visibleRef.current && !document.hidden && store.isPresentable(frame),
      draw: (image, frame, decodeMs) => {
        const size = dimensions();
        // Setting dimensions resets pixels; redraw the retained valid bitmap in the same task.
        if (canvas.width !== size.width || canvas.height !== size.height) { canvas.width = size.width; canvas.height = size.height; }
        context.drawImage(image, 0, 0, canvas.width, canvas.height);
        const firstPresentation = store.getDisplayed() !== frame;
        if (firstPresentation && !store.markDisplayed(frame)) return;
        canvas.dataset.frameSequence = String(frame.sequence);
        canvas.dataset.buildId = frame.buildId;
        canvas.dataset.sessionId = frame.sessionId;
        canvas.dataset.generation = String(frame.generation);
        canvas.dataset.format = isBinaryFrame(frame) ? 'jpeg' : 'png';
        canvas.dataset.renderStatus = 'displayed';
        canvas.setAttribute('aria-label', `实际原生画面，构建 ${frame.buildId}`);
        if (firstPresentation) window.dispatchEvent(new CustomEvent('arkdock:frame-presented', { detail: {
          sessionId: frame.sessionId, buildId: frame.buildId, generation: frame.generation, sequence: frame.sequence,
          capturedAt: frame.capturedAt, received: isBinaryFrame(frame) ? frame.receivedAt : null,
          decodeMs, format: isBinaryFrame(frame) ? 'jpeg' : 'png', payloadBytes: isBinaryFrame(frame) ? frame.payloadLength : frame.pngBase64.length,
          discarded: presenter.metrics.discarded, presented: presenter.metrics.presented + 1,
        } }));
      },
      clear: () => { context.clearRect(0, 0, canvas.width, canvas.height); canvas.dataset.renderStatus = 'empty'; delete canvas.dataset.frameSequence; delete canvas.dataset.buildId; },
      error: () => { canvas.dataset.renderStatus = 'decode-error'; },
    });
    presenterRef.current = presenter;
    const submit = () => presenter.submit(store.getSnapshot());
    const off = store.subscribe(submit);
    const observer = new ResizeObserver(() => presenter.replay());
    observer.observe(canvas);
    // Cross-monitor DPI changes may leave the CSS rectangle unchanged.
    let dpi = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
    const dpiChanged = () => {
      dpi.removeEventListener('change', dpiChanged);
      presenter.replay();
      dpi = window.matchMedia(`(resolution: ${window.devicePixelRatio}dppx)`);
      dpi.addEventListener('change', dpiChanged);
    };
    dpi.addEventListener('change', dpiChanged);
    const visibility = () => {
      const paused = !visibleRef.current || document.hidden;
      if (!paused) submit();
      presenter.setPaused(paused);
      if (!paused) presenter.replay();
    };
    document.addEventListener('visibilitychange', visibility);
    presenter.setPaused(!visibleRef.current || document.hidden);
    submit();
    return () => { off(); observer.disconnect(); dpi.removeEventListener('change', dpiChanged); document.removeEventListener('visibilitychange', visibility); presenter.dispose(); if (presenterRef.current === presenter) presenterRef.current = null; canvas.width = 1; canvas.height = 1; };
  }, [store]);
  useEffect(() => {
    const presenter = presenterRef.current;
    if (!presenter) return;
    const paused = !visible || document.hidden;
    if (!paused) presenter.submit(store.getSnapshot());
    presenter.setPaused(paused);
    if (!paused) presenter.replay();
  }, [visible, store]);
  // The native bitmap must use a stable Canvas; expose its image semantics to assistive technology.
  // oxlint-disable-next-line jsx-a11y/prefer-tag-over-role
  return <canvas ref={ref} className="native-frame" data-testid="native-canvas" role="img" aria-label="等待有效原生画面" />;
});
