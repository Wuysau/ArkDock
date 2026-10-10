import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { ReactNode } from 'react';
import type { AppSnapshot } from './ipc/model.ts';
import type { FrameStore } from './frame-store.ts';
import { deriveUiState, mapFramePoint } from './view-state.ts';
import { NativeCanvas } from './preview/NativeCanvas.tsx';
import type { PreviewFrame } from './preview/frame-types.ts';

export interface PreviewStageProps {
  snapshot: AppSnapshot | null;
  store: FrameStore;
  zoom: number | 'fit';
  busy: boolean;
  onClick: (frame: PreviewFrame, x: number, y: number) => void;
  interactionMode?: 'interact' | 'inspect';
  overlay?: ReactNode;
  visible?: boolean;
}

export function PreviewStage({ snapshot, store, zoom, busy, onClick, interactionMode = 'interact', overlay, visible = true }: PreviewStageProps) {
  const received = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  const displayed = useSyncExternalStore(store.subscribeDisplayed, store.getDisplayed, store.getDisplayed);
  const frame = displayed ?? received;
  const viewport = useRef<HTMLDivElement>(null);
  const [available, setAvailable] = useState({ width: 0, height: 0 });
  const ui = deriveUiState(snapshot, displayed);
  useEffect(() => {
    const element = viewport.current;
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => {
      if (!entry) return;
      const { width, height } = entry.contentRect;
      setAvailable((current) => current.width === width && current.height === height ? current : { width, height });
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  // The shell occupies 12 DIP on each axis. Fit uses actual viewport bounds and never distorts pixels.
  const scale = frame ? zoom === 'fit' ? Math.min(Math.max(1, available.width - 12) / frame.width,
    Math.max(1, available.height - 12) / frame.height) : zoom : 1;
  const canClick = visible && ui.inputAllowed && !busy && interactionMode === 'interact';
  const stale = Boolean(frame && (!ui.frameCurrent || ui.preview === 'stale'));
  return <div ref={viewport} className={`frame-viewport${zoom !== 'fit' ? ' is-zoomed' : ''}`} data-testid="preview-stage">
    {frame ? <>
      <div className="frame-scroll">
        <div className={`device-shell${stale ? ' is-stale' : ''}`}>
          <div className="native-surface-wrap" style={{ position: 'relative', width: frame.width * scale, height: frame.height * scale }}>
          <button className="native-surface" data-testid="native-screen"
            aria-label={`实际原生画面，构建 ${frame.buildId}${canClick ? '；Enter 点击画面中心' : '；当前不可交互'}`}
            title={canClick ? '点击原生画面；Enter 点击中心' : ui.capabilities.click.reason}
            disabled={!canClick && interactionMode !== 'inspect'}
            onKeyDown={(event) => {
              if (!canClick || (event.key !== 'Enter' && event.key !== ' ')) return;
              event.preventDefault();
              const actual = store.getDisplayed();
              if (actual && deriveUiState(snapshot, actual).inputAllowed) onClick(actual, Math.floor(actual.width / 2), Math.floor(actual.height / 2));
            }} onClick={(event) => {
              if (!canClick) return;
              // Assistive activation has no pointer location; key handlers prevent the browser's duplicate click.
              const actual = store.getDisplayed();
              if (!actual || !deriveUiState(snapshot, actual).inputAllowed) return;
              if (event.detail === 0) { onClick(actual, Math.floor(actual.width / 2), Math.floor(actual.height / 2)); return; }
              const image = event.currentTarget.querySelector('canvas');
              if (!image) return;
              const bounds = image.getBoundingClientRect();
              const point = mapFramePoint(actual, bounds, event.clientX, event.clientY);
              if (point) onClick(actual, point.x, point.y);
            }}>
            <NativeCanvas store={store} visible={visible} />
          </button>
          {interactionMode === 'inspect' && overlay}
          </div>
        </div>
      </div>
      {stale && <output className="frame-stale" data-testid="stale-frame">{snapshot?.build.error
        ? '旧画面 · 当前代码构建失败' : '旧画面 · 等待当前构建'} · 画面版本 {frame.generation}</output>}
      {!canClick && <span className="frame-input-note">{busy ? '请求处理中，暂缓输入' : ui.capabilities.click.reason}</span>}
    </> : <div className="frame-empty" />}
  </div>;
}
