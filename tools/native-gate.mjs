import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { inspectSdk } from './doctor.mjs';

export function nativeChecks({ platform = process.platform, sdk = process.env.ARKDOCK_SDK } = {}) {
  const results = [];
  if (platform !== 'win32') results.push({ status: 'BLOCKED', code: 'WINDOWS_DESKTOP_REQUIRED', message: '需要 Windows 11 原生进程及可交互桌面，WSL 不代替。' });
  results.push(...inspectSdk(sdk));
  return results;
}
if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  const results = nativeChecks();
  const startedAt = Date.now();
  if (results.every((r) => r.status === 'PASS')) {
    // The driver owns deadlines and finally cleanup. Never forcibly kill it with source/trust changed.
    const run = spawnSync(process.execPath,['tools/native-smoke.mjs'],{stdio:'inherit',shell:false,windowsHide:true});
    if (run.error) results.push({status:'BLOCKED',code:'NATIVE_DRIVER_UNAVAILABLE',message:run.error.message});
    else {
      try {
        const receipt = JSON.parse(readFileSync('artifacts/native-smoke.json','utf8'));
        const fresh = Date.parse(receipt.capturedAt) >= startedAt;
        const complete = ['D1','D2','C1/C2','C2'].every((id) => receipt.checks.some((check) => check.id === id && check.status === 'PASS'));
        const status = run.status === 0 && fresh && complete && receipt.status === 'PASS' ? 'PASS' : run.status === 2 ? 'BLOCKED' : 'FAIL';
        results.push({status,code:'D1_D2_NATIVE_ASSERTIONS',message:`本次实际驱动退出 ${run.status}；${receipt.checks.filter((check) => check.status === 'PASS').length} 组真实断言；新证据 ${fresh}；完整范围 ${complete}`});
        results.push(...receipt.checks.map((check) => ({status:check.status,code:check.id,message:check.name})));
      } catch { results.push({status:'FAIL',code:'NATIVE_RECEIPT_INVALID',message:'驱动未生成本次完整有效的验收记录。'}); }
    }
  } else results.push({status:'BLOCKED',code:'D1_D2_UNVERIFIED',message:'真实原生断言未执行；环境或具体 SDK 配置前置不满足。'});
  const status = results.some((r) => r.status === 'FAIL') ? 'FAIL' : results.some((r) => r.status === 'BLOCKED') ? 'BLOCKED' : 'PASS';
  mkdirSync('artifacts', { recursive: true });
  const report = { status, capturedAt: new Date().toISOString(), platform: process.platform,
    checks: results.map((r) => ({ ...r, message: r.code === 'SDK_PATH_ACCESSIBLE' ? '指定 SDK 目录可访问（路径已脱敏）' : r.message })) };
  writeFileSync('artifacts/native-gate.json', `${JSON.stringify(report, null, 2)}\n`);
  for (const result of results) console.log(`${result.status} ${result.code}: ${result.message}`);
  process.exitCode = status === 'PASS' ? 0 : status === 'BLOCKED' ? 2 : 1;
}
