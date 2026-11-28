export const MINUTE = 60_000;
export const DAY = 86_400_000;

export const bytes = (s: string) => new TextEncoder().encode(s);

export async function hmac(key: string, text: string): Promise<string> {
  const k = await crypto.subtle.importKey("raw", bytes(key), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return Array.from(new Uint8Array(await crypto.subtle.sign("HMAC", k, bytes(text))), (b) => b.toString(16).padStart(2, "0")).join("");
}

export const randomId = () => Array.from(crypto.getRandomValues(new Uint8Array(16)), (b) => b.toString(16).padStart(2, "0")).join("");

export function randomDigits(): string {
  for (;;) {
    const n = crypto.getRandomValues(new Uint32Array(1))[0];
    if (n < 4_294_000_000) return String(n % 1_000_000).padStart(6, "0");
  }
}

export interface Window {
  count: number;
  since: number;
}

export const emptyWindow = (): Window => ({ count: 0, since: 0 });

function roll(w: Window, now: number, ms: number): Window {
  if (now - w.since >= ms) {
    w.count = 0;
    w.since = now;
  }
  return w;
}

export const windowFull = (w: Window, now: number, ms: number, limit: number) => roll(w, now, ms).count >= limit;

export function windowAdd(w: Window, now: number, ms: number): void {
  roll(w, now, ms).count++;
}

export const windowIdle = (w: Window, now: number, ms: number) => w.count === 0 || now - w.since >= ms;

// Wrong tries by each caller, a caller being the network a request came from. Kept in the memory of
// the object that counts and never written down, so no address is ever stored; a restart forgets them.
export class CallerCounts {
  private by = new Map<string, Window>();

  constructor(
    private ms: number,
    private max = 200,
  ) {}

  full(caller: string, now: number, limit: number): boolean {
    const w = this.by.get(caller);
    return w !== undefined && windowFull(w, now, this.ms, limit);
  }

  add(caller: string, now: number): void {
    let w = this.by.get(caller);
    if (!w) {
      if (this.by.size >= this.max) this.prune(now);
      if (this.by.size >= this.max) this.by.delete(this.by.keys().next().value!);
      w = emptyWindow();
      this.by.set(caller, w);
    }
    windowAdd(w, now, this.ms);
  }

  forgive(caller: string): void {
    this.by.delete(caller);
  }

  prune(now: number): void {
    for (const [k, w] of this.by) if (now - w.since >= this.ms) this.by.delete(k);
  }
}
