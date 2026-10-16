import assert from 'node:assert/strict';
import test from 'node:test';
import { observePreviewPerformance, summarizeDurations } from '../../src/preview/performance-metrics.ts';

test('耗时百分位只取最近 120 个有效样本，不把空集伪装成零延迟', () => {
  assert.deepEqual(summarizeDurations([]), { count: 0, p50: null, p95: null });
  assert.deepEqual(summarizeDurations([0, 1, 5, 10, NaN, -1, Infinity]), { count: 4, p50: 1, p95: 10 });
  const samples = Array.from({ length: 200 }, (_, index) => index + 1);
  assert.deepEqual(summarizeDurations(samples), { count: 120, p50: 140, p95: 194 });
  assert.equal(samples[0], 1, 'percentiles do not reorder the live ring');
});

test('逐帧通知不触发面板更新，每秒只发布一次实际新帧绘制速率，结束即退订', () => {
  let frame = { sequence: 1 }, now = 0, listener, cancelled = 0, off = 0;
  const timers = [], samples = [];
  const input = { receiptDurationMs: [2, 5], dispatched: 3, coalescedMoves: 9 };
  let transport = { metrics: { discarded: 3, rejected: 2 }, readDurationMs: [1, 4], getMode: () => 'binary' };
  const stop = observePreviewPerformance({ store: { getDisplayed: () => frame, subscribeDisplayed: (next) => { listener = next; return () => off++; } },
    getInput: () => input, getTransport: () => transport, sample: (value) => samples.push(value),
    clock: { now: () => now, schedule: (callback, delay) => { assert.equal(delay, 1000); timers.push(callback); return timers.length; }, cancel: () => cancelled++ } });
  assert.equal(samples.length, 1); assert.equal(samples[0].presentedFps, null, 'opening panel does not count the retained image');
  for (let sequence = 2; sequence <= 61; sequence++) { frame = { sequence }; listener(); listener(); }
  assert.equal(samples.length, 1, '60 frame notifications do not render React 60 times');
  now = 1000; timers[0]();
  assert.equal(samples.length, 2); assert.equal(samples[1].presentedFps, 60);
  assert.equal(samples[1].discardedReads, 5); assert.equal(samples[1].coalescedMoves, 9);
  now = 3000; timers[1](); assert.equal(samples[2].presentedFps, 0, 'static page has no new frames');
  frame = null; listener(); transport = null; now = 4000; timers[2]();
  assert.equal(samples[3].presentedFps, null); assert.equal(samples[3].binaryRead.count, 0);
  const before = samples.length; stop(); frame = { sequence: 100 }; listener(); timers[3]();
  assert.equal(samples.length, before); assert.equal(off, 1); assert.equal(cancelled, 1);
});

test('延迟计时器按实际间隔计算帧率，恢复采样不重放隐藏期间的帧', () => {
  let now = 0, frame = { sequence: 1 }, listener, timer;
  const samples = [];
  const options = { store: { getDisplayed: () => frame, subscribeDisplayed: (next) => { listener = next; return () => {}; } },
    getInput: () => null, getTransport: () => null, sample: (value) => samples.push(value),
    clock: { now: () => now, schedule: (callback) => { timer = callback; return 1; }, cancel: () => {} } };
  let stop = observePreviewPerformance(options);
  frame = { sequence: 2 }; listener(); frame = { sequence: 3 }; listener();
  now = 2000; timer(); assert.equal(samples.at(-1).presentedFps, 1);
  stop(); frame = { sequence: 50 }; now = 9000;
  stop = observePreviewPerformance(options); assert.equal(samples.at(-1).presentedFps, null);
  now = 10_000; timer(); assert.equal(samples.at(-1).presentedFps, 0); stop();
});
