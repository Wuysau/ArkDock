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
