import type { DeviceFrameMetadata, DeviceRunIdentity, DeviceRuntimeAction, DeviceRuntimeState, RequestScope } from '../ipc/model.ts';

export const MAX_DEVICE_PNG_BYTES = 8 * 1024 * 1024;
export const DEVICE_FRAME_INTERVAL_MS = 100;
export const DEVICE_INPUT_MAX_AGE_MS = 5000;
const HEADER_BYTES = 16;
const METADATA_LIMIT = 4096;
const PNG_SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

export interface DeviceFrame { metadata: DeviceFrameMetadata; png: ArrayBuffer }
export interface DeviceBitmap { width: number; height: number; close(): void }
interface FrameClock { now(): number; schedule(callback: () => void, delay: number): unknown; cancel(id: unknown): void }

function integer(value: unknown, min: number, max = Number.MAX_SAFE_INTEGER): value is number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= min && value <= max;
}
function identifier(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0 && new TextEncoder().encode(value).length <= 256
    && Array.from(value).every((character) => character.charCodeAt(0) >= 32 && character.charCodeAt(0) !== 127);
}

/** Validate bounds and structure before asking the browser to allocate a bitmap. */
export function parseDeviceFrame(packet: ArrayBuffer, identity: DeviceRunIdentity, bundle: string | null, afterSequence = 0): DeviceFrame | null {
  if (packet.byteLength === 0) return null;
  if (packet.byteLength < HEADER_BYTES || packet.byteLength > HEADER_BYTES + METADATA_LIMIT + MAX_DEVICE_PNG_BYTES) throw new Error('DEVICE_FRAME_PACKET_LIMIT');
  const view = new DataView(packet), bytes = new Uint8Array(packet);
  if (bytes[0] !== 65 || bytes[1] !== 68 || bytes[2] !== 68 || bytes[3] !== 70 || view.getUint32(4, true) !== 1) throw new Error('DEVICE_FRAME_PROTOCOL');
  const jsonLength = view.getUint32(8, true), pngLength = view.getUint32(12, true);
  if (!jsonLength || jsonLength > METADATA_LIMIT || pngLength > MAX_DEVICE_PNG_BYTES || pngLength < 57
    || HEADER_BYTES + jsonLength + pngLength !== bytes.byteLength) throw new Error('DEVICE_FRAME_LENGTH');
  const raw: unknown = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(HEADER_BYTES, HEADER_BYTES + jsonLength)));
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('DEVICE_FRAME_METADATA');
  const metadata = raw as DeviceFrameMetadata;
  if (!identifier(metadata.runId) || !identifier(metadata.targetId) || !identifier(metadata.bundleName)
    || !integer(metadata.sequence, 1) || !integer(metadata.width, 1, 4096) || !integer(metadata.height, 1, 4096)
    || metadata.width * metadata.height > 8_388_608 || !integer(metadata.captureStartedAt, 0) || !integer(metadata.receivedAt, metadata.captureStartedAt)
    || metadata.payloadLength !== pngLength || typeof metadata.foregroundVerified !== 'boolean') throw new Error('DEVICE_FRAME_METADATA');
  if (metadata.runId !== identity.runId || metadata.targetId !== identity.targetId || metadata.bundleName !== bundle || metadata.sequence <= afterSequence) return null;
  const png = packet.slice(HEADER_BYTES + jsonLength), data = new Uint8Array(png), header = new DataView(png);
  if (!PNG_SIGNATURE.every((byte, index) => data[index] === byte) || header.getUint32(8) !== 13
    || header.getUint32(12) !== 0x49484452 || header.getUint32(16) !== metadata.width || header.getUint32(20) !== metadata.height) throw new Error('DEVICE_FRAME_PNG_DIMENSIONS');
  let offset = 8, hasPixels = false, ended = false;
  while (offset + 12 <= png.byteLength) {
    const length = header.getUint32(offset), type = header.getUint32(offset + 4);
    if (length > png.byteLength - offset - 12) throw new Error('DEVICE_FRAME_PNG_TRUNCATED');
    if (type === 0x49444154) hasPixels = true;
    offset += 12 + length;
    if (type === 0x49454e44) { ended = length === 0 && offset === png.byteLength; break; }
  }
  if (!ended || !hasPixels) throw new Error('DEVICE_FRAME_PNG_TRUNCATED');
  return { metadata, png };
}

export function deviceFrameInputReady(state: DeviceRuntimeState | null, displayed: DeviceFrameMetadata | null, now = Date.now()): boolean {
  return Boolean(state?.identity && displayed && state.foregroundVerified && displayed.foregroundVerified && state.frameReady
    && state.identity.runId === displayed.runId && state.identity.targetId === displayed.targetId && state.selectedTargetId === displayed.targetId && state.bundleName === displayed.bundleName
    && state.frame?.sequence === displayed.sequence && now >= displayed.receivedAt && now - displayed.captureStartedAt <= DEVICE_INPUT_MAX_AGE_MS);
}

/** A gesture targets the image observed at pointer-down. A newer screenshot
 * cannot retroactively authorize those coordinates against its new contents. */
export function deviceGestureFrameMatches(start: DeviceFrameMetadata, current: DeviceFrameMetadata | null): boolean {
  return Boolean(current && start.runId === current.runId && start.targetId === current.targetId
    && start.bundleName === current.bundleName && start.sequence === current.sequence
    && start.width === current.width && start.height === current.height);
}

/** Start and input can request a follow-up capture before streaming becomes
 * visible in a state receipt. Revocation must also cancel that in-flight work. */
export function deviceActionMayCapture(action: DeviceRuntimeAction, captureConsent: boolean): boolean {
  return captureConsent && ['capture', 'streamStart', 'start', 'input'].includes(action);
}

export function deviceCaptureCleanupScope(owner: { targetId: string | null; workspaceId: string | null }, current: { targetId: string | null; workspaceId: string | null; sessionId: string | null }): RequestScope | null {
  if (!owner.targetId || !owner.workspaceId || owner.targetId !== current.targetId || owner.workspaceId !== current.workspaceId) return null;
  return { targetId: owner.targetId, sessionId: current.sessionId };
}

/** object-fit:contain mapping. Bars and the SDK's unsupported zero coordinate
 * are rejected instead of quietly changing the user's target pixel. */
export function devicePoint(clientX: number, clientY: number, bounds: { left: number; top: number; width: number; height: number }, frame: DeviceFrameMetadata): { x: number; y: number } | null {
  if (bounds.width <= 0 || bounds.height <= 0 || !Number.isFinite(clientX) || !Number.isFinite(clientY)) return null;
  const scale = Math.min(bounds.width / frame.width, bounds.height / frame.height);
  const x = Math.floor((clientX - bounds.left - (bounds.width - frame.width * scale) / 2) / scale);
  const y = Math.floor((clientY - bounds.top - (bounds.height - frame.height * scale) / 2) / scale);
  return x > 0 && y > 0 && x < frame.width && y < frame.height ? { x, y } : null;
}

interface ControllerOptions<T extends DeviceBitmap> {
  read(scope: RequestScope, identity: DeviceRunIdentity, afterSequence: number): Promise<ArrayBuffer>;
  decode(frame: DeviceFrame): Promise<T>;
  draw(bitmap: T, frame: DeviceFrameMetadata): void;
  clear(): void;
  error(reason: unknown): void;
  clock?: FrameClock;
}

/** One scoped read/decode pipeline; no queued PNGs. Notifications coalesce to
 * the newest backend sequence. Hiding, revoking consent or changing identity
 * invalidates both pending work and retained pixels immediately. */
export class DeviceFrameController<T extends DeviceBitmap> {
  private options: ControllerOptions<T>;
  private clock: FrameClock;
  private state: DeviceRuntimeState | null = null;
  private scope: RequestScope = { targetId: null, sessionId: null };
  private key = '';
  private epoch = 0;
  private sequence = 0;
  private attemptedSequence = 0;
  private timer: unknown = null;
  private reading = false;
  private visible = true;
  private enabled = false;
  private disposed = false;
  private lastRead = -Infinity;
  constructor(options: ControllerOptions<T>) {
    this.options = options;
    this.clock = options.clock ?? { now: () => performance.now(), schedule: (callback, delay) => setTimeout(callback, delay), cancel: (id) => clearTimeout(id as ReturnType<typeof setTimeout>) };
  }
  update(state: DeviceRuntimeState | null, scope: RequestScope, enabled: boolean): void {
    if (this.disposed) return;
    const id = state?.identity;
    const key = JSON.stringify([scope.targetId, id?.workspaceId, id?.runId, id?.generation, id?.targetId, state?.bundleName, state?.frameReady, Boolean(state?.frame), enabled]);
    if (key !== this.key) { this.key = key; this.invalidate(); }
    this.state = state; this.scope = { ...scope }; this.enabled = enabled;
    if (!state?.frameReady || !state.frame) { this.sequence = 0; this.attemptedSequence = 0; this.options.clear(); }
    this.schedule();
  }
  setVisible(visible: boolean): void {
    if (this.visible === visible || this.disposed) return;
    this.visible = visible;
    if (!visible) this.invalidate(); else this.schedule();
  }
  dispose(): void { if (this.disposed) return; this.disposed = true; this.invalidate(); this.state = null; }
  private invalidate(): void {
    this.epoch++; this.sequence = 0; this.attemptedSequence = 0;
    if (this.timer !== null) this.clock.cancel(this.timer);
    this.timer = null; this.options.clear();
  }
  private wantsRead(): boolean {
    return !this.disposed && this.visible && this.enabled && Boolean(this.scope.targetId && this.state?.identity?.targetId && this.state.frameReady
      && this.state.frame && this.state.frame.sequence > Math.max(this.sequence, this.attemptedSequence));
  }
  private schedule(): void {
    if (this.reading || this.timer !== null || !this.wantsRead()) return;
    const delay = Math.max(0, DEVICE_FRAME_INTERVAL_MS - (this.clock.now() - this.lastRead));
    this.timer = this.clock.schedule(() => { this.timer = null; void this.read(); }, delay);
  }
  private async read(): Promise<void> {
    if (this.reading || !this.wantsRead()) return;
    const state = this.state!, identity = { ...state.identity! }, scope = { ...this.scope }, epoch = this.epoch;
    this.reading = true; this.lastRead = this.clock.now(); this.attemptedSequence = state.frame!.sequence;
    try {
      const packet = await this.options.read(scope, identity, this.sequence);
      if (epoch !== this.epoch || !this.visible || this.disposed || !this.enabled) return;
      const frame = parseDeviceFrame(packet, identity, state.bundleName, this.sequence);
      if (!frame) return;
      const bitmap = await this.options.decode(frame);
      try {
        // A faster producer must not starve display forever. This completed
        // bitmap is still from the current epoch; show it while fetching the
        // latest frame next. Input separately requires the exact backend sequence.
        if (epoch !== this.epoch || !this.visible || this.disposed || !this.enabled) return;
        if (bitmap.width !== frame.metadata.width || bitmap.height !== frame.metadata.height) throw new Error('DEVICE_FRAME_DECODE_DIMENSIONS');
        this.options.draw(bitmap, frame.metadata);
        this.sequence = frame.metadata.sequence;
      } finally { bitmap.close(); }
    } catch (reason) {
      if (epoch === this.epoch && !this.disposed) { this.options.clear(); this.options.error(reason); }
    } finally { this.reading = false; this.schedule(); }
  }
}
