import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import test from 'node:test';
import { nativeChecks } from '../../tools/native-gate.mjs';
import { inspectSdk, nodeVersionCheck } from '../../tools/doctor.mjs';

test('真实 Node 版本必须满足锁定工程下限，不能降级版本要求让基础 gate 通过', () => {
  assert.equal(nodeVersionCheck('v24.13.0').status, 'BLOCKED');
  assert.equal(nodeVersionCheck('v24.21.0').status, 'PASS');
  assert.equal(nodeVersionCheck('invalid').status, 'FAIL');
});

test('无 Windows 或未验证协议不能让原生门禁绿灯', () => {
  const unsupported = nativeChecks({ platform: 'linux', sdk: null });
  assert.ok(unsupported.some((c) => c.code === 'WINDOWS_DESKTOP_REQUIRED' && c.status === 'BLOCKED'));
  const directory = mkdtempSync(join(tmpdir(), 'arkdock-unverified-sdk-'));
  try {
    writeFileSync(join(directory, 'Previewer.exe'), 'unverified binary; must not execute');
    const noProtocol = nativeChecks({ platform: 'win32', sdk: directory });
    assert.ok(noProtocol.some((c) => c.code === 'PROTOCOL_UNVERIFIED' && c.status === 'BLOCKED'));
  } finally { rmSync(directory, { recursive: true }); }
});

test('原生检查进程缺环境返回非成功状态', () => {
  const directory = mkdtempSync(join(tmpdir(), 'arkdock-gate-fixture-'));
  try {
    const result = spawnSync(process.execPath, [resolve('tools/native-gate.mjs')], { cwd: directory, stdio: 'inherit', env: { ...process.env, ARKDOCK_SDK: '' } });
    assert.equal(result.error, undefined);
    assert.equal(result.status, 2);
    const report = JSON.parse(readFileSync(join(directory, 'artifacts/native-gate.json'), 'utf8'));
    assert.equal(report.status, 'BLOCKED');
    assert.ok(report.checks.some((c) => c.code === 'SDK_NOT_CONFIGURED'));
  } finally { rmSync(directory, { recursive: true }); }
});

test('doctor 仅探测指定 SDK；不执行目录中的可执行文件', () => {
  const dir = mkdtempSync(join(tmpdir(), 'arkdock-sdk-'));
  try {
    writeFileSync(join(dir, 'Previewer.exe'), 'would be executable; must not run');
    const diagnostics = inspectSdk(dir);
    assert.ok(diagnostics.some((c) => c.code === 'SDK_PATH_ACCESSIBLE'));
    assert.ok(diagnostics.some((c) => c.code === 'PROTOCOL_UNVERIFIED'));
    assert.doesNotMatch(JSON.stringify(diagnostics), /would be executable/);
  } finally { rmSync(dir, { recursive: true }); }
});

test('不存在 SDK 与 Linux/WSL 身份显式诊断', () => {
  assert.equal(inspectSdk(null)[0].code, 'SDK_NOT_CONFIGURED');
  assert.equal(inspectSdk('/definitely/missing/arkdock')[0].status, 'BLOCKED');
});
