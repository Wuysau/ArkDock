import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { exitStatus } from '../../tools/check-status.mjs';

test('编译器退出 2 是失败，只有原生 gate 定义的 2 是阻塞', () => {
  assert.equal(exitStatus(2, undefined), 'FAIL');
  assert.equal(exitStatus(2, undefined, [2]), 'BLOCKED');
  assert.equal(exitStatus(0, 'EPERM'), 'BLOCKED');
  assert.equal(exitStatus(null, 'ENOENT'), 'BLOCKED');
  assert.equal(exitStatus(0, undefined), 'PASS');
});

test('所有修改命令带当前目标与会话，dialog 前后由后端核验', () => {
  const spec = JSON.parse(readFileSync('protocol/model.json', 'utf8'));
  for (const [name, fields] of Object.entries(spec.commands)) {
    if (name !== 'snapshot') assert.equal(fields.scope, 'RequestScope', name);
  }
  const source = readFileSync('src-tauri/src/desktop.rs', 'utf8');
  assert.match(source, /validate_scope\(&self\.snapshot, &scope\)/);
  assert.equal((source.match(/validate_scope\(&service\.snapshot\(\)\?, &scope\)/g) ?? []).length, 2);
  assert.match(source, /engine\.drain_events\(\);\s*let result = engine\.handle\(scope, action\)/);
});
