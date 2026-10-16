import type { NativeControlReceipt } from '../ipc/model.ts';
import type { UiBridge } from '../ipc/client.ts';

const key = (receipt: NativeControlReceipt) => JSON.stringify([receipt.operationId, receipt.identity.workspaceId,
  receipt.identity.sessionId, receipt.identity.buildId, receipt.identity.generation, receipt.identity.width, receipt.identity.height]);

/** ACKs can precede the submit reply. Retain a bounded cache before installing a waiter. */
export class ControlReceiptInbox {
  private cache = new Map<string, NativeControlReceipt>();
  private waiters = new Map<string, (receipt: NativeControlReceipt | null) => void>();
  private connecting: Promise<void> | null = null;
  private off: (() => void) | null = null;
  private epoch = 0;
  private subscribe?: UiBridge['observeControlReceipts'];
  constructor(subscribe?: UiBridge['observeControlReceipts']) { this.subscribe = subscribe; }
  get connected() { return this.off !== null; }
  get size() { return this.cache.size; }

  connect(): Promise<void> {
    if (this.connecting) return this.connecting;
    if (!this.subscribe) return Promise.resolve();
    const epoch = this.epoch;
    const registration = this.subscribe((receipt) => { if (this.epoch === epoch) this.push(receipt); }).then((off) => {
      if (this.epoch !== epoch) off(); else this.off = off;
    }).catch(() => { /* Scoped state reads remain the bounded compatibility fallback. */ });
    let timer: ReturnType<typeof setTimeout>;
    this.connecting = Promise.race([registration, new Promise<void>((resolve) => {
      timer = setTimeout(resolve, 100);
    })]).finally(() => clearTimeout(timer));
    return this.connecting;
  }
  private push(receipt: NativeControlReceipt) {
    const id = key(receipt), previous = this.cache.get(id);
    if (previous && previous.observedAt > receipt.observedAt) return;
    this.cache.delete(id); this.cache.set(id, receipt);
    while (this.cache.size > 64) this.cache.delete(this.cache.keys().next().value!);
    this.waiters.get(id)?.(receipt);
  }
  latest(receipt: NativeControlReceipt) {
    const cached = this.cache.get(key(receipt));
    return cached && cached.observedAt >= receipt.observedAt ? cached : receipt;
  }
  /** A timeout returns to a state read; it never invents an ACK or fails the operation. */
  wait(receipt: NativeControlReceipt, milliseconds: number): Promise<NativeControlReceipt | null> {
    const cached = this.latest(receipt);
    if (cached !== receipt) return Promise.resolve(cached);
    const id = key(receipt);
    if (this.waiters.size >= 32 || this.waiters.has(id)) return Promise.resolve(null);
    return new Promise((resolve) => {
      const finish = (value: NativeControlReceipt | null) => {
        clearTimeout(timer); this.waiters.delete(id); resolve(value);
      };
      const timer = setTimeout(() => finish(null), milliseconds);
      this.waiters.set(id, finish);
    });
  }
  clear() {
    this.cache.clear();
    for (const finish of this.waiters.values()) finish(null);
    this.waiters.clear();
  }
  disconnect() {
    this.epoch++; this.off?.(); this.off = null; this.connecting = null; this.clear();
  }
}
