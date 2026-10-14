import { useEffect, useRef, useState } from 'react';
import type { AppSnapshot, DeviceRuntimeAction, DeviceRuntimeKind, DeviceRuntimeState } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';
import { describeError, normalizeError } from '../ipc/errors.ts';
import type { AppError } from '../ipc/errors.ts';
import { Icon } from '../Icon.tsx';

/** Device runtime is separate from Previewer; this panel never reuses its Canvas or frames. */
export function DeviceRuntimePanel({ snapshot, bridge, kind }: { snapshot: AppSnapshot | null; bridge: UiBridge; kind: Exclude<DeviceRuntimeKind, 'unknown'> }) {
  const [state, setState] = useState<DeviceRuntimeState | null>(null);
  const [connectionConsent, setConnectionConsent] = useState(false);
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const latest = useRef(snapshot); latest.current = snapshot;
  const liveRef = useRef(true);
  const serial = useRef(0);
  const key = JSON.stringify([snapshot?.target?.id, snapshot?.project?.id, snapshot?.sessionId, snapshot?.build.buildId, snapshot?.build.generation, kind]);
  const scope = () => ({ targetId: latest.current?.target?.id ?? null, sessionId: latest.current?.sessionId ?? null });
  function belongs(value: DeviceRuntimeState) {
    const actual = latest.current;
    return Boolean(actual && value.identity && value.identity.workspaceId === actual.project?.id && value.identity.sessionId === actual.sessionId
      && value.identity.buildId === actual.build.buildId && value.identity.generation === actual.build.generation);
  }
  useEffect(() => {
    const revisions = serial;
    liveRef.current = true; serial.current++; setState(null); setError(null); setPending(null); setConnectionConsent(false);
    if (bridge.transfer && latest.current?.sessionId && latest.current.target) {
      const revision = serial.current;
      void bridge.transfer('device_runtime_state', {}, scope()).then((value) => {
        if (liveRef.current && revision === serial.current && belongs(value)) setState(value);
      }).catch((reason) => { if (liveRef.current && revision === serial.current) setError(normalizeError(reason)); });
    }
    return () => { liveRef.current = false; revisions.current++; };
  }, [bridge, key]);
  async function act(action: DeviceRuntimeAction, targetId: string | null = null) {
    if (!bridge.transfer || !state?.identity || pending && action !== 'cancel' || (action === 'discover' || action === 'select') && !connectionConsent) return;
    const revision = ++serial.current;
    setPending(action); setError(null);
    try {
      const value = await bridge.transfer('device_runtime_action', { request: { identity: state.identity, action, targetId, kind,
        connectionConsent, deploymentConsent: false } }, scope());
      if (liveRef.current && revision === serial.current && belongs(value)) setState(value);
    } catch (reason) { if (liveRef.current && revision === serial.current) setError(normalizeError(reason)); }
    finally { if (liveRef.current && revision === serial.current) setPending(null); }
  }
  return <div className="device-runtime-panel">
    <span className="section-icon"><Icon name="device" size={26} /></span>
    <h3>{kind === 'emulator' ? '鸿蒙模拟器运行' : '鸿蒙真机运行'}</h3>
    <p className="muted">独立设备运行方式 · 设备画面通道尚未就绪</p>
    <p className="info-note">{state?.reason ?? '等待当前工程的 HDC 设备运行状态。'}</p>
    <dl className="details-list"><div><dt>HDC</dt><dd>{state?.hdcPresent ? state.hdcPath : '没有检测到合法安装的 HDC'}</dd></div>
      <div><dt>签名与部署</dt><dd>{state?.signatureStatus ?? '尚未核验'}</dd></div><div><dt>运行状态</dt><dd>{pending ? `请求处理中：${pending}` : state?.phase ?? '等待工程会话'}</dd></div></dl>
    <label className="device-consent"><input type="checkbox" checked={connectionConsent} disabled={Boolean(pending)}
      onChange={(event) => setConnectionConsent(event.target.checked)} />允许使用当前工程的 HDC 连接并列出设备</label>
    <div className="panel-actions"><button disabled={!state?.hdcPresent || !state.identity || !connectionConsent || Boolean(pending)}
      onClick={() => void act('discover')}>发现设备</button><button disabled={!state?.selectedTargetId || Boolean(pending)} onClick={() => void act('disconnect')}>断开当前设备</button>
      {pending && <button onClick={() => void act('cancel')}>取消操作</button>}</div>
    {state?.targets.length ? <ul className="device-targets">{state.targets.map((target) => <li key={target.id}><button
      disabled={!connectionConsent || Boolean(pending)} aria-pressed={state.selectedTargetId === target.id}
      title={`${target.connectionState} · ${target.reason}`} onClick={() => void act('select', target.id)}>
      <strong>{target.label}</strong><code>{target.id}</code><span>{target.kind === 'unknown' ? `类型待核验 · 按当前选择申请为${kind === 'emulator' ? '模拟器' : '真机'}` : target.kind === 'emulator' ? '模拟器' : '真机'} · {target.connectionState}</span>
    </button></li>)}</ul> : <p className="muted">没有已发现的设备；不会根据设备名称自动判定真机或模拟器。</p>}
    <button className="primary-button" disabled title={state?.reason ?? '需要可安装产物、签名、Ability 与真实设备画面接入'}>安装并启动应用 · 暂不可用</button>
    <p className="muted">连接授权不包含安装授权。签名产物、Ability、设备前台核验与画面传输完成接入后，安装还需单独确认。</p>
    {error && <output className="native-control-error" role="alert">{describeError(error)}</output>}
  </div>;
}
