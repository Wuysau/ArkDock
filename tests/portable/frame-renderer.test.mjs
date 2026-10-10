import assert from 'node:assert/strict';
import test from 'node:test';
import { parseBinaryFrame, MAX_JPEG_BYTES } from '../../src/preview/frame-decoder.ts';
import { FramePresenter } from '../../src/preview/frame-presenter.ts';
import { BinaryFrameController } from '../../src/preview/BinaryFrameController.ts';
import { createFrameStore } from '../../src/frame-store.ts';

// Contract fixtures and fake clocks are not evidence of ArkUI/native rendering.
const state = () => ({ revision: 1, phase: 'running', trusted: true, sessionId: 's',
  target: { id: 't', process: { pid: 1, startedAt: 'start' } }, project: { id: 'w', environment: 'windowsLocal', workspaceRoot: 'D:/w', appRoot: 'D:/w', module: 'entry', variant: 'default' },
  backend: { available: true, canClick: true }, build: { generation: 1, buildId: 'b', durationMs: 1, error: null } });
const meta = () => ({ protocolVersion: 1, workspaceId: 'w', sessionId: 's', buildId: 'b', generation: 1, sequence: 1, width: 360, height: 640, rotation: 0, capturedAt: 1, format: 'jpeg', payloadLength: 15 });
const jpeg = new Uint8Array([255, 216, 255, 192, 0, 9, 8, 2, 128, 1, 104, 1, 0, 255, 217]);
function packet(overrides = {}, bytes = jpeg) {
  const json = new TextEncoder().encode(JSON.stringify({ ...meta(), payloadLength: bytes.length, ...overrides }));
  const result = new Uint8Array(4 + json.length + bytes.length);
  new DataView(result.buffer).setUint32(0, json.length, true); result.set(json, 4); result.set(bytes, 4 + json.length);
  return result.buffer;
}
const legacy = (sequence = 1) => ({ sessionId: 's', buildId: 'b', generation: 1, sequence, width: 360, height: 640, rotation: 0, capturedAt: 1, pngBase64: 'fixture-only' });
const tick = async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; };

test('二进制解析在解码前验证 LE 封包、载荷、UTF-8、JPEG 内尺寸和完整构建身份', () => {
  const frame = parseBinaryFrame(packet(), state(), null, 50);
  assert.equal(frame.receivedAt, 50); assert.deepEqual(new Uint8Array(frame.jpeg), jpeg);
  assert.equal(parseBinaryFrame(new ArrayBuffer(0), state()), null);
  for (const override of [{ workspaceId: 'other' }, { sessionId: 'other' }, { buildId: 'other' }, { generation: 2 }]) assert.equal(parseBinaryFrame(packet(override), state()), null);
  assert.equal(parseBinaryFrame(packet(), state(), frame), null);
  for (const override of [{ protocolVersion: 2 }, { format: 'png' }, { width: 8193 }, { width: 8192, height: 8192 }, { width: 359 }, { rotation: 90 }, { sequence: 0 }, { generation: 1.5 }, { capturedAt: -1 }, { payloadLength: 3 }, { workspaceId: '字'.repeat(86) }]) assert.throws(() => parseBinaryFrame(packet(override), state()), /FRAME_/);
  const wrongEndian = packet(); new DataView(wrongEndian).setUint32(0, 200, false); assert.throws(() => parseBinaryFrame(wrongEndian, state()), /METADATA_LIMIT/);
  const invalidUtf8 = packet(); new Uint8Array(invalidUtf8)[4] = 255; assert.throws(() => parseBinaryFrame(invalidUtf8, state()));
  assert.throws(() => parseBinaryFrame(new ArrayBuffer(4 + 4096 + MAX_JPEG_BYTES + 1), state()), /PACKET_LIMIT/);
  const dimensions = jpeg.slice(); dimensions[9] = 16; assert.throws(() => parseBinaryFrame(packet({}, dimensions), state()), /DIMENSIONS/);
});

test('解码中的旧帧不覆盖新帧；一张解码、一张待处理、关闭旧 bitmap', async () => {
  const pending = [], drawn = [], closed = [];
  let current;
  const presenter = new FramePresenter({ decode: (frame) => { const result = deferred(); pending.push({ frame, ...result }); return result.promise; }, current: (frame) => frame === current,
    draw: (_image, frame) => drawn.push(frame.sequence), clear: () => {}, now: () => 1 });
  const image = (id) => ({ width: 360, height: 640, close: () => closed.push(id) });
  current = legacy(1); presenter.submit(current);
  current = legacy(2); presenter.submit(current);
  current = legacy(3); presenter.submit(current);
  assert.equal(pending.length, 1);
  pending[0].resolve(image(1)); await tick();
  assert.deepEqual(closed, [1]); assert.equal(pending.length, 2); assert.equal(pending[1].frame.sequence, 3);
  pending[1].resolve(image(3)); await tick(); assert.deepEqual(drawn, [3]);
  current = legacy(4); presenter.submit(current); assert.deepEqual(closed, [1], '上一帧保留到新帧完成解码');
  pending[2].resolve(image(4)); await tick(); assert.deepEqual(drawn, [3, 4]); assert.deepEqual(closed, [1, 3]);
  presenter.replay(); assert.deepEqual(drawn, [3, 4, 4]); presenter.dispose(); assert.deepEqual(closed, [1, 3, 4]);
});

test('停止、切会话、隐藏和卸载期间完成的解码均回收，不恢复过期画面', async () => {
  for (const action of ['clear', 'pause', 'dispose']) {
    const decoded = deferred(); let drawn = 0, closed = 0;
    const presenter = new FramePresenter({ decode: () => decoded.promise, current: () => true, draw: () => drawn++, clear: () => {}, now: () => 1 });
    presenter.submit(legacy());
    if (action === 'clear') presenter.submit(null); else if (action === 'pause') presenter.setPaused(true); else presenter.dispose();
    decoded.resolve({ width: 360, height: 640, close: () => closed++ }); await tick();
    assert.equal(drawn, 0); assert.equal(closed, 1); presenter.dispose();
  }
});

function harness(bridge) {
  let current = state(), timerId = 0;
  const timers = new Map();
  const store = createFrameStore({ requestFrame: () => 1, cancelFrame: () => {} });
  const controller = new BinaryFrameController({ bridge, store, getState: () => current, consumerId: 'test-owner', clock: { schedule: (callback, delay) => { timers.set(++timerId, { callback, delay }); return timerId; }, cancel: (id) => timers.delete(id) } });
  return { controller, store, timers, update: (next) => { current = next; store.sync(next); controller.update(next); }, fire: async () => { const [id, timer] = timers.entries().next().value; timers.delete(id); timer.callback(); await tick(); return timer.delay; } };
}

test('启用回执之前不读取也不接收兼容帧；空轮询自适应且不超过 30Hz', async () => {
  const ack = deferred(); let reads = 0;
  const h = harness({ request: (command) => command === 'enable_binary_preview_frames' ? ack.promise : Promise.resolve(state()), readFrame: async () => { reads++; return new ArrayBuffer(0); } });
  h.update(state()); h.controller.acceptLegacy(legacy()); assert.equal(h.store.getSnapshot(), null); assert.equal(reads, 0); assert.equal(h.timers.size, 0);
  ack.resolve(state()); await tick(); assert.equal(h.controller.getMode(), 'binary');
  const delays = []; for (let i = 0; i < 8; i++) delays.push(await h.fire());
  assert.equal(reads, 8); assert.ok(delays.every((delay) => delay >= 1000 / 30 && delay <= 250)); assert.equal(delays.at(-1), 250);
  h.controller.setVisible(false); assert.equal(h.timers.size, 0); h.controller.setVisible(true); assert.equal(h.timers.size, 1);
  h.controller.dispose(); assert.equal(h.timers.size, 0);
});

test('兼容路径仅在明确不支持协商后启用，一般错误保留 BLOCKED', async () => {
  for (const [error, mode] of [[{ code: 'PROTOCOL_VERSION_UNSUPPORTED' }, 'legacy'], [new Error('STALE_SCOPE'), 'blocked']]) {
    const h = harness({ request: async () => { throw error; }, readFrame: async () => assert.fail('未确认二进制协商不能读取') });
    h.update(state()); await tick(); assert.equal(h.controller.getMode(), mode);
    h.controller.acceptLegacy(legacy()); assert.equal(Boolean(h.store.getSnapshot()), mode === 'legacy'); assert.equal(h.timers.size, 0); h.controller.dispose();
  }
});

test('迟到协商清理携带原 consumerId，旧读取和旧截图不能进入新会话', async () => {
  const first = deferred(), second = deferred(), calls = [];
  const h = harness({ request: (command, args, scope) => { calls.push({ command, args, scope }); if (command === 'disable_binary_preview_frames') return Promise.resolve(state()); return calls.filter((c) => c.command === 'enable_binary_preview_frames').length === 1 ? first.promise : second.promise; }, readFrame: async () => packet() });
  h.update(state()); h.update({ ...state(), revision: 2, sessionId: 'new' });
  second.resolve({ ...state(), sessionId: 'new' }); await tick(); first.resolve(state()); await tick();
  const enabled = calls.filter((c) => c.command === 'enable_binary_preview_frames'), disabled = calls.find((c) => c.command === 'disable_binary_preview_frames');
  assert.notEqual(enabled[0].args.consumerId, enabled[1].args.consumerId); assert.equal(disabled.args.consumerId, enabled[0].args.consumerId); assert.equal(disabled.scope.sessionId, 's');
  await h.fire(); assert.equal(h.store.getSnapshot(), null); assert.equal(h.controller.metrics.discarded, 1);
  h.controller.dispose();
});

test('选择上下文与点击只使用已绘制帧，后续接收不冒充解码完成', () => {
  const store = createFrameStore({ requestFrame: () => 1, cancelFrame: () => {} });
  const first = legacy(1), second = legacy(2);
  store.push(state(), first); assert.equal(store.getDisplayed(), null); assert.equal(store.markDisplayed(first), true);
  store.push(state(), second); assert.equal(store.getDisplayed(), first); assert.equal(store.markDisplayed(first), false); assert.equal(store.markDisplayed(second), true);
  store.sync({ ...state(), revision: 2, phase: 'building', build: { generation: 2, buildId: 'new', durationMs: null } });
  assert.equal(store.getDisplayed(), second, '重建保留真实上次成功画面'); assert.equal(store.isPresentable(second), false);
  store.sync({ ...state(), revision: 3, phase: 'stopped' }); assert.equal(store.getDisplayed(), null); store.dispose();
});

test('默认时钟不把 WebView2 clearTimeout 绑定到 clock 对象，点击 wake 与清理均可执行', async () => {
  const originalSet = globalThis.setTimeout, originalClear = globalThis.clearTimeout;
  const cleared = []; let id = 0;
  globalThis.setTimeout = () => ++id;
  globalThis.clearTimeout = function (handle) {
    // WebIDL accepts a global invocation; calling clock.cancel directly supplies an invalid receiver.
    assert.ok(this === undefined || this === globalThis, 'WebView2: Illegal invocation'); cleared.push(handle);
  };
  let controller;
  try {
    const store = createFrameStore({ requestFrame: () => 1, cancelFrame: () => {} });
    controller = new BinaryFrameController({ bridge: { request: async () => state(), readFrame: async () => new ArrayBuffer(0) }, store, getState: state, consumerId: 'global-clock' });
    controller.update(state()); await tick(); assert.equal(id, 1);
    controller.wake(); assert.deepEqual(cleared, [1]); assert.equal(id, 2);
    controller.dispose(); assert.deepEqual(cleared, [1, 2]); store.dispose();
  } finally { controller?.dispose(); globalThis.setTimeout = originalSet; globalThis.clearTimeout = originalClear; }
});

test('同会话短暂不可用后恢复，每个 lease 唯一；迟到 disable 不取消后继读取者', async () => {
  const delayedDisable = deferred(), calls = [];
  let owner = null;
  const h = harness({ request: async (command, args) => {
    calls.push({ command, consumerId: args.consumerId });
    if (command === 'enable_binary_preview_frames') owner = args.consumerId;
    else { await delayedDisable.promise; if (owner === args.consumerId) owner = null; }
    return state();
  }, readFrame: async (_scope, _protocol, consumerId) => { assert.equal(owner, consumerId); return packet(); } });
  h.update(state()); await tick(); const previousOwner = owner;
  h.update({ ...state(), revision: 2, backend: { available: false } });
  h.update({ ...state(), revision: 3 }); await tick(); const successorOwner = owner;
  assert.notEqual(successorOwner, previousOwner);
  delayedDisable.resolve(); await tick(); assert.equal(owner, successorOwner);
  assert.equal(calls.filter((call) => call.command === 'enable_binary_preview_frames').length, 2);
  await h.fire(); assert.equal(h.store.getSnapshot().sequence, 1); h.controller.dispose(); await tick();
});

test('同一正在解码帧去重，原生隐藏暂停 replay，恢复重绘保留 bitmap', async () => {
  const completion=deferred();let decodes=0,draws=0,closed=0;
  const presenter=new FramePresenter({decode:()=>{decodes++;return completion.promise;},current:()=>true,draw:()=>draws++,clear:()=>{},now:()=>1});
  const frame=legacy();presenter.submit(frame);presenter.submit(frame);completion.resolve({width:360,height:640,close:()=>closed++});await tick();
  assert.equal(decodes,1);assert.equal(draws,1);
  presenter.setPaused(true);presenter.replay();assert.equal(draws,1);assert.equal(closed,0);
  presenter.submit(frame);presenter.setPaused(false);presenter.replay();assert.equal(draws,2);assert.equal(decodes,1);
  presenter.dispose();assert.equal(closed,1);
});
