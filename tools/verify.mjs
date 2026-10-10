import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { sourceFiles } from './files.mjs';
import { exitStatus } from './check-status.mjs';
import { nodeVersionCheck } from './doctor.mjs';

const records = [];
records.push({ name: 'node-engine', ...nodeVersionCheck() });
function run(name, command, args, blockedExitCodes = []) {
  const start = Date.now();
  const result = spawnSync(command, args, { stdio: 'inherit', shell: false });
  const status = exitStatus(result.status, result.error?.code, blockedExitCodes);
  const record = { name, status, durationMs: Date.now() - start, exitCode: result.status, error: result.error?.code };
  records.push(record); console.log(`${status} ${name}${record.error ? ` (${record.error})` : ''}`);
}
function npmStep(name, script) {
  if (!existsSync('node_modules/.modules.yaml')) { records.push({ name, status: 'BLOCKED', reason: '锁定的前端依赖尚未安装' }); console.log(`BLOCKED ${name}: 依赖未安装`); return; }
  if (process.env.npm_execpath) run(name, process.execPath, [process.env.npm_execpath, 'run', script]);
  else run(name, 'pnpm', ['run', script]);
}
run('repository-format', process.execPath, ['tools/check-format.mjs']);
run('ipc-contract', process.execPath, ['tools/generate-ipc.mjs', '--check']);
run('harness', process.execPath, ['tools/verify-harness.mjs']);
run('boundaries', process.execPath, ['tools/check-boundaries.mjs']);
run('portable-tests', process.execPath, ['--test', '--test-isolation=none', ...sourceFiles('tests/portable').filter((p) => p.endsWith('.test.mjs'))]);
if (process.argv.includes('--release') && !existsSync('src-tauri/Cargo.lock')) {
  records.push({ name: 'rust-lockfile', status: 'BLOCKED', reason: 'Cargo.lock 尚未在可用 Rust 环境生成并纳入交付' });
}
npmStep('frontend-lint', 'lint');
npmStep('frontend-typecheck', 'typecheck');
npmStep('frontend-build', 'build');
run('rust-fmt', 'cargo', ['fmt', '--manifest-path', 'src-tauri/Cargo.toml', '--all', '--', '--check']);
run('rust-clippy', 'cargo', ['clippy', '--locked', '--manifest-path', 'src-tauri/Cargo.toml', '--features', 'native-tools', '--all-targets', '--', '-D', 'warnings']);
run('rust-test', 'cargo', ['test', '--locked', '--manifest-path', 'src-tauri/Cargo.toml', '--features', 'native-tools', '--all-targets']);
run('rust-build', 'cargo', ['build', '--locked', '--manifest-path', 'src-tauri/Cargo.toml', '--features', 'native-tools']);
if (process.platform === 'win32') {
  run('windows-desktop-check', 'cargo', ['check', '--locked', '--manifest-path', 'src-tauri/Cargo.toml', '--features', 'desktop,native-tools', '--all-targets']);
  run('windows-desktop-clippy', 'cargo', ['clippy', '--locked', '--manifest-path', 'src-tauri/Cargo.toml', '--features', 'desktop,native-tools', '--all-targets', '--', '-D', 'warnings']);
}
if (process.argv.includes('--release')) {
  run('native-acceptance', process.execPath, ['tools/native-gate.mjs'], [2]);
  if (records.every((r) => r.status === 'PASS')) run('windows-package', process.execPath, ['tools/desktop.mjs', 'build']);
  else records.push({ name: 'windows-package', status: 'BLOCKED', reason: '发布前置门禁未全部通过' });
}
const status = records.some((r) => r.status === 'FAIL') ? 'FAIL' : records.some((r) => r.status !== 'PASS') ? 'BLOCKED' : 'PASS';
mkdirSync('artifacts', { recursive: true });
writeFileSync(`artifacts/verify${process.argv.includes('--release') ? '-release' : ''}.json`, `${JSON.stringify({ status, capturedAt: new Date().toISOString(), platform: process.platform, records }, null, 2)}\n`);
console.log(`${status} ${process.argv.includes('--release') ? 'release' : 'base'} verification`);
process.exitCode = status === 'PASS' ? 0 : status === 'BLOCKED' ? 2 : 1;
