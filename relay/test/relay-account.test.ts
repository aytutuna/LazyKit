import { env, exports } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { WEEKLY_CAP } from "../src/group";
import { purchaseKey } from "../src/purchase";
import { useServices } from "../src/services";
import { DEVICE_A, account, fakes, signUp, who } from "./helpers";

const PASSWORD = "correct horse battery";
const DEV_B = "b".repeat(32);
let n = 0;
const newGroup = () => `${(++n).toString(16)}`.padStart(32, "c");
const room = (g: string) => `${g}${g}`;

const relay = (path: string, init: RequestInit = {}, token?: string) =>
  exports.default.fetch(`https://alazy.app/relay/v1/${path}`, { ...init, headers: { ...(token ? { Authorization: `Bearer ${token}` } : {}), ...(init.headers as Record<string, string>) } });
const claim = (g: string, token: string, platform = "desktop") => relay(`group/${g}`, { method: "PUT", body: JSON.stringify({ platform }) }, token);
const host = (g: string, device: string, token?: string) => relay(`${room(g)}?role=host&group=${g}&device=${device}`, { headers: { Upgrade: "websocket" } }, token);
const signIn = async (email: string, device: string) =>
  ((await (await account("signin", "POST", { email, password: PASSWORD, app: "lazysend", device })).json()) as { token: string }).token;
const quotaOf = (token: string) => env.QUOTAS.get(env.QUOTAS.idFromName(token.split(".")[0]));
const empty = (stub: DurableObjectStub) => runInDurableObject(stub, async (_o, state) => expect([...(await state.storage.list())]).toEqual([]));

beforeEach(() => fakes());
afterEach(() => useServices());

describe("relay use needs an account", () => {
  it("refuses a room to a request with no session, a bad one or a signed-out one", async () => {
    const token = await signUp(who.email, PASSWORD);
    const g = newGroup();
    expect((await claim(g, token)).status).toBe(200);
    for (const bad of [undefined, "x", `${"0".repeat(64)}.${"A".repeat(43)}`]) {
      const res = await host(g, DEVICE_A, bad);
      expect(res.status).toBe(401);
      expect(res.headers.get("x-relay-reason")).toBe("sign-in");
    }
    expect((await host(g, DEVICE_A, token)).status).toBe(101);
    await account("signout", "POST", {}, token);
    expect((await host(g, DEVICE_A, token)).status).toBe(401);
  });

  it("refuses a session on a group of another account and on another device", async () => {
    const mine = await signUp(who.email, PASSWORD);
    const theirs = await signUp(who.other, PASSWORD);
    const g = newGroup();
    await claim(g, mine);
    const wrongAccount = await host(g, DEVICE_A, theirs);
    expect(wrongAccount.status).toBe(403);
    expect(wrongAccount.headers.get("x-relay-reason")).toBe("wrong-account");
    const wrongDevice = await host(g, DEV_B, mine);
    expect(wrongDevice.status).toBe(403);
    expect(wrongDevice.headers.get("x-relay-reason")).toBe("wrong-device");
    expect((await host(newGroup(), DEVICE_A, mine)).status).toBe(403);
  });

  it("serves a second device of the same account", async () => {
    const a = await signUp(who.email, PASSWORD);
    const b = await signIn(who.email, DEV_B);
    const g = newGroup();
    await claim(g, a);
    expect((await host(g, DEV_B, b)).status).toBe(101);
  });

  it("shares the cap and the paid time between an account's groups and devices", async () => {
    fakes({ verifier: { verify: async () => ({ until: Date.now() + 60_000, purchase: "shared-purchase" }) } });
    const a = await signUp(who.email, PASSWORD);
    const b = await signIn(who.email, DEV_B);
    const [g1, g2] = [newGroup(), newGroup()];
    await claim(g1, a);
    await claim(g2, b);
    await quotaOf(a).record(WEEKLY_CAP);
    const status = async (g: string, token: string) => (await (await relay(`group/${g}`, {}, token)).json()) as { capped: boolean; paid: boolean };
    expect((await status(g1, a)).capped).toBe(true);
    expect((await status(g2, b)).capped).toBe(true);
    // Bought on one device, felt on the other.
    expect((await relay(`group/${g1}/paid`, { method: "POST", body: JSON.stringify({ store: "apple", receipt: "r" }) }, a)).status).toBe(200);
    expect(await status(g2, b)).toMatchObject({ capped: false, paid: true });
  });
});

describe("deleting an account clears the relay", () => {
  it("removes groups, push tokens and usage, and keeps only the purchase hash", async () => {
    const until = Date.now() + 3_600_000;
    fakes({ verifier: { verify: async () => ({ until, purchase: "kept-until-paid-time-ends" }) } });
    const token = await signUp(who.email, PASSWORD);
    const [g1, g2] = [newGroup(), newGroup()];
    await claim(g1, token);
    await claim(g2, token);
    await relay(`group/${g1}/push`, { method: "POST", body: JSON.stringify({ device: DEVICE_A, platform: "fcm", token: "push-token-1" }) }, token);
    await env.GROUPS.get(env.GROUPS.idFromName(g2)).setPush(DEV_B, "apns", "push-token-2");
    await relay(`group/${g1}/paid`, { method: "POST", body: JSON.stringify({ store: "apple", receipt: "r" }) }, token);
    const quota = quotaOf(token);
    await quota.record(1234);

    const exported = (await (await account("export", "GET", undefined, token)).json()) as any;
    expect(exported.apps.relay.groups).toHaveLength(2);
    expect(exported.apps.relay.usage).toBeDefined();
    expect(JSON.stringify(exported)).toContain("push-token-1");

    const res = await account("me", "DELETE", { confirm: "delete" }, token);
    expect(res.status).toBe(200);
    expect(((await res.json()) as any).deleted.apps.relay).toMatchObject({ groups: 2, pushTokens: 2, usageWeeks: 1, purchaseKeptUntil: until });

    await empty(quota);
    await empty(env.GROUPS.get(env.GROUPS.idFromName(g1)));
    await empty(env.GROUPS.get(env.GROUPS.idFromName(g2)));
    await empty(env.ACCOUNTS.get(env.ACCOUNTS.idFromName(token.split(".")[0])));

    // Only the purchase, as hashes, until the paid time ends.
    const owner = env.PURCHASES.get(env.PURCHASES.idFromName(await purchaseKey("apple", "kept-until-paid-time-ends")));
    await runInDurableObject(owner, async (_o, state) => {
      const all = JSON.stringify([...(await state.storage.list())]);
      expect(all).not.toContain(who.email);
      expect(all).not.toContain(g1);
      expect(all).toContain(String(until));
    });

    // The token is dead, the groups are free, and the purchase cannot pay for someone else.
    expect((await host(g1, DEVICE_A, token)).status).toBe(401);
    expect((await relay(`group/${g1}`, {}, token)).status).toBe(401);
    const other = await signUp(who.other, PASSWORD);
    expect((await claim(g1, other)).status).toBe(200);
    expect((await relay(`group/${g1}/paid`, { method: "POST", body: JSON.stringify({ store: "apple", receipt: "r" }) }, other)).status).toBe(409);
  });

  it("stops counting bytes for a deleted account's group", async () => {
    const token = await signUp(who.email, PASSWORD);
    const g = newGroup();
    await claim(g, token);
    await account("me", "DELETE", { confirm: "delete" }, token);
    expect(await env.GROUPS.get(env.GROUPS.idFromName(g)).record(10)).toBe(false);
    expect((await env.GROUPS.get(env.GROUPS.idFromName(g)).status()).capped).toBe(true);
  });
});
