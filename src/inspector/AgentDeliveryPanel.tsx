import { useEffect, useRef, useState } from 'react';
import type { AppSnapshot, McpTaskSummary, RequestScope, SelectionContext } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';
import { describeError } from '../ipc/errors.ts';
import { contextMatches, mcpLifecycle, submitFrozenSelection, taskLabels } from '../mcp/model.ts';
import { useMcpStatus } from '../mcp/useMcpStatus.ts';
import { automaticProcessingPrompt } from '../mcp/automatic-processing.ts';
import '../mcp/mcp.css';

/** A task is frozen by the backend only after the current instruction is saved. */
export function AgentDeliveryPanel({ selection, scope, snapshot, visible, bridge, disabled, expired, hasInstruction, save, busy }: {
  selection: SelectionContext; scope: RequestScope; snapshot: AppSnapshot | null; visible: boolean; bridge: UiBridge;
  disabled: boolean; expired: boolean; hasInstruction: boolean;
  save: () => Promise<SelectionContext>; busy: (value: boolean) => void;
}) {
  const lifecycle = mcpLifecycle(snapshot);
  const { status, error } = useMcpStatus(bridge, lifecycle, visible && !expired);
  const [contextId, setContextId] = useState('');
  const [submitted, setSubmitted] = useState<McpTaskSummary | null>(null);
  const [notice, setNotice] = useState('');
  const [working, setWorking] = useState(false);
  const active = useRef(false);
  const alive = useRef(true);
  const identity = JSON.stringify([selection.selectionId, selection.workspaceId, selection.buildId, selection.generation, scope.targetId, scope.sessionId, lifecycle]);
  const current = useRef({ identity, expired, visible, snapshot }); current.current = { identity, expired, visible, snapshot };
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { setContextId(''); setSubmitted(null); setNotice(''); }, [identity]);
  const eligible = status?.contexts.filter(context => contextMatches(snapshot, context)) ?? [];
  const selectedContext = eligible.find(context => context.contextId === contextId);
  const task = status?.tasks.find(item => item.taskId === submitted?.taskId) ?? submitted;
  const stillCurrent = (requested: string) => alive.current && current.current.visible && !current.current.expired && current.current.identity === requested;
  async function perform(action: (requested: string) => Promise<void>) {
    if (disabled || active.current || !bridge.transfer || !visible || expired) return;
    const requested = identity;
    active.current = true; setWorking(true); busy(true); setNotice('');
    try { await action(requested); }
    catch (reason) { if (stillCurrent(requested)) setNotice(describeError(reason)); }
    finally { active.current = false; if (alive.current) { setWorking(false); busy(false); } }
  }
  async function copy() {
    await perform(async requested => {
      const saved = await save();
      if (!stillCurrent(requested)) return;
      const prompt = await bridge.transfer!('selection_prompt', { selectionId: saved.selectionId, detailed: false }, scope);
      if (!stillCurrent(requested)) return;
      await navigator.clipboard.writeText(prompt);
      if (stillCurrent(requested)) setNotice('修改要求已复制，可粘贴到客户端。需要图片时可在下方导出。');
    });
  }
  async function submit() {
    if (!selectedContext || !status?.available || task) return;
    const context = selectedContext;
    await perform(async requested => {
      const result = await submitFrozenSelection({ context, selectionId: selection.selectionId, save,
        isCurrent: () => stillCurrent(requested), snapshot: () => current.current.snapshot,
        submit: (selectionId, contextId) => bridge.transfer!('mcp_submit_task', { contextId, selectionId }, scope) });
      if (result && stillCurrent(requested)) { setSubmitted(result); setNotice('已提交修改意见与截图快照，等待客户端领取。'); }
    });
  }
  async function copyAutomatic() {
    if (!selectedContext) return;
    const contextId = selectedContext.contextId;
    await perform(async requested => {
      await navigator.clipboard.writeText(automaticProcessingPrompt(contextId));
      if (stillCurrent(requested)) setNotice('自动处理提示词已复制。请在对应的 Agent 会话中发送一次，保持等待后再提交任务。');
    });
  }
  return <section className="selection-target agent-delivery" aria-label="提交修改任务到 MCP">
    <header><h4>交给 MCP 客户端</h4><span className="agent-mode">明确选择上下文</span></header>
    <p className="agent-hint">提交会保存当前修改意见，并冻结组件与截图。在 Agent 会话中启动一次自动处理后，持续等待期间提交即可自动领取；源码变更仍需你验收。</p>
    <label>接收上下文<select aria-label="MCP 接收上下文" value={selectedContext?.contextId ?? ''} disabled={disabled || working || Boolean(task)} onChange={event => setContextId(event.target.value)}>
      <option value="">请选择已确认的上下文</option>{eligible.map(context => <option key={context.contextId} value={context.contextId}>{context.displayName || context.clientName} · {context.contextId}</option>)}
    </select></label>
    {status && eligible.length === 0 && <p className="agent-hint">请先在「MCP 接入」中连接客户端，确认用于当前工程的上下文，并信任工程。</p>}
    <button className="primary-button selection-primary" disabled={disabled || working || !hasInstruction || !selectedContext || !status?.available || Boolean(task)} onClick={() => void submit()}>{working ? '正在处理…' : task ? '已提交到 MCP' : '提交到 MCP'}</button>
    <div className="composer-actions"><button disabled={disabled || working || !selectedContext || !status?.available} onClick={() => void copyAutomatic()}>复制自动处理提示词</button>
      <button disabled={disabled || working || !hasInstruction} onClick={() => void copy()}>复制修改要求</button></div>
    {task && <output className="agent-receipt" aria-live="polite"><strong>{taskLabels[task.state]}</strong><code>{task.taskId}</code>{task.summary && <p>{task.summary}</p>}{task.validation && <p>Agent 验证说明：{task.validation}</p>}
      {task.state === 'completed' && <p>Agent 报告完成，请检查源码与验证结果。</p>}{task.state === 'unknown' && <p>连接已中断。请检查已有修改；需要重试时，在「MCP 接入」中明确重新排队。</p>}</output>}
    <p className="agent-hint">队列仅在本次运行中保留，重启 ArkDock 后清空。</p>
    {(notice || error) && <output className="composer-notice" aria-live="polite">{notice || error}</output>}
  </section>;
}
