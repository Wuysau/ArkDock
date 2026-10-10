import assert from 'node:assert/strict';
import test from 'node:test';
import { readPreviewFrame, request } from '../../src/ipc/client.ts';

// Boundary-only test doubles. These do not execute or prove native ArkUI rendering.
async function withDesktop(invoke, execute) {
  const previous = globalThis.window;
  globalThis.window = { __TAURI__: { core: { invoke } } };
  try { await execute(); }
  finally {
    if (previous === undefined) delete globalThis.window;
    else globalThis.window = previous;
  }
}

test('二进制 IPC helper 保留 ArrayBuffer 和当前 scope，不转成 JSON 画面', async () => {
  const scope = { targetId: 'target', sessionId: 'session' };
  const packet = new Uint8Array([0, 0, 0, 0, 255, 216, 255, 217]).buffer;
  await withDesktop(async (command, args) => {
    assert.equal(command, 'read_preview_frame');
    assert.deepEqual(args, { scope, protocolVersion: 1, consumerId: 'consumer' });
    return packet;
  }, async () => assert.equal(await readPreviewFrame(scope, 1, 'consumer'), packet));
});

test('快照 helper 拒绝把二进制读取误认成 AppSnapshot', async () => {
  await withDesktop(async () => assert.fail('Raw command must not enter snapshot invoke'), async () => {
    await assert.rejects(request('read_preview_frame', { protocolVersion: 1, consumerId: 'consumer' }, { targetId: null, sessionId: 's' }), /readPreviewFrame/);
  });
});

test('二进制 IPC 拒绝 JSON 载荷并保留结构化版本错误', async () => {
  const scope = { targetId: null, sessionId: 's' };
  await withDesktop(async () => ({ pngBase64: 'test-only' }), async () => {
    await assert.rejects(readPreviewFrame(scope, 1, 'consumer'), /响应类型错误/);
  });
  const versionError = { code: 'PROTOCOL_VERSION_UNSUPPORTED', message: 'unsupported', expectedProtocolVersion: 1, receivedProtocolVersion: 2 };
  await withDesktop(async () => { throw versionError; }, async () => {
    await assert.rejects(readPreviewFrame(scope, 2, 'consumer'), (error) => error === versionError);
  });
});
