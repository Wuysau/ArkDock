import { useSyncExternalStore } from 'react';
import type { AppSnapshot } from '../ipc/model.ts';
import type { FrameStore } from '../frame-store.ts';
import type { NativeControls } from './useNativeControls.ts';
import { controlAvailable, displayedIdentity, identityEqual } from './useNativeControls.ts';
import { Icon } from '../Icon.tsx';

function ready(snapshot: AppSnapshot | null, store: FrameStore, controls: NativeControls, name: string, busy: boolean) {
  return !busy && !controls.pending && controls.state?.inputReady
    && identityEqual(controls.state.identity, displayedIdentity(snapshot, store.getDisplayed())) && controlAvailable(controls.state, name);
}
export function PreviewPageSelect({ snapshot, store, controls, busy }: { snapshot: AppSnapshot | null; store: FrameStore; controls: NativeControls; busy: boolean }) {
  useSyncExternalStore(store.subscribeDisplayed, store.getDisplayed, store.getDisplayed);
  const state = controls.state;
  const pages = state?.pages ?? [];
  const currentPage = state?.currentPage ?? '';
  const routeConfirmed = Boolean(currentPage && pages.includes(currentPage));
  const available = pages.length > 1 && routeConfirmed && ready(snapshot, store, controls, 'page', busy);
  const reason = pages.length === 1 ? '当前工程只有一个编译页面，无需切换；不会重新替换入口页面'
    : !routeConfirmed ? '当前原生路由尚未确认，暂不发送页面替换命令'
      : state?.capabilities.find((item) => item.name === 'page')?.reason ?? '等待原生页面列表';
  return <label><span className="sr-only">原生应用页面</span><select aria-label="原生应用页面" value={routeConfirmed ? currentPage : ''}
    disabled={!available} title={reason} onChange={(event) => {
      const page = event.target.value;
      if (!available || page === currentPage || !pages.includes(page)) return;
      void controls.submit({ action: 'navigatePage', page });
    }}>
    <option value="" disabled>{pages.length === 1 ? `仅一个页面：${pages[0]} · 当前路由待确认` : '当前路由待确认'}</option>
    {pages.map((page) => <option key={page} value={page} disabled={page === currentPage}>{page}</option>)}
  </select></label>;
}
export function PreviewControlToolbar({ snapshot, store, controls, busy }: { snapshot: AppSnapshot | null; store: FrameStore; controls: NativeControls; busy: boolean }) {
  useSyncExternalStore(store.subscribeDisplayed, store.getDisplayed, store.getDisplayed);
  const state = controls.state;
  const selectedProfile = state?.profiles.find((profile) => profile.id === state.currentProfileId);
  const profileReady = ready(snapshot, store, controls, 'profile', busy);
  return <div className="native-config-toolbar">
    <label><span className="sr-only">预览设备配置</span><select aria-label="预览设备配置"
      value={state?.currentProfileId ?? ''} disabled={!profileReady}
      title={selectedProfile ? `${selectedProfile.width}×${selectedProfile.height} · ${selectedProfile.dpi} DPI · ${selectedProfile.shape}；切换后重启预览` : '等待设备配置'}
      onChange={(event) => {
        const profile = state?.profiles.find((candidate) => candidate.id === event.target.value);
        if (!profileReady || !profile || profile.id === state?.currentProfileId || profile.status !== 'available') return;
        void controls.submit({ action: 'switchProfile', profileId: profile.id });
      }}>
      {!state && <option value="">设备</option>}
      {state?.profiles.map((profile) => <option key={profile.id} value={profile.id} disabled={profile.status !== 'available'} title={profile.reason}>
        {profile.label} · {profile.width}×{profile.height}
      </option>)}
    </select></label>
    <button className="icon-button" aria-label={`切换设备方向，当前${state ? state.orientation === 'landscape' ? '横屏' : '竖屏' : '待确认'}`}
      disabled={!ready(snapshot, store, controls, 'orientation', busy)} title={state?.capabilities.find((item) => item.name === 'orientation')?.reason ?? '等待原生方向能力'}
      onClick={() => void controls.submit({ action: 'setOrientation', orientation: state?.orientation === 'landscape' ? 'portrait' : 'landscape' })}>
      <span className={`orientation-symbol ${state?.orientation === 'landscape' ? 'landscape' : ''}`} aria-hidden="true" />
    </button>
    <button className="icon-button" aria-label={`切换鸿蒙应用主题，当前${state ? state.colorMode === 'dark' ? '深色' : '浅色' : '待确认'}`}
      disabled={!ready(snapshot, store, controls, 'colorMode', busy)} title={`应用主题只影响鸿蒙应用；${state?.capabilities.find((item) => item.name === 'colorMode')?.reason ?? '等待原生应用主题能力'}`}
      onClick={() => void controls.submit({ action: 'setColorMode', colorMode: state?.colorMode === 'dark' ? 'light' : 'dark' })}>
      <Icon name={state?.colorMode === 'dark' ? 'moon' : 'sun'} size={16} />
    </button>
  </div>;
}
