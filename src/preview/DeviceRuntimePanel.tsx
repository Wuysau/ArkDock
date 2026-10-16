import { useEffect, useRef, useState } from 'react';
import type { PointerEvent as ReactPointerEvent } from 'react';
import type { AppSnapshot, DeviceFrameMetadata, DeviceInputKind, DeviceInputRequest, DeviceRuntimeAction, DeviceRuntimeKind, DeviceRuntimeRequest, DeviceRuntimeState, RequestScope } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';
import { describeError, normalizeError } from '../ipc/errors.ts';
import type { AppError } from '../ipc/errors.ts';
import { Icon } from '../Icon.tsx';
import { DeviceFrameController, deviceActionMayCapture, deviceCaptureCleanupScope, deviceFrameInputReady, deviceGestureFrameMatches, devicePoint } from './DeviceFrameController.ts';
import './DeviceRuntimePanel.css';

const phaseLabels: Record<string, string> = { idle: '等待选择', discovering: '正在发现', selected: '目标已选择', building: '正在构建',
  artifactReady: '签名产物已就绪', installed: '已安装', running: '应用运行中', stopped: '应用已停止', failed: '操作失败', disconnected: '已断开' };
const inputLabels = { click: '点击', doubleClick: '双击', longClick: '长按', swipe: '滑动', drag: '拖动' };
type PointerMode = keyof typeof inputLabels;
type ActionOptions = { targetId?: string; deploymentConsent?: boolean; input?: DeviceInputRequest; emulatorName?: string };
type Gesture = { pointerId: number; x: number; y: number; frame: DeviceFrameMetadata; mode: PointerMode };

function inputRequest(kind: DeviceInputKind, frame: DeviceFrameMetadata, fields: Partial<DeviceInputRequest> = {}): DeviceInputRequest {
  return { kind, frameSequence: frame.sequence, x: null, y: null, endX: null, endY: null, velocity: null, text: null, keyCode: null, ...fields };
}

/** Devices have their own run, capture consent and pixels; no Previewer session
 * or successful preview build is required to use this panel. */
export function DeviceRuntimePanel({ snapshot, bridge, kind }: { snapshot: AppSnapshot | null; bridge: UiBridge; kind: Exclude<DeviceRuntimeKind, 'unknown'> }) {
  const [state, setState] = useState<DeviceRuntimeState | null>(null);
  const [connectionConsent, setConnectionConsent] = useState(false);
  const [captureConsent, setCaptureConsent] = useState(false);
  const [inputConsent, setInputConsent] = useState(false);
  const [deploymentConsent, setDeploymentConsent] = useState(false);
  const [pending, setPending] = useState<DeviceRuntimeAction | null>(null);
  const [error, setError] = useState<AppError | null>(null);
  const [displayed, setDisplayed] = useState<DeviceFrameMetadata | null>(null);
  const [now, setNow] = useState(Date.now());
  const [pointerMode, setPointerMode] = useState<PointerMode>('click');
  const [text, setText] = useState('');
  const [emulatorChoice, setEmulatorChoice] = useState('');
  const canvas = useRef<HTMLCanvasElement>(null);
  const controller = useRef<DeviceFrameController<ImageBitmap> | null>(null);
  const gesture = useRef<Gesture | null>(null);
  const latest = useRef(snapshot); latest.current = snapshot;
  const stateRef = useRef(state); stateRef.current = state;
  const pendingRef = useRef<DeviceRuntimeAction | null>(null);
  const capturePending = useRef(false);
  const lifecycle = useRef(0), requestRevision = useRef(0);
  const wakePoll = useRef<() => void>(() => {});
  const stopCapture = useRef<() => void>(() => {});
  stopCapture.current = () => { void act('streamStop'); };
  const key = JSON.stringify([snapshot?.target?.id, snapshot?.project?.id, snapshot?.trusted, kind]);
  const consentKey = JSON.stringify([state?.identity?.runId, state?.identity?.generation, state?.selectedTargetId, state?.artifactSha256, state?.deploymentToken]);
  const inputScopeKey = JSON.stringify([state?.identity?.runId, state?.identity?.generation, state?.selectedTargetId]);
  const scope = (): RequestScope => ({ targetId: latest.current?.target?.id ?? null, sessionId: latest.current?.sessionId ?? null });

  function accept(value: DeviceRuntimeState, epoch: number): void {
    if (lifecycle.current !== epoch || value.identity?.workspaceId !== latest.current?.project?.id) return;
    const previous = stateRef.current;
    if (previous && previous.identity?.runId === value.identity?.runId && previous.identity?.generation === value.identity?.generation && previous.operationId > value.operationId) return;
    if (!value.busy) capturePending.current = false;
    stateRef.current = value; setState(value); setNow(Date.now());
  }

  useEffect(() => {
    const lifecycleCounter = lifecycle, revisionCounter = requestRevision;
    const epoch = ++lifecycle.current;
    const owner = { targetId: latest.current?.target?.id ?? null, workspaceId: latest.current?.project?.id ?? null };
    requestRevision.current++; pendingRef.current = null; capturePending.current = false; gesture.current = null;
    stateRef.current = null; setState(null); setError(null); setPending(null); setDisplayed(null);
    setConnectionConsent(false); setCaptureConsent(false); setInputConsent(false); setDeploymentConsent(false); setText(''); setEmulatorChoice('');
    let timer: ReturnType<typeof setTimeout> | null = null, polling = false, stopped = false;
    const schedule = (delay: number) => {
      if (stopped || polling || !bridge.transfer || !latest.current?.target || !latest.current.project || !latest.current.trusted) return;
      if (timer !== null) clearTimeout(timer);
      timer = setTimeout(() => { timer = null; void poll(); }, delay);
    };
    async function poll() {
      if (stopped || polling) return;
      if (pendingRef.current) { schedule(100); return; }
      polling = true;
      const revision = requestRevision.current;
      try {
        const value = await bridge.transfer!('device_runtime_state', {}, scope());
        if (!stopped && revision === requestRevision.current) accept(value, epoch);
      } catch (reason) { if (!stopped && revision === requestRevision.current) setError(normalizeError(reason)); }
      finally { polling = false; if (!stopped) { setNow(Date.now()); schedule(stateRef.current?.busy || stateRef.current?.streaming ? 250 : 1000); } }
    }
    wakePoll.current = () => schedule(0);
    schedule(0);
    return () => {
      stopped = true; lifecycleCounter.current++; revisionCounter.current++; wakePoll.current = () => {};
      if (timer !== null) clearTimeout(timer);
      gesture.current = null;
      const current = stateRef.current;
      const cleanupScope = deviceCaptureCleanupScope(owner, { targetId: latest.current?.target?.id ?? null,
        workspaceId: latest.current?.project?.id ?? null, sessionId: latest.current?.sessionId ?? null });
      // Leaving the view revokes its continuous screen capture, not app execution.
      // Previewer session changes are unrelated to this device run. Conversely,
      // a new project must never receive an old run's cleanup request.
      if ((current?.streaming || capturePending.current) && current?.identity && cleanupScope && bridge.transfer) {
        const request: DeviceRuntimeRequest = { identity: current.identity, action: 'streamStop', targetId: current.selectedTargetId, kind,
          connectionConsent: true, deploymentConsent: false, captureConsent: false, inputConsent: false, deploymentToken: null, input: null, emulatorName: null };
        void bridge.transfer('device_runtime_action', { request }, cleanupScope).catch(() => {});
      }
    };
  }, [bridge, key, kind]);

  useEffect(() => { setDeploymentConsent(false); }, [consentKey]);
  useEffect(() => { setCaptureConsent(false); setInputConsent(false); gesture.current = null; }, [inputScopeKey]);
  useEffect(() => { if (emulatorChoice && !state?.emulators.some((emulator) => emulator.name === emulatorChoice)) setEmulatorChoice(''); }, [state?.emulators, emulatorChoice]);

  useEffect(() => {
    const surface = canvas.current;
    const context = surface?.getContext('2d', { alpha: false });
    if (!surface || !context || !bridge.readDeviceFrame) return;
    const frames = new DeviceFrameController<ImageBitmap>({
      read: bridge.readDeviceFrame,
      decode: (frame) => createImageBitmap(new Blob([frame.png], { type: 'image/png' })),
      draw: (bitmap, metadata) => {
        surface.width = metadata.width; surface.height = metadata.height;
        context.drawImage(bitmap, 0, 0);
        surface.dataset.frameSequence = String(metadata.sequence); surface.dataset.runId = metadata.runId;
        setDisplayed(metadata); setNow(Date.now());
      },
      clear: () => { surface.width = 1; surface.height = 1; delete surface.dataset.frameSequence; delete surface.dataset.runId; setDisplayed(null); },
      error: (reason) => setError(normalizeError(reason)),
    });
    controller.current = frames;
    const visibility = () => frames.setVisible(!document.hidden);
    visibility(); document.addEventListener('visibilitychange', visibility);
    return () => { document.removeEventListener('visibilitychange', visibility); frames.dispose(); if (controller.current === frames) controller.current = null; };
  }, [bridge, key]);
  useEffect(() => { controller.current?.update(state, scope(), captureConsent); }, [state, captureConsent, bridge, key]);
  useEffect(() => { if (state?.streaming && !captureConsent && pendingRef.current !== 'streamStop') stopCapture.current(); }, [state?.streaming, captureConsent]);

  async function act(action: DeviceRuntimeAction, options: ActionOptions = {}): Promise<void> {
    const current = stateRef.current;
    const interrupts = ['cancel', 'disconnect', 'stop', 'streamStop'].includes(action);
    if (!bridge.transfer || !current?.identity || (pendingRef.current || current.busy) && !interrupts) return;
    if (!connectionConsent && !['cancel', 'disconnect', 'streamStop'].includes(action)) return;
    const epoch = lifecycle.current, revision = ++requestRevision.current;
    if (deviceActionMayCapture(action, captureConsent)) capturePending.current = true;
    pendingRef.current = action; setPending(action); setError(null);
    const request: DeviceRuntimeRequest = { identity: current.identity, action, targetId: action === 'discover' ? null : options.targetId ?? current.selectedTargetId, kind,
      connectionConsent: connectionConsent || action === 'streamStop', deploymentConsent: options.deploymentConsent ?? false,
      captureConsent, inputConsent, deploymentToken: action === 'deploy' ? current.deploymentToken : null,
      input: options.input ?? null, emulatorName: options.emulatorName ?? null };
    try {
      const value = await bridge.transfer('device_runtime_action', { request }, scope());
      if (revision === requestRevision.current) accept(value, epoch);
    } catch (reason) { if (epoch === lifecycle.current && revision === requestRevision.current) { capturePending.current = false; setError(normalizeError(reason)); } }
    finally {
      if (epoch === lifecycle.current && revision === requestRevision.current) { pendingRef.current = null; setPending(null); wakePoll.current(); }
    }
  }

  const busy = Boolean(pending || state?.busy);
  const selected = Boolean(state?.selectedTargetId);
  const connected = Boolean(connectionConsent && state?.hdcPresent && state.identity);
  const application = Boolean(selected && state?.bundleName);
  const canInput = Boolean(connected && captureConsent && inputConsent && !busy && deviceFrameInputReady(state, displayed, now));
  const selectedEmulator = state?.emulators.find((emulator) => emulator.name === emulatorChoice);

  function sendInput(input: DeviceInputRequest): void {
    if (!canInput || !deviceFrameInputReady(stateRef.current, displayed, Date.now())) { setError(normalizeError('设备画面已过期或前台应用未确认，请先刷新画面。')); return; }
    void act('input', { input });
  }
  function pointerDown(event: ReactPointerEvent<HTMLCanvasElement>): void {
    if (!canInput || !displayed || event.button !== 0 || gesture.current) return;
    const point = devicePoint(event.clientX, event.clientY, event.currentTarget.getBoundingClientRect(), displayed);
    if (!point) return;
    event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId);
    gesture.current = { pointerId: event.pointerId, ...point, frame: displayed, mode: pointerMode };
  }
  function pointerUp(event: ReactPointerEvent<HTMLCanvasElement>): void {
    const active = gesture.current;
    if (!active || active.pointerId !== event.pointerId) return;
    gesture.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    if (!displayed || !deviceGestureFrameMatches(active.frame, displayed)) return;
    const point = devicePoint(event.clientX, event.clientY, event.currentTarget.getBoundingClientRect(), displayed);
    if (!point) return;
    const distance = Math.hypot(point.x - active.x, point.y - active.y);
    if (active.mode === 'swipe' || active.mode === 'drag') {
      if (distance < 3) return;
      sendInput(inputRequest(active.mode, displayed, { x: active.x, y: active.y, endX: point.x, endY: point.y, velocity: 600 }));
    } else if (distance < 8) sendInput(inputRequest(active.mode, displayed, { x: point.x, y: point.y }));
  }

  return <section className="device-runtime-panel device-run-workspace" aria-label={kind === 'emulator' ? '模拟器运行' : '真机运行'}>
    <header className="device-run-heading"><span className="section-icon"><Icon name="device" size={23} /></span>
      <div><h3>{kind === 'emulator' ? '鸿蒙模拟器' : '鸿蒙真机'}</h3><p className="muted">安装应用 · 查看设备画面 · 交互调试</p></div>
      <span className={`device-run-phase${state?.phase === 'failed' ? ' is-error' : ''}`}>{busy ? '处理中' : phaseLabels[state?.phase ?? 'idle'] ?? state?.phase}</span>
    </header>
    <output className="info-note">{state?.reason ?? (snapshot?.trusted ? '正在读取设备运行状态…' : '请先选择并信任当前工程。')}</output>
    <section className="device-run-step" aria-label="运行目标"><h4><span>01</span>运行目标</h4>
      <label className="device-consent"><input type="checkbox" checked={connectionConsent} onChange={(event) => {
        const checked = event.target.checked; setConnectionConsent(checked);
        if (!checked) { setCaptureConsent(false); setInputConsent(false); setDeploymentConsent(false); void act('disconnect'); }
      }} />允许 HDC 连接与设备发现</label>
      <div className="panel-actions device-run-actions"><button disabled={!connected || busy} onClick={() => void act('discover')}>发现设备</button>
        <button disabled={!state?.identity || !selected && !busy} onClick={() => void act('disconnect')}>断开</button>
        {busy && <button onClick={() => void act('cancel')}>取消操作</button>}</div>
      {kind === 'emulator' && <div className="device-emulator-manager"><div className="device-run-actions"><button disabled={!connected || busy} onClick={() => void act('emulatorList')}>读取本地模拟器</button></div>
        <label>模拟器实例<select value={emulatorChoice} disabled={busy} onChange={(event) => setEmulatorChoice(event.target.value)}>
          <option value="">选择已有实例</option>{state?.emulators.map((emulator) => <option key={emulator.name} value={emulator.name}>{emulator.name}{emulator.running ? ' · 运行中' : ''}</option>)}</select></label>
        {selectedEmulator && <p className="muted">{selectedEmulator.reason}</p>}
        <div className="device-run-actions"><button disabled={!connected || !selectedEmulator || selectedEmulator.running || busy} onClick={() => void act('emulatorStart', { emulatorName: emulatorChoice })}>启动模拟器</button>
          <button disabled={!connected || !selectedEmulator?.owned || busy} onClick={() => void act('emulatorStop', { emulatorName: emulatorChoice })}>停止自有实例</button></div>
      </div>}
      {state?.targets.length ? <ul className="device-targets">{state.targets.map((target) => <li key={target.id}><button disabled={!connected || busy || target.connectionState !== 'Connected'}
        aria-pressed={state.selectedTargetId === target.id} title={target.reason} onClick={() => void act('select', { targetId: target.id })}>
        <strong>{target.label}</strong><code>{target.id}</code><span>{target.connectionState}{state.selectedTargetId === target.id ? ' · 当前运行目标' : ''}</span>
      </button></li>)}</ul> : <p className="muted">尚无已发现目标。连接设备后点击“发现设备”。</p>}
      <details className="device-tool-details"><summary>连接工具</summary><code>{state?.hdcPath ?? '当前 SDK 未找到 HDC'}</code>
        {state?.toolSha256 && <p><span>SHA256</span><code>{state.toolSha256}</code></p>}</details>
    </section>
    <section className="device-run-step" aria-label="构建与部署"><h4><span>02</span>构建与部署</h4>
      <div className="device-run-actions"><button disabled={!connected || !selected || busy} onClick={() => void act('build')}>构建并验证 HAP</button><span className="muted">签名：{state?.signatureStatus ?? '待验证'}</span></div>
      {state?.bundleName && <dl className="device-artifact-details"><div><dt>应用</dt><dd><code>{state.bundleName}</code></dd></div>
        <div><dt>启动入口</dt><dd>{state.moduleName} / {state.abilityName}</dd></div><div><dt>安装目标</dt><dd><code>{state.selectedTargetId}</code></dd></div>
        {state.artifactSha256 && <div><dt>HAP SHA256</dt><dd><code>{state.artifactSha256}</code></dd></div>}</dl>}
      {state?.deploymentReady && state.deploymentToken && <label className="device-consent device-deploy-confirm"><input type="checkbox" checked={deploymentConsent} disabled={busy}
        onChange={(event) => setDeploymentConsent(event.target.checked)} />确认将上述签名产物安装到此设备</label>}
      <div className="device-run-actions"><button className="primary-button" disabled={!connected || busy || !state?.deploymentReady || !state.deploymentToken || !deploymentConsent}
        onClick={() => { setDeploymentConsent(false); void act('deploy', { deploymentConsent: true }); }}>安装应用</button>
        <button disabled={!connected || busy || !application || !['installed', 'running', 'stopped', 'failed'].includes(state?.phase ?? '')} onClick={() => void act('start')}>启动</button>
        <button disabled={!connected || !application || !['running', 'failed'].includes(state?.phase ?? '')} onClick={() => void act('stop')}>停止应用</button></div>
    </section>
    <section className="device-run-step" aria-label="设备画面"><h4><span>03</span>设备画面</h4>
      <label className="device-consent"><input type="checkbox" checked={captureConsent} disabled={!selected} onChange={(event) => {
        const checked = event.target.checked; setCaptureConsent(checked);
        if (!checked) { setInputConsent(false); if (state?.streaming || capturePending.current) void act('streamStop'); }
      }} />允许获取所选设备的整个屏幕</label>
      <div className="device-run-actions"><button disabled={!connected || !application || !captureConsent || busy || !bridge.readDeviceFrame} onClick={() => void act('capture')}>刷新画面</button>
        <button disabled={!connected || !application || !captureConsent || busy && !state?.streaming || !bridge.readDeviceFrame}
          onClick={() => void act(state?.streaming ? 'streamStop' : 'streamStart')}>{state?.streaming ? '停止连续刷新' : '连续刷新'}</button></div>
      <div className={`device-pixel-surface${canInput ? ' is-interactive' : ''}`}>
        {/* Canvas exposes the bitmap as an image; pointer control has explicit consent below. */}
        {/* oxlint-disable-next-line jsx-a11y/prefer-tag-over-role */}
        <canvas ref={canvas} hidden={!displayed} role="img" aria-label={displayed ? `设备 ${displayed.targetId} 的实际画面，第 ${displayed.sequence} 帧` : '等待设备画面'}
          onPointerDown={pointerDown} onPointerUp={pointerUp} onPointerCancel={() => { gesture.current = null; }} onLostPointerCapture={() => { gesture.current = null; }} />
        {!displayed && <p className="device-pixel-empty">{captureConsent ? '等待实际设备截图' : '允许屏幕采集后显示设备画面'}</p>}
      </div>
      <p className="device-frame-caption">{displayed ? `${displayed.width} × ${displayed.height} · 第 ${displayed.sequence} 帧 · ${Math.max(0, Math.floor((now - displayed.receivedAt) / 1000))} 秒前` : 'PNG 截图刷新 · 速率取决于设备和连接'}
        {displayed && <span>{displayed.foregroundVerified && state?.foregroundVerified ? '目标应用前台已确认' : '未确认目标应用前台，输入已锁定'}</span>}</p>
    </section>
    <section className="device-run-step" aria-label="设备输入"><h4><span>04</span>交互</h4>
      <label className="device-consent"><input type="checkbox" checked={inputConsent} disabled={!captureConsent || !application} onChange={(event) => setInputConsent(event.target.checked)} />允许向所选设备发送触控和键盘输入</label>
      <p className="muted">输入仅在目标应用前台和新鲜画面确认后启用；操作前请留意设备上的系统弹窗。</p>
      <label className="device-input-mode">触控方式<select value={pointerMode} disabled={!inputConsent || busy} onChange={(event) => { gesture.current = null; setPointerMode(event.target.value as PointerMode); }}>
        {Object.entries(inputLabels).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
      <div className="device-run-actions device-keyboard">{([['返回', 2], ['Tab', 2049], ['Enter', 2054], ['退格', 2055]] as const).map(([label, keyCode]) =>
        <button key={keyCode} disabled={!canInput} onClick={() => displayed && sendInput(inputRequest('key', displayed, { keyCode }))}>{label}</button>)}</div>
      <form className="device-text-entry" onSubmit={(event) => { event.preventDefault(); if (displayed && text) sendInput(inputRequest('text', displayed, { text })); }}>
        <label>发送到设备已聚焦的文本框<input type="text" value={text} maxLength={512} disabled={!inputConsent || busy} placeholder="输入文字" onChange={(event) => setText(event.target.value)} /></label>
        <button type="submit" disabled={!canInput || !text || new TextEncoder().encode(text).length > 512}>发送文本</button>
      </form>
      {inputConsent && !canInput && <p className="muted">等待空闲设备、新鲜画面和目标应用前台确认。</p>}
    </section>
    {error && <output className="native-control-error" role="alert">{describeError(error)}</output>}
  </section>;
}
