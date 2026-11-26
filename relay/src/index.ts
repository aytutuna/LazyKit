import { MAX_TOKEN } from "./group";
import { RateLimiter } from "./ratelimit";
import type { Platform, Store } from "./services";

export { Room } from "./room";
export { Group } from "./group";
export { Purchase } from "./purchase";

const ROOM_ID = /^[0-9a-f]{64}$/;
const ID = /^[0-9a-f]{32}$/;
const MAX_BODY = 64 * 1024;

const limiter = new RateLimiter(120, 60_000);

const text = (body: string, status: number) => new Response(body, { status });

export default {
  async fetch(req: Request, env: Cloudflare.Env): Promise<Response> {
    const ip = req.headers.get("CF-Connecting-IP");
    // Not 429, which a room uses for too many streams.
    if (ip && !limiter.allow(ip)) return new Response("slow down", { status: 503, headers: { "Retry-After": "60" } });

    const url = new URL(req.url);
    const g = /^\/relay\/v1\/group\/([^/]+)(?:\/(push|paid)(?:\/([^/]+))?)?$/.exec(url.pathname);
    if (g) return groupApi(req, env, g[1], g[2], g[3]);

    const m = /^\/relay\/v1\/([^/]+)$/.exec(url.pathname);
    if (!m) return text("not found", 404);
    if (!ROOM_ID.test(m[1])) return text("bad room", 400);
    const role = url.searchParams.get("role");
    if (role !== "host" && role !== "guest" && role !== "stream") return text("bad role", 400);
    if (role === "stream" && !ID.test(url.searchParams.get("id") ?? "")) return text("bad stream", 400);
    if (role !== "stream" && !ID.test(url.searchParams.get("group") ?? "")) return text("bad group", 400);
    if (role !== "stream" && !ID.test(url.searchParams.get("device") ?? "")) return text("bad device", 400);
    if (req.headers.get("Upgrade") !== "websocket") return text("websocket only", 426);
    return env.ROOMS.get(env.ROOMS.idFromName(m[1])).fetch(req);
  },
};

async function groupApi(req: Request, env: Cloudflare.Env, group: string, what?: string, device?: string): Promise<Response> {
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
  const body = await readJson(req);
  if (!body) return text("bad body", 400);

  if (what === "push") {
    const { device, platform, token } = body;
    if (typeof device !== "string" || !ID.test(device)) return text("bad device", 400);
    if (platform !== "apns" && platform !== "fcm") return text("bad platform", 400);
    if (typeof token !== "string" || token.length === 0 || token.length > MAX_TOKEN) return text("bad token", 400);
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
