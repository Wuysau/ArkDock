import type { AppSnapshot, MotionIntensity } from '../ipc/model.ts';
import type { MotionPreference } from '../motion.ts';
import type { CSSProperties } from 'react';
import { useState } from 'react';
import { orbOpacityTransparencyCeiling } from '../orb-preferences.ts';
import { displayPath } from '../path-display.ts';
import { environmentLabel } from './candidates.ts';
import type { Panel } from './navigation.ts';
import { Icon } from '../Icon.tsx';
import { themeOptions } from '../theme.ts';
import type { Theme } from '../theme.ts';

/** Shell-level display and window intents shared by the settings and menu panels. */
export interface ShellControls {
  connected: boolean;
  nativeReady: boolean;
  busy: boolean;
  displayBusy: boolean;
  /** The native tray icon is registered, so a hidden window has a recovery surface. */
  trayAvailable: boolean;
  trayReason: string;
  collapseToOrb: () => void;
  hideToTray: () => void;
}

export interface PairInfo { action: string; enabled: boolean; reason: string; label: string }

export interface MenuIntents {
  open: (panel: Panel) => void;
  rediscover: () => void;
  chooseDirectory: () => void;
  pair: () => void;
  minimize: () => void;
  detach: () => void;
}

export function TrustPanel({ snapshot, busy, trust, browse }: {
  snapshot: AppSnapshot | null; busy: boolean; trust: () => void; browse: () => void;
}) {
  const project = snapshot?.project;
  if (!project) return <p>当前没有待信任工程。</p>;
  return <div className="settings-card"><span className="section-icon"><Icon name="shield" size={27} /></span><h3>允许这个工程执行构建？</h3><p>发现和关联只读取工程信息。信任后才允许工程的构建逻辑运行。</p><code className="path">{displayPath(project.appRoot)}</code><p className="muted">构建配置改变时将再次确认。可随时撤销信任。</p><div className="panel-actions"><button className="primary-button" disabled={busy} onClick={trust}>信任此工程</button><button onClick={browse}>换一个工程</button></div></div>;
}

export function SdkPanel({ snapshot, connected, busy, configure }: {
  snapshot: AppSnapshot | null; connected: boolean; busy: boolean; configure: () => void;
}) {
  return <div className="settings-card"><span className="section-icon"><Icon name="device" size={27} /></span><h3>连接真实原生运行时</h3><p>使用合法安装的 HarmonyOS SDK 与构建工具，无需启动 DevEco GUI。</p><dl className="details-list"><div><dt>SDK 目录</dt><dd className="path">{snapshot?.backend.sdkPath ? displayPath(snapshot.backend.sdkPath) : '未配置'}</dd></div><div><dt>工具链版本</dt><dd>{snapshot?.backend.sdkVersion ?? '尚未检测'}</dd></div></dl><p className="info-note">{snapshot?.backend.reason ?? '桌面桥接未连接，浏览器页面无法探测 SDK。'}</p><button className="primary-button" disabled={!connected || busy} onClick={configure}><Icon name="folder" size={16} />指定 SDK 目录</button><p className="muted">原生基线为 Stage 工程的 Phone 画面。新增控制在预览能力中显式启用，具体表现取决于所连接的 SDK；其他设备 Profile 各自说明未接入原因。</p></div>;
}

export function PathPanel({ snapshot, busy, copy, browse, untrust }: {
  snapshot: AppSnapshot | null; busy: boolean; copy: (text: string, label: string) => void; browse: () => void; untrust: () => void;
}) {
  const project = snapshot?.project;
  if (!project) return <button onClick={browse}>查看工程候选</button>;
  const appRoot = displayPath(project.appRoot);
  return <div className="settings-card"><h3>{project.name}</h3><dl className="details-list"><div><dt>应用根</dt><dd className="path">{appRoot}</dd></div><div><dt>工作区根</dt><dd className="path">{displayPath(project.workspaceRoot)}</dd></div><div><dt>环境</dt><dd>{environmentLabel(project.environment)}</dd></div><div><dt>模块 · 变体</dt><dd>{project.module} · {project.variant}</dd></div><div><dt>执行信任</dt><dd>{snapshot?.trusted ? '已信任当前构建配置' : '未信任 · 只读'}</dd></div></dl><div className="panel-actions"><button onClick={() => copy(appRoot, '工程路径')}>复制完整路径</button><button onClick={browse}>切换工程</button><button disabled={!snapshot?.trusted || busy} onClick={untrust}>撤销工程信任</button></div></div>;
}

/**
 * 拖动只在本地即时预览通透程度，松开、走键或失焦时才写入原生一次。
 * 悬浮球是收起后的唯一恢复入口，因此透明度上限保留可辨认的底。
 */
function OrbOpacitySetting({ opacity, disabled, commit }: {
  opacity: number; disabled: boolean; commit: (opacity: number) => void;
}) {
  const committed = 100 - opacity;
  const [dragged, setDragged] = useState<number | null>(null);
  const transparency = dragged ?? committed;
  const finish = () => {
    if (dragged === null) return;
    commit(100 - dragged);
    setDragged(null);
  };
  return <label className="setting-slider">
    <span className="setting-slider-head">
      <span><strong>透明度</strong><small>拖动调节悬浮球的通透程度；上限 80%，保证它作为恢复入口依然容易找到。</small></span>
      <output className="setting-slider-value" aria-hidden="true">{transparency}%</output>
    </span>
    <span className="setting-slider-control">
      <span className="orb-preview" aria-hidden="true"><span className="orb-specimen" style={{ opacity: (100 - transparency) / 100 }}>
        <span className="mark"><span /><span /></span><span className="orb-specimen-dot" />
      </span></span>
      <input type="range" min={0} max={orbOpacityTransparencyCeiling} step={5} value={transparency}
        aria-label="悬浮球透明度" aria-valuetext={`${transparency}% 透明`} disabled={disabled}
        style={{ '--orb-slider-fill': `${(transparency / orbOpacityTransparencyCeiling) * 100}%` } as CSSProperties}
        onChange={(event) => setDragged(Number(event.target.value))} onPointerUp={finish} onKeyUp={finish} onBlur={finish} />
    </span>
  </label>;
}

export function SettingsPanel({ snapshot, shell, theme, setTheme, motion, setMotion, intensity, setIntensity, chooseOrbAutoHide, orbOpacity, chooseOrbOpacity, changeDisplay, pair, detach, open }: {
  snapshot: AppSnapshot | null; shell: ShellControls;
  theme: Theme; setTheme: (value: Theme) => void;
  motion: MotionPreference; setMotion: (value: MotionPreference) => void;
  intensity: MotionIntensity; setIntensity: (value: MotionIntensity) => void;
  chooseOrbAutoHide: (enabled: boolean) => void;
  orbOpacity: number; chooseOrbOpacity: (opacity: number) => void;
  changeDisplay: (command: 'collapse_to_orb') => void;
  pair: PairInfo; detach: () => void; open: (panel: Panel) => void;
}) {
  const { connected, nativeReady, displayBusy } = shell;
  return <>
    <div className="settings-card"><h3>工具外观</h3><p className="muted">仅改变 ArkDock 外壳，不改变鸿蒙应用主题。</p><div className="theme-options" aria-label="外观主题">{themeOptions.map((option) => {
      const selected = theme === option.id;
      const descId = `theme-desc-${option.id}`;
      return <div key={option.id} className={`theme-card ${selected ? 'selected' : ''}`} data-theme-id={option.id}>
        <button type="button" className={`theme-card-button ${selected ? 'selected' : ''}`} aria-pressed={selected} aria-describedby={descId} onClick={() => setTheme(option.id)}>
          <span className="theme-swatch" data-swatch={option.id} aria-hidden="true">
            <span className="swatch-canvas" style={{ background: option.swatches[0] }} />
            <span className="swatch-surface" style={{ background: option.swatches[1] }} />
            <span className="swatch-accent" style={{ background: option.swatches[2] }} />
          </span>
          <span className="theme-card-heading"><Icon name={option.icon} size={15} /><strong>{option.label}</strong><span className="theme-card-check" aria-hidden="true" /></span>
        </button>
        <small id={descId} className="theme-card-description">{option.description}</small>
      </div>;
    })}</div></div>
    <div className="settings-card"><h3>动画偏好</h3><p className="muted">系统减少动画优先生效；关闭或轻量时不播放贴边涟漪与悬浮球装饰。</p><div className="motion-options" aria-label="减少动画偏好">{(['system', 'reduce'] as const).map((option) => <button key={option} aria-pressed={motion === option} className={motion === option ? 'selected' : ''} onClick={() => setMotion(option)}>{option === 'system' ? '跟随系统动画' : '减少动画'}</button>)}</div><div className="intensity-options" aria-label="动效强度">{(['off', 'light', 'standard', 'enhanced'] as const).map((option) => <button key={option} aria-pressed={intensity === option} className={intensity === option ? 'selected' : ''} onClick={() => setIntensity(option)}>{({ off: '关闭', light: '轻量', standard: '标准', enhanced: '增强' })[option]}</button>)}</div><output className="motion-status">{!connected ? '桌面桥接未连接，原生动效偏好尚未同步。' : snapshot?.motion.reduced ? '原生窗口已减少动画。' : snapshot?.motion.intensity === 'off' ? '原生窗口动效已关闭。' : snapshot?.motion.intensity === 'light' ? '原生窗口使用轻量动效。' : snapshot?.motion.intensity === 'enhanced' ? '原生窗口使用增强动效。' : '原生窗口使用标准动效。'}</output></div>
    <div className="settings-card"><h3>悬浮球</h3><label className="setting-toggle"><span><strong>贴边自动隐藏</strong><small>悬浮球闲置时沿屏幕边缘收起，靠近后恢复显示。</small></span><input type="checkbox" role="switch" aria-checked={snapshot?.display.orbAutoHide ?? false} aria-label="悬浮球贴边自动隐藏" checked={snapshot?.display.orbAutoHide ?? false} disabled={!connected || !nativeReady || displayBusy} onChange={(event) => chooseOrbAutoHide(event.target.checked)} /></label><OrbOpacitySetting opacity={orbOpacity} disabled={!connected || !nativeReady} commit={chooseOrbOpacity} /><button disabled={!connected || !nativeReady || displayBusy || snapshot?.display.mode !== 'expanded'} onClick={() => changeDisplay('collapse_to_orb')}><span className="orb-symbol" aria-hidden="true" />收起为悬浮球</button></div>
    <div className="settings-card"><h3>窗口与工程</h3><p className="pair-mode-description">{pair.label}{snapshot?.pairLayout?.reason && ` · ${snapshot.pairLayout.reason}`}</p><p className="muted">两个独立窗口全贴合，尺寸联动；共同最大化时 ArkDock 保持右侧。解除绑定会停止自有预览资源。</p><button disabled={!snapshot?.target || shell.busy} onClick={detach}>解除当前绑定</button>{snapshot?.project && <button onClick={() => open('path')}>管理工程信任</button>}</div>
  </>;
}

export function MenuPanel({ snapshot, shell, pair, intents }: {
  snapshot: AppSnapshot | null; shell: ShellControls; pair: PairInfo; intents: MenuIntents;
}) {
  const { connected, nativeReady, busy, displayBusy, trayAvailable, trayReason, collapseToOrb, hideToTray } = shell;
  const orbDisabled = !connected || !nativeReady || displayBusy || snapshot?.display.mode !== 'expanded';
  return <div className="menu-list">
    <button disabled={orbDisabled} onClick={collapseToOrb}><span className="orb-symbol" aria-hidden="true" />收起为悬浮球</button>
    <button disabled={!connected || !nativeReady || displayBusy || !trayAvailable} title={trayAvailable ? '隐藏窗口，由系统托盘恢复' : trayReason} onClick={hideToTray}><Icon name="monitor" />隐藏到系统托盘</button>
    {!trayAvailable && <p className="menu-note">{trayReason}</p>}
    <button onClick={() => intents.open('projects')}><Icon name="folder" />选择当前工程<Icon name="chevron" size={16} /></button>
    <button disabled={!snapshot?.target || busy} onClick={intents.rediscover}><Icon name="refresh" />重新识别</button>
    <button disabled={!connected || busy} onClick={intents.chooseDirectory}><Icon name="folder" />手动选择目录</button>
    <button onClick={() => intents.open('controls')}><Icon name="preview" />预览能力</button>
    <button onClick={() => intents.open('sdk')}><Icon name="device" />设备与原生工具链</button>
    <button onClick={() => intents.open('diagnostics')}><Icon name="log" />运行日志</button>
    <button onClick={() => intents.open('settings')}><Icon name="settings" />偏好设置</button>
    <button onClick={() => intents.open('help')}><Icon name="link" />使用帮助</button>
    <button disabled={!connected || busy || !pair.enabled} title={pair.reason} onClick={intents.pair}><Icon name="maximize" />{pair.action}</button>
    <button disabled={!connected || busy} onClick={intents.minimize}><Icon name="monitor" />最小化窗口</button>
    <button className="danger-button" disabled={!snapshot?.target || busy} onClick={intents.detach}><Icon name="close" />解除当前绑定</button>
  </div>;
}

export function HelpPanel() {
  return <div className="settings-card"><h3>从正在开发的窗口开始</h3><ol className="help-steps"><li>按住顶部标识或拖动区，靠近开发窗口右侧。</li><li>收到目标提示后松开；独立窗口由原生层贴边。</li><li>可靠工程直接关联；线索不足时确认候选，未发现时选择目录。</li><li>确认工程信任，再检查工具链，等待真实原生首帧。</li></ol><p className="muted">可随时重新识别、切换工程或解除绑定。只读识别目前不能中途取消；解除绑定使会话失效。右上角关闭按钮会隐藏到系统托盘，需要完全退出时使用托盘图标的「退出」。</p></div>;
}
