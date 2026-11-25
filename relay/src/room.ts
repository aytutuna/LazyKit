import { DurableObject } from "cloudflare:workers";

const FLUSH_BYTES = 1 << 20;

// Monday 00:00 UTC of the week containing t, as YYYY-MM-DD.
export function weekOf(t: number): string {
  const d = new Date(t);
  const back = (d.getUTCDay() + 6) % 7;
  d.setUTCDate(d.getUTCDate() - back);
  return d.toISOString().slice(0, 10);
}

type Role = "host" | "guest";

// One room holds at most one waiting host and one guest. Both ends carry the
// phone link's TLS stream as opaque binary frames; either end closing closes
// the other, and the host connects again for the next stream.
export class Room extends DurableObject {
  private usage: Record<string, number> = {};
  private unsaved = 0;

  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    ctx.blockConcurrencyWhile(async () => {
      this.usage = (await ctx.storage.get<Record<string, number>>("usage")) ?? {};
    });
  }

  async fetch(req: Request): Promise<Response> {
    const role = new URL(req.url).searchParams.get("role") as Role;
    const host = this.ctx.getWebSockets("host")[0];
    const guest = this.ctx.getWebSockets("guest")[0];
    if (role === "host" && host) return new Response("room taken", { status: 409 });
    if (role === "guest" && !host) return new Response("nobody waiting", { status: 404 });
    if (role === "guest" && guest) return new Response("room taken", { status: 409 });

    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1], [role]);
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  async webSocketMessage(ws: WebSocket, data: string | ArrayBuffer): Promise<void> {
    if (typeof data === "string") {
      ws.close(1003, "binary only");
      return;
    }
    const peer = this.peerOf(ws);
    if (!peer) {
      ws.close(1008, "no peer");
      return;
    }
    peer.send(data);
    await this.record(data.byteLength);
  }

  async webSocketClose(ws: WebSocket, code: number): Promise<void> {
    this.peerOf(ws)?.close(code === 1005 || code === 1006 ? 1000 : code, "peer closed");
    await this.flush();
  }

  async webSocketError(ws: WebSocket): Promise<void> {
    this.peerOf(ws)?.close(1011, "peer failed");
    await this.flush();
  }

  getUsage(): Record<string, number> {
    return { ...this.usage };
  }

  private peerOf(ws: WebSocket): WebSocket | undefined {
    const [role] = this.ctx.getTags(ws);
    return this.ctx.getWebSockets(role === "host" ? "guest" : "host")[0];
  }

  // The only place that decides what a byte is counted against; the room is
  // the finest unit, so a group or device key can be added here later.
  private async record(bytes: number): Promise<void> {
    const week = weekOf(Date.now());
    this.usage[week] = (this.usage[week] ?? 0) + bytes;
    this.unsaved += bytes;
    if (this.unsaved >= FLUSH_BYTES) await this.flush();
  }

  private async flush(): Promise<void> {
    if (this.unsaved === 0) return;
    this.unsaved = 0;
    await this.ctx.storage.put("usage", this.usage);
  }
}
