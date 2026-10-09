import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

test('IPC 单一来源两端生成结果一致', () => {
  const result = spawnSync(process.execPath, ['tools/generate-ipc.mjs', '--check'], { stdio: 'inherit' });
  assert.equal(result.error, undefined);
  assert.equal(result.status, 0);
  const definition = JSON.parse(readFileSync('protocol/model.json', 'utf8'));
  const rust = readFileSync('src-tauri/src/ipc/model.rs', 'utf8');
  const ts = readFileSync('src/ipc/model.ts', 'utf8');
  for (const name of Object.keys(definition.structs)) {
    assert.ok(rust.includes(`pub struct ${name}`));
    assert.ok(ts.includes(`export interface ${name}`));
  }
});

test('正式路径不能自动选择测试后端', () => {
  const config = JSON.parse(readFileSync('src-tauri/native-support.json', 'utf8'));
  assert.equal(config.backendId, 'native-previewer');
  const source = readFileSync('src-tauri/src/preview.rs', 'utf8');
  assert.ok(source.includes('PROTOCOL_UNVERIFIED'));
  assert.doesNotMatch(source.split('#[cfg(test)]')[0], /MockBackend|TestBackend|fallback_to_mock/);
});

test('Rust 自定义命令的参数与单一 IPC 定义一致', () => {
  const definition = JSON.parse(readFileSync('protocol/model.json', 'utf8'));
  const source = readFileSync('src-tauri/src/desktop.rs', 'utf8');
  for (const [name, fields] of Object.entries(definition.commands)) {
    const signature = source.match(new RegExp(`#\\[tauri::command\\]\\s*(?:async )?fn ${name}\\(([^)]*)\\)\\s*->\\s*Result<AppSnapshot, String>`));
    assert.ok(signature, `未注册/契约返回值错误：${name}`);
    for (const [field, type] of Object.entries(fields)) {
      const rustName = field.replace(/[A-Z]/g, (char) => `_${char.toLowerCase()}`);
      const rustType = { string: 'String', bool: 'bool' }[type] ?? type;
      assert.ok(signature[1].includes(`${rustName}: ${rustType}`), `${name}.${field} 参数漂移`);
    }
  }
});
