import assert from 'node:assert/strict';
import test from 'node:test';
import { sameTarget, responseStillCurrent, hasDockCandidate } from '../../src/request-state.ts';

test('当前响应必须属于相同目标生命周期、工程和会话', () => {
  const current = { target: { id: 't1', process: { pid: 100, startedAt: 'now' } }, sessionId: 's1', project: { id: 'p1' } };
  assert.equal(responseStillCurrent(current, current), true);
  assert.equal(responseStillCurrent({ ...current, sessionId: 'old' }, current), false);
  assert.equal(responseStillCurrent({ ...current, target: { ...current.target, process: { pid: 100, startedAt: 'old' } } }, current), false);
  assert.equal(responseStillCurrent({ ...current, project: { id: 'old' } }, current), false);
  assert.equal(responseStillCurrent({ target: null, project: null, sessionId: null }, current), false);
  assert.equal(sameTarget(current.target, { ...current.target, id: 'other' }), false);
});

test('仅真实释放绑定提示属于拖动候选，最小化或空间不足提示不高亮', () => {
  assert.equal(hasDockCandidate('Release to bind Terminal'), true);
  for (const hint of [null, 'Target minimized', 'Not enough space', 'Target unavailable']) assert.equal(hasDockCandidate(hint), false);
});
