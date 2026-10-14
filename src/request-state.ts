import type { AppSnapshot, RequestScope, TargetInfo } from './ipc/model.ts';

export function sameTarget(a: TargetInfo | null | undefined, b: TargetInfo | null | undefined): boolean {
  return !a || !b ? !a && !b : a.id === b.id && a.process.pid === b.process.pid && a.process.startedAt === b.process.startedAt;
}

/** The scope a new request must carry: the currently confirmed target and session. */
export function currentScope(state: AppSnapshot | null): RequestScope {
  return { targetId: state?.target?.id ?? null, sessionId: state?.sessionId ?? null };
}

/** A reply or a failure only belongs to the UI while its scope is still current. */
export function scopeStillCurrent(state: AppSnapshot | null, scope: RequestScope): boolean {
  return scope.targetId === (state?.target?.id ?? null) && scope.sessionId === (state?.sessionId ?? null);
}

export function responseStillCurrent(result: AppSnapshot, current: AppSnapshot | null): boolean {
  return Boolean(current && sameTarget(result.target, current.target) && result.sessionId === current.sessionId
    && result.project?.id === current.project?.id);
}

export function hasDockCandidate(hint: string | null | undefined): boolean {
  return Boolean(hint?.startsWith('Release to bind '));
}

const dockingMessages: Record<string, string> = {
  'Drag beside an ordinary application window': '正在拖动：靠近目标窗口右侧',
  'Native drag did not start; binding remains detached': '未检测到原生拖动，请重新按住顶部拖动区',
  'Dock bounds are unavailable': '无法读取 ArkDock 窗口边界，请重新拖动',
  'Dock bounds are unavailable (ArkDock native frame inspection failed); no binding will be made': '无法读取 ArkDock 原生窗口边界，本次不会贴附',
  'Desktop enumeration was incomplete; no binding will be made': '桌面窗口读取不完整，本次不会贴附',
  'No unobscured ordinary window edge is close enough': '尚未靠近可见的普通窗口边缘',
  'No live eligible target at release; binding remains detached': '松开时没有有效目标，请重新靠近窗口右侧',
  'No live eligible target at release; binding remains detached (candidate lifecycle or eligibility changed before release)': '松开前目标窗口已失效，请重新拖动贴附',
  'Window event continuity was lost; drag again to rebind': '窗口事件连续性中断，请重新拖动贴附',
  'Window lifecycle tracking limit reached; drag again': '窗口生命周期记录已重置，请重新拖动',
  'Target window closed; binding was removed': '目标窗口已关闭，贴附已解除',
  'Target lifecycle changed while ArkDock was collapsed': '收起期间目标窗口已变化，请重新贴附',
  'Target lifecycle changed while collapsed; expansion is detached': '收起期间目标窗口已变化，展开后请重新贴附',
  'Target lifecycle changed before joint expansion': '展开前目标窗口已变化，请重新贴附',
  'Target lifecycle cannot be verified; binding was removed': '无法核验目标窗口生命周期，贴附已解除',
  'Target is no longer an eligible ordinary window': '目标已不符合普通窗口条件，贴附已解除',
  'Target bounds cannot be read': '无法读取目标窗口边界，请重新贴附',
  'Target visible bounds cannot be read': '无法读取目标可见边界，请重新贴附',
  'Window lifecycle changed before positioning': '定位前窗口生命周期已变化，请重新贴附',
};
const dockingCounts = /^No unobscured ordinary window edge is close enough \(enumerated=(\d{1,4}), visible=(\d{1,4}), eligible=(\d{1,4}), nearby=(\d{1,4}), occluded=(\d{1,4}), unreadable=(\d{1,4}), lifecycleRejected=(\d{1,4}), trackFailed=(\d{1,4})\)$/;
const enumerationFailure = /^Desktop enumeration was incomplete; no binding will be made \((?:window limit (1024) reached|Win32 error (\d{1,10})); enumerated (\d{1,4})\)$/;
const releaseReasonPrefix = 'No live eligible target at release; binding remains detached · ';

/** Native hints may contain foreign window titles. Only fixed messages and counts leave this boundary. */
export function dockingStatus(hint: string | null | undefined): { label: string; detail: string } | null {
  if (!hint) return null;
  if (hasDockCandidate(hint)) return { label: '松开以连接 · 靠近目标外侧', detail: 'candidate available (target title omitted)' };
  const released = hint.startsWith(releaseReasonPrefix);
  // Optional effect diagnostics are not part of the count-only docking result.
  const source = (released ? hint.slice(releaseReasonPrefix.length) : hint).split(' · ')[0];
  const prefix = released ? '松开未贴附：' : '';
  const message = Object.hasOwn(dockingMessages, source) ? dockingMessages[source] : null;
  if (message) return { label: `${prefix}${message}`, detail: `${prefix}${message}` };
  const counts = dockingCounts.exec(source);
  if (counts && counts.slice(1).every((value) => Number(value) <= 1024)) return {
    label: `${prefix}没有可贴附边缘 · 接近 ${counts[4]} · 遮挡 ${counts[5]}`,
    detail: `${prefix}没有可贴附边缘 · 枚举 ${counts[1]} · 可见 ${counts[2]} · 合格 ${counts[3]} · 接近 ${counts[4]} · 遮挡 ${counts[5]} · 边界不可读 ${counts[6]} · 生命周期拒绝 ${counts[7]} · 跟踪失败 ${counts[8]}`,
  };
  const enumeration = enumerationFailure.exec(source);
  if (enumeration && Number(enumeration[3]) <= 1024
    && (!enumeration[2] || Number(enumeration[2]) <= 4_294_967_295)) return {
    label: `${prefix}桌面窗口读取不完整，本次不会贴附`,
    detail: `${prefix}桌面窗口读取不完整 · 已枚举 ${enumeration[3]} · ${enumeration[1] ? '达到窗口数量上限 1024' : `Win32 错误 ${enumeration[2]}`}`,
  };
  return { label: '贴附状态待确认，请查看运行日志', detail: 'unrecognized native docking status (content omitted)' };
}
