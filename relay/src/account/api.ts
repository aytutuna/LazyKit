import { readJson } from "../http";
import { RateLimiter, networkOf } from "../ratelimit";
import { services } from "../services";
import { relayData } from "../relay-data";
import type { Sent } from "./core/address";
import { AccountService, type AccountPort, type AddressPort, type Directory, type Entered, type IdentityPort, type OncePort } from "./core/service";
import { parseAttestation, verifyAttestation } from "../attest";
import { NotConfiguredError } from "../services";
import { stagesOf } from "./deps";
import { devMode, devTurnstile } from "./dev";
import { cloudflareTurnstile } from "./turnstile";

const startLimit = new RateLimiter(10, 3_600_000);
const signinLimit = new RateLimiter(30, 600_000);
const confirmLimit = new RateLimiter(20, 600_000);
const nonceLimit = new RateLimiter(60, 600_000);

const json = (body: unknown, status = 200) => Response.json(body, { status });
const fail = (error: string, status: number, headers?: HeadersInit) => Response.json({ error }, { status, headers });

const directory = (env: Cloudflare.Env): Directory => ({
  address: (name) => env.ADDRESSES.get(env.ADDRESSES.idFromName(name)) as unknown as AddressPort,
  account: (id) => env.ACCOUNTS.get(env.ACCOUNTS.idFromName(id)) as unknown as AccountPort,
  identity: (name) => env.IDENTITIES.get(env.IDENTITIES.idFromName(name)) as unknown as IdentityPort,
  once: (name) => env.SPENT.get(env.SPENT.idFromName(name)) as unknown as OncePort,
});

// The account service on this Worker's Durable Objects. Other Lazy services in the Worker call
// `session` to learn who a request is from.
export const accountService = (env: Cloudflare.Env) =>
  new AccountService(
    directory(env),
    {
      apps: (env.ACCOUNT_APPS ?? "").split(",").map((x) => x.trim()).filter(Boolean),
      appleAudiences: env.APPLE_SIGNIN_IDS,
      googleAudiences: env.GOOGLE_CLIENT_IDS,
      keys: services().keys,
      idKey: env.ID_KEY,
      nonceKey: env.ACCOUNT_KEY,
      stages: stagesOf(env),
    },
    { relay: relayData(env) },
  );

const sentStatus = (sent: Sent | "bad-email"): Response =>
  sent === "sent" ? json({ sent: true }, 202)
  : sent === "limited" ? fail("too many codes, try again later", 429, { "Retry-After": "600" })
  : sent === "busy" ? fail("mail is busy, try again later", 503, { "Retry-After": "3600" })
  : sent === "not-configured" ? fail("not configured", 501)
  : fail(sent, 400);

const entered = (r: Entered): Response => {
  if (r.ok) return json({ token: r.token, session: r.session, account: r.account });
  switch (r.why) {
    case "locked": return fail("too many tries, try again later", 429, { "Retry-After": "900" });
    case "limited": return fail("too many codes, try again later", 429, { "Retry-After": "600" });
    case "busy": return fail("mail is busy, try again later", 503, { "Retry-After": "3600" });
    case "bad-email": case "bad-password": case "bad-input": return fail(r.why, 400);
    case "not-configured": return fail("not configured", 501);
    case "no-email": return fail("the provider gave no verified email", 422);
    case "unavailable": return fail("the sign-in provider could not be reached", 503, { "Retry-After": "30" });
    case "need-fresh": return fail("sign in again first", 403);
    case "refused": return fail("token refused", 401);
    case "link-code": return json({ linkRequired: true }, 202);
    default: return fail("wrong email, password or code", 401);
  }
};

const reauth = (r: "ok" | "bad" | "locked" | "need-fresh") =>
  r === "ok" ? new Response(null, { status: 204 })
  : r === "locked" ? fail("too many tries, try again later", 429, { "Retry-After": "900" })
  : r === "need-fresh" ? fail("sign in again first", 403)
  : fail("current password is wrong", 401);

// Requests that make the service send mail come from a person's app or browser: a Turnstile token,
// or an App Attest or Play Integrity proof made for a nonce this server issued.
async function botCheck(env: Cloudflare.Env, svc: AccountService, body: Record<string, unknown>, ip: string | null, action: string): Promise<Response | undefined> {
  try {
    const proof = body.attest;
    if (proof && typeof proof === "object") {
      const p = proof as Record<string, unknown>;
      const parsed = parseAttestation(p);
      if (!parsed || !(await svc.nonceValid(p.nonce))) return fail("bot check failed", 403);
      const out = await verifyAttestation(env, p.nonce as string, parsed);
      if (out === "not-configured") return fail("not configured", 501);
      // The nonce is spent only by a proof that checks out, so a bad one does not use up a good one.
      return out === "ok" && (await svc.spendNonce(p.nonce)) ? undefined : fail("bot check failed", 403);
    }
    if (typeof body.turnstile !== "string" || body.turnstile === "") return fail("bot check needed", 403);
    return (await (services().turnstile ?? (devMode(env) ? devTurnstile : cloudflareTurnstile(env))).verify(body.turnstile, ip, action)) ? undefined : fail("bot check failed", 403);
  } catch (e) {
    if (e instanceof NotConfiguredError) return fail("not configured", 501);
    throw e;
  }
}

// The widget action for a route: Turnstile takes letters, digits, underscore and hyphen only.
const actionOf = (route: string) => route.slice(5).replaceAll("/", "-");

const FORM = ["POST signup", "POST code", "POST nonce", "POST signup/confirm", "POST signin/code", "POST signin", "POST apple", "POST google", "POST link/confirm"];

export async function accountApi(req: Request, env: Cloudflare.Env, path: string, ip: string | null): Promise<Response> {
  const svc = accountService(env);
  const route = `${req.method} ${path}`;
  // A request that carries no address is one shared caller, not an exemption.
  const key = ip ?? "unknown";
  const caller = ip ? networkOf(ip) : "unknown";
  const limited = (l: RateLimiter) => (!devMode(env) && !l.allow(key) ? fail("slow down", 503, { "Retry-After": "600" }) : undefined);
  const header = req.headers.get("Authorization");

  if (req.method === "GET" || req.method === "DELETE") {
    const one = /^sessions\/([A-Za-z0-9_-]{12})$/.exec(path);
    if (!["GET me", "GET sessions", "GET export", "DELETE me"].includes(route) && !(req.method === "DELETE" && one)) return fail("not found", 404);
    const s = await svc.session(header);
    if (!s) return fail("sign in", 401);
    if (route === "GET me") return json(await svc.me(s));
    if (route === "GET sessions") return json(await svc.sessions(s));
    if (route === "GET export") return json(await svc.export(s));
    if (one) return (await svc.revoke(s, one[1])) ? new Response(null, { status: 204 }) : fail("no such session", 404);
    const body = await readJson(req);
    if (body?.confirm !== "delete") return fail('send {"confirm":"delete"}', 400);
    const r = await svc.delete(s, body.password);
    return r.ok ? json({ deleted: r.deleted }) : reauth(r.why);
  }
  if (req.method !== "POST") return fail("method not allowed", 405);

  const withSession = ["POST signout", "POST password", "POST password/code", "POST email", "POST email/confirm"];
  if (!FORM.includes(route) && !withSession.includes(route)) return fail("not found", 404);

  if (route === "POST signout") {
    const s = await svc.session(header);
    if (!s) return fail("sign in", 401);
    await svc.signout(s);
    return new Response(null, { status: 204 });
  }

  const body = (await readJson(req)) ?? undefined;
  if (!body) return fail("bad body", 400);

  if (withSession.includes(route)) {
    const s = await svc.session(header);
    if (!s) return fail("sign in", 401);
    if (route === "POST email") {
      const slow = limited(startLimit);
      if (slow) return slow;
      const bot = await botCheck(env, svc, body, ip, actionOf(route));
      if (bot) return bot;
      const r = await svc.changeEmail(s, body.email, body.password);
      return r === "need-fresh" ? fail("sign in again first", 403) : r === "bad" ? fail("wrong password", 401) : r === "locked" ? fail("too many tries, try again later", 429, { "Retry-After": "900" }) : sentStatus(r);
    }
    if (route === "POST email/confirm") {
      const slow = limited(confirmLimit);
      if (slow) return slow;
      const r = await svc.confirmEmail(s, body.email, body.code, caller, body.oldCode);
      return r === "ok" ? new Response(null, { status: 204 }) : r === "need-fresh" ? fail("sign in again first", 403) : r === "mail-failed" ? fail("the notice could not be sent, try again", 503, { "Retry-After": "60" }) : fail(r === "bad" ? "wrong code" : r, r === "bad" ? 401 : 400);
    }
    if (route === "POST password/code") {
      const slow = limited(startLimit);
      if (slow) return slow;
      const bot = await botCheck(env, svc, body, ip, actionOf(route));
      if (bot) return bot;
      const r = await svc.passwordCode(s);
      return r === "bad" ? fail("the account already has a password", 400) : sentStatus(r);
    }
    const r = await svc.setPassword(s, body.password, body.current, body.code, caller);
    return r === "bad-password" ? fail("bad password", 400) : reauth(r);
  }

  if (route === "POST nonce") {
    const slow = limited(nonceLimit);
    if (slow) return slow;
    const nonce = await svc.nonce();
    return nonce ? json({ nonce }) : fail("not configured", 501);
  }

  if (route === "POST signup" || route === "POST code") {
    const slow = limited(startLimit);
    if (slow) return slow;
    const bot = await botCheck(env, svc, body, ip, actionOf(route));
    if (bot) return bot;
    return sentStatus(route === "POST signup" ? await svc.signup(body.email) : await svc.code(body.email));
  }

  const slow = limited(route === "POST signin" || route === "POST apple" || route === "POST google" ? signinLimit : confirmLimit);
  if (slow) return slow;
  const who = svc.login(body);
  if (!who) return fail("bad app or device", 400);
  switch (route) {
    case "POST apple": case "POST google": return entered(await svc.social(path as "apple" | "google", body.idToken, body.nonce, who, caller));
    case "POST signin": return entered(await svc.signinPassword(body.email, body.password, who, caller));
    case "POST signin/code": return entered(await svc.signinCode(body.email, body.code, who, caller));
    case "POST link/confirm": return entered(await svc.confirmLink(body.email, body.code, who, caller));
    default: return entered(await svc.confirm(body.email, body.code, body.password, who, caller));
  }
}
