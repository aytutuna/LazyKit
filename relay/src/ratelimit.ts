// Counts requests per key in a fixed window, in memory only. Keys are client
// addresses, so nothing here is ever written to storage or logged; a restart
// of the isolate forgets them, which is fine for an abuse limit.
const MAX_KEYS = 10_000;

export class RateLimiter {
  private seen = new Map<string, { start: number; count: number }>();
  private swept = 0;

  constructor(
    private limit: number,
    private windowMs: number,
  ) {}

  allow(address: string, now = Date.now()): boolean {
    const key = networkOf(address);
    if (this.seen.size >= MAX_KEYS && now - this.swept >= this.windowMs) this.sweep(now);
    let e = this.seen.get(key);
    if (!e) {
      // Full of live entries: drop the oldest instead of refusing, so a flood
      // of distinct sources cannot lock new users out.
      if (this.seen.size >= MAX_KEYS) this.seen.delete(this.seen.keys().next().value!);
      e = { start: now, count: 0 };
      this.seen.set(key, e);
    } else if (now - e.start >= this.windowMs) {
      e.start = now;
      e.count = 0;
    }
    return ++e.count <= this.limit;
  }

  private sweep(now: number): void {
    this.swept = now;
    for (const [k, e] of this.seen) if (now - e.start >= this.windowMs) this.seen.delete(k);
  }
}

// One IPv6 customer holds a whole /64, so that is the unit counted.
export function networkOf(address: string): string {
  if (!address.includes(":") || address.includes(".")) return address;
  const [head, tail] = address.split("::");
  const front = head ? head.split(":") : [];
  const back = tail ? tail.split(":") : [];
  const groups = tail === undefined ? front : [...front, ...Array(Math.max(0, 8 - front.length - back.length)).fill("0"), ...back];
  return groups
    .slice(0, 4)
    .map((g) => g.toLowerCase().replace(/^0+(?=.)/, ""))
    .join(":");
}
