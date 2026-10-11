import { spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { randomUUID } from 'node:crypto';
import { realpath, lstat } from 'node:fs/promises';
import { isAbsolute, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { BridgeClient, readPairingFile, requireValue, safeReason, normalizeLocalWorkspaceRoot } from './bridge-client.mjs';
import { SelectionProvider } from '../selection-provider.mjs';

const MAX_LINE = 1024 * 1024;
const MAX_PROMPT = 64 * 1024;
const ID = /^[a-zA-Z0-9_.:-]{1,256}$/;
const samePath = (a, b) => process.platform === 'win32'
  ? a.toLowerCase() === b.toLowerCase() : a === b;

// One owned stdio connection. Never discovers or attaches to another host's daemon.
export class AppServerRpc extends EventEmitter {
  constructor(child, { timeoutMs = 5000 } = {}) {
    super();
    this.child = child; this.timeoutMs = timeoutMs; this.pending = new Map();
    this.nextId = 1; this.buffer = ''; this.closed = false;
    child.stdout.setEncoding('utf8');
    child.stdout.on('data', (chunk) => this.consume(chunk));
    child.stdout.on('error', () => this.disconnect('APP_SERVER_DISCONNECTED'));
    child.stdin.on('error', () => this.disconnect('APP_SERVER_DISCONNECTED'));
    // Do not forward host stderr: it can contain private paths, prompts or provider errors.
    child.stderr?.resume();
    child.on('error', () => this.disconnect('APP_SERVER_START_FAILED'));
    child.on('exit', () => this.disconnect('APP_SERVER_DISCONNECTED'));
  }

  consume(chunk) {
    if (this.closed) return;
    this.buffer += chunk;
    while (true) {
      const end = this.buffer.indexOf('\n');
      if (end < 0) break;
      const line = this.buffer.slice(0, end); this.buffer = this.buffer.slice(end + 1);
      if (Buffer.byteLength(line) > MAX_LINE) { this.disconnect('APP_SERVER_FRAME_LIMIT'); return; }
      let message;
      try { message = JSON.parse(line); }
      catch { this.disconnect('APP_SERVER_PROTOCOL_INVALID'); return; }
      if (!message || typeof message !== 'object' || Array.isArray(message)) {
        this.disconnect('APP_SERVER_PROTOCOL_INVALID'); return;
      }
      if (typeof message.method === 'string') {
        if (message.id !== undefined) {
          // This broker has no approval UI. A request must never become implicit consent.
          if (['item/commandExecution/requestApproval', 'item/fileChange/requestApproval'].includes(message.method)) {
            this.write({ id: message.id, result: { decision: 'decline' } });
          } else this.write({ id: message.id, error: { code: -32601, message: 'ArkDock host interaction unavailable' } });
          this.emit('hostRequest', { method: message.method });
        } else this.emit('notification', message);
      } else if (this.pending.has(message.id)) {
        const pending = this.pending.get(message.id); this.pending.delete(message.id);
        clearTimeout(pending.timer);
        if (message.error) {
          const error = new Error('APP_SERVER_REQUEST_REJECTED');
          error.rpcCode = message.error.code;
          pending.reject(error);
        } else if ('result' in message) pending.resolve(message.result);
        else pending.reject(new Error('APP_SERVER_PROTOCOL_INVALID'));
      }
    }
    if (Buffer.byteLength(this.buffer) > MAX_LINE) this.disconnect('APP_SERVER_FRAME_LIMIT');
  }

  write(message) {
    requireValue(!this.closed && !this.child.stdin.destroyed, 'APP_SERVER_DISCONNECTED');
    const serialized = `${JSON.stringify(message)}\n`;
    requireValue(Buffer.byteLength(serialized) <= MAX_LINE, 'APP_SERVER_REQUEST_LIMIT');
    requireValue(this.child.stdin.writableLength <= MAX_LINE, 'APP_SERVER_BACKPRESSURE');
    this.child.stdin.write(serialized);
  }

  request(method, params) {
    requireValue(!this.closed && this.pending.size < 16, 'APP_SERVER_UNAVAILABLE');
    const id = this.nextId++;
    return new Promise((resolveRequest, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error('APP_SERVER_RESPONSE_TIMEOUT'));
      }, this.timeoutMs);
      this.pending.set(id, { resolve: resolveRequest, reject, timer });
      try { this.write({ id, method, params }); }
      catch (error) { clearTimeout(timer); this.pending.delete(id); reject(error); }
    });
  }

  disconnect(reason) {
    if (this.closed) return;
    this.closed = true;
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer); pending.reject(new Error(reason));
    }
    this.pending.clear(); this.buffer = '';
    this.emit('disconnect', reason);
  }

  dispose() {
    if (this.disposing) return this.disposing;
    this.disposing = (async () => {
      this.disconnect('APP_SERVER_DISPOSED');
      this.child.stdin.end();
      if (this.child.exitCode !== null || this.child.signalCode !== null) return;
      let exited;
      const exit = new Promise((done) => { exited = done; this.child.once('exit', done); });
      try {
        // Let App Server cancel its own tasks and close resources on stdin EOF.
        await Promise.race([exit, delay(2000, undefined, { ref: false })]);
        if (this.child.exitCode === null && this.child.signalCode === null && !this.child.killed) {
          this.child.kill(); // Exact self-created child only. Never kill by name or PID search.
          await Promise.race([exit, delay(500, undefined, { ref: false })]);
        }
      } finally { this.child.removeListener('exit', exited); }
    })();
    return this.disposing;
  }
}

export async function startOwnedServer(executable, workspaceRoot, options = {}) {
  requireValue(process.platform === 'win32', 'WINDOWS_ONLY');
  requireValue(typeof executable === 'string' && isAbsolute(executable) && /\.exe$/i.test(executable), 'CODEX_EXECUTABLE_REQUIRED');
  const canonical = await realpath(executable);
  requireValue((await lstat(canonical)).isFile(), 'CODEX_EXECUTABLE_REQUIRED');
  const child = spawn(canonical, ['app-server', '--listen', 'stdio://'], {
    cwd: workspaceRoot, windowsHide: true, shell: false, stdio: ['pipe', 'pipe', 'pipe'],
  });
  return new AppServerRpc(child, options);
}

export class ManagedCodexTransport {
  constructor({ rpc, bridge, workspaceRoot, workspaceId, writable = false, model, now = Date.now }) {
    workspaceRoot = normalizeLocalWorkspaceRoot(workspaceRoot);
    requireValue(typeof workspaceId === 'string' && workspaceId.length > 0, 'WORKSPACE_INVALID');
    this.rpc = rpc; this.bridge = bridge; this.workspaceRoot = workspaceRoot; this.workspaceId = workspaceId;
    this.writable = writable; this.model = model; this.now = now;
    this.instanceId = `codex-${randomUUID()}`; this.thread = null; this.activeTurnId = null;
    this.deliveries = new Map(); this.events = []; this.receipts = Promise.resolve(); this.stopped = false;
    this.receiptFailure = null; this.submitting = false;
    rpc.on('notification', (event) => this.onNotification(event));
    rpc.on('disconnect', () => {
      this.stopped = true;
      for (const record of this.deliveries.values()) {
        if (!record.terminal) this.recordReceipt(record, 'UNKNOWN', 'APP_SERVER_DISCONNECTED');
      }
    });
  }

  scope() {
    return { instanceId: this.instanceId, sessionId: this.thread.sessionId,
      conversationId: this.thread.id, workspaceRoot: this.workspaceRoot };
  }

  async initialize() {
    requireValue(!this.thread, 'ALREADY_INITIALIZED');
    this.workspaceRoot = await realpath(this.workspaceRoot);
    const host = await this.rpc.request('initialize', { clientInfo: {
      name: 'arkdock_managed', title: 'ArkDock managed Codex session', version: '1.0.0',
    } });
    requireValue(host.platformOs === 'windows', 'HOST_PLATFORM_UNSUPPORTED');
    this.rpc.write({ method: 'initialized', params: {} });
    const result = await this.rpc.request('thread/start', {
      cwd: this.workspaceRoot, approvalPolicy: 'never', sandbox: this.writable ? 'workspace-write' : 'read-only',
      serviceName: 'arkdock_managed', ...(this.model ? { model: this.model } : {}),
    });
    requireValue(result.thread && ID.test(result.thread.id ?? '')
      && ID.test(result.thread.sessionId ?? '') && samePath(await realpath(result.thread.cwd), this.workspaceRoot), 'THREAD_SCOPE_MISMATCH');
    this.thread = { id: result.thread.id, sessionId: result.thread.sessionId };
    this.agentVersion = result.thread.cliVersion;
    requireValue(this.agentVersion === '0.159.2', 'CODEX_VERSION_UNVERIFIED');
    await this.verifySession();
    const registration = await this.bridge.register({ ...this.scope(), workspaceId: this.workspaceId,
      transportKind: 'codex-app-server', agentKind: 'codex', agentVersion: this.agentVersion,
      pid: this.rpc.child.pid, capabilities: { images: true, fileReferences: true, busyState: true,
        agentAcknowledgment: false, completion: true },
    });
    requireValue(typeof registration.registrationId === 'string', 'BRIDGE_REGISTRATION_INVALID');
    this.registrationId = registration.registrationId;
    return { ...this.scope(), agentVersion: this.agentVersion, ownership: 'arkdock-managed-new-thread',
      writable: this.writable, registrationId: this.registrationId };
  }

  async verifySession() {
    requireValue(this.thread && !this.stopped && !this.rpc.closed, 'SESSION_UNAVAILABLE');
    const { thread } = await this.rpc.request('thread/read', { threadId: this.thread.id, includeTurns: false });
    requireValue(thread && thread.id === this.thread.id && thread.sessionId === this.thread.sessionId
      && typeof thread.cwd === 'string' && samePath(await realpath(thread.cwd), this.workspaceRoot), 'THREAD_SCOPE_MISMATCH');
    requireValue(['idle', 'active'].includes(thread.status?.type), 'THREAD_NOT_WRITABLE');
    return { busy: thread.status.type === 'active' || this.submitting
      || [...this.deliveries.values()].some((record) => !record.terminal), activeTurnId: this.activeTurnId };
  }

  async prepareDelivery(delivery) {
    requireValue(delivery && ID.test(delivery.deliveryId ?? '') && /^sel-[a-zA-Z0-9_-]{1,100}$/.test(delivery.selectionId ?? ''), 'DELIVERY_INVALID');
    requireValue(typeof delivery.prompt === 'string' && delivery.prompt.length > 0
      && Buffer.byteLength(delivery.prompt) <= MAX_PROMPT, 'PROMPT_LIMIT');
    requireValue(Array.isArray(delivery.images) && delivery.images.length <= 2, 'IMAGES_INVALID');
    requireValue(delivery.grant?.workspaceId === this.workspaceId, 'GRANT_SCOPE_MISMATCH');
    requireValue(typeof delivery.grant.grantId === 'string'
      && delivery.grant.grantId.startsWith(`${delivery.deliveryId}-`), 'DELIVERY_GRANT_MISMATCH');
    const provider = await SelectionProvider.open(delivery.grant);
    const context = await provider.context(delivery.selectionId);
    const input = [{ type: 'text', text: delivery.prompt, text_elements: [] }];
    for (const image of delivery.images) {
      requireValue(image && isAbsolute(image.path ?? '') && image.mimeType === 'image/jpeg', 'IMAGE_INVALID');
      const imagePath = await realpath(image.path);
      const metadata = [context.screenshot, context.crop].filter(Boolean).find((candidate) =>
        candidate.sha256 === image.sha256 && candidate.byteLength === image.byteLength
        && samePath(resolve(provider.root, delivery.selectionId, candidate.artifactId), imagePath));
      requireValue(metadata, 'IMAGE_NOT_AUTHORIZED');
      // SelectionProvider revalidates scope, runtime lease, digest, size and JPEG framing.
      await provider.call('arkdock.get_selection_screenshot', { selectionId: delivery.selectionId, crop: metadata.artifactId === 'crop.jpg' });
      input.push({ type: 'localImage', path: imagePath });
    }
    return input;
  }

  recordReceipt(record, state, reason) {
    if (record.terminal) return;
    if (state === 'SUBMITTED' && record.submitted) return;
    if (state === 'SUBMITTED') record.submitted = true;
    if (['COMPLETED', 'FAILED', 'UNKNOWN'].includes(state)) record.terminal = true;
    const body = { instanceId: this.instanceId, sessionId: this.thread.sessionId,
      conversationId: this.thread.id, deliveryId: record.id, state,
      receiptId: `${record.id}:${state}`, reason: reason ?? `CODEX_${state}` };
    this.receipts = this.receipts.then(() => this.bridge.receipt(body)).catch((error) => {
      this.receiptFailure = safeReason(error, 'RECEIPT_DELIVERY_FAILED');
      this.stopped = true; // Never submit more after an unobservable receipt/lease failure.
    });
  }

  onNotification({ method, params }) {
    if (!this.thread || params?.threadId !== this.thread.id) return;
    if (method === 'turn/started' && ID.test(params.turn?.id ?? '')) this.activeTurnId = params.turn.id;
    if (method === 'turn/completed' && params.turn?.id === this.activeTurnId) this.activeTurnId = null;
    if (!['item/started', 'item/completed', 'turn/completed'].includes(method)) return;
    const event = { method, turnId: params.turnId ?? params.turn?.id,
      clientId: params.item?.type === 'userMessage' ? params.item.clientId : undefined,
      status: params.turn?.status };
    if (this.submitting) {
      if (this.events.length >= 64) { this.rpc.disconnect('APP_SERVER_EVENT_LIMIT'); return; }
      this.events.push(event);
    } else this.applyEvent(event);
  }

  applyEvent(event) {
    for (const record of this.deliveries.values()) {
      if (!record.turnId || record.turnId !== event.turnId || record.terminal) continue;
      // A correlated user item proves thread submission, not model consumption.
      if (event.clientId === record.id) this.recordReceipt(record, 'SUBMITTED');
      if (event.method === 'turn/completed') {
        if (event.status === 'completed' && record.submitted) this.recordReceipt(record, 'COMPLETED');
        else if (['failed', 'interrupted'].includes(event.status)) this.recordReceipt(record, 'FAILED', 'AGENT_TURN_FAILED');
        else this.recordReceipt(record, 'UNKNOWN', 'TURN_FINISHED_WITHOUT_SUBMISSION');
      }
    }
  }

  async submitMessage(delivery, { steer = false, expectedTurnId } = {}) {
    requireValue(!this.deliveries.has(delivery.deliveryId), 'DUPLICATE_DELIVERY');
    requireValue(this.deliveries.size < 1024, 'SESSION_DELIVERY_LIMIT');
    const status = await this.verifySession();
    requireValue(!status.busy || (steer && expectedTurnId && expectedTurnId === this.activeTurnId), 'AGENT_BUSY');
    requireValue(!steer || (status.busy && expectedTurnId === this.activeTurnId), 'TURN_PRECONDITION_FAILED');
    const input = await this.prepareDelivery(delivery);
    // A second thread read closes the artifact-validation window; never redirect a delivery.
    const current = await this.verifySession();
    requireValue(steer ? current.busy && current.activeTurnId === expectedTurnId : !current.busy, 'AGENT_BUSY');
    const record = { id: delivery.deliveryId, terminal: false, submitted: false, turnId: null };
    this.deliveries.set(record.id, record); this.submitting = true;
    try {
      const response = await this.rpc.request(steer ? 'turn/steer' : 'turn/start', {
        threadId: this.thread.id, input, clientUserMessageId: record.id,
        ...(steer ? { expectedTurnId } : {}),
      });
      const turnId = steer ? response.turnId : response.turn?.id;
      requireValue(ID.test(turnId ?? '') && (!steer || turnId === expectedTurnId), 'TURN_RESPONSE_INVALID');
      record.turnId = turnId;
      this.recordReceipt(record, 'TRANSPORT_ACCEPTED');
      for (const event of this.events) this.applyEvent(event);
      return { turnId, deliveryId: record.id };
    } catch (error) {
      // A timeout/disconnect after writing is ambiguous: never replay a user request.
      this.recordReceipt(record, error.message === 'APP_SERVER_REQUEST_REJECTED' ? 'FAILED' : 'UNKNOWN', safeReason(error));
      throw error;
    } finally { this.submitting = false; this.events = []; await this.receipts; }
  }

  async pollOnce() {
    const { busy } = await this.verifySession();
    const result = await this.bridge.poll({ ...this.scope(), busy });
    requireValue(Array.isArray(result.deliveries) && result.deliveries.length <= 1, 'BRIDGE_DELIVERIES_INVALID');
    for (const delivery of result.deliveries) {
      try { await this.submitMessage(delivery); }
      catch (error) {
        if (!this.deliveries.has(delivery.deliveryId)) {
          const record = { id: delivery.deliveryId, terminal: false };
          this.recordReceipt(record, 'FAILED', safeReason(error));
        }
      }
    }
    await this.receipts;
    requireValue(!this.receiptFailure, this.receiptFailure ?? 'RECEIPT_DELIVERY_FAILED');
  }

  async run(signal) {
    while (!signal.aborted && !this.stopped) {
      await this.pollOnce();
      await delay(500, undefined, { signal }).catch(() => {});
    }
  }

  async dispose() {
    this.stopped = true;
    const active = this.activeTurnId ?? [...this.deliveries.values()].find((record) => !record.terminal)?.turnId;
    if (active && this.thread && !this.rpc.closed) {
      await Promise.race([
        this.rpc.request('turn/interrupt', { threadId: this.thread.id, turnId: active }).catch(() => {}),
        delay(750, undefined, { ref: false }),
      ]);
    }
    await this.rpc.dispose();
    await this.receipts;
    if (this.thread && this.registrationId) await this.bridge.revoke(this.scope()).catch(() => {});
  }
}

async function main() {
  const args = process.argv.slice(2);
  if (args.includes('--help')) {
    process.stdout.write('Usage: node tools/agent/codex-app-server.mjs --create-managed --pairing-file <absolute.json> --codex-executable <absolute codex.exe> [--workspace-write] [--model <id>]\nCreates a NEW managed conversation. Does not attach to an existing CLI or Desktop conversation.\n');
    return;
  }
  const options = {};
  for (let i = 0; i < args.length; i += 1) {
    const key = args[i];
    requireValue(['--create-managed', '--workspace-write', '--pairing-file', '--codex-executable', '--model'].includes(key)
      && options[key] === undefined, 'ARGUMENTS_INVALID');
    options[key] = ['--create-managed', '--workspace-write'].includes(key) ? true : args[++i];
    requireValue(options[key] !== undefined, 'ARGUMENTS_INVALID');
  }
  requireValue(options['--create-managed'] === true, 'EXPLICIT_MANAGED_CREATION_REQUIRED');
  const pair = await readPairingFile(options['--pairing-file']);
  requireValue(pair.agentKind === undefined || pair.agentKind === 'codex', 'PAIRING_AGENT_MISMATCH');
  const rpc = await startOwnedServer(options['--codex-executable'], pair.workspaceRoot);
  const transport = new ManagedCodexTransport({ rpc, bridge: new BridgeClient(pair),
    workspaceRoot: pair.workspaceRoot, workspaceId: pair.workspaceId,
    writable: options['--workspace-write'] === true, model: options['--model'],
  });
  const controller = new AbortController();
  process.once('SIGINT', () => controller.abort());
  process.once('SIGTERM', () => controller.abort());
  try {
    const identity = await transport.initialize();
    process.stdout.write(`${JSON.stringify({ event: 'managed-session-ready', ...identity })}\n`);
    await transport.run(controller.signal);
  } finally { await transport.dispose(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => { process.stderr.write(`${safeReason(error)}\n`); process.exitCode = 1; });
}
