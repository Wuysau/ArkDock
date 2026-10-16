import assert from 'node:assert/strict';
import test from 'node:test';
import { ControlReceiptInbox } from '../../src/preview/ControlReceiptInbox.ts';
import { NativeInputController } from '../../src/preview/NativeInputController.ts';

const identity = { workspaceId: 'w', sessionId: 's', buildId: 'b', generation: 1, width: 360, height: 640 };
const scope = { targetId: 't', sessionId: 's' };
const receipt = (operationId = 'op', overrides = {}) => ({ operationId, identity, phase: 'queued', observedAt: 1, ...overrides });
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };
const tick = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };

test('ACK 早于提交回复仍可消费；按操作和完整身份关联且缓存有界', async () => {
  let push, removed = 0;
  const inbox = new ControlReceiptInbox(async (listener) => { push = listener; return () => removed++; });
  await inbox.connect();
  const accepted = receipt('op', { phase: 'commandAccepted', observedAt: 2 });
  push(accepted); assert.equal(inbox.latest(receipt()), accepted);
  const stale = receipt('op', { identity: { ...identity, generation: 2 }, phase: 'failed', observedAt: 3 });
  push(stale); assert.equal(inbox.latest(receipt()), accepted);
  const next = receipt('next'), waiting = inbox.wait(next, 1000);
  push(receipt('unrelated', { phase: 'failed', observedAt: 4 }));
  const nextAck = receipt('next', { phase: 'observed', observedAt: 5 });
  push(nextAck); assert.equal(await waiting, nextAck);
  for (let i = 0; i < 200; i++) push(receipt(String(i)));
  assert.equal(inbox.size, 64);
  assert.equal(await inbox.wait(receipt('missed'), 1), null, 'missed event falls back without fabricated completion');
  const cancelledWait = inbox.wait(receipt('cancel'), 1000);
  inbox.disconnect(); assert.equal(await cancelledWait, null); assert.equal(removed, 1); assert.equal(inbox.size, 0);
});

test('旧监听器和迟到订阅不会污染重新连接的回执缓存', async () => {
  const subscribed = deferred(); let oldPush, removed = 0;
  const inbox = new ControlReceiptInbox((listener) => { oldPush = listener; return subscribed.promise; });
  const connecting = inbox.connect(); inbox.disconnect();
  oldPush(receipt()); assert.equal(inbox.size, 0);
  subscribed.resolve(() => removed++); await connecting; assert.equal(removed, 1); assert.equal(inbox.connected, false);
});

test('事件订阅无响应时有界退回状态读取，迟到订阅仍可清理', async () => {
  const subscribed = deferred(); let removed = 0;
  const inbox = new ControlReceiptInbox(() => subscribed.promise);
  await inbox.connect(); assert.equal(inbox.connected, false);
  inbox.disconnect(); subscribed.resolve(() => removed++); await tick();
  assert.equal(removed, 1);
});

test('连续移动仅保留最新位置；Down ACK 先于 Move，最终 Up 不丢失', async () => {
  const originalRaf = globalThis.requestAnimationFrame, originalCancel = globalThis.cancelAnimationFrame;
  const callbacks = new Map(); let id = 0;
  globalThis.requestAnimationFrame = (fn) => { callbacks.set(++id, fn); return id; };
  globalThis.cancelAnimationFrame = (key) => callbacks.delete(key);
  const calls = [], acks = [];
  const input = new NativeInputController((request) => { calls.push(request); const ack = deferred(); acks.push(ack); return ack.promise; },
    async () => calls.push({ action: 'release' }), (error) => assert.fail(String(error)));
  try {
    input.start(identity, scope, { x: 1, y: 1 });
    for (let x = 10; x <= 100; x++) input.move({ x, y: 1 });
    assert.deepEqual(calls.map((x) => x.action), ['pointerDown']);
    acks[0].resolve(receipt('down', { phase: 'commandAccepted' })); await tick();
    const ending = input.end({ x: 120, y: 1 }); await tick();
    assert.equal(calls.length, 2); assert.equal(calls[1].action, 'pointerMove'); assert.equal(calls[1].x, 120);
    acks[1].resolve(receipt('move', { phase: 'commandAccepted' })); await tick();
    assert.equal(calls[2].action, 'pointerUp');
    acks[2].resolve(receipt('up', { phase: 'commandAccepted' })); assert.equal(await ending, true);
    assert.equal(callbacks.size, 0);
  } finally { globalThis.requestAnimationFrame = originalRaf; globalThis.cancelAnimationFrame = originalCancel; }
});

test('取消等待在途 Down 的 ACK 后释放；不派发缓存 Move 或重复释放', async () => {
  const down = deferred(), calls = [];
  const input = new NativeInputController((request) => { calls.push(request.action); return down.promise; },
    async () => { calls.push('release'); }, (error) => assert.fail(String(error)));
  input.start(identity, scope, { x: 1, y: 1 });
  const cancelled = input.cancel(), duplicate = input.cancel();
  input.move({ x: 100, y: 100 });
  assert.deepEqual(calls, ['pointerDown']);
  down.resolve(receipt('down', { phase: 'commandAccepted' })); await Promise.all([cancelled, duplicate]);
  assert.deepEqual(calls, ['pointerDown', 'release']);
  assert.equal(await input.end({ x: 100, y: 100 }), false);
});
