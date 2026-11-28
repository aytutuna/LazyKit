// PBKDF2-SHA256 through WebCrypto. Argon2id would need a wasm build that Workers cannot
// compile at run time, and Workers has capped one PBKDF2 call at 100,000 rounds, so the
// work is chained: each stage derives from the last one's output, 100,000 rounds apiece.
const ROUNDS = 100_000;
export const DEFAULT_STAGES = 6;
const MAX_STAGES = 12;

export interface PasswordRecord {
  v: 1;
  stages: number;
  salt: string;
  hash: string;
}

const b64 = (b: Uint8Array) => btoa(String.fromCharCode(...b));
const unb64 = (s: string) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));

// Same text on every platform whatever the keyboard produced.
const passwordBytes = (p: string) => new TextEncoder().encode(p.normalize("NFKC"));

async function derive(password: string, salt: Uint8Array, stages: number): Promise<Uint8Array> {
  let input: Uint8Array = passwordBytes(password);
  for (let i = 0; i < stages; i++) {
    const key = await crypto.subtle.importKey("raw", input as Uint8Array<ArrayBuffer>, "PBKDF2", false, ["deriveBits"]);
    const s = new Uint8Array([...salt, i]);
    input = new Uint8Array(await crypto.subtle.deriveBits({ name: "PBKDF2", hash: "SHA-256", salt: s, iterations: ROUNDS }, key, 256));
  }
  return input;
}

export async function hashPassword(password: string, stages = DEFAULT_STAGES): Promise<PasswordRecord> {
  const salt = crypto.getRandomValues(new Uint8Array(16));
  return { v: 1, stages, salt: b64(salt), hash: b64(await derive(password, salt, stages)) };
}

export async function verifyPassword(password: string, rec: PasswordRecord): Promise<boolean> {
  if (rec.v !== 1 || rec.stages < 1 || rec.stages > MAX_STAGES) return false;
  return equal(await derive(password, unb64(rec.salt), rec.stages), unb64(rec.hash));
}

// Spent on an address with no account, so the answer takes as long as for one with an account.
export async function burnPasswordTime(stages = DEFAULT_STAGES): Promise<void> {
  await derive("x", new Uint8Array(16), stages);
}

export function equal(a: Uint8Array, b: Uint8Array): boolean {
  let diff = a.length ^ b.length;
  for (let i = 0; i < Math.max(a.length, b.length); i++) diff |= (a[i] ?? 0) ^ (b[i] ?? 0);
  return diff === 0;
}

export const MIN_PASSWORD = 10;
export const MAX_PASSWORD = 256;

export function passwordOk(p: unknown): p is string {
  return typeof p === "string" && p.length >= MIN_PASSWORD && p.length <= MAX_PASSWORD;
}
