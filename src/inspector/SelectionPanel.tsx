import { useEffect, useState } from 'react';
import type { AgentKind, AgentSessionList, InspectorNode, SelectionContext } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';
import type { RequestScope } from '../ipc/model.ts';

export function SelectionPanel({ selection, stale, candidates, sessions, scope, bridge, choose, close, updated }: {
  selection: SelectionContext; stale: boolean; candidates: InspectorNode[]; sessions: AgentSessionList | null;
  scope: RequestScope; bridge: UiBridge; choose: (node: InspectorNode) => void; close: () => void; updated: (selection: SelectionContext) => void;
}) {
  const [instruction, setInstruction] = useState(selection.userInstruction);
  const [working, setWorking] = useState(false);
  const [notice, setNotice] = useState('');
  const [bindingId, setBindingId] = useState('');
  const [agentKind, setAgentKind] = useState<AgentKind>('codex');
  const [threadId, setThreadId] = useState('');
  const [images, setImages] = useState<string[]>([]);
  const [bindings, setBindings] = useState(sessions?.bindings ?? []);
  const targetId = scope.targetId;
  const sessionId = scope.sessionId;
  useEffect(() => { setBindings(sessions?.bindings ?? []); }, [sessions]);
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
  const target = bindings.find((binding) => binding.bindingId === bindingId);
  const ready = Boolean(target?.writable && target.capabilities.identifyCurrentSession && target.capabilities.directSend && target.capabilities.receipt && target.expiresAt > Date.now());
  async function run(action: 'save' | 'copy' | 'send' | 'export' | 'crop') {
    if (working || stale || !bridge.transfer) return;
    setWorking(true); setNotice('');
    try {
      const saved = await bridge.transfer('selection_update_instruction', { selectionId: selection.selectionId, userInstruction: instruction }, scope);
      updated(saved);
      if (action === 'copy') {
        const prompt = await bridge.transfer('selection_prompt', { selectionId: selection.selectionId }, scope);
        await navigator.clipboard.writeText(prompt); setNotice('完整修改提示词已复制。');
      } else if (action === 'send') {
        const prepared = await bridge.transfer('agent_delivery_prepare', { selectionId: selection.selectionId, bindingId: target?.bindingId ?? null }, scope);
        if (prepared.state !== 'READY') { setNotice(`${prepared.state} · ${prepared.reason}`); return; }
        const result = await bridge.transfer('agent_delivery_send', { requestId: prepared.requestId }, scope);
        setNotice(result.state === 'SUBMITTED' && result.receiptId ? `SUBMITTED · 会话已接受，回执 ${result.receiptId}` : `${result.state} · ${result.reason}`);
      } else if (action === 'export' || action === 'crop') {
        await bridge.transfer('selection_export_image', { selectionId: selection.selectionId, crop: action === 'crop' }, scope);
        setNotice(action === 'crop' ? '裁剪截图已导出。' : '真实原生截图已导出。');
      } else setNotice('选择上下文和修改意见已保存；MCP 读取仍需显式授权。');
    } catch (error) { setNotice(String(error)); }
    finally { setWorking(false); }
  }
  async function bind() {
    if (working || !bridge.transfer || !threadId.trim() || stale) return;
    setWorking(true);
    try {
      const binding = await bridge.transfer('agent_session_bind', { draft: { agentKind, agentSessionId: threadId.trim(), conversationOrThreadId: threadId.trim() } }, scope);
      setBindings((current) => [...current.filter((value) => value.bindingId !== binding.bindingId), binding]); setBindingId(binding.bindingId);
      setNotice(binding.reason);
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
    <div className="selection-target"><p>发送目标 · {target ? `${target.agentKind} / ${target.workspaceId} / ${target.conversationOrThreadId}` : '当前 Agent 会话尚未核验'}</p>
      {bindings.length > 0 && <select aria-label="发送目标会话" value={bindingId} onChange={(event) => setBindingId(event.target.value)}><option value="">选择明确登记的会话</option>{bindings.map((binding) => <option key={binding.bindingId} value={binding.bindingId}>{binding.agentKind} · {binding.conversationOrThreadId}</option>)}</select>}
      <p>{ready ? 'READY · 当前会话支持直接提交和回执' : target?.reason ?? sessions?.adapters.map((adapter) => `${adapter.agentKind}: ${adapter.status} · ${adapter.reason}`).join('；') ?? 'SESSION_UNVERIFIED · 正在核验能力'}</p>
      <details><summary>登记或更换会话</summary><label>Agent<select aria-label="Agent 类型" value={agentKind} onChange={(event) => setAgentKind(event.target.value as AgentKind)}><option value="codex">Codex</option><option value="claude">Claude</option></select></label><label>明确的当前会话 ID<input aria-label="当前 Agent 会话 ID" value={threadId} maxLength={256} onChange={(event) => setThreadId(event.target.value)} /></label><button disabled={working || stale || !threadId.trim()} onClick={() => void bind()}>登记并核验</button><p>手动填写 ID 不能授予写入权限，须经适配器确认当前活动线程。</p></details>
    </div>
    <div className="selection-actions"><button className="primary-button selection-primary" disabled={working || stale || !ready || !instruction.trim()} onClick={() => void run('send')}>发送到当前 Agent</button><button disabled={working || stale} onClick={() => void run('copy')}>复制完整提示词</button><button disabled={working || stale} onClick={() => void run('export')}>导出截图</button><button disabled={working || stale} onClick={() => void run('crop')}>导出裁剪</button><button disabled={working || stale} onClick={() => void run('save')}>保存选择上下文</button></div>
    {notice && <output>{notice}</output>}
  </section>;
}
