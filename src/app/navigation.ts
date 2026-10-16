import type { AppSnapshot, TargetInfo } from '../ipc/model.ts';

export type Panel = 'projects' | 'sdk' | 'trust' | 'path' | 'settings' | 'menu' | 'controls' | 'diagnostics' | 'help' | 'mcp';
export const panelNames: Record<Panel, string> = {
  projects: '选择关联工程', sdk: '设备与原生工具链', trust: '工程信任', path: '工程身份与路径',
  mcp: 'MCP 接入', settings: '偏好设置', menu: '更多操作', controls: '预览能力', diagnostics: '运行日志', help: '使用帮助',
};

/** Lifecycle keys already used for an automatic popup, so it is never repeated. */
export interface AutomaticPanels { projects: string | null; trust: string | null }
export const emptyAutomaticPanels = (): AutomaticPanels => ({ projects: null, trust: null });

export function targetLifecycle(target: TargetInfo): string {
  return `${target.id}:${target.process.pid}:${target.process.startedAt}`;
}

/**
 * Pure popup decision. The same target lifecycle (or session for trust) is
 * consumed at most once, which is what keeps an automatic panel from re-opening
 * in a loop when the snapshot keeps reporting the same pending phase. The caller
 * still excludes an in-flight cancellation.
 */
export function automaticPanel(snapshot: AppSnapshot, seen: AutomaticPanels): { panel: Panel | null; seen: AutomaticPanels } {
  const target = snapshot.target;
  if (!target || snapshot.display.mode !== 'expanded') return { panel: null, seen };
  const lifecycle = targetLifecycle(target);
  if (snapshot.phase === 'selectProject' && seen.projects !== lifecycle) return { panel: 'projects', seen: { ...seen, projects: lifecycle } };
  if (snapshot.phase === 'untrusted' && snapshot.sessionId && seen.trust !== snapshot.sessionId) return { panel: 'trust', seen: { ...seen, trust: snapshot.sessionId } };
  return { panel: null, seen };
}
