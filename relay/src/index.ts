import { accountApi, accountService } from "./account/api";
import { MAX_TOKEN } from "./group";
import { parseAttestation, verifyAttestation } from "./attest";
import { readJson, text } from "./http";
import { RateLimiter } from "./ratelimit";
import type { Platform, Store } from "./services";

export { Room } from "./room";
export { Group } from "./group";
export { Quota } from "./quota";
export { Purchase } from "./purchase";
export { Account, Address, Budget, Spent } from "./account/account";
export { Identity } from "./account/identity";

const sessionOf = (req: Request, env: Cloudflare.Env) => accountService(env).session(req.headers.get("Authorization"));

const ROOM_ID = /^[0-9a-f]{64}$/;
const ID = /^[0-9a-f]{32}$/;
const ROOM_PATH = /^\/relay\/v1\/([^/]+)$/;
const ACCOUNT_PATH = /^\/account\/v1\/(.+)$/;

const limiter = new RateLimiter(1200, 60_000);

// 403 is also "wrong group", so the reason travels in a header the client reads.
const refused = (message: string, reason: string, status = 403) => new Response(message, { status, headers: { "x-relay-reason": reason } });
const unverified = () => refused("attestation required", "attestation-required");
const signIn = () => refused("sign in", "sign-in", 401);

export default {
  async fetch(req: Request, env: Cloudflare.Env): Promise<Response> {
    const url = new URL(req.url);
    const ip = req.headers.get("CF-Connecting-IP");
    // The host's end of a stream is gated by a random id that must be pending, and it follows
    // every guest connect, so counting it too would halve what a guest can open. Only a room's
    // path gets this exemption; everything else is counted whatever its query says.
    const exempt = ROOM_PATH.test(url.pathname) && url.searchParams.get("role") === "stream";
    // Not 429, which a room uses for too many streams.
    if (ip && !exempt && !limiter.allow(ip)) {
      return new Response("slow down", { status: 503, headers: { "Retry-After": "60" } });
    }

    const a = ACCOUNT_PATH.exec(url.pathname);
    if (a) return accountApi(req, env, a[1], ip);

    const g = /^\/relay\/v1\/group\/([^/]+)(?:\/(push|paid|challenge|attest)(?:\/([^/]+))?)?$/.exec(url.pathname);
    if (g) return groupApi(req, env, g[1], g[2], g[3]);

    const m = ROOM_PATH.exec(url.pathname);
    if (!m) return text("not found", 404);
    if (!ROOM_ID.test(m[1])) return text("bad room", 400);
    const role = url.searchParams.get("role");
    if (role !== "host" && role !== "guest" && role !== "stream") return text("bad role", 400);
    if (role === "stream" && !ID.test(url.searchParams.get("id") ?? "")) return text("bad stream", 400);
    const group = url.searchParams.get("group") ?? "";
    const device = url.searchParams.get("device") ?? "";
    if (role !== "stream" && !ID.test(group)) return text("bad group", 400);
    if (role !== "stream" && !ID.test(device)) return text("bad device", 400);
    if (req.headers.get("Upgrade") !== "websocket") return text("websocket only", 426);
    const forwarded = new Request(req);
    forwarded.headers.delete("x-relay-account");
    forwarded.headers.delete("x-relay-session");
    if (role !== "stream") {
      const s = await sessionOf(req, env);
      if (!s) return signIn();
      const stub = env.GROUPS.get(env.GROUPS.idFromName(group));
      if ((await stub.owner()) !== s.account) return refused("not your group", "wrong-account");
      // A session is made for one device, so a stolen token does not speak for another.
      if (s.device !== device) return refused("not this device's session", "wrong-device");
      if ((await stub.admit()) !== "ok") return unverified();
      forwarded.headers.set("x-relay-account", s.account);
      forwarded.headers.set("x-relay-session", s.session);
    }
    return env.ROOMS.get(env.ROOMS.idFromName(m[1])).fetch(forwarded);
  },
};

async function groupApi(req: Request, env: Cloudflare.Env, group: string, what?: string, device?: string): Promise<Response> {
  if (!ID.test(group)) return text("bad group", 400);
  const s = await sessionOf(req, env);
  if (!s) return signIn();
  const stub = env.GROUPS.get(env.GROUPS.idFromName(group));
  const quota = env.QUOTAS.get(env.QUOTAS.idFromName(s.account));

  if (!what && req.method === "PUT") {
    const body = await readJson(req);
    if (body?.platform !== "apple" && body?.platform !== "google" && body?.platform !== "desktop") return text("bad platform", 400);
    const r = await stub.claim(group, s.account, body.platform);
    if (r === "other") return refused("group belongs to another account", "wrong-account");
    if (r === "limit") return text("too many groups", 409);
    return Response.json(await quota.status());
  }

  if ((await stub.owner()) !== s.account) return refused("not your group", "wrong-account");

  if (!what) {
    if (req.method !== "GET") return text("method not allowed", 405);
    return Response.json(await quota.status());
  }

  if (what === "push" && req.method === "DELETE" && device) {
    if (!ID.test(device)) return text("bad device", 400);
    await stub.dropPush(device);
    return new Response(null, { status: 204 });
  }

  if (req.method !== "POST" || device) return text("method not allowed", 405);
  if (what === "challenge") return Response.json({ challenge: await stub.newChallenge() });
  const body = await readJson(req);
  if (!body) return text("bad body", 400);

  if (what === "attest") {
    const a = parseAttestation(body);
    if (!a || typeof body.challenge !== "string") return text("bad attestation", 400);
    if (!(await stub.takeChallenge(body.challenge))) return text("no such challenge", 400);
    const outcome = await verifyAttestation(env, body.challenge, a);
    if (outcome === "not-configured") return text("not configured", 501);
    if (outcome === "rejected") return text("attestation rejected", 422);
    await stub.setVerified();
    return new Response(null, { status: 204 });
  }

  if (what === "push") {
    const { platform, token } = body;
    // A session speaks for its own device only.
    if (body.device !== s.device) return refused("not this device's session", "wrong-device");
    if (platform !== "apns" && platform !== "fcm") return text("bad platform", 400);
    if (typeof token !== "string" || token.length === 0 || token.length > MAX_TOKEN) return text("bad token", 400);
    if ((await stub.admit()) !== "ok") return unverified();
    if (!(await stub.setPush(s.device, platform as Platform, token))) return text("too many devices", 409);
    return new Response(null, { status: 204 });
  }

  const { store, receipt } = body;
  if (store !== "apple" && store !== "google" && store !== "microsoft") return text("bad store", 400);
  if (typeof receipt !== "string" || receipt.length === 0) return text("bad receipt", 400);
  const result = await quota.buy(store as Store, receipt, s.account);
  if (result === "not-configured") return text("not configured", 501);
  if (result === "rejected") return text("receipt rejected", 422);
  if (result === "bound") return text("purchase belongs to another account", 409);
  return Response.json(await quota.status());
}
