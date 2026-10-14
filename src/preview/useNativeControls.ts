import { useCallback, useEffect, useRef, useState } from 'react';
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

export function useNativeControls({ bridge, snapshot, store, visible, windowVisible = true, wake }: {
  bridge: UiBridge; snapshot: AppSnapshot | null; store: FrameStore; visible: boolean; windowVisible?: boolean; wake: () => void;
}) {
  const [state, setState] = useState<NativeControlState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  const context = useRef({ snapshot, visible, windowVisible, state }); context.current = { snapshot, visible, windowVisible, state };
  const occupied = useRef(false);
  const inputQueue = useRef<{ request: NativeControlRequest; scope: RequestScope; serial: number }[]>([]);
  const inputSerial = useRef(0);
  const draining = useRef(false);
  const active = useRef(true);
  const wakeRef = useRef(wake); wakeRef.current = wake;
  const scope = (): RequestScope => ({ targetId: context.current.snapshot?.target?.id ?? null, sessionId: context.current.snapshot?.sessionId ?? null });
  const currentState = (value: NativeControlState) => {
    const actual = context.current.snapshot;
    return Boolean(actual && value.identity && value.identity.workspaceId === actual.project?.id && value.identity.sessionId === actual.sessionId
      && value.identity.buildId === actual.build.buildId && value.identity.generation === actual.build.generation);
  };
  const sameRuntime = useCallback((identity: NativeControlIdentity) => {
    const actual = context.current.snapshot;
    return Boolean(actual && actual.project?.id === identity.workspaceId && actual.sessionId === identity.sessionId
      && actual.build.buildId === identity.buildId && actual.build.generation === identity.generation);
  }, []);
  const awaitOperation = useCallback(async (receipt: NativeControlReceipt, requestScope: RequestScope, commandOnly: boolean, inputReady = false): Promise<NativeControlReceipt> => {
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
  }, [bridge, sameRuntime]);
  const operationRef = useRef(awaitOperation); operationRef.current = awaitOperation;
  const accept = (value: NativeControlState) => {
    if (!active.current || !currentState(value)) return;
    context.current.state = value;
    setState((current) => JSON.stringify(current) === JSON.stringify(value) ? current : value);
  };
  const [input] = useState(() => new NativeInputController(async (request, requestScope) => {
    if (!bridge.transfer) throw new Error('缺少原生控制桥接。');
    wakeRef.current();
    const queued = await bridge.transfer('preview_control_submit', { request }, requestScope);
    return operationRef.current(queued, requestScope, true);
  }, async (identity, requestScope) => {
    if (bridge.transfer) {
      const queued = await bridge.transfer('preview_control_submit', { request: controlRequest(identity, { action: 'pointerCancel' }) }, requestScope);
      return operationRef.current(queued, requestScope, false);
    }
  }, (reason, identity) => { if (active.current && sameRuntime(identity)) setError(String(reason)); }));
  useEffect(() => { active.current = true; return () => { active.current = false; void input.cancel(); }; }, [input]);
  const runtimeKey = JSON.stringify([snapshot?.target?.id, snapshot?.project?.id, snapshot?.sessionId, snapshot?.build.buildId, snapshot?.build.generation]);
  useEffect(() => {
    setState(null); setError(null); inputSerial.current++; inputQueue.current = []; void input.cancel();
    const actual = context.current.snapshot;
    if (!bridge.transfer || !actual?.target || !actual.sessionId || !actual.project) return;
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
    const cancelLocalInput = (all: boolean) => {
      inputSerial.current++; inputQueue.current = [];
      const identity = context.current.state?.identity;
      const requestScope = scope();
      void (async () => {
        await input.cancel();
        if (!identity || !bridge.transfer) return;
        try {
          const queued = all ? await bridge.transfer('preview_control_cancel', { identity }, requestScope)
            : await bridge.transfer('preview_control_submit', { request: controlRequest(identity, { action: 'pointerCancel' }) }, requestScope);
          await awaitOperation(queued, requestScope, false);
        }
        catch (reason) { if (active.current && identityEqual(identity, context.current.state?.identity)) setError(String(reason)); }
      })();
    };
    if (!windowVisible) cancelLocalInput(true);
    else if (!visible) cancelLocalInput(false);
    const blur = () => cancelLocalInput(false);
    // WebView2 document.hidden is not the owned main window's visibility.
    const visibility = () => { if (!context.current.windowVisible) cancelLocalInput(true); };
    window.addEventListener('blur', blur); document.addEventListener('visibilitychange', visibility);
    return () => { window.removeEventListener('blur', blur); document.removeEventListener('visibilitychange', visibility); };
  }, [visible, windowVisible, input, bridge, awaitOperation]);
  function binding(frame: PreviewFrame, name: string) {
    const actual = context.current;
    const identity = displayedIdentity(actual.snapshot, frame);
    if (!identity || !identityEqual(identity, actual.state?.identity)) {
      setError('当前原生画面或配置已变化；本次输入未提交。'); return null;
    }
    if (!actual.windowVisible || !actual.visible || !document.hasFocus()) {
      setError('请先聚焦 ArkDock 的实际预览画面；隐藏或检查模式不接收输入。'); return null;
    }
    if (occupied.current || !actual.state?.inputReady) {
      setError('原生配置操作尚未完成；本次输入未提交。'); return null;
    }
    if (!actual.state.experimentalEnabled || !controlAvailable(actual.state, name)) {
      setError('当前文字或触控能力未启用；本次输入未提交。'); return null;
    }
    return { identity, scope: scope() };
  }
  async function submit(fields: Partial<Omit<NativeControlRequest, 'identity'>> & Pick<NativeControlRequest, 'action'>): Promise<void> {
    if (!bridge.transfer || occupied.current) return;
    const identity = displayedIdentity(context.current.snapshot, store.getDisplayed());
    if (!identity || !identityEqual(identity, context.current.state?.identity)) { setError('等待当前配置的有效原生画面后再操作。'); return; }
    occupied.current = true; inputSerial.current++; inputQueue.current = []; setPending(fields.action); setError(null);
    const requestScope = scope();
    try {
      await input.cancel(); wakeRef.current();
      const cancelled = await bridge.transfer('preview_control_cancel', { identity }, requestScope);
      await awaitOperation(cancelled, requestScope, false, true);
      const queued = await bridge.transfer('preview_control_submit', { request: controlRequest(identity, fields) }, requestScope);
      // ColorMode may settle as accepted with no redraw after its observation
      // window. Keep the receipt's frame/configuration flags; do not invent a timeout.
      const receipt = await awaitOperation(queued, requestScope, fields.action === 'queryRouter' || fields.action === 'setColorMode');
      if (receipt.phase === 'cancelled') throw new Error(receipt.reason);
      accept(await bridge.transfer('preview_control_state', {}, requestScope));
    } catch (reason) { if (active.current && sameRuntime(identity)) setError(String(reason)); }
    finally { occupied.current = false; if (active.current) setPending(null); }
  }
  async function enable(enabled: boolean) {
    if (!bridge.transfer || occupied.current) return;
    occupied.current = true; inputSerial.current++; inputQueue.current = []; setPending('enable'); setError(null);
    try { await input.cancel(); accept(await bridge.transfer('preview_control_enable', { enabled }, scope())); }
    catch (reason) { if (active.current) setError(String(reason)); }
    finally { occupied.current = false; if (active.current) setPending(null); }
  }
  async function cancel() {
    inputSerial.current++; inputQueue.current = [];
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
  async function cancelInput() {
    inputSerial.current++; inputQueue.current = [];
    const identity = context.current.state?.identity;
    const requestScope = scope();
    await input.cancel();
    if (!identity || !bridge.transfer) return;
    try {
      const queued = await bridge.transfer('preview_control_submit', { request: controlRequest(identity, { action: 'pointerCancel' }) }, requestScope);
      // An input-only cancellation must not wait for unrelated theme/page configuration.
      await awaitOperation(queued, requestScope, false);
    } catch (reason) { if (active.current && sameRuntime(identity)) setError(String(reason)); }
  }
  function enqueueInput(fields: Partial<Omit<NativeControlRequest, 'identity'>> & Pick<NativeControlRequest, 'action'>, name: 'text' | 'key') {
    const frame = store.getDisplayed();
    if (!frame) { setError('当前没有有效原生画面；本次输入未提交。'); return; }
    const value = binding(frame, name);
    if (!value) return;
    if (!bridge.transfer) { setError('缺少原生控制桥接；本次输入未提交。'); return; }
    if (inputQueue.current.length >= 32) { setError('文字与按键输入队列已满；未提交本次输入。'); return; }
    inputQueue.current.push({ request: controlRequest(value.identity, fields), scope: value.scope, serial: inputSerial.current });
    drainInput();
  }
  function drainInput() {
    if (draining.current || !inputQueue.current.length || !active.current) return;
    draining.current = true;
    void (async () => {
      let serial = inputSerial.current;
      try {
        while (inputQueue.current.length && active.current) {
          const next = inputQueue.current.shift()!;
          serial = next.serial;
          const released = await input.waitForRelease(next.request.identity, next.scope);
          if (next.serial !== inputSerial.current) continue;
          if (!released) { inputQueue.current = []; setError('原生点击未确认释放；等待中的文字与按键已取消。'); break; }
          if (occupied.current || !context.current.windowVisible || !context.current.visible || !document.hasFocus()
            || !identityEqual(next.request.identity, displayedIdentity(context.current.snapshot, store.getDisplayed()))
            || !identityEqual(next.request.identity, context.current.state?.identity) || !context.current.state?.inputReady
            || next.scope.targetId !== scope().targetId || next.scope.sessionId !== scope().sessionId) {
            inputQueue.current = []; setError('输入焦点或原生会话已变化；等待中的文字与按键未提交。'); break;
          }
          wakeRef.current();
          const queued = await bridge.transfer!('preview_control_submit', { request: next.request }, next.scope);
          const receipt = await awaitOperation(queued, next.scope, true);
          if (['failed', 'timedOut', 'blocked', 'cancelled'].includes(receipt.phase)) throw new Error(receipt.reason);
        }
      } catch (reason) {
        if (serial === inputSerial.current) {
          inputQueue.current = [];
          if (active.current) setError(`输入未完成：${String(reason)}`);
        }
      }
      finally {
        draining.current = false;
        // A new local focus may enqueue after cancellation while the old
        // request is still completing. Its own identity/serial gates still apply.
        if (inputQueue.current.length && active.current) drainInput();
      }
    })();
  }
  function enqueueText(text: string) {
    if (!text) return;
    if (text.includes(String.fromCodePoint(0)) || /[\r\n]/u.test(text)) { setError('文字提交包含换行或无效字符；换行请使用 Enter 按键。'); return; }
    const scalars = Array.from(text);
    const chunks: string[] = [];
    for (let offset = 0; offset < scalars.length; offset += 128) chunks.push(scalars.slice(offset, offset + 128).join(''));
    if (chunks.length + inputQueue.current.length > 32) { setError('文字输入超出有界队列；本次提交未派发。'); return; }
    for (const chunk of chunks) enqueueInput({ action: 'textCommit', text: chunk }, 'text');
  }
  return { state, error, pending, input, submit, enable, cancel, cancelInput,
    start: (frame: PreviewFrame, x: number, y: number) => {
      if (draining.current || inputQueue.current.length) { setError('等待文字或按键提交完成后再点击；本次点击未提交。'); return false; }
      const value = binding(frame, 'touch');
      if (!value) return false;
      setError(null);
      return input.start(value.identity, value.scope, { x, y });
    },
    text: enqueueText,
    key: (key: NonNullable<NativeControlRequest['key']>) => enqueueInput({ action: 'keyPress', key }, 'key'),
  };
}

export type NativeControls = ReturnType<typeof useNativeControls>;
