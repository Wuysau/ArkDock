import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { autoCommitTask, beginTask, finishTask, handleStopCommit, isCodePath, readyTask } from '../../tools/task-completion.mjs';

function git(cwd, ...args) {
  return execFileSync('git', ['-C', cwd, ...args], { encoding: 'utf8', shell: false, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}
function removeOwned(path) {
  let stat;
  try { stat = lstatSync(path); } catch (error) { if (error.code === 'ENOENT') return; throw error; }
  if (stat.isSymbolicLink() || !stat.isDirectory()) { rmSync(path, { force: true }); return; }
  for (const child of readdirSync(path)) removeOwned(join(path, child));
  rmdirSync(path);
}
function fixture() {
  const root = mkdtempSync(join(tmpdir(), 'arkdock-autocommit-test-'));
  const dev = join(root, 'dev'); const task = join(root, 'task');
  mkdirSync(dev); git(dev, 'init', '-b', 'dev');
  git(dev, 'config', 'user.name', 'Autocommit Fixture');
  git(dev, 'config', 'user.email', 'fixture@example.invalid');
  git(dev, 'config', 'core.autocrlf', 'false');
  writeFileSync(join(dev, '.gitignore'), 'artifacts/\n.local/\n');
  mkdirSync(join(dev, 'src')); mkdirSync(join(dev, 'docs'));
  writeFileSync(join(dev, 'src', 'app.ts'), 'export const value = 1;\n');
  writeFileSync(join(dev, 'docs', 'design.md'), 'baseline design\n');
  writeFileSync(join(dev, 'AGENTS.md'), 'baseline instructions\n');
  git(dev, 'add', '.'); git(dev, 'commit', '-m', 'baseline');
  git(dev, 'worktree', 'add', '-b', 'codex/autocommit', task, 'dev');
  return { root, dev, task, close: () => removeOwned(root) };
}
function changeCode(f) { writeFileSync(join(f.task, 'src', 'app.ts'), 'export const value = 2;\n'); }
function changeDocs(f) { writeFileSync(join(f.task, 'docs', 'design.md'), 'rewritten design\n'); }

test('Stop auto-commit records changed code and configuration and never touches design or instruction files', () => {
  const f = fixture();
  try {
    const devHead = git(f.dev, 'rev-parse', 'dev');
    const baseline = git(f.task, 'rev-parse', 'HEAD');
    changeCode(f); changeDocs(f);
    writeFileSync(join(f.task, 'AGENTS.md'), 'rewritten instructions\n');
    writeFileSync(join(f.task, 'src', 'settings.json'), '{\n  "enabled": true\n}\n');
    const result = autoCommitTask({ cwd: f.task, stamp: '2026-01-01T00:00:00.000Z' });
    assert.equal(result.status, 'PASS');
    assert.equal(result.branch, 'codex/autocommit');
    assert.deepEqual(result.files, ['src/app.ts', 'src/settings.json']);
    assert.deepEqual(result.excluded.sort(), ['AGENTS.md', 'docs/design.md']);
    assert.equal(result.commit, git(f.task, 'rev-parse', 'HEAD'));
    assert.equal(git(f.task, 'rev-parse', 'HEAD^'), baseline);
    assert.equal(git(f.task, 'log', '-1', '--format=%s'), 'chore(autocommit): codex/autocommit 2026-01-01T00:00:00.000Z');
    // Only code and configuration landed in the commit; design and instruction files stay uncommitted.
    assert.deepEqual(git(f.task, 'show', '--pretty=format:', '--name-only', 'HEAD').split('\n').sort(), ['src/app.ts', 'src/settings.json']);
    // Design and instruction edits stay in the working tree and nothing unrelated is staged.
    assert.deepEqual(git(f.task, 'diff', '--name-only', '-z').split('\0').filter(Boolean).sort(), ['AGENTS.md', 'docs/design.md']);
    assert.equal(git(f.task, 'diff', '--cached', '--name-only'), '');
    // dev is never advanced by an auto-commit.
    assert.equal(git(f.dev, 'rev-parse', 'dev'), devHead);
    for (const path of result.files) assert.equal(isCodePath(path), true);
  } finally { f.close(); }
});

test('staged design files are never swept into an auto-commit', () => {
  const f = fixture();
  try {
    changeDocs(f); git(f.task, 'add', '--', 'docs/design.md');
    changeCode(f);
    const result = autoCommitTask({ cwd: f.task });
    assert.equal(result.status, 'PASS');
    assert.deepEqual(git(f.task, 'show', '--pretty=format:', '--name-only', 'HEAD').split('\n'), ['src/app.ts']);
    assert.equal(git(f.task, 'diff', '--cached', '--name-only'), 'docs/design.md');
  } finally { f.close(); }
});

test('the primary checkout, dev, main, and clean or design-only worktrees are never auto-committed', () => {
  const f = fixture();
  try {
    assert.equal(autoCommitTask({ cwd: f.dev }).reason, 'The primary checkout is never auto-committed.');
    const primary = git(f.dev, 'rev-parse', 'HEAD');
    changeCode(f);
    assert.equal(autoCommitTask({ cwd: f.task }).status, 'PASS');
    const clean = autoCommitTask({ cwd: f.task });
    assert.equal(clean.status, 'NOOP');
    assert.equal(clean.reason, 'No changes to commit.');
    changeDocs(f);
    const designOnly = autoCommitTask({ cwd: f.task });
    assert.equal(designOnly.status, 'NOOP');
    assert.deepEqual(designOnly.excluded, ['docs/design.md']);
    assert.equal(git(f.task, 'rev-parse', 'HEAD^'), primary);
    git(f.task, 'checkout', '--detach');
    assert.equal(handleStopCommit({ hook_event_name: 'Stop', session_id: 'fixture-session', cwd: f.task }).status, 'NOOP');
  } finally { f.close(); }
});

test('ineligible Stop events, other repositories, and in-flight completion transactions are ignored', () => {
  const f = fixture(); const other = fixture();
  try {
    for (const payload of [
      null,
      { hook_event_name: 'Notification', session_id: 'fixture-session', cwd: f.task },
      { hook_event_name: 'Stop', stop_hook_active: true, session_id: 'fixture-session', cwd: f.task },
    ]) assert.equal(handleStopCommit(payload).status, 'NOOP');
    changeCode(other);
    assert.equal(handleStopCommit({ hook_event_name: 'Stop', session_id: 'fixture-session', cwd: other.task }).status, 'PASS');
    beginTask({ cwd: f.task, sessionId: 'fixture-session' });
    changeCode(f);
    assert.equal(handleStopCommit({ hook_event_name: 'Stop', session_id: 'fixture-session', cwd: f.task }).status, 'PASS');
    readyTask({ cwd: f.task, sessionId: 'fixture-session', complete: true });
    const inFlight = handleStopCommit({ hook_event_name: 'Stop', session_id: 'fixture-session', cwd: f.task });
    assert.equal(inFlight.status, 'NOOP');
    assert.match(inFlight.reason, /completion transaction is in flight/);
    const done = finishTask({ cwd: f.task, sessionId: 'fixture-session' });
    assert.equal(done.status, 'PASS');
    assert.equal(existsSync(f.task), false);
  } finally { f.close(); other.close(); }
});

test('auto-committed code still completes through the reviewed lifecycle', () => {
  const f = fixture();
  try {
    beginTask({ cwd: f.task, sessionId: 'fixture-session' });
    changeCode(f); changeDocs(f);
    assert.equal(autoCommitTask({ cwd: f.task }).status, 'PASS');
    const ready = readyTask({ cwd: f.task, sessionId: 'fixture-session', complete: true });
    assert.deepEqual(ready.reviewed, []);
    assert.deepEqual(ready.excluded, ['docs/design.md']);
    const done = finishTask({ cwd: f.task, sessionId: 'fixture-session' });
    assert.equal(done.status, 'PASS');
    assert.equal(git(f.dev, 'rev-parse', 'dev'), done.sourceCommit);
    assert.equal(readFileSync(join(f.dev, 'src', 'app.ts'), 'utf8'), 'export const value = 2;\n');
    assert.equal(readFileSync(join(f.dev, 'docs', 'design.md'), 'utf8'), 'baseline design\n');
  } finally { f.close(); }
});
