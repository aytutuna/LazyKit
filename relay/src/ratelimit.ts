// Counts requests per key in a fixed window, in memory only. Keys are client
// addresses, so nothing here is ever written to storage or logged; a restart
// of the isolate forgets them, which is fine for an abuse limit.
export class RateLimiter {
  private seen = new Map<string, { start: number; count: number }>();

  constructor(
    private limit: number,
    private windowMs: number,
  ) {}

  allow(key: string, now = Date.now()): boolean {
    if (this.seen.size > 10_000) this.sweep(now);
    let e = this.seen.get(key);
    if (!e || now - e.start >= this.windowMs) {
      e = { start: now, count: 0 };
      this.seen.set(key, e);
    }
    return ++e.count <= this.limit;
  }

  private sweep(now: number): void {
    for (const [k, e] of this.seen) if (now - e.start >= this.windowMs) this.seen.delete(k);
  }
}
