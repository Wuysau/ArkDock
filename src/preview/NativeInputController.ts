import type { NativeControlIdentity, NativeControlReceipt, NativeControlRequest, RequestScope } from '../ipc/model.ts';

export function controlRequest(identity: NativeControlIdentity, fields: Partial<Omit<NativeControlRequest, 'identity'>> & Pick<NativeControlRequest, 'action'>): NativeControlRequest {
  return { identity, gestureId: null, x: null, y: null, text: null, key: null, orientation: null,
    colorMode: null, page: null, profileId: null, width: null, height: null, dpi: null, ...fields };
}

type Point = { x: number; y: number };
type Gesture = { id: string; identity: NativeControlIdentity; scope: RequestScope; origin: Point; point: Point;
  pending: Point | null; tail: Promise<void>; cleanup: Promise<void> | null; released: Promise<boolean>; settle: (success: boolean) => void;
  moving: boolean; closing: boolean; cancelled: boolean; dragged: boolean };

const sameIdentity = (left: NativeControlIdentity, right: NativeControlIdentity) => left.workspaceId === right.workspaceId
  && left.sessionId === right.sessionId && left.buildId === right.buildId && left.generation === right.generation
  && left.width === right.width && left.height === right.height;

/** At most one move is in flight and one latest position is retained. Down and Up are never coalesced. */
export class NativeInputController {
  private gesture: Gesture | null = null;
  private raf: number | null = null;
  private sequence = 0;
  constructor(private send: (request: NativeControlRequest, scope: RequestScope) => Promise<NativeControlReceipt>,
    private release: (identity: NativeControlIdentity, scope: RequestScope) => Promise<unknown>,
    private fail: (reason: unknown, identity: NativeControlIdentity) => void) {}

  start(identity: NativeControlIdentity, scope: RequestScope, point: Point): boolean {
    if (this.gesture) { this.fail('上一原生手势尚未释放；本次点击未提交。', identity); return false; }
    let settle!: (success: boolean) => void;
    const released = new Promise<boolean>((resolve) => { settle = resolve; });
    const gesture: Gesture = { id: `pointer-${Date.now()}-${++this.sequence}`, identity, scope, origin: point,
      point, pending: null, tail: Promise.resolve(), cleanup: null, released, settle,
      moving: false, closing: false, cancelled: false, dragged: false };
    this.gesture = gesture;
    gesture.tail = this.dispatch(gesture, 'pointerDown', point);
    void gesture.tail.then(() => this.schedule(gesture), (reason) => { this.fail(reason, gesture.identity); void this.cancel(); });
    return true;
  }

  move(point: Point): void {
    const gesture = this.gesture;
    if (!gesture || gesture.closing || gesture.cancelled) return;
    gesture.point = point;
    // A tap remains a down/up pair. Only motion beyond three native pixels begins a drag.
    if (!gesture.dragged && Math.hypot(point.x - gesture.origin.x, point.y - gesture.origin.y) < 3) return;
    gesture.dragged = true; gesture.pending = point;
    this.schedule(gesture);
  }

  async end(point: Point): Promise<boolean> {
    const gesture = this.gesture;
    if (!gesture || gesture.closing || gesture.cancelled) return false;
    this.move(point); gesture.closing = true; this.cancelScheduled();
    try {
      await gesture.tail;
      if (gesture.cancelled) return false;
      const latest = gesture.pending;
      gesture.pending = null;
      if (latest) await this.dispatch(gesture, 'pointerMove', latest);
      if (!gesture.cancelled) await this.dispatch(gesture, 'pointerUp', gesture.dragged ? point : gesture.origin);
      const success = !gesture.cancelled;
      gesture.settle(success);
      return success;
    } catch (reason) { this.fail(reason, gesture.identity); await this.cancel(); return false; }
    finally { gesture.settle(false); if (this.gesture === gesture) this.gesture = null; }
  }

  /** Locally captured text may wait for Up, but never for a still-held or unrelated gesture. */
  async waitForRelease(identity: NativeControlIdentity, scope: RequestScope): Promise<boolean> {
    const gesture = this.gesture;
    if (!gesture) return true;
    if (gesture.cancelled || !gesture.closing || !sameIdentity(identity, gesture.identity)
      || scope.targetId !== gesture.scope.targetId || scope.sessionId !== gesture.scope.sessionId) return false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race([gesture.released, new Promise<boolean>((resolve) => {
        timer = setTimeout(() => resolve(false), 15_000);
      })]);
    } finally { clearTimeout(timer); }
  }

  async cancel(): Promise<void> {
    const gesture = this.gesture;
    if (!gesture) return;
    if (gesture.cleanup) { await gesture.cleanup; return; }
    gesture.cancelled = true; gesture.closing = true; gesture.pending = null; this.cancelScheduled();
    gesture.settle(false);
    gesture.cleanup = (async () => {
      try { await gesture.tail.catch(() => {}); await this.release(gesture.identity, gesture.scope); }
      catch (reason) { this.fail(reason, gesture.identity); }
      finally { if (this.gesture === gesture) this.gesture = null; }
    })();
    await gesture.cleanup;
  }

  private cancelScheduled(): void { if (this.raf !== null) cancelAnimationFrame(this.raf); this.raf = null; }
  private schedule(gesture: Gesture): void {
    if (this.gesture !== gesture || gesture.closing || gesture.cancelled || gesture.moving || !gesture.pending || this.raf !== null) return;
    this.raf = requestAnimationFrame(() => {
      this.raf = null;
      if (gesture.closing || gesture.cancelled || gesture.moving || !gesture.pending) return;
      const point = gesture.pending; gesture.pending = null; gesture.moving = true;
      gesture.tail = gesture.tail.then(() => this.dispatch(gesture, 'pointerMove', point));
      void gesture.tail.then(() => { gesture.moving = false; this.schedule(gesture); }, (reason) => {
        gesture.moving = false; this.fail(reason, gesture.identity); void this.cancel();
      });
    });
  }
  private async dispatch(gesture: Gesture, action: 'pointerDown' | 'pointerMove' | 'pointerUp', point: Point): Promise<void> {
    if (gesture.cancelled) return;
    const receipt = await this.send(controlRequest(gesture.identity, { action, gestureId: gesture.id, ...point }), gesture.scope);
    if (gesture.cancelled) return;
    if (['failed', 'timedOut', 'blocked', 'cancelled'].includes(receipt.phase)) throw new Error(receipt.reason);
  }
}
