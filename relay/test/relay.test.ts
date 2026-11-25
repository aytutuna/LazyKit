import { env, exports } from "cloudflare:workers";
import { runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { MAX_BUFFERED, MAX_FRAME, MAX_STREAMS, weekOf } from "../src/room";

declare const __BIG__: boolean;

let n = 0;
const newRoom = () => (++n).toString(16).padStart(64, "0");

// The test config turns on web_socket_manual_reply_to_close, so a closed end
// stays CLOSING until it answers; stall makes it never answer.
class Peer {
  frames: ArrayBuffer[] = [];
  texts: string[] = [];
  closed: Promise<CloseEvent>;
  private waiting: (() => void)[] = [];
  received = 0;
  keep = true;
  stall = false;

  constructor(readonly ws: WebSocket) {
    ws.binaryType = "arraybuffer";
    ws.accept();
    ws.addEventListener("message", (e) => {
      if (typeof e.data === "string") this.texts.push(e.data);
      else {
        const buf = e.data as ArrayBuffer;
        this.received += buf.byteLength;
        if (this.keep) this.frames.push(buf);
      }
      this.waiting.splice(0).forEach((f) => f());
    });
    this.closed = new Promise((res) =>
      ws.addEventListener("close", (e) => {
        if (!this.stall) ws.close(e.code === 1005 ? 1000 : e.code);
        res(e);
      }),
    );
  }

  async next(): Promise<ArrayBuffer> {
    while (this.frames.length === 0) await new Promise<void>((r) => this.waiting.push(r));
    return this.frames.shift()!;
  }

  async nextText(): Promise<string> {
    while (this.texts.length === 0) await new Promise<void>((r) => this.waiting.push(r));
    return this.texts.shift()!;
  }

  async until(bytes: number): Promise<void> {
    while (this.received < bytes) await new Promise<void>((r) => this.waiting.push(r));
  }
}

const ask = (room: string, query: string) =>
  exports.default.fetch(`https://alazy.app/relay/v1/${room}?${query}`, { headers: { Upgrade: "websocket" } });

async function open(room: string, query: string): Promise<Peer> {
  const res = await ask(room, query);
  expect(res.status).toBe(101);
  return new Peer(res.webSocket!);
}

// The host side of a room: a control socket that opens a stream for every
// "open <id>" it is told about.
class Host {
  streams: Peer[] = [];
  private waiting: (() => void)[] = [];
  constructor(readonly room: string, readonly control: Peer) {
    control.ws.addEventListener("message", async (e) => {
      if (typeof e.data !== "string" || !e.data.startsWith("open ")) return;
      const id = e.data.slice("open ".length);
      this.streams.push(await open(room, `role=stream&id=${id}`));
      this.waiting.splice(0).forEach((f) => f());
    });
  }

  async stream(): Promise<Peer> {
    while (this.streams.length === 0) await new Promise<void>((r) => this.waiting.push(r));
    return this.streams.shift()!;
  }
}

const host = async (room: string) => new Host(room, await open(room, "role=host"));

// A guest stream and the host end it was matched with.
async function stream(h: Host): Promise<[Peer, Peer]> {
  const guest = await open(h.room, "role=guest");
  return [guest, await h.stream()];
}

const bytes = (...v: number[]) => new Uint8Array(v);
const stub = (room: string) => env.ROOMS.get(env.ROOMS.idFromName(room));
const usage = (room: string) => stub(room).getUsage();
const total = (u: Record<string, number>) => Object.values(u).reduce((a, b) => a + b, 0);
const stored = (room: string) =>
  runInDurableObject(stub(room), (_o, state) => state.storage.get<Record<string, number>>("usage"));

describe("relay", () => {
  it("matches a guest with the host and forwards both ways", async () => {
    const h = await host(newRoom());
    const [guest, side] = await stream(h);
    guest.ws.send(bytes(1, 2, 3));
    expect(new Uint8Array(await side.next())).toEqual(bytes(1, 2, 3));
    side.ws.send(bytes(9, 8));
    expect(new Uint8Array(await guest.next())).toEqual(bytes(9, 8));
    guest.ws.close(1000);
    await side.closed;
  });

  it("keeps frames in order and intact", async () => {
    const h = await host(newRoom());
    const [guest, side] = await stream(h);
    for (let i = 0; i < 50; i++) guest.ws.send(new Uint8Array(1000).fill(i));
    for (let i = 0; i < 50; i++) {
      const f = new Uint8Array(await side.next());
      expect(f.length).toBe(1000);
      expect(f[0]).toBe(i);
      expect(f[999]).toBe(i);
    }
  });

  it("carries several streams at once without mixing them", async () => {
    const h = await host(newRoom());
    const pairs: [Peer, Peer][] = [];
    for (let i = 0; i < 5; i++) pairs.push(await stream(h));
    pairs.forEach(([g, s], i) => {
      g.ws.send(bytes(i));
      s.ws.send(bytes(100 + i));
    });
    for (const [i, [g, s]] of pairs.entries()) {
      expect(new Uint8Array(await s.next())).toEqual(bytes(i));
      expect(new Uint8Array(await g.next())).toEqual(bytes(100 + i));
    }
    // One stream ends while the others carry on, as a finished request does
    // beside a long poll.
    pairs[2][0].ws.close(1000);
    await pairs[2][1].closed;
    pairs[0][0].ws.send(bytes(42));
    expect(new Uint8Array(await pairs[0][1].next())).toEqual(bytes(42));
    const [g, s] = await stream(h);
    g.ws.send(bytes(43));
    expect(new Uint8Array(await s.next())).toEqual(bytes(43));
  });

  it("limits the streams of one room", async () => {
    const h = await host(newRoom());
    for (let i = 0; i < MAX_STREAMS; i++) await stream(h);
    expect((await ask(h.room, "role=guest")).status).toBe(429);
  });

  it("closes a stream from both ends and takes the next one", async () => {
    const h = await host(newRoom());
    const [guest, side] = await stream(h);
    side.ws.close(1000, "done");
    expect((await guest.closed).code).toBe(1000);
    const [guest2, side2] = await stream(h);
    guest2.ws.send(bytes(7));
    expect(new Uint8Array(await side2.next())).toEqual(bytes(7));
  });

  it("is not confused by an end that never answers a close", async () => {
    const h = await host(newRoom());
    const [guest, side] = await stream(h);
    side.stall = true;
    guest.ws.close(1000);
    await side.closed;

    // The host side of the first stream is still CLOSING. A new stream must
    // work, and the late close of the old one must not touch it.
    const [guest2, side2] = await stream(h);
    guest2.ws.send(bytes(1));
    expect(new Uint8Array(await side2.next())).toEqual(bytes(1));
    side.ws.close(1000);
    await new Promise((r) => setTimeout(r, 50));
    side2.ws.send(bytes(2));
    expect(new Uint8Array(await guest2.next())).toEqual(bytes(2));
  });

  it("lets a new host replace one that never answered a close", async () => {
    const room = newRoom();
    const h1 = await host(room);
    h1.control.stall = true;
    const h2 = await host(room);
    expect((await h1.control.closed).code).toBe(1012);

    const [guest, side] = await stream(h2);
    guest.ws.send(bytes(3));
    expect(new Uint8Array(await side.next())).toEqual(bytes(3));
    expect(h1.streams.length).toBe(0);
  });

  it("keeps streams open when the host reconnects", async () => {
    const room = newRoom();
    const h1 = await host(room);
    const [guest, side] = await stream(h1);
    await host(room);
    guest.ws.send(bytes(4));
    expect(new Uint8Array(await side.next())).toEqual(bytes(4));
  });

  it("drops a host that went quiet", async () => {
    const room = newRoom();
    const h = await host(room);
    await runInDurableObject(stub(room), async (o, state) => {
      for (const ws of state.getWebSockets()) ws.serializeAttachment({ seen: 1 });
      await o.alarm();
    });
    expect((await h.control.closed).code).toBe(1001);
    expect((await ask(room, "role=guest")).status).toBe(404);
  });

  it("drops a stream that went quiet and closes its peer", async () => {
    const room = newRoom();
    const h = await host(room);
    const [guest, side] = await stream(h);
    await runInDurableObject(stub(room), async (o, state) => {
      for (const ws of state.getWebSockets("guest")) ws.serializeAttachment({ seen: 1 });
      await o.alarm();
    });
    expect((await guest.closed).code).toBe(1001);
    await side.closed;
  });

  it("answers a ping without a frame reaching the peer", async () => {
    const h = await host(newRoom());
    h.control.ws.send("ping");
    expect(await h.control.nextText()).toBe("pong");
    const [guest, side] = await stream(h);
    guest.ws.send("ping");
    expect(await guest.nextText()).toBe("pong");
    guest.ws.send(bytes(1));
    expect(new Uint8Array(await side.next())).toEqual(bytes(1));
  });

  it("refuses a guest when nobody is waiting", async () => {
    expect((await ask(newRoom(), "role=guest")).status).toBe(404);
  });

  it("refuses a stream nobody asked for", async () => {
    const room = newRoom();
    await host(room);
    expect((await ask(room, `role=stream&id=${"a".repeat(32)}`)).status).toBe(404);
  });

  it("refuses malformed rooms, roles and plain requests", async () => {
    expect((await ask("abc", "role=host")).status).toBe(400);
    expect((await ask(newRoom(), "role=spy")).status).toBe(400);
    expect((await ask(newRoom(), "role=stream&id=zz")).status).toBe(400);
    const plain = await exports.default.fetch(`https://alazy.app/relay/v1/${newRoom()}?role=host`);
    expect(plain.status).toBe(426);
    const other = await exports.default.fetch("https://alazy.app/relay/v2/x");
    expect(other.status).toBe(404);
  });

  it("closes a stream that sends text and its peer", async () => {
    const h = await host(newRoom());
    const [guest, side] = await stream(h);
    guest.ws.send("hello");
    expect((await guest.closed).code).toBe(1003);
    await side.closed;
  });

  it("closes a stream that sends a frame over the limit", async () => {
    const h = await host(newRoom());
    const [guest, side] = await stream(h);
    guest.ws.send(new Uint8Array(MAX_FRAME));
    expect(new Uint8Array(await side.next()).length).toBe(MAX_FRAME);
    guest.ws.send(new Uint8Array(MAX_FRAME + 1));
    expect((await guest.closed).code).toBe(1009);
    await side.closed;
  });

  it("closes a stream whose receiver does not keep up", async () => {
    const h = await host(newRoom());
    const [guest, side] = await stream(h);
    // A local pair never queues, so the host's end claims a full buffer.
    await runInDurableObject(stub(h.room), (_o, state) => {
      const [id] = state.getTags(state.getWebSockets("guest")[0]).slice(1);
      for (const ws of state.getWebSockets(id)) {
        if (state.getTags(ws)[0] === "stream") Object.defineProperty(ws, "bufferedAmount", { value: MAX_BUFFERED + 1 });
      }
    });
    guest.ws.send(bytes(1));
    expect((await guest.closed).code).toBe(1013);
    await side.closed;
  });

  it("counts forwarded bytes per room and week, both directions", async () => {
    const room = newRoom();
    const other = newRoom();
    const h = await host(room);
    const [guest, side] = await stream(h);
    guest.ws.send(new Uint8Array(1500));
    side.ws.send(new Uint8Array(500));
    await side.until(1500);
    await guest.until(500);
    expect((await usage(room))[weekOf(Date.now())]).toBe(2000);
    expect(total(await usage(other))).toBe(0);
  });

  it("saves the count shortly after it changes", async () => {
    const room = newRoom();
    const h = await host(room);
    const [guest, side] = await stream(h);
    guest.ws.send(new Uint8Array(300));
    await side.until(300);
    expect(await stored(room)).toBeUndefined();
    expect(await runDurableObjectAlarm(stub(room))).toBe(true);
    expect((await stored(room))?.[weekOf(Date.now())]).toBe(300);
  });

  it("keeps the count after the stream closes", async () => {
    const room = newRoom();
    const h = await host(room);
    const [guest, side] = await stream(h);
    guest.ws.send(new Uint8Array(300));
    await side.until(300);
    guest.ws.close(1000);
    await side.closed;
    expect(total((await stored(room)) ?? {})).toBe(300);
  });

  it("counts into the week of the bytes and forgets old weeks", async () => {
    const room = newRoom();
    const day = 86_400_000;
    const now = Date.now();
    await runInDurableObject(stub(room), async (o, state) => {
      await o.count(10, now - 100 * day);
      await o.count(20, now - 7 * day);
      await o.count(30, now);
      await o.alarm();
      expect(Object.keys(o.getUsage())).toEqual([weekOf(now - 7 * day), weekOf(now)]);
      expect(await state.storage.get("usage")).toEqual(o.getUsage());
    });
    const u = await usage(room);
    expect(u[weekOf(now)]).toBe(30);
    expect(u[weekOf(now - 7 * day)]).toBe(20);
  });

  it.runIf(__BIG__)("streams 1 GB", async () => {
    const h = await host(newRoom());
    const [guest, side] = await stream(h);
    const frame = new Uint8Array(MAX_FRAME).fill(0xab);
    const goal = 1024 ** 3;
    const t0 = Date.now();
    let sent = 0;
    side.keep = false;
    // No sender window: the relay's own limit has to hold, so the sender
    // yields to the event loop once in a while as a real socket does.
    while (sent < goal && sent !== Infinity) {
      guest.ws.send(frame);
      sent += frame.length;
      if (sent % (MAX_BUFFERED / 2) === 0) await side.until(sent - MAX_BUFFERED / 4);
    }
    await side.until(goal);
    const secs = (Date.now() - t0) / 1000;
    console.log(`1 GB through the relay in ${secs.toFixed(1)} s`);
    expect(total(await usage(h.room))).toBe(goal);
  }, 600_000);
});
