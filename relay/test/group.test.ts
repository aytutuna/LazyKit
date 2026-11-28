import { env, exports } from "cloudflare:workers";
import { runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";
import { MAX_DEVICES, WEEKLY_CAP } from "../src/group";
import { MAX_GROUPS } from "../src/quota";
import { RateLimiter, networkOf } from "../src/ratelimit";
import { purchaseKey } from "../src/purchase";
import { ReceiptRejectedError, TokenGoneError, useServices } from "../src/services";
import { groupSession, sessionFor } from "./helpers";

let n = 0;
const newGroup = () => (++n).toString(16).padStart(32, "e");
const device = (i = 1) => i.toString(16).padStart(32, "0");
const stub = (group: string) => env.GROUPS.get(env.GROUPS.idFromName(group));
const quota = (account: string) => env.QUOTAS.get(env.QUOTAS.idFromName(account));

const call = (group: string, path: string, init: RequestInit = {}, token?: string) =>
  exports.default.fetch(`https://alazy.app/relay/v1/group/${group}${path}`, {
    ...init,
    headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), ...(init.headers as Record<string, string>) },
  });

// A group with an owner signed in on device(1), claimed as a computer's group.
async function owned(i = 1) {
  const group = newGroup();
  const s = await groupSession(group, device(i));
  const post = (path: string, body: unknown, token = s.token) => call(group, path, { method: "POST", body: JSON.stringify(body) }, token);
  return { group, ...s, post, quota: quota(s.account) };
}

describe("quota", () => {
  afterEach(() => useServices());

  it("starts empty and under the cap", async () => {
    const o = await owned();
    const res = await call(o.group, "", {}, o.token);
    expect(await res.json()).toEqual({
      week: expect.any(String),
      used: 0,
      cap: WEEKLY_CAP,
      paidUntil: 0,
      paid: false,
      capped: false,
      accountToken: expect.stringMatching(/^[0-9a-f-]{36}$/),
    });
  });

  it("is capped from the byte the week's allowance is used up", async () => {
    const { quota: q } = await owned();
    expect(await q.record(WEEKLY_CAP - 1)).toBe(true);
    expect(await q.record(1)).toBe(false);
    expect((await q.status()).capped).toBe(true);
  });

  it("starts a fresh allowance every week", async () => {
    const { quota: q } = await owned();
    const now = Date.now();
    await q.record(WEEKLY_CAP, now - 7 * 86_400_000);
    expect((await q.status(now)).capped).toBe(false);
    expect((await q.status(now)).used).toBe(0);
  });

  it("counts every group of an account against one allowance", async () => {
    const a = await owned();
    const mine = newGroup();
    const email = `${a.group}@test.local`;
    const token = await sessionFor(email, device(2));
    const claim = await call(mine, "", { method: "PUT", body: JSON.stringify({ platform: "desktop" }) }, token);
    expect(claim.status).toBe(200);
    expect(await stub(a.group).record(WEEKLY_CAP - 10)).toBe(true);
    expect(await stub(mine).record(20)).toBe(false);
    expect((await a.quota.status()).used).toBe(WEEKLY_CAP + 10);
  });

  it("lifts the cap until the paid time and no longer", async () => {
    const { quota: q, account } = await owned();
    const now = Date.now();
    useServices({ verifier: { verify: async () => ({ until: now + 1000, purchase: "p1" }) } });
    await q.record(WEEKLY_CAP + 5, now);
    expect((await q.status(now)).capped).toBe(true);
    await q.buy("google", "token", account);
    expect((await q.status(now)).capped).toBe(false);
    expect((await q.status(now)).paid).toBe(true);
    expect((await q.status(now + 2000)).capped).toBe(true);
  });

  it("keeps the later of two paid times", async () => {
    const { quota: q, account } = await owned();
    let until = 5000;
    useServices({ verifier: { verify: async () => ({ until, purchase: "p2" }) } });
    await q.buy("apple", "a", account);
    until = 3000;
    await q.buy("microsoft", "b", account);
    expect((await q.status()).paidUntil).toBe(5000);
  });

  it("stores the count and the paid time", async () => {
    const { quota: q, account } = await owned();
    useServices({ verifier: { verify: async () => ({ until: Date.now() + 60_000, purchase: "p" + Math.random() }) } });
    await q.record(123);
    await q.buy("apple", "r", account);
    const week = (await q.status()).week;
    await runInDurableObject(q, async (_o, state) => {
      expect(await state.storage.get("usage")).toEqual({ [week]: 123 });
      expect(await state.storage.get<number>("paidUntil")).toBeGreaterThan(Date.now());
    });
  });

  it("answers a purchase with the account's status", async () => {
    const o = await owned();
    useServices({ verifier: { verify: async () => ({ until: Date.now() + 60_000, purchase: "p" + Math.random() }) } });
    const res = await o.post("/paid", { store: "apple", receipt: "r" });
    expect(res.status).toBe(200);
    expect((await res.json()) as { paid: boolean }).toMatchObject({ paid: true });
  });

  it("says plainly that receipts are not checked yet", async () => {
    const o = await owned();
    expect((await o.post("/paid", { store: "apple", receipt: "r" })).status).toBe(501);
    expect((await o.quota.status()).paid).toBe(false);
  });

  it("turns away a receipt the store refuses", async () => {
    useServices({ verifier: { verify: async () => { throw new ReceiptRejectedError("expired"); } } });
    const o = await owned();
    expect((await o.post("/paid", { store: "google", receipt: "r" })).status).toBe(422);
    expect((await o.quota.status()).paid).toBe(false);
  });

  it("lets one purchase pay for one account only", async () => {
    useServices({ verifier: { verify: async () => ({ until: Date.now() + 60_000, purchase: "shared" }) } });
    const [a, b] = [await owned(), await owned()];
    expect((await a.post("/paid", { store: "apple", receipt: "r" })).status).toBe(200);
    expect((await a.post("/paid", { store: "apple", receipt: "r" })).status).toBe(200);
    expect((await b.post("/paid", { store: "apple", receipt: "r" })).status).toBe(409);
    expect((await a.quota.status()).paid).toBe(true);
    expect((await b.quota.status()).paid).toBe(false);
  });

  it("keeps a purchase only as a hash of the account and forgets it when the paid time ends", async () => {
    const until = Date.now() + 60_000;
    useServices({ verifier: { verify: async () => ({ until, purchase: "gone" }) } });
    const o = await owned();
    expect((await o.post("/paid", { store: "apple", receipt: "r" })).status).toBe(200);
    const owner = env.PURCHASES.get(env.PURCHASES.idFromName(await purchaseKey("apple", "gone")));
    await runInDurableObject(owner, async (_o, state) => {
      const kept = await state.storage.get<{ account: string; until: number }>("bound");
      expect(kept?.account).toMatch(/^[0-9a-f]{64}$/);
      expect(kept?.account).not.toContain(o.account);
      expect(await state.storage.getAlarm()).toBe(until);
      await state.storage.put("bound", { account: kept!.account, until: Date.now() - 1 });
    });
    expect(await runDurableObjectAlarm(owner)).toBe(true);
    await runInDurableObject(owner, async (_o, state) => {
      expect(await state.storage.get("bound")).toBeUndefined();
    });
    expect(await owner.bind("another account", until + 1000)).toBe(true);
  });

  it("keeps purchases of different stores apart", async () => {
    useServices({ verifier: { verify: async () => ({ until: Date.now() + 60_000, purchase: "same-id" }) } });
    expect((await (await owned()).post("/paid", { store: "apple", receipt: "r" })).status).toBe(200);
    expect((await (await owned()).post("/paid", { store: "google", receipt: "r" })).status).toBe(200);
  });

  it("hands the account's token to the receipt check", async () => {
    const seen: string[] = [];
    useServices({
      verifier: {
        verify: async (_s, _r, token) => {
          seen.push(token);
          return { until: Date.now() + 1000, purchase: "p" + Math.random() };
        },
      },
    });
    const o = await owned();
    await o.post("/paid", { store: "apple", receipt: "r" });
    expect(seen).toEqual([(await o.quota.status()).accountToken]);
    expect(seen[0]).not.toContain(o.group);
  });

  it("refuses malformed purchases, groups and methods", async () => {
    const o = await owned();
    expect((await o.post("/paid", { store: "amazon", receipt: "r" })).status).toBe(400);
    expect((await o.post("/paid", { store: "apple", receipt: "" })).status).toBe(400);
    expect((await call(o.group, "/paid", { method: "POST", body: "[1]" }, o.token)).status).toBe(400);
    expect((await call(o.group, "/paid", { method: "POST", body: "x".repeat(70_000) }, o.token)).status).toBe(400);
    const huge = new ReadableStream({ pull: (c) => c.enqueue(new Uint8Array(40_000)) });
    const streamed = await call(o.group, "/paid", { method: "POST", body: huge, duplex: "half" } as RequestInit, o.token);
    expect(streamed.status).toBe(400);
    const declared = await call(o.group, "/paid", { method: "POST", body: "{}", headers: { "Content-Length": "999999" } }, o.token);
    expect([400, 413]).toContain(declared.status);
    expect((await call("short", "", {}, o.token)).status).toBe(400);
    expect((await call(o.group, "", { method: "POST" }, o.token)).status).toBe(405);
    expect((await call(o.group, "/paid", {}, o.token)).status).toBe(405);
  });
});

describe("groups and accounts", () => {
  afterEach(() => useServices());

  it("needs a signed-in account for every group call", async () => {
    const o = await owned();
    for (const [path, init] of [["", {}], ["/push", { method: "POST", body: "{}" }], ["/paid", { method: "POST", body: "{}" }], ["/challenge", { method: "POST" }], ["/attest", { method: "POST", body: "{}" }], [`/push/${device()}`, { method: "DELETE" }], ["", { method: "PUT", body: "{}" }]] as [string, RequestInit][]) {
      const res = await call(o.group, path, init);
      expect(res.status, path).toBe(401);
      expect(res.headers.get("x-relay-reason")).toBe("sign-in");
    }
    expect((await call(o.group, "", {}, "0".repeat(64) + "." + "A".repeat(43))).status).toBe(401);
  });

  it("claims a group for the first account, and refuses everyone else", async () => {
    const o = await owned();
    const stranger = await sessionFor("stranger@test.local", device(1));
    const claim = await call(o.group, "", { method: "PUT", body: JSON.stringify({ platform: "desktop" }) }, stranger);
    expect(claim.status).toBe(403);
    expect(claim.headers.get("x-relay-reason")).toBe("wrong-account");
    for (const [path, init] of [["", {}], ["/push", { method: "POST", body: JSON.stringify({ device: device(1), platform: "fcm", token: "t" }) }], ["/paid", { method: "POST", body: JSON.stringify({ store: "apple", receipt: "r" }) }], ["/challenge", { method: "POST" }], [`/push/${device()}`, { method: "DELETE" }]] as [string, RequestInit][]) {
      const res = await call(o.group, path, init, stranger);
      expect(res.status, path).toBe(403);
      expect(res.headers.get("x-relay-reason")).toBe("wrong-account");
    }
  });

  it("lets the owner claim again from another device", async () => {
    const o = await owned();
    const second = await groupSession(o.group, device(2));
    expect(second.account).toBe(o.account);
    expect((await call(o.group, "", {}, second.token)).status).toBe(200);
  });

  it("refuses an unclaimed group everything but a claim", async () => {
    const token = await sessionFor("lonely@test.local", device(1));
    const group = newGroup();
    expect((await call(group, "", {}, token)).status).toBe(403);
    expect((await call(group, "/challenge", { method: "POST" }, token)).status).toBe(403);
  });

  it("refuses a claim with a bad platform", async () => {
    const token = await sessionFor("claimant@test.local", device(1));
    for (const platform of ["windows", "", 3, undefined]) {
      expect((await call(newGroup(), "", { method: "PUT", body: JSON.stringify({ platform }) }, token)).status).toBe(400);
    }
  });

  it("limits how many groups an account keeps", async () => {
    const token = await sessionFor("collector@test.local", device(1));
    const put = (g: string) => call(g, "", { method: "PUT", body: JSON.stringify({ platform: "desktop" }) }, token);
    const first = newGroup();
    expect((await put(first)).status).toBe(200);
    for (let i = 1; i < MAX_GROUPS; i++) expect((await put(newGroup())).status).toBe(200);
    expect((await put(newGroup())).status).toBe(409);
    expect((await put(first)).status).toBe(200);
  });

  it("forgets a group nobody has used for 90 days and frees its place", async () => {
    const o = await owned();
    const g = stub(o.group);
    await runInDurableObject(g as unknown as DurableObjectStub, async (obj: any, state) => {
      obj.seenAt = Date.now() - 100 * 86_400_000;
      await state.storage.put("seenAt", obj.seenAt);
      await obj.alarm();
      expect([...(await state.storage.list())]).toEqual([]);
    });
    expect(await g.owner()).toBeUndefined();
    await runInDurableObject(o.quota, async (_o, state) => expect(await state.storage.get("groups")).toEqual([]));
  });

  it("keeps a group with a device through the sweep", async () => {
    const o = await owned();
    const g = stub(o.group);
    await g.setPush(device(1), "fcm", "t");
    await runInDurableObject(g as unknown as DurableObjectStub, async (obj: any, state) => {
      obj.seenAt = Date.now() - 100 * 86_400_000;
      await obj.alarm();
      expect(await state.storage.get("account")).toBe(o.account);
    });
  });
});

describe("push tokens", () => {
  afterEach(() => useServices());

  it("registers, replaces and drops a push token", async () => {
    const o = await owned();
    const sent: unknown[] = [];
    useServices({ push: { send: async (p, t) => void sent.push([p, t]) } });
    const g = stub(o.group);
    expect((await o.post("/push", { device: device(), platform: "apns", token: "one" })).status).toBe(204);
    expect(await g.wake("room", device(2), undefined, 100_000)).toBe(true);
    expect((await o.post("/push", { device: device(), platform: "fcm", token: "two" })).status).toBe(204);
    expect(await g.wake("room", device(2), undefined, 200_000)).toBe(true);
    expect((await call(o.group, `/push/${device()}`, { method: "DELETE" }, o.token)).status).toBe(204);
    expect(await g.wake("room", device(2), undefined, 300_000)).toBe(false);
    expect(sent).toEqual([["apns", "one"], ["fcm", "two"]]);
  });

  it("registers a token for the session's own device only", async () => {
    const o = await owned();
    const res = await o.post("/push", { device: device(2), platform: "fcm", token: "t" });
    expect(res.status).toBe(403);
    expect(res.headers.get("x-relay-reason")).toBe("wrong-device");
    expect(await stub(o.group).wake("room", device(9))).toBe(false);
  });

  it("refuses malformed push registrations", async () => {
    const o = await owned();
    const bad = (body: unknown) => o.post("/push", body).then((r) => r.status);
    expect(await bad({ device: "x", platform: "apns", token: "t" })).toBe(403);
    expect(await bad({ device: device(), platform: "sms", token: "t" })).toBe(400);
    expect(await bad({ device: device(), platform: "apns", token: "" })).toBe(400);
    expect(await bad({ device: device(), platform: "apns", token: "t".repeat(5000) })).toBe(400);
  });

  it("limits the devices of a group", async () => {
    const o = await owned();
    const g = stub(o.group);
    for (let i = 1; i < MAX_DEVICES; i++) await g.setPush(device(i + 100), "fcm", "t");
    expect((await o.post("/push", { device: device(), platform: "fcm", token: "t" })).status).toBe(204);
    const next = await groupSession(o.group, device(2));
    expect((await o.post("/push", { device: device(2), platform: "fcm", token: "t" }, next.token)).status).toBe(409);
    expect((await o.post("/push", { device: device(), platform: "fcm", token: "new" })).status).toBe(204);
  });

  it("makes room for a new device by dropping ones that went quiet", async () => {
    const g = stub((await owned()).group);
    const day = 86_400_000;
    const now = Date.now();
    for (let i = 1; i <= MAX_DEVICES; i++) expect(await g.setPush(device(i), "fcm", "t", now - 100 * day)).toBe(true);
    expect(await g.setPush(device(MAX_DEVICES + 1), "fcm", "t", now)).toBe(true);
    expect(await g.setPush(device(MAX_DEVICES + 2), "fcm", "t", now)).toBe(true);
    for (let i = 3; i <= MAX_DEVICES; i++) await g.setPush(device(i), "fcm", "t", now);
    expect(await g.setPush(device(MAX_DEVICES + 3), "fcm", "t", now)).toBe(false);
  });

  it("drops quiet devices on every registration and wake, not only when full", async () => {
    const g = stub((await owned()).group);
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
    const g = stub((await owned()).group);
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

  it("does not wake a device twice in a few seconds", async () => {
    const g = stub((await owned()).group);
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
    const g = stub((await owned()).group);
    useServices({ push: { send: async () => { throw new TokenGoneError("unregistered"); } } });
    await g.setPush(device(), "apns", "t");
    expect(await g.wake("room", device(2))).toBe(false);
    let sent = 0;
    useServices({ push: { send: async () => void sent++ } });
    expect(await g.wake("room", device(2), undefined, Date.now() + 60_000)).toBe(false);
    expect(sent).toBe(0);
  });

  it("does not wake when push is not set up", async () => {
    const g = stub((await owned()).group);
    await g.setPush(device(), "apns", "t");
    expect(await g.wake("room", device(2))).toBe(false);
  });

  it("wakes only the host's device when the room has one", async () => {
    const g = stub((await owned()).group);
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
    const g = stub((await owned()).group);
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
    const ip = "203.0.113.19";
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

  it("counts account calls too", async () => {
    const ip = "203.0.113.20";
    let last = 0;
    for (let i = 0; i < 1205; i++) {
      last = (await exports.default.fetch("https://alazy.app/account/v1/me", { headers: { "CF-Connecting-IP": ip } })).status;
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
