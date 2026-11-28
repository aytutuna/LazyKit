import { readJson, text } from "../http";
import { RateLimiter } from "../ratelimit";
import { accountName, identityName, type Checked, type Login, type Sent, type Signed } from "./account";
import { sha256 } from "./b64";
import { normalizeEmail } from "./email";
import { IdentityRejectedError, ProviderNotConfiguredError, verifyIdToken, type Provider } from "./oidc";
import { DEFAULT_STAGES, hashPassword, passwordOk } from "./password";

// What an app keeps about an account that deletion and export must reach. The account service
// knows nothing of it beyond these two calls.
export interface AppData {
  erase(env: Cloudflare.Env, account: string): Promise<unknown>;
  export(env: Cloudflare.Env, account: string): Promise<unknown>;
}

const ID = /^[0-9a-f]{32}$/;
const ACCOUNT = /^[0-9a-f]{64}$/;
const SECRET = /^[A-Za-z0-9_-]{43}$/;

const startLimit = new RateLimiter(10, 3_600_000);
const signinLimit = new RateLimiter(30, 600_000);
const confirmLimit = new RateLimiter(20, 600_000);

const json = (body: unknown, status = 200) => Response.json(body, { status });
const fail = (error: string, status: number, headers?: HeadersInit) => Response.json({ error }, { status, headers });

const stages = (env: Cloudflare.Env) => Math.min(12, Math.max(1, Math.floor(Number(env.PASSWORD_STAGES)) || DEFAULT_STAGES));
const accounts = (env: Cloudflare.Env, name: string) => env.ACCOUNTS.get(env.ACCOUNTS.idFromName(name));
const apps = (env: Cloudflare.Env) => (env.ACCOUNT_APPS ?? "").split(",").map((x) => x.trim()).filter(Boolean);

// The signed-in account of a request, or nothing. Other Lazy services call this too.
export async function sessionOf(req: Request, env: Cloudflare.Env): Promise<Checked | undefined> {
  const m = /^Bearer ([0-9a-f]{64})\.([A-Za-z0-9_-]{43})$/.exec(req.headers.get("Authorization") ?? "");
  if (!m) return undefined;
  const found = await accounts(env, m[1]).check(m[2]);
  return found && found.account === m[1] ? found : undefined;
}

function login(env: Cloudflare.Env, body: Record<string, unknown>): Login | undefined {
  const { app, device, label } = body;
  if (typeof app !== "string" || !apps(env).includes(app)) return undefined;
  if (typeof device !== "string" || !ID.test(device)) return undefined;
  if (label !== undefined && (typeof label !== "string" || label.length > 64)) return undefined;
  return { app, device, label: ((label as string | undefined) ?? "").replace(/\p{C}/gu, "") };
}

const sentStatus = (sent: Sent): Response | undefined =>
  sent === "limited" ? fail("too many codes, try again later", 429, { "Retry-After": "600" })
  : sent === "not-configured" ? fail("not configured", 501)
  : sent === "failed" ? fail("the email could not be sent", 502)
  : undefined;

async function signed(env: Cloudflare.Env, r: Signed, name: string): Promise<Response> {
  if (!r.ok) return r.why === "locked" ? fail("too many tries, try again later", 429, { "Retry-After": "900" }) : fail("wrong email, password or code", 401);
  return json({ token: r.token, session: r.session, account: await accounts(env, name).me() });
}

export async function accountApi(req: Request, env: Cloudflare.Env, path: string, ip: string | null, data: Record<string, AppData>): Promise<Response> {
  const route = `${req.method} ${path}`;
  const limited = (l: RateLimiter) => (ip && !l.allow(ip) ? fail("slow down", 503, { "Retry-After": "600" }) : undefined);

  if (req.method === "GET" || req.method === "DELETE") {
    const s = await sessionOf(req, env);
    if (!s) return fail("sign in", 401);
    const acct = accounts(env, s.account);
    if (route === "GET me") return json(await acct.me());
    if (route === "GET sessions") return json(await acct.sessions(s.session));
    if (route === "GET export") {
      const apps: Record<string, unknown> = {};
      for (const [k, d] of Object.entries(data)) apps[k] = await d.export(env, s.account);
      return json({ account: await acct.export(), apps });
    }
    const one = /^sessions\/([A-Za-z0-9_-]{12})$/.exec(path);
    if (req.method === "DELETE" && one) return (await acct.revoke(one[1])) ? new Response(null, { status: 204 }) : fail("no such session", 404);
    if (route === "DELETE me") {
      const body = await readJson(req);
      if (body?.confirm !== "delete") return fail('send {"confirm":"delete"}', 400);
      // Everything that hangs off the account goes first and again-safe; the account itself goes last.
      const identities = await acct.identityIds();
      for (const id of identities) await env.IDENTITIES.get(env.IDENTITIES.idFromName(id)).erase();
      const deleted: Record<string, unknown> = {};
      for (const [k, d] of Object.entries(data)) deleted[k] = await d.erase(env, s.account);
      const own = await acct.erase();
      return json({ deleted: { account: true, ...own, apps: deleted } });
    }
    return fail("not found", 404);
  }
  if (req.method !== "POST") return fail("method not allowed", 405);

  if (route === "POST signout") {
    const s = await sessionOf(req, env);
    if (!s) return fail("sign in", 401);
    await accounts(env, s.account).revoke(s.session);
    return new Response(null, { status: 204 });
  }

  const body = await readJson(req);
  if (!body) return fail("bad body", 400);

  if (route === "POST password") {
    const s = await sessionOf(req, env);
    if (!s) return fail("sign in", 401);
    if (!passwordOk(body.password)) return fail("bad password", 400);
    const r = await accounts(env, s.account).setPassword(await hashPassword(body.password, stages(env)), typeof body.current === "string" ? body.current : undefined, s.fresh, s.session);
    if (r === "locked") return fail("too many tries, try again later", 429, { "Retry-After": "900" });
    return r === "ok" ? new Response(null, { status: 204 }) : fail("current password is wrong", 401);
  }

  if (route === "POST apple" || route === "POST google") {
    const slow = limited(signinLimit);
    if (slow) return slow;
    return social(env, path as Provider, body);
  }

  const email = normalizeEmail(body.email);
  if (!email) return fail("bad email", 400);

  if (route === "POST signup") {
    const slow = limited(startLimit);
    if (slow) return slow;
    if (!passwordOk(body.password)) return fail("bad password", 400);
    // Hashed before the lookup, so an address with an account takes as long as a new one.
    const record = await hashPassword(body.password, stages(env));
    const r = await accounts(env, await accountName(email)).startSignup(email, record);
    return sentStatus(r === "exists" ? "sent" : r) ?? json({ sent: true }, 202);
  }

  if (route === "POST code") {
    const slow = limited(startLimit);
    if (slow) return slow;
    const r = await accounts(env, await accountName(email)).startSignin();
    return sentStatus(r === "none" ? "sent" : r) ?? json({ sent: true }, 202);
  }

  const who = login(env, body);
  if (!who) return fail("bad app or device", 400);
  const name = await accountName(email);

  if (route === "POST signup/confirm" || route === "POST signin/code") {
    const slow = limited(confirmLimit);
    if (slow) return slow;
    const code = typeof body.code === "string" ? body.code.trim() : "";
    const a = accounts(env, name);
    return signed(env, route === "POST signin/code" ? await a.signinWithCode(code, who) : await a.confirmSignup(code, who), name);
  }

  if (route === "POST signin") {
    const slow = limited(signinLimit);
    if (slow) return slow;
    if (typeof body.password !== "string" || body.password.length > 1024) return fail("bad password", 400);
    return signed(env, await accounts(env, name).signinWithPassword(body.password, who), name);
  }

  return fail("not found", 404);
}

// Both providers hand the app an ID token for a nonce the app made. The relay checks it and makes
// the session; the token is good for one sign-in only.
async function social(env: Cloudflare.Env, provider: Provider, body: Record<string, unknown>): Promise<Response> {
  const who = login(env, body);
  if (!who) return fail("bad app or device", 400);
  if (typeof body.idToken !== "string" || typeof body.nonce !== "string") return fail("bad token", 400);
  let ident;
  try {
    ident = await verifyIdToken(provider, body.idToken, body.nonce, provider === "apple" ? env.APPLE_SIGNIN_IDS : env.GOOGLE_CLIENT_IDS);
  } catch (e) {
    if (e instanceof ProviderNotConfiguredError) return fail("not configured", 501);
    if (e instanceof IdentityRejectedError) return fail("token refused", 401);
    throw e;
  }
  const id = await identityName(provider, ident.sub);
  const stub = env.IDENTITIES.get(env.IDENTITIES.idFromName(id));
  if (!(await stub.use(await sha256(body.idToken), ident.exp))) return fail("token refused", 401);

  let name = await stub.account();
  let email = name ? "" : normalizeEmail(ident.email);
  if (!name) {
    if (!email) return fail("the provider gave no verified email", 422);
    name = await accountName(email);
  }
  const r = await accounts(env, name).signinWithIdentity(email || "", { id, provider }, who);
  if (r.ok) await stub.bind(name);
  return signed(env, r, name);
}
