import { env, exports } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { accountName, identityName } from "../src/account/account";
import { MailFailedError, MailNotConfiguredError } from "../src/account/email";
import { useServices } from "../src/services";
import { DEVICE_A, NONCE, account, codeIn, fakes, idToken, sent, signUp, who } from "./helpers";

let EMAIL = "";
const PASSWORD = "correct horse battery";
const LOGIN = { app: "lazysend", device: DEVICE_A, label: "laptop" };
const stubOf = async (email: string) => env.ACCOUNTS.get(env.ACCOUNTS.idFromName(await accountName(email)));
const body = async (r: Response) => (await r.json()) as Record<string, any>;
const empty = (stub: DurableObjectStub<any>) => runInDurableObject(stub, async (_o, state) => expect([...(await state.storage.list())]).toEqual([]));

beforeEach(() => {
  fakes();
  EMAIL = who.email;
});
afterEach(() => useServices());

describe("sign-up", () => {
  it("confirms an address with a code and then signs in", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    expect(sent).toHaveLength(1);
    expect(sent[0].to).toBe(EMAIL);
    const me = await account("me", "GET", undefined, token);
    expect(await body(me)).toMatchObject({ email: EMAIL, password: true, providers: [], sessions: 1 });
  });

  it("makes no account until the code is entered", async () => {
    await account("signup", "POST", { email: EMAIL, password: PASSWORD });
    const r = await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN });
    expect(r.status).toBe(401);
  });

  it("answers the same for an address that already has an account, and sends nothing", async () => {
    await signUp(EMAIL, PASSWORD);
    const r = await account("signup", "POST", { email: EMAIL, password: "another long password" });
    expect(r.status).toBe(202);
    expect(await body(r)).toEqual({ sent: true });
    expect(sent).toHaveLength(1);
    const again = await account("signin", "POST", { email: EMAIL, password: "another long password", ...LOGIN });
    expect(again.status).toBe(401);
  });

  it("treats lookalike spellings of one address as the same account", async () => {
    await signUp("Ada@Example.COM", PASSWORD);
    const r = await account("signin", "POST", { email: " ａｄａ@example.com", password: PASSWORD, ...LOGIN, device: "b".repeat(32) });
    expect(r.status).toBe(200);
  });

  it("refuses bad addresses, weak passwords and unknown apps", async () => {
    expect((await account("signup", "POST", { email: "nope", password: PASSWORD })).status).toBe(400);
    expect((await account("signup", "POST", { email: EMAIL, password: "short" })).status).toBe(400);
    expect((await account("signup", "POST", { email: EMAIL, password: "x".repeat(300) })).status).toBe(400);
    expect((await account("signup", "POST", { email: EMAIL })).status).toBe(400);
    await account("signup", "POST", { email: EMAIL, password: PASSWORD });
    const code = codeIn(sent[0]);
    for (const extra of [{ app: "evil" }, { device: "short" }, { label: "x".repeat(65) }, { app: undefined }]) {
      const r = await account("signup/confirm", "POST", { email: EMAIL, code, ...LOGIN, ...extra });
      expect(r.status, JSON.stringify(extra)).toBe(400);
    }
    // None of those spent the code.
    expect((await account("signup/confirm", "POST", { email: EMAIL, code, ...LOGIN })).status).toBe(200);
  });

  it("says plainly that mail is not set up, and leaves nothing behind", async () => {
    useServices({ mailer: { send: async () => { throw new MailNotConfiguredError("x"); } } });
    const r = await account("signup", "POST", { email: EMAIL, password: PASSWORD });
    expect(r.status).toBe(501);
    fakes();
    expect((await account("signup", "POST", { email: EMAIL, password: PASSWORD })).status).toBe(202);
    expect(sent).toHaveLength(1);
  });

  it("reports a mail service that fails", async () => {
    useServices({ mailer: { send: async () => { throw new MailFailedError("x"); } } });
    expect((await account("signup", "POST", { email: EMAIL, password: PASSWORD })).status).toBe(502);
  });

  it("forgets a sign-up nobody confirmed", async () => {
    await account("signup", "POST", { email: EMAIL, password: PASSWORD });
    const stub = await stubOf(EMAIL);
    await runInDurableObject(stub, async (o: any) => o.alarm());
    await empty(stub);
  });
});

describe("codes", () => {
  const start = async () => {
    await signUp(EMAIL, PASSWORD);
    sent.length = 0;
    await account("code", "POST", { email: EMAIL });
    return codeIn(sent[0]);
  };

  it("signs in with an emailed code, once", async () => {
    const code = await start();
    const r = await account("signin/code", "POST", { email: EMAIL, code, ...LOGIN });
    expect(r.status).toBe(200);
    expect((await account("signin/code", "POST", { email: EMAIL, code, ...LOGIN })).status).toBe(401);
  });

  it("sends nothing for an address without an account, and answers the same", async () => {
    const r = await account("code", "POST", { email: "nobody@example.com" });
    expect(r.status).toBe(202);
    expect(sent).toHaveLength(0);
    expect((await account("signin/code", "POST", { email: "nobody@example.com", code: "123456", ...LOGIN })).status).toBe(401);
  });

  it("gives up on a code after three wrong tries, even for the right one", async () => {
    const code = await start();
    const wrong = code === "000000" ? "000001" : "000000";
    for (let i = 0; i < 3; i++) expect((await account("signin/code", "POST", { email: EMAIL, code: wrong, ...LOGIN })).status).toBe(401);
    expect((await account("signin/code", "POST", { email: EMAIL, code, ...LOGIN })).status).toBe(401);
  });

  it("counts parallel guesses against the same three tries", async () => {
    const code = await start();
    const guesses = Array.from({ length: 12 }, (_, i) => String(100000 + i)).map((g) => (g === code ? "999999" : g));
    const rs = await Promise.all(guesses.map((g) => account("signin/code", "POST", { email: EMAIL, code: g, ...LOGIN })));
    expect(rs.every((r) => r.status === 401)).toBe(true);
    const stub = await stubOf(EMAIL);
    await runInDurableObject(stub, async (_o, state) => {
      const s = (await state.storage.get<any>("s"))!;
      expect(s.codes.signin.tries).toBeLessThanOrEqual(3);
    });
  });

  it("expires a code after ten minutes", async () => {
    const code = await start();
    const stub = await stubOf(EMAIL);
    expect(await stub.signinWithCode(code, LOGIN, Date.now() + 11 * 60_000)).toEqual({ ok: false, why: "bad" });
  });

  it("refuses codes that are not six digits", async () => {
    const code = await start();
    for (const bad of ["12345", "1234567", "abcdef", "", "１２３４５６"]) expect((await account("signin/code", "POST", { email: EMAIL, code: bad, ...LOGIN })).status).toBe(401);
    expect(code).toMatch(/^\d{6}$/);
  });

  it("stops after ten wrong codes in a day, whatever codes are sent", async () => {
    await signUp(EMAIL, PASSWORD);
    const stub = await stubOf(EMAIL);
    let now = Date.now();
    for (let round = 0; round < 4; round++) {
      now += 11 * 60_000;
      await stub.startSignin(now);
      for (let i = 0; i < 3; i++) await stub.signinWithCode("000000", LOGIN, now);
    }
    sent.length = 0;
    now += 11 * 60_000;
    await stub.startSignin(now);
    expect(await stub.signinWithCode(codeIn(sent[0]), LOGIN, now)).toEqual({ ok: false, why: "bad" });
    // A day later the account is open again.
    now += 24 * 3_600_000;
    await stub.startSignin(now);
    expect((await stub.signinWithCode(codeIn(sent[sent.length - 1]), LOGIN, now)).ok).toBe(true);
  });

  it("allows three codes in ten minutes", async () => {
    await signUp(EMAIL, PASSWORD);
    expect((await account("code", "POST", { email: EMAIL })).status).toBe(202);
    expect((await account("code", "POST", { email: EMAIL })).status).toBe(202);
    expect((await account("code", "POST", { email: EMAIL })).status).toBe(429);
  });

  it("does not store the code in the clear", async () => {
    const code = await start();
    const stub = await stubOf(EMAIL);
    await runInDurableObject(stub, async (_o, state) => {
      expect(JSON.stringify(await state.storage.get("s"))).not.toContain(code);
    });
  });

  it("is refused without the server's key", async () => {
    const stub = await stubOf(EMAIL);
    await runInDurableObject(stub, async (o: any) => {
      const key = o.env.ACCOUNT_KEY;
      o.env.ACCOUNT_KEY = undefined;
      try {
        expect(await o.startSignup(EMAIL, { v: 1, stages: 1, salt: "", hash: "" })).toBe("not-configured");
      } finally {
        o.env.ACCOUNT_KEY = key;
      }
    });
  });
});

describe("passwords", () => {
  it("answers a wrong password and an unknown address alike", async () => {
    await signUp(EMAIL, PASSWORD);
    const wrong = await account("signin", "POST", { email: EMAIL, password: "not the password", ...LOGIN });
    const none = await account("signin", "POST", { email: "nobody@example.com", password: PASSWORD, ...LOGIN });
    expect(wrong.status).toBe(401);
    expect(none.status).toBe(401);
    expect(await wrong.text()).toBe(await none.text());
  });

  it("locks password tries after ten misses but still lets a code in", async () => {
    await signUp(EMAIL, PASSWORD);
    for (let i = 0; i < 10; i++) expect((await account("signin", "POST", { email: EMAIL, password: `wrong password ${i}`, ...LOGIN })).status).toBe(401);
    expect((await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN })).status).toBe(429);
    sent.length = 0;
    await account("code", "POST", { email: EMAIL });
    expect((await account("signin/code", "POST", { email: EMAIL, code: codeIn(sent[0]), ...LOGIN })).status).toBe(200);
  });

  it("counts parallel password guesses before checking them", async () => {
    await signUp(EMAIL, PASSWORD);
    const rs = await Promise.all(Array.from({ length: 25 }, (_, i) => account("signin", "POST", { email: EMAIL, password: `guess number ${i}`, ...LOGIN })));
    expect(rs.filter((r) => r.status === 401).length).toBeLessThanOrEqual(10);
    expect(rs.filter((r) => r.status === 429).length).toBeGreaterThanOrEqual(15);
  });

  it("opens the lock again after the window", async () => {
    await signUp(EMAIL, PASSWORD);
    const stub = await stubOf(EMAIL);
    const now = Date.now();
    for (let i = 0; i < 11; i++) await stub.signinWithPassword("wrong password!", LOGIN, now);
    expect(await stub.signinWithPassword(PASSWORD, LOGIN, now)).toEqual({ ok: false, why: "locked" });
    expect((await stub.signinWithPassword(PASSWORD, LOGIN, now + 16 * 60_000)).ok).toBe(true);
  });

  it("stores a salted chained PBKDF2 record and never the password", async () => {
    await signUp(EMAIL, PASSWORD);
    await signUp(who.other, PASSWORD);
    const salts: string[] = [];
    for (const e of [EMAIL, who.other]) {
      await runInDurableObject(await stubOf(e), async (_o, state) => {
        const s = (await state.storage.get<any>("s"))!;
        expect(s.password).toMatchObject({ v: 1, stages: 1 });
        expect(JSON.stringify(s)).not.toContain(PASSWORD);
        salts.push(s.password.salt);
      });
    }
    expect(salts[0]).not.toBe(salts[1]);
  });

  it("changes a password right after signing in, or with the old one", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    const stub = await stubOf(EMAIL);
    expect((await stub.check(token.split(".")[1], Date.now() + 86_400_000))?.fresh).toBe(false);
    expect((await account("password", "POST", { password: "a brand new password" }, token)).status).toBe(204);
    expect((await account("signin", "POST", { email: EMAIL, password: "a brand new password", ...LOGIN })).status).toBe(200);
    expect((await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN })).status).toBe(401);
  });

  it("asks for the old password from a session that is not new", async () => {
    await signUp(EMAIL, PASSWORD);
    const stub = await stubOf(EMAIL);
    const record = { v: 1 as const, stages: 1, salt: "AAAA", hash: "AAAA" };
    expect(await stub.setPassword(record, undefined, false, "x")).toBe("bad");
    expect(await stub.setPassword(record, "not the password", false, "x")).toBe("bad");
    expect(await stub.setPassword(record, PASSWORD, false, "x")).toBe("ok");
  });

  it("ends the other sessions when the password changes", async () => {
    const t1 = await signUp(EMAIL, PASSWORD);
    const t2 = (await body(await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN, device: "b".repeat(32) }))).token as string;
    expect((await account("password", "POST", { password: "a brand new password" }, t2)).status).toBe(204);
    expect((await account("me", "GET", undefined, t1)).status).toBe(401);
    expect((await account("me", "GET", undefined, t2)).status).toBe(200);
  });
});

describe("sessions", () => {
  it("lists sessions with their app and device, and marks the current one", async () => {
    const t1 = await signUp(EMAIL, PASSWORD);
    await account("signin", "POST", { email: EMAIL, password: PASSWORD, app: "lazyprint", device: "c".repeat(32), label: "Phone" });
    const list = (await (await account("sessions", "GET", undefined, t1)).json()) as any[];
    expect(list).toHaveLength(2);
    expect(list.find((s) => s.current)).toMatchObject({ app: "lazysend", device: DEVICE_A });
    expect(list.find((s) => !s.current)).toMatchObject({ app: "lazyprint", device: "c".repeat(32), label: "Phone" });
    expect(JSON.stringify(list)).not.toContain(t1.split(".")[1]);
  });

  it("revokes one session and signs out", async () => {
    const t1 = await signUp(EMAIL, PASSWORD);
    const r2 = await body(await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN, device: "b".repeat(32) }));
    expect((await account(`sessions/${r2.session}`, "DELETE", undefined, t1)).status).toBe(204);
    expect((await account("me", "GET", undefined, r2.token)).status).toBe(401);
    expect((await account(`sessions/${r2.session}`, "DELETE", undefined, t1)).status).toBe(404);
    expect((await account("signout", "POST", {}, t1)).status).toBe(204);
    expect((await account("me", "GET", undefined, t1)).status).toBe(401);
  });

  it("replaces the session of a device that signs in again", async () => {
    const t1 = await signUp(EMAIL, PASSWORD);
    const r = await body(await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN }));
    expect((await account("me", "GET", undefined, t1)).status).toBe(401);
    expect((await account("me", "GET", undefined, r.token)).status).toBe(200);
  });

  it("keeps only a hash of the token", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    await runInDurableObject(await stubOf(EMAIL), async (_o, state) => {
      expect(JSON.stringify(await state.storage.get("s"))).not.toContain(token.split(".")[1]);
    });
  });

  it("refuses forged, truncated and cross-account tokens", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    const other = await signUp(who.other, PASSWORD);
    const [name, secret] = token.split(".");
    const [otherName, otherSecret] = other.split(".");
    for (const bad of ["x", name, `${name}.`, `${name}.${secret.slice(1)}`, `${name}.${otherSecret}`, `${otherName}.${secret}`, `${"0".repeat(64)}.${secret}`, `${name}.${secret}.x`, token.toUpperCase()]) {
      expect((await account("me", "GET", undefined, bad)).status, bad).toBe(401);
    }
    expect((await exports.default.fetch("https://alazy.app/account/v1/me", { headers: { Authorization: token } })).status).toBe(401);
    expect((await exports.default.fetch("https://alazy.app/account/v1/me", { headers: { Authorization: `Basic ${token}` } })).status).toBe(401);
  });

  it("lets a session lapse after thirty idle days and keeps an active one", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    const stub = await stubOf(EMAIL);
    const secret = token.split(".")[1];
    const day = 86_400_000;
    expect(await stub.check(secret, Date.now() + 20 * day)).toBeTruthy();
    expect(await stub.check(secret, Date.now() + 40 * day)).toBeTruthy();
    expect(await stub.check(secret, Date.now() + 80 * day)).toBeUndefined();
    expect((await account("me", "GET", undefined, token)).status).toBe(401);
  });

  it("carries the app and the device the session was made for", async () => {
    const token = await signUp(EMAIL, PASSWORD, "lazyprint", "d".repeat(32));
    const stub = await stubOf(EMAIL);
    expect(await stub.check(token.split(".")[1])).toMatchObject({ app: "lazyprint", device: "d".repeat(32) });
  });

  it("limits how many sessions an account keeps", async () => {
    await signUp(EMAIL, PASSWORD);
    for (let i = 0; i < 25; i++) await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN, device: i.toString(16).padStart(32, "0") });
    expect((await (await stubOf(EMAIL)).me())!.sessions).toBe(20);
  });

  it("needs a session for everything but sign-in", async () => {
    for (const [method, path] of [["GET", "me"], ["GET", "sessions"], ["GET", "export"], ["DELETE", "me"], ["POST", "signout"], ["POST", "password"]]) {
      expect((await account(path, method, method === "POST" ? {} : undefined)).status, `${method} ${path}`).toBe(401);
    }
    expect((await account("me", "PUT", {})).status).toBe(405);
    expect((await account("nothing", "POST", { email: EMAIL, ...LOGIN })).status).toBe(404);
  });

  it("slows one address that asks for too much", async () => {
    const headers = { "CF-Connecting-IP": "203.0.113.50" };
    let last = 0;
    for (let i = 0; i < 12; i++) last = (await account("code", "POST", { email: `n${i}@example.com` }, undefined, headers)).status;
    expect(last).toBe(503);
  });
});

describe("sign in with Apple and Google", () => {
  const apple = async (claims = {}, opts = {}, extra = {}) => account("apple", "POST", { idToken: await idToken("apple", claims, opts), nonce: NONCE, ...LOGIN, ...extra });
  const google = async (claims = {}, opts = {}, extra = {}) => account("google", "POST", { idToken: await idToken("google", claims, opts), nonce: NONCE, ...LOGIN, ...extra });

  it("makes an account from a verified Apple token", async () => {
    const r = await apple();
    expect(r.status).toBe(200);
    const b = await body(r);
    expect(b.account).toMatchObject({ email: EMAIL, providers: ["apple"], password: false });
    expect((await account("me", "GET", undefined, b.token)).status).toBe(200);
  });

  it("makes an account from a verified Google token", async () => {
    const b = await body(await google());
    expect(b.account).toMatchObject({ email: EMAIL, providers: ["google"] });
  });

  it("links Apple, Google and a password to one account by email", async () => {
    await signUp(EMAIL, PASSWORD);
    await apple({}, {}, { device: "c".repeat(32) });
    const b = await body(await google({}, {}, { device: "b".repeat(32) }));
    expect(b.account).toMatchObject({ email: EMAIL, providers: ["apple", "google"], password: true, sessions: 3 });
  });

  it("finds the account by subject when Apple leaves the email out", async () => {
    await apple();
    const r = await apple({ email: undefined, email_verified: undefined }, {}, { device: "b".repeat(32) });
    expect(r.status).toBe(200);
    expect((await body(r)).account.email).toBe(EMAIL);
  });

  it("counts a private relay address as the address", async () => {
    const b = await body(await apple({ email: "x1y2z3@privaterelay.appleid.com", sub: "apple-sub-2" }));
    expect(b.account.email).toBe("x1y2z3@privaterelay.appleid.com");
  });

  it("takes the email in whatever spelling the provider uses", async () => {
    await signUp(EMAIL, PASSWORD);
    const b = await body(await google({ email: "ADA@Example.com" }, {}, { device: "b".repeat(32) }));
    expect(b.account).toMatchObject({ providers: ["google"], password: true });
  });

  it("refuses a token played twice", async () => {
    const token = await idToken("apple");
    const go = () => account("apple", "POST", { idToken: token, nonce: NONCE, ...LOGIN });
    expect((await go()).status).toBe(200);
    expect((await go()).status).toBe(401);
  });

  it("signs a known subject into its own account, never another address's", async () => {
    await apple();
    const r = await apple({ email: who.other });
    expect((await body(r)).account.email).toBe(EMAIL);
  });

  it("checks issuer, audience, expiry, nonce and signature", async () => {
    const now = Math.floor(Date.now() / 1000);
    const cases: [string, Response][] = [
      ["wrong issuer", await apple({ iss: "https://evil.example" })],
      ["google issuer on apple", await apple({ iss: "https://accounts.google.com" })],
      ["wrong audience", await apple({ aud: "com.other.app" })],
      ["audience list without ours", await apple({ aud: ["a", "b"] })],
      ["no audience", await apple({ aud: undefined })],
      ["expired", await apple({ exp: now - 3600 })],
      ["no expiry", await apple({ exp: undefined })],
      ["from the future", await apple({ iat: now + 3600 })],
      ["wrong nonce", await apple({ nonce: "m".repeat(24) })],
      ["no nonce", await apple({ nonce: undefined })],
      ["no subject", await apple({ sub: "" })],
      ["bad signature", await apple({}, { wrongKey: true })],
      ["unknown key", await apple({}, { kid: "zzz" })],
      ["alg none", await apple({}, { alg: "none" })],
      ["alg HS256", await apple({}, { alg: "HS256" })],
      ["short nonce from the app", await apple({ nonce: "short" }, {}, { nonce: "short" })],
      ["google token on apple", await account("apple", "POST", { idToken: await idToken("google"), nonce: NONCE, ...LOGIN })],
      ["apple token on google", await account("google", "POST", { idToken: await idToken("apple"), nonce: NONCE, ...LOGIN })],
      ["google audience", await google({ aud: "another-client" })],
      ["google bad issuer", await google({ iss: "https://accounts.example.com" })],
      ["google expired", await google({ exp: now - 3600 })],
    ];
    for (const [name, r] of cases) expect(r.status, name).toBe(401);
  });

  it("accepts the nonce as the app's SHA-256 too, and a Google issuer without the scheme", async () => {
    const hash = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(NONCE))), (b) => b.toString(16).padStart(2, "0")).join("");
    expect((await apple({ nonce: hash })).status).toBe(200);
    expect((await google({ iss: "accounts.google.com" })).status).toBe(200);
  });

  it("requires a verified email for a new identity", async () => {
    expect((await google({ email_verified: false })).status).toBe(422);
    expect((await google({ email_verified: "false" })).status).toBe(422);
    expect((await google({ email: undefined })).status).toBe(422);
    expect((await google({ email: "not an address" })).status).toBe(422);
    expect((await apple({ email_verified: "true" })).status).toBe(200);
  });

  it("refuses unknown apps and missing fields", async () => {
    expect((await apple({}, {}, { app: "evil" })).status).toBe(400);
    expect((await account("apple", "POST", { nonce: NONCE, ...LOGIN })).status).toBe(400);
    expect((await account("apple", "POST", { idToken: "a.b.c", ...LOGIN })).status).toBe(400);
    expect((await account("apple", "POST", { idToken: "garbage", nonce: NONCE, ...LOGIN })).status).toBe(401);
    expect((await account("apple", "POST", { idToken: "x".repeat(9000), nonce: NONCE, ...LOGIN })).status).toBe(401);
  });

  it("drops an unconfirmed password sign-up when the provider vouches for the address", async () => {
    await account("signup", "POST", { email: EMAIL, password: "attacker chosen password" });
    const attackerCode = codeIn(sent[0]);
    expect((await google()).status).toBe(200);
    const login = (password: string) => account("signin", "POST", { email: EMAIL, password, ...LOGIN, device: "b".repeat(32) });
    expect((await login("attacker chosen password")).status).toBe(401);
    expect((await account("signup/confirm", "POST", { email: EMAIL, code: attackerCode, ...LOGIN })).status).toBe(401);
  });

  it("keeps only hashes of the subject and the token", async () => {
    const token = await idToken("apple");
    await account("apple", "POST", { idToken: token, nonce: NONCE, ...LOGIN });
    const stub = env.IDENTITIES.get(env.IDENTITIES.idFromName(await identityName("apple", who.apple)));
    await runInDurableObject(stub, async (_o, state) => {
      const all = JSON.stringify([...(await state.storage.list())]);
      expect(all).not.toContain(who.apple);
      expect(all).not.toContain(token.split(".")[2]);
    });
  });
});

describe("deleting an account", () => {
  it("needs the confirmation word", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    expect((await account("me", "DELETE", {}, token)).status).toBe(400);
    expect((await account("me", "DELETE", { confirm: "yes" }, token)).status).toBe(400);
    expect((await account("me", "GET", undefined, token)).status).toBe(200);
  });

  it("removes the account, its sessions and its identity links, and the address can start again", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    await account("apple", "POST", { idToken: await idToken("apple"), nonce: NONCE, ...LOGIN, device: "b".repeat(32) });
    await account("google", "POST", { idToken: await idToken("google"), nonce: NONCE, ...LOGIN, device: "c".repeat(32) });
    const r = await account("me", "DELETE", { confirm: "delete" }, token);
    expect(r.status).toBe(200);
    expect((await body(r)).deleted).toMatchObject({ account: true, sessions: 3, identities: 2 });

    await empty(await stubOf(EMAIL));
    for (const [p, sub] of [["apple", who.apple], ["google", who.google]]) {
      await empty(env.IDENTITIES.get(env.IDENTITIES.idFromName(await identityName(p, sub))));
    }
    expect((await account("me", "GET", undefined, token)).status).toBe(401);
    expect((await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN })).status).toBe(401);
    const again = await body(await account("apple", "POST", { idToken: await idToken("apple"), nonce: NONCE, ...LOGIN }));
    expect(again.account).toMatchObject({ providers: ["apple"], password: false, sessions: 1 });
  });

  it("exports what the service holds without secrets", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    await account("google", "POST", { idToken: await idToken("google"), nonce: NONCE, ...LOGIN, device: "b".repeat(32) });
    const b = await body(await account("export", "GET", undefined, token));
    expect(b.account).toMatchObject({ email: EMAIL, hasPassword: true });
    expect(b.account.identities).toHaveLength(1);
    expect(b.account.sessions).toHaveLength(2);
    expect(b.apps.relay).toMatchObject({ groups: [] });
    const text = JSON.stringify(b);
    expect(text).not.toContain(token.split(".")[1]);
    expect(text).not.toContain(who.google);
    expect(text).not.toContain('"hash"');
  });
});
