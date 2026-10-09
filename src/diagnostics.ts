import type { AppSnapshot } from './ipc/model.ts';
export function diagnosticText(state: AppSnapshot | null): string {
  if (!state) return 'ArkDock · 桌面后端未连接';
  const paths = [state.project?.appRoot, state.project?.workspaceRoot, state.backend.sdkPath,
    ...state.candidates.flatMap((candidate) => [candidate.project.appRoot, candidate.project.workspaceRoot])]
    .filter((path): path is string => Boolean(path)).sort((a, b) => b.length - a.length);
  const text = [`ArkDock · ${state.backend.id} · SDK ${state.backend.sdkVersion ?? '未知'}`,
    `状态 ${state.phase} · 代码代次 ${state.build.generation} · 画面代次 ${state.build.displayedGeneration ?? '未知'}`,
    ...state.diagnostics.map((item) => `${new Date(item.observedAt).toISOString()} ${item.status} ${item.code}: ${item.message}`)].join('\n');
  let redacted = text;
  for (const path of paths) redacted = redacted.split(path).join('[本地路径]');
  return redacted.replace(/[A-Z]:[\\/][^\s"<>，；]+/gi, '[本地路径]')
    .replace(/\\\\[^\s"<>，；]+/g, '[网络路径]')
    .replace(/\/(?:home|Users|mnt|workspaces)\/[^\s"<>，；]+/g, '[环境路径]');
}
