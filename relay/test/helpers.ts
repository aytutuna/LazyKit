import { env, exports } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { beforeEach } from "vitest";
import type { Mail } from "../src/account/core/email";
import { useServices, type Services } from "../src/services";
import { jwtKit } from "./core/jwt";

export const sent: Mail[] = [];
export const codeIn = (m: Mail) => /\b(\d{6})\b/.exec(m.text)![1];

const kit = await jwtKit();
const sha = async (t: string) => Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(t))), (b) => b.toString(16).padStart(2, "0")).join("");

// A new person for every test, since Durable Object storage outlives a test.
let people = 0;
export const who = { email: "", other: "", apple: "", google: "" };

// Mail goes to `sent` and the identity providers publish one key, k1.
export function fakes(extra: Partial<Services> = {}): void {
  sent.length = 0;
  people++;
  Object.assign(who, { email: `ada${people}@example.com`, other: `grace${people}@example.com`, apple: `apple-sub-${people}`, google: `google-sub-${people}` });
  useServices({
    mailer: { send: async (m) => void sent.push(m) },
    keys: kit.keys,
    ...extra,
  });
}

// Mail is sent after the answer, so a test waits for it.
export async function mailed(count: number): Promise<void> {
  for (let i = 0; i < 100 && sent.length < count; i++) await new Promise((r) => setTimeout(r, 10));
  if (sent.length < count) throw new Error(`only ${sent.length} of ${count} mails arrived`);
}

export const APPLE_AUD = "com.lazyscan.lazyscan";

export const account = (path: string, method = "GET", body?: unknown, token?: string, headers: Record<string, string> = {}) =>
  exports.default.fetch(`https://alazy.app/account/v1/${path}`, {
    method,
    body: body === undefined ? undefined : JSON.stringify(body),
    headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), ...headers },
  });

export const newNonce = async () => ((await (await account("nonce", "POST", {})).json()) as { nonce: string }).nonce;

// A provider token for a nonce this server issued (Apple carries its SHA-256, Google the nonce itself).
export async function idToken(provider: "apple" | "google", claims: Record<string, unknown> = {}, opts: { kid?: string; wrongKey?: boolean; alg?: string; nonce?: string } = {}) {
  const raw = opts.nonce ?? (await newNonce());
  const now = Math.floor(Date.now() / 1000);
  const base =
    provider === "apple"
      ? { iss: "https://appleid.apple.com", aud: APPLE_AUD, sub: who.apple, email: who.email, email_verified: "true", nonce: await sha(raw) }
      : { iss: "https://accounts.google.com", aud: "test-google-client", sub: who.google, email: who.email, email_verified: true, nonce: raw };
  return { raw, idToken: await kit.sign({ ...base, iat: now, exp: now + 600, ...claims }, opts) };
}

export const DEVICE_A = "a".repeat(32);

// Signs up through the API with the fake mailbox and returns the session token.
export async function signUp(email: string, password = "correct horse battery", app = "lazysend", device = DEVICE_A): Promise<string> {
  const before = sent.length;
  const r = await account("signup", "POST", { email });
  if (r.status !== 202) throw new Error(`signup ${r.status}`);
  await mailed(before + 1);
  const c = await account("signup/confirm", "POST", { email, code: codeIn(sent[before]), password, app, device });
  if (c.status !== 200) throw new Error(`confirm ${c.status}`);
  return ((await c.json()) as { token: string }).token;
}

const cache = new Map<string, string>();
const ids = new Map<string, string>();
beforeEach(() => {
  cache.clear();
  ids.clear();
});

export const accountIdOf = (email: string) => ids.get(email)!;

// A signed-in account for a test that is about the relay: made directly, without mail or hashing.
export async function sessionFor(email: string, device: string, app = "lazysend"): Promise<string> {
  const key = `${email}|${app}|${device}`;
  const kept = cache.get(key);
  if (kept) return kept;
  let id = ids.get(email);
  if (!id) {
    id = Array.from(crypto.getRandomValues(new Uint8Array(16)), (b) => b.toString(16).padStart(2, "0")).join("");
    ids.set(email, id);
  }
  const stub = env.ACCOUNTS.get(env.ACCOUNTS.idFromName(id));
  await stub.create(id, email);
  const r = await stub.open({ app, device, label: "" });
  if (!r.ok) throw new Error("no session");
  cache.set(key, r.token);
  return r.token;
}

// A signed-in account that owns this group, one account per group.
export async function groupSession(group: string, device: string, platform: "apple" | "google" | "desktop" = "desktop") {
  const email = `${group}@test.local`;
  const token = await sessionFor(email, device);
  const id = accountIdOf(email);
  const r = await env.GROUPS.get(env.GROUPS.idFromName(group)).claim(group, id, platform);
  if (r !== "ok") throw new Error(`claim ${r}`);
  return { token, account: id, headers: { Authorization: `Bearer ${token}` } };
}

// runInDurableObject with the object left untyped, for tests that reach into an adapter's private fields.
export const inside: (stub: unknown, f: (o: any, state: DurableObjectState) => Promise<unknown>) => Promise<unknown> = runInDurableObject as any;
