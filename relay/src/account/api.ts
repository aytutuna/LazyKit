import { readJson } from "../http";
import { RateLimiter } from "../ratelimit";
import { services } from "../services";
import { relayData } from "../relay-data";
import type { Sent } from "./core/account";
import { AccountService, type AccountPort, type Directory, type Entered, type IdentityPort } from "./core/service";
import { DEFAULT_STAGES } from "./core/password";

const startLimit = new RateLimiter(10, 3_600_000);
const signinLimit = new RateLimiter(30, 600_000);
const confirmLimit = new RateLimiter(20, 600_000);

const json = (body: unknown, status = 200) => Response.json(body, { status });
const fail = (error: string, status: number, headers?: HeadersInit) => Response.json({ error }, { status, headers });

const directory = (env: Cloudflare.Env): Directory => ({
  account: (name) => env.ACCOUNTS.get(env.ACCOUNTS.idFromName(name)) as unknown as AccountPort,
  identity: (name) => env.IDENTITIES.get(env.IDENTITIES.idFromName(name)) as unknown as IdentityPort,
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
      stages: Math.min(12, Math.max(1, Math.floor(Number(env.PASSWORD_STAGES)) || DEFAULT_STAGES)),
    },
    { relay: relayData(env) },
  );

const sentStatus = (sent: Sent | "bad-email" | "bad-password"): Response =>
  sent === "sent" ? json({ sent: true }, 202)
  : sent === "limited" ? fail("too many codes, try again later", 429, { "Retry-After": "600" })
  : sent === "not-configured" ? fail("not configured", 501)
  : sent === "failed" ? fail("the email could not be sent", 502)
  : fail(sent, 400);

const entered = (r: Entered): Response => {
  if (r.ok) return json({ token: r.token, session: r.session, account: r.account });
  switch (r.why) {
    case "locked": return fail("too many tries, try again later", 429, { "Retry-After": "900" });
    case "bad-email": case "bad-password": case "bad-input": return fail(r.why, 400);
    case "not-configured": return fail("not configured", 501);
    case "no-email": return fail("the provider gave no verified email", 422);
    case "refused": return fail("token refused", 401);
    default: return fail("wrong email, password or code", 401);
  }
};

export async function accountApi(req: Request, env: Cloudflare.Env, path: string, ip: string | null): Promise<Response> {
  const svc = accountService(env);
  const route = `${req.method} ${path}`;
  const limited = (l: RateLimiter) => (ip && !l.allow(ip) ? fail("slow down", 503, { "Retry-After": "600" }) : undefined);

  if (req.method === "GET" || req.method === "DELETE") {
    const s = await svc.session(req.headers.get("Authorization"));
    if (!s) return fail("sign in", 401);
    if (route === "GET me") return json(await svc.me(s));
    if (route === "GET sessions") return json(await svc.sessions(s));
    if (route === "GET export") return json(await svc.export(s));
    const one = /^sessions\/([A-Za-z0-9_-]{12})$/.exec(path);
    if (req.method === "DELETE" && one) return (await svc.revoke(s, one[1])) ? new Response(null, { status: 204 }) : fail("no such session", 404);
    if (route === "DELETE me") {
      const body = await readJson(req);
      if (body?.confirm !== "delete") return fail('send {"confirm":"delete"}', 400);
      return json({ deleted: await svc.delete(s) });
    }
    return fail("not found", 404);
  }
  if (req.method !== "POST") return fail("method not allowed", 405);

  if (route === "POST signout") {
    const s = await svc.session(req.headers.get("Authorization"));
    if (!s) return fail("sign in", 401);
    await svc.signout(s);
    return new Response(null, { status: 204 });
  }

  const body = await readJson(req);
  if (!body) return fail("bad body", 400);

  if (route === "POST password") {
    const s = await svc.session(req.headers.get("Authorization"));
    if (!s) return fail("sign in", 401);
    const r = await svc.setPassword(s, body.password, body.current);
    if (r === "bad-password") return fail("bad password", 400);
    if (r === "locked") return fail("too many tries, try again later", 429, { "Retry-After": "900" });
    return r === "ok" ? new Response(null, { status: 204 }) : fail("current password is wrong", 401);
  }

  if (route === "POST signup" || route === "POST code") {
    const slow = limited(startLimit);
    if (slow) return slow;
    return sentStatus(route === "POST signup" ? await svc.signup(body.email, body.password) : await svc.code(body.email));
  }

  const slow = limited(route === "POST signup/confirm" || route === "POST signin/code" ? confirmLimit : signinLimit);
  const known = ["POST signup/confirm", "POST signin/code", "POST signin", "POST apple", "POST google"];
  if (!known.includes(route)) return fail("not found", 404);
  if (slow) return slow;
  const who = svc.login(body);
  if (!who) return fail("bad app or device", 400);
  if (route === "POST apple" || route === "POST google") return entered(await svc.social(path as "apple" | "google", body.idToken, body.nonce, who));
  if (route === "POST signin") return entered(await svc.signinPassword(body.email, body.password, who));
  return entered(await (route === "POST signin/code" ? svc.signinCode : svc.confirm).call(svc, body.email, body.code, who));
}
