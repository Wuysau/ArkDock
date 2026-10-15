import { useEffect, useState, useSyncExternalStore } from 'react';
import type { AppSnapshot } from './ipc/model.ts';
import { describeError } from './ipc/errors.ts';
import type { FrameStore } from './frame-store.ts';
import { deriveUiState } from './view-state.ts';
import type { CapabilityStatus } from './view-state.ts';
import type { NativeControls } from './preview/useNativeControls.ts';
import { controlAvailable, displayedIdentity, identityEqual } from './preview/useNativeControls.ts';

const labels = { click: '点击', swipe: '拖拽 / 滑动', text: '字符提交 / 已核按键', rotate: '设备方向',
  page: '页面切换', device: '设备切换', appTheme: '鸿蒙应用主题', emulator: '模拟器', physicalDevice: '真机' };
const statusLabels: Record<CapabilityStatus, string> = { available: '可用', pending: '待连接', experimental: '实验性',
  unsupported: '不支持', unavailable: '未接入', blocked: '不可用' };

export function PreviewCapabilities({ snapshot, store, busy, refresh, controls }: {
  snapshot: AppSnapshot | null; store: FrameStore; busy: boolean; refresh: () => void; controls?: NativeControls;
}) {
  const frame = useSyncExternalStore(store.subscribeDisplayed, store.getDisplayed, store.getDisplayed);
  const { capabilities } = deriveUiState(snapshot, frame, controls?.state);
  const state = controls?.state;
  const width = state?.width, height = state?.height, dpi = state?.dpi;
  const ime = state?.capabilities.find((item) => item.name === 'ime');
  const [resolution, setResolution] = useState({ width: 360, height: 640, dpi: 160 });
  useEffect(() => { if (width !== undefined && height !== undefined && dpi !== undefined) setResolution({ width, height, dpi }); }, [width, height, dpi]);
  const configured = Boolean(state?.experimentalEnabled && state.inputReady && identityEqual(state.identity, displayedIdentity(snapshot, frame)));
  const ready = (name: string) => Boolean(controls && configured && !busy && !controls.pending && controlAvailable(state ?? null, name));
  const validResolution = Number.isInteger(resolution.width) && resolution.width >= 50 && resolution.width <= 3000
    && Number.isInteger(resolution.height) && resolution.height >= 50 && resolution.height <= 3000 && resolution.width * resolution.height <= 4 * 1024 * 1024
    && Number.isInteger(resolution.dpi) && resolution.dpi >= 120 && resolution.dpi <= 640;
  return <>
    <p className="muted">能力来自当前后端。画面缩放只影响查看尺寸。</p>
    {controls && <div className="native-controls-settings">
      <label className="setting-toggle" aria-label="启用实验性原生控制"><span><strong>启用实验性原生控制</strong><small>开启后可向设备发送输入、方向与主题等原生控制命令。</small></span>
        <input type="checkbox" checked={Boolean(state?.experimentalEnabled)} disabled={!state || busy || Boolean(controls.pending)}
          onChange={(event) => void controls.enable(event.target.checked)} /></label>
      <p className="info-note">{state?.reason ?? '等待原生控制状态'}</p>
      <div className="native-config-fields"><label>原生设备方向<select value={state?.orientation ?? 'portrait'} disabled={!ready('orientation')}
        onChange={(event) => void controls.submit({ action: 'setOrientation', orientation: event.target.value === 'landscape' ? 'landscape' : 'portrait' })}><option value="portrait">竖屏</option><option value="landscape">横屏</option></select></label>
        <label>鸿蒙应用主题<select value={state?.colorMode ?? 'light'} disabled={!ready('colorMode')}
          onChange={(event) => void controls.submit({ action: 'setColorMode', colorMode: event.target.value === 'dark' ? 'dark' : 'light' })}><option value="light">Light</option><option value="dark">Dark</option></select></label></div>
      <p className="muted">应用主题只影响鸿蒙应用；ArkDock 界面主题请在偏好设置中调整。</p>
      <label className="native-profile-select">设备 Profile<select value={state?.currentProfileId ?? ''} disabled={!ready('profile')}
        onChange={(event) => void controls.submit({ action: 'switchProfile', profileId: event.target.value })}>
        {!state && <option value="">等待 Profile</option>}{state?.profiles.map((profile) => <option key={profile.id} value={profile.id}
          disabled={profile.status !== 'available' && !(state?.experimentalEnabled && profile.status === 'experimental')}>{profile.label} · {profile.width}×{profile.height} · {profile.dpi} DPI{profile.requiresRestart ? ' · 重启' : ''}{profile.status === 'blocked' ? ' · 不可用' : ''}</option>)}</select></label>
      {state?.profiles.map((profile) => <p className="muted profile-reason" key={profile.id}>{profile.label} · {profile.deviceType} · {profile.shape} · {profile.projectModel} · {profile.requiresRestart ? '受控重启' : '动态配置'}：{profile.reason}</p>)}
      <details className="native-resolution"><summary>原生尺寸与 DPI</summary><div className="native-resolution-fields">{(['width', 'height', 'dpi'] as const).map((field) => <label key={field}>{field === 'width' ? '宽' : field === 'height' ? '高' : 'DPI'}<input type="number" value={resolution[field]}
        min={field === 'dpi' ? 120 : 50} max={field === 'dpi' ? 640 : 3000} disabled={!ready('resolution')}
        onChange={(event) => setResolution((current) => ({ ...current, [field]: Number(event.target.value) }))} /></label>)}</div>
        <button disabled={!ready('resolution') || !validResolution} onClick={() => void controls.submit({ action: 'setResolution', ...resolution })}>应用原生尺寸</button></details>
      <p className="muted">文字输入支持向聚焦的应用提交 Unicode 字符与常用按键。未接入：完整 IME 预编辑、候选与原生光标编辑。</p>
      {(controls.pending || state?.operations.some((operation) => ['queued', 'awaitingFrame'].includes(operation.phase))) && <div className="panel-actions"><output>{controls.pending ? `请求处理中：${controls.pending}` : '等待原生画面与配置确认'}</output><button onClick={() => void controls.cancel()}>取消原生操作</button></div>}
      {controls.error && <output className="native-control-error" role="alert">{describeError(controls.error)}</output>}
      {state?.operations.length ? <details className="native-control-receipts"><summary>最近原生控制回执</summary>{state.operations.slice(-8).reverse().map((operation) => <article key={operation.operationId}>
        <strong>{operation.action} · {operation.phase}</strong><p>{operation.reason}</p><small>命令接受：{operation.commandAccepted ? '是' : '否'} · 有效帧：{operation.frameObserved ? '已观察' : '未观察'} · 像素改变：{operation.pixelsChanged ? '已确认' : '未确认'} · 配置确认：{operation.configurationConfirmed ? '是' : '否'}</small>
      </article>)}</details> : null}
    </div>}
    <div className="capability-list">
      {(Object.keys(labels) as (keyof typeof labels)[]).map((name) => <div key={name}>
        <strong>{labels[name]}</strong>
        <span className={`badge ${capabilities[name].status === 'available' ? '' : 'invalid'}`}>{statusLabels[capabilities[name].status]}</span>
        <p>{capabilities[name].reason}</p>
      </div>)}
      <div><strong>完整 IME 事务</strong><span className="badge invalid">{statusLabels[ime?.status ?? 'unavailable']}</span><p>{ime?.reason ?? '未接入：暂不支持完整 IME 预编辑、候选与光标事务。'}</p></div>
    </div>
    <button className="primary-button" disabled={capabilities.refresh.status !== 'available' || busy}
      title={capabilities.refresh.reason} onClick={refresh}>手动刷新</button>
  </>;
}
