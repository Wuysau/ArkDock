import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { ReactNode } from 'react';
import type { AppSnapshot, NativeControlState, PreviewKey } from './ipc/model.ts';
import type { FrameStore } from './frame-store.ts';
import { deriveUiState, mapFramePoint } from './view-state.ts';
import { NativeCanvas } from './preview/NativeCanvas.tsx';
import type { PreviewFrame } from './preview/frame-types.ts';
import { NativeTextCapture } from './preview/NativeTextCapture.tsx';
import { controlAvailable, displayedIdentity, identityEqual } from './preview/useNativeControls.ts';

export interface NativeStageInput {
  start: (frame: PreviewFrame, x: number, y: number) => boolean;
  move: (point: { x: number; y: number }) => void;
  end: (point: { x: number; y: number }) => Promise<boolean>;
  cancel: () => Promise<void>;
  text: (text: string) => void;
  key: (key: PreviewKey) => void;
}

export interface PreviewStageProps {
  snapshot: AppSnapshot | null;
  store: FrameStore;
  zoom: number | 'fit';
  busy: boolean;
  onClick: (frame: PreviewFrame, x: number, y: number) => void;
  interactionMode?: 'interact' | 'inspect';
  overlay?: ReactNode;
  visible?: boolean;
  controlState?: NativeControlState | null;
  nativeInput?: NativeStageInput;
  inputPaused?: boolean;
  inputError?: string | null;
}

// Pixel updates bypass React. Layout only changes when the surface or build identity changes.
function frameLayoutKey(frame: PreviewFrame | null): string {
  return frame ? JSON.stringify([frame.sessionId, frame.buildId, frame.generation, frame.width, frame.height, frame.rotation]) : '';
}

export function PreviewStage({ snapshot, store, zoom, busy, onClick, interactionMode = 'interact', overlay, visible = true, controlState, nativeInput, inputPaused = false, inputError = null }: PreviewStageProps) {
  const receivedKey = useSyncExternalStore(store.subscribe, () => frameLayoutKey(store.getSnapshot()), () => '');
  const displayedKey = useSyncExternalStore(store.subscribeDisplayed, () => frameLayoutKey(store.getDisplayed()), () => '');
  const displayed = displayedKey ? store.getDisplayed() : null;
  const frame = displayed ?? (receivedKey ? store.getSnapshot() : null);
  const viewport = useRef<HTMLDivElement>(null);
  const shell = useRef<HTMLDivElement>(null);
  const textInput = useRef<HTMLTextAreaElement>(null);
  const screen = useRef<HTMLButtonElement>(null);
  const pointer = useRef<{ id: number; frame: PreviewFrame; element: HTMLButtonElement; point: { x: number; y: number } } | null>(null);
  const suppressClick = useRef(false);
  const activation = useRef(0);
  const focusingScreen = useRef(false);
  const releasePending = useRef(false);
  const nativeInputRef = useRef(nativeInput); nativeInputRef.current = nativeInput;
  const [textState, setTextState] = useState<'idle' | 'waiting' | 'ready'>('idle');
  const [inputFeedback, setInputFeedback] = useState<string | null>(null);
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
  const configurationReady = !controlState?.experimentalEnabled || controlState.inputReady && identityEqual(controlState.identity, displayedIdentity(snapshot, displayed));
  const canClick = visible && scale > 0 && ui.inputAllowed && !busy && !inputPaused && configurationReady && interactionMode === 'interact';
  const streaming = Boolean(nativeInput && controlState?.experimentalEnabled && controlAvailable(controlState, 'touch'));
  const textEnabled = Boolean(canClick && nativeInput && controlState?.experimentalEnabled && controlAvailable(controlState, 'text'));
  const focusState = useRef({ snapshot, controlState, textEnabled, visible }); focusState.current = { snapshot, controlState, textEnabled, visible };
  const interactionKey = JSON.stringify([displayed?.sessionId, displayed?.buildId, displayed?.generation, displayed?.width, displayed?.height]);
  const releaseCapture = useCallback(() => {
    const active = pointer.current; pointer.current = null;
    if (active?.element.hasPointerCapture(active.id)) {
      try { active.element.releasePointerCapture(active.id); } catch { /* Native cancellation still runs if the WebView already lost capture. */ }
    }
  }, []);
  const cancelGesture = useCallback(() => {
    activation.current++; releasePending.current = false; releaseCapture();
    setTextState('idle');
    if (nativeInputRef.current) void nativeInputRef.current.cancel();
  }, [releaseCapture]);
  useEffect(() => { if (!canClick) { cancelGesture(); textInput.current?.blur(); } }, [canClick, cancelGesture]);
  useEffect(() => {
    const activations = activation, releases = releasePending;
    textInput.current?.blur(); setTextState('idle'); setInputFeedback(null);
    return () => {
      activations.current++; releases.current = false; releaseCapture();
      if (nativeInput) void nativeInput.cancel();
    };
  }, [interactionKey, nativeInput, releaseCapture]);
  useEffect(() => {
    const blur = () => { cancelGesture(); textInput.current?.blur(); };
    const hide = () => { if (!focusState.current.visible) { cancelGesture(); textInput.current?.blur(); } };
    window.addEventListener('blur', blur); document.addEventListener('visibilitychange', hide);
    return () => { window.removeEventListener('blur', blur); document.removeEventListener('visibilitychange', hide); };
  }, [cancelGesture]);
  function pointAt(element: HTMLButtonElement, actual: PreviewFrame, x: number, y: number, clamp = false) {
    const canvas = element.querySelector('canvas'); if (!canvas) return null;
    const bounds = canvas.getBoundingClientRect();
    if (bounds.width <= 0 || bounds.height <= 0) return null;
    return mapFramePoint(actual, bounds, clamp ? Math.max(bounds.left, Math.min(bounds.right - 0.001, x)) : x,
      clamp ? Math.max(bounds.top, Math.min(bounds.bottom - 0.001, y)) : y);
  }
  function focusText(element: HTMLButtonElement, expected: PreviewFrame): boolean {
    const latest = focusState.current;
    const identity = displayedIdentity(latest.snapshot, expected);
    if (latest.textEnabled && latest.controlState?.inputReady && identityEqual(identity, latest.controlState.identity)
      && identityEqual(identity, displayedIdentity(latest.snapshot, store.getDisplayed()))
      && document.hasFocus() && document.activeElement === element) {
      textInput.current?.focus({ preventScroll: true });
      return document.activeElement === textInput.current;
    }
    return false;
  }
  function finishGesture(element: HTMLButtonElement, expected: PreviewFrame, point: { x: number; y: number }) {
    if (!nativeInput) return;
    const serial = ++activation.current;
    releasePending.current = true;
    // end marks the gesture closing synchronously. Capture the first typed
    // character now; the input queue waits for this gesture's real Up ACK.
    const completion = nativeInput.end(point);
    const captured = focusText(element, expected);
    if (captured) setTextState('waiting');
    void completion.then((success) => {
      if (serial !== activation.current) return;
      releasePending.current = false;
      const identity = displayedIdentity(focusState.current.snapshot, expected);
      if (!identityEqual(identity, displayedIdentity(focusState.current.snapshot, store.getDisplayed()))) return;
      if (!success) {
        textInput.current?.blur(); setTextState('idle');
        setInputFeedback('原生点击未确认释放；等待中的文字不会提交，请重新点击输入框。');
      } else if (captured && document.hasFocus() && document.activeElement === textInput.current) {
        setTextState('ready'); setInputFeedback(null);
      }
    });
  }
  function startGesture(element: HTMLButtonElement, actual: PreviewFrame, point: { x: number; y: number }): boolean {
    // Focus before binding checks document.hasFocus, including the first click
    // after returning to this WebView. Do not move to the textarea while held.
    focusingScreen.current = true;
    try { element.focus({ preventScroll: true }); } finally { focusingScreen.current = false; }
    if (!nativeInput?.start(actual, point.x, point.y)) {
      setInputFeedback('本次点击未提交；请检查原生输入状态后重试。'); return false;
    }
    setInputFeedback(null); setTextState('idle');
    return true;
  }
  const stale = Boolean(frame && (!ui.frameCurrent || ui.preview === 'stale'));
  return <div ref={viewport} className={`frame-viewport${zoom !== 'fit' ? ' is-zoomed' : ''}`} data-testid="preview-stage">
      <div className="frame-scroll" style={{ visibility: frame ? undefined : 'hidden' }} aria-hidden={!frame || undefined}>
        <div ref={shell} className={`device-shell${stale ? ' is-stale' : ''}`}>
          <div className="native-surface-wrap" style={{ position: 'relative', width: (frame?.width ?? 0) * scale, height: (frame?.height ?? 0) * scale }}>
          <button ref={screen} className={`native-surface${streaming ? ' is-streaming' : ''}`} data-testid="native-screen"
            aria-label={frame ? `实际原生画面，构建 ${frame.buildId}${canClick ? streaming ? '；可点击、拖动和滑动' : '；Enter 点击画面中心' : '；当前不可交互'}` : '等待有效原生画面'}
            title={canClick ? streaming ? '点击、拖动或滑动实际原生画面' : '点击原生画面；Enter 点击中心' : inputPaused || !configurationReady ? '原生配置变更中，等待当前有效画面' : ui.capabilities.click.reason}
            disabled={!frame || (!canClick && interactionMode !== 'inspect')}
            onPointerDown={(event) => {
              if (!canClick || !streaming || !nativeInput || !event.isPrimary || event.button !== 0 || pointer.current) return;
              const actual = store.getDisplayed(); if (!actual) return;
              const point = pointAt(event.currentTarget, actual, event.clientX, event.clientY);
              if (!point || !startGesture(event.currentTarget, actual, point)) return;
              event.preventDefault();
              suppressClick.current = true;
              pointer.current = { id: event.pointerId, frame: actual, element: event.currentTarget, point };
              try { event.currentTarget.setPointerCapture(event.pointerId); } catch { cancelGesture(); }
            }} onPointerMove={(event) => {
              const active = pointer.current; if (!active || active.id !== event.pointerId || !nativeInput) return;
              const point = pointAt(active.element, active.frame, event.clientX, event.clientY, true);
              if (point) { active.point = point; nativeInput.move(point); }
            }} onPointerUp={(event) => {
              const active = pointer.current; if (!active || active.id !== event.pointerId || !nativeInput) return;
              event.preventDefault();
              const point = pointAt(active.element, active.frame, event.clientX, event.clientY, true) ?? active.point;
              releaseCapture(); finishGesture(active.element, active.frame, point);
            }} onPointerCancel={cancelGesture} onLostPointerCapture={() => { if (pointer.current) cancelGesture(); }}
            onBlur={() => { if (pointer.current) cancelGesture(); }}
            onKeyDown={(event) => {
              if (!canClick || (event.key !== 'Enter' && event.key !== ' ')) return;
              event.preventDefault();
              const actual = store.getDisplayed();
              if (!actual || !deriveUiState(snapshot, actual).inputAllowed) return;
              const point = { x: Math.floor(actual.width / 2), y: Math.floor(actual.height / 2) };
              if (streaming && nativeInput) { const element = event.currentTarget; if (startGesture(element, actual, point)) finishGesture(element, actual, point); }
              else onClick(actual, point.x, point.y);
            }} onClick={(event) => {
              if (!canClick) return;
              if (event.detail > 0 && suppressClick.current) { suppressClick.current = false; return; }
              // Assistive activation has no pointer location; key handlers prevent the browser's duplicate click.
              const actual = store.getDisplayed();
              if (!actual || !deriveUiState(snapshot, actual).inputAllowed) return;
              if (event.detail === 0) {
                const point = { x: Math.floor(actual.width / 2), y: Math.floor(actual.height / 2) };
                if (streaming && nativeInput) { const element = event.currentTarget; if (startGesture(element, actual, point)) finishGesture(element, actual, point); }
                else onClick(actual, point.x, point.y);
                return;
              }
              if (streaming) return;
              const image = event.currentTarget.querySelector('canvas');
              if (!image) return;
              const bounds = image.getBoundingClientRect();
              const point = mapFramePoint(actual, bounds, event.clientX, event.clientY);
              if (point) onClick(actual, point.x, point.y);
            }}>
            <NativeCanvas store={store} visible={visible && Boolean(frame)} />
          </button>
          <NativeTextCapture ref={textInput} enabled={textEnabled} commit={(text) => nativeInput?.text(text)} keyPress={(key) => nativeInput?.key(key)}
            focus={() => { setTextState(releasePending.current ? 'waiting' : 'ready'); setInputFeedback(null); }}
            blur={(next) => {
              setTextState('idle');
              // Focusing the screen starts another explicit click. start refuses
              // while old text is draining; a late cancel must not cancel that new click.
              if (next !== screen.current || !focusingScreen.current) cancelGesture();
            }} />
          {frame && interactionMode === 'inspect' && overlay}
          </div>
        </div>
      </div>
      {stale && <output className="frame-stale" data-testid="stale-frame">{snapshot?.build.error
        ? '旧画面 · 当前代码构建失败' : '旧画面 · 等待当前构建'} · 画面版本 {frame?.generation}</output>}
      {frame && !canClick && interactionMode === 'interact' && <span className="frame-input-note">{busy || inputPaused || !configurationReady ? '原生操作处理中，暂缓输入' : ui.capabilities.click.reason}</span>}
      {frame && interactionMode === 'interact' && (inputError || inputFeedback) && <output className="native-input-feedback">{inputError ?? inputFeedback}</output>}
      {textEnabled && <div className="native-input-bar">
        {textState !== 'idle' && <output className="native-input-status">{textState === 'waiting'
          ? '已捕获局部输入 · 等待原生点击释放；文字暂存'
          : '文字输入已打开 · 发送至已点击的原生输入框'}</output>}
        <button className="native-input-toggle" onPointerDown={(event) => { if (event.button === 0) event.preventDefault(); }}
          onClick={() => { if (document.hasFocus()) textInput.current?.focus({ preventScroll: true }); }}>输入文字</button>
      </div>}
      {!frame && <div className="frame-empty" />}
  </div>;
}
