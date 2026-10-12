import { useCallback, useEffect, useId, useRef, useState } from 'react';
import type { FocusEvent, KeyboardEvent, ReactNode } from 'react';
import { Icon } from './Icon.tsx';

type Edge = 'toolbar' | 'status';
type Activity = { hovered: boolean; focused: boolean; pinned: boolean; dismissed: boolean };
const edges: Edge[] = ['toolbar', 'status'];
const HIDE_DELAY_MS = 280;
const EDGE_REVEAL_DIP = 24;

/** Local chrome only: neither visibility nor hover sends a native preview request. */
export function PreviewChrome({ children, toolbar, status, statusIndicator, enabled, inspecting, modalOpen }: {
  children: ReactNode; toolbar: ReactNode; status: ReactNode; statusIndicator: ReactNode;
  enabled: boolean; inspecting: boolean; modalOpen: boolean;
}) {
  const id = useId();
  const root = useRef<HTMLDivElement>(null);
  const toolbarRegion = useRef<HTMLDivElement>(null);
  const statusRegion = useRef<HTMLDivElement>(null);
  const bounds = useRef<DOMRectReadOnly | null>(null);
  const activity = useRef<Record<Edge, Activity>>({
    toolbar: { hovered: false, focused: false, pinned: false, dismissed: false },
    status: { hovered: false, focused: false, pinned: false, dismissed: false },
  });
  const timers = useRef<Partial<Record<Edge, ReturnType<typeof setTimeout>>>>({});
  const latest = useRef({ enabled, modalOpen }); latest.current = { enabled, modalOpen };
  const [open, setOpen] = useState<Record<Edge, boolean>>({ toolbar: false, status: false });
  const openRef = useRef(open); openRef.current = open;
  const region = useCallback((edge: Edge) => edge === 'toolbar' ? toolbarRegion.current : statusRegion.current, []);
  const cancelTimer = useCallback((edge: Edge) => {
    const timer = timers.current[edge];
    if (timer !== undefined) clearTimeout(timer);
    delete timers.current[edge];
  }, []);
  const reveal = useCallback((edge: Edge) => {
    if (edge === 'toolbar' && !latest.current.enabled) return;
    cancelTimer(edge);
    setOpen(current => current[edge] ? current : { ...current, [edge]: true });
  }, [cancelTimer]);
  const settle = useCallback((edge: Edge) => {
    const flags = activity.current[edge];
    if (flags.hovered || flags.focused || flags.pinned) { reveal(edge); return; }
    if (timers.current[edge] !== undefined || latest.current.modalOpen) return;
    timers.current[edge] = setTimeout(() => {
      delete timers.current[edge];
      const current = activity.current[edge];
      const body = region(edge)?.querySelector('.preview-chrome-body');
      // Native select popups can leave relatedTarget null while the select retains focus.
      if (latest.current.modalOpen || current.hovered || current.focused || current.pinned
        || body?.contains(document.activeElement)) return;
      setOpen(value => value[edge] ? { ...value, [edge]: false } : value);
    }, HIDE_DELAY_MS);
  }, [region, reveal]);
  const hover = useCallback((edge: Edge, hovered: boolean) => {
    const flags = activity.current[edge];
    if (!hovered) flags.dismissed = false;
    const next = hovered && !flags.dismissed;
    if (flags.hovered === next) return;
    flags.hovered = next;
    settle(edge);
  }, [settle]);
  const close = useCallback((edge: Edge) => {
    cancelTimer(edge);
    const element = region(edge);
    if (element?.querySelector('.preview-chrome-body')?.contains(document.activeElement)) {
      element.querySelector<HTMLButtonElement>('.preview-chrome-trigger')?.focus({ preventScroll: true });
    }
    activity.current[edge] = { hovered: false, focused: false, pinned: false, dismissed: true };
    setOpen(value => value[edge] ? { ...value, [edge]: false } : value);
  }, [cancelTimer, region]);
  const toggle = (edge: Edge) => {
    if (openRef.current[edge]) close(edge);
    else { activity.current[edge].pinned = true; activity.current[edge].dismissed = false; reveal(edge); }
  };
  useEffect(() => {
    const element = root.current;
    if (!element) return;
    const measure = () => { bounds.current = element.getBoundingClientRect(); };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    window.addEventListener('resize', measure);
    return () => { observer.disconnect(); window.removeEventListener('resize', measure); edges.forEach(cancelTimer); };
  }, [cancelTimer]);
  useEffect(() => {
    const body = toolbarRegion.current?.querySelector<HTMLElement>('.preview-chrome-body');
    if (!body) return;
    const observer = new ResizeObserver(([entry]) => {
      if (!entry || !root.current) return;
      const height = entry.borderBoxSize[0]?.blockSize ?? entry.contentRect.height + 2;
      root.current.style.setProperty('--preview-toolbar-height', `${height + 12}px`);
    });
    observer.observe(body);
    return () => observer.disconnect();
  }, []);
  useEffect(() => { if (!enabled) close('toolbar'); }, [enabled, close]);
  useEffect(() => {
    if (modalOpen) edges.forEach(cancelTimer);
    else edges.forEach(settle);
  }, [modalOpen, cancelTimer, settle]);
  const handlers = (edge: Edge) => ({
    onFocusCapture: (event: FocusEvent<HTMLDivElement>) => {
      if (event.currentTarget.querySelector('.preview-chrome-body')?.contains(event.target)) {
        activity.current[edge].focused = true; reveal(edge);
      }
    },
    onBlurCapture: (event: FocusEvent<HTMLDivElement>) => {
      activity.current[edge].focused = event.relatedTarget instanceof Node
        && Boolean(event.currentTarget.querySelector('.preview-chrome-body')?.contains(event.relatedTarget));
      settle(edge);
    },
    onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => {
      if (event.key === 'Escape' && !event.nativeEvent.isComposing && !inspecting) {
        event.preventDefault(); event.stopPropagation(); close(edge);
      }
    },
  });
  return <div ref={root} className="preview-chrome" data-toolbar-open={open.toolbar} data-status-open={open.status} data-inspecting={inspecting}
    onPointerEnter={() => { if (root.current) bounds.current = root.current.getBoundingClientRect(); }}
    onPointerMove={(event) => {
      const rectangle = bounds.current;
      // An Inspector selection drag retains its pointer; do not reveal chrome in its path.
      if (!rectangle || modalOpen || event.pointerType === 'touch' || event.buttons !== 0) return;
      const target = event.target instanceof Node ? event.target : null;
      const y = event.clientY - rectangle.top;
      if (event.clientX < rectangle.left || event.clientX > rectangle.right || y < 0 || y > rectangle.height) {
        edges.forEach(edge => hover(edge, false)); return;
      }
      hover('toolbar', enabled && (y <= EDGE_REVEAL_DIP || Boolean(target && toolbarRegion.current?.contains(target))));
      hover('status', y >= rectangle.height - EDGE_REVEAL_DIP || Boolean(target && statusRegion.current?.contains(target)));
    }} onPointerLeave={() => edges.forEach(edge => hover(edge, false))}>
    {children}
    <div ref={toolbarRegion} className="preview-chrome-region chrome-top" data-open={open.toolbar} hidden={!enabled} {...handlers('toolbar')}>
      <button className="preview-chrome-trigger toolbar-trigger" aria-label={open.toolbar ? '收起预览控制栏' : '展开预览控制栏'} aria-expanded={open.toolbar} aria-controls={`${id}-toolbar`}
        title="预览控制 · 鼠标靠近上边缘，或按 Enter 展开" onClick={() => toggle('toolbar')}><Icon name="settings" size={14} /><span>预览控制</span><Icon name="chevron" size={12} /></button>
      <div id={`${id}-toolbar`} className="preview-chrome-body" {...(!open.toolbar ? { inert: '' } : {})}>{toolbar}</div>
    </div>
    <div ref={statusRegion} className="preview-chrome-region chrome-bottom" data-open={open.status} {...handlers('status')}>
      <button className="preview-chrome-trigger status-trigger" aria-label={open.status ? '收起预览状态详情' : '展开预览状态详情'} aria-expanded={open.status} aria-controls={`${id}-status`}
        title="预览状态 · 鼠标靠近下边缘，或按 Enter 展开" onClick={() => toggle('status')}>{statusIndicator}<Icon name="chevron" size={12} /></button>
      <div id={`${id}-status`} className="preview-chrome-body" {...(!open.status ? { inert: '' } : {})}>{status}</div>
    </div>
  </div>;
}
