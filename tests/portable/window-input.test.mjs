import assert from 'node:assert/strict';
import test from 'node:test';
import { requireOwnedTargetEdge } from '../../tools/window-input.mjs';

test('read-only edge preflight accepts target or verified own surface and blocks foreign coverage', () => {
  const bounds = { visible: true, minimized: false, frameRight: 1000, frameTop: 0, frameBottom: 800 };
  const probes = (owned, processId) => (command) => {
    assert.equal(command, 'probe', 'preflight must not dispatch mouse input');
    return { owned, processId };
  };
  assert.equal(requireOwnedTargetEdge(probes(true, 20), 1, bounds, 10).length, 3);
  assert.equal(requireOwnedTargetEdge(probes(false, 10), 1, bounds, 10).length, 3);
  assert.throws(() => requireOwnedTargetEdge(probes(false, 30), 1, bounds, 10), error => error.blocked === true);
  assert.throws(() => requireOwnedTargetEdge(probes(false, 10), 1, { ...bounds, visible: false }, 10), error => error.blocked === true);
});
