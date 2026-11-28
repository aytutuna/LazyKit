import { beforeEach, describe, expect, it } from "vitest";
import { accountName } from "../../src/account/core/account";
import { codeIn, kit, memoryService } from "./memory";

const PASSWORD = "correct horse battery";
const LOGIN = { app: "lazysend", device: "a".repeat(32), label: "laptop" };
const OTHER = { ...LOGIN, device: "b".repeat(32) };
const NONCE = "n".repeat(24);
const MIN = 60_000;
const DAY = 86_400_000;

let n = 0;
let email = "";
let rig = memoryService();
beforeEach(() => {
  email = `ada${++n}@example.com`;
  rig = memoryService();
});

async function signUp(address = email, password = PASSWORD) {
  const before = rig.mail.length;
  expect(await rig.service.signup(address, password)).toBe("sent");
  const r = await rig.service.confirm(address, codeIn(rig.mail[before]), LOGIN);
  if (!r.ok) throw new Error(`confirm ${r.why}`);
  return r;
}

const token = (provider: "apple" | "google", claims: Record<string, unknown> = {}, opts = {}) => {
  const now = Math.floor(Date.now() / 1000);
  const base =
    provider === "apple"
      ? { iss: "https://appleid.apple.com", aud: "com.lazyscan.lazyscan", sub: `apple-${n}`, email, email_verified: "true" }
      : { iss: "https://accounts.google.com", aud: "google-client", sub: `google-${n}`, email, email_verified: true };
  return kit.sign({ ...base, nonce: NONCE, iat: now, exp: now + 600, ...claims }, opts);
};

describe("sign-up and codes", () => {
  it("makes an account once the code is entered, and a session that names its app and device", async () => {
    const r = await signUp();
    const s = await rig.service.session(`Bearer ${r.token}`);
    expect(s).toMatchObject({ app: "lazysend", device: LOGIN.device });
    expect(await rig.service.me(s!)).toMatchObject({ email, password: true });
  });

  it("sends nothing and says nothing different for an address that has an account", async () => {
    await signUp();
    expect(await rig.service.signup(email, "another long password")).toBe("sent");
    expect(rig.mail).toHaveLength(1);
    expect(await rig.service.code("nobody@example.com")).toBe("sent");
    expect(rig.mail).toHaveLength(1);
  });

  it("gives up on a code after three wrong tries and never on time alone", async () => {
    await signUp();
    await rig.service.code(email);
    const code = codeIn(rig.mail[1]);
    const wrong = code === "000000" ? "000001" : "000000";
    for (let i = 0; i < 3; i++) expect(await rig.service.signinCode(email, wrong, LOGIN)).toEqual({ ok: false, why: "bad" });
    expect(await rig.service.signinCode(email, code, LOGIN)).toEqual({ ok: false, why: "bad" });
  });

  it("counts parallel guesses before comparing", async () => {
    await signUp();
    await rig.service.code(email);
    const code = codeIn(rig.mail[1]);
    const guesses = Array.from({ length: 20 }, (_, i) => String(100000 + i)).map((g) => (g === code ? "999999" : g));
    const rs = await Promise.all(guesses.map((g) => rig.service.signinCode(email, g, LOGIN)));
    expect(rs.every((r) => !r.ok)).toBe(true);
    const state = rig.accounts.get(await accountName(email))!.store.value!;
    expect(state.codes.signin!.tries).toBe(3);
  });

  it("expires a code after ten minutes and allows three codes in ten minutes", async () => {
    await signUp();
    await rig.service.code(email);
    rig.clock.now += 11 * MIN;
    expect(await rig.service.signinCode(email, codeIn(rig.mail[1]), LOGIN)).toEqual({ ok: false, why: "bad" });
    expect(await rig.service.code(email)).toBe("sent");
    expect(await rig.service.code(email)).toBe("sent");
    expect(await rig.service.code(email)).toBe("sent");
    expect(await rig.service.code(email)).toBe("limited");
  });

  it("stops after ten wrong codes in a day and opens again the next day", async () => {
    await signUp();
    for (let round = 0; round < 4; round++) {
      rig.clock.now += 11 * MIN;
      await rig.service.code(email);
      for (let i = 0; i < 3; i++) await rig.service.signinCode(email, "000000", LOGIN);
    }
    rig.clock.now += 11 * MIN;
    await rig.service.code(email);
    expect((await rig.service.signinCode(email, codeIn(rig.mail[rig.mail.length - 1]), LOGIN)).ok).toBe(false);
    rig.clock.now += DAY;
    await rig.service.code(email);
    expect((await rig.service.signinCode(email, codeIn(rig.mail[rig.mail.length - 1]), LOGIN)).ok).toBe(true);
  });

  it("keeps no code in the clear, and sends none without the secret", async () => {
    await signUp();
    await rig.service.code(email);
    const state = rig.accounts.get(await accountName(email))!.store.value!;
    expect(JSON.stringify(state)).not.toContain(codeIn(rig.mail[1]));
    const bare = memoryService({ codeKey: undefined });
    expect(await bare.service.signup(email, PASSWORD)).toBe("not-configured");
    expect(bare.mail).toHaveLength(0);
  });

  it("forgets a sign-up nobody confirmed", async () => {
    await rig.service.signup(email, PASSWORD);
    const { core, store } = rig.accounts.get(await accountName(email))!;
    rig.clock.now += 2 * 3_600_000;
    await core.sweep();
    expect(store.value).toBeUndefined();
    expect(store.alarm).toBeUndefined();
  });
});

describe("passwords and sessions", () => {
  it("answers a wrong password and an unknown address alike, and locks after ten misses", async () => {
    await signUp();
    const wrong = await rig.service.signinPassword(email, "not the password", LOGIN);
    expect(wrong).toEqual(await rig.service.signinPassword("nobody@example.com", PASSWORD, LOGIN));
    for (let i = 0; i < 9; i++) await rig.service.signinPassword(email, `wrong password ${i}`, LOGIN);
    expect(await rig.service.signinPassword(email, PASSWORD, LOGIN)).toEqual({ ok: false, why: "locked" });
    rig.clock.now += 16 * MIN;
    expect((await rig.service.signinPassword(email, PASSWORD, LOGIN)).ok).toBe(true);
  });

  it("stores a salted record and never the password", async () => {
    await signUp();
    const state = rig.accounts.get(await accountName(email))!.store.value!;
    expect(state.password).toMatchObject({ v: 1, stages: 1 });
    expect(JSON.stringify(state)).not.toContain(PASSWORD);
  });

  it("lets a session lapse after thirty idle days and refreshes an active one", async () => {
    const r = await signUp();
    const header = `Bearer ${r.token}`;
    rig.clock.now += 20 * DAY;
    expect(await rig.service.session(header)).toBeTruthy();
    rig.clock.now += 20 * DAY;
    expect(await rig.service.session(header)).toBeTruthy();
    rig.clock.now += 40 * DAY;
    expect(await rig.service.session(header)).toBeUndefined();
  });

  it("refuses forged and cross-account tokens", async () => {
    const a = await signUp();
    const b = await signUp(`grace${n}@example.com`);
    const [name, secret] = a.token.split(".");
    const [otherName, otherSecret] = b.token.split(".");
    for (const bad of [null, "", a.token, `Bearer ${name}`, `Bearer ${name}.${otherSecret}`, `Bearer ${otherName}.${secret}`, `Bearer ${"0".repeat(64)}.${secret}`, `Bearer ${name}.${secret}.x`]) {
      expect(await rig.service.session(bad), String(bad)).toBeUndefined();
    }
  });

  it("changes a password with the old one or right after signing in, and ends other sessions", async () => {
    const a = await signUp();
    const b = await rig.service.signinPassword(email, PASSWORD, OTHER);
    if (!b.ok) throw new Error("no");
    const s = (await rig.service.session(`Bearer ${b.token}`))!;
    expect(await rig.service.setPassword(s, "short", undefined)).toBe("bad-password");
    expect(await rig.service.setPassword(s, "a brand new password", undefined)).toBe("ok");
    expect(await rig.service.session(`Bearer ${a.token}`)).toBeUndefined();
    rig.clock.now += 11 * MIN;
    const old = (await rig.service.session(`Bearer ${b.token}`))!;
    expect(old.fresh).toBe(false);
    expect(await rig.service.setPassword(old, "yet another password", undefined)).toBe("bad");
    expect(await rig.service.setPassword(old, "yet another password", "wrong")).toBe("bad");
    expect(await rig.service.setPassword(old, "yet another password", "a brand new password")).toBe("ok");
  });

  it("limits sessions to twenty and replaces a device's own", async () => {
    await signUp();
    for (let i = 0; i < 25; i++) await rig.service.signinPassword(email, PASSWORD, { ...LOGIN, device: i.toString(16).padStart(32, "0") });
    const r = await rig.service.signinPassword(email, PASSWORD, LOGIN);
    if (!r.ok) throw new Error("no");
    expect((r.account as { sessions: number }).sessions).toBe(20);
  });

  it("refuses an unknown app or device", () => {
    expect(rig.service.login({ ...LOGIN, app: "evil" })).toBeUndefined();
    expect(rig.service.login({ ...LOGIN, device: "short" })).toBeUndefined();
    expect(rig.service.login({ ...LOGIN, label: "x".repeat(65) })).toBeUndefined();
    expect(rig.service.login({ ...LOGIN, label: "a\u0000b" })?.label).toBe("ab");
  });
});

describe("Apple and Google", () => {
  it("makes one account from either provider and links them by email", async () => {
    const a = await rig.service.social("apple", await token("apple"), NONCE, LOGIN);
    const g = await rig.service.social("google", await token("google"), NONCE, OTHER);
    expect(a.ok && g.ok).toBe(true);
    if (g.ok) expect(g.account).toMatchObject({ email, providers: ["apple", "google"], sessions: 2 });
  });

  it("finds the account by subject when the token has no email, and counts a private relay address", async () => {
    await rig.service.social("apple", await token("apple"), NONCE, LOGIN);
    const again = await rig.service.social("apple", await token("apple", { email: undefined, email_verified: undefined, iat: Math.floor(Date.now() / 1000) - 1 }), NONCE, OTHER);
    expect(again.ok).toBe(true);
    const relay = await rig.service.social("apple", await token("apple", { email: "x1@privaterelay.appleid.com", sub: "relay-sub" }), NONCE, LOGIN);
    if (relay.ok) expect(relay.account).toMatchObject({ email: "x1@privaterelay.appleid.com" });
  });

  it("takes a token once", async () => {
    const t = await token("apple");
    expect((await rig.service.social("apple", t, NONCE, LOGIN)).ok).toBe(true);
    expect(await rig.service.social("apple", t, NONCE, LOGIN)).toEqual({ ok: false, why: "refused" });
  });

  it("checks issuer, audience, expiry, nonce, signature and algorithm", async () => {
    const now = Math.floor(Date.now() / 1000);
    const bad: [string, Promise<string>][] = [
      ["issuer", token("apple", { iss: "https://evil.example" })],
      ["audience", token("apple", { aud: "com.other.app" })],
      ["audience list", token("apple", { aud: ["a", "b"] })],
      ["expired", token("apple", { exp: now - 3600 })],
      ["no expiry", token("apple", { exp: undefined })],
      ["future", token("apple", { iat: now + 3600 })],
      ["nonce", token("apple", { nonce: "m".repeat(24) })],
      ["no subject", token("apple", { sub: "" })],
      ["signature", token("apple", {}, { wrongKey: true })],
      ["key id", token("apple", {}, { kid: "zzz" })],
      ["alg none", token("apple", {}, { alg: "none" })],
      ["alg HS256", token("apple", {}, { alg: "HS256" })],
    ];
    for (const [name, t] of bad) expect(await rig.service.social("apple", await t, NONCE, LOGIN), name).toEqual({ ok: false, why: "refused" });
    expect(await rig.service.social("apple", await token("google"), NONCE, LOGIN)).toEqual({ ok: false, why: "refused" });
    expect(await rig.service.social("google", await token("apple"), NONCE, LOGIN)).toEqual({ ok: false, why: "refused" });
    expect(await rig.service.social("google", await token("google", { email_verified: false }), NONCE, LOGIN)).toEqual({ ok: false, why: "no-email" });
    expect(await rig.service.social("apple", "garbage", NONCE, LOGIN)).toEqual({ ok: false, why: "refused" });
    expect(await rig.service.social("apple", 7, NONCE, LOGIN)).toEqual({ ok: false, why: "bad-input" });
  });

  it("throws away an unconfirmed password sign-up for an address a provider vouches for", async () => {
    await rig.service.signup(email, "attacker chosen password");
    const attackerCode = codeIn(rig.mail[0]);
    expect((await rig.service.social("google", await token("google"), NONCE, LOGIN)).ok).toBe(true);
    expect((await rig.service.signinPassword(email, "attacker chosen password", OTHER)).ok).toBe(false);
    expect((await rig.service.confirm(email, attackerCode, OTHER)).ok).toBe(false);
  });

  it("keeps only hashes of the subject and the token", async () => {
    const t = await token("apple");
    await rig.service.social("apple", t, NONCE, LOGIN);
    const all = JSON.stringify([...rig.identities.values()].map((i) => i.store.value));
    expect(all).not.toContain(`apple-${n}`);
    expect(all).not.toContain(t.split(".")[2]);
  });
});

describe("export and deletion", () => {
  it("erases the account, its identity links and what apps hold, and reports it", async () => {
    const erased: string[] = [];
    rig = memoryService({ data: { relay: { erase: async (a) => (erased.push(a), { groups: 2 }), export: async () => ({ groups: [] }) } } });
    const r = await signUp();
    await rig.service.social("google", await token("google"), NONCE, OTHER);
    const s = (await rig.service.session(`Bearer ${r.token}`))!;
    const exported = await rig.service.export(s);
    expect(exported.account).toMatchObject({ email, hasPassword: true });
    expect(JSON.stringify(exported)).not.toContain(r.token.split(".")[1]);

    const gone = await rig.service.delete(s);
    expect(gone).toMatchObject({ account: true, sessions: 2, identities: 1, apps: { relay: { groups: 2 } } });
    expect(erased).toEqual([s.account]);
    const name = await accountName(email);
    expect(rig.accounts.get(name)!.store.value).toBeUndefined();
    expect(rig.accounts.get(name)!.store.alarm).toBeUndefined();
    expect([...rig.identities.values()].every((i) => i.store.value === undefined)).toBe(true);
    expect(await rig.service.session(`Bearer ${r.token}`)).toBeUndefined();
    expect(await rig.service.signinPassword(email, PASSWORD, LOGIN)).toEqual({ ok: false, why: "bad" });
  });
});
