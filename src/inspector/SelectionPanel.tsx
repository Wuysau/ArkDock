import { useEffect, useState } from 'react';
import { AgentDeliveryPanel } from './AgentDeliveryPanel.tsx';
import type { AgentSessionList, InspectorNode, SelectionContext } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';
import type { RequestScope } from '../ipc/model.ts';

export function SelectionPanel({ selection, stale, candidates, sessions, scope, bridge, choose, close, updated }: {
  selection: SelectionContext; stale: boolean; candidates: InspectorNode[]; sessions: AgentSessionList | null;
  scope: RequestScope; bridge: UiBridge; choose: (node: InspectorNode) => void; close: () => void; updated: (selection: SelectionContext) => void;
}) {
  const [instruction, setInstruction] = useState(selection.userInstruction);
  const [working, setWorking] = useState(false);
  const [notice, setNotice] = useState('');
  const [images, setImages] = useState<string[]>([]);
  const targetId = scope.targetId;
  const sessionId = scope.sessionId;
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
    const saved = await bridge.transfer("selection_update_instruction", { selectionId: selection.selectionId, userInstruction: instruction }, scope);
    updated(saved); return saved;
  }
  async function run(action: 'save' | 'copy' | 'export' | 'crop') {
    if (working || stale || !bridge.transfer) return;
    setWorking(true); setNotice('');
    try {
      await save();
      if (action === 'copy') {
        const prompt = await bridge.transfer('selection_prompt', { selectionId: selection.selectionId }, scope);
        await navigator.clipboard.writeText(prompt); setNotice('完整修改提示词已复制。');
      } else if (action === 'export' || action === 'crop') {
        await bridge.transfer('selection_export_image', { selectionId: selection.selectionId, crop: action === 'crop' }, scope);
        setNotice(action === 'crop' ? '裁剪截图已导出。' : '真实原生截图已导出。');
      } else setNotice('选择上下文和修改意见已保存；MCP 读取仍需显式授权。');
    } catch (error) { setNotice(String(error)); }
    finally { setWorking(false); }
  }
  const node = selection.selectedNode;
  const bounds = selection.componentBounds;
  return <section className="selection-panel" aria-label="选择组件详情" data-testid="selection-panel">
    <header><div><h3>{node?.componentType ?? '区域选择模式'}</h3><code>{selection.selectionId}</code></div><button aria-label="关闭组件详情" onClick={close}>×</button></header>
    {stale && <p role="alert">EXPIRED · 工程、构建或画面已改变，请重新选择。</p>}
    <dl><dt>工程</dt><dd>{selection.workspaceName}</dd><dt>页面</dt><dd>{node?.page ?? '原生响应未提供页面名'}</dd><dt>原生位置</dt><dd>{bounds.x.toFixed(1)}, {bounds.y.toFixed(1)} · {bounds.width.toFixed(1)} × {bounds.height.toFixed(1)}</dd><dt>组件层级</dt><dd>{node ? `深度 ${node.depth} · 父 ${node.parentId ?? '—'} · ${node.children.length} 子节点` : '区域没有组件层级'}</dd><dt>源码证据</dt><dd>{selection.sourceEvidence.status.toUpperCase()} · {selection.sourceEvidence.relativePath ?? '需要 Agent 结合源码定位'}{selection.sourceEvidence.line ? `:${selection.sourceEvidence.line}` : ''}</dd></dl>
    <p>{selection.sourceEvidence.reason}</p><p>{selection.inspectionReason}</p>
    {candidates.length > 1 && <label>命中层级候选<select aria-label="选择命中组件" value={node?.nodeId ?? ''} disabled={working || stale} onChange={(event) => { const candidate = candidates.find((item) => item.nodeId === event.target.value); if (candidate) choose(candidate); }}>{candidates.map((candidate) => <option key={candidate.nodeId} value={candidate.nodeId}>{candidate.componentType ?? '未知'} · #{candidate.nodeId} · 深度 {candidate.depth}</option>)}</select></label>}
    <details><summary>完整组件信息</summary><pre>{JSON.stringify(node ?? { region: bounds }, null, 2)}</pre></details>
    {images.length > 0 && <div className="selection-images">{images.map((url, index) => url && <img key={url} src={url} alt={index ? '选择区域裁剪' : '真实原生截图'} />)}</div>}
    <label htmlFor="selection-instruction">修改意见</label><textarea id="selection-instruction" maxLength={8192} value={instruction} disabled={working || stale} placeholder="描述要如何修改这个组件…" onChange={(event) => setInstruction(event.target.value)} />
    <AgentDeliveryPanel selection={selection} sessions={sessions} scope={scope} bridge={bridge} disabled={working || stale} revokeDisabled={working} hasInstruction={Boolean(instruction.trim())} save={save} busy={setWorking} />
    <div className="selection-actions"><button disabled={working || stale} onClick={() => void run('copy')}>复制完整提示词（覆盖剪贴板）</button><button disabled={working || stale} onClick={() => void run('export')}>导出截图</button><button disabled={working || stale} onClick={() => void run('crop')}>导出裁剪</button><button disabled={working || stale} onClick={() => void run('save')}>保存选择上下文</button></div>
    {notice && <output>{notice}</output>}
  </section>;
}
