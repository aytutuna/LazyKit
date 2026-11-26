import { env, exports } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";
import { MAX_DEVICES, WEEKLY_CAP } from "../src/group";
import { RateLimiter } from "../src/ratelimit";
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
    useServices({ verifier: { verify: async () => ({ until: now + 1000 }) } });
    await g.record(WEEKLY_CAP + 5, now);
    expect((await g.status(now)).capped).toBe(true);
    await g.buy("google", "token");
    expect((await g.status(now)).capped).toBe(false);
    expect((await g.status(now)).paid).toBe(true);
    expect((await g.status(now + 2000)).capped).toBe(true);
  });

  it("keeps the later of two paid times", async () => {
    const g = stub(newGroup());
    let until = 5000;
    useServices({ verifier: { verify: async () => ({ until }) } });
    await g.buy("apple", "a");
    until = 3000;
    await g.buy("microsoft", "b");
    expect((await g.status()).paidUntil).toBe(5000);
  });

  it("stores the count and the paid time", async () => {
    const group = newGroup();
    useServices({ verifier: { verify: async () => ({ until: Date.now() + 60_000 }) } });
    await stub(group).record(123);
    await stub(group).buy("apple", "r");
    const week = (await stub(group).status()).week;
    await runInDurableObject(stub(group), async (_o, state) => {
      expect(await state.storage.get("usage")).toEqual({ [week]: 123 });
      expect(await state.storage.get<number>("paidUntil")).toBeGreaterThan(Date.now());
    });
  });

  it("answers a purchase with the group's status", async () => {
    const group = newGroup();
    useServices({ verifier: { verify: async () => ({ until: Date.now() + 60_000 }) } });
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

  it("refuses malformed purchases, groups and methods", async () => {
    const group = newGroup();
    expect((await post(group, "/paid", { store: "amazon", receipt: "r" })).status).toBe(400);
    expect((await post(group, "/paid", { store: "apple", receipt: "" })).status).toBe(400);
    expect((await call(group, "/paid", { method: "POST", body: "[1]" })).status).toBe(400);
    expect((await call(group, "/paid", { method: "POST", body: "x".repeat(70_000) })).status).toBe(400);
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
    expect(await g.wake(device(), "room", 100_000)).toBe(true);
    expect((await post(group, "/push", { device: device(), platform: "fcm", token: "two" })).status).toBe(204);
    expect(await g.wake(device(), "room", 200_000)).toBe(true);
    expect((await call(group, `/push/${device()}`, { method: "DELETE" })).status).toBe(204);
    expect(await g.wake(device(), "room", 300_000)).toBe(false);
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

  it("does not wake a device twice in a few seconds", async () => {
    const g = stub(newGroup());
    let count = 0;
    useServices({ push: { send: async () => void count++ } });
    await g.setPush(device(), "fcm", "t");
    await g.wake(device(), "room", 1_000_000);
    await g.wake(device(), "room", 1_003_000);
    expect(count).toBe(1);
    await g.wake(device(), "room", 1_020_000);
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
    expect(await g.wake(device(), "room")).toBe(false);
    let sent = 0;
    useServices({ push: { send: async () => void sent++ } });
    expect(await g.wake(device(), "room", Date.now() + 60_000)).toBe(false);
    expect(sent).toBe(0);
  });

  it("does not wake when push is not set up", async () => {
    const g = stub(newGroup());
    await g.setPush(device(), "apns", "t");
    expect(await g.wake(device(), "room")).toBe(false);
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

  it("answers 429 from the worker once an address is over the limit", async () => {
    const ip = "203.0.113.7";
    let last = 0;
    for (let i = 0; i < 125; i++) {
      const res = await exports.default.fetch("https://alazy.app/relay/v1/nothing", { headers: { "CF-Connecting-IP": ip } });
      last = res.status;
      if (last === 429) break;
    }
    expect(last).toBe(429);
  });
});
