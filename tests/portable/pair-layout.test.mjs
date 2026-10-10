import assert from 'node:assert/strict';
import test from 'node:test';
import { pairControl } from '../../src/pair-layout.ts';

test('共同窗口控制不依赖 SDK 或工程信任；仅后端确认最大化后显示恢复', () => {
  const snapshot = { target: { id: 'target' }, trusted: false, backend: { available: false }, pairLayout: { state: 'paired', reason: null } };
  assert.equal(pairControl(snapshot).enabled, true);
  assert.equal(pairControl(snapshot).command, 'maximize_pair');
  assert.equal(pairControl({ ...snapshot, pairLayout: { state: 'maximizing' } }).enabled, false);
  assert.equal(pairControl({ ...snapshot, pairLayout: { state: 'maximizing' } }).command, 'maximize_pair');
  assert.equal(pairControl({ ...snapshot, pairLayout: { state: 'maximized' } }).command, 'restore_pair');
  assert.equal(pairControl({ ...snapshot, pairLayout: { state: 'restoring' } }).enabled, false);
});

test('缺连接、未知布局和阻塞状态不伪装可执行，保留后端原因', () => {
  assert.equal(pairControl(null).enabled, false);
  assert.equal(pairControl({ target: { id: 'target' }, pairLayout: null }).enabled, false);
  const blocked = pairControl({ target: { id: 'target' }, pairLayout: { state: 'blocked', reason: '工作区不足' } });
  assert.equal(blocked.enabled, false);
  assert.equal(blocked.reason, '工作区不足');
});
