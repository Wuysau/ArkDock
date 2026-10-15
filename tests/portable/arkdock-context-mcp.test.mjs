import assert from 'node:assert/strict';
import net from 'node:net';
import { spawn, spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

const script = join(process.cwd(), 'tools', 'agent', 'arkdock-context.mjs');
const node = process.execPath;

function pipeName(name) {
  return `\\\\.\\pipe\\arkdock-context-test-${process.pid}-${name}`;
}

/** Hosts the ArkDock side of the pipe and records every bounded JSON line. */
function hostPipe(name) {
  const frames = [];
  const waiters = [];
  const server = net.createServer((socket) => {
    let buffer = '';
    socket.on('data', (chunk) => {
      buffer += chunk.toString('utf8');
      let index;
      while ((index = buffer.indexOf('\n')) >= 0) {
        const line = buffer.slice(0, index).trim();
        buffer = buffer.slice(index + 1);
        if (!line) continue;
        const frame = JSON.parse(line);
        const waiter = waiters.find((entry) => entry.wants(frame));
        if (waiter) {
          clearTimeout(waiter.timer);
          waiters.splice(waiters.indexOf(waiter), 1);
          waiter.resolve(frame);
        } else {
          frames.push(frame);
        }
      }
    });
  });
  const listening = new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(name, () => resolve(undefined));
  });
  return {
    frames,
    listening,
    nextFrame(label, kind, timeoutMs = 10_000) {
      const queued = frames.find((frame) => frame.kind === kind);
      if (queued) {
        frames.splice(frames.indexOf(queued), 1);
        return Promise.resolve(queued);
      }
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          const index = waiters.findIndex((entry) => entry.reject === reject);
          if (index >= 0) waiters.splice(index, 1);
          reject(new Error(`等待管道帧超时：${label}`));
        }, timeoutMs);
        waiters.push({ wants: (frame) => frame.kind === kind, resolve, reject, timer });
      });
    },
    close() {
      return new Promise((resolve) => server.close(() => resolve(undefined)));
    },
  };
}

/** Drives one `serve` child with typed MCP requests and server-initiated reads. */
function startServer(pairingFile, pipe, extraArgs = []) {
  const child = spawn(node, [script, 'serve', '--pairing-file', pairingFile, '--pipe', pipe, ...extraArgs], {
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  const messages = [];
  const waiters = [];
  const stderrLog = [];
  let stdoutBuffer = '';
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => stderrLog.push(chunk.trim()));
  child.stdout.on('data', (chunk) => {
    stdoutBuffer += chunk;
    let index;
    while ((index = stdoutBuffer.indexOf('\n')) >= 0) {
      const line = stdoutBuffer.slice(0, index).trim();
      stdoutBuffer = stdoutBuffer.slice(index + 1);
      if (!line) continue;
      const message = JSON.parse(line);
      if (message.id === undefined) continue;
      const waiter = waiters.find((entry) => entry.wants(message));
      if (waiter) {
        clearTimeout(waiter.timer);
        waiters.splice(waiters.indexOf(waiter), 1);
        waiter.resolve(message);
      } else {
        messages.push(message);
      }
    }
  });
  const waitForMessage = (label, method, timeoutMs = 10_000) => {
    const queued = messages.find((message) => method === undefined || message.method === method);
    if (queued) {
      messages.splice(messages.indexOf(queued), 1);
      return Promise.resolve(queued);
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        reject(new Error(`等待 MCP 消息超时：${label}；stderr=${stderrLog.join("|").slice(0, 400)}`));
      }, timeoutMs);
      waiters.push({ wants: (message) => method === undefined || message.method === method, resolve, reject, timer });
    });
  };
  let mcpId = 0;
  return {
    child,
    messages,
    stderrLog,
    nextMessage(label, method, timeoutMs = 10_000) {
      return waitForMessage(label, method, timeoutMs);
    },
    request(method, params) {
      const id = ++mcpId;
      child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
      return waitForMessage(`回复:${method}`, undefined).then((message) => {
        if (message.id !== id) throw new Error(`响应 id 不匹配：期望 ${id}，得到 ${message.id}`);
        return message;
      });
    },
    notify(method, params) {
      child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', method, params })}\n`);
    },
    respond(id, result) {
      child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, result })}\n`);
    },
  };
}

function pairingFixture(directory, pipe) {
  const file = join(directory, 'arkdock-context-pairing.json');
  const fixture = { version: 1, pipeName: pipe };
  const pairingValue = 'f'.repeat(64);
  fixture.credential = pairingValue;
  writeFileSync(file, JSON.stringify(fixture));
  return { file, value: pairingValue };
}

test('stdio MCP 握手后向本地管道登记会话并随退出撤销', async () => {
  const pipe = pipeName('basic');
  const host = hostPipe(pipe);
  const directory = mkdtempSync(join(tmpdir(), 'arkdock-context-'));
  let session;
  try {
    await host.listening;
    const pairing = pairingFixture(directory, pipe);
    session = startServer(pairing.file, pipe, ['--agent', '测试工具']);
    const initialize = await session.request('initialize', {
      protocolVersion: '2025-03-26',
      capabilities: {},
      clientInfo: { name: 'portable-test', version: '0.0.0' },
    });
    assert.equal(initialize.result.serverInfo.name, 'arkdock-context');
    session.notify('notifications/initialized');

    const hello = await host.nextFrame('hello frame', 'hello');
    // The pairing FILE schema is owned by desktop/context_pairing.rs; this source
    // assertion fails loudly when the two ends drift apart.
    const rustSchema = readFileSync(join(process.cwd(), 'src-tauri', 'src', 'desktop', 'context_pairing.rs'), 'utf8');
    assert.ok(rustSchema.includes('credential: &credential'), 'Rust 配对文件字段与测试 fixture 漂移');
    assert.equal(hello.kind, 'hello');
    assert.equal(hello.token, pairing.value);
    assert.match(hello.session, /^mcp-[0-9a-f]{32}$/);
    assert.deepEqual(hello.roots, [], '无 roots 能力的宿主只上报弱提示');
    assert.equal(typeof hello.cwd, 'string');

    const tools = await session.request('tools/list', {});
    assert.equal(tools.result.tools[0].name, 'arkdock_context_status');

    const heartbeat = await host.nextFrame('heartbeat frame', 'heartbeat', 12_000);
    assert.equal(heartbeat.kind, 'heartbeat');
    assert.equal(heartbeat.session, hello.session);

    const exited = new Promise((resolve) => session.child.once('exit', resolve));
    session.child.stdin.end();
    const revoke = await host.nextFrame('revoke frame', 'revoke');
    assert.equal(revoke.kind, 'revoke');
    assert.equal(revoke.session, hello.session);
    const code = await exited;
    assert.equal(code, 0);
  } finally {
    session?.child.kill();
    await host.close();
    rmSync(directory, { recursive: true, force: true });
  }
});

test('支持 roots 的宿主收到 roots/list 且登记声明的工程根', async () => {
  const pipe = pipeName('roots');
  const host = hostPipe(pipe);
  const directory = mkdtempSync(join(tmpdir(), 'arkdock-context-'));
  let session;
  try {
    await host.listening;
    const pairing = pairingFixture(directory, pipe);
    session = startServer(pairing.file, pipe);
    const initialize = await session.request('initialize', {
      protocolVersion: '2025-03-26',
      capabilities: { roots: { listChanged: false } },
      clientInfo: { name: 'portable-test', version: '0.0.0' },
    });
    assert.equal(initialize.result.serverInfo.name, 'arkdock-context');
    session.notify('notifications/initialized');
    const rootsRequest = await session.nextMessage('roots/list', 'roots/list');
    assert.equal(rootsRequest.method, 'roots/list');
    session.respond(rootsRequest.id, {
      roots: [
        { uri: 'file:///C:/workspace/demo-app' },
        { uri: 'ssh://host/remote' },
        { uri: 'file:///home/posix/app' },
        { uri: 'file://wsl.localhost/Ubuntu/app' },
      ],
    });
    const hello = await host.nextFrame('hello frame', 'hello');
    assert.equal(hello.kind, 'hello');
    // file URI 归一为 Windows 路径；非 file 方案原样保留环境身份。
    if (process.platform === 'win32') {
      assert.deepEqual(hello.roots, [
        'C:\\workspace\\demo-app',
        'ssh://host/remote',
        '\\\\wsl.localhost\\Ubuntu\\app',
      ]);
    } else {
      assert.ok(hello.roots.length >= 1);
    }
    assert.equal(typeof hello.cwd, 'string');
    const exited = new Promise((resolve) => session.child.once('exit', resolve));
    session.child.stdin.end();
    await host.nextFrame('revoke frame', 'revoke');
    assert.equal(await exited, 0);
  } finally {
    session?.child.kill();
    await host.close();
    rmSync(directory, { recursive: true, force: true });
  }
});

test('配置生成器创建真实路径的示例且不覆盖既有目录', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'arkdock-context-'));
  try {
    const pairingFile = pairingFixture(directory, pipeName('config')).file;
    const out = join(directory, 'config');
    const first = spawnSync(node, [script, '--write-config', out, '--pairing-file', pairingFile]);
    assert.equal(first.status, 0, first.stderr?.toString());
    const readme = readFileSync(join(out, 'README.md'), 'utf8');
    assert.ok(readme.includes('arkdock-context-pairing.json'), 'README 必须指向真实配对文件');
    const claude = JSON.parse(readFileSync(join(out, 'claude-mcp-json.example.json'), 'utf8'));
    assert.equal(claude.mcpServers['arkdock-context'].command, node);
    assert.ok(claude.mcpServers['arkdock-context'].args.includes(pairingFile));
    const toml = readFileSync(join(out, 'codex-config.example.toml'), 'utf8');
    assert.ok(toml.includes('[mcp_servers.arkdock-context]'));
    const second = spawnSync(node, [script, '--write-config', out, '--pairing-file', pairingFile]);
    assert.equal(second.status, 2, '已存在的输出目录必须拒绝覆盖');
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
