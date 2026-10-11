import { open, realpath, lstat } from 'node:fs/promises';
import { isAbsolute, resolve } from 'node:path';

const MAX_BYTES = 128 * 1024;
export function requireValue(value, reason) { if (!value) throw new Error(reason); }
export function safeReason(error, fallback = 'TRANSPORT_FAILED') {
  return /^[A-Z][A-Z0-9_]{0,95}$/.test(error?.message ?? '') ? error.message : fallback;
}

export function normalizeLocalWorkspaceRoot(value, platform = process.platform) {
  requireValue(typeof value === 'string' && value.length > 0
    && !Array.from(value).some((character) => character.charCodeAt(0) < 32), 'PAIRING_INVALID');
  if (platform === 'win32') {
    // Rust canonicalize emits the extended local drive namespace. Only this
    // exact prefix may be shortened; UNC, WSL and device namespaces stay blocked.
    const local = /^\\\\\?\\[A-Za-z]:[\\/]/.test(value) ? value.slice(4) : value;
    requireValue(/^[A-Za-z]:[\\/]/.test(local), 'PAIRING_INVALID');
    return local;
  }
  requireValue(isAbsolute(value) && !value.startsWith('\\\\'), 'PAIRING_INVALID');
  return value;
}

export function validateEndpoint(endpoint) {
  requireValue(typeof endpoint === 'string' && endpoint.length < 256, 'BRIDGE_ENDPOINT_INVALID');
  // Check the literal hostname as well as the URL: URL normalizes 127.1 and integer IPs.
  requireValue(/^http:\/\/127\.0\.0\.1:[1-9][0-9]{0,4}\/?$/.test(endpoint), 'BRIDGE_ENDPOINT_INVALID');
  const url = new URL(endpoint);
  requireValue(url.port && Number(url.port) <= 65535, 'BRIDGE_ENDPOINT_INVALID');
  return url.origin;
}

export async function readPairingFile(file, now = Date.now) {
  requireValue(typeof file === 'string' && isAbsolute(file), 'PAIRING_PATH_INVALID');
  const canonical = await realpath(file);
  const info = await lstat(file);
  requireValue(info.isFile() && !info.isSymbolicLink()
    && (process.platform === 'win32' ? canonical.toLowerCase() === resolve(file).toLowerCase()
      : canonical === resolve(file)), 'PAIRING_PATH_INVALID');
  const handle = await open(canonical, 'r');
  let pair;
  try {
    const stat = await handle.stat();
    requireValue(stat.isFile() && stat.size > 0 && stat.size <= 16 * 1024, 'PAIRING_FILE_LIMIT');
    const buffer = Buffer.alloc(16 * 1024 + 1);
    let offset = 0;
    while (offset < buffer.length) {
      const { bytesRead } = await handle.read(buffer, offset, buffer.length - offset, offset);
      if (!bytesRead) break;
      offset += bytesRead;
    }
    requireValue(offset <= 16 * 1024, 'PAIRING_FILE_LIMIT');
    try { pair = JSON.parse(buffer.subarray(0, offset).toString('utf8')); }
    catch { throw new Error('PAIRING_INVALID'); }
  } finally { await handle.close(); }
  requireValue(pair && typeof pair === 'object' && !Array.isArray(pair), 'PAIRING_INVALID');
  validateEndpoint(pair.endpoint);
  requireValue(typeof pair.token === 'string' && /^[A-Za-z0-9_-]{32,256}$/.test(pair.token), 'PAIRING_INVALID');
  requireValue(typeof pair.workspaceId === 'string' && pair.workspaceId.length > 0
    && pair.workspaceId.length <= 256, 'PAIRING_INVALID');
  pair.workspaceRoot = normalizeLocalWorkspaceRoot(pair.workspaceRoot);
  requireValue(Number.isSafeInteger(pair.expiresAt) && pair.expiresAt > now(), 'PAIRING_EXPIRED');
  return pair;
}

export class BridgeClient {
  constructor({ endpoint, token, timeoutMs = 3000 }) {
    this.endpoint = validateEndpoint(endpoint);
    requireValue(typeof token === 'string' && /^[A-Za-z0-9_-]{32,256}$/.test(token), 'BRIDGE_TOKEN_INVALID');
    requireValue(Number.isInteger(timeoutMs) && timeoutMs > 0 && timeoutMs <= 10000, 'BRIDGE_TIMEOUT_INVALID');
    this.token = token;
    this.timeoutMs = timeoutMs;
  }

  async request(route, body, { signal } = {}) {
    requireValue(body && typeof body === 'object' && !Array.isArray(body), 'BRIDGE_REQUEST_INVALID');
    const serialized = JSON.stringify(body);
    requireValue(Buffer.byteLength(serialized) <= MAX_BYTES, 'BRIDGE_REQUEST_LIMIT');
    const timeout = AbortSignal.timeout(this.timeoutMs);
    const response = await fetch(`${this.endpoint}/v1/${route}`, {
      method: 'POST', redirect: 'error',
      headers: { authorization: `Bearer ${this.token}`, 'content-type': 'application/json' },
      body: serialized, signal: signal ? AbortSignal.any([signal, timeout]) : timeout,
    }).catch(() => { throw new Error('BRIDGE_UNREACHABLE'); });
    if (!response.ok) { await response.body?.cancel(); throw new Error(`BRIDGE_HTTP_${response.status}`); }
    if (!(response.headers.get('content-type') ?? '').toLowerCase().startsWith('application/json')) {
      await response.body?.cancel(); throw new Error('BRIDGE_RESPONSE_INVALID');
    }
    const chunks = []; let length = 0;
    const reader = response.body.getReader();
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        length += value.byteLength;
        requireValue(length <= MAX_BYTES, 'BRIDGE_RESPONSE_LIMIT');
        chunks.push(value);
      }
    } catch (error) { await reader.cancel().catch(() => {}); throw new Error(safeReason(error, 'BRIDGE_RESPONSE_FAILED')); }
    let result;
    try { result = JSON.parse(Buffer.concat(chunks).toString('utf8')); }
    catch { throw new Error('BRIDGE_RESPONSE_INVALID'); }
    requireValue(result && typeof result === 'object' && !Array.isArray(result), 'BRIDGE_RESPONSE_INVALID');
    return result;
  }

  register(body, options) { return this.request('register', body, options); }
  poll(body, options) { return this.request('poll', body, options); }
  receipt(body, options) { return this.request('receipt', body, options); }
  revoke(body, options) { return this.request('revoke', body, options); }
}
