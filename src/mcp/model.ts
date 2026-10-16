import type { AppSnapshot, McpContext, McpTaskState, McpTaskSummary, SelectionContext } from '../ipc/model.ts';

export const taskLabels: Record<McpTaskState, string> = {
  pending: '等待客户端领取', claimed: '客户端已领取', acknowledged: 'Agent 已确认',
  completed: 'Agent 报告完成', failed: 'Agent 报告失败', expired: '任务已过期', unknown: '连接中断 · 结果未知',
};

export function contextMatches(snapshot: AppSnapshot | null, context: McpContext): boolean {
  const registration = snapshot?.contextSource?.registrations.find(entry => entry.id === context.registrationId);
  return Boolean(snapshot?.target && snapshot.project && snapshot.sessionId && snapshot.trusted
    && context.connected && context.confirmed
    && context.registrationId === snapshot.contextSource?.boundRegistrationId
    && context.environment === snapshot.project.environment
    && registration?.executionDirectory === context.executionDirectory
    && registration.originalProject === context.originalProject);
}

export function selectionMatches(snapshot: AppSnapshot | null, selection: SelectionContext, now = Date.now()): boolean {
  return Boolean(snapshot?.trusted && snapshot.project?.id === selection.workspaceId
    && snapshot.sessionId === selection.sessionId && snapshot.build.buildId === selection.buildId
    && snapshot.build.generation === selection.generation && selection.expiresAt > now);
}

export function canRetry(snapshot: AppSnapshot | null, context: McpContext | undefined, task: McpTaskSummary): boolean {
  return Boolean(context && contextMatches(snapshot, context)
    && (task.state === 'failed' || task.state === 'unknown'));
}

export function mcpLifecycle(snapshot: AppSnapshot | null): string {
  return JSON.stringify([snapshot?.target?.id, snapshot?.target?.process, snapshot?.sessionId, snapshot?.project?.id,
    snapshot?.build.buildId, snapshot?.build.generation, snapshot?.trusted, snapshot?.contextSource?.boundRegistrationId]);
}

export async function submitFrozenSelection({ context, selectionId, save, isCurrent, snapshot, submit }: {
  context: McpContext; selectionId: string; save: () => Promise<SelectionContext>; isCurrent: () => boolean;
  snapshot: () => AppSnapshot | null; submit: (selectionId: string, contextId: string) => Promise<McpTaskSummary>;
}): Promise<McpTaskSummary | null> {
  const saved = await save();
  if (!isCurrent() || saved.selectionId !== selectionId || !selectionMatches(snapshot(), saved) || !contextMatches(snapshot(), context)) return null;
  const task = await submit(saved.selectionId, context.contextId);
  return isCurrent() ? task : null;
}

/** One request at a time, including across stop/start lifecycle transitions. */
export function createStatusPoller<T>(request: () => Promise<T>, schedule = (callback: () => void, delay: number) => setTimeout(callback, delay), cancel = (timer: ReturnType<typeof setTimeout>) => clearTimeout(timer), now = Date.now) {
  let revision = 0;
  let active: { revision: number; value: (value: T) => void; error: (error: unknown) => void } | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let inFlight = false;
  let lastStarted = -Infinity;
  function queue() {
    if (!active || inFlight || timer !== null) return;
    timer = schedule(() => { timer = null; void tick(); }, Math.max(0, 1000 - (now() - lastStarted)));
  }
  async function tick() {
    if (!active || inFlight) return;
    const requested = active;
    inFlight = true; lastStarted = now();
    try { const result = await request(); if (active?.revision === requested.revision) requested.value(result); }
    catch (error) { if (active?.revision === requested.revision) requested.error(error); }
    finally { inFlight = false; queue(); }
  }
  return {
    start(value: (value: T) => void, error: (error: unknown) => void) {
      const requested = ++revision;
      active = { revision: requested, value, error };
      queue();
      return () => {
        if (active?.revision !== requested) return;
        active = null;
        if (timer !== null) cancel(timer);
        timer = null;
      };
    },
  };
}
