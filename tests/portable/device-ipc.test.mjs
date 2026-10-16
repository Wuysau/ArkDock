import assert from 'node:assert/strict';
import test from 'node:test';
import { readDeviceFrame, request, transfer } from '../../src/ipc/client.ts';

test('device IPC works without a Previewer session but preserves all other scope gates', async (t) => {
  const previous = globalThis.window, calls = [];
  globalThis.window = { __TAURI__: { core: { invoke: async (command, args) => { calls.push({ command, args }); return command === 'read_device_frame' ? new ArrayBuffer(0) : {}; } } } };
  t.after(() => { globalThis.window = previous; });
  const scope = { targetId: 'editor', sessionId: null }, identity = { runId: 'run', workspaceId: 'workspace', targetId: 'phone', generation: 1 };
  await transfer('device_runtime_state', {}, scope);
  await transfer('device_runtime_action', { request: { identity } }, scope);
  assert.equal((await readDeviceFrame(scope, identity, 0)).byteLength, 0);
  assert.equal(calls.length, 3); assert.deepEqual(calls[2].args, { scope, identity, afterSequence: 0 });
  await assert.rejects(transfer('preview_control_state', {}, scope), /原生预览会话/);
  await assert.rejects(transfer('device_runtime_state', {}, { targetId: null, sessionId: null }), /工程目标/);
  await assert.rejects(readDeviceFrame(scope, identity, -1), /序号/);
  await assert.rejects(request('read_device_frame', { identity, afterSequence: 0 }, scope), /readDeviceFrame/);
  assert.equal(calls.length, 3);
});

test('device binary reader rejects data URLs and JSON in place of actual bytes', async (t) => {
  const previous = globalThis.window;
  globalThis.window = { __TAURI__: { core: { invoke: async () => 'data:image/png;base64,fixture' } } };
  t.after(() => { globalThis.window = previous; });
  await assert.rejects(readDeviceFrame({ targetId: 'editor', sessionId: null }, { runId: 'run', workspaceId: 'w', targetId: 'phone', generation: 1 }, 0), /响应类型错误/);
});
