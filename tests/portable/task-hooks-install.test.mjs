import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync, lstatSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve, relative } from 'node:path';
import { spawnSync } from 'node:child_process';
import { installTaskHooks } from '../../tools/install-task-hooks.mjs';

function fixture() {
  const directory = mkdtempSync(join(tmpdir(), 'arkdock-hook-install-'));
  const root = join(directory, 'repo'); mkdirSync(root);
  const git = args => {
    const result = spawnSync('git', ['-C', root, ...args], { encoding: 'utf8', shell: false, windowsHide: true });
    assert.equal(result.status, 0, result.stderr); return result.stdout.trim();
  };
  git(['init', '-b', 'dev']); git(['config', 'user.name', 'Hook fixture']); git(['config', 'user.email', 'fixture@example.invalid']);
  writeFileSync(join(root, '.gitignore'), '# Fixture has no project hook ignores\n');
  writeFileSync(join(root, 'source.txt'), 'Fixture only\n'); git(['add', '.']); git(['commit', '-m', 'Fixture baseline']);
  return { directory, root, git };
}
function cleanup(directory) {
  const path = resolve(directory), base = resolve(tmpdir());
  assert.ok(relative(base, path).startsWith('arkdock-hook-install-'));
  function noLinks(current) {
    assert.equal(lstatSync(current).isSymbolicLink(), false);
    if (lstatSync(current).isDirectory()) for (const name of readdirSync(current)) noLinks(join(current, name));
  }
  noLinks(path); rmSync(path, { recursive: true });
}

test('project Stop installation preserves other hooks, is idempotent and bootstraps new worktrees', () => {
  const f = fixture();
  try {
    mkdirSync(join(f.root, '.codex'));
    const other = { type: 'command', command: 'echo fixture-only', statusMessage: 'Unrelated fixture hook' };
    writeFileSync(join(f.root, '.codex/hooks.json'), JSON.stringify({ hooks: { Stop: [{ hooks: [other] }] } }));
    const first = installTaskHooks({ cwd: f.root });
    assert.equal(first.bootstrap, true); assert.match(first.nativeTrust, /REQUIRED/);
    assert.equal(f.git(['status', '--porcelain']), '');
    const config = JSON.parse(readFileSync(join(f.root, '.codex/hooks.json'), 'utf8'));
    assert.deepEqual(config.hooks.Stop[0].hooks, [other]);
    assert.equal(config.hooks.Stop.at(-1).hooks[0].type, 'command');
    const before = readFileSync(join(f.root, 'AGENTS.md'), 'utf8');
    installTaskHooks({ cwd: f.root });
    assert.equal(readFileSync(join(f.root, 'AGENTS.md'), 'utf8'), before);
    assert.equal(JSON.parse(readFileSync(join(f.root, '.codex/hooks.json'), 'utf8')).hooks.Stop.length, 2);
    const task = join(f.directory, 'task'); f.git(['worktree', 'add', '-b', 'codex/fixture', task]);
    assert.ok(existsSync(join(task, '.codex/hooks.json')));
    assert.match(readFileSync(join(task, 'AGENTS.md'), 'utf8'), /任务实际完成/);
    assert.equal(JSON.parse(readFileSync(join(task, '.codex/hooks.json'), 'utf8')).hooks.Stop.at(-1).hooks[0].command, config.hooks.Stop.at(-1).hooks[0].command);
  } finally { cleanup(f.directory); }
});

test('existing active Git hooks remain configured and are not silently replaced', () => {
  const f = fixture();
  try {
    const original = join(f.root, '.git/hooks/post-checkout');
    writeFileSync(original, '#!/bin/sh\n# Fixture user hook\n');
    const result = installTaskHooks({ cwd: f.root });
    assert.equal(result.bootstrap, false);
    assert.equal(readFileSync(original, 'utf8'), '#!/bin/sh\n# Fixture user hook\n');
    const config = spawnSync('git', ['-C', f.root, 'config', '--local', '--get', 'core.hooksPath'], { encoding: 'utf8', shell: false, windowsHide: true });
    assert.equal(config.status, 1);
  } finally { cleanup(f.directory); }
});

test('inherited Git hooksPath is preserved without adding a local override', () => {
  const f = fixture();
  const previous = process.env.GIT_CONFIG_GLOBAL;
  try {
    const global = join(f.directory, 'fixture-global.gitconfig');
    const hooks = join(f.directory, 'user-hooks'); mkdirSync(hooks);
    const original = '#!/bin/sh\n# Inherited fixture hook\n';
    writeFileSync(join(hooks, 'post-checkout'), original);
    f.git(['config', '--file', global, 'core.hooksPath', hooks]);
    process.env.GIT_CONFIG_GLOBAL = global;
    const result = installTaskHooks({ cwd: f.root });
    assert.equal(result.bootstrap, false);
    assert.equal(resolve(f.git(['config', '--path', '--get', 'core.hooksPath'])), resolve(hooks));
    assert.equal(readFileSync(join(hooks, 'post-checkout'), 'utf8'), original);
    const local = spawnSync('git', ['-C', f.root, 'config', '--local', '--get', 'core.hooksPath'], { encoding: 'utf8', shell: false, windowsHide: true });
    assert.equal(local.status, 1);
  } finally {
    if (previous === undefined) delete process.env.GIT_CONFIG_GLOBAL; else process.env.GIT_CONFIG_GLOBAL = previous;
    cleanup(f.directory);
  }
});
