import { existsSync, readFileSync, statSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { matchSdk } from './sdk-profile.mjs';

const require = createRequire(import.meta.url);
const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));

const check = (status, code, message) => ({ status, code, message });
export function nodeVersionCheck(version = process.version) {
  const minimum = /^>=(\d+)\.(\d+)\.(\d+)$/.exec(pkg.engines.node)?.slice(1).map(Number);
  const actual = /^v?(\d+)\.(\d+)\.(\d+)/.exec(version)?.slice(1).map(Number);
  if (!minimum || !actual) return check('FAIL', 'NODE_VERSION_INVALID', 'Node 版本或工程版本下限无法解析');
  let supported = true;
  for (let i = 0; i < 3; i++) { if (actual[i] !== minimum[i]) { supported = actual[i] > minimum[i]; break; } }
  return check(supported ? 'PASS' : 'BLOCKED', 'NODE_VERSION', `${version}；工程要求 ${pkg.engines.node}`);
}
function windowsPrerequisites() {
  const inventory = spawnSync('pwsh', ['-NoProfile', '-File', fileURLToPath(new URL('./windows-doctor.ps1', import.meta.url))], { encoding: 'utf8', shell: false, windowsHide: true, timeout: 10_000, maxBuffer: 65_536 });
  if (inventory.error) return [check('BLOCKED', 'WINDOWS_INVENTORY_UNAVAILABLE', inventory.error.code ?? '无法读取系统前置')];
  if (inventory.status !== 0) return [check('FAIL', 'WINDOWS_INVENTORY_FAILED', `只读前置探测退出 ${inventory.status}`)];
  let installed;
  try { installed = JSON.parse(inventory.stdout); } catch { return [check('FAIL', 'WINDOWS_INVENTORY_INVALID', '系统前置探测未返回有效 JSON')]; }
  return [
    check(installed.build >= 22000 ? 'PASS' : 'BLOCKED', 'WINDOWS_VERSION', `${installed.caption} ${installed.version} ${installed.architecture}`),
    check(installed.msvc.length && installed.windowsSdk.length ? 'PASS' : 'BLOCKED', 'MSVC_WINDOWS_SDK', `MSVC ${installed.msvc.join(', ') || '缺失'}；Windows SDK ${installed.windowsSdk.join(', ') || '缺失'}`),
    check(installed.webView2.length ? 'PASS' : 'BLOCKED', 'WEBVIEW2', installed.webView2.join(', ') || '未发现已安装 Runtime'),
    check(installed.interactive ? 'PASS' : 'BLOCKED', 'DESKTOP_SESSION', `可交互标记 ${installed.interactive}；显示器 ${installed.monitors}；实际窗口断言另见 verify:windows`),
  ];
}
export function inspectSdk(path) {
  if (!path) return [check('BLOCKED', 'SDK_NOT_CONFIGURED', '使用 --sdk <目录> 或 ARKDOCK_SDK 指定合法安装的 SDK；不会全盘扫描。')];
  try {
    if (!statSync(path).isDirectory()) return [check('BLOCKED', 'SDK_PATH_NOT_DIRECTORY', '指定 SDK 不是目录。')];
  } catch { return [check('BLOCKED', 'SDK_PATH_INACCESSIBLE', '指定 SDK 路径不可访问。')]; }
  const profile = matchSdk(path);
  return [
    check('PASS', 'SDK_PATH_ACCESSIBLE', `目录可访问：${resolve(path)}；尚不能据此确认 SDK 完整性或版本。`),
    profile ? check('PASS','SDK_PROFILE_MATCHED',`SDK ${profile.sdkVersion} / API ${profile.apiVersion}；Previewer 与构建入口 SHA256 符合已实测配置，完整断言仍由 verify:native 执行。`)
      : check('BLOCKED', 'PROTOCOL_UNVERIFIED', '未匹配已实测 SDK 配置；存在目录或二进制不等于可用后端。请选择完整 Command Line Tools 根目录。'),
  ];
}
export function toolVersion(name, args) {
  const result = spawnSync(name, args, { encoding: 'utf8', shell: false, timeout: 5000, maxBuffer: 16_384 });
  if (result.error) return check('BLOCKED', `${name.toUpperCase()}_UNAVAILABLE`, result.error.code ?? '无法启动工具版本探测');
  if (result.status !== 0) return check('FAIL', `${name.toUpperCase()}_VERSION_FAILED`, `退出 ${result.status}`);
  return check('PASS', `${name.toUpperCase()}_VERSION`, result.stdout.trim());
}
export function diagnose(sdk = null) {
  const linuxKernel = process.platform === 'linux' && existsSync('/proc/sys/kernel/osrelease')
    ? readFileSync('/proc/sys/kernel/osrelease', 'utf8').trim() : '';
  const environment = linuxKernel.toLowerCase().includes('microsoft') ? 'WSL2' : process.platform;
  return [
    check(process.platform === 'win32' ? 'PASS' : 'BLOCKED', 'PLATFORM', `${environment} ${process.arch}：产品原生桌面只支持 Windows 11 本地；桌面会话仍需人工核验。`),
    nodeVersionCheck(),
    toolVersion('rustc', ['--version']),
    toolVersion('cargo', ['--version']),
    ...process.platform === 'win32' ? windowsPrerequisites() : [],
    (() => { try { return { ...toolVersion(process.execPath, [require.resolve('@tauri-apps/cli/tauri.js'), '--version']), code: 'TAURI_CLI_VERSION' }; } catch { return check('BLOCKED', 'TAURI_CLI_MISSING', '锁定的项目内 Tauri CLI 尚未安装'); } })(),
    ...inspectSdk(sdk),
    check('SKIPPED', 'NATIVE_ACCEPTANCE_SEPARATE', 'doctor 只读检查环境；运行 pnpm verify:native 执行真实构建、画面、点击和 .ets 更新断言。'),
  ];
}
if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  const index = process.argv.indexOf('--sdk');
  const sdk = index >= 0 ? process.argv[index + 1] : process.env.ARKDOCK_SDK;
  if (index >= 0 && !sdk) { console.error('FAIL --sdk 需要目录参数'); process.exitCode = 1; }
  else {
    const results = diagnose(sdk);
    for (const result of results) console.log(`${result.status} ${result.code}: ${result.message}`);
    process.exitCode = results.some((r) => r.status === 'FAIL') ? 1 : results.some((r) => r.status === 'BLOCKED') ? 2 : 0;
  }
}
