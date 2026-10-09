import { readFileSync } from 'node:fs';
import { sourceFiles } from './files.mjs';

const errors = [];
for (const path of sourceFiles('src')) {
  if (!/\.(ts|tsx)$/.test(path)) continue;
  const code = readFileSync(path, 'utf8');
  if (/node:|@tauri-apps\/plugin-(shell|fs|process)|child_process|SetParent|EnumWindows|SetWindowPos/.test(code)) errors.push(`${path}: UI 禁止系统/进程能力`);
  if (!path.startsWith('src/ipc/') && /__TAURI__|\binvoke\s*\(|\bfetch\s*\(|\bWebSocket\s*\(/.test(code)) errors.push(`${path}: 原生/网络入口必须集中于 src/ipc`);
}
const capability = JSON.parse(readFileSync('src-tauri/capabilities/main.json', 'utf8'));
if (capability.permissions.some((p) => /shell|fs:|process:|:default$/.test(p))) errors.push('Tauri capabilities 超出最小事件监听权限');
for (const path of sourceFiles('src-tauri/src')) {
  if (!path.endsWith('.rs')) continue;
  const code = readFileSync(path, 'utf8').split('#[cfg(test)]')[0];
  if (/Command::new\("(cmd|powershell|sh|bash)"\)|taskkill|killall|SetParent\(/.test(code)) errors.push(`${path}: 不允许任意 Shell、按名结束进程或跨进程父化`);
}
for (const error of errors) console.error(`FAIL ${error}`);
if (errors.length) process.exitCode = 1;
else console.log('PASS 能力入口/最小权限结构检查（文本检查不证明所有运行期安全）。');
