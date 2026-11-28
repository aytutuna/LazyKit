import { beforeEach, describe, expect, it } from "vitest";
import type { Entered } from "../../src/account/core/service";
import { canonicalEmail } from "../../src/account/core/email";
import { codeIn, kit, memoryService } from "./memory";

const PASSWORD = "correct horse battery";
const LOGIN = { app: "lazysend", device: "a".repeat(32), label: "laptop" };
const OTHER = { ...LOGIN, device: "b".repeat(32) };
const MIN = 60_000;
const DAY = 86_400_000;
const sha = async (t: string) => Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(t))), (b) => b.toString(16).padStart(2, "0")).join("");

let n = 0;
let email = "";
let rig = memoryService();
beforeEach(() => {
  email = `ada${++n}@example.com`;
  rig = memoryService();
});

const header = (r: Entered) => `Bearer ${(r as { token: string }).token}`;
const lastMail = () => rig.mail[rig.mail.length - 1];

async function signUp(address = email, password = PASSWORD, login = LOGIN) {
  expect(await rig.service.signup(address)).toBe("sent");
  await rig.flush();
  const r = await rig.service.confirm(address, codeIn(lastMail()), password, login);
  if (!r.ok) throw new Error(`confirm ${r.why}`);
  return r;
}

async function askCode(address = email) {
  expect(await rig.service.code(address)).toBe("sent");
  await rig.flush();
  return codeIn(lastMail());
}

// A provider token for the nonce the server issued.
async function token(provider: "apple" | "google", claims: Record<string, unknown> = {}, opts = {}, nonce?: string) {
  const raw = nonce ?? (await rig.service.nonce())!;
  const now = Math.floor(Date.now() / 1000);
  const base =
    provider === "apple"
      ? { iss: "https://appleid.apple.com", aud: "com.lazyscan.lazyscan", sub: `apple-${n}`, email, email_verified: "true", nonce: await sha(raw) }
      : { iss: "https://accounts.google.com", aud: "google-client", sub: `google-${n}`, email, email_verified: true, nonce: raw };
  return { raw, idToken: await kit.sign({ ...base, iat: now, exp: now + 600, ...claims }, opts) };
}

async function social(provider: "apple" | "google", claims: Record<string, unknown> = {}, opts = {}, login = LOGIN, caller = "") {
  const t = await token(provider, claims, opts);
  return rig.service.social(provider, t.idToken, t.raw, login, caller);
}

describe("sign-up", () => {
  it("makes an account from a code and a password, with a random id and a session for its app and device", async () => {
    const r = await signUp();
    const s = await rig.service.session(header(r));
    expect(s).toMatchObject({ app: "lazysend", device: LOGIN.device });
    expect(s!.account).toMatch(/^[0-9a-f]{32}$/);
    expect(s!.account).not.toBe((await sha(email)).slice(0, 32));
    expect(await rig.service.me(s!)).toMatchObject({ email, password: true });
  });

  it("takes the password with the code, so a later sign-up cannot swap it", async () => {
    expect(await rig.service.signup(email)).toBe("sent");
    expect(await rig.service.signup(email)).toBe("sent");
    await rig.flush();
    const r = await rig.service.confirm(email, codeIn(lastMail()), "victim password 1", LOGIN);
    expect(r.ok).toBe(true);
    for (const guess of ["attacker password", "victim password 2"]) expect((await rig.service.signinPassword(email, guess, OTHER)).ok).toBe(false);
    expect((await rig.service.signinPassword(email, "victim password 1", OTHER)).ok).toBe(true);
  });

  it("does not spend the code on a bad password", async () => {
    await rig.service.signup(email);
    await rig.flush();
    const code = codeIn(lastMail());
    expect(await rig.service.confirm(email, code, "short", LOGIN)).toEqual({ ok: false, why: "bad-password" });
    expect((await rig.service.confirm(email, code, PASSWORD, LOGIN)).ok).toBe(true);
  });

  it("tells an address that has an account so by mail, and sends no code", async () => {
    await signUp();
    const before = rig.mail.length;
    expect(await rig.service.signup(email)).toBe("sent");
    await rig.flush();
    expect(rig.mail).toHaveLength(before + 1);
    expect(lastMail().subject).toBe("You already have a Lazy account");
    expect(lastMail().text).not.toMatch(/\d{6}/);
  });

  it("forgets a sign-up nobody confirmed, but not a newer one", async () => {
    await rig.service.signup(email);
    await rig.flush();
    const [{ core, store }] = [...rig.addresses.values()];
    rig.clock.now += 5 * MIN;
    await rig.service.signup(email);
    rig.clock.now += 6 * MIN;
    await core.sweep();
    expect(store.value?.codes.signup).toBeDefined();
    rig.clock.now += DAY + 3_600_000;
    await core.sweep();
    expect(store.value).toBeUndefined();
  });

  it("refuses an address it cannot read and says plainly when it is not set up", async () => {
    expect(await rig.service.signup("nope")).toBe("bad-email");
    expect(await memoryService({ codeKey: undefined }).service.signup(email)).toBe("not-configured");
    expect(await memoryService({ idKey: undefined }).service.signup(email)).toBe("not-configured");
    const quiet = memoryService({ mailReady: false });
    expect(await quiet.service.signup(email)).toBe("not-configured");
    expect(await quiet.service.code(email)).toBe("not-configured");
  });
});

describe("mail", () => {
  it("passes the same window for every address, so a refusal tells nothing", async () => {
    await signUp();
    rig.clock.now += 11 * MIN;
    const known: string[] = [];
    const unknown: string[] = [];
    for (let i = 0; i < 5; i++) known.push(await rig.service.code(email));
    for (let i = 0; i < 5; i++) unknown.push(await rig.service.code(`nobody${n}@example.com`));
    expect(known).toEqual(unknown);
    expect(known[3]).toBe("limited");
    rig.clock.now += 11 * MIN;
    const a: string[] = [];
    const b: string[] = [];
    for (let i = 0; i < 5; i++) a.push(await rig.service.signup(email));
    for (let i = 0; i < 5; i++) b.push(await rig.service.signup(`fresh${n}@example.com`));
    expect(a).toEqual(b);
  });

  it("allows ten mails a day to one address, and starts again the next day", async () => {
    const seen: string[] = [];
    for (let i = 0; i < 12; i++) {
      seen.push(await rig.service.signup(email));
      rig.clock.now += 11 * MIN;
    }
    expect(seen).toEqual([...Array(10).fill("sent"), "limited", "limited"]);
    rig.clock.now += DAY;
    expect(await rig.service.signup(email)).toBe("sent");
  });

  it("says plainly when the whole service has used its mail for the day", async () => {
    rig.budget.left = 2;
    expect(await rig.service.signup(email)).toBe("sent");
    expect(await rig.service.code(`other${n}@example.com`)).toBe("sent");
    expect(await rig.service.signup(`third${n}@example.com`)).toBe("busy");
    expect(await rig.service.code(`fourth${n}@example.com`)).toBe("busy");
  });

  it("never sends more than three mails in a window, even for parallel requests", async () => {
    await signUp();
    rig.mail.length = 0;
    rig.mailer.delay = 20;
    await Promise.all(Array.from({ length: 30 }, () => rig.service.code(email)));
    await rig.flush();
    expect(rig.mail.length).toBeLessThanOrEqual(3);
  });

  it("takes back the code but keeps the count when the mail is refused", async () => {
    rig.mailer.fail = true;
    expect(await rig.service.signup(email)).toBe("sent");
    await rig.flush();
    const [{ store }] = [...rig.addresses.values()];
    expect(store.value!.codes.signup).toBeUndefined();
    expect(store.value!.sends).toHaveLength(1);
    rig.mailer.fail = false;
    expect(await rig.service.signup(email)).toBe("sent");
    await rig.flush();
    expect(rig.mail).toHaveLength(1);
  });

  it("answers before the mail service does", async () => {
    rig.mailer.delay = 200;
    const t0 = Date.now();
    expect(await rig.service.signup(email)).toBe("sent");
    expect(Date.now() - t0).toBeLessThan(150);
    await rig.flush();
  });

  it("mails the registered address, whatever spelling asked, and counts spellings as one account", async () => {
    await signUp(`Ada.Lovelace+one${n}@gmail.com`);
    rig.mail.length = 0;
    await rig.service.code(`adalovelace+two${n}@googlemail.com`);
    await rig.flush();
    expect(rig.mail[0].to).toBe(`ada.lovelace+one${n}@gmail.com`);
    expect(rig.accounts.size).toBe(1);
    expect(rig.addresses.size).toBe(1);
    expect(canonicalEmail("a.b+c@gmail.com")).toBe("ab@gmail.com");
    expect(canonicalEmail("a.b+c@example.com")).toBe("a.b@example.com");
  });
});

describe("codes", () => {
  it("signs in once with a code", async () => {
    await signUp();
    const code = await askCode();
    expect((await rig.service.signinCode(email, code, OTHER)).ok).toBe(true);
    expect((await rig.service.signinCode(email, code, OTHER)).ok).toBe(false);
  });

  it("answers an unknown address like a known one", async () => {
    expect(await rig.service.code(`nobody${n}@example.com`)).toBe("sent");
    expect(rig.mail).toHaveLength(0);
    expect(await rig.service.signinCode(`nobody${n}@example.com`, "123456", LOGIN)).toEqual({ ok: false, why: "bad" });
  });

  it("gives each caller three tries per code and leaves the owner the rest", async () => {
    await signUp();
    const code = await askCode();
    const wrong = code === "000000" ? "000001" : "000000";
    for (let i = 0; i < 10; i++) expect((await rig.service.signinCode(email, wrong, LOGIN, "stranger")).ok).toBe(false);
    expect((await rig.service.signinCode(email, code, OTHER, "stranger")).ok).toBe(false);
    expect((await rig.service.signinCode(email, code, OTHER, "owner")).ok).toBe(true);
  });

  it("counts parallel guesses before comparing", async () => {
    await signUp();
    const code = await askCode();
    const guesses = Array.from({ length: 30 }, (_, i) => String(100000 + i)).map((g) => (g === code ? "999999" : g));
    const rs = await Promise.all(guesses.map((g, i) => rig.service.signinCode(email, g, LOGIN, `c${i % 6}`)));
    expect(rs.every((r) => !r.ok)).toBe(true);
    const [{ store }] = [...rig.addresses.values()];
    expect(store.value!.codes.signin!.tries).toBeLessThanOrEqual(18);
  });

  it("stops a stranger after ten wrong codes in a day without closing the owner out", async () => {
    await signUp();
    for (let round = 0; round < 5; round++) {
      await askCode();
      for (let i = 0; i < 3; i++) await rig.service.signinCode(email, "000000", LOGIN, "stranger");
      rig.clock.now += 11 * MIN;
    }
    const owner = await askCode();
    expect((await rig.service.signinCode(email, owner, LOGIN, "owner")).ok).toBe(true);
  });

  it("is not shut by a handful of networks: they spend a code, and the owner asks for another", async () => {
    await signUp();
    const good = await askCode();
    for (let net = 0; net < 4; net++) for (let i = 0; i < 3; i++) await rig.service.signinCode(email, "000000", OTHER, `net${net}`);
    expect((await rig.service.signinCode(email, good, LOGIN, "owner")).ok).toBe(true);
    const next = await askCode();
    for (let net = 0; net < 11; net++) for (let i = 0; i < 3; i++) await rig.service.signinCode(email, "000000", OTHER, `more${net}`);
    expect((await rig.service.signinCode(email, next, LOGIN, "owner")).ok).toBe(false);
    rig.clock.now += 11 * MIN;
    expect((await rig.service.signinCode(email, await askCode(), LOGIN, "owner")).ok).toBe(true);
  });

  it("expires a code after ten minutes", async () => {
    await signUp();
    const code = await askCode();
    rig.clock.now += 11 * MIN;
    expect((await rig.service.signinCode(email, code, LOGIN)).ok).toBe(false);
  });

  it("refuses a signup code where a sign-in code is wanted", async () => {
    await rig.service.signup(email);
    await rig.flush();
    expect((await rig.service.signinCode(email, codeIn(lastMail()), LOGIN)).ok).toBe(false);
  });

  it("keeps no code in the clear", async () => {
    await signUp();
    const code = await askCode();
    const [{ store }] = [...rig.addresses.values()];
    expect(JSON.stringify(store.value)).not.toContain(code);
  });
});

describe("passwords", () => {
  it("answers a wrong password and an unknown address alike, locked included", async () => {
    await signUp();
    const real: Entered[] = [];
    const none: Entered[] = [];
    for (let i = 0; i < 11; i++) real.push(await rig.service.signinPassword(email, "wrong password!", LOGIN, "stranger"));
    for (let i = 0; i < 11; i++) none.push(await rig.service.signinPassword(`nobody${n}@example.com`, "wrong password!", LOGIN, "stranger"));
    expect(real).toEqual(none);
    expect(real[10]).toEqual({ ok: false, why: "locked" });
  });

  it("locks a stranger without locking the owner out", async () => {
    await signUp();
    for (let i = 0; i < 12; i++) await rig.service.signinPassword(email, `wrong password ${i}`, LOGIN, "stranger");
    expect((await rig.service.signinPassword(email, PASSWORD, OTHER, "owner")).ok).toBe(true);
    expect((await rig.service.signinPassword(email, PASSWORD, OTHER, "stranger")).ok).toBe(false);
    rig.clock.now += 16 * MIN;
    expect((await rig.service.signinPassword(email, PASSWORD, OTHER, "stranger")).ok).toBe(true);
  });

  it("stays open to the owner while many networks guess, slows the answers, and locks everyone only past four hundred tries", async () => {
    await signUp();
    const [{ core }] = [...rig.addresses.values()];
    for (let net = 0; net < 5; net++) for (let i = 0; i < 10; i++) await rig.service.signinPassword(email, "wrong password!!", OTHER, `net${net}`);
    expect((await rig.service.signinPassword(email, PASSWORD, LOGIN, "owner")).ok).toBe(true);
    expect(await core.passwordTry("far")).toBe("slow");
    for (let net = 5; net < 45; net++) for (let i = 0; i < 10; i++) await core.passwordTry(`net${net}`);
    expect(await rig.service.signinPassword(email, PASSWORD, LOGIN, "owner2")).toEqual({ ok: false, why: "locked" });
    rig.clock.now += 16 * MIN;
    expect((await rig.service.signinPassword(email, PASSWORD, LOGIN, "owner2")).ok).toBe(true);
  });

  it("stores a salted record and never the password", async () => {
    const r = await signUp();
    const s = (await rig.service.session(header(r)))!;
    const { store } = rig.accounts.get(s.account)!;
    expect(store.value!.password).toMatchObject({ v: 1, stages: 1 });
    expect(JSON.stringify(store.value)).not.toContain(PASSWORD);
  });
});

describe("sessions and re-checks", () => {
  it("lapses after thirty idle days, and at 180 days however busy", async () => {
    const r = await signUp();
    rig.clock.now += 20 * DAY;
    expect(await rig.service.session(header(r))).toBeTruthy();
    rig.clock.now += 20 * DAY;
    expect(await rig.service.session(header(r))).toBeTruthy();
    for (let i = 0; i < 6; i++) {
      rig.clock.now += 25 * DAY;
      await rig.service.session(header(r));
    }
    expect(await rig.service.session(header(r))).toBeUndefined();
  });

  it("counts an open connection as use, but not past idle or absolute expiry", async () => {
    const r = await signUp();
    const s = (await rig.service.session(header(r)))!;
    expect(await rig.service.alive(s.account, s.session)).toBe(true);
    for (let i = 0; i < 5; i++) {
      rig.clock.now += 25 * DAY;
      expect(await rig.service.alive(s.account, s.session)).toBe(true);
    }
    rig.clock.now += 40 * DAY;
    expect(await rig.service.alive(s.account, s.session)).toBe(false);
    const idle = await signUp(`idle${n}@example.com`);
    const i = (await rig.service.session(header(idle)))!;
    rig.clock.now += 31 * DAY;
    expect(await rig.service.alive(i.account, i.session)).toBe(false);
  });

  it("refuses forged and cross-account tokens", async () => {
    const a = await signUp();
    const b = await signUp(`grace${n}@example.com`);
    const [name, secret] = (a as { token: string }).token.split(".");
    const [otherName, otherSecret] = (b as { token: string }).token.split(".");
    for (const bad of [null, "", header(a).slice(7), `Bearer ${name}`, `Bearer ${name}.${otherSecret}`, `Bearer ${otherName}.${secret}`, `Bearer ${"0".repeat(32)}.${secret}`, `Bearer ${name}.${secret}.x`]) {
      expect(await rig.service.session(bad), String(bad)).toBeUndefined();
    }
  });

  it("lets a new session change the password and ends the others; the old password is needed later", async () => {
    const a = await signUp();
    const b = await rig.service.signinPassword(email, PASSWORD, OTHER);
    const s = (await rig.service.session(header(b)))!;
    expect(await rig.service.setPassword(s, "short", undefined)).toBe("bad-password");
    expect(await rig.service.setPassword(s, "a brand new password", undefined)).toBe("ok");
    expect(await rig.service.session(header(a))).toBeUndefined();
    rig.clock.now += 11 * MIN;
    const old = (await rig.service.session(header(b)))!;
    expect(old.fresh).toBe(false);
    expect(await rig.service.setPassword(old, "yet another password", undefined)).toBe("bad");
    expect(await rig.service.setPassword(old, "yet another password", "wrong")).toBe("bad");
    expect(await rig.service.setPassword(old, "yet another password", "a brand new password")).toBe("ok");
    await rig.flush();
    expect(rig.mail.filter((m) => m.subject === "Your Lazy password was changed")).toHaveLength(2);
  });

  it("will not let an old session of an Apple-only account set a password or delete it", async () => {
    const r = await social("apple");
    rig.clock.now += 20 * DAY;
    const s = (await rig.service.session(header(r)))!;
    expect(s.fresh).toBe(false);
    expect(await rig.service.setPassword(s, "attacker password", undefined)).toBe("bad");
    expect(await rig.service.delete(s)).toEqual({ ok: false, why: "need-fresh" });
    expect(await rig.service.session(header(r))).toBeTruthy();
  });

  it("makes a first password on an account without one wait for a code from the current address", async () => {
    const r = await social("apple");
    rig.clock.now += 20 * DAY;
    const s = (await rig.service.session(header(r)))!;
    expect(s.fresh).toBe(false);
    expect(await rig.service.setPassword(s, "attacker password", undefined, "123456")).toBe("bad");
    const before = rig.mail.length;
    expect(await rig.service.passwordCode(s)).toBe("sent");
    await rig.flush();
    expect(rig.mail[rig.mail.length - 1].to).toBe(email);
    expect(rig.mail.length).toBe(before + 1);
    const code = codeIn(lastMail());
    expect(await rig.service.setPassword(s, "attacker password", undefined, code === "000000" ? "000001" : "000000")).toBe("bad");
    expect(await rig.service.setPassword(s, "short", undefined, code)).toBe("bad-password");
    expect(await rig.service.setPassword(s, "my own new password", undefined, code)).toBe("ok");
    expect((await rig.service.signinPassword(email, "my own new password", OTHER)).ok).toBe(true);
    expect(await rig.service.setPassword(s, "another new password", undefined, code)).toBe("bad");
  });

  it("asks for a password code at most three times a day, and not of an account that has a password", async () => {
    const a = await signUp();
    expect(await rig.service.passwordCode((await rig.service.session(header(a)))!)).toBe("bad");
    const r = await social("google", { sub: `other-${n}`, email: `g${n}@example.com` });
    const s = (await rig.service.session(header(r)))!;
    for (let i = 0; i < 3; i++) {
      expect(await rig.service.passwordCode(s)).toBe("sent");
      rig.clock.now += 11 * MIN;
    }
    expect(await rig.service.passwordCode(s)).toBe("limited");
  });

  it("makes an old session show the password before deleting", async () => {
    const r = await signUp();
    rig.clock.now += 20 * DAY;
    const s = (await rig.service.session(header(r)))!;
    expect(await rig.service.delete(s)).toEqual({ ok: false, why: "bad" });
    expect(await rig.service.delete(s, "wrong password!")).toEqual({ ok: false, why: "bad" });
    expect((await rig.service.signinPassword(email, PASSWORD, OTHER)).ok).toBe(true);
    expect((await rig.service.delete(s, PASSWORD)).ok).toBe(true);
    expect((await rig.service.signinPassword(email, PASSWORD, OTHER)).ok).toBe(false);
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
  it("makes an account from a verified token for a nonce the server issued", async () => {
    const r = await social("apple");
    expect(r).toMatchObject({ ok: true, account: { email, providers: ["apple"], password: false } });
  });

  it("does not take a nonce read out of the token, or one the server did not issue", async () => {
    const t = await token("apple");
    const claim = JSON.parse(atob(t.idToken.split(".")[1].replace(/-/g, "+").replace(/_/g, "/"))).nonce as string;
    expect((await rig.service.social("apple", t.idToken, claim, LOGIN)).ok).toBe(false);
    const own = "r".repeat(24);
    const made = await token("apple", {}, {}, own);
    expect((await rig.service.social("apple", made.idToken, own, LOGIN)).ok).toBe(false);
    const forged = `${"x".repeat(24)}.${Date.now() + 60_000}.${"0".repeat(64)}`;
    const f = await token("google", {}, {}, forged);
    expect((await rig.service.social("google", f.idToken, forged, LOGIN)).ok).toBe(false);
    expect((await rig.service.social("apple", t.idToken, t.raw, LOGIN)).ok).toBe(true);
  });

  it("lets a nonce expire and be used once", async () => {
    const stale = await token("google");
    rig.clock.now += 11 * MIN;
    expect(await rig.service.social("google", stale.idToken, stale.raw, LOGIN)).toEqual({ ok: false, why: "refused" });
    rig.clock.now -= 11 * MIN;
    const a = await token("apple");
    const again = await token("apple", { sub: `other-${n}`, email: `x${n}@example.com` }, {}, a.raw);
    expect((await rig.service.social("apple", a.idToken, a.raw, LOGIN)).ok).toBe(true);
    expect(await rig.service.social("apple", again.idToken, a.raw, LOGIN)).toEqual({ ok: false, why: "refused" });
  });

  it("takes a token once", async () => {
    const t = await token("apple");
    expect((await rig.service.social("apple", t.idToken, t.raw, LOGIN)).ok).toBe(true);
    expect(await rig.service.social("apple", t.idToken, t.raw, LOGIN)).toEqual({ ok: false, why: "refused" });
  });

  it("signs a known subject in without an email, and counts a private relay address", async () => {
    await social("apple");
    const again = await social("apple", { email: undefined, email_verified: undefined }, {}, OTHER);
    expect(again.ok).toBe(true);
    const relay = await social("apple", { email: `x1${n}@privaterelay.appleid.com`, sub: `relay-${n}` });
    expect(relay).toMatchObject({ ok: true, account: { email: `x1${n}@privaterelay.appleid.com` } });
  });

  it("checks issuer, audience, expiry, nonce, signature and algorithm", async () => {
    const now = Math.floor(Date.now() / 1000);
    const bad: [string, Entered][] = [
      ["issuer", await social("apple", { iss: "https://evil.example" })],
      ["audience", await social("apple", { aud: "com.other.app" })],
      ["audience list", await social("apple", { aud: ["a", "b"] })],
      ["expired", await social("apple", { exp: now - 3600 })],
      ["no expiry", await social("apple", { exp: undefined })],
      ["future", await social("apple", { iat: now + 3600 })],
      ["nonce claim", await social("apple", { nonce: "m".repeat(24) })],
      ["no subject", await social("apple", { sub: "" })],
      ["signature", await social("apple", {}, { wrongKey: true })],
      ["key id", await social("apple", {}, { kid: "zzz" })],
      ["alg none", await social("apple", {}, { alg: "none" })],
      ["alg HS256", await social("apple", {}, { alg: "HS256" })],
      ["google token on apple", await rig.service.social("apple", (await token("google")).idToken, (await token("google")).raw, LOGIN)],
    ];
    for (const [name, r] of bad) expect(r, name).toEqual({ ok: false, why: "refused" });
    expect(await social("google", { email_verified: false })).toEqual({ ok: false, why: "no-email" });
    expect(await rig.service.social("apple", "garbage", (await rig.service.nonce())!, LOGIN)).toEqual({ ok: false, why: "refused" });
    expect(await rig.service.social("apple", 7, "x", LOGIN)).toEqual({ ok: false, why: "bad-input" });
  });

  it("says the provider is unreachable instead of failing", async () => {
    const t = await token("apple");
    const down = memoryService();
    (down.service as any).cfg.keys = { get: async () => { throw new Error("offline"); } };
    const nonce = (await down.service.nonce())!;
    const u = await token("apple", {}, {}, nonce);
    expect(await down.service.social("apple", u.idToken, nonce, LOGIN)).toEqual({ ok: false, why: "unavailable" });
    expect(t.raw).toBeTruthy();
  });

  it("asks the owner of an existing account for a code before linking, and mails a notice after", async () => {
    const a = await signUp();
    const before = rig.mail.length;
    const first = await social("google");
    expect(first).toEqual({ ok: false, why: "link-code" });
    await rig.flush();
    expect(rig.mail).toHaveLength(before + 1);
    expect(lastMail().to).toBe(email);
    const s = (await rig.service.session(header(a)))!;
    expect((await rig.service.me(s))).toMatchObject({ providers: [] });

    expect((await rig.service.confirmLink(email, "000000", OTHER, "stranger")).ok).toBe(false);
    const linked = await rig.service.confirmLink(email, codeIn(lastMail()), LOGIN);
    expect(linked).toMatchObject({ ok: true, account: { providers: ["google"], password: true } });
    await rig.flush();
    expect(lastMail().subject).toBe("A sign-in method was added to your Lazy account");
    const again = await social("google", {}, {}, { ...OTHER, device: "c".repeat(32) });
    expect(again.ok).toBe(true);
  });

  it("does not link when the code is for another purpose or is guessed", async () => {
    await signUp();
    await social("google");
    await rig.flush();
    for (let i = 0; i < 4; i++) expect((await rig.service.confirmLink(email, "123456", OTHER, "stranger")).ok).toBe(false);
    const code = codeIn(lastMail());
    expect((await rig.service.signinCode(email, code, OTHER)).ok).toBe(false);
  });

  it("drops a pending link when the account is deleted before the code is used", async () => {
    const a = await signUp();
    await social("google");
    await rig.flush();
    const code = codeIn(lastMail());
    expect((await rig.service.delete((await rig.service.session(header(a)))!, PASSWORD)).ok).toBe(true);
    expect((await rig.service.confirmLink(email, code, OTHER)).ok).toBe(false);
    expect([...rig.identities.values()].every((i) => i.store.value?.account === undefined)).toBe(true);
  });

  it("keeps only hashes of the subject and the token", async () => {
    const t = await token("apple");
    await rig.service.social("apple", t.idToken, t.raw, LOGIN);
    const all = JSON.stringify([...rig.identities.values()].map((i) => i.store.value));
    expect(all).not.toContain(`apple-${n}`);
    expect(all).not.toContain(t.idToken.split(".")[2]);
  });
});

describe("changing the email", () => {
  const to = (address: string) => rig.mail.filter((m) => m.to === address);
  const lastTo = (address: string) => to(address)[to(address).length - 1];
  const THIEF = { ...LOGIN, device: "c".repeat(32) };

  async function ask(s: NonNullable<Awaited<ReturnType<typeof rig.service.session>>>, from: string, next: string, password?: string) {
    expect(await rig.service.changeEmail(s, next, password)).toBe("sent");
    await rig.flush();
    return { oldCode: codeIn(lastTo(from)), newCode: codeIn(lastTo(next)) };
  }

  it("takes a new session, the password and a code from each mailbox, tells both, and ends the other sessions", async () => {
    const a = await signUp();
    const other = await rig.service.signinPassword(email, PASSWORD, OTHER);
    const s = (await rig.service.session(header(a)))!;
    const next = `grace${n}@example.com`;
    expect(await rig.service.changeEmail(s, next, undefined)).toBe("bad");
    expect(await rig.service.changeEmail(s, next, "wrong password!")).toBe("bad");
    const { oldCode, newCode } = await ask(s, email, next, PASSWORD);
    expect(await rig.service.confirmEmail(s, next, newCode, "x")).toBe("bad");
    expect(await rig.service.confirmEmail(s, next, newCode, "x", "000000")).toBe("bad");
    await ask(s, email, next, PASSWORD);
    const codes = { oldCode: codeIn(lastTo(email)), newCode: codeIn(lastTo(next)) };
    expect(oldCode).toMatch(/^[0-9]{6}$/);
    expect(await rig.service.confirmEmail(s, next, codes.newCode, "x", codes.oldCode)).toBe("ok");
    await rig.flush();
    expect(lastTo(email).text).toContain(`is being changed to ${next}`);
    expect(lastTo(next).text).toContain(`which was ${email}`);
    expect(await rig.service.session(header(other))).toBeUndefined();
    expect(await rig.service.session(header(a))).toBeTruthy();
    expect((await rig.service.signinPassword(next, PASSWORD, OTHER)).ok).toBe(true);
    expect((await rig.service.signinPassword(email, PASSWORD, OTHER)).ok).toBe(false);
    expect(await rig.service.me(s)).toMatchObject({ email: next });
    expect(rig.accounts.size).toBe(1);
  });

  it("is out of reach of a thief who has only the password: he cannot read the old mailbox", async () => {
    const owner = await signUp();
    const stolen = await rig.service.signinPassword(email, PASSWORD, THIEF, "far away");
    const s = (await rig.service.session(header(stolen)))!;
    const next = "thief@evil.test";
    expect(await rig.service.changeEmail(s, next, PASSWORD)).toBe("sent");
    await rig.flush();
    const newCode = codeIn(lastTo(next));
    for (const guess of ["000000", "123456", undefined]) expect(await rig.service.confirmEmail(s, next, newCode, "far away", guess)).toBe("bad");
    expect(await rig.service.me(s)).toMatchObject({ email });
    expect(await rig.service.session(header(owner))).toBeTruthy();
    expect(to(email).filter((m) => /is being changed/.test(m.text))).toHaveLength(0);
  });

  it("asks an account without a password for both mailboxes too", async () => {
    const r = await social("apple");
    const s = (await rig.service.session(header(r)))!;
    const next = `grace${n}@example.com`;
    const { oldCode, newCode } = await ask(s, email, next);
    expect(await rig.service.confirmEmail(s, next, newCode, "x", "000000")).toBe("bad");
    await ask(s, email, next);
    expect(await rig.service.confirmEmail(s, next, codeIn(lastTo(next)), "x", codeIn(lastTo(email)))).toBe("ok");
    expect(oldCode).toMatch(/^[0-9]{6}$/);
  });

  it("is refused to a session that is not new, and asked for only three times a day", async () => {
    const a = await signUp();
    const s = (await rig.service.session(header(a)))!;
    for (let i = 0; i < 3; i++) {
      expect(await rig.service.changeEmail(s, `x${i}${n}@example.com`, PASSWORD)).toBe("sent");
      rig.clock.now += 11 * MIN;
    }
    expect(await rig.service.changeEmail(s, `x9${n}@example.com`, PASSWORD)).toBe("limited");
    rig.clock.now += DAY + MIN;
    const old = (await rig.service.session(header(a)))!;
    expect(await rig.service.changeEmail(old, `y${n}@example.com`, PASSWORD)).toBe("need-fresh");
  });

  it("lets nobody use up another address's mail or crowd its owner out", async () => {
    const a = await signUp();
    const other = await signUp(`grace${n}@example.com`, "somebody else entirely");
    const s = (await rig.service.session(header(other)))!;
    const [{ store }] = [...rig.addresses.values()].filter((x) => x.store.value?.account);
    const before = [...rig.addresses.values()].map((x) => x.store.value?.sends.length);
    for (let i = 0; i < 2; i++) expect(await rig.service.changeEmail(s, email, "somebody else entirely")).toBe("sent");
    await rig.flush();
    expect([...rig.addresses.values()].map((x) => x.store.value?.sends.length).slice(0, 1)).toEqual(before.slice(0, 1));
    expect(store.value!.codes).toBeDefined();
    expect(to(email).filter((m) => /\b[0-9]{6}\b/.test(m.text))).toHaveLength(1);
    expect(a.ok).toBe(true);
    expect(await rig.service.code(email)).toBe("sent");
  });

  it("does not stop the owner of a free address from getting a code because another account asked first", async () => {
    const attacker = await signUp(`attacker${n}@evil.test`, "attacker password 1");
    const owner = await signUp();
    const free = `target${n}@example.com`;
    const sx = (await rig.service.session(header(attacker)))!;
    const sy = (await rig.service.session(header(owner)))!;
    expect(await rig.service.changeEmail(sx, free, "attacker password 1")).toBe("sent");
    await rig.flush();
    const xCode = codeIn(lastTo(free));
    expect(await rig.service.changeEmail(sy, free, PASSWORD)).toBe("sent");
    await rig.flush();
    expect(to(free)).toHaveLength(2);
    const yOld = codeIn(lastTo(email));
    expect(await rig.service.confirmEmail(sy, free, codeIn(lastTo(free)), "x", yOld)).toBe("ok");
    expect(xCode).toMatch(/^[0-9]{6}$/);
  });

  it("does not change the email if the old address cannot be told", async () => {
    const a = await signUp();
    const s = (await rig.service.session(header(a)))!;
    const next = `grace${n}@example.com`;
    const { oldCode, newCode } = await ask(s, email, next, PASSWORD);
    rig.mailer.fail = true;
    expect(await rig.service.confirmEmail(s, next, newCode, "x", oldCode)).toBe("mail-failed");
    rig.mailer.fail = false;
    expect(await rig.service.me(s)).toMatchObject({ email });
    expect((await rig.service.signinPassword(email, PASSWORD, OTHER)).ok).toBe(true);
  });

  it("does not take over an address that has an account", async () => {
    const a = await signUp();
    await signUp(`grace${n}@example.com`, "somebody else entirely");
    const s = (await rig.service.session(header(a)))!;
    const taken = `grace${n}@example.com`;
    const before = rig.mail.length;
    expect(await rig.service.changeEmail(s, taken, PASSWORD)).toBe("sent");
    await rig.flush();
    expect(rig.mail.slice(before).filter((m) => m.to === taken)).toHaveLength(0);
    expect(await rig.service.confirmEmail(s, taken, "123456", "x", codeIn(lastTo(email)))).toBe("bad");
  });
});

describe("export and deletion", () => {
  it("erases the account, its identity links and what apps hold, frees the address and reports it", async () => {
    const erased: string[] = [];
    rig = memoryService({ data: { relay: { erase: async (a) => (erased.push(a), { groups: 2 }), export: async () => ({ groups: [] }) } } });
    const r = await signUp();
    const t = await token("google");
    await rig.service.social("google", t.idToken, t.raw, OTHER);
    await rig.flush();
    await rig.service.confirmLink(email, codeIn(lastMail()), OTHER);
    const s = (await rig.service.session(header(r)))!;
    const exported = await rig.service.export(s);
    expect(exported.account).toMatchObject({ email, hasPassword: true });
    expect(JSON.stringify(exported)).not.toContain((r as { token: string }).token.split(".")[1]);

    const gone = await rig.service.delete(s);
    expect(gone).toMatchObject({ ok: true, deleted: { account: true, sessions: 2, identities: 1, apps: { relay: { groups: 2 } } } });
    expect(erased).toEqual([s.account]);
    expect(rig.accounts.get(s.account)!.store.value).toBeUndefined();
    expect(rig.accounts.get(s.account)!.store.alarm).toBeUndefined();
    expect([...rig.identities.values()].every((i) => i.store.value === undefined)).toBe(true);
    expect([...rig.addresses.values()].every((x) => x.store.value?.account === undefined)).toBe(true);
    expect(await rig.service.session(header(r))).toBeUndefined();
    expect(await rig.service.signinPassword(email, PASSWORD, LOGIN)).toEqual({ ok: false, why: "bad" });
    await rig.service.signup(email);
    await rig.flush();
    const again = await rig.service.confirm(email, codeIn(lastMail()), PASSWORD, LOGIN);
    expect(again.ok).toBe(true);
    expect((await rig.service.session(header(again)))!.account).not.toBe(s.account);
  });
});
