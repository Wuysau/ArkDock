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

// Pixel updates bypass React. Layout only changes when the surface or build identity changes.
function frameLayoutKey(frame: PreviewFrame | null): string {
  return frame ? JSON.stringify([frame.sessionId, frame.buildId, frame.generation, frame.width, frame.height, frame.rotation]) : '';
}

export function PreviewStage({ snapshot, store, zoom, busy, onClick, interactionMode = 'interact', overlay, visible = true }: PreviewStageProps) {
  const receivedKey = useSyncExternalStore(store.subscribe, () => frameLayoutKey(store.getSnapshot()), () => '');
  const displayedKey = useSyncExternalStore(store.subscribeDisplayed, () => frameLayoutKey(store.getDisplayed()), () => '');
  const displayed = displayedKey ? store.getDisplayed() : null;
  const frame = displayed ?? (receivedKey ? store.getSnapshot() : null);
  const viewport = useRef<HTMLDivElement>(null);
  const shell = useRef<HTMLDivElement>(null);
  const [available, setAvailable] = useState({ width: 0, height: 0, horizontalInset: 0, verticalInset: 0 });
  const ui = deriveUiState(snapshot, displayed);
  useEffect(() => {
    const element = viewport.current;
    const device = shell.current;
    if (!element || !device) return;
    const observer = new ResizeObserver((entries) => {
      const bounds = entries.find((entry) => entry.target === element)?.contentRect;
      const style = getComputedStyle(device);
      const length = (value: string) => Number.parseFloat(value) || 0;
      const horizontalInset = length(style.paddingLeft) + length(style.paddingRight) + length(style.borderLeftWidth) + length(style.borderRightWidth);
      const verticalInset = length(style.paddingTop) + length(style.paddingBottom) + length(style.borderTopWidth) + length(style.borderBottomWidth);
      setAvailable((current) => {
        const width = bounds?.width ?? current.width, height = bounds?.height ?? current.height;
        return current.width === width && current.height === height && current.horizontalInset === horizontalInset && current.verticalInset === verticalInset
          ? current : { width, height, horizontalInset, verticalInset };
      });
    });
    observer.observe(element);
    observer.observe(device);
    return () => observer.disconnect();
  }, []);
  // The measured shell inset follows CSS tokens; fit applies one uniform scale to the actual frame.
  const scale = frame ? zoom === 'fit' ? Math.min(Math.max(0, available.width - available.horizontalInset) / frame.width,
    Math.max(0, available.height - available.verticalInset) / frame.height) : zoom : 0;
  const canClick = visible && scale > 0 && ui.inputAllowed && !busy && interactionMode === 'interact';
  const stale = Boolean(frame && (!ui.frameCurrent || ui.preview === 'stale'));
  return <div ref={viewport} className={`frame-viewport${zoom !== 'fit' ? ' is-zoomed' : ''}`} data-testid="preview-stage">
      <div className="frame-scroll" style={{ visibility: frame ? undefined : 'hidden' }} aria-hidden={!frame || undefined}>
        <div ref={shell} className={`device-shell${stale ? ' is-stale' : ''}`}>
          <div className="native-surface-wrap" style={{ position: 'relative', width: (frame?.width ?? 0) * scale, height: (frame?.height ?? 0) * scale }}>
          <button className="native-surface" data-testid="native-screen"
            aria-label={frame ? `实际原生画面，构建 ${frame.buildId}${canClick ? '；Enter 点击画面中心' : '；当前不可交互'}` : '等待有效原生画面'}
            title={canClick ? '点击原生画面；Enter 点击中心' : ui.capabilities.click.reason}
            disabled={!frame || (!canClick && interactionMode !== 'inspect')}
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
            <NativeCanvas store={store} visible={visible && Boolean(frame)} />
          </button>
          {frame && interactionMode === 'inspect' && overlay}
          </div>
        </div>
      </div>
      {stale && <output className="frame-stale" data-testid="stale-frame">{snapshot?.build.error
        ? '旧画面 · 当前代码构建失败' : '旧画面 · 等待当前构建'} · 画面版本 {frame?.generation}</output>}
      {frame && !canClick && interactionMode === 'interact' && <span className="frame-input-note">{busy ? '请求处理中，暂缓输入' : ui.capabilities.click.reason}</span>}
      {!frame && <div className="frame-empty" />}
  </div>;
}
