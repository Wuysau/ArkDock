import { useEffect, useRef, useState } from 'react';
import type { AppSnapshot, NativeControlIdentity, NativeControlReceipt, NativeControlRequest, NativeControlState, RequestScope } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';
import type { FrameStore } from '../frame-store.ts';
import type { PreviewFrame } from './frame-types.ts';
import { deriveUiState } from '../view-state.ts';
import { controlRequest, NativeInputController } from './NativeInputController.ts';

export const identityEqual = (left: NativeControlIdentity | null | undefined, right: NativeControlIdentity | null | undefined): boolean => Boolean(left && right
  && left.workspaceId === right.workspaceId && left.sessionId === right.sessionId && left.buildId === right.buildId
  && left.generation === right.generation && left.width === right.width && left.height === right.height);

export function displayedIdentity(snapshot: AppSnapshot | null, frame: PreviewFrame | null): NativeControlIdentity | null {
  if (!snapshot?.project || !frame || !deriveUiState(snapshot, frame).frameCurrent) return null;
  return { workspaceId: snapshot.project.id, sessionId: frame.sessionId, buildId: frame.buildId,
    generation: frame.generation, width: frame.width, height: frame.height };
}
export function controlAvailable(state: NativeControlState | null, name: string): boolean {
  const capability = state?.capabilities.find((item) => item.name === name);
  return Boolean(capability && (capability.status === 'available' || state?.experimentalEnabled && capability.status === 'experimental'));
}

export function useNativeControls({ bridge, snapshot, store, visible, wake }: {
  bridge: UiBridge; snapshot: AppSnapshot | null; store: FrameStore; visible: boolean; wake: () => void;
}) {
  const [state, setState] = useState<NativeControlState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  const context = useRef({ snapshot, visible, state }); context.current = { snapshot, visible, state };
  const occupied = useRef(false);
  const inputQueue = useRef<{ request: NativeControlRequest; scope: RequestScope }[]>([]);
  const draining = useRef(false);
  const active = useRef(true);
  const wakeRef = useRef(wake); wakeRef.current = wake;
  const scope = (): RequestScope => ({ targetId: context.current.snapshot?.target?.id ?? null, sessionId: context.current.snapshot?.sessionId ?? null });
  const currentState = (value: NativeControlState) => {
    const actual = context.current.snapshot;
    return Boolean(actual && value.identity && value.identity.workspaceId === actual.project?.id && value.identity.sessionId === actual.sessionId
      && value.identity.buildId === actual.build.buildId && value.identity.generation === actual.build.generation);
  };
  const sameRuntime = (identity: NativeControlIdentity) => {
    const actual = context.current.snapshot;
    return Boolean(actual && actual.project?.id === identity.workspaceId && actual.sessionId === identity.sessionId
      && actual.build.buildId === identity.buildId && actual.build.generation === identity.generation);
  };
  async function awaitOperation(receipt: NativeControlReceipt, requestScope: RequestScope, commandOnly: boolean, inputReady = false): Promise<NativeControlReceipt> {
    if (!bridge.transfer) throw new Error('缺少原生控制桥接。');
    const deadline = performance.now() + 20_000;
    let latest = receipt;
    // The submit response is only Queued. Operation IDs are polled without
    // committing every touch ACK to React application state.
    while (performance.now() < deadline) {
      if (['failed', 'timedOut', 'blocked'].includes(latest.phase)) throw new Error(latest.reason);
      const terminal = latest.phase === 'cancelled' || latest.phase === 'observed'
        || commandOnly && latest.commandAccepted && latest.phase === 'commandAccepted';
      if (terminal && !inputReady) return latest;
      const value = await bridge.transfer('preview_control_state', {}, requestScope);
      const operation = value.operations.find((item) => item.operationId === receipt.operationId);
      if (operation) latest = operation;
      if (['failed', 'timedOut', 'blocked'].includes(latest.phase)) throw new Error(latest.reason);
      const finished = latest.phase === 'cancelled' || latest.phase === 'observed'
        || commandOnly && latest.commandAccepted && latest.phase === 'commandAccepted';
      if (finished && (!inputReady || value.inputReady)) return latest;
      if (!sameRuntime(receipt.identity)) throw new Error('原生会话或构建已变化，旧操作不再继续。');
      await new Promise<void>((resolve) => setTimeout(resolve, 60));
    }
    throw new Error('等待原生操作确认超时；命令接受与画面完成仍未得到确认。');
  }
  const accept = (value: NativeControlState) => {
    if (!active.current || !currentState(value)) return;
    context.current.state = value;
    setState((current) => JSON.stringify(current) === JSON.stringify(value) ? current : value);
  };
  const [input] = useState(() => new NativeInputController(async (request, requestScope) => {
    if (!bridge.transfer) throw new Error('缺少原生控制桥接。');
    wakeRef.current();
    const queued = await bridge.transfer('preview_control_submit', { request }, requestScope);
    return awaitOperation(queued, requestScope, true);
  }, async (identity, requestScope) => {
    if (bridge.transfer) {
      const queued = await bridge.transfer('preview_control_cancel', { identity }, requestScope);
      return awaitOperation(queued, requestScope, false, true);
    }
  }, (reason) => { if (active.current) setError(String(reason)); }));
  useEffect(() => { active.current = true; return () => { active.current = false; void input.cancel(); }; }, [input]);
  const runtimeKey = JSON.stringify([snapshot?.target?.id, snapshot?.project?.id, snapshot?.sessionId, snapshot?.build.buildId, snapshot?.build.generation]);
  useEffect(() => {
    setState(null); setError(null); inputQueue.current = []; void input.cancel();
    if (!bridge.transfer || !snapshot?.target || !snapshot.sessionId || !snapshot.project) return;
    let live = true;
    let timer: ReturnType<typeof setTimeout>;
    const requestScope = scope();
    async function read() {
      try {
        const value = await bridge.transfer!('preview_control_state', {}, requestScope);
        if (live) accept(value);
      } catch (reason) { if (live && active.current) setError(String(reason)); }
      finally { if (live) timer = setTimeout(() => void read(), 800); }
    }
    void read();
    return () => { live = false; clearTimeout(timer); };
  }, [bridge, runtimeKey, input]);
  useEffect(() => {
    const cancelLocalInput = () => {
      inputQueue.current = [];
      const identity = context.current.state?.identity;
      const requestScope = scope();
      void (async () => {
        await input.cancel();
        if (!identity || !bridge.transfer) return;
        try { const queued = await bridge.transfer('preview_control_cancel', { identity }, requestScope); await awaitOperation(queued, requestScope, false, true); }
        catch (reason) { if (active.current && identityEqual(identity, context.current.state?.identity)) setError(String(reason)); }
      })();
    };
    if (!visible) cancelLocalInput();
    const blur = () => cancelLocalInput();
    const visibility = () => { if (document.hidden) cancelLocalInput(); };
    window.addEventListener('blur', blur); document.addEventListener('visibilitychange', visibility);
    return () => { window.removeEventListener('blur', blur); document.removeEventListener('visibilitychange', visibility); };
  }, [visible, input, bridge]);
  function binding(frame: PreviewFrame, name: string) {
    const actual = context.current;
    const identity = displayedIdentity(actual.snapshot, frame);
    return identity && actual.visible && !document.hidden && document.hasFocus() && !occupied.current && actual.state?.experimentalEnabled && actual.state.inputReady
      && identityEqual(identity, actual.state.identity) && controlAvailable(actual.state, name) ? { identity, scope: scope() } : null;
  }
  async function submit(fields: Partial<Omit<NativeControlRequest, 'identity'>> & Pick<NativeControlRequest, 'action'>): Promise<void> {
    if (!bridge.transfer || occupied.current) return;
    const identity = displayedIdentity(context.current.snapshot, store.getDisplayed());
    if (!identity || !identityEqual(identity, context.current.state?.identity)) { setError('等待当前配置的有效原生画面后再操作。'); return; }
    occupied.current = true; inputQueue.current = []; setPending(fields.action); setError(null);
    const requestScope = scope();
    try {
      await input.cancel(); wakeRef.current();
      const cancelled = await bridge.transfer('preview_control_cancel', { identity }, requestScope);
      await awaitOperation(cancelled, requestScope, false, true);
      const queued = await bridge.transfer('preview_control_submit', { request: controlRequest(identity, fields) }, requestScope);
      const receipt = await awaitOperation(queued, requestScope, fields.action === 'queryRouter');
      if (receipt.phase === 'cancelled') throw new Error(receipt.reason);
      accept(await bridge.transfer('preview_control_state', {}, requestScope));
    } catch (reason) { if (identityEqual(identity, displayedIdentity(context.current.snapshot, store.getDisplayed()))) setError(String(reason)); }
    finally { occupied.current = false; if (active.current) setPending(null); }
  }
  async function enable(enabled: boolean) {
    if (!bridge.transfer || occupied.current) return;
    occupied.current = true; inputQueue.current = []; setPending('enable'); setError(null);
    try { await input.cancel(); accept(await bridge.transfer('preview_control_enable', { enabled }, scope())); }
    catch (reason) { if (active.current) setError(String(reason)); }
    finally { occupied.current = false; if (active.current) setPending(null); }
  }
  async function cancel() {
    inputQueue.current = [];
    await input.cancel();
    const identity = context.current.state?.identity;
    if (!identity || !bridge.transfer) return;
    try {
      const requestScope = scope();
      const queued = await bridge.transfer('preview_control_cancel', { identity }, requestScope);
      await awaitOperation(queued, requestScope, false, true);
      accept(await bridge.transfer('preview_control_state', {}, requestScope));
    }
    catch (reason) { if (active.current) setError(String(reason)); }
  }
  function enqueueInput(fields: Partial<Omit<NativeControlRequest, 'identity'>> & Pick<NativeControlRequest, 'action'>, name: 'text' | 'key') {
    const frame = store.getDisplayed(); const value = frame && binding(frame, name);
    if (!value || !bridge.transfer) return;
    if (inputQueue.current.length >= 32) { setError('文字与按键输入队列已满；未提交本次输入。'); return; }
    inputQueue.current.push({ request: controlRequest(value.identity, fields), scope: value.scope });
    if (draining.current) return;
    draining.current = true;
    void (async () => {
      try {
        while (inputQueue.current.length && active.current) {
          const next = inputQueue.current.shift()!;
          if (occupied.current || !context.current.visible || document.hidden || !document.hasFocus()
            || !identityEqual(next.request.identity, displayedIdentity(context.current.snapshot, store.getDisplayed()))
            || !identityEqual(next.request.identity, context.current.state?.identity) || !context.current.state?.inputReady) {
            inputQueue.current = []; break;
          }
          wakeRef.current();
          const queued = await bridge.transfer!('preview_control_submit', { request: next.request }, next.scope);
          const receipt = await awaitOperation(queued, next.scope, true);
          if (['failed', 'timedOut', 'blocked', 'cancelled'].includes(receipt.phase)) throw new Error(receipt.reason);
        }
      } catch (reason) { inputQueue.current = []; if (active.current) setError(`输入未完成：${String(reason)}`); }
      finally { draining.current = false; }
    })();
  }
  function enqueueText(text: string) {
    if (!text) return;
    if (/[\u0000\r\n]/u.test(text)) { setError('文字提交包含换行或无效字符；换行请使用 Enter 按键。'); return; }
    const scalars = Array.from(text);
    const chunks: string[] = [];
    for (let offset = 0; offset < scalars.length; offset += 128) chunks.push(scalars.slice(offset, offset + 128).join(''));
    if (chunks.length + inputQueue.current.length > 32) { setError('文字输入超出有界队列；本次提交未派发。'); return; }
    for (const chunk of chunks) enqueueInput({ action: 'textCommit', text: chunk }, 'text');
  }
  return { state, error, pending, input, submit, enable, cancel,
    start: (frame: PreviewFrame, x: number, y: number) => { const value = binding(frame, 'touch'); return value ? input.start(value.identity, value.scope, { x, y }) : false; },
    text: enqueueText,
    key: (key: NonNullable<NativeControlRequest['key']>) => enqueueInput({ action: 'keyPress', key }, 'key'),
  };
}

export type NativeControls = ReturnType<typeof useNativeControls>;
