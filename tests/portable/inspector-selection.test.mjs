import assert from 'node:assert/strict';
import test from 'node:test';
import { createHitIndex, inspectionMetadata, regionBetween, samePixels, treeMatchesFrame, treeMatchesRuntime } from '../../src/inspector/selection-model.ts';

const node = (id, depth, bounds, parentId = null, attributes = []) => ({ nodeId: id, depth, bounds, parentId, attributes, children: [], visible: true, componentType: 'Column', page: null, paintOrder: null });

test('本地命中优先层级并保留歧义候选，不按最小面积猜组件', () => {
  const index = createHitIndex([node('tiny', 0, { x: 10, y: 10, width: 5, height: 5 }), node('deep', 3, { x: 0, y: 0, width: 100, height: 100 })], 360, 640);
  assert.deepEqual(index.hit(12, 12).map((n) => n.nodeId), ['deep', 'tiny']);
  assert.deepEqual(index.hit(-1, 12), []);
  assert.deepEqual(index.hit(360, 12), []);
});

test('仅显式裁切祖先限制溢出子组件，隐藏祖先与父环拒绝', () => {
  const bounds = { x: 0, y: 0, width: 10, height: 10 };
  const child = node('child', 1, { x: 5, y: 5, width: 30, height: 30 }, 'parent');
  assert.equal(createHitIndex([node('parent', 0, bounds), child], 360, 640).hit(15, 15).length, 1);
  assert.equal(createHitIndex([node('parent', 0, bounds, null, [{ name: 'clip', value: 'true' }]), child], 360, 640).hit(15, 15).length, 0);
  assert.equal(createHitIndex([{ ...node('parent', 0, bounds), visible: false }, child], 360, 640).hit(8, 8).length, 0);
  assert.equal(createHitIndex([node('parent', 0, bounds, 'child'), child], 360, 640).hit(8, 8).length, 0);
});

test('截图框选双向拖动有界，组件树必须匹配显示帧完整身份', () => {
  assert.deepEqual(regionBetween({ x: 355, y: 635 }, { x: 400, y: 800 }, 360, 640), { x: 355, y: 635, width: 5, height: 5 });
  const frame = { workspaceId: 'w', sessionId: 's', buildId: 'b', generation: 1, sequence: 2 };
  const tree = { ...frame, frameSequence: 2 };
  assert.equal(treeMatchesFrame(tree, frame), true);
  for (const field of ['workspaceId', 'sessionId', 'buildId', 'generation', 'frameSequence']) assert.equal(treeMatchesFrame({ ...tree, [field]: 'old' }, frame), false);
});
test('新帧必须保持原生像素内容才能复用组件树，IPC 不发送前端像素字段', () => {
  const a = new Uint8Array([1, 2, 3]).buffer;
  assert.equal(samePixels(a, a.slice(0)), true);
  assert.equal(samePixels(a, new Uint8Array([1, 2, 4]).buffer), false);
  assert.equal(samePixels(a, undefined), false);
  const frame = { protocolVersion: 1, workspaceId: 'w', sessionId: 's', buildId: 'b', generation: 1, sequence: 10, width: 360, height: 640, capturedAt: 1, rotation: 0, format: 'jpeg', payloadLength: 3, jpeg: a, receivedAt: 5 };
  const metadata = inspectionMetadata(frame);
  assert.equal(metadata.jpeg, undefined);
  assert.equal(metadata.receivedAt, undefined);
  assert.equal(metadata.sequence, 10);
  assert.equal(treeMatchesRuntime({ ...metadata, frameSequence: 8 }, metadata), true);
  assert.equal(treeMatchesRuntime({ ...metadata, sessionId: 'old', frameSequence: 8 }, metadata), false);
  assert.equal(treeMatchesRuntime({ ...metadata, frameSequence: 11 }, metadata), false);
});
