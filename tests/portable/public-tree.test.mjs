import assert from 'node:assert/strict';
import test from 'node:test';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { findForbiddenPublicPaths } from '../../tools/check-public-tree.mjs';

test('public tree guard catches private Harness and execution-record paths', () => {
  assert.deepEqual(findForbiddenPublicPaths(['artifacts/native-smoke.json', '.local/sdk.json', '.codex/task.md', '.agents/state.md', '.playwright-mcp/log.txt']),
    ['artifacts/native-smoke.json', '.local/sdk.json', '.codex/task.md', '.agents/state.md', '.playwright-mcp/log.txt']);
  assert.deepEqual(findForbiddenPublicPaths(['examples/app/.codex/state.md', 'tests/.local/sdk.json', 'examples/app/artifacts/verify.json']),
    ['examples/app/.codex/state.md', 'tests/.local/sdk.json', 'examples/app/artifacts/verify.json']);
  assert.deepEqual(findForbiddenPublicPaths([
    'AGENTS.md',
    'CLAUDE.md',
    'ENVIRONMENT.md',
    'subproject/AGENTS.md',
    'subproject/ENVIRONMENT.md',
    'docs/harness/security.md',
    'docs/plans/active/change.md',
    'docs/evidence/run.md',
    'docs/status.md',
    'docs/native-preview/validation.md',
    'docs/runbooks/windows-native-preview.md',
    'tools/verify-harness.mjs',
  ]), [
    'AGENTS.md',
    'CLAUDE.md',
    'ENVIRONMENT.md',
    'subproject/AGENTS.md',
    'subproject/ENVIRONMENT.md',
    'docs/harness/security.md',
    'docs/plans/active/change.md',
    'docs/evidence/run.md',
    'docs/status.md',
    'docs/native-preview/validation.md',
    'docs/runbooks/windows-native-preview.md',
    'tools/verify-harness.mjs',
  ]);
  assert.deepEqual(findForbiddenPublicPaths(['docs/product.md', 'docs/compatibility.md', 'src/App.tsx']), []);
});

test('public formatting ignores local Harness content and still validates public files', () => {
  const directory = mkdtempSync(join(tmpdir(), 'arkdock-public-format-'));
  const checker = fileURLToPath(new URL('../../tools/check-format.mjs', import.meta.url));
  try {
    mkdirSync(join(directory, 'docs/harness'), { recursive: true });
    writeFileSync(join(directory, 'AGENTS.md'), 'local instructions\r\n');
    writeFileSync(join(directory, 'docs/harness/private.json'), '{invalid private data');
    writeFileSync(join(directory, 'public.json'), '{}\n');
    assert.equal(spawnSync(process.execPath, [checker], { cwd: directory, encoding: 'utf8' }).status, 0);
    writeFileSync(join(directory, 'public.json'), '{invalid public data');
    assert.equal(spawnSync(process.execPath, [checker], { cwd: directory, encoding: 'utf8' }).status, 1);
  } finally {
    assert.ok(resolve(directory).startsWith(resolve(tmpdir()) + sep));
    rmSync(directory, { recursive: true, force: true });
  }
});
