import assert from 'node:assert/strict';
import test from 'node:test';
import { runSnapshotCommand } from '../../src/app/command-navigation.ts';

const target = { id: 'agent-window', process: { pid: 42, startedAt: '1' } };
const state = (fields = {}) => ({ revision: 1, target, sessionId: null, project: null, trusted: false,
  phase: 'selectProject', contextSource: { boundRegistrationId: null }, ...fields });
const selected = (fields = {}) => state({ revision: 2, sessionId: 'preview', project: { id: 'project' },
  trusted: true, phase: 'building', contextSource: { boundRegistrationId: 'context-registration' }, ...fields });

function navigation(initial = state(), panel = 'mcp') {
  const actual = { snapshot: initial, panel, revision: 0, changes: [] };
  const effects = { snapshot: () => actual.snapshot, receive: next => {
    if (next.revision >= actual.snapshot.revision) actual.snapshot = next;
  }, revision: () => actual.revision, open: next => { actual.panel = next; actual.changes.push(next); } };
  return { actual, effects };
}

test('starting native docking dismisses the old MCP sheet before waiting for the window reply', async () => {
  const { actual, effects } = navigation();
  let finish;
  const pending = runSnapshotCommand('begin_drag', () => new Promise(resolve => { finish = resolve; }), effects);
  assert.equal(actual.panel, null);
  assert.deepEqual(actual.changes, [null]);
  finish(state({ revision: 2 }));
  assert.equal(await pending, true);
  assert.equal(actual.panel, null);
});

test('confirming a trusted MCP project returns to preview, including confirmation on the same attached target', async () => {
  for (const phase of ['building', 'running', 'sdkMissing']) {
    const { actual, effects } = navigation(selected({ revision: 1 }));
    assert.equal(await runSnapshotCommand('context_select_project', async () => selected({ phase }), effects, 'context-registration'), true);
    assert.equal(actual.panel, null);
    effects.receive(selected({ revision: 3, phase }));
    assert.equal(actual.panel, null, 'subsequent snapshots cannot restore the old MCP sheet');
  }
});

test('MCP confirmation opens trust or module selection when those steps remain necessary', async () => {
  for (const [reply, expected] of [[selected({ phase: 'untrusted', trusted: false }), 'trust'],
    [selected({ phase: 'selectProject', project: null }), 'projects']]) {
    const { actual, effects } = navigation();
    assert.equal(await runSnapshotCommand('context_select_project', async () => reply, effects, 'context-registration'), true);
    assert.equal(actual.panel, expected);
  }
});

test('failed or superseded context confirmation does not close or reopen another user-selected page', async () => {
  const { actual, effects } = navigation();
  await assert.rejects(runSnapshotCommand('context_select_project', async () => { throw new Error('CONTEXT_EXPIRED'); }, effects, 'context-registration'), /CONTEXT_EXPIRED/);
  assert.equal(actual.panel, 'mcp');
  let finish;
  const pending = runSnapshotCommand('context_select_project', () => new Promise(resolve => { finish = resolve; }), effects, 'context-registration');
  actual.panel = 'diagnostics'; actual.revision++;
  finish(selected());
  assert.equal(await pending, false);
  assert.equal(actual.panel, 'diagnostics');
  assert.deepEqual(actual.changes, []);
});

test('a late confirmation cannot navigate for a replaced window or another context registration', async () => {
  for (const newer of [selected({ revision: 3, target: { ...target, process: { pid: 42, startedAt: '2' } } }),
    selected({ revision: 3, contextSource: { boundRegistrationId: 'new-registration' } })]) {
    const { actual, effects } = navigation();
    assert.equal(await runSnapshotCommand('context_select_project', async () => {
      actual.snapshot = newer;
      return selected();
    }, effects, 'context-registration'), false);
    assert.equal(actual.panel, 'mcp');
    assert.deepEqual(actual.changes, []);
  }
});

test('ordinary snapshot requests preserve manually opened MCP settings', async () => {
  const { actual, effects } = navigation(selected({ revision: 1 }));
  assert.equal(await runSnapshotCommand('snapshot', async () => selected(), effects), true);
  assert.equal(actual.panel, 'mcp');
  assert.deepEqual(actual.changes, []);
});
