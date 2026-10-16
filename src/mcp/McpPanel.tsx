import { useEffect, useRef, useState } from 'react';
import type { AppSnapshot } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';
import { describeError } from '../ipc/errors.ts';
import { currentScope } from '../request-state.ts';
import { canRetry, contextMatches, mcpLifecycle, taskLabels } from './model.ts';
import { useMcpStatus } from './useMcpStatus.ts';
import './mcp.css';

export function McpPanel({ bridge, snapshot, visible, busy, selectContext }: {
  bridge: UiBridge; snapshot: AppSnapshot | null; visible: boolean; busy: boolean; selectContext: (registrationId: string) => void;
}) {
  const lifecycle = mcpLifecycle(snapshot);
  const { status, error } = useMcpStatus(bridge, lifecycle, visible);
  const [notice, setNotice] = useState('');
  const [pending, setPending] = useState(false);
  const [retryContextId, setRetryContextId] = useState('');
  const operation = useRef(false);
  const current = useRef(lifecycle); current.current = lifecycle;
  // Each mount and lifecycle owns its notices, never a late action from another project.
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  useEffect(() => { setRetryContextId(''); }, [lifecycle]);
  const [noticeKey, setNoticeKey] = useState(lifecycle);
  async function perform(action: () => Promise<string>) {
    if (operation.current || busy) return;
    const requested = lifecycle;
    operation.current = true; setPending(true); setNotice(''); setNoticeKey(requested);
    try { const message = await action(); if (mounted.current && current.current === requested) setNotice(message); }
    catch (reason) { if (mounted.current && current.current === requested) setNotice(describeError(reason)); }
    finally { operation.current = false; if (mounted.current) setPending(false); }
  }
  return <div className="mcp-panel">
    <section className="settings-card"><h3>客户端配置</h3>
      <p className="muted">在客户端添加 stdio MCP 服务器。复制的路径来自当前 ArkDock 可执行文件，无需单独安装服务。</p>
      {!bridge.available ? <output>桌面桥接未连接。</output> : !status && !error ? <output>正在读取接入配置…</output> : null}
      {status && <><label>命令<input readOnly value={status.configuration.command} aria-label="MCP 命令" /></label>
        <label>参数<input readOnly value={JSON.stringify(status.configuration.args)} aria-label="MCP 参数" /></label>
        <details><summary>JSON 配置</summary><pre>{status.configuration.json}</pre></details>
        <div className="mcp-actions"><button disabled={pending} onClick={() => void perform(async () => { await navigator.clipboard.writeText(status.configuration.json); return 'MCP JSON 配置已复制。'; })}>复制 JSON 配置</button>
          <button disabled={pending} onClick={() => void perform(async () => { await navigator.clipboard.writeText(JSON.stringify({ command: status.configuration.command, args: status.configuration.args }, null, 2)); return '命令和参数已复制。'; })}>复制命令与参数</button></div>
        <p className="muted">{status.available ? 'MCP 服务可用。' : status.reason}</p></>}
    </section>
    <section className="settings-card"><h3>上下文</h3><p className="muted">请在客户端调用连接工具，并明确原始工程及实际执行目录。这里的连接不证明当前可见对话或代码已修改。</p>
      {status && status.contexts.length === 0 && <p>尚无 MCP 上下文。</p>}
      {status?.contexts.map(context => <article className="mcp-entry" key={context.contextId}>
        <strong>{context.displayName || context.clientName}</strong><span className="muted">{context.clientName} · {context.connected ? '在线' : '离线'} · {context.confirmed && snapshot?.contextSource?.boundRegistrationId === context.registrationId ? '已确认' : '待确认'}</span>
        <code>{context.contextId}</code><span>原始工程</span><code>{context.originalProject}</code><span>实际执行目录</span><code>{context.executionDirectory}</code>
        <div className="mcp-actions"><button disabled={busy || pending || !snapshot?.target || !context.connected || context.environment !== 'windowsLocal' || snapshot.contextSource?.boundRegistrationId === context.registrationId}
          onClick={() => selectContext(context.registrationId)}>确认用于当前预览</button>
          <button disabled={busy || pending || !bridge.mcpRevokeContext} onClick={() => void perform(async () => { await bridge.mcpRevokeContext!(context.contextId); return '该上下文已撤销；相关任务访问已失效。'; })}>撤销上下文</button></div>
      </article>)}
    </section>
    <section className="settings-card"><h3>任务队列</h3><p className="muted">任务保留提交时的修改意见与截图。队列只在本次运行中保留，重启 ArkDock 后清空；未领取任务在构建变化时过期。</p>
      {status?.tasks.some(task => task.state === 'failed' || task.state === 'unknown') && <label>重试接收上下文<select aria-label="重试接收上下文" value={retryContextId} onChange={event => setRetryContextId(event.target.value)} disabled={pending || busy}>
        <option value="">请明确选择当前已确认的上下文</option>{status.contexts.filter(context => contextMatches(snapshot, context)).map(context => <option key={context.contextId} value={context.contextId}>{context.displayName || context.clientName} · {context.contextId}</option>)}
      </select></label>}
      {status && status.tasks.length === 0 && <p>暂无任务。选择预览组件并填写修改意见后，可提交到 MCP。</p>}
      {status?.tasks.map(task => <article className="mcp-entry" key={task.taskId}>
        <strong>{taskLabels[task.state]}</strong><code>{task.taskId}</code><span className="muted">{status.contexts.find(context => context.contextId === task.contextId)?.displayName || task.contextId}</span>
        {task.summary && <p>{task.summary}</p>}{task.validation && <p>Agent 验证说明：{task.validation}</p>}
        {task.state === 'completed' && <p className="muted">完成状态由 Agent 上报；请自行检查源码与验证结果。</p>}
        {task.state === 'unknown' && <p className="muted">请检查已有修改后再决定是否重试。</p>}
        {(task.state === 'unknown' || task.state === 'failed') && <button disabled={busy || pending || !bridge.transfer || !canRetry(snapshot, status.contexts.find(context => context.contextId === retryContextId), task)} onClick={() => void perform(async () => {
          await bridge.transfer!('mcp_retry_task', { taskId: task.taskId, contextId: retryContextId }, currentScope(snapshot)); return '任务已重新排队，等待客户端领取。';
        })}>重新排队</button>}
      </article>)}
    </section>
    {error && <p role="alert">{error}</p>}{noticeKey === lifecycle && notice && <output aria-live="polite">{notice}</output>}
  </div>;
}
