import { useEffect, useRef, useState } from 'react';
import { AgentDeliveryPanel } from './AgentDeliveryPanel.tsx';
import type { AppSnapshot, InspectorNode, SelectionContext } from '../ipc/model.ts';
import { Icon } from '../Icon.tsx';
import type { UiBridge } from '../ipc/client.ts';
import { describeError } from '../ipc/errors.ts';
import type { RequestScope } from '../ipc/model.ts';

export function SelectionPanel({ selection, stale, candidates, scope, snapshot, visible, bridge, choose, close, updated }: {
  selection: SelectionContext; stale: boolean; candidates: InspectorNode[];
  snapshot: AppSnapshot | null; visible: boolean; scope: RequestScope; bridge: UiBridge; choose: (node: InspectorNode) => void; close: () => void; updated: (selection: SelectionContext) => void;
}) {
  const [instruction, setInstruction] = useState(selection.userInstruction);
  const [working, setWorking] = useState(false);
  const [notice, setNotice] = useState('');
  const [images, setImages] = useState<string[]>([]);
  const targetId = scope.targetId;
  const sessionId = scope.sessionId;
  const alive = useRef(true);
  const active = useRef(false);
  const identity = JSON.stringify([selection.selectionId, selection.workspaceId, selection.buildId, selection.generation, targetId, sessionId]);
  const current = useRef({ identity, stale }); current.current = { identity, stale };
  const stillCurrent = (requested: string) => alive.current && !current.current.stale && current.current.identity === requested;
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => {
    let live = true;
    const urls: string[] = [];
    setImages([]);
    if (!bridge.transfer || stale) return;
    void Promise.all([false, true].map(async (crop) => {
      const bytes = await bridge.transfer!('selection_read_image', { selectionId: selection.selectionId, crop }, { targetId, sessionId });
      if (!(bytes instanceof ArrayBuffer) || !live) return '';
      const url = URL.createObjectURL(new Blob([bytes], { type: 'image/jpeg' }));
      urls.push(url); return url;
    })).then((values) => { if (live) setImages(values); }).catch(() => { if (live) setNotice('截图读取失败；重新选择以取得当前画面。'); });
    return () => { live = false; for (const url of urls) URL.revokeObjectURL(url); };
  }, [selection.selectionId, bridge, targetId, sessionId, stale]);
  async function save() {
    if (!bridge.transfer) throw new Error("缺少桌面桥接");
    const requested = identity;
    const saved = await bridge.transfer("selection_update_instruction", { selectionId: selection.selectionId, userInstruction: instruction }, scope);
    if (!stillCurrent(requested)) throw new Error('选择已改变，请重新选择后继续。');
    updated(saved); return saved;
  }
  async function run(action: 'save' | 'copy_details' | 'export' | 'crop') {
    if (working || active.current || stale || !bridge.transfer) return;
    const requested = identity;
    active.current = true;
    setWorking(true); setNotice('');
    try {
      await save();
      if (!stillCurrent(requested)) return;
      if (action === 'copy_details') {
        const prompt = await bridge.transfer('selection_prompt', { selectionId: selection.selectionId, detailed: true }, scope);
        if (!stillCurrent(requested)) return;
        await navigator.clipboard.writeText(prompt);
        if (stillCurrent(requested)) setNotice('详细诊断提示词已复制，包含内部标识和完整组件属性。');
      } else if (action === 'export' || action === 'crop') {
        await bridge.transfer('selection_export_image', { selectionId: selection.selectionId, crop: action === 'crop' }, scope);
        if (stillCurrent(requested)) setNotice(action === 'crop' ? '裁剪截图已导出。' : '真实原生截图已导出。');
      } else setNotice('选择上下文和修改意见已保存；提交到 MCP 后，所选上下文才可读取任务快照。');
    } catch (error) { if (stillCurrent(requested)) setNotice(describeError(error)); }
    finally { active.current = false; if (alive.current) setWorking(false); }
  }
  const node = selection.selectedNode;
  const bounds = selection.componentBounds;
  const source = selection.sourceEvidence;
  const sourceLabel = source.status === 'verified' ? '已核验' : source.status === 'candidate' ? '候选 · 需核对' : '待定位';
  return <section className="selection-panel" aria-label="选择组件详情" data-testid="selection-panel">
    <header><div><h3>{node?.componentType ?? '截图区域'}</h3><span className="selection-kind">{node ? 'ArkUI 组件' : '区域选择'}</span></div><button className="icon-button" aria-label="关闭组件详情" onClick={close}><Icon name="close" size={17} /></button></header>
    {stale && <p role="alert">EXPIRED · 工程、构建或画面已改变，请重新选择。</p>}
    <label htmlFor="selection-instruction">修改意见</label><textarea id="selection-instruction" maxLength={8192} value={instruction} disabled={working || stale} placeholder="描述要如何修改这个组件…" onChange={(event) => setInstruction(event.target.value)} />
    <AgentDeliveryPanel selection={selection} scope={scope} snapshot={snapshot} visible={visible} bridge={bridge} disabled={working || stale} expired={stale} hasInstruction={Boolean(instruction.trim())} save={save} busy={setWorking} />
    <details className="selection-evidence"><summary>组件与截图详情</summary>
    <dl className="selection-summary"><dt>工程</dt><dd>{selection.workspaceName}</dd>
      {node?.page && <><dt>页面</dt><dd>{node.page}</dd></>}
      <dt>源码位置</dt><dd><span className={`selection-source-state ${source.status}`}>{sourceLabel}</span><code>{source.relativePath ?? '结合组件和截图定位'}{source.line ? `:${source.line}` : ''}</code></dd>
      <dt>选中区域</dt><dd>{bounds.x.toFixed(1)}, {bounds.y.toFixed(1)} · {bounds.width.toFixed(1)} × {bounds.height.toFixed(1)}</dd>
    </dl>
    {source.status !== 'verified' && <p className="selection-source-note">复制内容会提示 Agent 先核对源码，避免只按行号修改。</p>}
    {candidates.length > 1 && <label>选择命中组件<select aria-label="选择命中组件" value={node?.nodeId ?? ''} disabled={working || stale} onChange={(event) => { const candidate = candidates.find((item) => item.nodeId === event.target.value); if (candidate) choose(candidate); }}>{candidates.map((candidate) => <option key={candidate.nodeId} value={candidate.nodeId}>{candidate.componentType ?? '未知'} · #{candidate.nodeId} · 深度 {candidate.depth}</option>)}</select></label>}
    {images.length > 0 && <div className="selection-images">{images.map((url, index) => url && <img key={url} src={url} alt={index ? '选择区域裁剪' : '真实原生截图'} />)}</div>}
    <div className="selection-actions"><button disabled={working || stale} onClick={() => void run('export')}>导出截图</button><button disabled={working || stale} onClick={() => void run('crop')}>导出裁剪</button></div>
    </details>
    <details className="selection-technical"><summary>定位与技术详情</summary>
      <p>{source.reason}</p><p>{selection.inspectionReason}</p>
      <dl><dt>选择 ID</dt><dd><code>{selection.selectionId}</code></dd><dt>工作区 ID</dt><dd><code>{selection.workspaceId}</code></dd>
        <dt>预览会话</dt><dd><code>{selection.sessionId}</code></dd><dt>构建 / 帧</dt><dd><code>{selection.buildId} · 代次 {selection.generation} · 帧 {selection.frameSequence}</code></dd>
        {node && <><dt>组件层级</dt><dd>深度 {node.depth} · 父 {node.parentId ?? '—'} · {node.children.length} 子节点</dd></>}
      </dl><details><summary>完整组件属性</summary><pre>{JSON.stringify(node ?? { region: bounds }, null, 2)}</pre></details>
      <div className="selection-actions"><button disabled={working || stale} onClick={() => void run('copy_details')}>复制详细诊断提示词</button>
        <button disabled={working || stale} onClick={() => void run('save')}>保存选择上下文</button></div>
    </details>
    {notice && <output>{notice}</output>}
  </section>;
}
