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
  // 会话源配对是有意的预贴附能力：必须先于任何工程/会话存在，否则发现被
  // “先选工程才能发现工程”死锁。后端仍核验调用窗口，且不授予执行信任。
  const preDockingCommands = new Set(['context_pair_create', 'context_pair_revoke']);
  for (const [name, fields] of Object.entries(spec.commands)) {
    if (name !== 'snapshot' && !preDockingCommands.has(name)) assert.equal(fields.scope, 'RequestScope', name);
  }
  const source = ['src-tauri/src/desktop.rs', 'src-tauri/src/desktop/inspection.rs'].map((path) => readFileSync(path, 'utf8')).join('\n');
  assert.match(source, /validate_scope\(&self\.snapshot, &scope\)/);
  for (const name of ['select_directory', 'configure_sdk', 'selection_export_image']) {
    const body = source.slice(source.indexOf(`async fn ${name}(`)).split('#[tauri::command]')[0];
    assert.match(body, /check_window\(&window\)/, name);
    assert.match(body, /validate_scope\(&service\.snapshot\(\)\?, &scope\)/, name);
    assert.match(body, name === 'selection_export_image' ? /data_request\(\s*window,\s*service,\s*scope/ : /request\(/, `${name} validates again after dialog`);
  }
  assert.match(source, /engine\.drain_events\(\);\s*let result = engine\.handle\(scope, action\)/);
});
