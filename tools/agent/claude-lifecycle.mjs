import { createHash } from 'node:crypto';
import { openSync, closeSync, readSync, fstatSync, realpathSync, lstatSync, writeFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { BridgeClient, readPairingFile, requireValue } from './bridge-client.mjs';

const ID = /^[A-Za-z0-9_-]{1,128}$/;
const samePath = (a, b) => process.platform === 'win32' ? a.toLowerCase() === b.toLowerCase() : a === b;
const digest = (pair) => createHash('sha256').update(pair.token).digest('hex');
const paths = (file) => ({ active: `${file}.claude-session.json`, revoked: `${file}.claude-revoked` });

function readOwnJson(file) {
  requireValue(!lstatSync(file).isSymbolicLink() && samePath(realpathSync(file), resolve(file)), 'CHANNEL_GUARD_PATH_INVALID');
  const handle = openSync(file, 'r');
  try {
    const stat = fstatSync(handle);
    requireValue(stat.isFile() && stat.size > 0 && stat.size <= 4096, 'CHANNEL_GUARD_INVALID');
    const bytes = Buffer.alloc(4097);
    const count = readSync(handle, bytes, 0, bytes.length, 0);
    requireValue(count <= 4096, 'CHANNEL_GUARD_INVALID');
    return JSON.parse(bytes.subarray(0, count).toString('utf8'));
  } finally { closeSync(handle); }
}

export function createLifecycleGuard(pairingFile, pair, sessionId, { cwd = process.cwd(), now = Date.now } = {}) {
  requireValue(typeof sessionId === 'string' && ID.test(sessionId), 'CLAUDE_SESSION_ID_REQUIRED');
  const own = paths(pairingFile);
  const root = realpathSync(pair.workspaceRoot);
  requireValue(samePath(root, realpathSync(cwd)), 'CHANNEL_WORKSPACE_MISMATCH');
  return () => {
    requireValue(!existsSync(own.revoked), 'CHANNEL_SESSION_REVOKED');
    requireValue(existsSync(own.active), 'CHANNEL_LIFECYCLE_REQUIRED');
    const active = readOwnJson(own.active);
    requireValue(active.version === 1 && active.sessionId === sessionId && active.pairingDigest === digest(pair)
      && samePath(active.workspaceRoot, root) && Number.isSafeInteger(active.createdAt)
      && active.createdAt <= now() && active.createdAt < pair.expiresAt, 'CHANNEL_SESSION_MISMATCH');
  };
}

export async function runLifecycleHook(pairingFile, input, { now = Date.now, bridgeFactory = (pair) => new BridgeClient(pair) } = {}) {
  // An expired pairing still permits revocation; the hook never reads transcript_path.
  const pair = await readPairingFile(pairingFile, () => 0);
  requireValue(input && typeof input === 'object' && ID.test(input.session_id ?? '')
    && ['SessionStart', 'SessionEnd', 'CwdChanged'].includes(input.hook_event_name), 'CHANNEL_HOOK_INVALID');
  const own = paths(pairingFile);
  const revoke = async () => {
    // Synchronous irreversible tombstone precedes any network await. A new session
    // must obtain a new pairing; an old token can never resurrect this guard.
    try { writeFileSync(own.revoked, 'revoked\n', { flag: 'wx', mode: 0o600 }); }
    catch (error) { if (error.code !== 'EEXIST') throw error; }
    await bridgeFactory(pair).revoke({ sessionId: input.session_id });
  };
  if (input.hook_event_name !== 'SessionStart') {
    if (input.hook_event_name === 'CwdChanged') {
      try {
        if (typeof input.new_cwd === 'string'
          && samePath(realpathSync(input.new_cwd), realpathSync(pair.workspaceRoot))) return;
      } catch { /* An unresolvable changed directory also revokes the old scope. */ }
    }
    return revoke();
  }
  if (input.source !== 'compact') requireValue(pair.expiresAt > now(), 'PAIRING_EXPIRED');
  requireValue(!existsSync(own.revoked), 'CHANNEL_SESSION_REVOKED');
  if (['clear', 'fork'].includes(input.source)) return revoke();
  const root = realpathSync(pair.workspaceRoot);
  requireValue(['startup', 'resume', 'compact'].includes(input.source), 'CHANNEL_HOOK_INVALID');
  requireValue(typeof input.cwd === 'string' && samePath(realpathSync(input.cwd), root), 'CHANNEL_WORKSPACE_MISMATCH');
  if (existsSync(own.active)) {
    const current = readOwnJson(own.active);
    if (current.sessionId !== input.session_id || current.pairingDigest !== digest(pair)) return revoke();
    return;
  }
  requireValue(input.source !== 'compact', 'CHANNEL_LIFECYCLE_REQUIRED');
  writeFileSync(own.active, `${JSON.stringify({ version: 1, sessionId: input.session_id,
    workspaceRoot: root, pairingDigest: digest(pair), createdAt: now() })}\n`, { flag: 'wx', mode: 0o600 });
}

export async function writeClaudeConfiguration(pairingFile) {
  await readPairingFile(pairingFile);
  const script = fileURLToPath(new URL('./claude-channel.mjs', import.meta.url));
  const hook = { type: 'command', command: process.execPath,
    args: [script, '--hook', '--pairing-file', pairingFile], timeout: 5 };
  const files = {
    mcp: `${pairingFile}.claude-mcp.json`, settings: `${pairingFile}.claude-settings.json`,
  };
  const mcp = { mcpServers: { arkdock: { command: process.execPath,
    args: [script, '--pairing-file', pairingFile] } } };
  const settings = { hooks: { SessionStart: [{ hooks: [hook] }], SessionEnd: [{ hooks: [hook] }],
    CwdChanged: [{ hooks: [hook] }] } };
  // Exclusive writes preserve any files the user has already reviewed or changed.
  writeFileSync(files.mcp, `${JSON.stringify(mcp, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  writeFileSync(files.settings, `${JSON.stringify(settings, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  return files;
}
