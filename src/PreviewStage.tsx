import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { AppSnapshot, NativeFrame } from './ipc/model.ts';
import type { FrameStore } from './frame-store.ts';
import { deriveUiState, mapFramePoint } from './view-state.ts';

export interface PreviewStageProps {
  snapshot: AppSnapshot | null;
  store: FrameStore;
  zoom: number | 'fit';
  busy: boolean;
  onClick: (frame: NativeFrame, x: number, y: number) => void;
}

export function PreviewStage({ snapshot, store, zoom, busy, onClick }: PreviewStageProps) {
  const frame = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  const viewport = useRef<HTMLDivElement>(null);
  const [available, setAvailable] = useState({ width: 0, height: 0 });
  const ui = deriveUiState(snapshot, frame);
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
  const canClick = ui.inputAllowed && !busy;
  const stale = Boolean(frame && (!ui.frameCurrent || ui.preview === 'stale'));
  return <div ref={viewport} className={`frame-viewport${zoom !== 'fit' ? ' is-zoomed' : ''}`} data-testid="preview-stage">
    {frame ? <>
      <div className="frame-scroll">
        <div key={frame.sessionId} className={`device-shell first-frame${stale ? ' is-stale' : ''}`}>
          <button className="native-surface" data-testid="native-screen"
            aria-label={`实际原生画面，构建 ${frame.buildId}${canClick ? '；Enter 点击画面中心' : '；当前不可交互'}`}
            title={canClick ? '点击原生画面；Enter 点击中心' : ui.capabilities.click.reason}
            disabled={!canClick} style={{ width: frame.width * scale, height: frame.height * scale }}
            onKeyDown={(event) => {
              if (!canClick || (event.key !== 'Enter' && event.key !== ' ')) return;
              event.preventDefault();
              onClick(frame, Math.floor(frame.width / 2), Math.floor(frame.height / 2));
            }} onClick={(event) => {
              if (!canClick) return;
              // Assistive activation has no pointer location; key handlers prevent the browser's duplicate click.
              if (event.detail === 0) { onClick(frame, Math.floor(frame.width / 2), Math.floor(frame.height / 2)); return; }
              const image = event.currentTarget.querySelector('img');
              if (!image) return;
              const bounds = image.getBoundingClientRect();
              const point = mapFramePoint(frame, bounds, event.clientX, event.clientY);
              if (point) onClick(frame, point.x, point.y);
            }}>
            <img className="native-frame" src={`data:image/png;base64,${frame.pngBase64}`}
              alt={`实际原生画面，构建 ${frame.buildId}`} draggable={false} />
          </button>
        </div>
      </div>
      {stale && <output className="frame-stale" data-testid="stale-frame">{snapshot?.build.error
        ? '旧画面 · 当前代码构建失败' : '旧画面 · 等待当前构建'} · 画面版本 {frame.generation}</output>}
      {!canClick && <span className="frame-input-note">{busy ? '请求处理中，暂缓输入' : ui.capabilities.click.reason}</span>}
    </> : <div className="frame-empty" />}
  </div>;
}
