import { useEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import { Icon } from './Icon.tsx';
export function Sheet({ title, children, close, kind = 'panel', returnFocus }: { title: string; children: ReactNode; close: () => void; kind?: string; returnFocus?: HTMLElement | null }) {
  const ref = useRef<HTMLDialogElement>(null);
  const closeRef = useRef(close);
  const anchor = useRef(returnFocus);
  closeRef.current = close;
  useEffect(() => {
    const previous = anchor.current ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    const preferred = ref.current?.querySelector<HTMLElement>('[data-autofocus]');
    (preferred ?? ref.current)?.focus();
    return () => {
      requestAnimationFrame(() => {
        // React removes inert with the closing dialog. A replacement dialog owns focus instead.
        if (!document.querySelector('dialog[open]') && previous?.isConnected) previous.focus();
      });
    };
  }, []);
  return <div className="sheet-backdrop" onPointerDown={(event) => { if (event.target === event.currentTarget) closeRef.current(); }}>
    <dialog open ref={ref} className={`panel sheet ${kind}`} aria-modal="true" aria-label={title} tabIndex={-1} onKeyDown={(event) => {
      if (event.key === 'Escape' && !event.nativeEvent.isComposing) { event.stopPropagation(); closeRef.current(); }
      if (event.key !== 'Tab') return;
      const items = Array.from(ref.current?.querySelectorAll<HTMLElement>('button:enabled, input:enabled, select:enabled, a[href], [tabindex="0"]') ?? []).filter((item) => item.getClientRects().length > 0);
      if (!items.length) { event.preventDefault(); return; }
      const first = items[0]; const last = items[items.length - 1];
      if (event.shiftKey && (document.activeElement === first || document.activeElement === ref.current)) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && (document.activeElement === last || document.activeElement === ref.current)) { event.preventDefault(); first.focus(); }
    }}>
      <div className="sheet-handle" aria-hidden="true" />
      <div className="panel-title"><div><span className="eyebrow">ARKDOCK</span><h2>{title}</h2></div><button className="icon-button" aria-label="关闭面板" title="关闭面板 · Esc" onClick={close}><Icon name="close" /></button></div>
      <div className="panel-body">{children}</div>
    </dialog>
  </div>;
}
