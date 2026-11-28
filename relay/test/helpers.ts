import { env, exports } from "cloudflare:workers";
import { beforeEach } from "vitest";
import { accountName } from "../src/account/core/account";
import type { Mail } from "../src/account/core/email";
import { useServices, type Services } from "../src/services";
import { jwtKit } from "./core/jwt";

export const sent: Mail[] = [];
export const codeIn = (m: Mail) => /\b(\d{6})\b/.exec(m.text)![1];

const kit = await jwtKit();

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

export const NONCE = "n".repeat(24);
export const APPLE_AUD = "com.lazyscan.lazyscan";

export async function idToken(provider: "apple" | "google", claims: Record<string, unknown> = {}, opts: { kid?: string; wrongKey?: boolean; alg?: string } = {}): Promise<string> {
  const now = Math.floor(Date.now() / 1000);
  const base =
    provider === "apple"
      ? { iss: "https://appleid.apple.com", aud: APPLE_AUD, sub: who.apple, email: who.email, email_verified: "true" }
      : { iss: "https://accounts.google.com", aud: "test-google-client", sub: who.google, email: who.email, email_verified: true };
  const payload = { ...base, nonce: NONCE, iat: now, exp: now + 600, ...claims };
  return kit.sign(payload, opts);
}

export const account = (path: string, method = "GET", body?: unknown, token?: string, headers: Record<string, string> = {}) =>
  exports.default.fetch(`https://alazy.app/account/v1/${path}`, {
    method,
    body: body === undefined ? undefined : JSON.stringify(body),
    headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), ...headers },
  });

export const DEVICE_A = "a".repeat(32);

// Signs up through the API with the fake mailbox and returns the session token.
export async function signUp(email: string, password = "correct horse battery", app = "lazysend", device = DEVICE_A): Promise<string> {
  const before = sent.length;
  const r = await account("signup", "POST", { email, password });
  if (r.status !== 202) throw new Error(`signup ${r.status}`);
  const code = codeIn(sent[before]);
  const c = await account("signup/confirm", "POST", { email, code, app, device });
  if (c.status !== 200) throw new Error(`confirm ${c.status}`);
  return ((await c.json()) as { token: string }).token;
}

const cache = new Map<string, string>();
beforeEach(() => cache.clear());

// A signed-in account for a test that is about the relay: made directly, without mail or hashing.
export async function sessionFor(email: string, device: string, app = "lazysend"): Promise<string> {
  const key = `${email}|${app}|${device}`;
  const kept = cache.get(key);
  if (kept) return kept;
  const stub = env.ACCOUNTS.get(env.ACCOUNTS.idFromName(await accountName(email)));
  const r = await stub.signinWithIdentity(email, { id: `test-${email}`, provider: "apple" }, { app, device, label: "" });
  if (!r.ok) throw new Error("no session");
  cache.set(key, r.token);
  return r.token;
}

// A signed-in account that owns this group, one account per group.
export async function groupSession(group: string, device: string, platform: "apple" | "google" | "desktop" = "desktop") {
  const email = `${group}@test.local`;
  const token = await sessionFor(email, device);
  const name = await accountName(email);
  const r = await env.GROUPS.get(env.GROUPS.idFromName(group)).claim(group, name, platform);
  if (r !== "ok") throw new Error(`claim ${r}`);
  return { token, account: name, headers: { Authorization: `Bearer ${token}` } };
}
