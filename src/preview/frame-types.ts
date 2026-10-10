import type { BinaryFrameMetadata, NativeFrame } from '../ipc/model.ts';

/** Compressed pixels stay outside AppSnapshot and React application state. */
export type BinaryPreviewFrame = BinaryFrameMetadata & { jpeg: ArrayBuffer; receivedAt: number };
export type PreviewFrame = NativeFrame | BinaryPreviewFrame;
export type FrameMetadata = Omit<NativeFrame, 'pngBase64'>;
export const isBinaryFrame = (frame: PreviewFrame): frame is BinaryPreviewFrame => 'jpeg' in frame;
