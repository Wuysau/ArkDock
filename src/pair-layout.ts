import type { AppSnapshot } from './ipc/model.ts';

export function pairControl(snapshot: AppSnapshot | null) {
  const state = snapshot?.pairLayout?.state ?? 'unbound';
  const pending = state === 'maximizing' || state === 'restoring';
  const labels = { unbound: '未连接共同工作区', paired: '全贴合 · 联动缩放', maximizing: '正在共同最大化',
    maximized: '共同最大化 · ArkDock 保持右侧', restoring: '正在恢复共同布局', blocked: '共同布局暂不可用' };
  return {
    state, pending,
    command: state === 'maximized' ? 'restore_pair' as const : 'maximize_pair' as const,
    action: state === 'maximized' ? '恢复共同布局' : '共同最大化',
    enabled: Boolean(snapshot?.target && (state === 'paired' || state === 'maximized')),
    label: labels[state],
    reason: snapshot?.pairLayout?.reason ?? (snapshot?.target ? labels[state] : '先拖到开发窗口旁，连接共同工作区'),
  };
}
