import { DurableObject } from "cloudflare:workers";

export const MAX_FRAME = 64 * 1024;
export const MAX_BUFFERED = 8 * 1024 * 1024;
export const MAX_STREAMS = 16;
const KEEP_WEEKS = 8;
const FLUSH_MS = 1000;
const SWEEP_MS = 30_000;
const CONTROL_IDLE_MS = 90_000;
const STREAM_IDLE_MS = 120_000;
const JOIN_MS = 10_000;

// Monday 00:00 UTC of the week containing t, as YYYY-MM-DD.
export function weekOf(t: number): string {
  const d = new Date(t);
  const back = (d.getUTCDay() + 6) % 7;
  d.setUTCDate(d.getUTCDate() - back);
  return d.toISOString().slice(0, 10);
}

// A room has one control socket, the host waiting for guests, and any number
// of streams. A guest that connects gets a stream id sent to the host on the
// control socket; the host opens its end of the stream under that id and the
// room forwards binary frames between the two ends, one stream per TCP
// connection of the phone link.
export class Room extends DurableObject {
  private usage: Record<string, number> = {};
  private unsaved = false;
  private joining = new Map<string, (ok: boolean) => void>();

  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
    ctx.blockConcurrencyWhile(async () => {
      this.usage = (await ctx.storage.get<Record<string, number>>("usage")) ?? {};
    });
  }

  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const role = url.searchParams.get("role");
    if (role === "host") return this.acceptHost();
    if (role === "guest") return this.acceptGuest();
    return this.acceptStream(url.searchParams.get("id") ?? "");
  }

  private control(): WebSocket | undefined {
    return this.ctx.getWebSockets("host").find((s) => s.readyState === WebSocket.OPEN);
  }

  // A new host takes over from the old one, which may be a connection that
  // died without a close.
  private async acceptHost(): Promise<Response> {
    for (const old of this.ctx.getWebSockets("host")) old.close(1012, "replaced");
    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1], ["host"]);
    pair[1].serializeAttachment({ seen: Date.now() });
    await this.schedule(SWEEP_MS);
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  private async acceptGuest(): Promise<Response> {
    const control = this.control();
    if (!control) return new Response("nobody waiting", { status: 404 });
    const open = this.ctx.getWebSockets("guest").filter((s) => s.readyState === WebSocket.OPEN).length;
    if (open + this.joining.size >= MAX_STREAMS) return new Response("too many streams", { status: 429 });

    const id = crypto.randomUUID().replaceAll("-", "");
    let timer: ReturnType<typeof setTimeout> | undefined;
    const joined = new Promise<boolean>((res) => {
      this.joining.set(id, res);
      timer = setTimeout(() => res(false), JOIN_MS);
    });
    control.send(`open ${id}`);
    const ok = await joined;
    clearTimeout(timer);
    this.joining.delete(id);
    if (!ok) return new Response("host did not answer", { status: 504 });

    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1], ["guest", id]);
    pair[1].serializeAttachment({ seen: Date.now() });
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  private async acceptStream(id: string): Promise<Response> {
    const join = this.joining.get(id);
    if (!join) return new Response("no such stream", { status: 404 });
    this.joining.delete(id);
    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1], ["stream", id]);
    pair[1].serializeAttachment({ seen: Date.now() });
    join(true);
    await this.schedule(SWEEP_MS);
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  async webSocketMessage(ws: WebSocket, data: string | ArrayBuffer): Promise<void> {
    const [role] = this.ctx.getTags(ws);
    if (typeof data === "string" || role === "host") return this.end(ws, 1003, "binary frames on streams only");
    if (data.byteLength > MAX_FRAME) return this.end(ws, 1009, "frame too large");
    const peer = this.peerOf(ws);
    if (!peer) return this.end(ws, 1008, "no peer");
    ws.serializeAttachment({ seen: Date.now() });
    peer.send(data);
    if (peer.bufferedAmount > MAX_BUFFERED) return this.end(ws, 1013, "receiver too slow");
    await this.count(data.byteLength);
  }

  async webSocketClose(ws: WebSocket, code: number): Promise<void> {
    const out = code === 1005 || code === 1006 ? 1000 : code;
    this.peerOf(ws)?.close(out, "peer closed");
    try {
      ws.close(out);
    } catch {}
    await this.flush();
  }

  async webSocketError(ws: WebSocket): Promise<void> {
    this.peerOf(ws)?.close(1011, "peer failed");
    await this.flush();
  }

  async alarm(): Promise<void> {
    await this.flush();
    const now = Date.now();
    for (const ws of this.ctx.getWebSockets()) {
      const [role] = this.ctx.getTags(ws);
      const seen = Math.max(
        ws.deserializeAttachment()?.seen ?? 0,
        this.ctx.getWebSocketAutoResponseTimestamp(ws)?.getTime() ?? 0,
      );
      const limit = role === "host" ? CONTROL_IDLE_MS : STREAM_IDLE_MS;
      if (now - seen > limit) this.end(ws, 1001, "idle");
    }
    if (this.ctx.getWebSockets().length > 0) await this.schedule(SWEEP_MS);
  }

  getUsage(): Record<string, number> {
    return { ...this.usage };
  }

  // The only place that decides what a byte is counted against; the room is
  // the finest unit, so a group or device key can be added here later.
  async count(bytes: number, now = Date.now()): Promise<void> {
    const week = weekOf(now);
    this.usage[week] = (this.usage[week] ?? 0) + bytes;
    if (!this.unsaved) {
      this.unsaved = true;
      await this.schedule(FLUSH_MS);
    }
  }

  // Closes a stream from both ends. A socket that has been closed stays in
  // the room's list until its far end answers, so lookups skip closing ones.
  private end(ws: WebSocket, code: number, reason: string): void {
    const peer = this.peerOf(ws);
    ws.close(code, reason);
    peer?.close(code === 1009 || code === 1003 ? 1011 : code, reason);
  }

  private peerOf(ws: WebSocket): WebSocket | undefined {
    const [role, id] = this.ctx.getTags(ws);
    if (role === "host" || !id) return undefined;
    return this.ctx.getWebSockets(id).find((s) => s !== ws && s.readyState === WebSocket.OPEN);
  }

  private async schedule(ms: number): Promise<void> {
    const at = Date.now() + ms;
    const cur = await this.ctx.storage.getAlarm();
    if (cur === null || cur > at) await this.ctx.storage.setAlarm(at);
  }

  private async flush(): Promise<void> {
    this.unsaved = false;
    const oldest = weekOf(Date.now() - KEEP_WEEKS * 7 * 86_400_000);
    for (const week of Object.keys(this.usage)) if (week < oldest) delete this.usage[week];
    await this.ctx.storage.put("usage", this.usage);
  }
}
