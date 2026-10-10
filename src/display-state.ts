import type { DisplayStatus } from './ipc/model.ts';

export function displayTransitioning(display: DisplayStatus | null | undefined): boolean {
  return display?.mode === 'collapsing' || display?.mode === 'expanding';
}

/** Native tray/menu actions may have advanced the sequence since the last UI request. */
export function nextDisplayRequestId(previous: number, display: DisplayStatus | null | undefined): number {
  const next = Math.max(previous, display?.requestId ?? 0) + 1;
  if (!Number.isSafeInteger(next)) throw new Error('显示请求编号已超出有效范围，请重新启动 ArkDock。');
  return next;
}
