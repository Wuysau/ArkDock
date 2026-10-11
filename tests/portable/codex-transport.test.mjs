import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, rm, realpath, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, toNamespacedPath } from 'node:path';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { PassThrough, Writable } from 'node:stream';
import { runInNewContext } from 'node:vm';
import { AppServerRpc, ManagedCodexTransport } from '../../tools/agent/codex-app-server.mjs';
import { BridgeClient, readPairingFile, validateEndpoint, normalizeLocalWorkspaceRoot } from '../../tools/agent/bridge-client.mjs';
import { DesktopSessionBridge, runDesktopStdio } from '../../tools/agent/codex-desktop-plugin.mjs';

async function selectionFixture(t) {
  const temporary = await realpath(await mkdtemp(join(tmpdir(), 'arkdock-codex-test-')));
  // Only these test-owned files are created; this fixture creates no links/junctions.
  t.after(() => rm(temporary, { recursive: true, force: true }));
  const directory = join(temporary, 'selections');
  const selectionId = 'sel-codex-fixture';
  await mkdir(join(directory, selectionId), { recursive: true });
  const bytes = Buffer.from([0xff, 0xd8, 0xff, 0xd9]);
  const screenshot = { artifactId: 'screenshot.jpg', format: 'jpeg', byteLength: bytes.length,
    sha256: createHash('sha256').update(bytes).digest('hex') };
  const imagePath = join(directory, selectionId, 'screenshot.jpg');
  const context = { selectionId, workspaceId: 'workspace', sessionId: 'preview-session',
    buildId: 'build', generation: 1, createdAt: Date.now() - 100, expiresAt: Date.now() + 600000, screenshot };
  await writeFile(join(directory, selectionId, 'context.json'), JSON.stringify(context));
  await writeFile(imagePath, bytes);
  await writeFile(join(directory, 'runtime.json'), JSON.stringify({ ...context, available: true,
    observedAt: Date.now() - 100, expiresAt: Date.now() + 600000 }));
  const capabilityFile = join(temporary, 'grant.json');
  const grant = { directory, workspaceId: 'workspace', sessionId: 'preview-session', capabilityFile,
    grantId: 'delivery-one-fixture', contextSha256: createHash('sha256').update(JSON.stringify(context)).digest('hex') };
  await writeFile(capabilityFile, JSON.stringify({ ...grant, version: 1, selectionIds: [selectionId],
    allowedTools: ['arkdock.get_selection_screenshot', 'arkdock.get_selection'], expiresAt: Date.now() + 600000 }));
  return { temporary, context, imagePath, delivery: { deliveryId: 'delivery-one', selectionId,
    prompt: 'Only a protocol fixture; change no files.', grant, images: [{ path: imagePath,
      sha256: screenshot.sha256, byteLength: screenshot.byteLength, mimeType: 'image/jpeg' }] } };
}

class FixtureRpc extends EventEmitter {
  constructor(cwd) {
    super(); this.calls = []; this.closed = false; this.child = { pid: process.pid };
    this.thread = { id: 'thread-owned', sessionId: 'root-owned', cliVersion: '0.159.2', cwd, status: { type: 'idle' } };
    this.turnId = 'turn-one'; this.complete = true; this.emitUserItem = true; this.error = null;
  }
  write(message) { this.calls.push(message); }
  async request(method, params) {
    this.calls.push({ method, params });
    if (method === 'initialize') return { platformOs: 'windows' };
    if (method === 'thread/start' || method === 'thread/read') return { thread: structuredClone(this.thread) };
    if (method === 'turn/start' || method === 'turn/steer') {
      if (this.error) throw new Error(this.error);
      this.emit('notification', { method: 'turn/started', params: { threadId: this.thread.id, turn: { id: this.turnId } } });
      if (this.emitUserItem) this.emit('notification', { method: 'item/started', params: { threadId: this.thread.id,
        turnId: this.turnId, item: { type: 'userMessage', id: 'server-item-id', clientId: params.clientUserMessageId } } });
      if (this.complete) this.emit('notification', { method: 'turn/completed', params: { threadId: this.thread.id,
        turn: { id: this.turnId, status: 'completed' } } });
      return method === 'turn/start' ? { turn: { id: this.turnId, status: 'inProgress' } } : { turnId: this.turnId };
    }
    throw new Error('FIXTURE_UNKNOWN_METHOD');
  }
  dispose() { this.closed = true; this.emit('disconnect', 'DISPOSED'); }
}

async function managedFixture(t) {
  const fixture = await selectionFixture(t);
  const rpc = new FixtureRpc(fixture.temporary);
  const registrations = [], receipts = [], polls = [];
  const bridge = { register: async (body) => { registrations.push(body); return { registrationId: 'registration' }; },
    receipt: async (body) => { receipts.push(body); return { accepted: true }; },
    poll: async (body) => { polls.push(body); return { deliveries: [] }; }, revoke: async () => ({ accepted: true }) };
  const transport = new ManagedCodexTransport({ rpc, bridge, workspaceRoot: fixture.temporary, workspaceId: 'workspace' });
  await transport.initialize();
  return { ...fixture, rpc, bridge, transport, registrations, receipts, polls };
}

test('managed thread scope, authorized localImage and correlated lifecycle receipts', async (t) => {
  const f = await managedFixture(t);
  assert.equal(f.registrations[0].sessionId, 'root-owned');
  assert.equal(f.registrations[0].conversationId, 'thread-owned');
  assert.equal(f.rpc.calls.find((call) => call.method === 'thread/start').params.sandbox, 'read-only');
  await f.transport.submitMessage(f.delivery);
  const start = f.rpc.calls.find((call) => call.method === 'turn/start');
  assert.equal(start.params.threadId, 'thread-owned');
  assert.deepEqual(start.params.input[1], { type: 'localImage', path: await realpath(f.imagePath) });
  assert.deepEqual(f.receipts.map((receipt) => receipt.state), ['TRANSPORT_ACCEPTED', 'SUBMITTED', 'COMPLETED']);
  assert.equal(f.registrations[0].capabilities.agentAcknowledgment, false);
  assert.equal(f.rpc.calls.filter((call) => call.method === 'thread/read').length, 3);
  await assert.rejects(f.transport.submitMessage(f.delivery), /DUPLICATE_DELIVERY/);
  assert.ok(f.rpc.calls.every((call) => !['thread/resume', 'thread/list'].includes(call.method)));
});

test('busy keeps Bridge lease without dequeue; steering requires exact active turn', async (t) => {
  const f = await managedFixture(t);
  f.rpc.thread.status = { type: 'active', activeFlags: [] };
  await f.transport.pollOnce();
  assert.equal(f.polls[0].busy, true);
  await assert.rejects(f.transport.submitMessage(f.delivery), /AGENT_BUSY/);
  await assert.rejects(f.transport.submitMessage(f.delivery, { steer: true, expectedTurnId: 'wrong' }), /AGENT_BUSY/);
  f.transport.activeTurnId = f.rpc.turnId;
  await f.transport.submitMessage(f.delivery, { steer: true, expectedTurnId: f.rpc.turnId });
  assert.equal(f.rpc.calls.find((call) => call.method === 'turn/steer').params.expectedTurnId, 'turn-one');
});

test('workspace or thread mutation blocks dispatch and does not read another history', async (t) => {
  const f = await managedFixture(t);
  f.rpc.thread.id = 'unrelated-thread';
  await assert.rejects(f.transport.submitMessage(f.delivery), /THREAD_SCOPE_MISMATCH/);
  f.rpc.thread.id = 'thread-owned'; f.rpc.thread.cwd = join(f.temporary, 'selections');
  await assert.rejects(f.transport.submitMessage(f.delivery), /THREAD_SCOPE_MISMATCH/);
  assert.ok(!f.rpc.calls.some((call) => call.method === 'turn/start'));
});

test('grant workspace, image tamper and arbitrary file references block sends', async (t) => {
  const f = await managedFixture(t);
  await assert.rejects(f.transport.prepareDelivery({ ...f.delivery,
    grant: { ...f.delivery.grant, workspaceId: 'other' } }), /GRANT_SCOPE_MISMATCH/);
  const foreign = join(f.temporary, 'foreign.jpg'); await writeFile(foreign, 'foreign');
  await assert.rejects(f.transport.prepareDelivery({ ...f.delivery,
    images: [{ ...f.delivery.images[0], path: foreign }] }), /IMAGE_NOT_AUTHORIZED/);
  await writeFile(f.imagePath, 'tampered');
  await assert.rejects(f.transport.submitMessage(f.delivery), /IMAGE_INTEGRITY_MISMATCH/);
});

test('Rust extended local-drive image paths resolve to the exact authorized Selection artifact', { skip: process.platform !== 'win32' }, async (t) => {
  const f = await managedFixture(t);
  const input = await f.transport.prepareDelivery({ ...f.delivery,
    images: [{ ...f.delivery.images[0], path: toNamespacedPath(f.imagePath) }] });
  assert.equal(input[1].type, 'localImage');
  assert.equal(input[1].path, await realpath(f.imagePath));
});

test('turn completion without correlated user-item submission remains UNKNOWN', async (t) => {
  const f = await managedFixture(t); f.rpc.emitUserItem = false;
  await f.transport.submitMessage(f.delivery);
  assert.deepEqual(f.receipts.map((receipt) => receipt.state), ['TRANSPORT_ACCEPTED', 'UNKNOWN']);
});

test('cross-thread/cross-turn notifications never mark another delivery submitted', async (t) => {
  const f = await managedFixture(t); f.rpc.complete = false; f.rpc.emitUserItem = false;
  await f.transport.submitMessage(f.delivery);
  for (const [threadId, turnId] of [['other', 'turn-one'], ['thread-owned', 'other']]) {
    f.rpc.emit('notification', { method: 'item/started', params: { threadId, turnId,
      item: { type: 'userMessage', id: 'server-item-id', clientId: f.delivery.deliveryId } } });
  }
  await f.transport.receipts;
  assert.deepEqual(f.receipts.map((receipt) => receipt.state), ['TRANSPORT_ACCEPTED']);
  await f.transport.dispose();
  assert.equal(f.receipts.at(-1).state, 'UNKNOWN');
});

test('ambiguous timeout is UNKNOWN and cannot automatically retry; rejected request is FAILED', async (t) => {
  const f = await managedFixture(t); f.rpc.error = 'APP_SERVER_RESPONSE_TIMEOUT';
  await assert.rejects(f.transport.submitMessage(f.delivery), /APP_SERVER_RESPONSE_TIMEOUT/);
  assert.equal(f.receipts.at(-1).state, 'UNKNOWN');
  await assert.rejects(f.transport.submitMessage(f.delivery), /DUPLICATE_DELIVERY/);
  f.rpc.error = 'APP_SERVER_REQUEST_REJECTED';
  const replacement = { ...f.delivery.grant, grantId: 'delivery-two-fixture' };
  const stored = JSON.parse(await readFile(replacement.capabilityFile, 'utf8'));
  await writeFile(replacement.capabilityFile, JSON.stringify({ ...stored, grantId: replacement.grantId }));
  await assert.rejects(f.transport.submitMessage({ ...f.delivery, deliveryId: 'delivery-two', grant: replacement }), /APP_SERVER_REQUEST_REJECTED/);
  assert.equal(f.receipts.at(-1).state, 'FAILED');
});

test('real stdio protocol fixture covers fragmented JSON, correlation, approval decline and bounded failure', async (t) => {
  const script = `const rl=require('node:readline').createInterface({input:process.stdin});
    rl.on('line',line=>{const r=JSON.parse(line); if(r.method==='initialize'){
      process.stdout.write('{"id":'+r.id+',"result":');setTimeout(()=>process.stdout.write('{"platformOs":"windows"}}\\n'),5);
    }else if(r.method==='approval')process.stdout.write(JSON.stringify({id:71,method:'item/commandExecution/requestApproval',params:{}})+'\\n');
    else if(r.id===71)process.stdout.write(JSON.stringify({method:'approvalResult',params:r.result})+'\\n');
    else if(r.method==='oversize')process.stdout.write('x'.repeat(1024*1024+1));});`;
  const child = spawn(process.execPath, ['-e', script], { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  const rpc = new AppServerRpc(child, { timeoutMs: 1000 }); t.after(() => rpc.dispose());
  assert.equal((await rpc.request('initialize', {})).platformOs, 'windows');
  const approval = once(rpc, 'notification'); rpc.write({ method: 'approval' });
  assert.deepEqual((await approval)[0], { method: 'approvalResult', params: { decision: 'decline' } });
  const disconnected = once(rpc, 'disconnect'); rpc.write({ method: 'oversize' });
  assert.equal((await disconnected)[0], 'APP_SERVER_FRAME_LIMIT');
});

test('owned App Server disposal allows delayed graceful EOF cleanup before forcing a child exit', async () => {
  const child = spawn(process.execPath, ['-e',
    'process.stdin.resume(); process.stdin.on("end",()=>setTimeout(()=>process.exit(0),75));'],
  { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  const rpc = new AppServerRpc(child);
  await rpc.dispose();
  assert.equal(child.exitCode, 0);
  assert.equal(child.killed, false);
  assert.equal(rpc.closed, true);
});

test('Bridge uses literal loopback bearer auth, rejects redirect/oversize and validates pairing expiry', async (t) => {
  for (const value of ['http://127.1:9000', 'http://localhost:9000', 'http://127.0.0.1:9000/path', 'https://127.0.0.1:9000',
    'http://127.0.0.1:9000@evil.example', 'http://2130706433:9000']) assert.throws(() => validateEndpoint(value));
  const seen = [];
  const server = createServer((request, response) => {
    seen.push({ url: request.url, authorization: request.headers.authorization });
    request.resume();
    if (request.url === '/v1/register') { response.writeHead(302, { location: '/stolen' }); response.end(); }
    else { response.setHeader('content-type', 'application/json'); response.end(' '.repeat(128 * 1024 + 1)); }
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  t.after(() => new Promise((done) => server.close(done)));
  const pair = { endpoint: `http://127.0.0.1:${server.address().port}`, token: 'a'.repeat(48),
    workspaceId: 'workspace', workspaceRoot: (await selectionFixture(t)).temporary, expiresAt: Date.now() + 60000 };
  const client = new BridgeClient(pair);
  await assert.rejects(client.register({}), /BRIDGE_UNREACHABLE/);
  assert.equal(seen.length, 1); assert.equal(seen[0].authorization, `Bearer ${pair.token}`);
  await assert.rejects(client.poll({}), /BRIDGE_RESPONSE_LIMIT/);
  const file = join(pair.workspaceRoot, 'pairing.json');
  await writeFile(file, JSON.stringify(pair));
  assert.equal((await readPairingFile(file)).workspaceId, 'workspace');
  await assert.rejects(readPairingFile(file, () => pair.expiresAt), /PAIRING_EXPIRED/);
});

test('extended Windows local drive roots normalize without flattening UNC, WSL or device identity', () => {
  assert.equal(normalizeLocalWorkspaceRoot('\\\\?\\D:\\Project Root\\ArkDock', 'win32'), 'D:\\Project Root\\ArkDock');
  assert.equal(normalizeLocalWorkspaceRoot('D:/Project Root/ArkDock', 'win32'), 'D:/Project Root/ArkDock');
  for (const value of ['\\\\?\\UNC\\server\\share', '\\\\wsl.localhost\\Ubuntu\\home\\project',
    '\\\\server\\share', '\\\\.\\C:\\project', '\\\\?\\Volume{0000}\\project', '/home/project', 'C:relative', '\\project']) {
    assert.throws(() => normalizeLocalWorkspaceRoot(value, 'win32'), /PAIRING_INVALID/);
  }
});

async function desktopFixture(t) {
  const f = await selectionFixture(t); const registrations = [], receipts = [], revoked = [];
  let now = Date.now(); let roots = [{ uri: pathToFileURL(f.temporary).href }];
  const meta = { 'openai/session': 'host-conversation' };
  const delivery = { ...f.delivery, workspaceId: 'workspace', sessionId: 'host-conversation', conversationId: 'host-conversation' };
  let queued = [delivery];
  const bridge = new DesktopSessionBridge({ pairing: { workspaceRoot: f.temporary, workspaceId: 'workspace' },
    now: () => now, roots: async () => roots,
    bridge: { register: async (body) => { registrations.push(body); return { registrationId: 'registered' }; },
      poll: async () => { const deliveries = queued; queued = []; return { deliveries }; },
      receipt: async (body) => { receipts.push(body); return { accepted: true }; },
      revoke: async (body) => { revoked.push(body); return { accepted: true }; } } });
  bridge.clientInfo = { name: 'Codex Desktop', version: '26.1007.2314.0' };
  return { ...f, meta, bridge, registrations, receipts, revoked,
    setRoots: (value) => { roots = value; }, advance: (ms) => { now += ms; } };
}

test('Desktop optional bridge needs host session metadata, exact one host root and explicit UI consent', async (t) => {
  const f = await desktopFixture(t);
  await assert.rejects(f.bridge.call('arkdock.bridge_connect', { uiInstance: 'ui-one', consent: true }, {}), /DESKTOP_SESSION_UNVERIFIED/);
  await assert.rejects(f.bridge.call('arkdock.bridge_connect', { uiInstance: 'ui-one', consent: true, sessionId: 'self-asserted' }, f.meta), /DESKTOP_TOOL_ARGUMENTS_INVALID/);
  f.setRoots([]);
  await assert.rejects(f.bridge.call('arkdock.bridge_connect', { uiInstance: 'ui-one', consent: true }, f.meta), /DESKTOP_WORKSPACE_UNVERIFIED/);
  f.setRoots([{ uri: pathToFileURL(f.temporary).href }]);
  await assert.rejects(f.bridge.call('arkdock.bridge_connect', { uiInstance: 'ui-one', consent: false }, f.meta), /DESKTOP_CONSENT_REQUIRED/);
  await f.bridge.call('arkdock.bridge_connect', { uiInstance: 'ui-one', consent: true }, f.meta);
  assert.equal(f.registrations[0].transportKind, 'codex-desktop-plugin');
  assert.equal(f.registrations[0].conversationId, f.meta['openai/session']);
});

test('Desktop ui/message completion only accepts transport; model explicitly acknowledges and completes', async (t) => {
  const f = await desktopFixture(t);
  await f.bridge.call('arkdock.bridge_connect', { uiInstance: 'ui-one', consent: true }, f.meta);
  const response = await f.bridge.call('arkdock.bridge_poll', { uiInstance: 'ui-one' }, f.meta);
  assert.equal(response.structuredContent.deliveries.length, 1);
  assert.equal(f.receipts.length, 0);
  const args = { deliveryId: f.delivery.deliveryId };
  await assert.rejects(f.bridge.call('arkdock.desktop_acknowledge', args, f.meta), /DESKTOP_RECEIPT_ORDER_INVALID/);
  await f.bridge.call('arkdock.bridge_sent', { ...args, uiInstance: 'ui-one', success: true }, f.meta);
  assert.deepEqual(f.receipts.map((r) => r.state), ['TRANSPORT_ACCEPTED']);
  await assert.rejects(f.bridge.call('arkdock.desktop_complete', { ...args, success: true, summary: 'done' }, f.meta), /DESKTOP_RECEIPT_ORDER_INVALID/);
  await f.bridge.call('arkdock.desktop_acknowledge', args, f.meta);
  const image = await f.bridge.call('arkdock.desktop_screenshot', { ...args, selectionId: f.delivery.selectionId }, f.meta);
  assert.equal(image.content[0].type, 'image');
  await f.bridge.call('arkdock.desktop_complete', { ...args, success: true, summary: 'Fixture only' }, f.meta);
  assert.deepEqual(f.receipts.map((r) => r.state), ['TRANSPORT_ACCEPTED', 'AGENT_ACKNOWLEDGED', 'COMPLETED']);
  assert.ok(f.receipts.every((r) => !('workspaceRoot' in r) && typeof r.reason === 'string'));
  assert.deepEqual(f.bridge.listTools().find((tool) => tool.name === 'arkdock.desktop_acknowledge')._meta.ui.visibility, ['model']);
});

test('Desktop thread switch, expired UI lease and changed roots revoke pairing before another poll', async (t) => {
  for (const mutation of ['session', 'lease', 'roots']) {
    const f = await desktopFixture(t);
    await f.bridge.call('arkdock.bridge_connect', { uiInstance: 'ui-one', consent: true }, f.meta);
    if (mutation === 'lease') f.advance(5001);
    if (mutation === 'roots') f.setRoots([{ uri: pathToFileURL(join(f.temporary, 'selections')).href }]);
    await assert.rejects(f.bridge.call('arkdock.bridge_poll', { uiInstance: 'ui-one' },
      mutation === 'session' ? { 'openai/session': 'other-conversation' } : f.meta), /DESKTOP_(SESSION_CHANGED|UI_LEASE_EXPIRED|WORKSPACE_MISMATCH)/);
    assert.equal(f.revoked.length, 1); assert.equal(f.bridge.closed, true);
  }
});

test('Desktop MCP stdio obtains roots through a host request and session only from the host envelope', async (t) => {
  const f = await desktopFixture(t), input = new PassThrough(), replies = [];
  const send = (message) => input.write(`${JSON.stringify({ jsonrpc: '2.0', ...message })}\n`);
  const output = new Writable({ write(chunk, _encoding, callback) {
    const message = JSON.parse(chunk.toString()); replies.push(message);
    if (message.method === 'roots/list') queueMicrotask(() => send({ id: message.id,
      result: { roots: [{ uri: pathToFileURL(f.temporary).href }] } }));
    if (message.id === 8) input.end();
    callback();
  } });
  const done = runDesktopStdio(f.bridge, input, output);
  send({ id: 1, method: 'initialize', params: { protocolVersion: '2025-11-25',
    capabilities: { roots: { listChanged: true } }, clientInfo: { name: 'Codex Desktop', version: 'fixture' } } });
  send({ method: 'notifications/initialized' });
  send({ id: 2, method: 'tools/list' });
  send({ id: 3, method: 'resources/read', params: { uri: 'ui://arkdock/session-bridge.html' } });
  send({ id: 4, method: 'tools/call', params: { name: 'arkdock.bridge_connect',
    arguments: { uiInstance: 'ui-one', consent: true, _meta: f.meta } } });
  send({ id: 5, method: 'tools/call', params: { name: 'arkdock.bridge_connect',
    arguments: { uiInstance: 'ui-one', consent: true } } });
  send({ id: 6, method: 'tools/call', params: { name: 'arkdock.bridge_connect',
    arguments: { uiInstance: 'ui-one', consent: true }, _meta: f.meta } });
  send({ method: 'notifications/roots/list_changed' });
  send({ id: 7, method: 'tools/call', params: { name: 'arkdock.bridge_poll', arguments: { uiInstance: 'ui-one' }, _meta: f.meta } });
  send({ id: 8, method: 'ping' });
  await done;
  const result = (id) => replies.find((message) => message.id === id).result;
  assert.equal(result(1).protocolVersion, '2025-11-25');
  assert.equal(result(3).contents[0].mimeType, 'text/html;profile=mcp-app');
  assert.match(result(4).content[0].text, /DESKTOP_TOOL_ARGUMENTS_INVALID/);
  assert.match(result(5).content[0].text, /DESKTOP_SESSION_UNVERIFIED/);
  assert.equal(result(6).structuredContent.connected, true);
  assert.match(result(7).content[0].text, /DESKTOP_BRIDGE_CLOSED/);
  assert.equal(f.registrations.length, 1); assert.equal(f.revoked.length, 1);
});

test('Desktop UI messages only use the parent host; no pairing token, roots or session claims cross from UI', async () => {
  const html = await readFile(new URL('../../tools/agent/codex-desktop-ui.html', import.meta.url), 'utf8');
  const source = html.match(/<script type="module">([\s\S]*?)<\/script>/)[1];
  const nodes = new Map(['status', 'connect', 'stop'].map((id) => [id, { disabled: true, textContent: '' }]));
  const handlers = new Map(), sent = []; let delayInitialize;
  const dispatch = (data, sender) => handlers.get('message')({ source: sender, origin: 'https://fixture-host.example', data });
  const parent = { postMessage(message) {
    sent.push(message);
    if (message.method === 'ui/initialize') { delayInitialize = () => dispatch({ jsonrpc: '2.0', id: message.id,
      result: { protocolVersion: '2026-01-26', hostCapabilities: { serverTools: {}, message: { text: {} }, experimental: { 'openai/message': {} } } } }, parent); return; }
    if (message.id === undefined) return;
    const result = message.method === 'tools/call' ? message.params.name === 'arkdock.bridge_poll'
      ? { structuredContent: { deliveries: [{ deliveryId: 'fixture-delivery', prompt: 'synthetic fixture' }] } }
      : { structuredContent: { connected: true } } : {};
    queueMicrotask(() => {
      dispatch({ jsonrpc: '2.0', id: message.id, result }, parent);
      if (message.params?.name === 'arkdock.bridge_sent') {
        queueMicrotask(() => dispatch({ jsonrpc: '2.0', method: 'ui/notifications/host-context-changed', params: {} }, parent));
      }
    });
  } };
  const window = { parent, addEventListener: (name, listener) => handlers.set(name, listener) };
  const initialized = runInNewContext(`(async () => {${source}})()`, { window,
    document: { getElementById: (id) => nodes.get(id) }, crypto: { randomUUID: () => 'ui-fixture' },
    setTimeout, clearTimeout, Map, Promise, Error });
  dispatch({ jsonrpc: '2.0', id: 1, result: { hostCapabilities: { message: { text: {} }, experimental: { 'openai/message': {} } } } }, {});
  await new Promise((done) => setImmediate(done));
  assert.equal(nodes.get('connect').disabled, true);
  delayInitialize(); await initialized;
  assert.equal(nodes.get('connect').disabled, false);
  await nodes.get('connect').onclick();
  const message = sent.find((item) => item.method === 'ui/message');
  assert.equal(message.params._meta['openai/message'].target, 'active');
  assert.equal(message.params._meta['openai/message'].send, true);
  assert.equal(sent.filter((item) => item.method === 'ui/message').length, 1);
  const tools = sent.filter((item) => item.method === 'tools/call');
  assert.ok(tools.every((item) => !('_meta' in item.params)
    && !('sessionId' in item.params.arguments) && !('workspaceRoot' in item.params.arguments)));
  assert.ok(sent.every((item) => !JSON.stringify(item).includes('token')));
  assert.equal(tools.find((item) => item.params.name === 'arkdock.bridge_sent').params.arguments.success, true);
  assert.ok(tools.some((item) => item.params.name === 'arkdock.bridge_disconnect'));
});
