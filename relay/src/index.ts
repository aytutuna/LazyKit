export { Room } from "./room";

const ROOM_ID = /^[0-9a-f]{64}$/;
const STREAM_ID = /^[0-9a-f]{32}$/;

export default {
  async fetch(req: Request, env: Cloudflare.Env): Promise<Response> {
    const url = new URL(req.url);
    const m = /^\/relay\/v1\/([^/]+)$/.exec(url.pathname);
    if (!m) return new Response("not found", { status: 404 });
    if (!ROOM_ID.test(m[1])) return new Response("bad room", { status: 400 });
    const role = url.searchParams.get("role");
    if (role !== "host" && role !== "guest" && role !== "stream") return new Response("bad role", { status: 400 });
    if (role === "stream" && !STREAM_ID.test(url.searchParams.get("id") ?? "")) return new Response("bad stream", { status: 400 });
    if (req.headers.get("Upgrade") !== "websocket") return new Response("websocket only", { status: 426 });
    return env.ROOMS.get(env.ROOMS.idFromName(m[1])).fetch(req);
  },
};
