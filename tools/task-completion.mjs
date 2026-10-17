// Explicit completion markers are required: a chat ending is not a completed task.
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import {
  existsSync, lstatSync, mkdirSync, readFileSync, realpathSync, renameSync,
  readdirSync, rmdirSync, unlinkSync, writeFileSync,
} from 'node:fs';
import { hostname } from 'node:os';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

export class CompletionBlocked extends Error {
  constructor(message) { super(`BLOCKED: ${message}`); this.name = 'CompletionBlocked'; }
}
const fail = message => { throw new CompletionBlocked(message); };
const digest = value => createHash('sha256').update(value).digest('hex');
const key = value => process.platform === 'win32' ? value.toLowerCase() : value;
const same = (a, b) => key(resolve(a)) === key(resolve(b));
const inside = (root, path) => {
  const result = relative(key(resolve(root)), key(resolve(path)));
  return result === '' || (!isAbsolute(result) && result !== '..' && !result.startsWith(`..${sep}`));
};
function git(cwd, args, options = {}) {
  try {
    return execFileSync('git', ['--literal-pathspecs', '-C', cwd, ...args], {
      encoding: 'utf8', shell: false, windowsHide: true, timeout: 60_000,
      stdio: ['ignore', 'pipe', 'pipe'], env: { ...process.env, GIT_TERMINAL_PROMPT: '0' }, ...options,
    });
  } catch (error) {
    const detail = String(error.stderr ?? error.message).trim().slice(0, 1200);
    fail(`git ${args[0]} failed: ${detail}`);
  }
}
function ancestor(cwd, older, newer) {
  try {
    execFileSync('git', ['-C', cwd, 'merge-base', '--is-ancestor', older, newer], {
      shell: false, windowsHide: true, stdio: 'ignore', timeout: 15_000,
    });
    return true;
  } catch { return false; }
}
function plainDirectory(path) {
  if (!existsSync(path)) fail(`Directory does not exist: ${path}`);
  const stat = lstatSync(path);
  if (stat.isSymbolicLink() || !stat.isDirectory()) fail(`Directory must not be a link: ${path}`);
  return realpathSync(path);
}
function repository(cwd) {
  if (!cwd || !existsSync(cwd)) fail('The requested repository directory does not exist.');
  const root = plainDirectory(git(resolve(cwd), ['rev-parse', '--show-toplevel']).trim());
  const common = plainDirectory(git(root, ['rev-parse', '--path-format=absolute', '--git-common-dir']).trim());
  const gitDir = plainDirectory(git(root, ['rev-parse', '--path-format=absolute', '--git-dir']).trim());
  const branch = git(root, ['symbolic-ref', '--quiet', '--short', 'HEAD']).trim();
  if (!branch) fail('A detached HEAD cannot register a task.');
  return { root, common, gitDir, branch, head: git(root, ['rev-parse', 'HEAD']).trim() };
}
function registrations(cwd) {
  const records = [];
  let current = null;
  for (const field of git(cwd, ['worktree', 'list', '--porcelain', '-z']).split('\0')) {
    if (field.startsWith('worktree ')) {
      current = { path: field.slice(9) }; records.push(current);
    } else if (current && field.startsWith('branch ')) current.branch = field.slice(7);
    else if (current && field.startsWith('HEAD ')) current.head = field.slice(5);
    else if (current && field === 'bare') current.bare = true;
    else if (current && field.startsWith('locked')) current.locked = true;
  }
  return records;
}
function inProgress(repo) {
  for (const name of ['MERGE_HEAD', 'CHERRY_PICK_HEAD', 'REVERT_HEAD', 'rebase-merge', 'rebase-apply', 'sequencer']) {
    if (existsSync(join(repo.gitDir, name))) fail(`An operation is in progress in ${repo.root}: ${name}`);
  }
}
function clean(repo) {
  inProgress(repo);
  if (git(repo.root, ['status', '--porcelain=v1', '-z', '--untracked-files=all'])) {
    fail(`Tracked or untracked changes exist in ${repo.root}; they will not be stashed or committed.`);
  }
}
function destination(repo, devBranch) {
  const matches = registrations(repo.root).filter(record => record.branch === `refs/heads/${devBranch}`);
  if (matches.length !== 1 || matches[0].locked || matches[0].bare) fail(`${devBranch} must have exactly one unlocked registered worktree.`);
  const dev = repository(matches[0].path);
  if (!same(dev.common, repo.common) || dev.branch !== devBranch) fail('The destination belongs to another repository or branch.');
  return dev;
}
function session(value) {
  const result = value ?? process.env.CODEX_THREAD_ID;
  if (typeof result !== 'string' || !result.trim() || result.length > 256 || ['\0', '\r', '\n'].some(char => result.includes(char))) fail('A valid session ID or CODEX_THREAD_ID is required.');
  return result;
}
function metadata(common) {
  const path = join(common, 'arkdock-task-completion');
  if (!existsSync(path)) mkdirSync(path);
  return plainDirectory(path);
}
function statePath(common, id) { return join(metadata(common), `${digest(id)}.json`); }
function load(common, id) {
  const path = statePath(common, id);
  if (!existsSync(path)) return null;
  const stat = lstatSync(path);
  if (stat.isSymbolicLink() || !stat.isFile()) fail('The task manifest is not a regular file.');
  const value = JSON.parse(readFileSync(path, 'utf8'));
  if (value.version !== 1 || value.sessionId !== id || !same(value.common, common)) fail('The task manifest identity is invalid.');
  return value;
}
function save(manifest) {
  const path = statePath(manifest.common, manifest.sessionId);
  const temporary = `${path}.${process.pid}.tmp`;
  writeFileSync(temporary, `${JSON.stringify(manifest, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  renameSync(temporary, path);
}
function locked(common, action) {
  const path = join(metadata(common), 'completion.lock');
  try { mkdirSync(path); } catch { fail('Another completion owns this repository lock; retry after it finishes.'); }
  const owner = { pid: process.pid, host: hostname(), startedAt: new Date().toISOString() };
  const ownerPath = join(path, 'owner.json');
  writeFileSync(ownerPath, JSON.stringify(owner), { flag: 'wx', mode: 0o600 });
  try { return action(); } finally {
    const actual = JSON.parse(readFileSync(ownerPath, 'utf8'));
    if (!sameRecords(actual, owner)) fail('Completion lock owner changed; preserving the lock.');
    unlinkSync(ownerPath); rmdirSync(path);
  }
}
export function recoverLock({ cwd = process.cwd(), repo: repoPath } = {}) {
  const repo = repository(repoPath ?? cwd);
  const path = join(metadata(repo.common), 'completion.lock');
  if (!existsSync(path)) return { status: 'NOOP', reason: 'No completion lock exists.' };
  plainDirectory(path);
  const ownerPath = join(path, 'owner.json');
  if (!existsSync(ownerPath) || lstatSync(ownerPath).isSymbolicLink() || !lstatSync(ownerPath).isFile()) fail('Lock owner is unknown; automatic recovery is forbidden.');
  const bytes = readFileSync(ownerPath);
  const owner = JSON.parse(bytes.toString('utf8'));
  if (owner.host !== hostname() || !Number.isSafeInteger(owner.pid) || owner.pid <= 0) fail('Lock process identity is unknown or belongs to another host.');
  try { process.kill(owner.pid, 0); fail('The completion lock owner is still alive.'); }
  catch (error) { if (error.code !== 'ESRCH') fail('The completion lock process is alive or cannot be inspected.'); }
  if (readdirSync(path).some(name => name !== 'owner.json')) fail('The stale lock contains unexpected files; nothing was removed.');
  if (!readFileSync(ownerPath).equals(bytes)) fail('The lock owner changed during inspection.');
  unlinkSync(ownerPath); rmdirSync(path);
  return { status: 'PASS', reason: 'Only an empty stale lock with a confirmed departed owner was removed.', pid: owner.pid };
}
function archiveParent(taskRoot, requested) {
  const sharedParent = plainDirectory(dirname(taskRoot));
  const parent = resolve(requested ?? sharedParent);
  // Same directory or one explicit sibling directory; never inside a worktree.
  if (!same(parent, sharedParent) && !same(dirname(parent), sharedParent)) fail('The archive root must be the task parent or one explicit sibling directory.');
  if (same(parent, taskRoot)) fail('The archive root cannot be the task directory.');
  if (existsSync(parent)) return plainDirectory(parent);
  if (!same(dirname(parent), sharedParent)) fail('Archive root parent is outside the approved scope.');
  mkdirSync(parent);
  return plainDirectory(parent);
}
export function beginTask({ cwd = process.cwd(), sessionId, archiveRoot, devBranch = 'dev' } = {}) {
  const id = session(sessionId);
  const repo = repository(cwd);
  return locked(repo.common, () => {
    if (same(repo.gitDir, repo.common) || repo.branch === devBranch) fail('Register a linked task worktree, never the primary or dev worktree.');
    const registered = registrations(repo.root).filter(record => same(record.path, repo.root));
    if (registered.length !== 1 || registered[0].locked || registered[0].branch !== `refs/heads/${repo.branch}`) fail('The source is not an unlocked registered task worktree.');
    clean(repo);
    const dev = destination(repo, devBranch);
    inProgress(dev);
    const previous = load(repo.common, id);
    if (previous && previous.phase !== 'complete') {
      if (same(previous.taskRoot, repo.root) && previous.branch === repo.branch && previous.baseline === repo.head) return previous;
      fail('This session already owns an unfinished task; do not replace its manifest.');
    }
    const parent = archiveParent(repo.root, archiveRoot);
    for (const record of registrations(repo.root)) {
      if (inside(record.path, parent)) fail('The archive root is inside a registered worktree.');
    }
    const manifest = {
      version: 1, sessionId: id, common: repo.common, gitDir: repo.gitDir,
      taskRoot: repo.root, branch: repo.branch, baseline: repo.head,
      adminIdentity: directoryIdentity(repo.gitDir),
      devBranch, devRoot: dev.root, devBaseline: dev.head, archiveRoot: parent,
      startedAt: new Date().toISOString(), phase: 'begun', complete: false,
    };
    save(manifest);
    return { ...manifest, manifestPath: statePath(repo.common, id) };
  });
}

const deniedPaths = /(^|\/)(docs|design|references|\.codex|\.agents|\.local|artifacts|node_modules|target|dist|\.git)(\/|$)|(^|\/)(AGENTS|CLAUDE|ENVIRONMENT)\.md$|(^|\/)(\.env(?:\..*)?|credentials?(?:\.[^/]*)?|secrets?(?:\.[^/]*)?)$/i;
const deniedExtensions = /\.(png|jpe?g|gif|webp|bmp|ico|pdf|docx?|pptx?|exe|dll|pdb|zip|7z|pfx|p12|pem|key|log)$/i;
const sourceRoots = /^(src|src-tauri|protocol|tools|tests|examples|\.github)\//;
const sourceExtensions = /\.(rs|tsx?|jsx?|mjs|cjs|css|html|json|json5|toml|ya?ml|lock|ps1|sh|cmd|bat|h|hpp|c|cpp|ets|txt)$/i;
export function isCodePath(path) {
  if (deniedPaths.test(path) || deniedExtensions.test(path) || /(^|\/)verify-harness\.mjs$/i.test(path)) return false;
  return sourceRoots.test(path) && (sourceExtensions.test(path) || ['.gitignore', '.gitattributes', '.editorconfig'].includes(basename(path)))
    || /^[^/]+\.(json|toml|mjs|cjs|ts|yaml|yml|lock|html)$/.test(path)
    || ['.gitignore', '.gitattributes', '.editorconfig', 'Cargo.lock'].includes(path);
}
function safePath(root, path) {
  if (typeof path !== 'string' || !path || ['\0', '\r', '\n', '\\'].some(char => path.includes(char)) || isAbsolute(path) || path.split('/').some(part => !part || part === '.' || part === '..')) fail('A reviewed path must be a plain relative Git path.');
  const absolute = resolve(root, ...path.split('/'));
  if (!inside(root, absolute) || same(root, absolute)) fail('The reviewed path escapes the task directory.');
  let current = root;
  for (const part of path.split('/')) {
    current = join(current, part);
    if (existsSync(current) && lstatSync(current).isSymbolicLink()) fail(`A reviewed path crosses a symbolic link or junction: ${path}`);
  }
  return absolute;
}
function changes(root) {
  const paths = [...git(root, ['diff', '--no-renames', '--name-only', '-z', 'HEAD']).split('\0'),
    ...git(root, ['ls-files', '--others', '--exclude-standard', '-z']).split('\0')];
  return [...new Set(paths.filter(Boolean))].sort();
}
function staged(root) { return git(root, ['diff', '--cached', '--name-only', '-z']).split('\0').filter(Boolean); }
function checkCodeBytes(bytes, path) {
  const text = bytes.toString('utf8');
  if (bytes.includes(0) || /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----|\bAKIA[A-Z0-9]{16}\b|\bgh[pousr]_[A-Za-z0-9]{36,}\b|\bsk-[A-Za-z0-9]{32,}\b/.test(text)) fail(`A reviewed code file contains binary or potential secret content: ${path}`);
}
function fileRecord(root, path) {
  const absolute = safePath(root, path);
  if (!existsSync(absolute)) return { path, kind: 'deleted', sha256: digest('deleted') };
  const stat = lstatSync(absolute);
  if (!stat.isFile() || stat.size > 8 * 1024 * 1024) fail(`A reviewed source must be a regular file under 8 MiB: ${path}`);
  const bytes = readFileSync(absolute);
  checkCodeBytes(bytes, path);
  return { path, kind: 'file', sha256: digest(bytes), mode: stat.mode & 0o777 };
}
function auditHistory(repo, dev) {
  const history = [];
  const commits = git(repo.root, ['rev-list', `${dev.head}..${repo.head}`]).trim().split('\n').filter(Boolean);
  for (const commit of commits) {
    const paths = [...new Set(git(repo.root, ['diff-tree', '--root', '--no-commit-id', '--no-renames', '--name-only', '-r', '-m', '-z', commit]).split('\0').filter(Boolean))].sort();
    const files = [];
    for (const path of paths) {
      safePath(repo.root, path);
      if (!isCodePath(path)) fail(`Existing task history contains an excluded design, image, Harness, or unsupported path: ${path}`);
      const entry = git(repo.root, ['ls-tree', '-z', commit, '--', path]);
      if (!entry) { files.push({ path, kind: 'deleted' }); continue; }
      const match = entry.match(/^(100644|100755) blob ([0-9a-f]+)\t/);
      if (!match) fail(`Existing task history contains a non-regular source blob: ${path}`);
      const size = Number(git(repo.root, ['cat-file', '-s', match[2]]).trim());
      if (!Number.isSafeInteger(size) || size > 8 * 1024 * 1024) fail(`Historical source blob exceeds 8 MiB: ${path}`);
      const bytes = git(repo.root, ['cat-file', 'blob', match[2]], { encoding: null, maxBuffer: 9 * 1024 * 1024 });
      checkCodeBytes(bytes, path);
      files.push({ path, kind: 'file', mode: match[1], sha256: digest(bytes), blob: match[2] });
    }
    history.push({ commit, files });
  }
  return history;
}
function verifySource(manifest, expectedHead = manifest.reviewedHead) {
  const repo = repository(manifest.taskRoot);
  if (!same(repo.root, manifest.taskRoot) || !same(repo.common, manifest.common) || !same(repo.gitDir, manifest.gitDir)
    || repo.branch !== manifest.branch || repo.head !== expectedHead) fail('The task directory, repository, branch, or HEAD changed after review.');
  inProgress(repo);
  const found = registrations(repo.root).filter(record => same(record.path, repo.root) && record.branch === `refs/heads/${manifest.branch}`);
  if (found.length !== 1 || found[0].locked) fail('The task registration changed after review.');
  return repo;
}
function sameRecords(a, b) { return JSON.stringify(a) === JSON.stringify(b); }
function verifyReview(manifest) {
  const current = changes(manifest.taskRoot);
  const expected = [...manifest.reviewed.map(record => record.path), ...manifest.excluded].sort();
  if (!sameRecords(current, expected)) fail('The task change set changed after review; create a new ready marker.');
  if (!sameRecords(manifest.reviewed.map(record => fileRecord(manifest.taskRoot, record.path)), manifest.reviewed)) fail('Reviewed code content changed; create a new ready marker.');
  if (staged(manifest.taskRoot).some(path => !manifest.reviewed.some(record => record.path === path))) fail('The index contains an unreviewed path.');
}
export function readyTask({ cwd = process.cwd(), sessionId, complete = false, paths, message } = {}) {
  if (complete !== true) fail('Ready requires explicit task completion; pauses and status replies cannot complete a task.');
  const id = session(sessionId);
  const repo = repository(cwd);
  return locked(repo.common, () => {
    const manifest = load(repo.common, id);
    if (!manifest || !same(manifest.taskRoot, repo.root) || manifest.branch !== repo.branch) fail('Begin this session in this linked task worktree before marking it ready.');
    if (!['begun', 'ready'].includes(manifest.phase)) fail('A transaction already started; retry finish without replacing its review.');
    inProgress(repo);
    if (!ancestor(repo.root, manifest.baseline, repo.head)) fail('The task branch no longer descends from its registered baseline.');
    const all = changes(repo.root);
    const selected = paths ?? all.filter(isCodePath);
    if (!Array.isArray(selected) || new Set(selected).size !== selected.length) fail('Review paths must be a unique array.');
    for (const path of selected) {
      safePath(repo.root, path);
      if (!isCodePath(path)) fail(`Design, images, private Harness, or unsupported source cannot be committed: ${path}`);
      if (!all.includes(path)) fail(`The reviewed path is not changed: ${path}`);
    }
    // Omitting changed code is not an implicit permission to silently archive it.
    if (all.some(path => isCodePath(path) && !selected.includes(path))) fail('Review all changed code files before completing the task.');
    if (staged(repo.root).some(path => !selected.includes(path))) fail('The index contains an unreviewed or excluded path.');
    const dev = destination(repo, manifest.devBranch);
    clean(dev);
    if (!same(dev.root, manifest.devRoot)) fail('The destination worktree changed after begin.');
    if (!ancestor(repo.root, dev.head, repo.head)) fail('dev diverged; reconcile the task branch deliberately before completion.');
    manifest.reviewedHistory = auditHistory(repo, dev);
    manifest.reviewed = selected.slice().sort().map(path => fileRecord(repo.root, path));
    manifest.excluded = all.filter(path => !selected.includes(path));
    manifest.reviewedHead = repo.head;
    manifest.devHead = dev.head;
    manifest.message = message ?? `Complete task on ${manifest.branch}`;
    if (typeof manifest.message !== 'string' || !manifest.message.trim() || manifest.message.length > 4000 || manifest.message.includes('\0')) fail('A valid commit message is required.');
    manifest.complete = true;
    manifest.phase = 'ready';
    manifest.readyAt = new Date().toISOString();
    save(manifest);
    return manifest;
  });
}
function runningExecutables(root) {
  if (process.platform !== 'win32') return [];
  try {
    const output = execFileSync('pwsh', ['-NoProfile', '-NonInteractive', '-Command',
      'ConvertTo-Json -InputObject @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath } | Select-Object ProcessId,ExecutablePath) -Compress'],
    { encoding: 'utf8', shell: false, windowsHide: true, timeout: 20_000 });
    return JSON.parse(output).filter(item => item.ExecutablePath && inside(root, item.ExecutablePath))
      .map(item => ({ pid: item.ProcessId, executable: item.ExecutablePath }));
  } catch { fail('Cannot safely inspect processes before preserving and unregistering the task directory.'); }
}
function ensureNoProcesses(root) {
  const processes = runningExecutables(root);
  if (processes.length) fail(`A process is executing from this task worktree (${processes.map(item => item.pid).join(', ')}); close it yourself and retry. No process was stopped.`);
}
function validateDestination(manifest, { merged = false } = {}) {
  const dev = destination({ root: manifest.devRoot, common: manifest.common }, manifest.devBranch);
  if (!same(dev.root, manifest.devRoot)) fail('The registered dev worktree moved.');
  clean(dev);
  const expected = merged ? manifest.sourceCommit : manifest.devHead;
  if (dev.head !== expected) fail('dev HEAD changed during completion; its other work is preserved.');
  return dev;
}
function checkArchiveScope(manifest) {
  const parent = archiveParent(manifest.taskRoot, manifest.archiveRoot);
  if (!same(parent, manifest.archiveRoot)) fail('The archive root changed.');
  for (const record of registrations(manifest.devRoot)) {
    if (inside(record.path, parent)) fail('The archive root is inside a registered worktree.');
  }
  if (manifest.archivePath && (!same(dirname(manifest.archivePath), parent) || inside(manifest.taskRoot, manifest.archivePath))) fail('The planned archive path is outside its approved sibling root.');
}
function directoryIdentity(path) {
  const stat = lstatSync(plainDirectory(path));
  return { device: stat.dev, inode: stat.ino, birthtimeMs: stat.birthtimeMs };
}
function validateRegistration(manifest, path = manifest.gitDir) {
  const directParent = join(manifest.common, 'worktrees');
  if (!same(dirname(manifest.gitDir), directParent) || same(manifest.gitDir, directParent)) fail('The task admin directory is not a direct child of common/worktrees.');
  if (!sameRecords(directoryIdentity(path), manifest.adminIdentity)) fail('The registered admin directory lifecycle changed.');
  const regularText = name => {
    const file = join(path, name);
    const stat = lstatSync(file);
    if (stat.isSymbolicLink() || !stat.isFile()) fail('The task admin identity file is not regular.');
    return readFileSync(file, 'utf8').trim();
  };
  if (!same(regularText('gitdir'), join(manifest.taskRoot, '.git')) || regularText('HEAD') !== `ref: refs/heads/${manifest.branch}`) fail('The registered admin directory no longer identifies this task branch and path.');
  if (git(manifest.devRoot, ['rev-parse', `refs/heads/${manifest.branch}`]).trim() !== manifest.sourceCommit) fail('The task branch tip changed before registration archive.');
}
function writeReceipt(manifest, status, detail) {
  manifest.receipt = { status, phase: manifest.phase, at: new Date().toISOString(), detail,
    sourceCommit: manifest.sourceCommit ?? null, devHead: manifest.mergedHead ?? null,
    archivePath: manifest.archivePath ?? null, worktreeRemoved: manifest.phase === 'complete' };
  save(manifest);
  return manifest.receipt;
}
export function finishTask({ cwd = process.cwd(), sessionId, repo: repoPath } = {}) {
  const id = session(sessionId);
  const located = repository(repoPath ?? cwd);
  return locked(located.common, () => {
    const manifest = load(located.common, id);
    if (!manifest || !manifest.complete) return { status: 'NOOP', reason: 'No completed task marker for this session.' };
    if (manifest.phase === 'complete') return { ...manifest.receipt, idempotent: true };
    if (existsSync(cwd) && !same(repository(cwd).root, manifest.taskRoot) && !repoPath) fail('Finish must target the registered task worktree.');
    try {
      checkArchiveScope(manifest);
      if (['ready', 'prepared'].includes(manifest.phase)) {
        const actual = repository(manifest.taskRoot);
        // Recover a commit whose durable receipt was interrupted after Git succeeded.
        if (manifest.phase === 'prepared' && actual.head !== manifest.reviewedHead) {
          const parent = git(actual.root, ['rev-parse', 'HEAD^']).trim();
          const tree = git(actual.root, ['rev-parse', 'HEAD^{tree}']).trim();
          const body = git(actual.root, ['log', '-1', '--format=%B']).trim();
          if (parent !== manifest.reviewedHead || tree !== manifest.preparedTree || !body.endsWith(`ArkDock-Task-ID: ${digest(id)}`)) fail('HEAD changed independently of the prepared task commit.');
          manifest.sourceCommit = actual.head; manifest.phase = 'committed'; save(manifest);
        } else {
          verifySource(manifest);
          verifyReview(manifest);
          validateDestination(manifest);
          ensureNoProcesses(manifest.taskRoot);
          if (manifest.reviewed.length) {
            const files = manifest.reviewed.filter(record => record.kind === 'file').map(record => record.path);
            if (files.length) git(manifest.taskRoot, ['add', '--', ...files]);
            for (const record of manifest.reviewed.filter(item => item.kind === 'deleted')) {
              // A staged rename may already have removed this path from the index.
              git(manifest.taskRoot, ['update-index', '--force-remove', '--', record.path]);
            }
            verifyReview(manifest);
            manifest.preparedTree = git(manifest.taskRoot, ['write-tree']).trim();
            manifest.phase = 'prepared'; save(manifest);
            git(manifest.taskRoot, ['-c', 'commit.gpgSign=false', 'commit', '-m', `${manifest.message}\n\nArkDock-Task-ID: ${digest(id)}`]);
          }
          manifest.sourceCommit = git(manifest.taskRoot, ['rev-parse', 'HEAD']).trim();
          manifest.phase = 'committed'; save(manifest);
          if (manifest.preparedTree && git(manifest.taskRoot, ['rev-parse', 'HEAD^{tree}']).trim() !== manifest.preparedTree) {
            fail('An existing Git commit hook changed the reviewed tree; dev and the worktree are preserved for review.');
          }
        }
      }
      if (manifest.phase === 'committed') {
        verifySource(manifest, manifest.sourceCommit);
        if (manifest.preparedTree && git(manifest.taskRoot, ['rev-parse', 'HEAD^{tree}']).trim() !== manifest.preparedTree) fail('The task commit is not the reviewed tree.');
        if (changes(manifest.taskRoot).some(path => !manifest.excluded.includes(path))) fail('New source changes appeared after the task commit.');
        const currentDev = destination({ root: manifest.devRoot, common: manifest.common }, manifest.devBranch);
        if (currentDev.head === manifest.sourceCommit) {
          clean(currentDev); // Recovery after a successful merge with an interrupted receipt.
        } else {
          const dev = validateDestination(manifest);
          if (!ancestor(dev.root, dev.head, manifest.sourceCommit)) fail('Only a fast-forward merge into dev is allowed; divergent work is preserved.');
          git(dev.root, ['merge', '--ff-only', '--no-edit', manifest.sourceCommit]);
          validateDestination(manifest, { merged: true });
        }
        manifest.mergedHead = manifest.sourceCommit;
        manifest.phase = 'merged'; save(manifest);
      }
      if (['merged', 'archive-planned'].includes(manifest.phase)) {
        validateDestination(manifest, { merged: true });
        if (!manifest.archivePath) {
          manifest.archivePath = join(manifest.archiveRoot, `${basename(manifest.taskRoot)}-completed-${digest(id).slice(0, 12)}-${Date.now()}`);
          if (existsSync(manifest.archivePath)) fail('The unique archive name already exists.');
          manifest.phase = 'archive-planned'; save(manifest);
        }
        checkArchiveScope(manifest);
        if (existsSync(manifest.taskRoot)) {
          if (existsSync(manifest.archivePath)) fail('Both source and archive paths exist; manual review is required.');
          verifySource(manifest, manifest.sourceCommit);
          if (changes(manifest.taskRoot).some(path => !manifest.excluded.includes(path))) fail('New source changes appeared before archive.');
          ensureNoProcesses(manifest.taskRoot);
          // A root rename preserves files and junctions without traversing SDK links.
          if (inside(manifest.taskRoot, process.cwd())) process.chdir(manifest.archiveRoot);
          renameSync(manifest.taskRoot, manifest.archivePath);
        }
        if (existsSync(manifest.taskRoot) || !existsSync(manifest.archivePath)) fail('The source was not safely preserved in its archive.');
        const archive = plainDirectory(manifest.archivePath);
        const marker = readFileSync(join(archive, '.git'), 'utf8').trim();
        if (!marker.startsWith('gitdir: ') || !same(marker.slice(8), manifest.gitDir)) fail('The archive does not identify the registered task repository.');
        manifest.phase = 'archived'; save(manifest);
      }
      if (manifest.phase === 'archived') {
        if (existsSync(manifest.taskRoot)) fail('The old task path exists; refusing to remove any live directory.');
        const completed = join(metadata(manifest.common), 'completed-registrations');
        if (!existsSync(completed)) mkdirSync(completed);
        plainDirectory(completed);
        if (!manifest.registrationArchive) {
          manifest.registrationArchive = join(completed, `${basename(manifest.gitDir)}-${digest(id).slice(0, 12)}-${Date.now()}`);
          if (existsSync(manifest.registrationArchive)) fail('The unique registration archive already exists.');
          save(manifest);
        }
        if (!same(dirname(manifest.registrationArchive), completed)) fail('The registration archive is outside its approved scope.');
        if (existsSync(manifest.gitDir)) {
          if (existsSync(manifest.registrationArchive)) fail('Both original and archived admin directories exist.');
          const registered = registrations(manifest.devRoot).filter(record => same(record.path, manifest.taskRoot));
          if (registered.length !== 1 || registered[0].branch !== `refs/heads/${manifest.branch}` || registered[0].locked) fail('The missing task registration changed.');
          validateRegistration(manifest);
          // Never pass the old filesystem path to git worktree remove: even if
          // another process recreates it now, this rename touches only metadata.
          renameSync(manifest.gitDir, manifest.registrationArchive);
        }
        validateRegistration(manifest, manifest.registrationArchive);
        if (registrations(manifest.devRoot).some(record => same(record.path, manifest.taskRoot))) fail('The task registration remains; the archive is preserved for retry.');
        manifest.phase = 'complete'; manifest.finishedAt = new Date().toISOString();
        return writeReceipt(manifest, 'PASS', 'Reviewed code committed, dev fast-forwarded, task directory preserved, and only its exact admin registration archived.');
      }
      fail(`Unknown completion phase: ${manifest.phase}`);
    } catch (error) {
      writeReceipt(manifest, 'BLOCKED', error.message);
      throw error;
    }
  });
}
export function handleStop(input, { repo, cwd } = {}) {
  if (!input || input.hook_event_name !== 'Stop' || input.stop_hook_active === true) return { status: 'NOOP', reason: 'This is not an eligible Stop event.' };
  const id = input.session_id ?? input.thread_id;
  if (typeof id !== 'string' || !id.trim()) return { status: 'NOOP', reason: 'Stop has no matching session ID.' };
  const taskCwd = input.cwd ?? cwd ?? process.cwd();
  let located;
  try { located = repository(repo ?? taskCwd); }
  catch { return { status: 'NOOP', reason: 'Stop is outside an accessible registered repository.' }; }
  const manifest = load(located.common, id);
  if (!manifest || !manifest.complete) return { status: 'NOOP', reason: 'No completed task marker; status replies and pauses do nothing.' };
  // Codex may keep its chat cwd in the primary checkout while implementation is
  // isolated in a linked tree. Session + ready marker selects the exact task.
  if (existsSync(taskCwd)) {
    let caller;
    try { caller = repository(taskCwd); } catch { return { status: 'NOOP', reason: 'Stop cwd is not an accessible repository.' }; }
    if (!same(caller.common, located.common) || !registrations(located.root).some(record => same(record.path, caller.root))) {
      return { status: 'NOOP', reason: 'Stop cwd belongs to another repository or an unregistered worktree.' };
    }
  } else if (manifest.phase !== 'complete' || !same(resolve(taskCwd), manifest.taskRoot)) {
    return { status: 'NOOP', reason: 'Stop cwd is not a live registered worktree.' };
  }
  return finishTask({ cwd: manifest.taskRoot, sessionId: id, repo: located.root });
}
// Stop-driven auto-commit of code and configuration only. Reviewed design,
// instruction, image, and Harness files are never committed by this path.
const autoCommitPrefix = 'chore(autocommit): ';
function deferOnLocked(common, action) {
  try { return locked(common, action); }
  catch (error) {
    if (error instanceof CompletionBlocked && /owns this repository lock/.test(error.message)) {
      return { status: 'NOOP', reason: 'Another ArkDock operation owns the repository lock; the auto-commit resumes on the next stop.' };
    }
    throw error;
  }
}
function changedCode(root) {
  const all = changes(root);
  return { all, selected: all.filter(isCodePath) };
}
export function autoCommitTask({ cwd = process.cwd(), repo: repoPath, stamp } = {}) {
  const repo = repository(repoPath ?? cwd);
  // The primary checkout, dev, and main are never auto-committed; only a linked task branch moves.
  if (same(repo.gitDir, repo.common)) return { status: 'NOOP', reason: 'The primary checkout is never auto-committed.' };
  if (['dev', 'main'].includes(repo.branch)) return { status: 'NOOP', reason: `Branch ${repo.branch} is never auto-committed.` };
  const registered = registrations(repo.root).filter(record => same(record.path, repo.root) && record.branch === `refs/heads/${repo.branch}`);
  if (registered.length !== 1 || registered[0].locked) return { status: 'NOOP', reason: 'The linked worktree registration is missing, ambiguous, or locked.' };
  try { inProgress(repo); }
  catch { return { status: 'NOOP', reason: 'A merge, rebase, cherry-pick, or revert is in progress; no auto-commit was attempted.' }; }
  const { all, selected } = changedCode(repo.root);
  if (!selected.length) {
    return { status: 'NOOP', reason: all.length ? `Only ${all.length} non-code path(s) changed; nothing is auto-committed.` : 'No changes to commit.', excluded: all };
  }
  return deferOnLocked(repo.common, () => {
    const current = changedCode(repo.root);
    if (!sameRecords(current.selected, selected)) fail('The changed code set moved while the auto-commit waited for the repository lock.');
    // Reuse the reviewed-source gate: regular file, under 8 MiB, no binary or secret content.
    for (const path of selected) fileRecord(repo.root, path);
    git(repo.root, ['add', '-A', '--', ...selected]);
    const prepared = git(repo.root, ['diff', '--name-only', '-z', 'HEAD', '--', ...selected]).split('\0').filter(Boolean).sort();
    if (!sameRecords(prepared, selected)) fail('The staged code set does not match the changed code set.');
    const before = git(repo.root, ['rev-parse', 'HEAD']).trim();
    const at = stamp ?? new Date().toISOString();
    const message = `${autoCommitPrefix}${repo.branch} ${at}\n\n${prepared.length} file(s)\n${prepared.map(path => `- ${path}`).join('\n')}`;
    // A pathspec commit ignores unrelated staged content, so a staged design file is never swept in.
    git(repo.root, ['-c', 'commit.gpgSign=false', 'commit', '-m', message, '--', ...prepared]);
    const commit = git(repo.root, ['rev-parse', 'HEAD']).trim();
    const ancestry = git(repo.root, ['rev-list', '--parents', '-n', '1', 'HEAD']).trim().split(/\s+/);
    if (ancestry[1] !== before) fail('The auto-commit did not advance HEAD by exactly one commit.');
    const committed = [...new Set(git(repo.root, ['diff-tree', '--no-commit-id', '--no-renames', '--name-only', '-r', '-z', 'HEAD']).split('\0').filter(Boolean))].sort();
    if (!sameRecords(committed, prepared) || committed.some(path => !isCodePath(path))) {
      fail(`The auto-commit recorded paths outside the changed code set: ${committed.join(', ')}`);
    }
    return { status: 'PASS', branch: repo.branch, commit, files: prepared, excluded: current.all.filter(path => !selected.includes(path)) };
  });
}
export function handleStopCommit(input, { cwd } = {}) {
  if (!input || input.hook_event_name !== 'Stop' || input.stop_hook_active === true) return { status: 'NOOP', reason: 'This is not an eligible Stop event.' };
  const taskCwd = input.cwd ?? cwd ?? process.cwd();
  let located;
  try { located = repository(taskCwd); }
  catch { return { status: 'NOOP', reason: 'Stop cwd is outside an accessible repository.' }; }
  const id = input.session_id ?? input.thread_id;
  if (typeof id === 'string' && id.trim()) {
    let manifest = null;
    try { manifest = load(located.common, id); } catch { manifest = null; }
    // Never disturb an in-flight reviewed completion transaction.
    if (manifest && manifest.phase !== 'begun') {
      return { status: 'NOOP', reason: 'An ArkDock completion transaction is in flight; the auto-commit stays out of its way.' };
    }
  }
  return autoCommitTask({ cwd: located.root });
}
function argumentsFor(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i++) {
    const option = argv[i];
    if (option === '--complete') options.complete = true;
    else {
      const mapping = { '--session': 'sessionId', '--cwd': 'cwd', '--repo': 'repo', '--archive-root': 'archiveRoot', '--dev': 'devBranch', '--message': 'message' };
      if (option === '--path') { options.paths ??= []; options.paths.push(argv[++i]); }
      else if (mapping[option] && argv[i + 1]) options[mapping[option]] = argv[++i];
      else fail(`Unknown or incomplete option: ${option}`);
    }
  }
  return options;
}
export function main(argv = process.argv.slice(2)) {
  const [command, ...args] = argv;
  try {
    const options = argumentsFor(args);
    let result;
    if (command === 'begin') result = beginTask(options);
    else if (command === 'ready') result = readyTask(options);
    else if (command === 'finish') result = finishTask(options);
    else if (command === 'recover-lock') result = recoverLock(options);
    else if (command === 'autocommit') {
      let input;
      try { input = JSON.parse(readFileSync(0, 'utf8')); } catch { input = null; }
      result = handleStopCommit(input, options);
    } else if (command === 'stop') {
      let input;
      try { input = JSON.parse(readFileSync(0, 'utf8')); } catch { input = null; }
      result = handleStop(input, options);
    } else fail('Use begin, ready --complete, finish, recover-lock, autocommit, or stop (stdin JSON).');
    process.stdout.write(`${JSON.stringify(result)}\n`);
    return 0;
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    return 2;
  }
}
if (process.argv[1] && same(fileURLToPath(import.meta.url), process.argv[1])) process.exitCode = main();
