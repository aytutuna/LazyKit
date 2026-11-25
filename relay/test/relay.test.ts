import { env, exports } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { weekOf } from "../src/room";

declare const __BIG__: boolean;

let n = 0;
const newRoom = () => (++n).toString(16).padStart(64, "0");

class Peer {
  frames: ArrayBuffer[] = [];
  closed: Promise<CloseEvent>;
  private waiting: (() => void)[] = [];
  received = 0;
  keep = true;

  constructor(readonly ws: WebSocket) {
    ws.binaryType = "arraybuffer";
    ws.accept();
    ws.addEventListener("message", (e) => {
      const buf = e.data as ArrayBuffer;
      this.received += buf.byteLength;
      if (this.keep) this.frames.push(buf);
      this.waiting.splice(0).forEach((f) => f());
    });
    this.closed = new Promise((res) => ws.addEventListener("close", res as EventListener));
  }

  async next(): Promise<ArrayBuffer> {
    while (this.frames.length === 0) await new Promise<void>((r) => this.waiting.push(r));
    return this.frames.shift()!;
  }

  async until(bytes: number): Promise<void> {
    while (this.received < bytes) await new Promise<void>((r) => this.waiting.push(r));
  }
}

async function connect(room: string, role: string) {
  return exports.default.fetch(`https://alazy.app/relay/v1/${room}?role=${role}`, {
    headers: { Upgrade: "websocket" },
  });
}

async function join(room: string, role: "host" | "guest"): Promise<Peer> {
  const res = await connect(room, role);
  expect(res.status).toBe(101);
  return new Peer(res.webSocket!);
}

const bytes = (...v: number[]) => new Uint8Array(v);
const usage = (room: string) => env.ROOMS.get(env.ROOMS.idFromName(room)).getUsage();
const total = (u: Record<string, number>) => Object.values(u).reduce((a, b) => a + b, 0);

describe("relay", () => {
  it("joins a host and a guest and forwards both ways", async () => {
    const room = newRoom();
    const host = await join(room, "host");
    const guest = await join(room, "guest");

    guest.ws.send(bytes(1, 2, 3));
    expect(new Uint8Array(await host.next())).toEqual(bytes(1, 2, 3));
    host.ws.send(bytes(9, 8));
    expect(new Uint8Array(await guest.next())).toEqual(bytes(9, 8));
    guest.ws.close(1000);
    await host.closed;
  });

  it("keeps frames in order and intact", async () => {
    const room = newRoom();
    const host = await join(room, "host");
    const guest = await join(room, "guest");
    for (let i = 0; i < 50; i++) guest.ws.send(new Uint8Array(1000).fill(i));
    for (let i = 0; i < 50; i++) {
      const f = new Uint8Array(await host.next());
      expect(f.length).toBe(1000);
      expect(f[0]).toBe(i);
      expect(f[999]).toBe(i);
    }
  });

  it("closes the other end when one closes, then takes a new stream", async () => {
    const room = newRoom();
    const host = await join(room, "host");
    const guest = await join(room, "guest");
    host.ws.close(1000, "done");
    const ev = await guest.closed;
    expect(ev.code).toBe(1000);

    const host2 = await join(room, "host");
    const guest2 = await join(room, "guest");
    guest2.ws.send(bytes(7));
    expect(new Uint8Array(await host2.next())).toEqual(bytes(7));
  });

  it("refuses a guest when nobody is waiting", async () => {
    const res = await connect(newRoom(), "guest");
    expect(res.status).toBe(404);
  });

  it("refuses a third device and a second host", async () => {
    const room = newRoom();
    const host = await join(room, "host");
    expect((await connect(room, "host")).status).toBe(409);
    const guest = await join(room, "guest");
    expect((await connect(room, "guest")).status).toBe(409);
    expect((await connect(room, "host")).status).toBe(409);

    guest.ws.send(bytes(5));
    expect(new Uint8Array(await host.next())).toEqual(bytes(5));
  });

  it("refuses malformed rooms, roles and plain requests", async () => {
    expect((await connect("abc", "host")).status).toBe(400);
    expect((await connect(newRoom(), "spy")).status).toBe(400);
    const plain = await exports.default.fetch(`https://alazy.app/relay/v1/${newRoom()}?role=host`);
    expect(plain.status).toBe(426);
    const other = await exports.default.fetch("https://alazy.app/relay/v2/x");
    expect(other.status).toBe(404);
  });

  it("closes a device that sends text", async () => {
    const room = newRoom();
    const host = await join(room, "host");
    host.ws.send("hello");
    expect((await host.closed).code).toBe(1003);
  });

  it("counts forwarded bytes per room and week, both directions", async () => {
    const room = newRoom();
    const other = newRoom();
    const host = await join(room, "host");
    const guest = await join(room, "guest");
    guest.ws.send(new Uint8Array(1500));
    host.ws.send(new Uint8Array(500));
    await host.until(1500);
    await guest.until(500);

    const u = await usage(room);
    expect(u[weekOf(Date.now())]).toBe(2000);
    expect(total(await usage(other))).toBe(0);
  });

  it("keeps the count after the room closes", async () => {
    const room = newRoom();
    const host = await join(room, "host");
    const guest = await join(room, "guest");
    guest.ws.send(new Uint8Array(300));
    await host.until(300);
    guest.ws.close(1000);
    await host.closed;
    expect(total(await usage(room))).toBe(300);
  });

  it.runIf(__BIG__)("streams 1 GB", async () => {
    const room = newRoom();
    const host = await join(room, "host");
    const guest = await join(room, "guest");
    const frame = new Uint8Array(256 * 1024).fill(0xab);
    const goal = 1024 ** 3;
    const window = 32 * 1024 * 1024;
    const t0 = Date.now();
    let sent = 0;
    host.keep = false;
    while (sent < goal) {
      guest.ws.send(frame);
      sent += frame.length;
      if (sent - host.received >= window) await host.until(sent - window / 2);
    }
    await host.until(goal);
    const secs = (Date.now() - t0) / 1000;
    console.log(`1 GB through the relay in ${secs.toFixed(1)} s`);
    expect(total(await usage(room))).toBe(goal);
  }, 600_000);
});
