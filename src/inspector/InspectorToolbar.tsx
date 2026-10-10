import type { InteractionMode } from '../ipc/model.ts';

export function InspectorToolbar({ mode, enabled, change }: { mode: InteractionMode; enabled: boolean; change: (mode: InteractionMode) => void }) {
  return <button className={`icon-button inspector-toggle${mode === 'inspect' ? ' is-active' : ''}`} aria-label="选择 ArkUI 元素"
    aria-pressed={mode === 'inspect'} disabled={!enabled} data-testid="inspector-toggle"
    title={enabled ? mode === 'inspect' ? '退出元素选择 · Esc' : '选择 ArkUI 元素；此模式不触发应用点击' : '等待当前有效原生画面'}
    onClick={() => change(mode === 'inspect' ? 'interact' : 'inspect')}>
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M9 4H4v5M15 4h5v5M4 15v5h5M20 15v5h-5" stroke="currentColor" strokeWidth="1.5" /><path d="m9 8 10 6-5 1-2 5-3-12Z" fill="currentColor" /></svg>
  </button>;
}
