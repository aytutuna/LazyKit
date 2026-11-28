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

// Wrong tries by one caller and by everyone, so a stranger can shut out his own address but not
// the owner's (the caller is the network a request came from; the total is a higher ceiling).
export interface Fails {
  total: Window;
  by: Record<string, Window>;
}

export const noFails = (): Fails => ({ total: { count: 0, since: 0 }, by: {} });
const MAX_CALLERS = 50;

function roll(w: Window, now: number, ms: number): Window {
  if (now - w.since >= ms) {
    w.count = 0;
    w.since = now;
  }
  return w;
}

export function failsFull(f: Fails, caller: string, now: number, ms: number, perCaller: number, total: number): boolean {
  const mine = f.by[caller] ? roll(f.by[caller], now, ms) : undefined;
  return roll(f.total, now, ms).count >= total || (mine?.count ?? 0) >= perCaller;
}

export function failsAdd(f: Fails, caller: string, now: number, ms: number): void {
  roll(f.total, now, ms).count++;
  if (!f.by[caller]) {
    const keys = Object.keys(f.by);
    if (keys.length >= MAX_CALLERS) {
      const stale = keys.filter((k) => now - f.by[k].since >= ms);
      for (const k of stale.length > 0 ? stale : [keys[0]]) delete f.by[k];
    }
    f.by[caller] = { count: 0, since: now };
  }
  roll(f.by[caller], now, ms).count++;
}

export function failsForgive(f: Fails, caller: string): void {
  if (f.by[caller]) f.by[caller].count = 0;
  f.total.count = Math.max(0, f.total.count - 1);
}

export const failsIdle = (f: Fails, now: number, ms: number) => now - f.total.since >= ms && Object.values(f.by).every((w) => now - w.since >= ms);
