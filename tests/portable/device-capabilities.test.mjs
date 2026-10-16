import assert from 'node:assert/strict';
import test from 'node:test';
import { deviceCapabilityContext, observeDeviceCapabilities } from '../../src/preview/device-capabilities.ts';

const snapshot = () => ({ target: { id: 'host' }, project: { id: 'project', sdkId: 'sdk' }, trusted: true,
  sessionId: null, backend: { available: false, sdkPath: 'sdk-root' } });
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };
const tick = async () => { for (let i = 0; i < 6; i++) await Promise.resolve(); };

test('设备能力读取不要求 Previewer 会话，未知工具状态不会被推断可用', async () => {
  let current = snapshot(), calls = 0;
  const pending = deferred(), changes = [], timers = new Map();
  const stop = observeDeviceCapabilities({ getSnapshot: () => current, change: (state) => changes.push(state),
    bridge: { transfer: (command, _args, scope) => { calls++; assert.equal(command, 'device_runtime_state'); assert.equal(scope.sessionId, null); return pending.promise; } },
    clock: { schedule: (callback, delay) => { assert.equal(delay, 1000); timers.set(1, callback); return 1; }, cancel: (handle) => timers.delete(handle) } });
  assert.equal(calls, 1); assert.equal(timers.size, 0); assert.equal(changes.length, 0);
  current = { ...current, sessionId: 'new-preview-session' };
  const device = { identity: { workspaceId: 'project' }, hdcPresent: false, reason: 'HDC_MISSING' };
  pending.resolve(device); await tick();
  assert.equal(changes[0], device); assert.equal(timers.size, 1); stop(); assert.equal(timers.size, 0);
  assert.equal(deviceCapabilityContext({ ...current, trusted: false }), null);
});

test('工程、目标或信任变更后拒收旧设备状态，且不再继续旧轮询', async () => {
  for (const change of [(state) => ({ ...state, project: { ...state.project, id: 'new' } }),
    (state) => ({ ...state, target: { id: 'new-host' } }), (state) => ({ ...state, trusted: false })]) {
    let current = snapshot(), published = 0, schedules = 0;
    const pending = deferred();
    const stop = observeDeviceCapabilities({ getSnapshot: () => current, change: () => published++, bridge: { transfer: () => pending.promise },
      clock: { schedule: () => { schedules++; return 1; }, cancel: () => {} } });
    current = change(current); pending.resolve({ identity: { workspaceId: 'project' }, hdcPresent: true }); await tick();
    assert.equal(published, 0); assert.equal(schedules, 0); stop();
  }
});

test('隐藏再显示仍共享单请求门闩，旧读取结束不更新已销毁面板', async () => {
  const gate = { pending: false }, requests = [], changes = [], timers = new Map(); let serial = 0;
  const options = { gate, getSnapshot: snapshot, change: (state) => changes.push(state),
    bridge: { transfer: () => { const pending = deferred(); requests.push(pending); return pending.promise; } },
    clock: { schedule: (callback) => { timers.set(++serial, callback); return serial; }, cancel: (handle) => timers.delete(handle) } };
  const stopOld = observeDeviceCapabilities(options); stopOld();
  const stopNew = observeDeviceCapabilities(options);
  assert.equal(requests.length, 1, 'visibility changes cannot accumulate concurrent IPC requests');
  requests[0].resolve({ identity: { workspaceId: 'project' }, hdcPresent: true }); await tick();
  assert.equal(changes.length, 0);
  const [id, callback] = timers.entries().next().value; timers.delete(id); callback();
  assert.equal(requests.length, 2);
  requests[1].resolve({ identity: { workspaceId: 'stale-project' }, hdcPresent: true }); await tick();
  assert.deepEqual(changes, [null], 'a mismatched payload stays unknown');
  stopNew(); assert.equal(timers.size, 0);
});
