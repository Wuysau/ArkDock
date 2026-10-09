import { useSyncExternalStore } from 'react';
import type { AppSnapshot } from './ipc/model.ts';
import type { FrameStore } from './frame-store.ts';
import { deriveUiState } from './view-state.ts';

const labels = { click: '点击', swipe: '拖拽 / 滑动', text: '文字输入 / IME', rotate: '设备方向',
  page: '页面切换', device: '设备切换', appTheme: '鸿蒙应用主题', emulator: '模拟器', physicalDevice: '真机' };

export function PreviewCapabilities({ snapshot, store, busy, refresh }: {
  snapshot: AppSnapshot | null; store: FrameStore; busy: boolean; refresh: () => void;
}) {
  const frame = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const { capabilities } = deriveUiState(snapshot, frame);
  return <>
    <p className="muted">能力来自当前后端。画面缩放只影响查看尺寸。</p>
    <div className="capability-list">
      {(Object.keys(labels) as (keyof typeof labels)[]).map((name) => <div key={name}>
        <strong>{labels[name]}</strong>
        <span className={`badge ${capabilities[name].status === 'available' ? '' : 'invalid'}`}>{capabilities[name].status}</span>
        <p>{capabilities[name].reason}</p>
      </div>)}
    </div>
    <button className="primary-button" disabled={capabilities.refresh.status !== 'available' || busy}
      title={capabilities.refresh.reason} onClick={refresh}>手动刷新</button>
  </>;
}
