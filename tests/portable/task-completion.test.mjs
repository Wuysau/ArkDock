import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, rmdirSync, symlinkSync, writeFileSync } from 'node:fs';
import { hostname, tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { beginTask, readyTask, finishTask, handleStop, isCodePath, recoverLock } from '../../tools/task-completion.mjs';

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
  const root = mkdtempSync(join(tmpdir(), 'arkdock-completion-test-'));
  const dev = join(root, 'dev'); const task = join(root, 'task');
  mkdirSync(dev); git(dev, 'init', '-b', 'dev');
  git(dev, 'config', 'user.name', 'Completion Fixture');
  git(dev, 'config', 'user.email', 'fixture@example.invalid');
  git(dev, 'config', 'core.autocrlf', 'false');
  writeFileSync(join(dev, '.gitignore'), 'artifacts/\n.local/\n');
  mkdirSync(join(dev, 'src')); writeFileSync(join(dev, 'src', 'app.ts'), 'export const value = 1;\n');
  git(dev, 'add', '.'); git(dev, 'commit', '-m', 'baseline');
  git(dev, 'worktree', 'add', '-b', 'codex/test', task, 'dev');
  return { root, dev, task, close: () => removeOwned(root) };
}
const taskOptions = f => ({ cwd: f.task, sessionId: 'fixture-session' });
const change = f => writeFileSync(join(f.task, 'src', 'app.ts'), 'export const value = 2;\n');

test('completed scoped Stop commits reviewed code, fast-forwards dev, preserves exe/cache/junction target, and is idempotent', () => {
  const f = fixture();
  try {
    const external = join(f.root, 'sdk'); mkdirSync(external);
    writeFileSync(join(external, 'keep.txt'), 'SDK must stay unchanged');
    beginTask(taskOptions(f)); change(f);
    mkdirSync(join(f.task, 'docs')); writeFileSync(join(f.task, 'docs', 'design.md'), 'local design excluded');
    mkdirSync(join(f.task, 'artifacts')); writeFileSync(join(f.task, 'artifacts', 'ArkDock.exe'), 'fixture binary');
    symlinkSync(external, join(f.task, 'artifacts', 'sdk-link'), process.platform === 'win32' ? 'junction' : 'dir');
    const ready = readyTask({ ...taskOptions(f), complete: true });
    assert.deepEqual(ready.reviewed.map(item => item.path), ['src/app.ts']);
    assert.deepEqual(ready.excluded, ['docs/design.md']);
    const done = handleStop({ hook_event_name: 'Stop', stop_hook_active: false, session_id: 'fixture-session', cwd: f.task }, { repo: f.dev });
    assert.equal(done.status, 'PASS'); assert.equal(done.worktreeRemoved, true);
    assert.equal(existsSync(f.task), false);
    assert.equal(readFileSync(join(f.dev, 'src', 'app.ts'), 'utf8'), 'export const value = 2;\n');
    assert.equal(readFileSync(join(done.archivePath, 'artifacts', 'ArkDock.exe'), 'utf8'), 'fixture binary');
    assert.equal(readFileSync(join(done.archivePath, 'docs', 'design.md'), 'utf8'), 'local design excluded');
    assert.equal(lstatSync(join(done.archivePath, 'artifacts', 'sdk-link')).isSymbolicLink(), true);
    assert.equal(readFileSync(join(external, 'keep.txt'), 'utf8'), 'SDK must stay unchanged');
    assert.doesNotMatch(git(f.dev, 'worktree', 'list', '--porcelain'), /refs\/heads\/codex\/test/);
    const completed = JSON.parse(readFileSync(join(f.dev, '.git', 'arkdock-task-completion', `${execFileSync(process.execPath, ['-e', "process.stdout.write(require('node:crypto').createHash('sha256').update('fixture-session').digest('hex'))"], { encoding: 'utf8' })}.json`), 'utf8'));
    assert.equal(existsSync(completed.gitDir), false);
    assert.equal(existsSync(completed.registrationArchive), true);
    assert.equal(readFileSync(join(completed.registrationArchive, 'gitdir'), 'utf8').trim().replaceAll('\\', '/'), join(f.task, '.git').replaceAll('\\', '/'));
    assert.equal(git(f.dev, 'show-ref', '--verify', 'refs/heads/codex/test').split(' ')[0], git(f.dev, 'rev-parse', 'dev'));
    assert.deepEqual(git(f.dev, 'show', '--pretty=format:', '--name-only', 'dev').split('\n'), ['src/app.ts']);
    const again = handleStop({ hook_event_name: 'Stop', session_id: 'fixture-session', cwd: f.task }, { repo: f.dev });
    assert.equal(again.idempotent, true); assert.equal(again.sourceCommit, done.sourceCommit);
  } finally { f.close(); }
});

test('stopping a status reply, paused task, another session, active hook, or unrelated repo never commits', () => {
  const f = fixture(); const unrelated = fixture();
  try {
    const before = git(f.dev, 'rev-parse', 'dev');
    assert.equal(handleStop({ hook_event_name: 'Stop', session_id: 'fixture-session', cwd: f.task }).status, 'NOOP');
    beginTask(taskOptions(f)); change(f);
    assert.equal(handleStop({ hook_event_name: 'Stop', session_id: 'fixture-session', cwd: f.task }).status, 'NOOP');
    assert.throws(() => readyTask(taskOptions(f)), /explicit task completion/);
    readyTask({ ...taskOptions(f), complete: true });
    for (const payload of [
      { hook_event_name: 'Stop', session_id: 'another-session', cwd: f.task },
      { hook_event_name: 'Stop', stop_hook_active: true, session_id: 'fixture-session', cwd: f.task },
      { hook_event_name: 'Notification', session_id: 'fixture-session', cwd: f.task },
      { hook_event_name: 'Stop', session_id: 'fixture-session', cwd: unrelated.task },
    ]) assert.equal(handleStop(payload).status, 'NOOP');
    assert.equal(handleStop({ hook_event_name: 'Stop', session_id: 'fixture-session', cwd: unrelated.task }, { repo: f.dev }).status, 'NOOP');
    assert.equal(git(f.dev, 'rev-parse', 'dev'), before);
    assert.equal(existsSync(f.task), true);
  } finally { f.close(); unrelated.close(); }
});

test('Stop from the registered primary chat checkout completes its exact session linked task', () => {
  const f = fixture();
  try {
    beginTask(taskOptions(f)); change(f); readyTask({ ...taskOptions(f), complete: true });
    const result = handleStop({ hook_event_name: 'Stop', stop_hook_active: false, session_id: 'fixture-session', cwd: f.dev }, { repo: f.dev });
    assert.equal(result.status, 'PASS'); assert.equal(existsSync(f.task), false);
    assert.equal(git(f.dev, 'rev-parse', 'HEAD'), result.sourceCommit);
  } finally { f.close(); }
});

test('begin refuses primary, dev, detached, and initially dirty task worktrees', () => {
  const f = fixture();
  try {
    assert.throws(() => beginTask({ cwd: f.dev, sessionId: 'fixture-session' }), /primary or dev/);
    change(f); assert.throws(() => beginTask(taskOptions(f)), /Tracked or untracked/);
    git(f.task, 'restore', 'src/app.ts'); git(f.task, 'checkout', '--detach');
    assert.throws(() => beginTask(taskOptions(f)), /git symbolic-ref/);
  } finally { f.close(); }
});

test('dirty dev, divergent dev, changed reviewed hashes, and a new unreviewed path block without touching dev or removing task', () => {
  for (const scenario of ['dev-dirty', 'dev-diverged', 'hash', 'path']) {
    const f = fixture();
    try {
      beginTask(taskOptions(f)); change(f);
      const before = git(f.dev, 'rev-parse', 'dev');
      if (scenario === 'dev-dirty') {
        writeFileSync(join(f.dev, 'other-user.txt'), 'another task owns this');
        assert.throws(() => readyTask({ ...taskOptions(f), complete: true }), /Tracked or untracked/);
      } else if (scenario === 'dev-diverged') {
        writeFileSync(join(f.dev, 'other-user.txt'), 'another task owns this'); git(f.dev, 'add', '.'); git(f.dev, 'commit', '-m', 'concurrent task');
        assert.throws(() => readyTask({ ...taskOptions(f), complete: true }), /dev diverged/);
      } else {
        readyTask({ ...taskOptions(f), complete: true });
        if (scenario === 'hash') writeFileSync(join(f.task, 'src', 'app.ts'), 'export const value = 3;\n');
        else writeFileSync(join(f.task, 'src', 'late.ts'), 'export const late = true;\n');
        assert.throws(() => finishTask(taskOptions(f)), /content changed|change set changed/);
      }
      if (scenario !== 'dev-diverged') assert.equal(git(f.dev, 'rev-parse', 'dev'), before);
      assert.equal(existsSync(f.task), true);
    } finally { f.close(); }
  }
});

test('dev becoming dirty after ready is blocked before even committing the task branch', () => {
  const f = fixture();
  try {
    beginTask(taskOptions(f)); change(f); readyTask({ ...taskOptions(f), complete: true });
    const before = git(f.task, 'rev-parse', 'HEAD');
    writeFileSync(join(f.dev, 'src', 'app.ts'), 'another task modification\n');
    assert.throws(() => finishTask(taskOptions(f)), /Tracked or untracked/);
    assert.equal(git(f.task, 'rev-parse', 'HEAD'), before);
    assert.equal(readFileSync(join(f.dev, 'src', 'app.ts'), 'utf8'), 'another task modification\n');
  } finally { f.close(); }
});

test('ready rejects escaped paths, explicit design files, secret content, staged excluded paths and junctions', () => {
  const f = fixture();
  try {
    beginTask(taskOptions(f)); change(f);
    assert.throws(() => readyTask({ ...taskOptions(f), complete: true, paths: ['../dev/src/app.ts'] }), /relative Git path/);
    mkdirSync(join(f.task, 'docs')); writeFileSync(join(f.task, 'docs', 'design.md'), 'design');
    assert.throws(() => readyTask({ ...taskOptions(f), complete: true, paths: ['docs/design.md'] }), /Design, images/);
    git(f.task, 'add', 'docs/design.md');
    assert.throws(() => readyTask({ ...taskOptions(f), complete: true }), /index contains/);
    git(f.task, 'restore', '--staged', 'docs/design.md');
    writeFileSync(join(f.task, 'src', 'app.ts'), ['-----BEGIN ', 'PRIVATE KEY-----'].join(''));
    assert.throws(() => readyTask({ ...taskOptions(f), complete: true }), /potential secret/);
    change(f);
    const external = join(f.root, 'external'); mkdirSync(external);
    writeFileSync(join(external, 'outside.ts'), 'export const outside = true;\n');
    symlinkSync(external, join(f.task, 'src', 'linked'), process.platform === 'win32' ? 'junction' : 'dir');
    assert.throws(() => readyTask({ ...taskOptions(f), complete: true, paths: ['src/linked/outside.ts'] }), /junction/);
    assert.equal(readFileSync(join(external, 'outside.ts'), 'utf8'), 'export const outside = true;\n');
  } finally { f.close(); }
});

test('source renames commit both sides without leaving a deletion outside the reviewed commit', () => {
  const f = fixture();
  try {
    beginTask(taskOptions(f)); git(f.task, 'mv', 'src/app.ts', 'src/renamed.ts');
    const ready = readyTask({ ...taskOptions(f), complete: true });
    assert.deepEqual(ready.reviewed.map(record => record.path), ['src/app.ts', 'src/renamed.ts']);
    const result = finishTask(taskOptions(f)); assert.equal(result.status, 'PASS');
    assert.equal(existsSync(join(f.dev, 'src', 'app.ts')), false);
    assert.equal(existsSync(join(f.dev, 'src', 'renamed.ts')), true);
  } finally { f.close(); }
});

test('a blocked archive retains its phase and retry does not duplicate the commit or delete a live path', () => {
  const f = fixture();
  try {
    const began = beginTask(taskOptions(f)); change(f);
    const ready = readyTask({ ...taskOptions(f), complete: true });
    const collision = join(f.root, 'task-archive-collision'); mkdirSync(collision);
    writeFileSync(join(collision, 'keep.txt'), 'unrelated directory must be preserved');
    writeFileSync(began.manifestPath, JSON.stringify({ ...ready, archivePath: collision }));
    assert.throws(() => finishTask(taskOptions(f)), /Both source and archive/);
    const commit = git(f.task, 'rev-parse', 'HEAD');
    assert.equal(git(f.dev, 'rev-parse', 'HEAD'), commit);
    assert.equal(readFileSync(join(collision, 'keep.txt'), 'utf8'), 'unrelated directory must be preserved');
    assert.equal(existsSync(f.task), true);
    const blocked = JSON.parse(readFileSync(began.manifestPath, 'utf8'));
    assert.equal(blocked.receipt.status, 'BLOCKED');
    assert.equal(blocked.phase, 'merged');
    // Only the owned test fixture's collision is cleared; product code never deletes it.
    removeOwned(collision);
    const retry = finishTask(taskOptions(f));
    assert.equal(retry.status, 'PASS'); assert.equal(retry.sourceCommit, commit);
    assert.equal(git(f.dev, 'rev-list', '--count', 'dev'), '2');
  } finally { f.close(); }
});

test('excluded or secret files manually committed after begin cannot enter dev through a whole-branch fast-forward', () => {
  for (const scenario of ['design', 'secret']) {
    const f = fixture();
    try {
      beginTask(taskOptions(f));
      const before = git(f.dev, 'rev-parse', 'dev');
      if (scenario === 'design') {
        mkdirSync(join(f.task, 'docs')); writeFileSync(join(f.task, 'docs', 'design.md'), 'excluded design checkpoint');
        git(f.task, 'add', 'docs/design.md');
      } else {
        writeFileSync(join(f.task, 'src', 'app.ts'), ['-----BEGIN ', 'PRIVATE KEY-----'].join(''));
        git(f.task, 'add', 'src/app.ts');
      }
      git(f.task, 'commit', '-m', 'manual checkpoint');
      change(f);
      assert.throws(() => readyTask({ ...taskOptions(f), complete: true }), scenario === 'design' ? /history contains an excluded/ : /potential secret/);
      assert.equal(git(f.dev, 'rev-parse', 'dev'), before);
      assert.equal(existsSync(f.task), true);
    } finally { f.close(); }
  }
});

test('reviewed legitimate code checkpoints and the final uncommitted code are both included in dev', () => {
  const f = fixture();
  try {
    beginTask(taskOptions(f)); change(f);
    git(f.task, 'add', 'src/app.ts'); git(f.task, 'commit', '-m', 'code checkpoint');
    const checkpoint = git(f.task, 'rev-parse', 'HEAD');
    writeFileSync(join(f.task, 'src', 'app.ts'), 'export const value = 3;\n');
    const ready = readyTask({ ...taskOptions(f), complete: true });
    assert.equal(ready.reviewedHistory.length, 1); assert.equal(ready.reviewedHistory[0].commit, checkpoint);
    assert.deepEqual(ready.reviewedHistory[0].files.map(item => item.path), ['src/app.ts']);
    const result = finishTask(taskOptions(f)); assert.equal(result.status, 'PASS');
    assert.equal(git(f.dev, 'rev-list', '--count', 'dev'), '3');
    assert.equal(readFileSync(join(f.dev, 'src', 'app.ts'), 'utf8'), 'export const value = 3;\n');
  } finally { f.close(); }
});

test('archive scope cannot cross into a repository or outside the task sibling directory', () => {
  const f = fixture();
  try {
    assert.throws(() => beginTask({ ...taskOptions(f), archiveRoot: join(f.dev, 'archives') }), /archive root must/);
    assert.throws(() => beginTask({ ...taskOptions(f), archiveRoot: f.dev }), /inside a registered worktree/);
  } finally { f.close(); }
});

test('explicit recover-lock confirms a departed owner, refuses alive or unknown identities, and removes no unrelated contents', () => {
  const f = fixture();
  try {
    const began = beginTask(taskOptions(f));
    const lock = join(f.dev, '.git', 'arkdock-task-completion', 'completion.lock');
    mkdirSync(lock);
    const owner = join(lock, 'owner.json');
    writeFileSync(owner, JSON.stringify({ pid: process.pid, host: hostname(), startedAt: new Date().toISOString() }));
    assert.throws(() => recoverLock({ cwd: f.dev }), /alive|cannot be inspected/);
    const departed = Number(execFileSync(process.execPath, ['-e', 'process.stdout.write(String(process.pid))'], { encoding: 'utf8' }));
    writeFileSync(owner, JSON.stringify({ pid: departed, host: 'other-host' }));
    assert.throws(() => recoverLock({ cwd: f.dev }), /another host/);
    writeFileSync(owner, JSON.stringify({ pid: departed, host: hostname() }));
    writeFileSync(join(lock, 'unrelated.txt'), 'preserve');
    assert.throws(() => recoverLock({ cwd: f.dev }), /unexpected files/);
    assert.equal(readFileSync(join(lock, 'unrelated.txt'), 'utf8'), 'preserve');
    removeOwned(join(lock, 'unrelated.txt'));
    assert.equal(recoverLock({ cwd: f.dev }).status, 'PASS');
    assert.equal(existsSync(lock), false);
    assert.equal(existsSync(began.manifestPath), true);
    mkdirSync(lock);
    assert.throws(() => recoverLock({ cwd: f.dev }), /owner is unknown/);
  } finally { f.close(); }
});

test('code allowlist excludes design, raster output, Harness, secret names, and executable artifacts', () => {
  for (const path of ['docs/design/x.md', 'AGENTS.md', '.codex/hooks.json', 'src/screen.png', 'tools/verify-harness.mjs', 'src/.env', 'artifacts/a.exe']) assert.equal(isCodePath(path), false);
  for (const path of ['src/app.tsx', 'src-tauri/Cargo.toml', 'src-tauri/.gitignore', 'protocol/model.json', 'tools/task-completion.mjs', 'tests/portable/a.test.mjs', 'package.json', 'pnpm-lock.yaml', '.gitignore', '.github/workflows/verify.yaml']) assert.equal(isCodePath(path), true);
});
