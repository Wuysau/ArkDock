import assert from 'node:assert/strict';
import test from 'node:test';
import { DeviceFrameController, DEVICE_FRAME_INTERVAL_MS, MAX_DEVICE_PNG_BYTES, deviceActionMayCapture, deviceCaptureCleanupScope, deviceFrameInputReady, deviceGestureFrameMatches, devicePoint, parseDeviceFrame } from '../../src/preview/DeviceFrameController.ts';

// Synthetic contract packets and fake bitmap decoders are not device evidence.
const png = Uint8Array.from(Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=', 'base64'));
const identity = () => ({ runId: 'run', workspaceId: 'workspace', targetId: 'phone', generation: 1 });
const metadata = (fields = {}) => ({ runId: 'run', targetId: 'phone', bundleName: 'app.test', sequence: 1, width: 1, height: 1,
  captureStartedAt: 100, receivedAt: 120, payloadLength: png.length, foregroundVerified: true, ...fields });
const state = (fields = {}) => ({ identity: identity(), selectedTargetId: 'phone', bundleName: 'app.test', frameReady: true,
  foregroundVerified: true, frame: metadata(), streaming: true, busy: false, ...fields });
const scope = { targetId: 'editor', sessionId: null };
function packet(fields = {}, payload = png) {
  const json = new TextEncoder().encode(JSON.stringify(metadata({ payloadLength: payload.length, ...fields })));
  const bytes = new Uint8Array(16 + json.length + payload.length), view = new DataView(bytes.buffer);
  bytes.set([65, 68, 68, 70]); view.setUint32(4, 1, true); view.setUint32(8, json.length, true); view.setUint32(12, payload.length, true);
  bytes.set(json, 16); bytes.set(payload, 16 + json.length); return bytes.buffer;
}
const tick = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };

test('device binary frames validate independent ADDF framing, metadata and PNG before allocation', () => {
  assert.deepEqual(new Uint8Array(parseDeviceFrame(packet(), identity(), 'app.test').png), png);
  assert.equal(parseDeviceFrame(new ArrayBuffer(0), identity(), 'app.test'), null);
  assert.equal(parseDeviceFrame(packet({ runId: 'old' }), identity(), 'app.test'), null);
  assert.equal(parseDeviceFrame(packet({ targetId: 'other' }), identity(), 'app.test'), null);
  assert.equal(parseDeviceFrame(packet({ bundleName: 'other.app' }), identity(), 'app.test'), null);
  assert.equal(parseDeviceFrame(packet(), identity(), 'app.test', 1), null);
  for (const fields of [{ width: 5000 }, { height: 0 }, { width: 4096, height: 4096 }, { sequence: 1.2 }, { receivedAt: 99 },
    { payloadLength: 2 }, { foregroundVerified: 'true' }, { runId: 'x'.repeat(257) }, { width: 2 }]) {
    assert.throws(() => parseDeviceFrame(packet(fields), identity(), 'app.test'), /DEVICE_FRAME_/);
  }
  const badMagic = packet(); new Uint8Array(badMagic)[0] = 0;
  assert.throws(() => parseDeviceFrame(badMagic, identity(), 'app.test'), /PROTOCOL/);
  const badVersion = packet(); new DataView(badVersion).setUint32(4, 2, true);
  assert.throws(() => parseDeviceFrame(badVersion, identity(), 'app.test'), /PROTOCOL/);
  const badUtf8 = packet(); new Uint8Array(badUtf8)[16] = 255;
  assert.throws(() => parseDeviceFrame(badUtf8, identity(), 'app.test'));
  assert.throws(() => parseDeviceFrame(new ArrayBuffer(MAX_DEVICE_PNG_BYTES + 5000), identity(), 'app.test'), /LIMIT/);
  assert.throws(() => parseDeviceFrame(packet({}, png.slice(0, -1)), identity(), 'app.test'), /TRUNCATED/);
  assert.throws(() => parseDeviceFrame(packet({}, Uint8Array.from([...png, 0])), identity(), 'app.test'), /TRUNCATED/);
});

function harness(read, decode) {
  let time = 0, nextTimer = 0;
  const timers = new Map(), drawn = [], closed = [], errors = [];
  let cleared = 0;
  const controller = new DeviceFrameController({ read,
    decode: decode ?? (async (frame) => ({ width: 1, height: 1, close: () => closed.push(frame.metadata.sequence) })),
    draw: (_bitmap, frame) => drawn.push(frame.sequence), clear: () => cleared++, error: (error) => errors.push(error),
    clock: { now: () => time, schedule: (callback, delay) => { timers.set(++nextTimer, { callback, delay }); return nextTimer; }, cancel: (id) => timers.delete(id) } });
  return { controller, timers, drawn, closed, errors, cleared: () => cleared,
    update: (value = state(), enabled = true) => controller.update(value, scope, enabled),
    fire: async () => { const [id, timer] = timers.entries().next().value; timers.delete(id); time += timer.delay; timer.callback(); await tick(); return timer.delay; } };
}

test('device controller keeps one reader/decoder and coalesces notifications to newest sequence at <=10Hz', async () => {
  const decode = deferred(); let reads = 0, decodes = 0;
  const h = harness(async () => packet({ sequence: ++reads === 1 ? 1 : 100 }), async (frame) => {
    if (++decodes === 1) return decode.promise;
    return { width: 1, height: 1, close: () => h.closed.push(frame.metadata.sequence) };
  });
  h.update(); assert.equal(await h.fire(), 0); assert.equal(reads, 1); assert.equal(decodes, 1);
  for (let sequence = 2; sequence <= 100; sequence++) h.update(state({ frame: metadata({ sequence }) }));
  assert.equal(reads, 1); assert.equal(h.timers.size, 0);
  decode.resolve({ width: 1, height: 1, close: () => h.closed.push(1) }); await tick();
  assert.deepEqual(h.drawn, [1]); assert.deepEqual(h.closed, [1]); assert.equal(h.timers.size, 1);
  assert.equal(await h.fire(), DEVICE_FRAME_INTERVAL_MS);
  assert.deepEqual(h.drawn, [1, 100]); assert.deepEqual(h.closed, [1, 100]);
  for (let i = 0; i < 20; i++) h.update(state({ frame: metadata({ sequence: 100 }) }));
  assert.equal(h.timers.size, 0, 'an unchanged image is not polled again');
  h.controller.dispose();
});

test('continuous faster frame production cannot starve a slower decoder and stale displayed frames never enable input', async () => {
  let sequence = 1;
  const decoders = [];
  const h = harness(async () => packet({ sequence }), () => { const result = deferred(); decoders.push(result); return result.promise; });
  h.update();
  for (let completed = 1; completed <= 5; completed++) {
    await h.fire();
    sequence++;
    h.update(state({ frame: metadata({ sequence }) }));
    decoders[completed - 1].resolve({ width: 1, height: 1, close: () => h.closed.push(completed) });
    await tick();
    assert.equal(h.drawn.at(-1), completed, 'each completed same-epoch decode makes visual progress');
    assert.equal(deviceFrameInputReady(state({ frame: metadata({ sequence }) }), metadata({ sequence: completed }), 121), false);
  }
  assert.deepEqual(h.drawn, [1, 2, 3, 4, 5]);
  assert.deepEqual(h.closed, [1, 2, 3, 4, 5]);
  assert.equal(h.timers.size, 1);
  h.controller.dispose();
});

test('a click or drag begun before a screenshot change is cancelled instead of relabelled with the new frame', () => {
  const down = metadata({ sequence: 7, width: 360, height: 640 });
  const completed = [];
  const release = (displayed) => { if (deviceGestureFrameMatches(down, displayed)) completed.push(displayed.sequence); };
  release(metadata({ sequence: 8, width: 360, height: 640 }));
  release({ ...down, runId: 'replacement-run' });
  release({ ...down, targetId: 'other-phone' });
  release({ ...down, bundleName: 'other.app' });
  release({ ...down, width: 640, height: 360 });
  release(null);
  assert.deepEqual(completed, [], 'old gesture coordinates never acquire a newer frame identity');
  release({ ...down });
  assert.deepEqual(completed, [7]);
});

test('revoking capture interrupts pending start and input before a streaming receipt exists', () => {
  for (const action of ['capture', 'streamStart', 'start', 'input']) {
    const capturePending = deviceActionMayCapture(action, true);
    const streaming = false, requests = [];
    if (streaming || capturePending) requests.push('streamStop');
    assert.deepEqual(requests, ['streamStop'], `${action} follow-up capture is covered by revocation`);
    assert.equal(deviceActionMayCapture(action, false), false);
  }
  for (const action of ['build', 'deploy', 'discover', 'select', 'stop', 'streamStop', 'cancel', 'disconnect']) {
    assert.equal(deviceActionMayCapture(action, true), false, action);
  }
});

test('capture cleanup follows Previewer session changes but cannot cross project or target ownership', () => {
  const owner = { targetId: 'editor', workspaceId: 'workspace' };
  assert.deepEqual(deviceCaptureCleanupScope(owner, { ...owner, sessionId: 'new-preview-session' }), { targetId: 'editor', sessionId: 'new-preview-session' });
  assert.deepEqual(deviceCaptureCleanupScope(owner, { ...owner, sessionId: null }), { targetId: 'editor', sessionId: null });
  assert.equal(deviceCaptureCleanupScope(owner, { ...owner, targetId: 'other-editor', sessionId: 'new-session' }), null);
  assert.equal(deviceCaptureCleanupScope(owner, { ...owner, workspaceId: 'other-project', sessionId: 'new-session' }), null);
  assert.equal(deviceCaptureCleanupScope(owner, { targetId: null, workspaceId: null, sessionId: null }), null);
});

test('a late device packet cannot cross run generation or target changes', async () => {
  const response = deferred(); let reads = 0, decodes = 0;
  const h = harness(async () => ++reads === 1 ? response.promise : packet({ targetId: 'second' }), async () => {
    decodes++; return { width: 1, height: 1, close: () => {} };
  });
  h.update(); await h.fire();
  h.update(state({ identity: { ...identity(), generation: 2, targetId: 'second' }, selectedTargetId: 'second', frame: metadata({ targetId: 'second' }) }));
  assert.equal(h.timers.size, 0, 'new runs still wait for the previous single reader');
  response.resolve(packet()); await tick(); assert.equal(decodes, 0);
  await h.fire(); assert.equal(decodes, 1); assert.deepEqual(h.drawn, [1]);
  h.controller.dispose();
});

test('hide, consent revocation, missing frame and disposal release delayed bitmaps without drawing', async () => {
  for (const operation of ['hide', 'revoke', 'missing', 'dispose']) {
    const decode = deferred(); let closed = 0;
    const h = harness(async () => packet(), () => decode.promise);
    h.update(); await h.fire();
    if (operation === 'hide') h.controller.setVisible(false);
    if (operation === 'revoke') h.update(state(), false);
    if (operation === 'missing') h.update(state({ frame: null, frameReady: false }));
    if (operation === 'dispose') h.controller.dispose();
    decode.resolve({ width: 1, height: 1, close: () => closed++ }); await tick();
    assert.deepEqual(h.drawn, [], operation); assert.equal(closed, 1, operation); assert.equal(h.timers.size, 0, operation);
    h.controller.dispose();
  }
});

test('bad decoder dimensions clear the display and cannot enable input', async () => {
  const h = harness(async () => packet(), async () => ({ width: 2, height: 1, close: () => h.closed.push(1) }));
  h.update(); await h.fire();
  assert.deepEqual(h.drawn, []); assert.deepEqual(h.closed, [1]); assert.equal(h.errors.length, 1);
  assert.match(h.errors[0].message, /DECODE_DIMENSIONS/); assert.ok(h.cleared() >= 2);
  h.controller.dispose();
});

test('input requires current foreground, exact displayed sequence and fresh capture time', () => {
  assert.equal(deviceFrameInputReady(state(), metadata(), 121), true);
  assert.equal(deviceFrameInputReady(state(), metadata(), 6000), false);
  assert.equal(deviceFrameInputReady(state({ frame: metadata({ receivedAt: 6000 }) }), metadata({ receivedAt: 6000 }), 6001), false, 'a slow capture is stale even when just received');
  assert.equal(deviceFrameInputReady(state(), metadata(), 119), false);
  assert.equal(deviceFrameInputReady(state({ foregroundVerified: false }), metadata(), 121), false);
  assert.equal(deviceFrameInputReady(state({ frame: metadata({ sequence: 2 }) }), metadata(), 121), false);
  assert.equal(deviceFrameInputReady(state(), metadata({ targetId: 'other' }), 121), false);
  assert.equal(deviceFrameInputReady(state({ frameReady: false }), metadata(), 121), false);
});

test('contain mapping rejects bars, out-of-range and unsupported zero coordinates', () => {
  const frame = metadata({ width: 360, height: 640 }), bounds = { left: 10, top: 20, width: 400, height: 640 };
  assert.deepEqual(devicePoint(210, 340, bounds, frame), { x: 180, y: 320 });
  assert.equal(devicePoint(11, 300, bounds, frame), null);
  assert.equal(devicePoint(30, 340, bounds, frame), null);
  assert.equal(devicePoint(500, 340, bounds, frame), null);
  assert.equal(devicePoint(NaN, 340, bounds, frame), null);
});
