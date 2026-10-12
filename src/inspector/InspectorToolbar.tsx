import type { InteractionMode } from '../ipc/model.ts';

export function InspectorToolbar({ mode, enabled, change }: { mode: InteractionMode; enabled: boolean; change: (mode: InteractionMode) => void }) {
  return <button className={`icon-button inspector-toggle${mode === 'inspect' ? ' is-active' : ''}`} aria-label="选择 ArkUI 元素"
    aria-pressed={mode === 'inspect'} disabled={!enabled} data-testid="inspector-toggle"
    title={enabled ? mode === 'inspect' ? '退出元素选择 · Esc' : '选择 ArkUI 元素；此模式不触发应用点击' : '等待当前有效原生画面'}
    onClick={() => change(mode === 'inspect' ? 'interact' : 'inspect')}>
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M9 4H4v5M15 4h5v5M4 15v5h5M20 15v5h-5" stroke="currentColor" strokeWidth="1.5" /><path d="m9 8 10 6-5 1-2 5-3-12Z" fill="currentColor" /></svg>
  </button>;
}

export function InspectorActionsBar({ enabled, region, refresh, problem }: {
  enabled: boolean; region: () => void; refresh: () => void; problem: string | null;
}) {
  return <div className="inspector-actions" aria-label="元素选择操作">
    <span>元素选择</span>
    <button disabled={!enabled} data-testid="inspector-region" title="拖动框选实际画面区域" onClick={region}>框选</button>
    <button disabled={!enabled} data-testid="inspector-refresh-tree" title="重新读取当前原生组件树" onClick={refresh}>刷新组件树</button>
    {problem && <details className="inspector-query-details"><summary>读取异常</summary><p>组件树暂不可用，可使用框选。</p><p>{problem}</p></details>}
  </div>;
}
