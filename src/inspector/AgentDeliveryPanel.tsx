import { useEffect, useRef, useState } from 'react';
import type { AgentDelivery, AgentKind, AgentPairing, AgentSessionList, AssistedOffer, RequestScope, SelectionContext } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';

const transportNames = { unsupported: '未接通', explicitBridge: '显式桥接', claudeChannel: 'Claude Channel · Research Preview', codexAppServer: 'ArkDock 管理的 Codex 会话', codexDesktopPlugin: 'Codex 会话插件', assistedWindow: 'Windows 辅助填写' };
export function AgentDeliveryPanel({ selection, sessions, scope, bridge, disabled, revokeDisabled, hasInstruction, save, busy }: {
  selection: SelectionContext; sessions: AgentSessionList | null; scope: RequestScope; bridge: UiBridge;
  disabled: boolean; revokeDisabled: boolean; hasInstruction: boolean; save: () => Promise<SelectionContext>; busy: (value: boolean) => void;
}) {
  const [list, setList] = useState(sessions);
  const [bindingId, setBindingId] = useState(() => sessions?.bindings.filter((b) => b.writable).length === 1 ? sessions.bindings.find((b) => b.writable)!.bindingId : '');
  const [registrationId, setRegistrationId] = useState('');
  const [kind, setKind] = useState<AgentKind>('codex');
  const [pair, setPair] = useState<AgentPairing | null>(null);
  const [offer, setOffer] = useState<AssistedOffer | null>(null);
  const [delivery, setDelivery] = useState<AgentDelivery | null>(null);
  const [notice, setNotice] = useState('');
  const [working, setWorking] = useState(false);
  const alive = useRef(true);
  const pendingStatus = useRef(false);
  const selectedManually = useRef(false);
  const deliveryId = delivery?.requestId;
  const deliveryState = delivery?.state;
  const { targetId, sessionId } = scope;
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => {
    let live = true; let reading = false;
    async function refresh() {
      if (reading || !bridge.transfer) return;
      reading = true;
      try {
        const value = await bridge.transfer('agent_session_list', {}, { targetId, sessionId });
        if (live) setList(value);
      } catch { if (live) setList(null); }
      finally { reading = false; }
    }
    void refresh(); const timer = window.setInterval(() => void refresh(), 1000);
    return () => { live = false; window.clearInterval(timer); };
  }, [bridge, targetId, sessionId]);
  useEffect(() => {
    if (!deliveryId || !deliveryState || ['COMPLETED', 'EXPIRED', 'FAILED', 'FILLED_PENDING_USER_SEND', 'UNSUPPORTED', 'SESSION_UNVERIFIED'].includes(deliveryState)) return;
    let live = true;
    const timer = window.setInterval(() => {
      if (!bridge.transfer || pendingStatus.current) return;
      pendingStatus.current = true;
      void bridge.transfer('agent_delivery_status', { requestId: deliveryId }, { targetId, sessionId })
        .then((value) => { if (live) setDelivery(value); })
        .catch((error) => { if (live) setNotice(String(error)); })
        .finally(() => { pendingStatus.current = false; });
    }, 750);
    return () => { live = false; window.clearInterval(timer); };
  }, [deliveryId, deliveryState, bridge, targetId, sessionId]);
  useEffect(() => {
    const confirmed = list?.bindings.filter((binding) => binding.writable) ?? [];
    if (!selectedManually.current && !bindingId && confirmed.length === 1) setBindingId(confirmed[0].bindingId);
  }, [list, bindingId]);
  const target = list?.bindings.find((binding) => binding.bindingId === bindingId);
  const registration = list?.registrations.find((value) => value.transportInstanceId === target?.transportInstanceId && value.expiresAt > Date.now());
  const direct = Boolean(target?.writable && registration && target.capabilities.directSend && target.expiresAt > Date.now());
  const assisted = Boolean(offer && offer.targetId === targetId && offer.expiresAt > Date.now());
  async function perform(task: () => Promise<void>, revoke = false) {
    if ((revoke ? revokeDisabled : disabled) || working || !bridge.transfer) return;
    setWorking(true); busy(true); setNotice('');
    try { await task(); }
    catch (error) { if (alive.current) setNotice(String(error)); }
    finally { if (alive.current) { setWorking(false); busy(false); } }
  }
  async function primary() {
    await perform(async () => {
      const saved = await save();
      if (direct && target) {
        const prepared = await bridge.transfer!('agent_delivery_prepare', { selectionId: saved.selectionId, bindingId: target.bindingId }, scope);
        const result = prepared.state === 'READY' ? await bridge.transfer!('agent_delivery_send', { requestId: prepared.requestId }, scope) : prepared;
        if (alive.current) setDelivery(result);
      } else if (assisted && offer) {
        const result = await bridge.transfer!('agent_assisted_fill', { selectionId: saved.selectionId, confirmationId: offer.confirmationId }, scope);
        if (alive.current) { setDelivery(result); setOffer(null); }
      } else {
        const prompt = await bridge.transfer!('selection_prompt', { selectionId: saved.selectionId }, scope);
        await navigator.clipboard.writeText(prompt);
        if (alive.current) setNotice('修改要求已复制；请在目标 Agent 中粘贴并自行提交。');
      }
    });
  }
  return <section className="selection-target agent-delivery" aria-label="发送到 Agent">
    <header><h4>发送到 Agent</h4><span className="agent-mode">{direct ? '直接连接' : assisted ? '辅助填写' : '复制提示词'}</span></header>
    {list?.bindings.length ? <label>已确认的目标<select aria-label="发送目标会话" value={bindingId} disabled={disabled || working} onChange={(event) => { selectedManually.current = true; setBindingId(event.target.value); setDelivery(null); setOffer(null); }}><option value="">选择已配对会话</option>{list.bindings.map((binding) => <option key={binding.bindingId} value={binding.bindingId}>{binding.agentKind} · {binding.conversationOrThreadId}</option>)}</select></label> : <p>尚未绑定可写会话。创建配对后，在指定 Agent 中启用对应连接。</p>}
    {target && <dl><dt>Agent / 版本</dt><dd>{target.agentKind} · {target.agentVersion ?? '未核验'}</dd><dt>工程</dt><dd>{selection.workspaceName}</dd><dt>目标进程</dt><dd>PID {target.process.pid} · {target.targetId}</dd><dt>会话</dt><dd>{target.conversationOrThreadId}</dd><dt>通信方式</dt><dd>{transportNames[target.transportKind]}</dd><dt>能力</dt><dd>{direct ? '可提交' : '连接待核验'} · {target.capabilities.images ? '图片已授权读取' : '无图片能力'} · {registration?.busy ? 'Agent 正忙' : '提交前再次核验'}</dd></dl>}
    {offer && <div className="agent-confirmation"><strong>{offer.app} · PID {offer.pid}</strong><p>输入区：{offer.composerName}</p><p>{offer.reason}</p><p>需要在目标窗口中确认提交。30 秒内有效，切换目标将取消。</p><button disabled={working} onClick={() => setOffer(null)}>取消辅助填写</button></div>}
    <button className="primary-button selection-primary" disabled={disabled || working || !hasInstruction || Boolean(direct && registration?.busy)} onClick={() => void primary()}>{working ? '正在处理…' : direct ? '发送到当前 Agent' : assisted ? '填入 Agent 输入框' : '复制修改要求'}</button>
    {!direct && !assisted && <p className="agent-hint">复制会覆盖系统剪贴板，内容包含当前工程和所选组件的信息。</p>}
    {!direct && <button disabled={disabled || working} onClick={() => void perform(async () => {
      const result = await bridge.transfer!('agent_assisted_inspect', {}, scope);
      if (alive.current) setOffer(result);
    })}>检查当前目标输入框</button>}
    {delivery && <output aria-live="polite" className="agent-receipt"><strong>{delivery.state}</strong><p>{delivery.reason}</p>{delivery.transportReceipt && <small>传输回执：{delivery.transportReceipt}</small>}{delivery.agentAcknowledgment && <small>Agent 确认：{delivery.agentAcknowledgment}</small>}{delivery.state === 'FILLED_PENDING_USER_SEND' && <p>已填写，尚未发送。请在目标窗口确认并提交。</p>}</output>}
    <details><summary>连接与授权</summary><label>Agent 类型<select value={kind} disabled={working} onChange={(event) => setKind(event.target.value as AgentKind)}><option value="codex">Codex</option><option value="claude">Claude Code</option></select></label>
      <button disabled={disabled || working} onClick={() => void perform(async () => {
        const result = await bridge.transfer!('agent_pair_create', { agentKind: kind }, scope);
        if (alive.current) { setPair(result); setBindingId(''); setDelivery(null); setNotice('配对文件已保存。按连接指南在指定 Agent 中启用；这一步尚未发送任何组件。'); }
      })}>保存本次配对文件</button>
      {pair && <p>凭证有效期至 {new Date(pair.expiresAt).toLocaleTimeString()}。文件：<code>{pair.configPath}</code>。仅交给指定本地 Agent，请勿提交到 Git。</p>}
      <p>Claude Channel 需要显式启用及会话 Hook；Codex App Server 仅管理新建会话。Desktop 插件需另行启用并证明会话和工程。</p>
      {Boolean(list?.registrations.length) && <><label>已登记的连接<select value={registrationId} onChange={(event) => setRegistrationId(event.target.value)}><option value="">请选择并核对工程 / 会话</option>{list!.registrations.map((r) => <option key={r.registrationId} value={r.registrationId}>{transportNames[r.transportKind]} · {r.conversationOrThreadId} · PID {r.pid}</option>)}</select></label><button disabled={disabled || working || !registrationId} onClick={() => void perform(async () => {
        const result = await bridge.transfer!('agent_registration_bind', { registrationId }, scope);
        if (alive.current) { setBindingId(result.bindingId); setNotice(result.bindingEvidence); setList(await bridge.transfer!('agent_session_list', {}, scope)); }
      })}>确认绑定此会话</button></>}
      <button disabled={revokeDisabled || working || !targetId || !sessionId} onClick={() => void perform(async () => {
        await bridge.transfer!('agent_pair_revoke', {}, scope);
        if (alive.current) { setPair(null); setBindingId(''); setOffer(null); setList(null); setNotice('已撤销会话连接与截图读取授权。'); }
      }, true)}>撤销连接与授权</button>
    </details>
    {notice && <output aria-live="polite">{notice}</output>}
  </section>;
}
