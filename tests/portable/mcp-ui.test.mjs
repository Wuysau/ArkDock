import assert from 'node:assert/strict';
import test from 'node:test';
import { canRetry, contextMatches, createStatusPoller, selectionMatches, submitFrozenSelection, taskLabels } from '../../src/mcp/model.ts';
import { mcpRevokeContext, mcpStatus, transfer } from '../../src/ipc/client.ts';

const context = { contextId: 'context', registrationId: 'registration', connected: true, confirmed: true,
  originalProject: 'D:/original', executionDirectory: 'D:/worktree/module', environment: 'windowsLocal' };
const snapshot = { target: { id: 'target' }, project: { id: 'workspace', environment: 'windowsLocal' }, trusted: true,
  sessionId: 'session', build: { buildId: 'build', generation: 1 }, contextSource: { boundRegistrationId: 'registration',
    registrations: [{ id: 'registration', executionDirectory: context.executionDirectory, originalProject: context.originalProject }] } };

test('MCP submit eligibility requires the confirmed current registration and selection identity', () => {
  assert.equal(contextMatches(snapshot, context), true);
  for (const changed of [{ connected: false }, { confirmed: false }, { registrationId: 'other' }, { executionDirectory: 'D:/other' }, { originalProject: 'D:/other' }, { environment: 'remote' }]) {
    assert.equal(contextMatches(snapshot, { ...context, ...changed }), false);
  }
  for (const changed of [{ trusted: false }, { target: null }, { project: null }, { sessionId: null }, { contextSource: null }]) {
    assert.equal(contextMatches({ ...snapshot, ...changed }, context), false);
  }
  const selection = { workspaceId: 'workspace', sessionId: 'session', buildId: 'build', generation: 1, expiresAt: 100 };
  assert.equal(selectionMatches(snapshot, selection, 99), true);
  assert.equal(selectionMatches(snapshot, selection, 100), false);
  for (const field of ['workspaceId', 'sessionId', 'buildId', 'generation']) assert.equal(selectionMatches(snapshot, { ...selection, [field]: 'other' }, 99), false);
});

test('MCP retry is explicit, restricted to failed/unknown, and can target a new confirmed connection', () => {
  assert.equal(canRetry(snapshot, undefined, { state: 'unknown' }), false);
  for (const state of ['failed', 'unknown']) assert.equal(canRetry(snapshot, context, { contextId: 'disconnected-old-process', state }), true);
  for (const state of ['pending', 'claimed', 'acknowledged', 'completed', 'expired']) assert.equal(canRetry(snapshot, context, { state }), false);
  assert.equal(canRetry({ ...snapshot, trusted: false }, context, { state: 'failed' }), false);
  assert.equal(taskLabels.completed, 'Agent 报告完成');
});

test('submission saves current instruction before queuing and stops after a stale save or revoked binding', async () => {
  const selection = { selectionId: 'selection', workspaceId: 'workspace', sessionId: 'session', buildId: 'build', generation: 1, expiresAt: Date.now() + 10000 };
  const calls = [];
  let actual = snapshot, current = true, saved = selection;
  const options = { context, selectionId: selection.selectionId,
    save: async () => { calls.push('save current instruction'); return saved; },
    isCurrent: () => current, snapshot: () => actual,
    submit: async (selectionId, contextId) => { calls.push({ selectionId, contextId }); return { taskId: 'task' }; } };
  assert.deepEqual(await submitFrozenSelection(options), { taskId: 'task' });
  assert.deepEqual(calls, ['save current instruction', { selectionId: 'selection', contextId: 'context' }]);
  calls.length = 0; current = false;
  assert.equal(await submitFrozenSelection(options), null); assert.deepEqual(calls, ['save current instruction']);
  current = true; actual = { ...snapshot, contextSource: { ...snapshot.contextSource, boundRegistrationId: 'other' } }; calls.length = 0;
  assert.equal(await submitFrozenSelection(options), null); assert.deepEqual(calls, ['save current instruction']);
  actual = snapshot; saved = { ...selection, selectionId: 'other-selection' }; calls.length = 0;
  assert.equal(await submitFrozenSelection(options), null); assert.deepEqual(calls, ['save current instruction']);
  saved = selection; calls.length = 0;
  assert.equal(await submitFrozenSelection({ ...options, save: async () => { current = false; return selection; } }), null);
  assert.deepEqual(calls, []);
});

test('status poller drops late lifecycle responses, never overlaps, and limits starts to once per second', async () => {
  let time = 0, timerId = 0, requestCount = 0;
  const timers = new Map(), responses = [], values = [], errors = [];
  const poller = createStatusPoller(() => { requestCount++; return new Promise((resolve, reject) => responses.push({ resolve, reject })); },
    (callback, delay) => { const id = ++timerId; timers.set(id, { callback, at: time + delay }); return id; }, id => timers.delete(id), () => time);
  const run = at => { time = at; for (const [id, timer] of [...timers]) if (timer.at <= at) { timers.delete(id); timer.callback(); } };
  const flush = async () => { await Promise.resolve(); await Promise.resolve(); };
  const stopOld = poller.start(value => values.push(['old', value]), error => errors.push(error));
  run(0); assert.equal(requestCount, 1);
  run(5000); assert.equal(requestCount, 1);
  stopOld();
  const stopNew = poller.start(value => values.push(['new', value]), error => errors.push(error));
  run(5000); assert.equal(requestCount, 1);
  responses[0].resolve('stale'); await flush(); run(5000);
  assert.deepEqual(values, []); assert.equal(requestCount, 2);
  responses[1].resolve('current'); await flush();
  assert.deepEqual(values, [['new', 'current']]); run(5999); assert.equal(requestCount, 2);
  run(6000); assert.equal(requestCount, 3);
  stopNew(); responses[2].reject(new Error('unmounted')); await flush();
  run(20000); assert.equal(requestCount, 3); assert.deepEqual(errors, []); assert.equal(timers.size, 0);
});

test('MCP setup works before docking while task operations retain scoped gates', async t => {
  const previous = globalThis.window, calls = [];
  globalThis.window = { __TAURI__: { core: { invoke: async (command, args) => { calls.push({ command, args }); return command === 'mcp_revoke_context' ? true : {}; } } } };
  t.after(() => { globalThis.window = previous; });
  await mcpStatus(); await mcpRevokeContext('selected-context');
  assert.deepEqual(calls, [{ command: 'mcp_status', args: {} }, { command: 'mcp_revoke_context', args: { contextId: 'selected-context' } }]);
  const args = { contextId: 'selected-context', selectionId: 'selection' };
  await assert.rejects(transfer('mcp_submit_task', args, { targetId: null, sessionId: null }), /原生预览会话/);
  await assert.rejects(transfer('mcp_retry_task', { taskId: 'task', contextId: 'selected-context' }, { targetId: 'target', sessionId: null }), /原生预览会话/);
  assert.equal(calls.length, 2);
  const scope = { targetId: 'target', sessionId: 'session' };
  await transfer('mcp_submit_task', args, scope);
  assert.deepEqual(calls[2], { command: 'mcp_submit_task', args: { ...args, scope } });
});
