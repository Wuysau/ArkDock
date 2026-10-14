// Explicit browser fixture only. Production index.html never imports this entry.
import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { SelectionPanel } from '../../src/inspector/SelectionPanel.tsx';
import { SelectionOverlay } from '../../src/inspector/SelectionOverlay.tsx';
import { createHitIndex } from '../../src/inspector/selection-model.ts';
import type { UiBridge, DataCommand } from '../../src/ipc/client.ts';
import type { InspectorBounds, InspectorNode, RequestScope, SelectionContext } from '../../src/ipc/model.ts';
import '../../src/styles.css';
import '../../src/inspector/inspector.css';

const scenarios = ['idle-empty', 'idle-filled', 'pending-inspect', 'confirmed-offer', 'inspect-failure',
  'overlay-pointer', 'overlay-semantic', 'overlay-keyboard'] as const;
type Scenario = typeof scenarios[number];
const requested = new URLSearchParams(location.search).get('scenario');
const scenario: Scenario = scenarios.includes(requested as Scenario) ? requested as Scenario : 'idle-empty';
const scope = { targetId: 'layout-fixture-target', sessionId: 'layout-fixture-session' };
const calls: string[] = [];
const progress = { imageReads: 0, inspectReads: 0, selectedContexts: 0 };
const eventSelections: { nodeId: string | null; bounds: InspectorBounds }[] = [];
const eventNode: InspectorNode = { nodeId: 'fixture-center-button', componentType: 'Button', parentId: null, children: [],
  bounds: { x: 120, y: 260, width: 120, height: 120 }, visible: true, attributes: [], page: 'UI fixture only', depth: 0, paintOrder: 1 };
const eventIndex = createHitIndex([eventNode], 360, 640);
const delay = () => new Promise<void>((done) => window.setTimeout(done, 25));
const now = Date.now();
const expiry = now + 600_000;
const workspaceName = 'ArkDock UI layout fixture · 很长的工程名用于验证窄窗中的初始操作可见性与换行行为';
let selection: SelectionContext = { selectionId: 'sel-layout-fixture-0123456789abcdef', workspaceId: 'fixture-workspace',
  workspaceName, sessionId: scope.sessionId, buildId: 'fixture-build', generation: 1, frameSequence: 1,
  selectionKind: 'component', inspectionExactFrameMatch: false,
  inspectionReason: '这是明确的 UI 布局测试数据，不代表原生 ArkUI 能力。很长的组件检查说明用于验证详情展开时的独立滚动与操作区可见性。',
  selectedNode: { nodeId: 'fixture-button', componentType: 'Button', parentId: 'fixture-column', children: [],
    bounds: { x: 24, y: 200, width: 280, height: 48 }, visible: true,
    attributes: [{ name: 'text', value: 'Synthetic fixture button; no ArkUI runtime' }],
    page: 'pages/deeply/nested/very-long-synthetic-page-name-for-layout-verification/Index', depth: 8, paintOrder: 4 },
  componentBounds: { x: 24, y: 200, width: 280, height: 48 },
  sourceEvidence: { status: 'candidate', relativePath: 'entry/src/main/ets/pages/deeply/nested/synthetic/layout/Index.ets',
    line: 128, reason: '源码位置仅是合成候选。该说明故意保留较长文字，验证面板会将详细证据折叠且不会把修改意见和 Agent 操作挤出初始视口。' },
  screenshot: { artifactId: 'fixture-screenshot', format: 'jpeg', width: 360, height: 640, byteLength: 0, sha256: 'fixture-only' },
  crop: { artifactId: 'fixture-crop', format: 'jpeg', width: 280, height: 48, byteLength: 0, sha256: 'fixture-only' },
  userInstruction: scenario === 'idle-empty' ? '' : '仅验证布局，不发送、不修改源码。', createdAt: now, expiresAt: expiry };

async function testImage(crop: boolean): Promise<ArrayBuffer> {
  const canvas = document.createElement('canvas');
  canvas.width = crop ? 280 : 360; canvas.height = crop ? 48 : 640;
  const context = canvas.getContext('2d')!;
  context.fillStyle = '#28364b'; context.fillRect(0, 0, canvas.width, canvas.height);
  context.fillStyle = '#d1def1'; context.font = '14px sans-serif'; context.fillText('UI layout fixture only', 12, 26);
  const blob = await new Promise<Blob>((done) => canvas.toBlob((value) => done(value!), 'image/jpeg'));
  return blob.arrayBuffer();
}

async function testTransfer(command: DataCommand, args: Record<string, unknown>, requestScope: RequestScope): Promise<unknown> {
  calls.push(command);
  if (requestScope.targetId !== scope.targetId || requestScope.sessionId !== scope.sessionId) throw new Error('TEST scope mismatch');
  await delay();
  if (command === 'selection_read_image') { const bytes = await testImage(Boolean(args.crop)); progress.imageReads++; return bytes; }
  if (command === 'selection_update_instruction') { selection = { ...selection, userInstruction: String(args.userInstruction) }; return selection; }
  if (command === 'agent_assisted_inspect') {
    progress.inspectReads++;
    if (scenario === 'pending-inspect') return new Promise<never>(() => {});
    if (scenario === 'inspect-failure') throw new Error('TEST fixture cannot confirm an empty target composer');
    if (scenario !== 'confirmed-offer') throw new Error('TEST fixture has no inspected composer');
    return { confirmationId: 'fixture-offer',
    targetId: scope.targetId, app: 'Synthetic TEST Agent', pid: 1,
    composerName: 'TEST composer · very long synthetic input area description for layout', expiresAt: expiry,
    reason: '这是测试桥接返回的辅助填写提议，不会检查任何外部窗口。此长说明用于检查辅助填写状态的按钮是否仍在初始视口中。' };
  }
  // Do not implement prompt, prepare, send, fill, pairing, export, or clipboard routes.
  throw new Error(`TEST fixture refuses side effect: ${command}`);
}
const bridge: UiBridge = { available: true, request: async () => { throw new Error('TEST fixture has no native snapshot'); },
  observe: async () => () => {}, transfer: testTransfer as UiBridge['transfer'] };

declare global { interface Window { __ARKDOCK_SELECTION_LAYOUT__: {
  scenario: Scenario; calls: string[]; progress: typeof progress; requestAssisted: () => boolean;
  eventSelections: typeof eventSelections;
} } }
window.__ARKDOCK_SELECTION_LAYOUT__ = { scenario, calls, progress, eventSelections, requestAssisted: () => {
  const control = [...document.querySelectorAll<HTMLButtonElement>('.agent-delivery button')]
    .find((button) => button.textContent === '检查目标输入框');
  if (!control || control.disabled) return false;
  control.click(); return true;
} };

function Demo() {
  const overlayScenario = scenario.startsWith('overlay-');
  const [selected, setSelected] = useState<SelectionContext | null>(overlayScenario ? null : selection);
  const candidates = [selection.selectedNode!, { ...selection.selectedNode!, nodeId: 'fixture-column', componentType: 'Column', depth: 7 }];
  function commit(bounds: InspectorBounds, node: InspectorNode | null) {
    progress.selectedContexts++; eventSelections.push({ nodeId: node?.nodeId ?? null, bounds });
    setSelected({ ...selection, selectionId: `sel-layout-fixture-event-${progress.selectedContexts}`, selectedNode: node,
      componentBounds: bounds, userInstruction: '' });
  }
  return <div className="selection-demo-shell">
    <aside className="selection-demo-banner" aria-label="开发布局验证标识"><strong>UI 布局验证 · 非原生预览</strong></aside>
    <div className="selection-demo-application"><main className="app" data-binding="bound" data-workspace="trusted" data-display="expanded">
      <div className="dock-body">
        <header className="header"><strong>ArkDock</strong><span>UI fixture only</span></header>
        <section className="workspace-strip"><button className="workspace-main"><span className="workspace-icon">◇</span><span><strong>{workspaceName}</strong><span className="workspace-path">Windows · entry</span></span></button><button className="icon-button" aria-label="布局刷新">↻</button></section>
        <output className="state-strip"><span>合成布局场景 · 不连接外部窗口</span><span className="pair-badge">布局验证</span></output>
        <section className="preview-toolbar"><div className="toolbar-selects"><button className="icon-button" aria-label="元素选择">↖</button><label><select aria-label="测试运行方式"><option>UI 布局 fixture</option></select></label><label><select aria-label="测试页面"><option>Index</option></select></label><button className="icon-button" aria-label="刷新布局">↻</button></div><div className="toolbar-actions"><button className="stop-button">停止</button><label className="zoom-control"><span className="sr-only">测试画面缩放</span><select aria-label="测试画面缩放"><option>适应窗口</option></select></label><span className="toolbar-divider"/><span className="mode-caption">非原生预览</span><button className="icon-button" aria-label="布局设置">⚙</button></div></section>
        <section className="preview" aria-label="UI 布局占位"><div className="stage-caption"><span>UI LAYOUT FIXTURE</span><span>NO NATIVE RUNTIME</span></div>
          <div className="inspector-preview"><div className="selection-demo-placeholder">合成布局占位 · 非 ArkUI</div>
            {overlayScenario && <SelectionOverlay width={360} height={640} index={eventIndex} regionMode={false}
              selected={selected?.componentBounds ?? null} select={commit} />}
            <output className="inspection-status"><span>已选择合成组件</span><button>框选</button><button>刷新树</button></output>
            {selected && <SelectionPanel key={selected.selectionId} selection={selected} stale={false} candidates={candidates}
              scope={scope} bridge={bridge} choose={() => {}} close={() => {}} updated={setSelected}/>}
          </div>
        </section>
        <footer className="footer"><button className="status-summary"><span className="dot"/><span className="status-copy"><strong>UI 布局验证</strong><span>无原生后端，无外部发送</span></span><span className="diagnostic-count">TEST bridge</span></button></footer>
      </div>
    </main></div>
  </div>;
}
const style = document.createElement('style');
style.textContent = 'html,body,#selection-demo-root{width:100%;height:100%;margin:0;overflow:hidden}.selection-demo-shell{height:100%;display:flex;flex-direction:column}.selection-demo-banner{position:absolute;z-index:30;inset:0 0 auto;height:44px;pointer-events:none;display:flex;align-items:center;padding:0 12px;background:#392d16;color:#ffdfa5;border-bottom:1px solid #987d40;font:11px "Segoe UI","Microsoft YaHei UI",sans-serif}.selection-demo-application{flex:1;min-height:0;overflow:hidden}.selection-demo-application>.app{height:100%;min-height:0}.selection-demo-placeholder{flex:1;display:grid;place-items:start center;padding-top:64px;color:var(--muted);border:1px dashed var(--line);border-radius:12px}';
document.head.append(style);
createRoot(document.getElementById('selection-demo-root')!).render(<Demo/>);
