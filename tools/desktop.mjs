import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';

const require = createRequire(import.meta.url);

if (process.platform !== 'win32') { console.error('BLOCKED：ArkDock 桌面开发/打包需要 Windows 11 原生环境。'); process.exitCode = 2; }
else {
  const action = process.argv[2];
  const validation = process.argv[3] === '--validation';
  if (!['dev', 'build'].includes(action) || (process.argv.length > 3 && (!validation || action !== 'dev')) || process.argv.length > 4) { console.error('FAIL：仅允许 dev/build，以及 dev --validation。'); process.exitCode = 1; }
  else {
    // The pinned official CLI is project-local; no global cargo installation or PATH mutation.
    const args = [require.resolve('@tauri-apps/cli/tauri.js'), action, '--features', 'desktop'];
    const env = { ...process.env };
    if (validation) {
      args.push('--config', 'tests/native/desktop-validation.json');
      env.WEBVIEW2_USER_DATA_FOLDER = resolve('.local/webview-validation');
      env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9224';
    }
    if (action === 'build') args.push('--', '--locked');
    const result = spawnSync(process.execPath, args, { stdio: 'inherit', shell: false, env });
    process.exitCode = result.error ? 2 : result.status ?? 1;
  }
}
