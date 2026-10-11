import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, unlink, rmdir } from 'node:fs/promises';
import { existsSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { createHash } from 'node:crypto';
import { PassThrough, Readable, Writable } from 'node:stream';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { ClaudeChannel, runChannelStdio } from '../../tools/agent/claude-channel.mjs';
import { createLifecycleGuard, runLifecycleHook, writeClaudeConfiguration } from '../../tools/agent/claude-lifecycle.mjs';

const CLIENT = { name: 'claude-code', version: '2.1.284-fixture' };
const SESSION = 'd97cbe80-eeb8-4fa9-9e20-14ec09c1d15a';
const rpc = (id, method, params) => `${JSON.stringify({ jsonrpc: '2.0', ...(id ? { id } : {}), method, params })}\n`;
const waitFor = async (condition) => {
  const until = Date.now() + 5000;
  while (!condition()) { assert.ok(Date.now() < until, 'fixture response timed out'); await new Promise((done) => setTimeout(done, 10)); }
};

async function fixture(t) {
  const directory = await mkdtemp(join(tmpdir(), 'arkdock-claude-channel-'));
  const files = [], directories = [directory];
  const write = async (file, value) => { files.push(file); await writeFile(file, typeof value === 'string' || Buffer.isBuffer(value) ? value : JSON.stringify(value)); };
  t.after(async () => {
    // Only exact test-owned entries; no recursive removal or external junction targets.
    for (const file of [...new Set(files)].reverse()) await unlink(file).catch((error) => { if (error.code !== 'ENOENT') throw error; });
    for (const folder of directories.reverse()) await rmdir(folder);
  });
  const pairingFile = join(directory, 'pairing.json');
  const pairing = { endpoint: 'http://127.0.0.1:19876', token: 'fixture-secret-'.padEnd(48, 'x'),
    workspaceId: 'workspace-test', workspaceRoot: directory, expiresAt: Date.now() + 60000 };
  await write(pairingFile, pairing);
  files.push(`${pairingFile}.claude-session.json`, `${pairingFile}.claude-revoked`,
    `${pairingFile}.claude-mcp.json`, `${pairingFile}.claude-settings.json`);
  const calls = { register: [], poll: [], receipt: [], revoke: [] }; let pending = [];
  const bridge = {
    async register(body) { calls.register.push(body); return { registrationId: 'registration-fixture', expiresAt: Date.now() + 10000 }; },
    async poll(body) { calls.poll.push(body); const deliveries = pending; pending = []; return { deliveries }; },
    async receipt(body) { calls.receipt.push(body); return { accepted: true }; },
    async revoke(body) { calls.revoke.push(body); return { accepted: true }; },
  };
  const guard = createLifecycleGuard(pairingFile, pairing, SESSION, { cwd: directory });
  const hook = (input) => runLifecycleHook(pairingFile, { session_id: SESSION, cwd: directory, ...input }, { bridgeFactory: () => bridge });
  const start = () => hook({ hook_event_name: 'SessionStart', source: 'startup' });
  const channel = new ClaudeChannel({ pairing, sessionId: SESSION, guard, bridge });
  const delivery = { deliveryId: 'delivery-fixture', selectionId: 'sel-fixture', prompt: 'Change the selected ArkUI component',
    workspaceId: pairing.workspaceId, expiresAt: Date.now() + 30000 };
  return { directory, files, directories, write, pairing, pairingFile, channel, bridge, calls, guard, hook, start, delivery,
    queue: (...deliveries) => { pending = deliveries; } };
}

test('Claude lifecycle requires matching explicit hook, workspace, and session identity', async (t) => {
  const f = await fixture(t);
  assert.throws(f.guard, /CHANNEL_LIFECYCLE_REQUIRED/);
  await f.start(); f.guard();
  assert.throws(createLifecycleGuard(f.pairingFile, f.pairing, 'different', { cwd: f.directory }), /CHANNEL_SESSION_MISMATCH/);
  assert.throws(() => createLifecycleGuard(f.pairingFile, f.pairing, SESSION), /CHANNEL_WORKSPACE_MISMATCH/);
  await f.channel.register(CLIENT);
  assert.equal(f.calls.register[0].sessionId, SESSION);
  assert.equal(f.calls.register[0].capabilities.busyState, false);
  await f.channel.close(); assert.equal(f.calls.revoke.length, 1);
});

test('SessionEnd revokes locally before network failure and a new session cannot revive pairing', async (t) => {
  const f = await fixture(t); await f.start();
  f.bridge.revoke = async () => { assert.throws(f.guard, /CHANNEL_SESSION_REVOKED/); throw new Error('BRIDGE_UNREACHABLE'); };
  await assert.rejects(f.hook({ hook_event_name: 'SessionEnd', reason: 'clear' }), /BRIDGE_UNREACHABLE/);
  await assert.rejects(f.hook({ hook_event_name: 'SessionStart', source: 'startup', session_id: 'next-session' }), /CHANNEL_SESSION_REVOKED/);
});

test('workspace and session changes invalidate the one consumed pairing', async (t) => {
  const f = await fixture(t); await f.start();
  await f.hook({ hook_event_name: 'SessionStart', source: 'resume', session_id: 'other-session' });
  assert.throws(f.guard, /CHANNEL_SESSION_REVOKED/);
  const g = await fixture(t); await g.start();
  await g.hook({ hook_event_name: 'CwdChanged', cwd: f.directory, new_cwd: f.directory });
  assert.throws(g.guard, /CHANNEL_SESSION_REVOKED/);
});

test('channel writes real notification format, deduplicates deliveries, and waits for explicit acknowledgement', async (t) => {
  const f = await fixture(t); await f.start(); await f.channel.register(CLIENT);
  const events = []; f.queue(f.delivery, f.delivery); await f.channel.poll((event) => { events.push(event); });
  assert.equal(events.length, 1);
  assert.equal(events[0].method, 'notifications/claude/channel');
  assert.equal(events[0].params.meta.delivery_id, f.delivery.deliveryId);
  assert.ok(Object.values(events[0].params.meta).every((value) => typeof value === 'string'));
  assert.deepEqual(f.calls.receipt.map((r) => r.state), ['TRANSPORT_ACCEPTED']);
  await assert.rejects(f.channel.call('arkdock.complete_delivery', { deliveryId: f.delivery.deliveryId, success: true, summary: 'Done' }), /DELIVERY_ACK_REQUIRED/);
  await f.channel.call('arkdock.acknowledge_delivery', { deliveryId: f.delivery.deliveryId });
  await f.channel.call('arkdock.acknowledge_delivery', { deliveryId: f.delivery.deliveryId });
  await f.channel.call('arkdock.complete_delivery', { deliveryId: f.delivery.deliveryId, success: true, summary: 'Verified requested change.' });
  assert.deepEqual(f.calls.receipt.map((r) => r.state), ['TRANSPORT_ACCEPTED', 'AGENT_ACKNOWLEDGED', 'COMPLETED']);
  assert.equal(new Set(f.calls.receipt.map((r) => r.receiptId)).size, 3);
  assert.ok(f.calls.receipt.every((receipt) => !Object.hasOwn(receipt, 'workspaceRoot')),
    'Rust Bridge receipt schema rejects poll-only fields');
  await assert.rejects(f.channel.call('arkdock.acknowledge_delivery', { deliveryId: 'delivery-foreign' }), /DELIVERY_NOT_FOUND/);
  await assert.rejects(f.channel.call('arkdock.complete_delivery', { deliveryId: f.delivery.deliveryId, success: false, summary: 'Contradictory result' }), /DELIVERY_ACK_REQUIRED/);
});

test('selection tools use the granted provider and reject foreign selections, stale runtime and image tampering', async (t) => {
  const f = await fixture(t); await f.start(); await f.channel.register(CLIENT);
  const store = join(f.directory, 'selections'), selected = join(store, f.delivery.selectionId);
  await mkdir(store); await mkdir(selected); f.directories.push(store, selected);
  const image = Buffer.from([0xff, 0xd8, 0xff, 0xd9]); // Protocol/integrity fixture, never ArkUI evidence.
  const metadata = { artifactId: 'screenshot.jpg', format: 'jpeg', byteLength: image.length,
    sha256: createHash('sha256').update(image).digest('hex') };
  const grant = { directory: store, capabilityFile: join(f.directory, 'grant.json'), workspaceId: f.pairing.workspaceId,
    sessionId: 'preview-session', grantId: `${f.delivery.deliveryId}-fixture` };
  const runtime = { workspaceId: grant.workspaceId, sessionId: grant.sessionId, buildId: 'build', generation: 1,
    available: true, observedAt: Date.now() - 1, expiresAt: Date.now() + 10000 };
  const context = JSON.stringify({ ...runtime, selectionId: f.delivery.selectionId,
    createdAt: Date.now() - 1, screenshot: metadata });
  await f.write(grant.capabilityFile, { version: 1, ...grant, expiresAt: f.pairing.expiresAt,
    contextSha256: createHash('sha256').update(context).digest('hex'),
    allowedTools: ['arkdock.get_selection', 'arkdock.get_selection_screenshot'], selectionIds: [f.delivery.selectionId] });
  await f.write(join(selected, 'context.json'), context);
  await f.write(join(store, 'runtime.json'), runtime);
  await f.write(join(selected, 'screenshot.jpg'), image);
  f.queue({ ...f.delivery, grant }); await f.channel.poll(() => {});
  const args = { deliveryId: f.delivery.deliveryId, selectionId: f.delivery.selectionId };
  const result = await f.channel.call('arkdock.get_selection_screenshot', args);
  assert.deepEqual(Buffer.from(result.content[0].data, 'base64'), image);
  assert.equal(f.calls.receipt.length, 1, 'reading the image is not an Agent acknowledgement');
  await assert.rejects(f.channel.call('arkdock.get_selection', { ...args, selectionId: 'sel-other' }), /DELIVERY_SCOPE_MISMATCH/);
  await writeFile(join(selected, 'screenshot.jpg'), 'bad');
  await assert.rejects(f.channel.call('arkdock.get_selection_screenshot', args), /IMAGE_INTEGRITY_MISMATCH/);
  await writeFile(join(store, 'runtime.json'), JSON.stringify({ ...runtime, available: false }));
  await assert.rejects(f.channel.call('arkdock.get_selection', args), /SELECTION_RUNTIME_STALE/);
});

test('revocation during poll prevents a notification and output uncertainty is never retried', async (t) => {
  const f = await fixture(t); await f.start(); await f.channel.register(CLIENT);
  f.bridge.poll = async () => { writeFileSync(`${f.pairingFile}.claude-revoked`, 'revoked'); return { deliveries: [f.delivery] }; };
  await assert.rejects(f.channel.poll(() => assert.fail('must not send')), /CHANNEL_SESSION_REVOKED/);
  const g = await fixture(t); await g.start(); await g.channel.register(CLIENT); g.queue(g.delivery);
  await assert.rejects(g.channel.poll(() => { throw new Error('PIPE_FAILED'); }), /PIPE_FAILED/);
  g.queue(g.delivery); await g.channel.poll(() => assert.fail('uncertain delivery must not retry'));
  assert.equal(g.calls.receipt.length, 0);
});

test('delivery from another host session is rejected before notification', async (t) => {
  const f = await fixture(t); await f.start(); await f.channel.register(CLIENT);
  f.queue({ ...f.delivery, sessionId: 'different-session' });
  await assert.rejects(f.channel.poll(() => assert.fail('foreign session must not receive events')), /DELIVERY_SCOPE_MISMATCH/);
  assert.equal(f.calls.receipt.length, 0);
});

test('acknowledgement and completion survive selection expiry but data access does not', async (t) => {
  const f = await fixture(t); await f.start(); await f.channel.register(CLIENT);
  f.queue(f.delivery); await f.channel.poll(() => {});
  f.channel.deliveries.get(f.delivery.deliveryId).delivery.expiresAt = 1;
  await assert.rejects(f.channel.call('arkdock.get_selection', { deliveryId: f.delivery.deliveryId,
    selectionId: f.delivery.selectionId }), /DELIVERY_EXPIRED/);
  await f.channel.call('arkdock.acknowledge_delivery', { deliveryId: f.delivery.deliveryId });
  await f.channel.call('arkdock.complete_delivery', { deliveryId: f.delivery.deliveryId, success: true,
    summary: 'Edit and rebuild finished after the original selection expired.' });
  assert.equal(f.calls.receipt.at(-1).state, 'COMPLETED');
});

test('early Agent tools await the in-flight transport receipt instead of reporting a false rejection', async (t) => {
  const f = await fixture(t); await f.start(); await f.channel.register(CLIENT);
  const release = Promise.withResolvers(); let entered = false;
  const receipt = f.bridge.receipt;
  f.bridge.receipt = async (body) => {
    if (body.state === 'TRANSPORT_ACCEPTED') { entered = true; await release.promise; }
    return receipt(body);
  };
  f.queue(f.delivery);
  const sending = f.channel.poll(() => {});
  await waitFor(() => entered);
  const acknowledging = f.channel.call('arkdock.acknowledge_delivery', { deliveryId: f.delivery.deliveryId });
  release.resolve(); await sending; await acknowledging;
  assert.deepEqual(f.calls.receipt.map((r) => r.state), ['TRANSPORT_ACCEPTED', 'AGENT_ACKNOWLEDGED']);
});

test('pairing expiry limits new registration while an established session can finish and compact', async (t) => {
  const f = await fixture(t); await f.start(); await f.channel.register(CLIENT);
  f.queue(f.delivery); await f.channel.poll(() => {});
  const now = () => f.pairing.expiresAt + 1;
  f.channel.guard = createLifecycleGuard(f.pairingFile, f.pairing, SESSION, { cwd: f.directory, now });
  f.channel.now = now;
  await runLifecycleHook(f.pairingFile, { hook_event_name: 'SessionStart', source: 'compact',
    session_id: SESSION, cwd: f.directory }, { now, bridgeFactory: () => f.bridge });
  await f.channel.call('arkdock.acknowledge_delivery', { deliveryId: f.delivery.deliveryId });
  await f.channel.call('arkdock.complete_delivery', { deliveryId: f.delivery.deliveryId, success: true, summary: 'Long task finished' });
  await assert.rejects(new ClaudeChannel({ pairing: f.pairing, sessionId: SESSION, guard: f.channel.guard,
    bridge: f.bridge, now }).register(CLIENT), /PAIRING_EXPIRED/);
});

test('grant preflight failures reject early tool waiters and never adopt another delivery grant', async (t) => {
  const f = await fixture(t); await f.start(); await f.channel.register(CLIENT);
  f.queue({ ...f.delivery, grant: { workspaceId: f.pairing.workspaceId, grantId: 'delivery-other-101' } });
  await assert.rejects(f.channel.poll(() => assert.fail('foreign grant must not send')), /DELIVERY_SCOPE_MISMATCH/);
  await assert.rejects(f.channel.call('arkdock.acknowledge_delivery', { deliveryId: f.delivery.deliveryId }), /DELIVERY_SCOPE_MISMATCH/);
});

test('stdio negotiates channel capability and initialization, then revokes on EOF', async (t) => {
  const f = await fixture(t); await f.start(); const input = new PassThrough(); const messages = [];
  const output = new Writable({ write(chunk, _encoding, done) { messages.push(JSON.parse(chunk.toString())); done(); } });
  const running = runChannelStdio(f.channel, input, output, { pollMs: 10 });
  input.write(rpc(1, 'tools/list'));
  input.write(rpc(2, 'initialize', { protocolVersion: '2026-07-28', clientInfo: CLIENT }));
  input.write(rpc(null, 'notifications/initialized')); input.write(rpc(3, 'tools/list'));
  await waitFor(() => f.calls.poll.length > 0 && messages.length >= 3);
  input.end(); await running;
  assert.equal(messages[0].error.message, 'NOT_INITIALIZED');
  assert.equal(messages[1].result.protocolVersion, '2025-06-18');
  assert.deepEqual(messages[1].result.capabilities.experimental, { 'claude/channel': {} });
  assert.equal(messages[2].result.tools.length, 5); assert.equal(f.calls.revoke.length, 1);
});

test('stdio refuses missing lifecycle and oversized requests without exposing paths or credentials', async (t) => {
  const f = await fixture(t); const input = new PassThrough(); const lines = [];
  const output = new Writable({ write(chunk, _encoding, done) { lines.push(chunk.toString()); done(); } });
  const running = runChannelStdio(f.channel, input, output, { pollMs: 5, lifecycleWaitMs: 10 });
  input.write(rpc(1, 'initialize', { protocolVersion: '2025-06-18', clientInfo: CLIENT }));
  input.write(rpc(null, 'notifications/initialized'));
  await assert.rejects(running, /CHANNEL_LIFECYCLE_REQUIRED/);
  assert.equal(f.calls.register.length, 0); assert.ok(!lines.join('').includes(f.pairing.token));
  const g = await fixture(t);
  await assert.rejects(runChannelStdio(g.channel, Readable.from(['x'.repeat(128 * 1024 + 1)]), output), /REQUEST_LIMIT/);
});

test('configuration generation writes reviewable session files with structured hook arguments only', async (t) => {
  const f = await fixture(t); const generated = await writeClaudeConfiguration(f.pairingFile);
  const settings = JSON.parse(await readFile(generated.settings, 'utf8'));
  const mcp = JSON.parse(await readFile(generated.mcp, 'utf8'));
  assert.deepEqual(Object.keys(settings.hooks), ['SessionStart', 'SessionEnd', 'CwdChanged']);
  assert.ok(settings.hooks.SessionEnd[0].hooks[0].args.includes(f.pairingFile));
  assert.deepEqual(mcp.mcpServers.arkdock.args.slice(-2), ['--pairing-file', f.pairingFile]);
  assert.ok(!JSON.stringify({ settings, mcp }).includes(f.pairing.token));
  await assert.rejects(writeClaudeConfiguration(f.pairingFile), /EEXIST/);
});

test('real child stdio and loopback HTTP fixture carry same-session delivery and Agent tool receipt', async (t) => {
  const f = await fixture(t); const httpCalls = []; let sent = false;
  const server = createServer(async (request, response) => {
    let body = ''; for await (const chunk of request) body += chunk;
    assert.equal(request.headers.authorization, `Bearer ${f.pairing.token}`);
    const value = JSON.parse(body); httpCalls.push({ route: request.url, value });
    response.setHeader('content-type', 'application/json');
    let result = { accepted: true };
    if (request.url === '/v1/register') result = { registrationId: 'registration-child', expiresAt: Date.now() + 10000 };
    if (request.url === '/v1/poll') { result = { deliveries: sent ? [] : [f.delivery] }; sent = true; }
    response.end(JSON.stringify(result));
  });
  await new Promise((done) => server.listen(0, '127.0.0.1', done));
  t.after(() => new Promise((done) => server.close(done)));
  f.pairing.endpoint = `http://127.0.0.1:${server.address().port}`;
  await writeFile(f.pairingFile, JSON.stringify(f.pairing)); await f.start();
  const child = spawn(process.execPath, [resolve('tools/agent/claude-channel.mjs'), '--pairing-file', f.pairingFile],
    { cwd: f.directory, env: { ...process.env, CLAUDE_CODE_SESSION_ID: SESSION }, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  t.after(() => { if (child.exitCode === null) child.kill(); });
  const messages = []; let buffer = '', stderr = '';
  child.stdout.on('data', (chunk) => { buffer += chunk; let at; while ((at = buffer.indexOf('\n')) >= 0) {
    messages.push(JSON.parse(buffer.slice(0, at))); buffer = buffer.slice(at + 1);
  } });
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  const exited = new Promise((done) => child.once('exit', (code) => done(code)));
  child.stdin.write(rpc(1, 'initialize', { protocolVersion: '2025-06-18', clientInfo: CLIENT }));
  child.stdin.write(rpc(null, 'notifications/initialized'));
  await waitFor(() => httpCalls.some((item) => item.value.state === 'TRANSPORT_ACCEPTED'));
  const notification = messages.find((item) => item.method === 'notifications/claude/channel');
  assert.equal(notification.params.meta.session_id, SESSION);
  child.stdin.write(rpc(2, 'tools/call', { name: 'arkdock.acknowledge_delivery', arguments: { deliveryId: f.delivery.deliveryId } }));
  await waitFor(() => messages.some((item) => item.id === 2));
  assert.ok(httpCalls.some((item) => item.value.state === 'AGENT_ACKNOWLEDGED'));
  child.stdin.end(); assert.equal(await exited, 0); assert.equal(stderr, '');
  assert.ok(httpCalls.some((item) => item.route === '/v1/revoke'));
  assert.ok(!existsSync(`${f.pairingFile}.claude-revoked`), 'EOF revokes the Bridge without altering unrelated files');
});
