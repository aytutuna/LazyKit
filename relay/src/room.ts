import { DurableObject } from "cloudflare:workers";

export const MAX_FRAME = 64 * 1024;
export const MAX_BUFFERED = 8 * 1024 * 1024;
export const MAX_STREAMS = 16;
const FLUSH_MS = 1000;
const SWEEP_MS = 30_000;
const CONTROL_IDLE_MS = 90_000;
const STREAM_IDLE_MS = 120_000;
const JOIN_MS = 10_000;

// A room has one control socket, the host waiting for guests, and any number
// of streams. A guest that connects gets a stream id sent to the host on the
// control socket; the host opens its end of the stream under that id and the
// room forwards binary frames between the two ends, one stream per TCP
// connection of the phone link.
export class Room extends DurableObject {
  private group?: string;
  private hostDevice?: string;
  private pending = 0;
  private unsaved = false;
  private joining = new Map<string, (ok: boolean) => void>();

  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
    ctx.blockConcurrencyWhile(async () => {
      this.group = await ctx.storage.get<string>("group");
      this.hostDevice = await ctx.storage.get<string>("hostDevice");
    });
  }

  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const role = url.searchParams.get("role");
    if (role === "stream") return this.acceptStream(url.searchParams.get("id") ?? "");
    // A room belongs to the first group that uses it.
    const group = url.searchParams.get("group") ?? "";
    if (this.group === undefined) {
      this.group = group;
      await this.ctx.storage.put("group", group);
    }
    if (this.group !== group) return new Response("wrong group", { status: 403 });
    if (role === "host") return this.acceptHost(url.searchParams.get("device") ?? "");
    return this.acceptGuest(url.pathname.split("/").pop() ?? "");
  }

  private groupStub() {
    return this.env.GROUPS.get(this.env.GROUPS.idFromName(this.group!));
  }

  private control(): WebSocket | undefined {
    return this.ctx.getWebSockets("host").find((s) => s.readyState === WebSocket.OPEN);
  }

  // A new host takes over from the old one, which may be a connection that
  // died without a close.
  private async acceptHost(device: string): Promise<Response> {
    this.hostDevice = device;
    await this.ctx.storage.put("hostDevice", device);
    for (const old of this.ctx.getWebSockets("host")) old.close(1012, "replaced");
    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1], ["host"]);
    pair[1].serializeAttachment({ seen: Date.now() });
    await this.schedule(SWEEP_MS);
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  private async acceptGuest(room: string): Promise<Response> {
    const group = this.groupStub();
    if ((await group.status()).capped) return new Response("weekly cap reached", { status: 402 });
    const control = this.control();
    if (!control) {
      // A closed phone app is woken to connect; the guest tries again.
      const woken = this.hostDevice ? await group.wake(this.hostDevice, room) : false;
      return new Response("nobody waiting", { status: 404, headers: woken ? { "x-relay-woken": "1" } : {} });
    }
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

  // Bytes are handed to the group in batches, so a busy stream costs one
  // call a second.
  async count(bytes: number): Promise<void> {
    this.pending += bytes;
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
    const bytes = this.pending;
    this.pending = 0;
    this.unsaved = false;
    if (bytes === 0 || this.group === undefined) return;
    try {
      if (await this.groupStub().record(bytes)) return;
    } catch (e) {
      this.pending += bytes;
      throw e;
    }
    for (const ws of this.ctx.getWebSockets("guest")) this.end(ws, 1008, "weekly cap reached");
  }
}
