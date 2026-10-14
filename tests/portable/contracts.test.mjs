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
  const desktop = readFileSync('src-tauri/src/desktop.rs', 'utf8');
  const sources = new Map([['', desktop]]);
  for (const [, module] of desktop.matchAll(/^mod ([a-z_]+);\r?$/gm)) {
    sources.set(module, readFileSync(`src-tauri/src/desktop/${module}.rs`, 'utf8'));
  }
  const handler = desktop.match(/tauri::generate_handler!\[([\s\S]*?)\]/);
  assert.ok(handler, '缺少桌面命令注册');
  const registered = handler[1].split(',').map((entry) => entry.trim()).filter(Boolean);
  const commandNames = registered.map((entry) => entry.split('::').at(-1));
  assert.equal(new Set(commandNames).size, commandNames.length, '命令注册重复');
  assert.deepEqual(commandNames.toSorted(), Object.keys(definition.commands).toSorted(), '注册命令与单一来源漂移');
  const declared = [...sources].flatMap(([module, source]) => [...source.matchAll(/#\[tauri::command\]\s*(?:pub\(crate\) )?(?:async )?fn ([a-z_]+)\(/g)]
    .map(([, name]) => module ? `${module}::${name}` : name));
  assert.deepEqual(declared.toSorted(), registered.toSorted(), '原生命令声明与实际注册漂移');
  for (const [name, fields] of Object.entries(definition.commands)) {
    const entry = registered.find((entry) => entry.split('::').at(-1) === name);
    const module = entry.includes('::') ? entry.slice(0, entry.lastIndexOf('::')) : '';
    assert.ok(sources.has(module), `未声明的命令模块：${entry}`);
    const source = sources.get(module);
    const response = definition.commandResponses?.[name] ?? { rust: 'AppSnapshot', error: 'String' };
    const result = `Result<${response.rust}, ${response.error}>`.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    const signature = source.match(new RegExp(`#\\[tauri::command\\]\\s*(?:pub\\(crate\\) )?(?:async )?fn ${name}\\(([^)]*)\\)\\s*->\\s*${result}`));
    assert.ok(signature, `未注册/契约返回值错误：${name}`);
    for (const [field, type] of Object.entries(fields)) {
      const rustName = field.replace(/[A-Z]/g, (char) => `_${char.toLowerCase()}`);
      const baseType = type.replace(/\?$/, '');
      const mapped = { string: 'String', bool: 'bool' }[baseType] ?? baseType;
      const rustType = type.endsWith('?') ? `Option<${mapped}>` : mapped;
      assert.ok(signature[1].includes(`${rustName}: ${rustType}`), `${name}.${field} 参数漂移`);
    }
  }
});

test('二进制帧返回类型和结构化版本错误不退化为快照或图像 JSON', () => {
  const definition = JSON.parse(readFileSync('protocol/model.json', 'utf8'));
  assert.deepEqual(definition.commandResponses.read_preview_frame, {
    rust: 'tauri::ipc::Response', typescript: 'ArrayBuffer', error: 'PreviewProtocolError',
  });
  assert.equal(definition.structs.BinaryFrameMetadata.protocolVersion, 'u32');
  assert.equal(definition.structs.BinaryFrameMetadata.payloadLength, 'u32');
  assert.equal(definition.structs.BinaryFrameMetadata.pngBase64, undefined);
  const ts = readFileSync('src/ipc/model.ts', 'utf8');
  assert.match(ts, /read_preview_frame: ArrayBuffer;/);
  assert.equal(definition.structs.PreviewProtocolError.expectedProtocolVersion, 'u32');
});
