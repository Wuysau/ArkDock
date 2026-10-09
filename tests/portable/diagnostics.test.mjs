import assert from 'node:assert/strict';
import test from 'node:test';
import { diagnosticText } from '../../src/diagnostics.ts';

test('复制诊断脱敏当前工程、SDK、其他候选和无登记的 Windows/WSL 路径', () => {
  const state = {
    phase: 'stale', project: { appRoot: 'D:\\Private Project\\app', workspaceRoot: 'D:\\Private Project' },
    backend: { id: 'native-previewer', sdkPath: 'D:/SDK tools/harmony', sdkVersion: '26.0.0.105' },
    build: { generation: 2, displayedGeneration: 1 },
    candidates: [{ project: { appRoot: 'D:/Other/app', workspaceRoot: 'D:/Other' } }],
    diagnostics: [{ observedAt: 1791500000000, status: 'FAIL', code: 'BUILD',
      message: 'D:\\Private Project\\app; D:/SDK tools/harmony; D:/Other/app; C:\\Users\\secret\\test.ets; \\\\wsl.localhost\\Ubuntu\\home\\private; /home/person/app' }],
  };
  const result = diagnosticText(state);
  assert.match(result, /FAIL BUILD/);
  assert.match(result, /代码代次 2/);
  for (const secret of ['Private Project', 'SDK tools', 'D:/Other', 'secret', 'person', 'Ubuntu']) assert.equal(result.includes(secret), false, secret);
  assert.match(diagnosticText(null), /桌面后端未连接/);
});

test('未知历史路径含空格时不会泄露私有后缀', () => {
  const state = { backend: { id: 'native', sdkVersion: null }, build: { generation: 1, displayedGeneration: null }, candidates: [], diagnostics: [
    { observedAt: 1, status: 'FAIL', code: 'PATH', message: 'C:\\Users\\Alice Smith\\Secret project\\entry\\Index.ets; \\\\server\\Team Project\\private\\entry.ets' },
  ] };
  const text = diagnosticText(state);
  assert.match(text, /FAIL PATH/);
  for (const part of ['Smith', 'Secret', 'Team', 'private', 'entry.ets']) assert.equal(text.includes(part), false, part);
});

test('环境路径空格与 Windows 路径单引号不截断脱敏，后续诊断记录仍保留', () => {
  const state = { backend: { id: 'native', sdkVersion: null }, build: { generation: 1, displayedGeneration: null }, candidates: [], diagnostics: [
    { observedAt: 1, status: 'FAIL', code: 'POSIX_PATH', message: '/home/alice/Secret Project/entry/Index.ets' },
    { observedAt: 2, status: 'FAIL', code: 'WIN_PATH', message: "C:\\Users\\Bob's Private Project\\entry\\Index.ets" },
    { observedAt: 3, status: 'PASS', code: 'FOLLOWING', message: '下一条正常记录' },
  ] };
  const text = diagnosticText(state);
  for (const part of ['alice', 'Secret', 'Private', 'Bob', 'Index.ets']) assert.equal(text.includes(part), false, part);
  assert.match(text, /PASS FOLLOWING: 下一条正常记录/);
});
