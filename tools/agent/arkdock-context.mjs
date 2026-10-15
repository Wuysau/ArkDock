#!/usr/bin/env node
// stdio MCP adapter for ArkDock's local context API. Declarations are suggestions,
// never proof of the visible conversation, execution permission, or active scope.
import net from 'node:net';
import process from 'node:process';
import { createHmac, randomBytes, timingSafeEqual } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const DEFAULT_PIPE = '\\\\.\\pipe\\arkdock-context';
const HEARTBEAT_MS = 5_000;
const REQUEST_TIMEOUT_MS = 3_000;
const MAX_PIPE_BYTES = 16 * 1024;
const MAX_STDIO_BYTES = 64 * 1024;
const MAX_ROOTS = 8;
const MAX_PATH_BYTES = 4096;
const SUPPORTED_VERSIONS = ['2025-11-25', '2025-06-18', '2025-03-26', '2024-11-05'];
const scriptPath = fileURLToPath(import.meta.url);
const object = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const hasControl = (value) => [...value].some((character) => {
  const code = character.codePointAt(0);
  return code < 32 || code === 127;
});

function boundedString(value, limit, label) {
  if (typeof value !== 'string' || !value.trim() || Buffer.byteLength(value) > limit || hasControl(value)) {
    throw new Error(`${label} 必须为非空、不含控制字符、至多 ${limit} UTF-8 字节的字符串。`);
  }
  return value;
}

function fail(message) {
  process.stderr.write(`arkdock-context: ${message}\n`);
  process.exit(2);
}

function parseArguments(argv) {
  const options = { mode: 'serve' };
  for (let index = 0; index < argv.length; index++) {
    const option = argv[index];
    if (option === 'serve') options.mode = 'serve';
    else if (['--write-config', '--pairing-file', '--pipe', '--agent'].includes(option)) {
      const value = argv[++index];
      if (!value || value.startsWith('--')) fail(`${option} 缺少参数。`);
      if (option === '--write-config') { options.mode = 'write-config'; options.configDir = value; }
      if (option === '--pairing-file') options.pairingFile = value;
      if (option === '--pipe') options.pipe = value;
      if (option === '--agent') options.agent = boundedString(value, 64, '--agent');
    } else fail(`未知参数：${option}`);
  }
  if (!options.pairingFile) fail('缺少 --pairing-file。');
  return options;
}

function readPairing(path) {
  let parsed;
  try { parsed = JSON.parse(readFileSync(path, 'utf8')); }
  catch { throw new Error('无法读取有效配对文件；请从 ArkDock 重新生成。'); }
  if (parsed?.version !== 1 || typeof parsed.credential !== 'string' || !/^[0-9a-f]{64}$/.test(parsed.credential)) {
    throw new Error('配对文件需要 version:1 与 64 位十六进制 credential。');
  }
  if (parsed.pipeName !== undefined && (typeof parsed.pipeName !== 'string' || !parsed.pipeName || hasControl(parsed.pipeName))) {
    throw new Error('配对文件 pipeName 必须为本机管道路径字符串。');
  }
  return { token: parsed.credential, pipeName: parsed.pipeName || DEFAULT_PIPE };
}

// Buffer complete lines, with a byte limit before decoding or JSON allocation.
function readLines(stream, limit, onLine, onOverflow) {
  let buffer = Buffer.alloc(0);
  let stopped = false;
  stream.on('data', (chunk) => {
    if (stopped) return;
    const data = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    let offset = 0;
    while (offset < data.length) {
      const newline = data.indexOf(10, offset);
      const end = newline < 0 ? data.length : newline;
      if (buffer.length + end - offset > limit) {
        stopped = true;
        buffer = Buffer.alloc(0);
        onOverflow();
        return;
      }
      buffer = Buffer.concat([buffer, data.subarray(offset, end)]);
      if (newline < 0) return;
      const line = buffer.toString('utf8');
      buffer = Buffer.alloc(0);
      onLine(line);
      offset = newline + 1;
    }
  });
}

class McpStdio {
  constructor(handlers) {
    this.handlers = handlers;
    this.nextId = 0;
    this.pending = new Map();
    this.state = 'new';
    this.inflight = 0;
  }
  start() {
    readLines(process.stdin, MAX_STDIO_BYTES, (line) => this.receive(line), () => this.handlers.onEnd(2));
    process.stdin.on('end', () => this.handlers.onEnd(0));
    process.stdout.on('error', () => this.handlers.onEnd(0));
  }
  send(message) { process.stdout.write(`${JSON.stringify(message)}\n`); }
  receive(line) {
    if (!line.trim()) return;
    let message;
    try { message = JSON.parse(line); }
    catch { this.send({ jsonrpc: '2.0', id: null, error: { code: -32700, message: 'Parse error' } }); return; }
    if (!object(message) || message.jsonrpc !== '2.0') {
      this.send({ jsonrpc: '2.0', id: null, error: { code: -32600, message: 'Invalid Request' } });
      return;
    }
    const hasId = Object.hasOwn(message, 'id');
    if (message.method === undefined) {
      const pending = this.pending.get(message.id);
      if (pending && (Object.hasOwn(message, 'result') !== Object.hasOwn(message, 'error'))) {
        this.pending.delete(message.id);
        pending(message);
      }
      return;
    }
    if (typeof message.method !== 'string' || (hasId && typeof message.id !== 'string' && !Number.isSafeInteger(message.id))) {
      this.send({ jsonrpc: '2.0', id: null, error: { code: -32600, message: 'Invalid Request' } });
      return;
    }
    const reply = (result, error) => {
      if (hasId) this.send({ jsonrpc: '2.0', id: message.id, ...(error ? { error } : { result }) });
    };
    if (this.state !== 'ready' && !['initialize', 'notifications/initialized', 'ping'].includes(message.method)) {
      reply(undefined, { code: -32000, message: 'Initialization is not complete' });
      return;
    }
    const handler = Object.hasOwn(this.handlers, message.method) ? this.handlers[message.method] : undefined;
    if (!handler) { reply(undefined, { code: -32601, message: 'Method not supported' }); return; }
    if (this.inflight >= 32) { reply(undefined, { code: -32000, message: 'Too many pending requests' }); return; }
    this.inflight++;
    // Apply synchronous lifecycle transitions before reading the next stdio line.
    // Clients may send initialized and tools/list in the same stream chunk.
    try {
      Promise.resolve(handler(message, reply)).catch(() => {
        reply(undefined, { code: -32603, message: 'Internal error' });
      }).finally(() => { this.inflight--; });
    } catch {
      this.inflight--;
      reply(undefined, { code: -32603, message: 'Internal error' });
    }
  }
  request(method, params) {
    const id = `arkdock-${++this.nextId}`;
    return new Promise((resolvePromise) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        this.send({ jsonrpc: '2.0', method: 'notifications/cancelled', params: { requestId: id, reason: 'timeout' } });
        resolvePromise(undefined);
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(id, (message) => { clearTimeout(timer); resolvePromise(message); });
      this.send({ jsonrpc: '2.0', id, method, params });
    });
  }
}

class PipeSession {
  constructor(pipePath, token, contextOf, beforeConnect) {
    this.pipePath = pipePath;
    this.token = token;
    this.contextOf = contextOf;
    this.beforeConnect = beforeConnect;
    this.session = `mcp-${randomBytes(16).toString('hex')}`;
    this.sequence = 0;
    this.enabled = false;
    this.connected = false;
    this.authenticated = false;
    this.serverVerified = false;
    this.pendingProof = null;
    this.lastError = null;
    this.acceptedContext = null;
    this.pending = new Map();
  }
  async connect() {
    if (this.authenticated) return;
    this.serverVerified = false;
    await this.beforeConnect();
    const socket = net.connect(this.pipePath);
    this.socket = socket;
    socket.setNoDelay(true);
    const disconnect = (error) => {
      if (this.socket !== socket) return;
      this.connected = false;
      this.authenticated = false;
      this.serverVerified = false;
      if (error) this.lastError = error;
      this.pendingProof?.reject(new Error(this.lastError || 'CONTEXT_DISCONNECTED'));
      this.pendingProof = null;
      for (const waiter of this.pending.values()) waiter.reject(new Error(this.lastError || 'CONTEXT_DISCONNECTED'));
      this.pending.clear();
    };
    socket.on('error', (error) => disconnect(String(error.code || 'CONTEXT_PIPE_ERROR')));
    socket.on('close', () => disconnect());
    readLines(socket, MAX_PIPE_BYTES, (line) => {
      let ack;
      try { ack = JSON.parse(line); } catch { socket.destroy(new Error('CONTEXT_ACK_INVALID')); return; }
      if (this.pendingProof) {
        const pending = this.pendingProof;
        this.pendingProof = null;
        const valid = object(ack) && Object.keys(ack).length === 3 && ack.kind === 'proof'
          && ack.challenge === pending.challenge && typeof ack.proof === 'string'
          && /^[0-9a-f]{64}$/.test(ack.proof)
          && timingSafeEqual(Buffer.from(ack.proof, 'hex'), pending.expected);
        if (valid) {
          this.serverVerified = true;
          pending.resolve();
        } else {
          this.lastError = 'CONTEXT_SERVER_PROOF_INVALID';
          pending.reject(new Error(this.lastError));
          socket.destroy();
        }
        return;
      }
      const waiter = this.pending.get(ack?.sequence);
      if (!object(ack) || ack.kind !== 'ack' || ack.session !== this.session || !waiter || typeof ack.ok !== 'boolean') {
        socket.destroy(new Error('CONTEXT_ACK_INVALID')); return;
      }
      this.pending.delete(ack.sequence);
      if (ack.ok) waiter.resolve(ack);
      else {
        this.lastError = typeof ack.error === 'string' && /^CONTEXT_[A-Za-z0-9_]+$/.test(ack.error) ? ack.error : 'CONTEXT_REJECTED';
        this.authenticated = false;
        waiter.reject(new Error(this.lastError));
        socket.destroy();
      }
    }, () => socket.destroy(new Error('CONTEXT_ACK_LIMIT')));
    await new Promise((resolvePromise, reject) => {
      const timer = setTimeout(() => socket.destroy(new Error('CONTEXT_CONNECT_TIMEOUT')), REQUEST_TIMEOUT_MS);
      socket.once('connect', () => { clearTimeout(timer); this.connected = true; resolvePromise(); });
      socket.once('error', (error) => { clearTimeout(timer); reject(error); });
      socket.once('close', () => { clearTimeout(timer); reject(new Error('CONTEXT_DISCONNECTED')); });
    });
    // A process that merely occupies the fixed pipe name must not receive the token
    // or project metadata. Prove possession of the pairing key before sending hello.
    await new Promise((resolvePromise, reject) => {
      const challenge = randomBytes(32).toString('hex');
      const expected = createHmac('sha256', Buffer.from(this.token, 'hex'))
        .update(`arkdock-context-server-v1\0${challenge}`).digest();
      const timer = setTimeout(() => {
        this.pendingProof = null;
        this.lastError = 'CONTEXT_SERVER_PROOF_TIMEOUT';
        reject(new Error(this.lastError));
        socket.destroy();
      }, REQUEST_TIMEOUT_MS);
      this.pendingProof = {
        challenge, expected,
        resolve: () => { clearTimeout(timer); resolvePromise(); },
        reject: (error) => { clearTimeout(timer); reject(error); },
      };
      socket.write(`${JSON.stringify({ kind: 'probe', challenge })}\n`);
    });
    await this.send('hello', this.contextOf());
    this.authenticated = true;
    this.lastError = null;
  }
  send(kind, context = {}) {
    const sequence = ++this.sequence;
    const frame = { kind, token: this.token, session: this.session, sequence, ...context };
    const encoded = JSON.stringify(frame);
    if (Buffer.byteLength(encoded) > MAX_PIPE_BYTES) return Promise.reject(new Error('CONTEXT_FRAME_LIMIT'));
    if (!this.connected || !this.serverVerified || !this.socket || this.socket.destroyed) return Promise.reject(new Error('CONTEXT_DISCONNECTED'));
    return new Promise((resolvePromise, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(sequence);
        this.lastError = 'CONTEXT_ACK_TIMEOUT';
        reject(new Error(this.lastError));
        this.socket.destroy();
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(sequence, {
        resolve: (ack) => {
          clearTimeout(timer);
          if (kind === 'hello' || kind === 'update') this.acceptedContext = context;
          if (kind === 'revoke') this.acceptedContext = null;
          resolvePromise(ack);
        },
        reject: (error) => { clearTimeout(timer); reject(error); },
      });
      this.socket.write(`${encoded}\n`);
    });
  }
  async sync() {
    this.enabled = true;
    if (this.authenticated) await this.send('update', this.contextOf());
    else await this.connect();
  }
  async renew() {
    if (!this.enabled) throw new Error('CONTEXT_NOT_REGISTERED');
    if (this.authenticated) await this.send('heartbeat');
    else await this.connect();
  }
  async revoke() {
    this.enabled = false;
    try { if (this.authenticated) await this.send('revoke'); }
    finally { this.authenticated = false; this.serverVerified = false; this.connected = false; this.socket?.end(); this.socket?.destroy(); }
  }
  status() {
    return {
      pipe: this.pipePath, session: this.session, sequence: this.sequence,
      connected: this.connected, authenticated: this.authenticated,
      serverVerified: this.serverVerified,
      registered: this.authenticated && this.enabled, pending: this.pending.size > 0 || this.pendingProof !== null,
      enabled: this.enabled, lastError: this.lastError, ...this.contextOf(),
      acceptedContext: this.authenticated ? this.acceptedContext : null,
      requiresConfirmation: true, visibleConversationVerified: false,
    };
  }
}

export function rootUriToPath(uri) {
  if (typeof uri !== 'string' || !uri.startsWith('file://')) return null;
  let url;
  try { url = new URL(uri); } catch { return null; }
  if (url.search || url.hash || url.username || url.password || url.port) return null;
  let path;
  try { path = decodeURIComponent(url.pathname); } catch { return null; }
  if (url.hostname) return `\\\\${url.hostname}${path.replaceAll('/', '\\')}`;
  if (/^\/[A-Za-z]:\//u.test(path)) return path.slice(1).replaceAll('/', '\\');
  // Preserve POSIX identity. The desktop treats it as remote/unsupported, never C:\.
  return path.startsWith('/') ? path : null;
}

export function normalizeRoots(uris) {
  if (!Array.isArray(uris) || uris.length > MAX_ROOTS) throw new Error('CONTEXT_ROOTS_LIMIT');
  const roots = [];
  for (const entry of uris) {
    const path = rootUriToPath(entry?.uri);
    if (!path) throw new Error('CONTEXT_ROOT_URI_INVALID');
    boundedString(path, MAX_PATH_BYTES, 'root');
    if (!roots.includes(path)) roots.push(path);
  }
  return roots;
}

function validateDeclaration(args) {
  if (!object(args) || Object.keys(args).some((key) => !['originalProject', 'executionDirectory', 'environment', 'hostSession'].includes(key))) {
    throw new Error('参数只接受 originalProject、executionDirectory、environment 和可选 hostSession。');
  }
  if (!['windowsLocal', 'wsl', 'remote'].includes(args.environment)) throw new Error('environment 需要 windowsLocal、wsl 或 remote。');
  const context = { environment: args.environment };
  for (const key of ['originalProject', 'executionDirectory']) {
    const path = boundedString(args[key], MAX_PATH_BYTES, key);
    const windows = /^[A-Za-z]:[\\/]/u.test(path) || /^\\\\[^\\]+\\[^\\]+/u.test(path);
    const wsl = /^\\\\wsl(?:\$|\.localhost)\\/iu.test(path) || /^(?:wsl:\/\/|vscode-remote:\/\/wsl\+)/iu.test(path);
    const remote = /^(?:ssh|vscode-remote):\/\//iu.test(path) && !wsl;
    const posix = path.startsWith('/') && !path.startsWith('//');
    if (!(windows || wsl || remote || posix)) throw new Error(`${key} 必须为保留环境身份的绝对路径或远程 URI。`);
    if (key === 'executionDirectory' && ((args.environment === 'windowsLocal' && (!windows || wsl || remote)) ||
        (args.environment === 'wsl' && !(wsl || posix)) ||
        (args.environment === 'remote' && !(remote || posix)))) throw new Error(`${key} 与 environment 不一致。`);
    context[key] = path;
  }
  if (args.hostSession !== undefined) context.hostSession = boundedString(args.hostSession, 128, 'hostSession');
  return context;
}

const declarationSchema = {
  type: 'object', additionalProperties: false,
  properties: {
    originalProject: { type: 'string', description: '原始工程绝对路径，不能从 MCP 进程目录猜测。' },
    executionDirectory: { type: 'string', description: '本轮实际编辑/执行的绝对目录或 worktree，可与原始工程不同。' },
    environment: { type: 'string', enum: ['windowsLocal', 'wsl', 'remote'] },
    hostSession: { type: 'string', description: '可选的真实宿主会话标识，仅显示；未知时省略，禁止编造。' },
  }, required: ['originalProject', 'executionDirectory', 'environment'],
};
const emptySchema = { type: 'object', properties: {}, additionalProperties: false };
const TOOLS = [
  { name: 'arkdock_connect_current_project', description: '连接当前工程：显式登记原始工程和实际工作目录。在 ArkDock 中确认候选后关联；不会验证可见对话或授权构建。', inputSchema: declarationSchema },
  { name: 'arkdock_context_update', description: '完整替换本连接的工程声明（切换 worktree/工程时调用）；省略 hostSession 将清除旧值。仍需 ArkDock 确认。', inputSchema: declarationSchema },
  { name: 'arkdock_context_renew', description: '续期已登记的上下文；不会证明当前可见对话。', inputSchema: emptySchema },
  { name: 'arkdock_context_revoke', description: '断开本连接的工程登记并停止自动重连，直至再次连接当前工程。', inputSchema: emptySchema },
  { name: 'arkdock_context_status', description: '只读查询传输、后端认证回执、工程声明与建议候选状态。', inputSchema: emptySchema },
];

function writeConfig(options) {
  const target = resolve(options.configDir);
  if (target === dirname(target) || existsSync(target)) throw new Error('输出目录无效或已存在，拒绝覆盖。');
  const pairingAbsolute = resolve(options.pairingFile);
  const args = [scriptPath, 'serve', '--pairing-file', pairingAbsolute];
  if (options.pipe) args.push('--pipe', options.pipe);
  if (options.agent) args.push('--agent', options.agent);
  const server = { command: process.execPath, args };
  const claude = { mcpServers: { 'arkdock-context': server } };
  const toml = `[mcp_servers.arkdock-context]\ncommand = ${JSON.stringify(server.command)}\nargs = ${JSON.stringify(args)}\n`;
  const powershell = [server.command, ...args].map((part) => `'${part.replaceAll("'", "''")}'`).join(' ');
  mkdirSync(target);
  writeFileSync(join(target, 'claude-mcp-json.example.json'), `${JSON.stringify(claude, null, 2)}\n`);
  writeFileSync(join(target, 'codex-config.example.toml'), toml);
  writeFileSync(join(target, 'README.md'), `# ArkDock MCP 配置示例

脚本：${scriptPath}
凭证文件：${pairingAbsolute}（只引用本机文件；不要提交或外发文件内容）

将 claude-mcp-json.example.json 或 codex-config.example.toml 中的服务器配置合并到宿主支持的配置位置。
通用客户端使用相同 command/args；PowerShell 手动启动命令：

\`\`\`powershell
& ${powershell}
\`\`\`

重启宿主 MCP 连接后，请 Agent 调用 arkdock_connect_current_project，分别提供原始工程、实际执行/worktree 目录及环境。
在 ArkDock 工程候选中检查并确认实际目录。状态工具的 authenticated/registered 仅在收到后端回执后为 true。
更新目录用 arkdock_context_update；停止登记用 arkdock_context_revoke；手动续期用 arkdock_context_renew。
roots 和启动目录仅是向后兼容提示；普通 MCP 无法证明哪个对话正在窗口中显示。
每 5 秒续期；EOF/断连撤销，后端 TTL 为 15 秒。构建仍需要既有工程信任。
本生成器未安装任何配置，未修改全局设置，未启动真实宿主或桌面验收。
`);
  process.stderr.write(`已生成配置示例：${target}\n`);
}

function serve(options) {
  const pairing = readPairing(options.pairingFile);
  const pipePath = options.pipe || pairing.pipeName;
  if (process.platform === 'win32' && !pipePath.startsWith('\\\\.\\pipe\\')) throw new Error('仅允许本机 Windows 命名管道。');
  const spawnDirectory = process.cwd();
  const cwdHint = Buffer.byteLength(spawnDirectory) <= MAX_PATH_BYTES && !hasControl(spawnDirectory) ? spawnDirectory : undefined;
  let roots = [];
  let rootSource = 'none';
  let rootsCapable = false;
  let rootsGeneration = 0;
  let explicit;
  let stopping = false;
  let queue = Promise.resolve();
  let queued = 0;
  const enqueue = (operation) => {
    if (queued >= 32) return Promise.reject(new Error('CONTEXT_BUSY'));
    queued++;
    const result = queue.then(operation);
    queue = result.catch(() => {}).finally(() => { queued--; });
    return result;
  };
  const contextOf = () => ({ roots, ...(cwdHint ? { cwd: cwdHint } : {}), ...(options.agent ? { agent: options.agent } : {}), ...explicit });
  const pipe = new PipeSession(pipePath, pairing.token, contextOf, refreshRoots);
  const status = () => ({ ...pipe.status(), rootSource, declarationSource: explicit ? 'explicit-tool' : 'compatibility-hint' });
  const report = (reply, error) => reply({ content: [{ type: 'text', text: JSON.stringify({ ...status(), ...(error ? { error: error.message } : {}) }) }], isError: Boolean(error) });

  async function refreshRoots() {
    if (!rootsCapable || mcp.state !== 'ready') return;
    const generation = ++rootsGeneration;
    const response = await mcp.request('roots/list', {});
    if (generation !== rootsGeneration) return;
    try {
      if (!response || response.error || !object(response.result)) throw new Error('roots unavailable');
      const next = normalizeRoots(response.result.roots);
      // Reserve room for the complete explicit context and credentials on the wire.
      const bytes = Buffer.byteLength(JSON.stringify({ ...contextOf(), roots: next }));
      if (bytes > MAX_PIPE_BYTES - 512) throw new Error('roots frame limit');
      roots = next;
      rootSource = 'roots';
    } catch {
      roots = [];
      rootSource = response ? 'roots-rejected' : 'roots-timeout';
    }
  }

  const mcp = new McpStdio({
    initialize: (message, reply) => {
      if (mcp.state !== 'new' || message.id === undefined || !object(message.params) || typeof message.params.protocolVersion !== 'string' || !object(message.params.capabilities)) {
        reply(undefined, { code: -32602, message: 'Invalid or repeated initialization' }); return;
      }
      rootsCapable = object(message.params.capabilities.roots);
      rootSource = rootsCapable ? 'pending' : 'cwd-hint';
      mcp.state = 'initializing';
      reply({ protocolVersion: SUPPORTED_VERSIONS.includes(message.params.protocolVersion) ? message.params.protocolVersion : SUPPORTED_VERSIONS[0], capabilities: { tools: {} }, serverInfo: { name: 'arkdock-context', version: '0.2.0' }, instructions: '需要连接工程时调用 arkdock_connect_current_project，并明确原始工程及实际执行目录。所有声明需在 ArkDock 中确认，不能证明可见对话。' });
    },
    'notifications/initialized': (message) => {
      if (message.id !== undefined || mcp.state !== 'initializing') return;
      mcp.state = 'ready';
      enqueue(() => pipe.sync()).catch((error) => { pipe.lastError = error.code || error.message; });
    },
    'notifications/roots/list_changed': () => {
      if (!rootsCapable || !pipe.enabled) return;
      // Invalidate cached roots before the next async read; old replies cannot revive them.
      rootsGeneration++;
      roots = [];
      rootSource = 'pending';
      enqueue(async () => {
        if (!pipe.enabled) return;
        if (pipe.authenticated) await pipe.send('update', contextOf());
        await refreshRoots();
        if (pipe.enabled) await pipe.sync();
      }).catch((error) => { pipe.lastError = error.code || error.message; });
    },
    ping: (_message, reply) => reply({}),
    'tools/list': (_message, reply) => reply({ tools: TOOLS }),
    'tools/call': async (message, reply) => {
      const name = message.params?.name;
      if (!TOOLS.some((tool) => tool.name === name)) { reply(undefined, { code: -32602, message: 'Unknown tool' }); return; }
      const args = message.params.arguments ?? {};
      if (name === 'arkdock_context_status') {
        if (!object(args) || Object.keys(args).length) { report(reply, new Error('状态工具不接受参数。')); return; }
        report(reply); return;
      }
      try {
        const declaration = ['arkdock_connect_current_project', 'arkdock_context_update'].includes(name) ? validateDeclaration(args) : undefined;
        if (!declaration && (!object(args) || Object.keys(args).length)) throw new Error('该工具不接受参数。');
        await enqueue(async () => {
          if (declaration) {
            if (name === 'arkdock_context_update' && !explicit) throw new Error('请先调用 arkdock_connect_current_project。');
            if (Buffer.byteLength(JSON.stringify({ ...contextOf(), ...declaration })) > MAX_PIPE_BYTES - 512) throw new Error('CONTEXT_FRAME_LIMIT');
            explicit = declaration;
            await pipe.sync();
          } else if (name === 'arkdock_context_renew') await pipe.renew();
          else { explicit = undefined; roots = []; rootsGeneration++; rootSource = 'revoked'; await pipe.revoke(); }
        });
        report(reply);
      } catch (error) { report(reply, error); }
    },
    onEnd: shutdown,
  });

  const heartbeat = setInterval(() => {
    if (mcp.state !== 'ready' || !pipe.enabled || queued || stopping) return;
    enqueue(async () => {
      if (rootsCapable && pipe.authenticated) { await refreshRoots(); await pipe.sync(); }
      else await pipe.renew();
    }).catch((error) => { pipe.lastError = error.code || error.message; });
  }, HEARTBEAT_MS);
  heartbeat.unref();

  async function shutdown(code = 0) {
    if (stopping) return;
    stopping = true;
    clearInterval(heartbeat);
    pipe.enabled = false;
    const timer = setTimeout(() => process.exit(code), REQUEST_TIMEOUT_MS + 100);
    try { await enqueue(() => pipe.revoke()); } catch { /* disconnect also revokes */ }
    clearTimeout(timer);
    process.exit(code);
  }
  process.on('SIGINT', () => shutdown());
  process.on('SIGTERM', () => shutdown());
  mcp.start();
  process.stderr.write('arkdock-context: stdio MCP 已启动；等待初始化。\n');
}

if (process.argv[1] && resolve(process.argv[1]) === scriptPath) {
  try {
    const options = parseArguments(process.argv.slice(2));
    if (options.mode === 'write-config') writeConfig(options);
    else serve(options);
  } catch (error) { fail(error.message); }
}
