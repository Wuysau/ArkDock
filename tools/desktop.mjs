import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);

if (process.platform !== 'win32') { console.error('BLOCKED：ArkDock 桌面开发/打包需要 Windows 11 原生环境。'); process.exitCode = 2; }
else {
  const action = process.argv[2];
  if (!['dev', 'build'].includes(action)) { console.error('FAIL：仅允许 dev/build。'); process.exitCode = 1; }
  else {
    // The pinned official CLI is project-local; no global cargo installation or PATH mutation.
    const args = [require.resolve('@tauri-apps/cli/tauri.js'), action, '--features', 'desktop'];
    if (action === 'build') args.push('--', '--locked');
    const result = spawnSync(process.execPath, args, { stdio: 'inherit', shell: false });
    process.exitCode = result.error ? 2 : result.status ?? 1;
  }
}
