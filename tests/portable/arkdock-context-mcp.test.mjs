import assert from 'node:assert/strict';
import net from 'node:net';
import { createHmac } from 'node:crypto';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import test from 'node:test';
import { normalizeRoots, rootUriToPath } from '../../tools/agent/arkdock-context.mjs';

const script = join(process.cwd(), 'tools', 'agent', 'arkdock-context.mjs');
const node = process.execPath;
const declaration = { originalProject: 'C:\\repos\\app', executionDirectory: 'C:\\repos\\app-worktree\\task', environment: 'windowsLocal', hostSession: 'real-host-session' };

function inbox() {
  const items = [];
  const waiters = [];
  return {
    items,
    push(item) {
      const index = waiters.findIndex((waiter) => waiter.predicate(item));
      if (index < 0) { items.push(item); return; }
      const waiter = waiters.splice(index, 1)[0];
      clearTimeout(waiter.timer);
      waiter.resolve(item);
    },
    next(predicate, label, timeout = 7000) {
      const index = items.findIndex(predicate);
      if (index >= 0) return Promise.resolve(items.splice(index, 1)[0]);
      return new Promise((resolvePromise, reject) => {
        const waiter = { predicate, resolve: resolvePromise };
        waiter.timer = setTimeout(() => {
          waiters.splice(waiters.indexOf(waiter), 1);
          reject(new Error(`Timed out: ${label}`));
        }, timeout);
        waiters.push(waiter);
      });
    },
  };
}

function hostPipe(path, options = {}) {
  const incoming = inbox();
  const sockets = new Set();
  const owners = new WeakMap();
  const allFrames = [];
  const ack = (frame, result = {}) => owners.get(frame)?.write(`${JSON.stringify({ kind: 'ack', session: frame.session, sequence: frame.sequence, ok: true, ...result })}\n`);
  const server = net.createServer((socket) => {
    sockets.add(socket);
    socket.on('close', () => sockets.delete(socket));
    socket.on('error', () => {});
    socket.setEncoding('utf8');
    let buffer = '';
    socket.on('data', (chunk) => {
      buffer += chunk;
      let index;
      while ((index = buffer.indexOf('\n')) >= 0) {
        const line = buffer.slice(0, index);
        buffer = buffer.slice(index + 1);
        assert.ok(Buffer.byteLength(line) <= 16384, 'all outgoing pipe frames are bounded');
        const frame = JSON.parse(line);
        owners.set(frame, socket);
        allFrames.push(frame);
        if (frame.kind === 'probe') {
          if (options.proof === 'oversized') {
            socket.write('x'.repeat(16385));
            incoming.push(frame);
            continue;
          }
          if (options.proof !== 'silent') {
            const proof = createHmac('sha256', Buffer.from('f'.repeat(64), 'hex')).update(`arkdock-context-server-v1\0${frame.challenge}`).digest('hex');
            const response = { kind: 'proof', challenge: frame.challenge, proof };
            if (options.proof === 'invalid') response.proof = '0'.repeat(64);
            if (options.proof === 'wrong-challenge') response.challenge = '0'.repeat(64);
            if (options.proof === 'malformed') response.proof = 'not-hex';
            socket.write(`${JSON.stringify(response)}\n`);
          }
        } else if (options.autoAck !== false) ack(frame, options.reject ? { ok: false, error: options.reject } : {});
        incoming.push(frame);
      }
    });
  });
  const listening = new Promise((resolvePromise, reject) => {
    server.once('error', reject);
    server.listen(path, resolvePromise);
  });
  return {
    listening, allFrames, ack,
    next(kind, predicate = () => true) { return incoming.next((frame) => frame.kind === kind && predicate(frame), `pipe ${kind}`); },
    disconnect() { for (const socket of sockets) socket.destroy(); },
    close() { for (const socket of sockets) socket.destroy(); return new Promise((resolvePromise) => server.close(resolvePromise)); },
  };
}

function client(command, args, cwd) {
  const child = spawn(command, args, { cwd, stdio: ['pipe', 'pipe', 'pipe'] });
  const messages = inbox();
  let stderr = '';
  let buffer = '';
  let id = 0;
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  child.stdout.on('data', (chunk) => {
    buffer += chunk;
    let index;
    while ((index = buffer.indexOf('\n')) >= 0) {
      messages.push(JSON.parse(buffer.slice(0, index)));
      buffer = buffer.slice(index + 1);
    }
  });
  const exited = new Promise((resolvePromise) => child.once('exit', (code) => resolvePromise(code)));
  const closed = new Promise((resolvePromise) => child.once('close', resolvePromise));
  const send = (message) => child.stdin.write(`${JSON.stringify(message)}\n`);
  const request = (method, params = {}) => {
    const requestId = ++id;
    send({ jsonrpc: '2.0', id: requestId, method, params });
    return messages.next((message) => message.id === requestId && !message.method, `response ${method}`).catch((error) => { throw new Error(`${error.message}; stderr=${stderr}`); });
  };
  return {
    child, exited, messages, request,
    stderr: () => stderr,
    notify(method) { send({ jsonrpc: '2.0', method }); },
    respond(requestId, result, error) { send({ jsonrpc: '2.0', id: requestId, ...(error ? { error } : { result }) }); },
    next(method) { return messages.next((message) => message.method === method, method); },
    async initialize(capabilities = {}, version = '2025-11-25', complete = true) {
      const response = await request('initialize', { protocolVersion: version, capabilities, clientInfo: { name: 'isolated-test', version: '1' } });
      if (complete) send({ jsonrpc: '2.0', method: 'notifications/initialized' });
      return response;
    },
    async call(name, args = {}) {
      const response = await request('tools/call', { name, arguments: args });
      return { response, status: response.result?.content?.[0]?.text ? JSON.parse(response.result.content[0].text) : undefined };
    },
    async stop() {
      child.stdin.end();
      const timer = setTimeout(() => child.kill(), 4000);
      await exited;
      await closed;
      clearTimeout(timer);
    },
  };
}

async function fixture(t, name, options = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'arkdock-context-'));
  const pipe = process.platform === 'win32' ? `\\\\.\\pipe\\arkdock-context-test-${process.pid}-${name}` : join(directory, 'test.sock');
  const host = hostPipe(pipe, options);
  await host.listening;
  const file = join(directory, 'pairing.json');
  const credential = 'f'.repeat(64);
  writeFileSync(file, JSON.stringify({ version: 1, pipeName: pipe, credential }));
  const session = client(node, [script, 'serve', '--pairing-file', file, '--agent', '测试工具'], directory);
  t.after(async () => {
    await session.stop();
    await host.close();
    assert.equal(resolve(dirname(directory)), resolve(tmpdir()), 'recursive cleanup stays in the explicitly allocated test temp directory');
    assert.ok(directory.includes('arkdock-context-'));
    rmSync(directory, { recursive: true, force: true });
  });
  return { host, session, directory, file, credential };
}

async function eventually(operation, predicate) {
  for (let count = 0; count < 100; count++) {
    const value = await operation();
    if (predicate(value)) return value;
    await delay(20);
  }
  assert.fail('condition not reached');
}

test('configuration names a runnable absolute script and preserves pairing-file pipe choice', async (t) => {
  const { directory, file, host } = await fixture(t, 'config');
  const out = join(directory, 'generated');
  const result = spawnSync(node, [script, '--write-config', out, '--pairing-file', file]);
  assert.equal(result.status, 0, result.stderr.toString());
  const config = JSON.parse(readFileSync(join(out, 'claude-mcp-json.example.json'), 'utf8')).mcpServers['arkdock-context'];
  assert.equal(config.command, node);
  assert.equal(config.args[0], script);
  assert.ok(!config.args.includes('--pipe'), 'default must not mask pairing-file pipeName');
  assert.ok(readFileSync(join(out, 'codex-config.example.toml'), 'utf8').includes(JSON.stringify(config.args)));
  const launched = client(config.command, config.args, directory);
  try {
    assert.equal((await launched.initialize()).result.serverInfo.name, 'arkdock-context');
    await host.next('hello');
    assert.equal((await launched.call('arkdock_context_renew')).response.result.isError, false);
    assert.equal(spawnSync(node, [script, '--write-config', out, '--pairing-file', file]).status, 2);
    assert.ok(!readFileSync(join(out, 'README.md'), 'utf8').includes('f'.repeat(64)));
  } finally { await launched.stop(); }
});

test('initialization negotiates a supported version and waits for initialized before roots/list', async (t) => {
  const { session, host } = await fixture(t, 'lifecycle');
  assert.equal((await session.request('tools/list')).error.code, -32000);
  const init = await session.initialize({ roots: { listChanged: true } }, '2099-01-01', false);
  assert.equal(init.result.protocolVersion, '2025-11-25');
  await delay(70);
  assert.equal(session.messages.items.some((message) => message.method === 'roots/list'), false);
  assert.equal(host.allFrames.length, 0);
  session.notify('notifications/initialized');
  const roots = await session.next('roots/list');
  session.respond(roots.id, { roots: [{ uri: 'file:///C:/workspace/app' }, { uri: 'file:///home/remote/app' }, { uri: 'file://wsl.localhost/Ubuntu/app' }] });
  const hello = await host.next('hello');
  assert.deepEqual(hello.roots, ['C:\\workspace\\app', '/home/remote/app', '\\\\wsl.localhost\\Ubuntu\\app']);
  assert.equal(hello.sequence, 1);
  assert.equal((await session.initialize({}, '2025-11-25', false)).error.code, -32602);
});

test('explicit connect/update/renew/revoke carries original and worktree without roots capability', async (t) => {
  const { session, host, credential } = await fixture(t, 'tools');
  await session.initialize();
  const hello = await host.next('hello');
  assert.deepEqual(hello.roots, []);
  const names = (await session.request('tools/list')).result.tools.map((tool) => tool.name);
  assert.deepEqual(names, ['arkdock_connect_current_project', 'arkdock_context_update', 'arkdock_context_renew', 'arkdock_context_revoke', 'arkdock_context_status']);
  const connected = await session.call('arkdock_connect_current_project', declaration);
  assert.equal(connected.response.result.isError, false);
  assert.equal(connected.status.authenticated, true);
  assert.equal(connected.status.requiresConfirmation, true);
  assert.equal(connected.status.visibleConversationVerified, false);
  assert.equal(connected.status.originalProject, declaration.originalProject);
  assert.equal(connected.status.executionDirectory, declaration.executionDirectory);
  assert.ok(!JSON.stringify(connected.response).includes(credential));
  const update = await host.next('update');
  assert.equal(update.hostSession, declaration.hostSession);
  assert.equal(update.token, credential);
  assert.ok(update.sequence > hello.sequence);
  const second = { ...declaration, executionDirectory: 'C:\\repos\\app-worktree\\other' };
  delete second.hostSession;
  assert.equal((await session.call('arkdock_context_update', second)).response.result.isError, false);
  const replacement = await host.next('update');
  assert.equal(replacement.hostSession, undefined, 'omitted optional metadata is cleared');
  assert.equal(replacement.executionDirectory, second.executionDirectory);
  await session.call('arkdock_context_renew');
  const renewed = await host.next('heartbeat');
  assert.ok(renewed.sequence > replacement.sequence);
  const revoked = await session.call('arkdock_context_revoke');
  assert.equal(revoked.status.registered, false);
  assert.equal(revoked.status.enabled, false);
  await host.next('revoke');
  assert.equal((await session.call('arkdock_context_renew')).response.result.isError, true);
  await delay(5100);
  assert.equal(host.allFrames.filter((frame) => frame.kind === 'hello').length, 1, 'revocation disables automatic retry');
  assert.equal((await session.call('arkdock_connect_current_project', second)).response.result.isError, false);
  const restarted = await host.next('hello');
  assert.ok(restarted.sequence > renewed.sequence);
});

test('initialized notification and tools request can share one stdio chunk', async (t) => {
  const { session } = await fixture(t, 'pipeline');
  await session.initialize({}, '2025-03-26', false);
  session.child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' })}\n${JSON.stringify({ jsonrpc: '2.0', id: 999, method: 'tools/list', params: {} })}\n`);
  const response = await session.messages.next((message) => message.id === 999, 'pipelined tools/list');
  assert.ok(response.result.tools.length >= 5);
});

test('transport connect is not authenticated until a matching backend ACK', async (t) => {
  const { session, host } = await fixture(t, 'ack', { autoAck: false });
  await session.initialize();
  const hello = await host.next('hello');
  const status = (await session.call('arkdock_context_status')).status;
  assert.equal(status.connected, true);
  assert.equal(status.serverVerified, true);
  assert.equal(status.authenticated, false);
  assert.equal(status.registered, false);
  host.ack(hello);
  await eventually(() => session.call('arkdock_context_status'), (result) => result.status.authenticated);
  const called = session.call('arkdock_connect_current_project', declaration);
  const update = await host.next('update');
  host.ack(update, { ok: false, error: 'CONTEXT_TOKEN_INVALID' });
  const rejected = await called;
  assert.equal(rejected.response.result.isError, true);
  assert.equal(rejected.status.authenticated, false);
  await eventually(() => session.call('arkdock_context_status'), (result) => !result.status.authenticated);
});

test('missing ACK times out rather than claiming a successful registration', async (t) => {
  const { session, host } = await fixture(t, 'ack-timeout', { autoAck: false });
  await session.initialize();
  await host.next('hello');
  await delay(3200);
  const status = (await session.call('arkdock_context_status')).status;
  assert.equal(status.authenticated, false);
  assert.equal(status.lastError, 'CONTEXT_ACK_TIMEOUT');
});

test('roots changes clear stale backend roots before refresh, including rejected response', async (t) => {
  const { session, host } = await fixture(t, 'root-change');
  await session.initialize({ roots: { listChanged: true } });
  let request = await session.next('roots/list');
  session.respond(request.id, { roots: [{ uri: 'file:///C:/old' }] });
  await host.next('hello');
  session.notify('notifications/roots/list_changed');
  assert.deepEqual((await host.next('update')).roots, []);
  request = await session.next('roots/list');
  session.respond(request.id, { roots: [{ uri: 'file:///C:/new' }] });
  assert.deepEqual((await host.next('update')).roots, ['C:\\new']);
  session.notify('notifications/roots/list_changed');
  assert.deepEqual((await host.next('update')).roots, []);
  request = await session.next('roots/list');
  session.respond(request.id, undefined, { code: -32601, message: 'unsupported now' });
  assert.deepEqual((await host.next('update')).roots, []);
  assert.equal((await session.call('arkdock_context_status')).status.rootSource, 'roots-rejected');
});

test('a delayed roots response cannot revive roots invalidated by a newer change', async (t) => {
  const { session, host } = await fixture(t, 'root-race');
  await session.initialize({ roots: { listChanged: true } });
  const stale = await session.next('roots/list');
  session.notify('notifications/roots/list_changed');
  session.respond(stale.id, { roots: [{ uri: 'file:///C:/stale' }] });
  assert.deepEqual((await host.next('hello')).roots, []);
  const fresh = await session.next('roots/list');
  session.respond(fresh.id, { roots: [{ uri: 'file:///C:/fresh' }] });
  await host.next('update', (frame) => frame.roots.includes('C:\\fresh'));
  assert.equal(host.allFrames.some((frame) => frame.roots?.includes('C:\\stale')), false);
});

test('reconnection re-announces explicit metadata with a higher sequence', async (t) => {
  const { session, host } = await fixture(t, 'reconnect');
  await session.initialize();
  const first = await host.next('hello');
  await session.call('arkdock_connect_current_project', declaration);
  const update = await host.next('update');
  host.disconnect();
  await eventually(() => session.call('arkdock_context_status'), (result) => !result.status.connected);
  assert.equal((await session.call('arkdock_context_renew')).response.result.isError, false);
  const reconnected = await host.next('hello');
  assert.equal(reconnected.session, first.session);
  assert.ok(reconnected.sequence > update.sequence);
  assert.equal(reconnected.originalProject, declaration.originalProject);
  assert.equal(reconnected.executionDirectory, declaration.executionDirectory);
});

test('tool inputs reject environment mismatches, controls, relative paths and UTF-8 overflow', async (t) => {
  const { session, host } = await fixture(t, 'validation');
  await session.initialize();
  await host.next('hello');
  const inputs = [
    { ...declaration, executionDirectory: '/home/app' },
    { ...declaration, executionDirectory: 'C:relative' },
    { ...declaration, executionDirectory: 'C:\\bad\npath' },
    { ...declaration, executionDirectory: `C:\\${'界'.repeat(1500)}` },
    { ...declaration, hostSession: '界'.repeat(50) },
    { ...declaration, trusted: true },
    { ...declaration, executionDirectory: '\\\\wsl.localhost\\Ubuntu\\app' },
  ];
  for (const input of inputs) assert.equal((await session.call('arkdock_connect_current_project', input)).response.result.isError, true);
  assert.equal(host.allFrames.filter((frame) => frame.kind === 'update').length, 0);
  const remote = await session.call('arkdock_connect_current_project', { originalProject: '/home/app', executionDirectory: '/home/task', environment: 'remote' });
  assert.equal(remote.response.result.isError, false);
  assert.equal((await host.next('update')).executionDirectory, '/home/task');
  const crossEnvironment = await session.call('arkdock_context_update', { originalProject: 'D:\\host-project', executionDirectory: '/home/wsl-task', environment: 'wsl' });
  assert.equal(crossEnvironment.response.result.isError, false);
  const crossed = await host.next('update');
  assert.equal(crossed.originalProject, 'D:\\host-project');
  assert.equal(crossed.executionDirectory, '/home/wsl-task');
});

test('oversized roots snapshots fail closed within the complete pipe frame budget', async (t) => {
  const { session, host } = await fixture(t, 'roots-size');
  await session.initialize({ roots: {} });
  const request = await session.next('roots/list');
  session.respond(request.id, { roots: Array.from({ length: 8 }, (_, index) => ({ uri: `file:///C:/${index}${'a'.repeat(3000)}` })) });
  assert.deepEqual((await host.next('hello')).roots, []);
  assert.equal((await session.call('arkdock_context_status')).status.rootSource, 'roots-rejected');
});

test('malformed JSON-RPC input and unknown prototype method do not crash the stdio server', async (t) => {
  const { session } = await fixture(t, 'malformed');
  session.child.stdin.write('null\n{bad json\n');
  assert.equal((await session.messages.next((message) => message.error?.code === -32600, 'invalid object')).id, null);
  assert.equal((await session.messages.next((message) => message.error?.code === -32700, 'parse error')).id, null);
  await session.initialize();
  assert.equal((await session.request('toString')).error.code, -32601);
  assert.deepEqual((await session.request('ping')).result, {});
});

test('an unbounded stdio line is terminated before allocating an unbounded JSON request', async (t) => {
  const { session } = await fixture(t, 'stdio-limit');
  session.child.stdin.write('x'.repeat(65537));
  assert.equal(await session.exited, 2);
});

test('roots are protocol file URIs and preserve remote environment identity', () => {
  assert.equal(rootUriToPath('ssh://server/app'), null);
  assert.equal(rootUriToPath('file:///home/app'), '/home/app');
  assert.equal(rootUriToPath('file:///C:/app%20name'), 'C:\\app name');
  assert.throws(() => normalizeRoots([{ uri: 'file:///C:/bad%0Apath' }]));
  assert.throws(() => normalizeRoots(Array(9).fill({ uri: 'file:///C:/app' })));
  assert.throws(() => normalizeRoots([{ uri: 'ssh://server/app' }]));
});

test('pairing data cannot turn a local pipe into network connection options', async (t) => {
  const { directory } = await fixture(t, 'pairing-options');
  const file = join(directory, 'invalid-pairing.json');
  writeFileSync(file, JSON.stringify({ version: 1, credential: 'f'.repeat(64), pipeName: { host: '127.0.0.1', port: 1 } }));
  const result = spawnSync(node, [script, 'serve', '--pairing-file', file], { timeout: 3000 });
  assert.equal(result.status, 2);
  assert.ok(result.stderr.toString().includes('pipeName'));
  assert.ok(!result.stderr.toString().includes('f'.repeat(64)));
});

test('a fake local endpoint never receives credentials or paths without valid server proof', async (t) => {
  const { session, host, credential } = await fixture(t, 'fake-proof', { proof: 'invalid' });
  await session.initialize();
  await eventually(async () => host.allFrames, (frames) => frames.length > 0);
  assert.equal(host.allFrames[0].kind, 'probe');
  await eventually(() => session.call('arkdock_context_status'), (result) => !result.status.connected && Boolean(result.status.lastError));
  assert.deepEqual(Object.keys(host.allFrames[0]).sort(), ['challenge', 'kind']);
  assert.equal(host.allFrames.length, 1);
  assert.ok(!JSON.stringify(host.allFrames).includes(credential));
  assert.ok(!JSON.stringify(host.allFrames).includes('cwd'));
  const rejected = await session.call('arkdock_connect_current_project', declaration);
  assert.equal(rejected.response.result.isError, true);
  assert.equal(rejected.status.authenticated, false);
  assert.equal(host.allFrames.every((frame) => frame.kind === 'probe'), true);
  assert.notEqual(host.allFrames[0].challenge, host.allFrames[1].challenge, 'reconnect must use a fresh challenge');
});

for (const proof of ['wrong-challenge', 'malformed', 'oversized']) {
  test(`server proof ${proof} closes before sending a credential-bearing frame`, async (t) => {
    const { session, host } = await fixture(t, `proof-${proof}`, { proof });
    await session.initialize();
    await host.next('probe');
    const result = await eventually(() => session.call('arkdock_context_status'), (response) => !response.status.connected && Boolean(response.status.lastError));
    assert.equal(result.status.serverVerified, false);
    assert.equal(result.status.authenticated, false);
    assert.equal(result.status.registered, false);
    assert.equal(host.allFrames.length, 1);
    assert.deepEqual(Object.keys(host.allFrames[0]).sort(), ['challenge', 'kind']);
  });
}

test('silent server proof times out while authentication remains false and leaks no context', async (t) => {
  const { session, host } = await fixture(t, 'proof-silent', { proof: 'silent' });
  await session.initialize();
  await host.next('probe');
  const pending = (await session.call('arkdock_context_status')).status;
  assert.equal(pending.connected, true);
  assert.equal(pending.pending, true);
  assert.equal(pending.serverVerified, false);
  assert.equal(pending.authenticated, false);
  await delay(3200);
  const expired = (await session.call('arkdock_context_status')).status;
  assert.equal(expired.lastError, 'CONTEXT_SERVER_PROOF_TIMEOUT');
  assert.equal(expired.registered, false);
  assert.equal(expired.pending, false);
  assert.equal(host.allFrames.length, 1);
});
