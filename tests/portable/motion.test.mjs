import assert from 'node:assert/strict';
import test from 'node:test';
import { createDockRippleController } from '../../src/dock-ripple.ts';
import { createMotionPreferenceSync, motionSettings } from '../../src/motion.ts';
import { displayTransitioning, nextDisplayRequestId } from '../../src/display-state.ts';
import { createOrbAutoHidePreferenceSync, createOrbOpacityPreferenceSync, nativeOrbPreferenceChanged, parseOrbAutoHide, parseOrbOpacity, settledOrbChoice } from '../../src/orb-preferences.ts';
import { targetLifecycleKey } from '../../src/dock-ripple.ts';

const standard = { reduced: false, intensity: 'standard' };
const target = { id: 'editor', process: { pid: 24, startedAt: '2026-10-10T10:00:00Z' } };
const binding = (revision = 1, bindingId = 'binding-a', side = 'right') => ({ revision, target, dockConfirmation: { bindingId, side }, display: { mode: 'expanded' } });

test('贴边确认只播放一次；跟随、缩放、旧回复与动画结束不重播', () => {
  const controller = createDockRippleController();
  const ripple = controller.update(binding(), standard);
  assert.equal(ripple.side, 'right');
  assert.equal(ripple.waves, 2);
  assert.equal(controller.update(binding(2), standard), ripple);
  assert.equal(controller.update(binding(1, 'old-binding'), standard), ripple);
  assert.equal(controller.finish('old-binding'), ripple);
  assert.equal(controller.finish('binding-a'), null);
  assert.equal(controller.update(binding(3), standard), null);
  assert.equal(controller.update(binding(4, 'binding-b', 'left'), { ...standard, intensity: 'enhanced' }).waves, 3);
  assert.equal(controller.update(binding(5), standard), null, '已消费过的绑定 ID 不能再次触发动效');
});

test('解除、目标生命周期改变和隐藏立即清除原目标涟漪', () => {
  for (const incoming of [
    { ...binding(2), target: null, dockConfirmation: null },
    { ...binding(2), target: { ...target, process: { ...target.process, startedAt: '2026-10-10T11:00:00Z' } } },
    { ...binding(2), display: { mode: 'collapsing' } },
  ]) {
    const controller = createDockRippleController();
    assert.ok(controller.update(binding(), standard));
    assert.equal(controller.update(incoming, standard), null);
    assert.equal(controller.update(binding(1), standard), null, '旧目标回复不能恢复已取消的涟漪');
  }
});

test('减少动画、关闭、轻量取消运行中的涟漪并消费确认；恢复偏好不补播', () => {
  for (const preference of [{ reduced: true, intensity: 'enhanced' }, { reduced: false, intensity: 'off' }, { reduced: false, intensity: 'light' }]) {
    const controller = createDockRippleController();
    assert.ok(controller.update(binding(), standard));
    assert.equal(controller.update(binding(2), preference), null);
    assert.equal(controller.update(binding(3, 'quiet-binding'), preference), null);
    assert.equal(controller.update(binding(4, 'quiet-binding'), standard), null);
    assert.ok(controller.update(binding(5, 'next-binding'), standard));
  }
});

test('系统减少动画覆盖任何强度；用户减少动画不会被系统恢复覆盖', () => {
  assert.deepEqual(motionSettings('system', 'enhanced', true), { reduced: true, intensity: 'enhanced' });
  assert.deepEqual(motionSettings('reduce', 'standard', false), { reduced: true, intensity: 'standard' });
  assert.deepEqual(motionSettings('system', 'light', false), { reduced: false, intensity: 'light' });
});

const settle = () => new Promise((resolve) => setImmediate(resolve));
const job = (preferences = standard, targetKey = 'editor-lifecycle', sessionId = 'session-a') => ({ targetKey, scope: { targetId: 'editor', sessionId }, preferences });

test('原生偏好写入串行，快速切换仅保留最新值；迟到回复不回写 UI', async () => {
  const requests = [];
  const accepted = [];
  const failures = [];
  const sync = createMotionPreferenceSync({
    send(value) { return new Promise((resolve) => requests.push({ value, resolve })); },
    accept(value, result) { accepted.push({ value, result }); },
    fail(value, error) { failures.push({ value, error }); },
  });
  sync.enqueue(job());
  sync.enqueue(job({ reduced: false, intensity: 'light' }));
  sync.enqueue(job({ reduced: true, intensity: 'enhanced' }, 'new-editor-lifecycle', 'session-b'));
  assert.equal(requests.length, 1);
  requests[0].resolve('old');
  await settle();
  assert.equal(accepted.length, 0);
  assert.equal(requests.length, 2);
  assert.equal(requests[1].value.scope.sessionId, 'session-b');
  assert.equal(requests[1].value.preferences.reduced, true);
  requests[1].resolve('current');
  await settle();
  assert.equal(accepted.length, 1);
  assert.equal(accepted[0].result, 'current');
  sync.enqueue(job({ reduced: true, intensity: 'enhanced' }, 'new-editor-lifecycle', 'session-b'));
  await settle();
  assert.equal(requests.length, 2, '确认快照不会引起偏好请求循环');
  assert.equal(failures.length, 0);
  sync.dispose();
});

test('偏好失败不循环，销毁后迟到请求无 UI 副作用', async () => {
  let sends = 0;
  const failures = [];
  const sync = createMotionPreferenceSync({ send: async () => { sends++; throw new Error('STALE_REQUEST'); }, accept: () => assert.fail('failed write accepted'), fail: (_, error) => failures.push(error.message) });
  sync.enqueue(job());
  await settle();
  sync.enqueue(job());
  await settle();
  assert.equal(sends, 1);
  assert.deepEqual(failures, ['STALE_REQUEST']);
  sync.dispose();
  sync.enqueue(job({ reduced: true, intensity: 'off' }));
  await settle();
  assert.equal(sends, 1);
  let resolve;
  const disposed = createMotionPreferenceSync({ send: () => new Promise((done) => { resolve = done; }), accept: () => assert.fail('disposed write accepted'), fail: () => assert.fail('disposed write failed UI') });
  disposed.enqueue(job());
  disposed.dispose();
  resolve('late');
  await settle();
});

test('显示请求编号超过当前原生菜单编号，过渡状态由原生模式决定', () => {
  assert.equal(nextDisplayRequestId(2, { requestId: 15 }), 16);
  assert.equal(nextDisplayRequestId(22, { requestId: 15 }), 23);
  assert.equal(displayTransitioning({ mode: 'collapsing' }), true);
  assert.equal(displayTransitioning({ mode: 'expanding' }), true);
  assert.equal(displayTransitioning({ mode: 'orb' }), false);
  assert.throws(() => nextDisplayRequestId(Number.MAX_SAFE_INTEGER, null));
});

test('自动隐藏只读取明确布尔偏好，不执行或容忍其他存储内容', () => {
  assert.equal(parseOrbAutoHide('true'), true);
  assert.equal(parseOrbAutoHide('false'), false);
  for (const value of [null, '', '1', '0', 'TRUE', ' true', '{"enabled":true}', 'window.close()']) assert.equal(parseOrbAutoHide(value), null);
});

test('透明度只读取 20–100 的明确整数百分比，不执行或容忍其他存储内容', () => {
  assert.equal(parseOrbOpacity('100'), 100);
  assert.equal(parseOrbOpacity('20'), 20);
  assert.equal(parseOrbOpacity('55'), 55);
  for (const value of [null, '', ' ', '19', '101', '0', '-40', '60.5', '060', 'NaN', '{"orbOpacity":60}', 'window.close()', '0x3c', '6e1']) {
    assert.equal(parseOrbOpacity(value), null, `rejected: ${JSON.stringify(value)}`);
  }
});

test('仅同一目标会话内原生偏好变化是外部选择，目标重置不覆盖已保存选择', () => {
  const previous = { target, sessionId: 'session-a', display: { orbAutoHide: true } };
  const changed = { ...previous, display: { orbAutoHide: false } };
  assert.equal(nativeOrbPreferenceChanged(previous, changed), true);
  assert.equal(nativeOrbPreferenceChanged(previous, { ...changed, target: { ...target, id: 'target-b' } }), false);
  assert.equal(nativeOrbPreferenceChanged(previous, { ...changed, target: { ...target, process: { ...target.process, pid: 25 } } }), false);
  assert.equal(nativeOrbPreferenceChanged(previous, { ...changed, sessionId: 'session-b' }), false);
  assert.equal(nativeOrbPreferenceChanged(previous, previous), false);
  assert.equal(nativeOrbPreferenceChanged(null, changed), false);
});

test('自动隐藏启动与目标切换使用当前生命周期，快速设置合并且不重复请求', async () => {
  const requests = [];
  const accepted = [];
  const sync = createOrbAutoHidePreferenceSync({
    send(value) { return new Promise((resolve) => requests.push({ value, resolve })); },
    accept(value) { accepted.push(value); },
    fail: () => assert.fail('unexpected auto-hide write failure'),
  });
  const base = { scope: { targetId: null, sessionId: null }, targetKey: 'unbound', enabled: true };
  sync.enqueue(base);
  sync.enqueue(base);
  assert.equal(requests.length, 1, '同一个首次快照只写入一次');
  requests[0].resolve({});
  await settle();
  sync.enqueue({ ...base, enabled: false });
  sync.enqueue({ ...base, enabled: true });
  const current = { ...base, targetKey: 'new-target-lifecycle', scope: { targetId: 'target-b', sessionId: 'session-b' } };
  sync.enqueue(current);
  assert.equal(requests.length, 2);
  requests[1].resolve({});
  await settle();
  assert.equal(requests.length, 3);
  assert.deepEqual(requests[2].value, current);
  assert.deepEqual(accepted, [base], '旧目标偏好回包不更新当前 UI');
  requests[2].resolve({});
  await settle();
  sync.enqueue(current);
  assert.equal(requests.length, 3);
  sync.dispose();
});

test('当前偏好写入完成后采用较新原生确认，旧生命周期与写入前快照不能重播', () => {
  const result = { target, sessionId: 'session-a', revision: 8, display: { orbAutoHide: false } };
  const current = { ...result, revision: 9, display: { orbAutoHide: true } };
  const job = { scope: { targetId: target.id, sessionId: 'session-a' }, targetKey: targetLifecycleKey(result), enabled: false };
  assert.equal(settledOrbChoice(job, result, current), true);
  const includedMenuChange = { ...result, display: { orbAutoHide: true } };
  assert.equal(settledOrbChoice(job, includedMenuChange, includedMenuChange), true, '返回快照已包含原生更改时也采用确认值');
  assert.equal(settledOrbChoice(job, result, { ...current, revision: 7 }), null, '写入前快照不能重设启动保存值');
  assert.equal(settledOrbChoice(job, result, { ...current, target: { ...target, id: 'target-b' } }), null);
  assert.equal(settledOrbChoice(job, result, { ...current, sessionId: 'session-b' }), null);
  assert.equal(settledOrbChoice(job, { ...result, target: { ...target, process: { ...target.process, pid: 25 } } }, current), null);
  assert.equal(settledOrbChoice(job, result, null), null);
});

test('透明度写入串行合并：拖动中的中间值只保留最新一次，确认值不回播', async () => {
  const requests = [];
  const accepted = [];
  const sync = createOrbOpacityPreferenceSync({
    send(value) { return new Promise((resolve) => requests.push({ value, resolve })); },
    accept(value) { accepted.push(value.opacity); },
    fail: () => assert.fail('unexpected opacity write failure'),
  });
  const job = (opacity, targetKey = 'editor-lifecycle', sessionId = 'session-a') => ({ targetKey, scope: { targetId: 'editor', sessionId }, opacity });
  sync.enqueue(job(80));
  sync.enqueue(job(50));
  sync.enqueue(job(20));
  assert.equal(requests.length, 1, '写入进行中的连续取值合并为最新一次');
  requests[0].resolve({});
  await settle();
  assert.equal(requests.length, 2);
  assert.equal(requests[1].value.opacity, 20);
  requests[1].resolve({});
  await settle();
  assert.deepEqual(accepted, [20]);
  sync.enqueue(job(20, 'new-target-lifecycle', 'session-b'));
  assert.equal(requests.length, 3, '生命周期变化后相同数值也要重新应用');
  requests[2].resolve({});
  await settle();
  sync.enqueue(job(20, 'new-target-lifecycle', 'session-b'));
  assert.equal(requests.length, 3, '已确认的数值不会重复请求');
  sync.dispose();
});
