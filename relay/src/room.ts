import { DurableObject } from "cloudflare:workers";

export const MAX_FRAME = 64 * 1024;
export const MAX_BUFFERED = 8 * 1024 * 1024;
export const MAX_STREAMS = 16;
// Application close code for the weekly cap; 1008 stays for protocol faults.
export const CLOSE_CAPPED = 4402;
const FLUSH_MS = 1000;
const SWEEP_MS = 30_000;
const CONTROL_IDLE_MS = 90_000;
const STREAM_IDLE_MS = 120_000;
const JOIN_MS = 10_000;
// The client pings every 30 s, so a control socket quiet for longer than this is not being read.
const HOST_SILENT_MS = 45_000;
// A room nobody has connected to for this long forgets its group.
const ROOM_KEEP_MS = 90 * 86_400_000;

// Kept per socket across hibernation. owed counts the frames forwarded to this end that it has
// not acknowledged yet; eof is set once this end has half-closed.
interface Mark {
  seen?: number;
  owed?: number;
  eof?: boolean;
}

// Close codes a peer may be sent; workerd's close() refuses the rest (1004, 1005, 1006, 1015).
const sendable = (code: number) => (code >= 1000 && code <= 1003) || (code >= 1007 && code <= 1014) || (code >= 3000 && code <= 4999);

// A room has one control socket, the host waiting for guests, and any number
// of streams. A guest that connects gets a stream id sent to the host on the
// control socket; the host opens its end of the stream under that id and the
// room forwards binary frames between the two ends, one stream per TCP
// connection of the phone link.
export class Room extends DurableObject {
  private group?: string;
  private host?: string;
  private stored = false;
  private idleSince?: number;
  private pending = 0;
  private unsaved = false;
  private joining = new Map<string, (ok: boolean) => void>();

  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
    ctx.blockConcurrencyWhile(async () => {
      this.group = await ctx.storage.get<string>("group");
      this.host = await ctx.storage.get<string>("host");
      this.stored = this.group !== undefined;
      this.idleSince = await ctx.storage.get<number>("idleSince");
    });
  }

  async fetch(req: Request): Promise<Response> {
    const url = new URL(req.url);
    const role = url.searchParams.get("role");
    if (role === "stream") return this.acceptStream(url.searchParams.get("id") ?? "");
    // A room belongs to the first group that joins it; a request that finds nobody to join
    // leaves nothing behind.
    const group = url.searchParams.get("group") ?? "";
    if ((this.group ?? group) !== group) return new Response("wrong group", { status: 403 });
    const device = url.searchParams.get("device") ?? "";
    if (role === "host") return this.acceptHost(group, device);
    return this.acceptGuest(group, url.pathname.split("/").pop() ?? "", device);
  }

  // Called once a host or guest has really joined.
  private async keep(group: string): Promise<void> {
    this.group ??= group;
    if (!this.stored) {
      this.stored = true;
      await this.ctx.storage.put("group", this.group);
    }
    if (this.idleSince !== undefined) {
      this.idleSince = undefined;
      await this.ctx.storage.delete("idleSince");
    }
  }

  private lastSeen(ws: WebSocket): number {
    return Math.max(this.marks(ws).seen ?? 0, this.ctx.getWebSocketAutoResponseTimestamp(ws)?.getTime() ?? 0);
  }

  private marks(ws: WebSocket): Mark {
    return ws.deserializeAttachment() ?? {};
  }

  private mark(ws: WebSocket, patch: Mark): void {
    ws.serializeAttachment({ ...this.marks(ws), ...patch });
  }

  private groupStub(group: string) {
    return this.env.GROUPS.get(this.env.GROUPS.idFromName(group));
  }

  private control(): WebSocket | undefined {
    return this.ctx.getWebSockets("host").find((s) => s.readyState === WebSocket.OPEN);
  }

  // A new host takes over from the old one, which may be a connection that
  // died without a close.
  private async acceptHost(group: string, device: string): Promise<Response> {
    await this.keep(group);
    await this.groupStub(group).touch(device);
    // The last host is the device a push should wake when a guest finds nobody here.
    if (this.host !== device) {
      this.host = device;
      await this.ctx.storage.put("host", device);
    }
    for (const old of this.ctx.getWebSockets("host")) old.close(1012, "replaced");
    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1], ["host"]);
    this.mark(pair[1], { seen: Date.now() });
    await this.schedule(SWEEP_MS);
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  private async acceptGuest(groupId: string, room: string, device: string): Promise<Response> {
    const group = this.groupStub(groupId);
    if ((await group.status()).capped) return new Response("weekly cap reached", { status: 402 });
    await group.touch(device);
    const control = this.control();
    if (!control) return this.nobodyWaiting(groupId, room, device);
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
    if (!ok) {
      // A host that has pinged lately is only slow, and only this guest
      // loses. One that has been silent for longer is a suspended or killed
      // app: treat it like no host so the push goes out.
      if (Date.now() - this.lastSeen(control) < HOST_SILENT_MS) return new Response("host did not answer", { status: 504 });
      control.close(1001, "not answering");
      return this.nobodyWaiting(groupId, room, device);
    }

    await this.keep(groupId);
    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1], ["guest", id]);
    this.mark(pair[1], { seen: Date.now() });
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  // The group's other devices may be closed phone apps; they are woken to
  // connect and the guest tries again.
  private async nobodyWaiting(group: string, room: string, device: string): Promise<Response> {
    const woken = await this.groupStub(group).wake(room, device, this.host);
    return new Response("nobody waiting", { status: 404, headers: woken ? { "x-relay-woken": "1" } : {} });
  }

  private async acceptStream(id: string): Promise<Response> {
    const join = this.joining.get(id);
    if (!join) return new Response("no such stream", { status: 404 });
    this.joining.delete(id);
    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1], ["stream", id]);
    this.mark(pair[1], { seen: Date.now() });
    join(true);
    await this.schedule(SWEEP_MS);
    return new Response(null, { status: 101, webSocket: pair[0] });
  }

  async webSocketMessage(ws: WebSocket, data: string | ArrayBuffer): Promise<void> {
    const [role] = this.ctx.getTags(ws);
    // "eof" is a stream's half-close: the writer is done, the reader is not.
    // "ack" returns one frame of credit to the sender. An end may ack only frames it was sent
    // and half-close once, so neither can be used to make the room queue without limit.
    if ((data === "eof" || data === "ack") && role !== "host") {
      const peer = this.peerOf(ws);
      if (!peer) return;
      const mine = this.marks(ws);
      if (data === "ack") {
        if (!mine.owed) return this.end(ws, 1008, "ack without a frame");
        this.mark(ws, { owed: mine.owed - 1 });
      } else {
        if (mine.eof) return this.end(ws, 1008, "second eof");
        this.mark(ws, { eof: true });
      }
      peer.send(data);
      if (peer.bufferedAmount > MAX_BUFFERED) return this.end(ws, 1013, "receiver too slow");
      return;
    }
    if (typeof data === "string" || role === "host") return this.end(ws, 1003, "binary frames on streams only");
    if (data.byteLength > MAX_FRAME) return this.end(ws, 1009, "frame too large");
    const peer = this.peerOf(ws);
    if (!peer) return this.end(ws, 1008, "no peer");
    this.mark(ws, { seen: Date.now() });
    peer.send(data);
    this.mark(peer, { owed: (this.marks(peer).owed ?? 0) + 1 });
    await this.count(data.byteLength);
    if (peer.bufferedAmount > MAX_BUFFERED) return this.end(ws, 1013, "receiver too slow");
  }

  async webSocketClose(ws: WebSocket, code: number): Promise<void> {
    // 1005 is a close without a code; 1006 is a connection that died, which the other end
    // must not take for a finished transfer.
    const out = code === 1005 ? 1000 : sendable(code) ? code : 1011;
    try {
      this.peerOf(ws)?.close(out, out === 1011 ? "peer lost" : "peer closed");
    } catch {}
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
      const seen = this.lastSeen(ws);
      const limit = role === "host" ? CONTROL_IDLE_MS : STREAM_IDLE_MS;
      if (now - seen > limit) this.end(ws, 1001, "idle");
    }
    if (this.ctx.getWebSockets().length > 0) return this.schedule(SWEEP_MS);
    if (this.group === undefined) return;
    this.idleSince ??= now;
    await this.ctx.storage.put("idleSince", this.idleSince);
    if (now - this.idleSince < ROOM_KEEP_MS) return this.ctx.storage.setAlarm(this.idleSince + ROOM_KEEP_MS);
    await this.ctx.storage.deleteAll();
    this.group = undefined;
    this.host = undefined;
    this.stored = false;
    this.idleSince = undefined;
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
      if (await this.groupStub(this.group).record(bytes)) return;
    } catch (e) {
      this.pending += bytes;
      throw e;
    }
    for (const ws of this.ctx.getWebSockets("guest")) this.end(ws, CLOSE_CAPPED, "weekly cap reached");
  }
}
