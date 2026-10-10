import type { BinaryFrameMetadata, InspectorBounds, InspectorNode, InspectorTree } from '../ipc/model.ts';

export function sameInspectionFrame(a: BinaryFrameMetadata, b: BinaryFrameMetadata): boolean {
  return a.workspaceId === b.workspaceId && a.sessionId === b.sessionId && a.buildId === b.buildId
    && a.generation === b.generation && a.sequence === b.sequence;
}

export function treeMatchesFrame(tree: InspectorTree | null, frame: BinaryFrameMetadata | null): boolean {
  return Boolean(tree && frame && tree.workspaceId === frame.workspaceId && tree.sessionId === frame.sessionId
    && tree.buildId === frame.buildId && tree.generation === frame.generation && tree.frameSequence === frame.sequence);
}
export function treeMatchesRuntime(tree: InspectorTree | null, frame: BinaryFrameMetadata | null): boolean {
  return Boolean(tree && frame && tree.workspaceId === frame.workspaceId && tree.sessionId === frame.sessionId
    && tree.buildId === frame.buildId && tree.generation === frame.generation && tree.frameSequence <= frame.sequence);
}
export function samePixels(a: ArrayBuffer | undefined, b: ArrayBuffer | undefined): boolean {
  if (!a || !b || a.byteLength !== b.byteLength) return false;
  if (a === b) return true;
  const left = new Uint8Array(a), right = new Uint8Array(b);
  for (let i = 0; i < left.length; i++) if (left[i] !== right[i]) return false;
  return true;
}
/** Strip frontend pixel/lifecycle fields before the deny-unknown-fields native boundary. */
export function inspectionMetadata(frame: BinaryFrameMetadata): BinaryFrameMetadata {
  return { protocolVersion: frame.protocolVersion, workspaceId: frame.workspaceId, sessionId: frame.sessionId,
    buildId: frame.buildId, generation: frame.generation, sequence: frame.sequence, width: frame.width, height: frame.height,
    capturedAt: frame.capturedAt, rotation: frame.rotation, format: frame.format, payloadLength: frame.payloadLength };
}

export function regionBetween(a: { x: number; y: number }, b: { x: number; y: number }, width: number, height: number): InspectorBounds | null {
  const x = Math.max(0, Math.min(a.x, b.x));
  const y = Math.max(0, Math.min(a.y, b.y));
  const right = Math.min(width, Math.max(a.x, b.x) + 1);
  const bottom = Math.min(height, Math.max(a.y, b.y) + 1);
  return right > x && bottom > y ? { x, y, width: right - x, height: bottom - y } : null;
}

const contains = (b: InspectorBounds, x: number, y: number) => Number.isFinite(b.x + b.y + b.width + b.height)
  && b.width > 0 && b.height > 0 && x >= b.x && y >= b.y && x < b.x + b.width && y < b.y + b.height;

/** Bounded spatial buckets; movement never queries Previewer or scans the full tree. */
export function createHitIndex(nodes: InspectorNode[], width: number, height: number) {
  const byId = new Map(nodes.map((node) => [node.nodeId, node]));
  const cell = 32;
  const columns = Math.ceil(width / cell);
  const buckets = new Map<number, InspectorNode[]>();
  for (const node of nodes) {
    const bounds = node.bounds;
    if (!bounds || node.visible === false || !Number.isFinite(bounds.x + bounds.y + bounds.width + bounds.height) || bounds.width <= 0 || bounds.height <= 0) continue;
    const x0 = Math.max(0, Math.floor(bounds.x / cell));
    const x1 = Math.min(columns - 1, Math.floor((bounds.x + bounds.width) / cell));
    const y0 = Math.max(0, Math.floor(bounds.y / cell));
    const y1 = Math.min(Math.ceil(height / cell) - 1, Math.floor((bounds.y + bounds.height) / cell));
    for (let y = y0; y <= y1; y++) for (let x = x0; x <= x1; x++) {
      const key = y * columns + x;
      const bucket = buckets.get(key) ?? [];
      bucket.push(node); buckets.set(key, bucket);
    }
  }
  return {
    hit(x: number, y: number): InspectorNode[] {
      if (x < 0 || y < 0 || x >= width || y >= height) return [];
      return (buckets.get(Math.floor(y / cell) * columns + Math.floor(x / cell)) ?? []).filter((node) => {
        if (!node.bounds || !contains(node.bounds, x, y)) return false;
        let parent = node.parentId ? byId.get(node.parentId) : undefined;
        let remaining = nodes.length;
        while (parent && remaining-- > 0) {
          if (parent.visible === false) return false;
          const clips = parent.attributes.some((attribute) => /^(clip|\$clip)$/.test(attribute.name) && attribute.value === 'true');
          if (clips && parent.bounds && !contains(parent.bounds, x, y)) return false;
          parent = parent.parentId ? byId.get(parent.parentId) : undefined;
        }
        return remaining >= 0;
      }).sort((a, b) => b.depth - a.depth || ((b.paintOrder ?? 0) - (a.paintOrder ?? 0)) || a.nodeId.localeCompare(b.nodeId));
    },
  };
}
