import assert from 'node:assert/strict';
import test from 'node:test';
import { deriveUiState, frameBelongsToCurrentBuild, mapFramePoint } from '../../src/view-state.ts';
import { createFrameStore } from '../../src/frame-store.ts';

const project = { id: 'workspace-1', name: 'native-project', environment: 'windowsLocal', workspaceRoot: 'D:/worktree',
  appRoot: 'D:/worktree/app', module: 'entry', variant: 'default', sdkId: 'verified-sdk' };
const target = { id: 'target-1', app: 'Terminal', title: 'Development', process: { pid: 40, startedAt: '2026-10-09T00:00:00Z' } };
const nativeFrame = { sessionId: 'session-1', buildId: 'build-2', generation: 2, sequence: 1, width: 360, height: 640,
  capturedAt: 100, rotation: 0, pngBase64: 'test-only-payload-not-a-native-proof' };
function snapshot(overrides = {}) {
  return { revision: 2, phase: 'running', target, project, candidates: [], trusted: true, sessionId: 'session-1',
    backend: { id: 'native-previewer', mode: 'native', available: true, reason: '', sdkPath: null, sdkVersion: null,
      canClick: true, canSwipe: false, canText: false, canRotate: false },
    build: { generation: 2, displayedGeneration: 2, buildId: 'build-2', durationMs: 80, error: null, stale: false },
    diagnostics: [], dockHint: null, layoutRequired: false, sessionRef: null, ...overrides };
}
function mailbox() {
  const callbacks = new Map();
  let id = 0;
  const store = createFrameStore({
    requestFrame: (callback) => { callbacks.set(++id, callback); return id; },
    cancelFrame: (handle) => { callbacks.delete(handle); },
  });
  return { store, callbacks, flush: () => { const batch = Array.from(callbacks.values()); callbacks.clear(); for (const callback of batch) callback(); } };
}

test('窗口、工程和预览保持独立；绑定与信任不能伪造首帧', () => {
  const noConnection = deriveUiState(null, null);
  assert.equal(noConnection.binding, 'unbound');
  assert.equal(noConnection.capabilities.click.status, 'pending');
  const untrusted = deriveUiState(snapshot({ phase: 'untrusted', trusted: false }), nativeFrame);
  assert.equal(untrusted.binding, 'bound');
  assert.equal(untrusted.workspace, 'untrusted');
  assert.equal(untrusted.preview, 'unavailable');
  assert.equal(untrusted.inputAllowed, false);
  assert.equal(untrusted.capabilities.refresh.status, 'pending');
  assert.match(untrusted.capabilities.refresh.reason, /信任/);
  const noProject = deriveUiState(snapshot({ project: null, sessionId: null, phase: 'selectProject', trusted: false }), null);
  assert.equal(noProject.binding, 'bound');
  assert.equal(noProject.workspace, 'selecting');
});

test('真实 dockHint 是候选提示，SDK 缺失保持 unavailable 并说明原因', () => {
  const hint = deriveUiState(snapshot({ target: null, project: null, sessionId: null, dockHint: 'Release to bind Terminal' }), null);
  assert.equal(hint.binding, 'candidate');
  const missing = snapshot({ phase: 'sdkMissing', backend: { ...snapshot().backend, available: false, reason: 'SDK_MISSING: 未配置合法 SDK' } });
  const ui = deriveUiState(missing, null);
  assert.equal(ui.workspace, 'ready');
  assert.equal(ui.preview, 'unavailable');
  assert.equal(ui.capabilities.refresh.status, 'unavailable');
  assert.match(ui.capabilities.refresh.reason, /SDK_MISSING/);
});

test('编译完成与运行阶段均不能替代当前会话的有效首帧', () => {
  const building = snapshot({ phase: 'building', build: { ...snapshot().build, durationMs: null, displayedGeneration: null } });
  assert.equal(deriveUiState(building, null).preview, 'building');
  const compiled = { ...building, build: { ...building.build, durationMs: 80 } };
  assert.equal(deriveUiState(compiled, null).preview, 'waitingFrame');
  assert.equal(deriveUiState(snapshot(), null).preview, 'waitingFrame');
  assert.equal(deriveUiState(snapshot(), { ...nativeFrame, sessionId: 'old-session' }).preview, 'waitingFrame');
  assert.equal(deriveUiState(snapshot(), nativeFrame).preview, 'running');
  assert.equal(deriveUiState(snapshot(), nativeFrame).inputAllowed, true);
});

test('构建失败保留同会话旧画面，明确过期并关闭输入', () => {
  const failed = snapshot({ phase: 'stale', build: { ...snapshot().build, generation: 3, buildId: 'build-3', error: 'Compiler error', stale: true } });
  const ui = deriveUiState(failed, nativeFrame);
  assert.equal(ui.preview, 'stale');
  assert.equal(ui.frameCurrent, false);
  assert.equal(ui.inputAllowed, false);
  assert.match(ui.previewLabel, /当前代码构建失败/);
  assert.match(ui.capabilities.click.reason, /旧画面/);
  assert.equal(deriveUiState(failed, null).preview, 'error');
  assert.equal(deriveUiState(failed, { ...nativeFrame, sessionId: 'old-session' }).preview, 'error');
});

test('姿态、文字、滑动与设备控制等待独立能力状态，后端布尔值不能伪造能力', () => {
  const ui = deriveUiState(snapshot({ backend: { ...snapshot().backend, canSwipe: true, canText: true, canRotate: true } }), nativeFrame);
  for (const name of ['swipe', 'text', 'rotate', 'page', 'device', 'appTheme']) {
    assert.equal(ui.capabilities[name].status, 'unavailable', name);
    assert.match(ui.capabilities[name].reason, /等待原生控制能力状态/, name);
    assert.equal(deriveUiState(null, null).capabilities[name].status, 'pending', name);
  }
  for (const name of ['emulator', 'physicalDevice']) {
    assert.equal(ui.capabilities[name].status, 'blocked', name);
    assert.match(ui.capabilities[name].reason, /未接入/, name);
  }
  const runningWithoutClick = deriveUiState(snapshot({ backend: { ...snapshot().backend, canClick: false } }), nativeFrame);
  assert.equal(runningWithoutClick.capabilities.click.status, 'unsupported');
  assert.equal(runningWithoutClick.inputAllowed, false);
  assert.doesNotMatch(runningWithoutClick.previewLabel, /可交互/);
});

test('原生控制报告保留实验与阻塞状态及原因，执行信任和 SDK 门禁优先', () => {
  const names = { swipe: 'touch', text: 'text', rotate: 'orientation', page: 'page', device: 'profile', appTheme: 'colorMode' };
  const control = { experimentalEnabled: false, inputReady: false, capabilities: Object.values(names).map((name) => ({
    name, status: 'experimental', reason: `${name} 公开协议已接入；NOT_RUN 等待实际 SDK 验收`,
  })) };
  const ui = deriveUiState(snapshot(), nativeFrame, control);
  for (const [name, nativeName] of Object.entries(names)) {
    assert.equal(ui.capabilities[name].status, 'experimental', name);
    assert.equal(ui.capabilities[name].reason, control.capabilities.find((item) => item.name === nativeName).reason);
  }
  const reported = { ...control, capabilities: [{ name: 'text', status: 'available', reason: '当前原生后端明确报告支持字符提交' },
    { name: 'orientation', status: 'blocked', reason: '设备方向配置尚未确认' }] };
  assert.deepEqual(deriveUiState(snapshot(), nativeFrame, reported).capabilities.text, { status: 'available', reason: reported.capabilities[0].reason });
  assert.deepEqual(deriveUiState(snapshot(), nativeFrame, reported).capabilities.rotate, { status: 'blocked', reason: reported.capabilities[1].reason });
  assert.equal(deriveUiState(snapshot(), nativeFrame, reported).capabilities.swipe.status, 'unavailable', '缺失单项能力不能继承其他能力');
  for (const [state, status] of [[snapshot({ trusted: false }), 'pending'],
    [snapshot({ backend: { ...snapshot().backend, available: false, reason: 'SDK_MISSING' } }), 'unavailable']]) {
    for (const name of Object.keys(names)) assert.equal(deriveUiState(state, nativeFrame, reported).capabilities[name].status, status, name);
  }
  const device = { reason: 'HDC 设备已连接；签名、部署与画面传输仍未核验', hdcPresent: true, deploymentReady: true, frameReady: false };
  const withDevice = deriveUiState(snapshot(), nativeFrame, control, device);
  for (const name of ['emulator', 'physicalDevice']) assert.deepEqual(withDevice.capabilities[name], { status: 'blocked', reason: device.reason });
});

test('最新帧邮箱合并一帧内通知并拒绝旧会话、构建和顺序', () => {
  const { store, callbacks, flush } = mailbox();
  let notifications = 0;
  const unsubscribe = store.subscribe(() => notifications++);
  for (let sequence = 1; sequence <= 120; sequence++) store.push(snapshot(), { ...nativeFrame, sequence });
  assert.equal(callbacks.size, 1);
  assert.equal(notifications, 0);
  assert.equal(store.getSnapshot().sequence, 120);
  flush();
  assert.equal(notifications, 1);
  const current = store.getSnapshot();
  store.push(snapshot(), { ...nativeFrame, sequence: 119 });
  store.push(snapshot(), { ...nativeFrame, sequence: 121, sessionId: 'old-session' });
  store.push(snapshot(), { ...nativeFrame, sequence: 121, buildId: 'old-build' });
  store.push(snapshot(), { ...nativeFrame, sequence: 121, generation: 1 });
  assert.equal(store.getSnapshot(), current);
  assert.equal(callbacks.size, 0);
  unsubscribe();
});

test('重建与失败保留最后成功画面；新构建重置 sequence', () => {
  const { store, flush } = mailbox();
  store.subscribe(() => undefined);
  store.push(snapshot(), { ...nativeFrame, sequence: 80 });
  flush();
  const previous = store.getSnapshot();
  const newerBuild = snapshot({ revision: 3, phase: 'building', build: { ...snapshot().build, generation: 3,
    buildId: 'build-3', durationMs: null, stale: true } });
  store.sync(newerBuild);
  assert.equal(store.getSnapshot(), previous);
  store.push(newerBuild, { ...nativeFrame, generation: 3, buildId: 'build-3', sequence: 1 });
  assert.equal(store.getSnapshot(), previous, '没有确认编译完成时不得接收首帧');
  const failure = { ...newerBuild, revision: 4, phase: 'stale', build: { ...newerBuild.build, durationMs: 90, error: 'Error' } };
  store.sync(failure);
  assert.equal(store.getSnapshot(), previous);
  store.push(failure, { ...nativeFrame, generation: 3, buildId: 'build-3', sequence: 1 });
  assert.equal(store.getSnapshot(), previous);
  const recovered = { ...failure, revision: 5, phase: 'running', build: { ...failure.build, error: null, stale: false } };
  store.push(recovered, { ...nativeFrame, generation: 3, buildId: 'build-3', sequence: 1 });
  assert.equal(store.getSnapshot().buildId, 'build-3');
  assert.equal(store.getSnapshot().sequence, 1);
});

test('切换生命周期、撤销信任、停止、解除立即清空，不被迟到快照复活', () => {
  const changes = [
    { sessionId: 'session-2' },
    { target: { ...target, id: 'target-2' } },
    { target: { ...target, process: { ...target.process, startedAt: 'new-process-lifecycle' } } },
    { project: { ...project, id: 'workspace-2' } },
    { trusted: false, phase: 'untrusted' },
    { phase: 'stopped' },
    { target: null, project: null, sessionId: null, phase: 'idle' },
  ];
  for (const change of changes) {
    const { store, flush } = mailbox();
    store.subscribe(() => undefined);
    store.push(snapshot(), nativeFrame);
    flush();
    const next = snapshot({ revision: 3, ...change });
    store.sync(next);
    assert.equal(store.getSnapshot(), null, JSON.stringify(change));
    store.push(snapshot(), nativeFrame);
    assert.equal(store.getSnapshot(), null, '迟到状态不得覆盖最新生命周期');
  }
});

test('异常帧尺寸与元数据不显示；卸载取消通知并释放画面', () => {
  const { store, callbacks } = mailbox();
  const unsubscribe = store.subscribe(() => assert.fail('清理后不得通知'));
  for (const change of [{ width: 0 }, { width: 8193 }, { width: 8192, height: 8192 }, { height: Number.NaN },
    { sequence: -1 }, { sequence: 1.2 }, { capturedAt: -1 }, { rotation: 45 }, { pngBase64: '' }]) {
    assert.equal(frameBelongsToCurrentBuild(snapshot(), { ...nativeFrame, ...change }), false);
    store.push(snapshot(), { ...nativeFrame, ...change });
    assert.equal(store.getSnapshot(), null);
  }
  store.push(snapshot(), nativeFrame);
  assert.equal(callbacks.size, 1);
  unsubscribe();
  assert.equal(callbacks.size, 0);
  store.dispose();
  assert.equal(store.getSnapshot(), null);
  store.push(snapshot({ revision: 3 }), nativeFrame);
  assert.equal(store.getSnapshot(), null);
});

test('原生点击按实际画面尺寸映射，缩放和 DPI 不改变输入坐标，外框与黑边不接收输入', () => {
  for (const scale of [0.5, 0.75, 1, 1.25, 1.5, 2]) {
    const bounds = { left: 24, top: 48, width: nativeFrame.width * scale, height: nativeFrame.height * scale };
    assert.deepEqual(mapFramePoint(nativeFrame, bounds, bounds.left + bounds.width / 2, bounds.top + bounds.height / 2), { x: 180, y: 320 });
    assert.deepEqual(mapFramePoint(nativeFrame, bounds, bounds.left, bounds.top), { x: 0, y: 0 });
    assert.equal(mapFramePoint(nativeFrame, bounds, bounds.left - 0.01, bounds.top), null);
    assert.equal(mapFramePoint(nativeFrame, bounds, bounds.left, bounds.top - 0.01), null);
    assert.equal(mapFramePoint(nativeFrame, bounds, bounds.left + bounds.width, bounds.top), null);
    assert.equal(mapFramePoint(nativeFrame, bounds, bounds.left, bounds.top + bounds.height), null);
  }
  assert.equal(mapFramePoint(nativeFrame, { left: 0, top: 0, width: 0, height: 10 }, 0, 0), null);
  assert.equal(mapFramePoint(nativeFrame, { left: 0, top: 0, width: 10, height: 10 }, Number.NaN, 0), null);
});
