import { useEffect, useRef, useState } from 'react';
import type { AgentDelivery, AssistedOffer, RequestScope, SelectionContext } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';

/** Copy or fill a confirmed composer. This UI never queues or submits an Agent message. */
export function AgentDeliveryPanel({ selection, scope, bridge, disabled, expired, hasInstruction, save, busy }: {
  selection: SelectionContext; scope: RequestScope; bridge: UiBridge;
  disabled: boolean; expired: boolean; hasInstruction: boolean;
  save: () => Promise<SelectionContext>; busy: (value: boolean) => void;
}) {
  const [offer, setOffer] = useState<AssistedOffer | null>(null);
  const [delivery, setDelivery] = useState<AgentDelivery | null>(null);
  const [notice, setNotice] = useState('');
  const [working, setWorking] = useState(false);
  const active = useRef(false);
  const alive = useRef(true);
  const identity = JSON.stringify([selection.selectionId, selection.workspaceId, selection.buildId, selection.generation, scope.targetId, scope.sessionId]);
  const current = useRef({ identity, expired }); current.current = { identity, expired };
  const { targetId, sessionId } = scope;
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => { setOffer(null); setDelivery(null); setNotice(''); }, [identity, expired]);
  useEffect(() => {
    if (!offer) return;
    const timer = window.setTimeout(() => {
      setOffer(value => value?.confirmationId === offer.confirmationId ? null : value);
      setNotice('输入框确认已过期，请重新检查。');
    }, Math.max(0, offer.expiresAt - Date.now()));
    return () => window.clearTimeout(timer);
  }, [offer]);
  const usable = Boolean(offer && offer.targetId === targetId && offer.expiresAt > Date.now());
  const stillCurrent = (requested: string) => alive.current && !current.current.expired && current.current.identity === requested;
  async function perform(task: (requested: string) => Promise<void>) {
    if (disabled || active.current || !bridge.transfer) return;
    const requested = identity;
    active.current = true; setWorking(true); busy(true); setNotice('');
    try { await task(requested); }
    catch (error) { if (stillCurrent(requested)) setNotice(String(error)); }
    finally {
      active.current = false;
      if (alive.current) { setWorking(false); busy(false); }
    }
  }
  async function copy() {
    await perform(async requested => {
      const saved = await save();
      if (!stillCurrent(requested)) return;
      const prompt = await bridge.transfer!('selection_prompt', { selectionId: saved.selectionId, detailed: false }, { targetId, sessionId });
      if (!stillCurrent(requested)) return;
      await navigator.clipboard.writeText(prompt);
      if (stillCurrent(requested)) setNotice('精简修改要求已复制。粘贴到 Agent 输入框后，由你决定是否发送。');
    });
  }
  async function inspect() {
    await perform(async requested => {
      setOffer(null); setDelivery(null);
      try {
        const result = await bridge.transfer!('agent_assisted_inspect', {}, { targetId, sessionId });
        if (stillCurrent(requested) && result.targetId === targetId) setOffer(result);
      } catch (error) {
        if (stillCurrent(requested)) setNotice(`无法确认可填写的输入框：${String(error)}\n可复制修改要求后手动粘贴。`);
      }
    });
  }
  async function fill() {
    if (!usable || !offer) return;
    const confirmationId = offer.confirmationId;
    await perform(async requested => {
      // Consume the local confirmation too; no automatic retry after an uncertain write.
      setOffer(null); setDelivery(null);
      const saved = await save();
      if (!stillCurrent(requested)) return;
      const result = await bridge.transfer!('agent_assisted_fill', { selectionId: saved.selectionId, confirmationId }, { targetId, sessionId });
      if (stillCurrent(requested)) setDelivery(result);
    });
  }
  return <section className="selection-target agent-delivery" aria-label="复制或填写修改要求">
    <header><h4>交给 Agent 修改</h4><span className="agent-mode">由你发送</span></header>
    <p className="agent-hint">复制后粘贴到对话框。可识别的空输入框也能辅助填写，发送由你决定。</p>
    <button className="primary-button selection-primary" disabled={disabled || working || !hasInstruction} onClick={() => void copy()}>
      {working ? '正在处理…' : '复制修改要求'}
    </button>
    <div className="composer-actions"><button disabled={disabled || working || !targetId || !hasInstruction} title="只检查当前绑定窗口的输入框；不需要 Agent 会话配对" onClick={() => void inspect()}>检查目标输入框</button></div>
    {offer && <div className="agent-confirmation">
      <strong>填写到 {offer.app}</strong><p>输入框：{offer.composerName}</p>
      <p>只填写已确认的空输入框，不按 Enter、不点击发送。确认在 30 秒内有效。</p>
      <div className="composer-actions"><button className="primary-button" disabled={disabled || working || !hasInstruction || !usable} onClick={() => void fill()}>确认填入输入框</button>
        <button disabled={working} onClick={() => setOffer(null)}>取消</button></div>
    </div>}
    {delivery && <output aria-live="polite" className="agent-receipt">
      <strong>{delivery.state === 'FILLED_PENDING_USER_SEND' ? '已填入，等待你发送' : delivery.state === 'UNKNOWN' ? '填写结果尚未确认' : '未完成填写'}</strong>
      <p>{delivery.reason}</p>{delivery.state === 'UNKNOWN' && <p>请先检查目标输入框，避免重复填写。</p>}
    </output>}
    <p className="agent-hint">复制会覆盖剪贴板，不会自动附加截图。需要图片时，可在下方导出并手动添加。</p>
    {notice && <output className="composer-notice" aria-live="polite">{notice}</output>}
  </section>;
}
