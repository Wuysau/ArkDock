#!/usr/bin/env node
// ArkDock context source: a stdio MCP server that tells the local ArkDock desktop app
// which workspace the CURRENT agent session uses, so a docked preview can associate
// and build that project.
//
// Trust summary (nothing here is a credential sender's claim):
// - ArkDock authenticates the connection by the OS pipe client PID plus a pairing
//   secret that this script reads from the pairing FILE (never argv, never a log);
// - workspace roots come from the host via MCP `roots/list` when the host supports
//   it; otherwise this script's spawn-time working directory is reported as a weak
//   hint that ArkDock will never treat as proof of the active conversation;
// - this server exposes exactly one read-only tool (`arkdock_context_status`) and no
//   execution capability of any kind.
//
// Usage:
//   node arkdock-context.mjs serve --pairing-file <path> [--pipe <name>] [--agent <label>]
//   node arkdock-context.mjs --write-config <dir> --pairing-file <path> [--pipe <name>] [--agent <label>]
//
// The MCP stdio framing is newline-delimited JSON-RPC 2.0; stdout carries only MCP
// frames, diagnostics go to stderr.

import net from 'node:net';
import process from 'node:process';
import { createInterface } from 'node:readline';
import { randomBytes } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';

const DEFAULT_PIPE = '\\\\.\\pipe\\arkdock-context';
const HEARTBEAT_MS = 5_000;
const ROOTS_TIMEOUT_MS = 3_000;
const MAX_ROOTS = 8;
const MAX_PATH_LEN = 4096;

function hasControlCharacters(value) {
  return [...value].some((character) => {
    const code = character.codePointAt(0);
    return code < 32 || code === 127;
  });
}

function fail(message) {
  process.stderr.write(`arkdock-context: ${message}\n`);
  process.exit(2);
}

function parseArguments(argv) {
  const options = { mode: 'serve', pipe: DEFAULT_PIPE };
  for (let index = 0; index < argv.length; index++) {
    const option = argv[index];
    if (option === 'serve') options.mode = 'serve';
    else if (option === '--write-config') {
      options.mode = 'write-config';
      options.configDir = argv[++index];
      if (!options.configDir) fail('--write-config 需要一个尚不存在的输出目录。');
    } else if (option === '--pairing-file') {
      options.pairingFile = argv[++index];
      if (!options.pairingFile) fail('--pairing-file 需要配对凭证文件路径。');
    } else if (option === '--pipe') {
      options.pipe = argv[++index];
      if (!options.pipe) fail('--pipe 需要命名管道路径。');
    } else if (option === '--agent') {
      options.agent = argv[++index];
      if (options.agent && (options.agent.length > 64 || hasControlCharacters(options.agent))) {
        fail('--agent 标签必须为 1..64 个不含控制字符的字符。');
      }
    } else fail(`未知参数：${option}`);
  }
  if (!options.pairingFile) fail('缺少 --pairing-file（由 ArkDock 生成的配对凭证文件路径）。');
  return options;
}

// The pairing file holds the secret; it is read once, kept in memory only, and never
// logged, echoed or forwarded anywhere except the local ArkDock pipe frames.
function readPairing(path) {
  let raw;
  try {
    raw = readFileSync(path, 'utf8');
  } catch {
    fail(`无法读取配对文件：${path}`);
  }
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch {
    fail('配对文件不是有效的 JSON。');
  }
  if (parsed?.version !== 1 || typeof parsed.credential !== 'string' || !/^[0-9a-f]{64}$/.test(parsed.credential)) {
    fail('配对文件缺少 version:1 与 64 位十六进制 credential；请重新从 ArkDock 生成。');
  }
  // The file field is `credential` (owned by desktop/context_pairing.rs); the wire
  // frames keep their own `token` field name.
  return {
    pipeName: typeof parsed.pipeName === 'string' && parsed.pipeName ? parsed.pipeName : DEFAULT_PIPE,
    token: parsed.credential,
  };
}
// ---------------------------------------------------------------------------
// MCP stdio plumbing
// ---------------------------------------------------------------------------

class McpStdio {
  constructor(handlers) {
    this.handlers = handlers;
    this.nextId = 1;
    this.pending = new Map();
    this.ready = false;
  }

  start() {
    this.reader = createInterface({ input: process.stdin, terminal: false });
    this.reader.on('line', (line) => this.receive(line));
    process.stdin.on('end', () => this.handlers.onEnd?.());
  }

  send(message) {
    process.stdout.write(`${JSON.stringify(message)}\n`);
  }

  receive(line) {
    if (!line.trim()) return;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      return; // A malformed stdio line is dropped; the MCP client owns that channel.
    }
    if (message.id !== undefined && (message.method !== undefined || message.result !== undefined || message.error !== undefined)) {
      if (message.method !== undefined) {
        this.dispatch(message);
      } else {
        const pending = this.pending.get(message.id);
        if (pending) {
          this.pending.delete(message.id);
          pending(message);
        }
      }
      return;
    }
    if (message.method !== undefined) this.dispatch(message);
  }

  dispatch(message) {
    const reply = (result, error) => {
      if (message.id === undefined) return;
      const response = error === undefined
        ? { jsonrpc: '2.0', id: message.id, result }
        : { jsonrpc: '2.0', id: message.id, error };
      this.send(response);
    };
    const handler = this.handlers[message.method];
    if (handler) {
      try {
        handler(message, reply);
      } catch (error) {
        reply(undefined, { code: -32603, message: String(error?.message ?? error) });
      }
      return;
    }
    if (message.id !== undefined) {
      reply(undefined, { code: -32601, message: `Method not supported: ${message.method}` });
    }
  }

  request(method, params, timeoutMs) {
    const id = this.nextId++;
    const request = { jsonrpc: '2.0', id, method, params };
    return new Promise((resolvePromise) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        resolvePromise(undefined);
      }, timeoutMs);
      this.pending.set(id, (message) => {
        clearTimeout(timer);
        resolvePromise(message);
      });
      this.send(request);
    });
  }
}

// ---------------------------------------------------------------------------
// Pipe session
// ---------------------------------------------------------------------------

class PipeSession {
  constructor(pipePath, pairing, session, contextOf) {
    this.pipePath = pipePath;
    this.token = pairing.token;
    this.session = session;
    this.contextOf = contextOf; // () => ({ roots, cwd, agent })
    this.socket = null;
    this.connected = false;
    this.lastError = null;
    this.buffer = '';
    this.timer = setInterval(() => this.tick(), HEARTBEAT_MS);
    this.timer.unref?.();
  }

  start() {
    this.tick();
  }

  tick() {
    if (this.connected || this.connecting) return;
    this.connecting = true;
    const socket = net.connect(this.pipePath);
    this.socket = socket;
    socket.setNoDelay(true);
    socket.on('connect', () => {
      this.connecting = false;
      const { roots, cwd, agent } = this.contextOf();
      this.write({ kind: 'hello', token: this.token, session: this.session, roots, cwd, agent });
      this.connected = true;
      this.lastError = null;
    });
    socket.on('data', () => {
      // The pipe is inbound-only; ArkDock never replies on it.
    });
    socket.on('error', (error) => {
      this.connecting = false;
      this.connected = false;
      this.lastError = String(error?.code ?? error?.message ?? error);
    });
    socket.on('close', () => {
      this.connecting = false;
      this.connected = false;
    });
  }

  write(frame) {
    if (!this.socket || this.socket.destroyed) return false;
    try {
      this.socket.write(`${JSON.stringify(frame)}\n`);
      return true;
    } catch {
      return false;
    }
  }

  heartbeat() {
    if (this.connected) this.write({ kind: 'heartbeat', token: this.token, session: this.session });
  }

  updateContext() {
    if (!this.connected) return;
    const { roots } = this.contextOf();
    this.write({ kind: 'update', token: this.token, session: this.session, roots });
  }

  revoke() {
    if (this.connected) {
      this.write({ kind: 'revoke', token: this.token, session: this.session });
    }
    clearInterval(this.timer);
    this.socket?.destroy();
    this.connected = false;
  }

  status() {
    return {
      pipe: this.pipePath,
      session: this.session,
      connected: this.connected,
      lastError: this.lastError,
      ...this.contextOf(),
    };
  }
}

// ---------------------------------------------------------------------------
// Roots conversion (environment identity is preserved, never rewritten)
// ---------------------------------------------------------------------------

export function rootUriToPath(uri) {
  if (typeof uri !== 'string' || !uri) return null;
  if (uri.startsWith('file://')) {
    const raw = uri.slice('file://'.length).split('?')[0].split('#')[0];
    const slash = raw.indexOf('/');
    const authority = slash >= 0 ? raw.slice(0, slash) : raw;
    let decoded;
    try {
      decoded = decodeURIComponent(slash >= 0 ? raw.slice(slash) : '');
    } catch {
      return null;
    }
    if (authority === '') {
      // file:///D:/x -> D:\x ; a POSIX root (file:///home/x) keeps an unknown-platform
      // identity that ArkDock does not support, so it is dropped instead of faked.
      if (/^\/[A-Za-z]:/.test(decoded)) {
        return decoded.slice(1).replaceAll('/', '\\');
      }
      return null;
    }
    // file://host/... -> \\host\... (UNC authority, e.g. wsl.localhost).
    return `\\\\${authority}${decoded.replaceAll('/', '\\')}`;
  }
  // Non-file schemes (wsl$, ssh, vscode-remote) are passed through verbatim so the
  // backend can classify and retain their environment identity.
  if (/^[a-z][a-z0-9+.-]*:/i.test(uri)) return uri;
  return null;
}
export function normalizeRoots(uris) {
  const roots = [];
  for (const uri of Array.isArray(uris) ? uris : []) {
    const path = rootUriToPath(uri?.uri ?? uri);
    if (!path || path.length > MAX_PATH_LEN || path.includes('\0')) continue;
    if (!roots.includes(path)) roots.push(path);
    if (roots.length === MAX_ROOTS) break;
  }
  return roots;
}

// ---------------------------------------------------------------------------
// Modes
// ---------------------------------------------------------------------------

function writeConfig(options) {
  const target = resolve(options.configDir);
  const parent = dirname(target);
  if (!parent || target === parent) fail('输出目录无效。');
  if (existsSync(target)) fail(`输出目录已存在，拒绝覆盖：${target}`);
  const pairingAbsolute = resolve(options.pairingFile);
  const node = process.execPath;
  const args = ['serve', '--pairing-file', pairingAbsolute, '--pipe', options.pipe];
  if (options.agent) args.push('--agent', options.agent);
  const claudeBlock = {
    mcpServers: {
      'arkdock-context': { command: node, args, env: {} },
    },
  };
  const codexBlock = [
    '[mcp_servers.arkdock-context]',
    `command = ${JSON.stringify(node)}`,
    `args = ${JSON.stringify(args)}`,
  ].join('\n');
  const generic = [node, ...args].map((part) => (/[\s"]/.test(part) ? JSON.stringify(part) : part)).join(' ');
  const readme = `# ArkDock 上下文源接入（由 arkdock-context.mjs 生成）

生成时间：${new Date().toISOString()}
管道：${options.pipe}
配对凭证：${pairingAbsolute}（令牌只存于该文件，勿提交、勿外发）

## Claude Code（项目 .mcp.json 片段）
\`\`\`json
${JSON.stringify(claudeBlock, null, 2)}
\`\`\`

## Codex CLI（~/.codex/config.toml 片段）
\`\`\`toml
${codexBlock}
\`\`\`

## 通用 MCP 客户端
\`\`\`
${generic}
\`\`\`

## 行为
- 新对话启动本 MCP 服务器时，自动向 ArkDock 登记当前工作区（宿主支持 roots/list 用 roots，否则用启动目录弱提示）。
- 每 5 秒心跳续期；对话退出自动撤销登记。
- ArkDock 侧的自动关联/构建仍遵守其既有信任门禁；本脚本不执行任何工程命令。

## 验收注意
宿主是否支持 roots、是否按会话隔离 MCP 连接因产品而异；不满足时 ArkDock 只会给出
“连接当前工程”的显式确认候选，不会自动关联。
`;
  mkdirSync(target, { recursive: false });
  writeFileSync(join(target, 'README.md'), readme);
  writeFileSync(join(target, 'claude-mcp-json.example.json'), `${JSON.stringify(claudeBlock, null, 2)}\n`);
  writeFileSync(join(target, 'codex-config.example.toml'), `${codexBlock}\n`);
  process.stderr.write(`已生成配置示例：${target}\n`);
}


function serve(options) {
  const pairing = readPairing(options.pairingFile);
  const pipePath = options.pipe === DEFAULT_PIPE ? pairing.pipeName : options.pipe;
  const session = `mcp-${randomBytes(16).toString('hex')}`;
  let declaredRoots = [];
  let rootSource = 'none';

  const pipe = new PipeSession(pipePath, pairing, session, () => ({
    roots: declaredRoots,
    cwd: process.cwd(),
    agent: options.agent ?? null,
  }));

  // Renew the registration on a fixed cadence; the backend expires entries that
  // stop heartbeating, so a crashed MCP server never leaves stale context.
  const heartbeatTimer = setInterval(() => pipe.heartbeat(), HEARTBEAT_MS);
  heartbeatTimer.unref?.();


  const mcp = new McpStdio({
    initialize: (message, reply) => {
      const clientCapabilities = message.params?.capabilities ?? {};
      const canRoots = Boolean(clientCapabilities.roots);
      reply({
        protocolVersion: message.params?.protocolVersion ?? '2025-03-26',
        capabilities: { tools: {} },
        serverInfo: { name: 'arkdock-context', version: '0.1.0' },
      });
      mcp.ready = true;
      if (canRoots) {
        mcp.request('roots/list', {}, ROOTS_TIMEOUT_MS).then((response) => {
          declaredRoots = normalizeRoots(response?.result?.roots);
          rootSource = response ? 'roots' : 'roots-timeout';
          pipe.start();
          if (pipe.connected) pipe.updateContext();
        });
      } else {
        rootSource = 'cwd';
        pipe.start();
      }
    },
    'notifications/initialized': () => {
      if (!mcp.ready) return;
      // Already handled after initialize; keep for clients that never send roots.
    },
    ping: (_message, reply) => reply({}),
    'tools/list': (_message, reply) => {
      reply({
        tools: [{
          name: 'arkdock_context_status',
          description: '只读：查询本会话与 ArkDock 上下文源的连接状态（不执行任何工程命令）。',
          inputSchema: { type: 'object', properties: {} },
        }],
      });
    },
    'tools/call': (message, reply) => {
      if (message.params?.name !== 'arkdock_context_status') {
        reply(undefined, { code: -32602, message: '未知工具' });
        return;
      }
      reply({ content: [{ type: 'text', text: JSON.stringify({ ...pipe.status(), rootSource }) }], isError: false });
    },
    shutdown: (_message, reply) => {
      reply({});
      shutdown(0);
    },
    onEnd: () => shutdown(0),
  });

  function shutdown(code) {
    pipe.revoke();
    try { process.exit(code); } catch { /* exiting */ }
  }
  process.on('SIGINT', () => shutdown(0));
  process.on('SIGTERM', () => shutdown(0));

  mcp.start();
  process.stderr.write(`arkdock-context: 会话 ${session} 已启动（管道 ${pipePath}）。\n`);
}

const options = parseArguments(process.argv.slice(2));
if (options.mode === 'write-config') writeConfig(options);
else serve(options);
