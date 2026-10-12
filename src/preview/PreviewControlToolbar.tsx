import { useSyncExternalStore } from 'react';
import type { AppSnapshot } from '../ipc/model.ts';
import type { FrameStore } from '../frame-store.ts';
import type { NativeControls } from './useNativeControls.ts';
import { controlAvailable, displayedIdentity, identityEqual } from './useNativeControls.ts';
import { Icon } from '../Icon.tsx';

function ready(snapshot: AppSnapshot | null, store: FrameStore, controls: NativeControls, name: string, busy: boolean) {
  return !busy && !controls.pending && controls.state?.experimentalEnabled && controls.state.inputReady
    && identityEqual(controls.state.identity, displayedIdentity(snapshot, store.getDisplayed())) && controlAvailable(controls.state, name);
}
export function PreviewPageSelect({ snapshot, store, controls, busy }: { snapshot: AppSnapshot | null; store: FrameStore; controls: NativeControls; busy: boolean }) {
  useSyncExternalStore(store.subscribeDisplayed, store.getDisplayed, store.getDisplayed);
  const state = controls.state;
  const available = ready(snapshot, store, controls, 'page', busy);
  const reason = state?.capabilities.find((item) => item.name === 'page')?.reason ?? '等待原生页面列表';
  return <label><span className="sr-only">原生应用页面</span><select aria-label="原生应用页面" value={state?.currentPage ?? ''}
    disabled={!available} title={reason} onChange={(event) => void controls.submit({ action: 'navigatePage', page: event.target.value })}>
    <option value="" disabled>{state?.currentPage ? '选择页面' : '当前路由待确认'}</option>
    {state?.pages.map((page) => <option key={page} value={page}>{page}</option>)}
  </select></label>;
}
export function PreviewControlToolbar({ snapshot, store, controls, busy }: { snapshot: AppSnapshot | null; store: FrameStore; controls: NativeControls; busy: boolean }) {
  useSyncExternalStore(store.subscribeDisplayed, store.getDisplayed, store.getDisplayed);
  const state = controls.state;
  return <div className="native-config-toolbar">
    <button className="icon-button" aria-label={`切换设备方向，当前${state ? state.orientation === 'landscape' ? '横屏' : '竖屏' : '待确认'}`}
      disabled={!ready(snapshot, store, controls, 'orientation', busy)} title={state?.capabilities.find((item) => item.name === 'orientation')?.reason ?? '等待原生方向能力'}
      onClick={() => void controls.submit({ action: 'setOrientation', orientation: state?.orientation === 'landscape' ? 'portrait' : 'landscape' })}>
      <span className={`orientation-symbol ${state?.orientation === 'landscape' ? 'landscape' : ''}`} aria-hidden="true" />
    </button>
    <button className="icon-button" aria-label={`切换鸿蒙应用主题，请求值${state ? state.colorMode === 'dark' ? '深色' : '浅色' : '待确认'}，传播待观察`}
      disabled={!ready(snapshot, store, controls, 'colorMode', busy)} title={`主题请求值 · 传播待观察；${state?.capabilities.find((item) => item.name === 'colorMode')?.reason ?? '等待原生应用主题能力'}`}
      onClick={() => void controls.submit({ action: 'setColorMode', colorMode: state?.colorMode === 'dark' ? 'light' : 'dark' })}>
      <Icon name={state?.colorMode === 'dark' ? 'moon' : 'sun'} size={16} />
    </button>
  </div>;
}
