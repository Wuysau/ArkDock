import { useRef, useState } from 'react';
import type { InspectorBounds, InspectorNode } from '../ipc/model.ts';
import { mapFramePoint } from '../view-state.ts';
import { createHitIndex, regionBetween } from './selection-model.ts';

export function SelectionOverlay({ width, height, index, regionMode, selected, select, reject }: {
  width: number; height: number; index: ReturnType<typeof createHitIndex> | null; regionMode: boolean;
  selected: InspectorBounds | null; select: (bounds: InspectorBounds, node: InspectorNode | null, candidates: InspectorNode[]) => void;
  reject?: (reason: string) => void;
}) {
  const start = useRef<{ x: number; y: number } | null>(null);
  const pointerHandled = useRef(false);
  const [hover, setHover] = useState<InspectorNode | null>(null);
  const [region, setRegion] = useState<InspectorBounds | null>(null);
  const current = region ?? hover?.bounds ?? selected;
  function choose(point: { x: number; y: number }) {
    const candidates = index?.hit(point.x, point.y) ?? [];
    const node = candidates[0];
    if (node?.bounds) select(node.bounds, node, candidates);
    else reject?.('此位置没有命中可选择的组件，请刷新树后重选，或使用框选。');
  }
  function chooseCenter() {
    if (regionMode) select({ x: width / 4, y: height / 4, width: width / 2, height: height / 2 }, null, []);
    else choose({ x: width / 2, y: height / 2 });
  }
  return <button type="button" className={`selection-overlay${regionMode ? ' region-mode' : ''}`} data-testid="selection-overlay"
    aria-label={regionMode ? '区域选择模式，拖动框选真实画面；Enter 选择中心区域' : 'ArkUI 元素选择模式；Enter 选择中心组件'}
    onKeyDown={(event) => {
      if (event.key !== 'Enter' && event.key !== ' ') return;
      event.preventDefault(); event.stopPropagation();
      chooseCenter();
    }}
    onPointerDown={(event) => {
      event.preventDefault(); event.stopPropagation();
      if (event.button !== 0) return;
      pointerHandled.current = false;
      const point = mapFramePoint({ width, height }, event.currentTarget.getBoundingClientRect(), event.clientX, event.clientY);
      if (!point) return;
      event.currentTarget.setPointerCapture(event.pointerId);
      start.current = point;
      if (regionMode) setRegion(regionBetween(point, point, width, height));
    }} onPointerMove={(event) => {
      event.stopPropagation();
      const point = mapFramePoint({ width, height }, event.currentTarget.getBoundingClientRect(), event.clientX, event.clientY);
      if (!point) { if (!start.current) setHover(null); return; }
      if (regionMode && start.current) setRegion(regionBetween(start.current, point, width, height));
      else if (!regionMode) setHover(index?.hit(point.x, point.y)[0] ?? null);
    }} onPointerUp={(event) => {
      event.preventDefault(); event.stopPropagation();
      const origin = start.current; start.current = null;
      if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
      const point = mapFramePoint({ width, height }, event.currentTarget.getBoundingClientRect(), event.clientX, event.clientY);
      if (!point || !origin) { setRegion(null); return; }
      if (regionMode) {
        const bounds = regionBetween(origin, point, width, height);
        if (bounds) { pointerHandled.current = true; select(bounds, null, []); }
      } else {
        pointerHandled.current = true;
        choose(point);
      }
      setRegion(null);
    }} onPointerCancel={() => { start.current = null; setRegion(null); }}
    onPointerLeave={() => { if (!start.current) setHover(null); }} onClick={(event) => {
      event.preventDefault(); event.stopPropagation();
      // UI Automation/default button activation emits click without a pointer
      // sequence. Treat it like keyboard activation; don't swallow the action.
      if (event.detail === 0) { chooseCenter(); return; }
      if (pointerHandled.current) { pointerHandled.current = false; return; }
      if (!regionMode) {
        const point = mapFramePoint({ width, height }, event.currentTarget.getBoundingClientRect(), event.clientX, event.clientY);
        if (point) choose(point);
      }
    }}>
    {current && <div className="selection-outline" style={{ left: `${current.x / width * 100}%`, top: `${current.y / height * 100}%`, width: `${current.width / width * 100}%`, height: `${current.height / height * 100}%` }}>
      <span>{region ? '拖动框选' : hover ? `点击选择 ${hover.componentType ?? '组件'}` : '已锁定选择'} · {Math.round(current.width)} × {Math.round(current.height)}</span>
    </div>}
  </button>;
}
