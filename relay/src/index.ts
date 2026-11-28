import { MAX_TOKEN } from "./group";
import { parseAttestation, verifyAttestation } from "./attest";
import { RateLimiter } from "./ratelimit";
import type { Platform, Store } from "./services";

export { Room } from "./room";
export { Group } from "./group";
export { Purchase } from "./purchase";
export { Install } from "./install";

const ROOM_ID = /^[0-9a-f]{64}$/;
const ID = /^[0-9a-f]{32}$/;
const ROOM_PATH = /^\/relay\/v1\/([^/]+)$/;
const MAX_BODY = 64 * 1024;

const limiter = new RateLimiter(1200, 60_000);
// A computer proves itself with a key it made, so a modified client can make as many as it likes;
// what is counted here is how many fresh installs one network may start in an hour.
const installs = new RateLimiter(10, 3_600_000);

const text = (body: string, status: number) => new Response(body, { status });

// 403 is also "wrong group", so the reason travels in a header the client reads.
const unverified = () => new Response("attestation required", { status: 403, headers: { "x-relay-reason": "attestation-required" } });

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

    const g = /^\/relay\/v1\/group\/([^/]+)(?:\/(push|paid|challenge|attest)(?:\/([^/]+))?)?$/.exec(url.pathname);
    if (g) return groupApi(req, env, g[1], g[2], g[3], ip);

    const m = ROOM_PATH.exec(url.pathname);
    if (!m) return text("not found", 404);
    if (!ROOM_ID.test(m[1])) return text("bad room", 400);
    const role = url.searchParams.get("role");
    if (role !== "host" && role !== "guest" && role !== "stream") return text("bad role", 400);
    if (role === "stream" && !ID.test(url.searchParams.get("id") ?? "")) return text("bad stream", 400);
    if (role !== "stream" && !ID.test(url.searchParams.get("group") ?? "")) return text("bad group", 400);
    if (role !== "stream" && !ID.test(url.searchParams.get("device") ?? "")) return text("bad device", 400);
    if (req.headers.get("Upgrade") !== "websocket") return text("websocket only", 426);
    if (role !== "stream" && !(await env.GROUPS.get(env.GROUPS.idFromName(url.searchParams.get("group")!)).isVerified())) return unverified();
    return env.ROOMS.get(env.ROOMS.idFromName(m[1])).fetch(req);
  },
};

async function groupApi(req: Request, env: Cloudflare.Env, group: string, what?: string, device?: string, ip?: string | null): Promise<Response> {
  if (!ID.test(group)) return text("bad group", 400);
  const stub = env.GROUPS.get(env.GROUPS.idFromName(group));

  if (!what) {
    if (req.method !== "GET") return text("method not allowed", 405);
    return Response.json(await stub.status());
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
    if (a.platform === "desktop" && ip && !installs.allow(ip)) return new Response("slow down", { status: 503, headers: { "Retry-After": "3600" } });
    if (!(await stub.takeChallenge(body.challenge))) return text("no such challenge", 400);
    const outcome = await verifyAttestation(env, group, body.challenge, a, !(await stub.isVerified()));
    if (outcome === "not-configured") return text("not configured", 501);
    if (outcome === "rejected") return text("attestation rejected", 422);
    if (outcome === "limit") return text("too many new groups for this install", 429);
    await stub.setVerified();
    return new Response(null, { status: 204 });
  }

  if (what === "push") {
    const { device, platform, token } = body;
    if (typeof device !== "string" || !ID.test(device)) return text("bad device", 400);
    if (platform !== "apns" && platform !== "fcm") return text("bad platform", 400);
    if (typeof token !== "string" || token.length === 0 || token.length > MAX_TOKEN) return text("bad token", 400);
    if (!(await stub.isVerified())) return unverified();
    if (!(await stub.setPush(device, platform as Platform, token))) return text("too many devices", 409);
    return new Response(null, { status: 204 });
  }

  const { store, receipt } = body;
  if (store !== "apple" && store !== "google" && store !== "microsoft") return text("bad store", 400);
  if (typeof receipt !== "string" || receipt.length === 0) return text("bad receipt", 400);
  const result = await stub.buy(store as Store, receipt, group);
  if (result === "not-configured") return text("not configured", 501);
  if (result === "rejected") return text("receipt rejected", 422);
  if (result === "bound") return text("purchase belongs to another group", 409);
  return Response.json(await stub.status());
}

async function readJson(req: Request): Promise<Record<string, unknown> | undefined> {
  const declared = Number(req.headers.get("Content-Length") ?? 0);
  if (declared > MAX_BODY || !req.body) return undefined;
  // The length may be missing or wrong, so the count is on what arrives.
  const reader = req.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > MAX_BODY) {
      await reader.cancel();
      return undefined;
    }
    chunks.push(value);
  }
  const raw = new TextDecoder().decode(await new Blob(chunks as BlobPart[]).arrayBuffer());
  try {
    const v = JSON.parse(raw);
    return v && typeof v === "object" && !Array.isArray(v) ? v : undefined;
  } catch {
    return undefined;
  }
}
