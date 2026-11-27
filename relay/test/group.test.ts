import { env, exports } from "cloudflare:workers";
import { runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";
import { MAX_DEVICES, WEEKLY_CAP } from "../src/group";
import { RateLimiter, networkOf } from "../src/ratelimit";
import { purchaseKey } from "../src/purchase";
import { ReceiptRejectedError, TokenGoneError, useServices } from "../src/services";

let n = 0;
const newGroup = () => (++n).toString(16).padStart(32, "e");
const device = (i = 1) => i.toString(16).padStart(32, "0");
const stub = (group: string) => env.GROUPS.get(env.GROUPS.idFromName(group));

const call = (group: string, path: string, init?: RequestInit) =>
  exports.default.fetch(`https://alazy.app/relay/v1/group/${group}${path}`, init);
const post = (group: string, path: string, body: unknown) =>
  call(group, path, { method: "POST", body: JSON.stringify(body) });

describe("group", () => {
  afterEach(() => useServices());

  it("starts empty and under the cap", async () => {
    const res = await call(newGroup(), "");
    expect(await res.json()).toEqual({
      week: expect.any(String),
      used: 0,
      cap: WEEKLY_CAP,
      paidUntil: 0,
      paid: false,
      capped: false,
    });
  });

  it("is capped from the byte the week's allowance is used up", async () => {
    const g = stub(newGroup());
    expect(await g.record(WEEKLY_CAP - 1)).toBe(true);
    expect(await g.record(1)).toBe(false);
    expect((await g.status()).capped).toBe(true);
  });

  it("starts a fresh allowance every week", async () => {
    const g = stub(newGroup());
    const now = Date.now();
    await g.record(WEEKLY_CAP, now - 7 * 86_400_000);
    expect((await g.status(now)).capped).toBe(false);
    expect((await g.status(now)).used).toBe(0);
  });

  it("lifts the cap until the paid time and no longer", async () => {
    const g = stub(newGroup());
    const now = Date.now();
    useServices({ verifier: { verify: async () => ({ until: now + 1000, purchase: "p1" }) } });
    await g.record(WEEKLY_CAP + 5, now);
    expect((await g.status(now)).capped).toBe(true);
    await g.buy("google", "token", "g");
    expect((await g.status(now)).capped).toBe(false);
    expect((await g.status(now)).paid).toBe(true);
    expect((await g.status(now + 2000)).capped).toBe(true);
  });

  it("keeps the later of two paid times", async () => {
    const g = stub(newGroup());
    let until = 5000;
    useServices({ verifier: { verify: async () => ({ until, purchase: "p2" }) } });
    await g.buy("apple", "a", "g");
    until = 3000;
    await g.buy("microsoft", "b", "g");
    expect((await g.status()).paidUntil).toBe(5000);
  });

  it("stores the count and the paid time", async () => {
    const group = newGroup();
    useServices({ verifier: { verify: async () => ({ until: Date.now() + 60_000, purchase: "p" + Math.random() }) } });
    await stub(group).record(123);
    await stub(group).buy("apple", "r", group);
    const week = (await stub(group).status()).week;
    await runInDurableObject(stub(group), async (_o, state) => {
      expect(await state.storage.get("usage")).toEqual({ [week]: 123 });
      expect(await state.storage.get<number>("paidUntil")).toBeGreaterThan(Date.now());
    });
  });

  it("answers a purchase with the group's status", async () => {
    const group = newGroup();
    useServices({ verifier: { verify: async () => ({ until: Date.now() + 60_000, purchase: "p" + Math.random() }) } });
    const res = await post(group, "/paid", { store: "apple", receipt: "r" });
    expect(res.status).toBe(200);
    expect((await res.json()) as { paid: boolean }).toMatchObject({ paid: true });
  });

  it("says plainly that receipts are not checked yet", async () => {
    const group = newGroup();
    expect((await post(group, "/paid", { store: "apple", receipt: "r" })).status).toBe(501);
    expect((await stub(group).status()).paid).toBe(false);
  });

  it("turns away a receipt the store refuses", async () => {
    useServices({
      verifier: {
        verify: async () => {
          throw new ReceiptRejectedError("expired");
        },
      },
    });
    const group = newGroup();
    expect((await post(group, "/paid", { store: "google", receipt: "r" })).status).toBe(422);
    expect((await stub(group).status()).paid).toBe(false);
  });

  it("lets one purchase pay for one group only", async () => {
    useServices({ verifier: { verify: async () => ({ until: Date.now() + 60_000, purchase: "shared" }) } });
    const [a, b] = [newGroup(), newGroup()];
    expect((await post(a, "/paid", { store: "apple", receipt: "r" })).status).toBe(200);
    expect((await post(a, "/paid", { store: "apple", receipt: "r" })).status).toBe(200);
    expect((await post(b, "/paid", { store: "apple", receipt: "r" })).status).toBe(409);
    expect((await stub(a).status()).paid).toBe(true);
    expect((await stub(b).status()).paid).toBe(false);
  });

  it("keeps a purchase only as a hash of the group and forgets it when the paid time ends", async () => {
    const until = Date.now() + 60_000;
    useServices({ verifier: { verify: async () => ({ until, purchase: "gone" }) } });
    const group = newGroup();
    expect((await post(group, "/paid", { store: "apple", receipt: "r" })).status).toBe(200);
    const owner = env.PURCHASES.get(env.PURCHASES.idFromName(await purchaseKey("apple", "gone")));
    await runInDurableObject(owner, async (_o, state) => {
      const kept = await state.storage.get<{ group: string; until: number }>("bound");
      expect(kept?.group).toMatch(/^[0-9a-f]{64}$/);
      expect(kept?.group).not.toContain(group);
      expect(await state.storage.getAlarm()).toBe(until);
      await state.storage.put("bound", { group: kept!.group, until: Date.now() - 1 });
    });
    expect(await runDurableObjectAlarm(owner)).toBe(true);
    await runInDurableObject(owner, async (_o, state) => {
      expect(await state.storage.get("bound")).toBeUndefined();
    });
    // Once forgotten, the same purchase is no longer tied to the first group.
    expect(await owner.bind(newGroup(), until + 1000)).toBe(true);
  });

  it("keeps purchases of different stores apart", async () => {
    useServices({ verifier: { verify: async () => ({ until: Date.now() + 60_000, purchase: "same-id" }) } });
    expect((await post(newGroup(), "/paid", { store: "apple", receipt: "r" })).status).toBe(200);
    expect((await post(newGroup(), "/paid", { store: "google", receipt: "r" })).status).toBe(200);
  });

  it("hands the group to the receipt check", async () => {
    const seen: string[] = [];
    useServices({
      verifier: {
        verify: async (_s, _r, group) => {
          seen.push(group);
          return { until: Date.now() + 1000, purchase: "p" + Math.random() };
        },
      },
    });
    const group = newGroup();
    await post(group, "/paid", { store: "apple", receipt: "r" });
    expect(seen).toEqual([group]);
  });

  it("refuses malformed purchases, groups and methods", async () => {
    const group = newGroup();
    expect((await post(group, "/paid", { store: "amazon", receipt: "r" })).status).toBe(400);
    expect((await post(group, "/paid", { store: "apple", receipt: "" })).status).toBe(400);
    expect((await call(group, "/paid", { method: "POST", body: "[1]" })).status).toBe(400);
    expect((await call(group, "/paid", { method: "POST", body: "x".repeat(70_000) })).status).toBe(400);
    const huge = new ReadableStream({ pull: (c) => c.enqueue(new Uint8Array(40_000)) });
    const streamed = await call(group, "/paid", { method: "POST", body: huge, duplex: "half" } as RequestInit);
    expect(streamed.status).toBe(400);
    const declared = await call(group, "/paid", { method: "POST", body: "{}", headers: { "Content-Length": "999999" } });
    expect([400, 413]).toContain(declared.status);
    expect((await call("short", "")).status).toBe(400);
    expect((await call(group, "", { method: "POST" })).status).toBe(405);
    expect((await call(group, "/paid")).status).toBe(405);
  });

  it("registers, replaces and drops a push token", async () => {
    const group = newGroup();
    const sent: unknown[] = [];
    useServices({ push: { send: async (p, t) => void sent.push([p, t]) } });
    const g = stub(group);
    expect((await post(group, "/push", { device: device(), platform: "apns", token: "one" })).status).toBe(204);
    expect(await g.wake("room", device(2), undefined, 100_000)).toBe(true);
    expect((await post(group, "/push", { device: device(), platform: "fcm", token: "two" })).status).toBe(204);
    expect(await g.wake("room", device(2), undefined, 200_000)).toBe(true);
    expect((await call(group, `/push/${device()}`, { method: "DELETE" })).status).toBe(204);
    expect(await g.wake("room", device(2), undefined, 300_000)).toBe(false);
    expect(sent).toEqual([
      ["apns", "one"],
      ["fcm", "two"],
    ]);
  });

  it("refuses malformed push registrations", async () => {
    const group = newGroup();
    const bad = (body: unknown) => post(group, "/push", body).then((r) => r.status);
    expect(await bad({ device: "x", platform: "apns", token: "t" })).toBe(400);
    expect(await bad({ device: device(), platform: "sms", token: "t" })).toBe(400);
    expect(await bad({ device: device(), platform: "apns", token: "" })).toBe(400);
    expect(await bad({ device: device(), platform: "apns", token: "t".repeat(5000) })).toBe(400);
  });

  it("limits the devices of a group", async () => {
    const group = newGroup();
    for (let i = 1; i <= MAX_DEVICES; i++) {
      expect((await post(group, "/push", { device: device(i), platform: "fcm", token: "t" })).status).toBe(204);
    }
    expect((await post(group, "/push", { device: device(MAX_DEVICES + 1), platform: "fcm", token: "t" })).status).toBe(409);
    expect((await post(group, "/push", { device: device(1), platform: "fcm", token: "new" })).status).toBe(204);
  });

  it("makes room for a new device by dropping ones that went quiet", async () => {
    const g = stub(newGroup());
    const day = 86_400_000;
    const now = Date.now();
    for (let i = 1; i <= MAX_DEVICES; i++) expect(await g.setPush(device(i), "fcm", "t", now - 100 * day)).toBe(true);
    expect(await g.setPush(device(MAX_DEVICES + 1), "fcm", "t", now)).toBe(true);
    expect(await g.setPush(device(MAX_DEVICES + 2), "fcm", "t", now)).toBe(true);
    for (let i = 3; i <= MAX_DEVICES; i++) await g.setPush(device(i), "fcm", "t", now);
    expect(await g.setPush(device(MAX_DEVICES + 3), "fcm", "t", now)).toBe(false);
  });

  it("drops quiet devices on every registration and wake, not only when full", async () => {
    const g = stub(newGroup());
    const day = 86_400_000;
    const now = Date.now();
    let sent = 0;
    useServices({ push: { send: async () => void sent++ } });
    await g.setPush(device(1), "fcm", "old", now - 100 * day);
    await g.setPush(device(2), "fcm", "old", now - 100 * day);
    expect(await g.wake("room", device(9), undefined, now)).toBe(false);
    expect(sent).toBe(0);
    await runInDurableObject(g, async (_o, state) => {
      expect(await state.storage.get("devices")).toEqual({});
    });
  });

  it("keeps a device that connects, and forgets one that does not", async () => {
    const g = stub(newGroup());
    const day = 86_400_000;
    const now = Date.now();
    await g.setPush(device(1), "fcm", "a", now - 80 * day);
    await g.setPush(device(2), "fcm", "b", now - 80 * day);
    await g.touch(device(1), now - 5 * day);
    await g.touch(device(1), now - 5 * day + 1000);
    await g.setPush(device(3), "fcm", "c", now + 20 * day);
    await runInDurableObject(g, async (_o, state) => {
      const kept = await state.storage.get<Record<string, unknown>>("devices");
      expect(Object.keys(kept!)).toEqual([device(1), device(3)]);
    });
  });

  it("sweeps old weeks and devices on an alarm and deletes a group with nothing left", async () => {
    const group = newGroup();
    const g = stub(group);
    const day = 86_400_000;
    const now = Date.now();
    await g.record(5, now - 100 * day);
    await g.setPush(device(1), "fcm", "t", now - 100 * day);
    await runInDurableObject(g, async (o, state) => {
      expect(await state.storage.getAlarm()).not.toBeNull();
      await o.alarm();
      expect(await state.storage.list()).toEqual(new Map());
      expect(await state.storage.getAlarm()).toBeNull();
    });
    await g.setPush(device(2), "fcm", "t");
    await runInDurableObject(g, async (o, state) => {
      await o.alarm();
      expect((await state.storage.get<Record<string, unknown>>("devices"))![device(2)]).toBeDefined();
      expect(await state.storage.getAlarm()).not.toBeNull();
    });
  });

  it("does not wake a device twice in a few seconds", async () => {
    const g = stub(newGroup());
    let count = 0;
    useServices({ push: { send: async () => void count++ } });
    await g.setPush(device(), "fcm", "t");
    await g.wake("room", device(2), undefined, 1_000_000);
    await g.wake("room", device(2), undefined, 1_003_000);
    expect(count).toBe(1);
    await g.wake("room", device(2), undefined, 1_020_000);
    expect(count).toBe(2);
  });

  it("forgets a token the push service says is gone", async () => {
    const g = stub(newGroup());
    useServices({
      push: {
        send: async () => {
          throw new TokenGoneError("unregistered");
        },
      },
    });
    await g.setPush(device(), "apns", "t");
    expect(await g.wake("room", device(2))).toBe(false);
    let sent = 0;
    useServices({ push: { send: async () => void sent++ } });
    expect(await g.wake("room", device(2), undefined, Date.now() + 60_000)).toBe(false);
    expect(sent).toBe(0);
  });

  it("does not wake when push is not set up", async () => {
    const g = stub(newGroup());
    await g.setPush(device(), "apns", "t");
    expect(await g.wake("room", device(2))).toBe(false);
  });

  it("wakes only the host's device when the room has one", async () => {
    const g = stub(newGroup());
    const sent: string[] = [];
    useServices({ push: { send: async (_p, t) => void sent.push(t) } });
    for (const i of [1, 2, 3]) await g.setPush(device(i), "fcm", `t${i}`);
    expect(await g.wake("room", device(2), device(3), 5_000_000)).toBe(true);
    expect(sent).toEqual(["t3"]);
    expect(await g.wake("room", device(3), device(3), 6_000_000)).toBe(false);
    expect(await g.wake("room", device(2), device(9), 7_000_000)).toBe(false);
    expect(sent).toEqual(["t3"]);
  });

  it("wakes every other device and not the caller", async () => {
    const g = stub(newGroup());
    const sent: string[] = [];
    useServices({ push: { send: async (_p, t) => void sent.push(t) } });
    for (const i of [1, 2, 3]) await g.setPush(device(i), "fcm", `t${i}`);
    expect(await g.wake("room", device(2), undefined, 5_000_000)).toBe(true);
    expect(sent.sort()).toEqual(["t1", "t3"]);
  });
});

describe("rate limit", () => {
  it("allows a window's worth of requests per address and then starts over", () => {
    const l = new RateLimiter(3, 1000);
    expect([1, 2, 3, 4].map(() => l.allow("a", 0))).toEqual([true, true, true, false]);
    expect(l.allow("b", 0)).toBe(true);
    expect(l.allow("a", 999)).toBe(false);
    expect(l.allow("a", 1000)).toBe(true);
  });

  it("drops stale addresses when many have been seen", () => {
    const l = new RateLimiter(1, 1000);
    for (let i = 0; i < 10_001; i++) l.allow(`ip${i}`, 0);
    expect(l.allow("ip1", 5000)).toBe(true);
    expect((l as unknown as { seen: Map<string, unknown> }).seen.size).toBeLessThan(10);
  });

  it("counts an IPv6 /64 as one address", () => {
    const l = new RateLimiter(2, 1000);
    expect(l.allow("2001:db8:1:2::1", 0)).toBe(true);
    expect(l.allow("2001:DB8:1:2:ffff:0:0:9", 0)).toBe(true);
    expect(l.allow("2001:db8:1:2:3:4:5:6", 0)).toBe(false);
    expect(l.allow("2001:db8:1:3::1", 0)).toBe(true);
    expect(networkOf("::1")).toBe("0:0:0:0");
    expect(networkOf("::ffff:1.2.3.4")).toBe("::ffff:1.2.3.4");
    expect(networkOf("192.0.2.1")).toBe("192.0.2.1");
  });

  it("lets new addresses in while full of live ones by dropping the oldest, and sweeps once a window", () => {
    const l = new RateLimiter(5, 1000);
    const size = () => (l as unknown as { seen: Map<string, unknown> }).seen.size;
    for (let i = 0; i < 10_000; i++) l.allow(`ip${i}`, 0);
    expect(l.allow("new", 500)).toBe(true);
    expect(size()).toBe(10_000);
    expect(l.allow("ip1", 500)).toBe(true);
    expect(l.allow("newer", 1000)).toBe(true);
    expect(size()).toBeLessThan(10);
  });

  it("answers 503 with Retry-After from the worker once an address is over the limit", async () => {
    const ip = "203.0.113.7";
    let last = 0;
    for (let i = 0; i < 1205; i++) {
      const res = await exports.default.fetch("https://alazy.app/relay/v1/nothing", { headers: { "CF-Connecting-IP": ip } });
      last = res.status;
      if (last === 503) {
        expect(res.headers.get("Retry-After")).toBe("60");
        break;
      }
    }
    expect(last).toBe(503);
  });

  it("counts a group call that claims to be a stream", async () => {
    const ip = "203.0.113.9";
    let last = 0;
    for (let i = 0; i < 1205; i++) {
      const res = await exports.default.fetch(`https://alazy.app/relay/v1/group/${"a".repeat(32)}?role=stream`, {
        headers: { "CF-Connecting-IP": ip },
      });
      last = res.status;
      if (last === 503) break;
    }
    expect(last).toBe(503);
  });

  it("does not count a host's stream opens against the address", async () => {
    const ip = "203.0.113.8";
    const stream = () =>
      exports.default.fetch(`https://alazy.app/relay/v1/${"0".repeat(64)}?role=stream&id=${"a".repeat(32)}`, {
        headers: { "CF-Connecting-IP": ip, Upgrade: "websocket" },
      });
    for (let i = 0; i < 1300; i++) expect((await stream()).status).toBe(404);
  });
});
