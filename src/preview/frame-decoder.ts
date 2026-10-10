import type { AppSnapshot, BinaryFrameMetadata } from '../ipc/model.ts';
import type { BinaryPreviewFrame, PreviewFrame } from './frame-types.ts';
import { isBinaryFrame } from './frame-types.ts';

export const FRAME_PROTOCOL_VERSION = 1;
export const MAX_METADATA_BYTES = 4096;
export const MAX_JPEG_BYTES = 1024 * 1024 - 40;
const MAX_PIXELS = 16 * 1024 * 1024;
const idValid = (value: unknown): value is string => typeof value === 'string' && value.length > 0 && new TextEncoder().encode(value).length <= 256;
const positive = (value: unknown): value is number => Number.isSafeInteger(value) && Number(value) > 0;

/** Read JPEG SOF dimensions before invoking the browser's allocating decoder. */
function jpegDimensions(bytes: Uint8Array): { width: number; height: number } {
  let offset = 2;
  while (offset + 4 <= bytes.length) {
    if (bytes[offset++] !== 0xff) throw new Error('FRAME_JPEG_MARKER');
    while (bytes[offset] === 0xff) offset++;
    const marker = bytes[offset++];
    if (marker === 0xd9 || marker === 0xda) break;
    if (marker === 0x01 || (marker >= 0xd0 && marker <= 0xd7)) continue;
    const size = (bytes[offset] << 8) | bytes[offset + 1];
    if (size < 2 || offset + size > bytes.length) throw new Error('FRAME_JPEG_SEGMENT');
    if ([0xc0, 0xc1, 0xc2, 0xc3, 0xc5, 0xc6, 0xc7, 0xc9, 0xca, 0xcb, 0xcd, 0xce, 0xcf].includes(marker)) {
      if (size < 8) throw new Error('FRAME_JPEG_DIMENSIONS');
      return { height: (bytes[offset + 3] << 8) | bytes[offset + 4], width: (bytes[offset + 5] << 8) | bytes[offset + 6] };
    }
    offset += size;
  }
  throw new Error('FRAME_JPEG_DIMENSIONS');
}

/** Empty mailbox is null. All envelope, allocation and identity gates precede decode. */
export function parseBinaryFrame(packet: ArrayBuffer, state: AppSnapshot, previous: PreviewFrame | null = null,
  receivedAt = performance.now()): BinaryPreviewFrame | null {
  if (!(packet instanceof ArrayBuffer)) throw new Error('FRAME_RESPONSE_TYPE');
  if (packet.byteLength === 0) return null;
  if (packet.byteLength < 8 || packet.byteLength > 4 + MAX_METADATA_BYTES + MAX_JPEG_BYTES) throw new Error('FRAME_PACKET_LIMIT');
  const length = new DataView(packet).getUint32(0, true);
  if (!length || length > MAX_METADATA_BYTES || 4 + length > packet.byteLength - 4) throw new Error('FRAME_METADATA_LIMIT');
  const metadata: BinaryFrameMetadata = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array(packet, 4, length)));
  if (!metadata || metadata.protocolVersion !== FRAME_PROTOCOL_VERSION || metadata.format !== 'jpeg'
    || ![metadata.workspaceId, metadata.sessionId, metadata.buildId].every(idValid)
    || !positive(metadata.generation) || !positive(metadata.sequence)
    || !positive(metadata.width) || metadata.width > 8192 || !positive(metadata.height) || metadata.height > 8192
    || metadata.width * metadata.height > MAX_PIXELS || metadata.rotation !== 0
    || !Number.isSafeInteger(metadata.capturedAt) || metadata.capturedAt < 0
    || !positive(metadata.payloadLength) || metadata.payloadLength !== packet.byteLength - 4 - length
    || metadata.payloadLength > MAX_JPEG_BYTES) throw new Error('FRAME_METADATA_INVALID');
  // Identity and ordering reject stale packets without copying their JPEG payload.
  if (!state.target || !state.project || !state.trusted || !state.sessionId || metadata.workspaceId !== state.project.id
    || metadata.sessionId !== state.sessionId || metadata.buildId !== state.build.buildId || metadata.generation !== state.build.generation
    || (previous?.sessionId === metadata.sessionId && previous.buildId === metadata.buildId
      && previous.generation === metadata.generation && previous.sequence >= metadata.sequence)) return null;
  const jpeg = new Uint8Array(packet, 4 + length);
  if (jpeg.length < 4 || jpeg[0] !== 0xff || jpeg[1] !== 0xd8 || jpeg[jpeg.length - 2] !== 0xff || jpeg[jpeg.length - 1] !== 0xd9) throw new Error('FRAME_JPEG_ENCODING');
  const dimensions = jpegDimensions(jpeg);
  if (dimensions.width !== metadata.width || dimensions.height !== metadata.height) throw new Error('FRAME_JPEG_DIMENSIONS');
  return Object.freeze({ ...metadata, jpeg: packet.slice(4 + length), receivedAt });
}

export interface DecodedImage { width: number; height: number; close: () => void }
export async function decodePreviewFrame(frame: PreviewFrame): Promise<ImageBitmap> {
  let blob: Blob;
  if (isBinaryFrame(frame)) blob = new Blob([frame.jpeg], { type: 'image/jpeg' });
  else {
    const decoded = atob(frame.pngBase64);
    const bytes = new Uint8Array(decoded.length);
    for (let index = 0; index < decoded.length; index++) bytes[index] = decoded.charCodeAt(index);
    blob = new Blob([bytes], { type: 'image/png' });
  }
  const image = await createImageBitmap(blob);
  if (image.width !== frame.width || image.height !== frame.height) { image.close(); throw new Error('FRAME_DECODE_DIMENSIONS'); }
  return image;
}
