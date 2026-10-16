import assert from 'node:assert/strict';
import test from 'node:test';
import { applySnapshot, acceptFrame, filterCandidates, needsDirectoryFallback, phaseText } from '../../src/view-state.ts';
import { applyTheme, isTheme, parseTheme, readTheme, themeOptions } from '../../src/theme.ts';

test('迟到快照不覆盖较新后端状态', () => {
  const current = { revision: 8, sessionId: 'live', phase: 'untrusted' };
  assert.equal(applySnapshot(current, { revision: 7, phase: 'running' }), current);
  assert.equal(applySnapshot(current, current), current);
  assert.equal(applySnapshot(current, { revision: 9 }).revision, 9);
});

test('只有当前成功构建的真实帧可显示，旧会话/旧版本拒绝', () => {
  const state = { sessionId: 's1', build: { generation: 3, displayedGeneration: 3, buildId: 'b3' } };
  const frame = { sessionId: 's1', buildId: 'b3', generation: 3, sequence: 2 };
  assert.equal(acceptFrame(state, null, frame), frame);
  assert.equal(acceptFrame(state, frame, { ...frame, sequence: 1 }), frame);
  assert.equal(acceptFrame(state, frame, { ...frame, sessionId: 'old' }), frame);
  assert.equal(acceptFrame(state, frame, { ...frame, generation: 2 }), frame);
  assert.equal(acceptFrame(state, frame, { ...frame, buildId: 'old' }), frame);
  assert.equal(acceptFrame({ ...state, build: { ...state.build, displayedGeneration: 2 } }, null, frame), frame,
    '后端已接受的新帧不应因显示代次快照迟到而丢失');
  const previousBuild = { ...frame, buildId: 'b2', generation: 2, sequence: 80 };
  const firstNewFrame = { ...frame, sequence: 1 };
  assert.equal(acceptFrame(state, previousBuild, firstNewFrame), firstNewFrame,
    '每次实际 Previewer 重启会重新计数，新构建首帧必须替换旧画面');
});

test('搜索不静默截断候选；完整路径和模块都可检索', () => {
  const items = Array.from({ length: 250 }, (_, i) => ({ id: String(i), project: { name: `app-${i}`, appRoot: `/worktree/${i}`, workspaceRoot: '/root', module: 'entry', environment: 'windowsLocal' }, reasons: [] }));
  assert.equal(filterCandidates(items, '').length, 250);
  assert.equal(filterCandidates(items, '/worktree/249')[0].id, '249');
  assert.equal(filterCandidates(items, 'entry').length, 250);
});

test('不可用状态有真实下一步且不标成功', () => {
  assert.match(phaseText('sdkMissing'), /SDK/);
  assert.match(phaseText('protocolBlocked'), /工具链/);
  assert.match(phaseText('untrusted'), /信任/);
  assert.match(phaseText('unsupported'), /Windows/);
});

test('仅无效或不支持环境的候选仍能使用目录兜底', () => {
  assert.equal(needsDirectoryFallback([]), true);
  assert.equal(needsDirectoryFallback([{ valid: false, project: { environment: 'windowsLocal' } }]), true);
  assert.equal(needsDirectoryFallback([{ valid: true, project: { environment: 'wsl' } }]), true);
  assert.equal(needsDirectoryFallback([{ valid: true, project: { environment: 'windowsLocal' } }]), false);
});

test('主题偏好支持极简浅色与深色钛金，保留原有主题并对非法存储值安全回退', () => {
  assert.deepEqual(themeOptions.map((item) => item.id), ['minimal-light', 'titanium-dark', 'dark', 'light', 'system']);
  for (const id of ['minimal-light', 'titanium-dark', 'dark', 'light', 'system']) {
    assert.equal(isTheme(id), true);
    assert.equal(parseTheme(id), id);
  }
  for (const invalid of [null, undefined, '', 'neon', 'DARK', '{"theme":"light"}']) {
    assert.equal(isTheme(invalid), false);
    assert.equal(parseTheme(invalid), 'dark');
  }
  const previousStorage = globalThis.localStorage;
  const previousDocument = globalThis.document;
  const store = new Map();
  globalThis.localStorage = { getItem: (key) => store.get(key) ?? null, setItem: (key, value) => store.set(key, String(value)) };
  globalThis.document = { documentElement: { dataset: {} } };
  try {
    assert.equal(readTheme(), 'dark');
    for (const id of ['minimal-light', 'titanium-dark', 'light', 'dark', 'system']) {
      applyTheme(id);
      assert.equal(globalThis.document.documentElement.dataset.theme, id);
      assert.equal(readTheme(), id);
    }
    store.set('arkdock:theme', 'corrupted-value');
    assert.equal(readTheme(), 'dark');
  } finally {
    if (previousStorage === undefined) delete globalThis.localStorage; else globalThis.localStorage = previousStorage;
    if (previousDocument === undefined) delete globalThis.document; else globalThis.document = previousDocument;
  }
});
