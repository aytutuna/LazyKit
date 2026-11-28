import { env, exports } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { identityName } from "../src/account/core/account";
import { canonicalEmail } from "../src/account/core/email";
import { hmac } from "../src/account/core/util";
import { NotConfiguredError, services, useServices } from "../src/services";
import { DEVICE_A, account, inside, codeIn, fakes, idToken, mailed, newNonce, sent, signUp, who , tweak, ipHeader } from "./helpers";

const PASSWORD = "correct horse battery";
const LOGIN = { app: "lazysend", device: DEVICE_A, label: "laptop" };
const DEV_B = "b".repeat(32);
let EMAIL = "";
const body = async (r: Response) => (await r.json()) as Record<string, any>;
const addressStub = async (email: string) => env.ADDRESSES.get(env.ADDRESSES.idFromName(await hmac("test-id-key", `lazy address v1\0${canonicalEmail(email)}`)));
const accountStub = (token: string) => env.ACCOUNTS.get(env.ACCOUNTS.idFromName(token.split(".")[0]));
const empty = (stub: DurableObjectStub) => runInDurableObject(stub, async (_o, state) => expect([...(await state.storage.list())]).toEqual([]));
const apple = async (claims = {}, opts = {}, extra = {}) => {
  const t = await idToken("apple", claims, opts);
  return account("apple", "POST", { idToken: t.idToken, nonce: t.raw, ...LOGIN, ...extra });
};
const google = async (claims = {}, opts = {}, extra = {}) => {
  const t = await idToken("google", claims, opts);
  return account("google", "POST", { idToken: t.idToken, nonce: t.raw, ...LOGIN, ...extra });
};

beforeEach(() => {
  fakes();
  EMAIL = who.email;
});
afterEach(() => useServices());

describe("sign-up over HTTP", () => {
  it("confirms an address with a code and the password, then signs in", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    expect(await body(await account("me", "GET", undefined, token))).toMatchObject({ email: EMAIL, password: true, providers: [], sessions: 1 });
    expect((await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN, device: DEV_B })).status).toBe(200);
    expect((await account("signin", "POST", { email: EMAIL, password: "not the password", ...LOGIN, device: DEV_B })).status).toBe(401);
  });

  it("answers 202 for an address that already has an account and mails it a notice, not a code", async () => {
    await signUp(EMAIL, PASSWORD);
    const r = await account("signup", "POST", { email: EMAIL });
    expect([r.status, await body(r)]).toEqual([202, { sent: true }]);
    await mailed(2);
    expect(sent[1].subject).toBe("You already have a Lazy account");
  });

  it("treats lookalike spellings and plus tags of one address as one account, and mails the registered one", async () => {
    await signUp(`Ada.Ex${who.apple}+one@Example.COM`, PASSWORD);
    const r = await account("signin", "POST", { email: ` ａｄａ.ex${who.apple}+two@example.com`, password: PASSWORD, ...LOGIN, device: DEV_B });
    expect(r.status).toBe(200);
    await account("code", "POST", { email: `ada.ex${who.apple}+three@example.com` });
    await mailed(2);
    expect(sent[1].to).toBe(`ada.ex${who.apple}+one@example.com`);
  });

  it("refuses bad addresses, weak passwords and unknown apps, without spending the code", async () => {
    expect((await account("signup", "POST", { email: "nope" })).status).toBe(400);
    await account("signup", "POST", { email: EMAIL });
    await mailed(1);
    const code = codeIn(sent[0]);
    for (const extra of [{ password: "short" }, { password: "x".repeat(300) }, { password: undefined }, { app: "evil" }, { device: "short" }, { label: "x".repeat(65) }]) {
      const r = await account("signup/confirm", "POST", { email: EMAIL, code, password: PASSWORD, ...LOGIN, ...extra });
      expect(r.status, JSON.stringify(extra)).toBe(400);
    }
    expect((await account("signup/confirm", "POST", { email: EMAIL, code, password: PASSWORD, ...LOGIN })).status).toBe(200);
  });

  it("says plainly that mail is not set up", async () => {
    tweak({ mailer: { send: async () => {}, ready: () => false } });
    expect((await account("signup", "POST", { email: EMAIL })).status).toBe(501);
    expect((await account("code", "POST", { email: EMAIL })).status).toBe(501);
  });

  it("answers 202 even when the mail service fails, and takes the code back but keeps the count", async () => {
    tweak({ mailer: { send: async () => { throw new Error("refused"); } } });
    expect((await account("signup", "POST", { email: EMAIL })).status).toBe(202);
    await new Promise((r) => setTimeout(r, 50));
    const stub = await addressStub(EMAIL);
    await runInDurableObject(stub, async (_o, state) => {
      const s = (await state.storage.get<any>("s"))!;
      expect(s.codes.signup).toBeUndefined();
      expect(s.sends).toHaveLength(1);
    });
  });

  it("sends at most three mails for parallel code requests on one address", async () => {
    await signUp(EMAIL, PASSWORD);
    tweak({ mailer: { send: async (m) => { await new Promise((r) => setTimeout(r, 30)); sent.push(m); } } });
    sent.length = 0;
    await Promise.all(Array.from({ length: 20 }, () => account("code", "POST", { email: EMAIL })));
    await new Promise((r) => setTimeout(r, 200));
    expect(sent.length).toBeLessThanOrEqual(3);
  });

  it("limits one address and one caller the same whether or not the account exists", async () => {
    const real = `real${who.apple}@example.com`;
    await signUp(real, PASSWORD);
    const statuses = async (email: string, ip: string) => {
      const out: number[] = [];
      for (let i = 0; i < 5; i++) out.push((await account("code", "POST", { email }, undefined, { "CF-Connecting-IP": ip })).status);
      return out;
    };
    // The real address has used one send of its window on signing up; the other one asks for it.
    const ghost = `ghost${who.apple}@example.com`;
    await account("signup", "POST", { email: ghost });
    expect(await statuses(ghost, "198.51.100.31")).toEqual(await statuses(real, "198.51.100.32"));
  });
});

describe("bot check over HTTP", () => {
  it("wants a Turnstile token before it sends any mail", async () => {
    const plain = (path: string, body: object) => exports.default.fetch(`https://alazy.app/account/v1/${path}`, { method: "POST", body: JSON.stringify(body), headers: ipHeader });
    for (const path of ["signup", "code"]) {
      expect((await plain(path, { email: EMAIL })).status).toBe(403);
      expect((await plain(path, { email: EMAIL, turnstile: "" })).status).toBe(403);
      expect((await plain(path, { email: EMAIL, turnstile: "forged" })).status).toBe(403);
    }
    expect((await plain("signup", { email: EMAIL, turnstile: "ok-token" })).status).toBe(202);
    expect(sent).toHaveLength(1);
  });

  it("says plainly when Turnstile is not set up", async () => {
    tweak({ turnstile: { verify: async () => { throw new NotConfiguredError("no secret"); } } });
    expect((await account("signup", "POST", { email: EMAIL })).status).toBe(501);
  });

  it("takes an App Attest or Play Integrity proof for a nonce the server issued, once, instead of a token", async () => {
    const make = (nonce: string) => ({ requestDetails: { requestPackageName: "com.lazysend.lazysend", nonce, timestampMillis: String(Date.now()) }, appIntegrity: { appRecognitionVerdict: "PLAY_RECOGNIZED", packageName: "com.lazysend.lazysend" }, deviceIntegrity: { deviceRecognitionVerdict: ["MEETS_DEVICE_INTEGRITY"] } });
    tweak({ integrity: { decode: async () => make(current) } });
    let current = await newNonce();
    const proof = { nonce: current, platform: "google", token: "tok" };
    const plain = (body: object) => exports.default.fetch("https://alazy.app/account/v1/signup", { method: "POST", body: JSON.stringify(body), headers: ipHeader });
    expect((await plain({ email: EMAIL, attest: proof })).status).toBe(202);
    expect((await plain({ email: who.other, attest: proof })).status).toBe(403);
    current = "n".repeat(24) + ".1.0";
    expect((await plain({ email: who.other, attest: { nonce: current, platform: "google", token: "tok" } })).status).toBe(403);
    current = await newNonce();
    tweak({ integrity: { decode: async () => make("another") } });
    expect((await plain({ email: who.other, attest: { nonce: current, platform: "google", token: "tok" } })).status).toBe(403);
  });
});

describe("codes and passwords over HTTP", () => {
  it("lets a stranger fail without shutting the owner out", async () => {
    await signUp(EMAIL, PASSWORD);
    sent.length = 0;
    await account("code", "POST", { email: EMAIL });
    await mailed(1);
    const code = codeIn(sent[0]);
    const stranger = { "CF-Connecting-IP": "198.51.100.5" };
    const owner = { "CF-Connecting-IP": "198.51.100.6" };
    const wrong = code === "000000" ? "000001" : "000000";
    for (let i = 0; i < 6; i++) expect((await account("signin/code", "POST", { email: EMAIL, code: wrong, ...LOGIN }, undefined, stranger)).status).toBe(401);
    expect((await account("signin/code", "POST", { email: EMAIL, code, ...LOGIN }, undefined, owner)).status).toBe(200);
    for (let i = 0; i < 12; i++) await account("signin", "POST", { email: EMAIL, password: `wrong password ${i}`, ...LOGIN }, undefined, stranger);
    expect((await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN, device: DEV_B }, undefined, owner)).status).toBe(200);
  });

  it("answers locked alike for a real and an unknown address", async () => {
    await signUp(EMAIL, PASSWORD);
    const ip = { "CF-Connecting-IP": "198.51.100.7" };
    const last = async (email: string) => {
      let r!: Response;
      for (let i = 0; i < 11; i++) r = await account("signin", "POST", { email, password: "wrong password!", ...LOGIN }, undefined, ip);
      return [r.status, await r.text()];
    };
    expect(await last(`ghost${who.apple}@example.com`)).toEqual(await last(EMAIL));
  });

  it("does not spend the sign-in limit on a route that does not exist", async () => {
    const ip = { "CF-Connecting-IP": "198.51.100.8" };
    for (let i = 0; i < 60; i++) expect((await account("nothing", "POST", { email: EMAIL, ...LOGIN }, undefined, ip)).status).toBe(404);
    expect((await account("signin", "POST", { email: EMAIL, password: "x".repeat(12), ...LOGIN }, undefined, ip)).status).toBe(401);
  });

  it("slows one address that asks for too much", async () => {
    const ip = { "CF-Connecting-IP": "203.0.113.50" };
    let last = 0;
    for (let i = 0; i < 12; i++) last = (await account("code", "POST", { email: `n${i}${who.apple}@example.com` }, undefined, ip)).status;
    expect(last).toBe(503);
  });

  it("keeps the address and the code out of the clear, and the account id random", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    const stub = await addressStub(EMAIL);
    await runInDurableObject(stub, async (_o, state) => {
      const all = JSON.stringify(await state.storage.get("s"));
      expect(all).not.toContain(codeIn(sent[0]));
    });
    expect(token.split(".")[0]).toMatch(/^[0-9a-f]{32}$/);
    expect(token.split(".")[0]).not.toBe((await hmac("test-id-key", EMAIL)).slice(0, 32));
  });
});

describe("sessions over HTTP", () => {
  it("lists sessions, revokes one and signs out", async () => {
    const t1 = await signUp(EMAIL, PASSWORD);
    const r2 = await body(await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN, device: DEV_B }));
    const list = (await (await account("sessions", "GET", undefined, t1)).json()) as any[];
    expect(list).toHaveLength(2);
    expect(JSON.stringify(list)).not.toContain(t1.split(".")[1]);
    expect((await account(`sessions/${r2.session}`, "DELETE", undefined, t1)).status).toBe(204);
    expect((await account("me", "GET", undefined, r2.token)).status).toBe(401);
    expect((await account("signout", "POST", {}, t1)).status).toBe(204);
    expect((await account("me", "GET", undefined, t1)).status).toBe(401);
  });

  it("refuses forged tokens and needs a session for everything but sign-in", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    const [name, secret] = token.split(".");
    for (const bad of ["x", name, `${name}.`, `${name}.${secret.slice(1)}`, `${"0".repeat(32)}.${secret}`, token.toUpperCase()]) {
      expect((await account("me", "GET", undefined, bad)).status, bad).toBe(401);
    }
    expect((await exports.default.fetch("https://alazy.app/account/v1/me", { headers: { Authorization: `Basic ${token}` } })).status).toBe(401);
    for (const [method, path] of [["GET", "me"], ["GET", "sessions"], ["GET", "export"], ["DELETE", "me"], ["POST", "signout"], ["POST", "password"], ["POST", "email"], ["POST", "email/confirm"]]) {
      expect((await account(path, method, method === "POST" ? {} : undefined)).status, `${method} ${path}`).toBe(401);
    }
    expect((await account("me", "PUT", {})).status).toBe(405);
  });

  it("sets a first password only with a code from the current address", async () => {
    const r = await apple();
    const token = (await body(r)).token as string;
    expect((await account("password", "POST", { password: "my own new password" }, token)).status).toBe(401);
    expect((await account("password/code", "POST", { turnstile: "" }, token)).status).toBe(403);
    expect((await account("password/code", "POST", {}, token)).status).toBe(202);
    await mailed(1);
    expect((await account("password", "POST", { password: "my own new password", code: "000000" }, token)).status).toBe(401);
    expect((await account("password", "POST", { password: "my own new password", code: codeIn(sent[0]) }, token)).status).toBe(204);
    expect((await account("password/code", "POST", {}, token)).status).toBe(400);
    expect((await account("signin", "POST", { email: EMAIL, password: "my own new password", ...LOGIN, device: DEV_B })).status).toBe(200);
  });

  it("changes the password from a new session and ends the other sessions", async () => {
    const t1 = await signUp(EMAIL, PASSWORD);
    const t2 = (await body(await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN, device: DEV_B }))).token as string;
    expect((await account("password", "POST", { password: "short" }, t2)).status).toBe(400);
    expect((await account("password", "POST", { password: "a brand new password" }, t2)).status).toBe(204);
    expect((await account("me", "GET", undefined, t1)).status).toBe(401);
    expect((await account("signin", "POST", { email: EMAIL, password: "a brand new password", ...LOGIN })).status).toBe(200);
  });

  it("asks an old session for the password before deleting", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    await inside(accountStub(token), async (o) => {
      o.core.s.sessions[0].created -= 3_600_000;
    });
    expect((await account("me", "DELETE", { confirm: "delete" }, token)).status).toBe(401);
    expect((await account("me", "DELETE", { confirm: "delete", password: "wrong password!" }, token)).status).toBe(401);
    expect((await account("me", "DELETE", { confirm: "delete", password: PASSWORD }, token)).status).toBe(200);
  });
});

describe("sign in with Apple and Google over HTTP", () => {
  it("serves a nonce and makes an account from a token that carries it", async () => {
    const r = await apple();
    expect(r.status).toBe(200);
    expect((await body(r)).account).toMatchObject({ email: EMAIL, providers: ["apple"], password: false });
    expect((await body(await google({}, {}, { device: DEV_B }))).linkRequired).toBe(true);
  });

  it("refuses a nonce the server did not issue, a nonce read from the token, and a token played twice", async () => {
    const t = await idToken("apple");
    const claim = JSON.parse(atob(t.idToken.split(".")[1].replace(/-/g, "+").replace(/_/g, "/"))).nonce;
    expect((await account("apple", "POST", { idToken: t.idToken, nonce: claim, ...LOGIN })).status).toBe(401);
    expect((await account("apple", "POST", { idToken: t.idToken, nonce: t.raw, ...LOGIN })).status).toBe(200);
    expect((await account("apple", "POST", { idToken: t.idToken, nonce: t.raw, ...LOGIN })).status).toBe(401);
    const own = await idToken("apple", {}, { nonce: "r".repeat(24) });
    expect((await account("apple", "POST", { idToken: own.idToken, nonce: own.raw, ...LOGIN })).status).toBe(401);
  });

  it("checks issuer, audience, expiry, signature and algorithm", async () => {
    const now = Math.floor(Date.now() / 1000);
    const cases: [string, Response][] = [
      ["issuer", await apple({ iss: "https://evil.example" })],
      ["audience", await apple({ aud: "com.other.app" })],
      ["expired", await apple({ exp: now - 3600 })],
      ["signature", await apple({}, { wrongKey: true })],
      ["key id", await apple({}, { kid: "zzz" })],
      ["alg none", await apple({}, { alg: "none" })],
      ["google issuer", await google({ iss: "https://accounts.example.com" })],
      ["google audience", await google({ aud: "another-client" })],
    ];
    for (const [name, r] of cases) expect(r.status, name).toBe(401);
    expect((await google({ email_verified: false })).status).toBe(422);
    expect((await account("apple", "POST", { nonce: await newNonce(), ...LOGIN })).status).toBe(400);
    expect((await apple({}, {}, { app: "evil" })).status).toBe(400);
  });

  it("says 503 when the provider's keys cannot be fetched", async () => {
    const t = await idToken("apple");
    tweak({ keys: { get: async () => { throw new Error("offline"); } } });
    const r = await account("apple", "POST", { idToken: t.idToken, nonce: t.raw, ...LOGIN });
    expect([r.status, r.headers.get("Retry-After")]).toEqual([503, "30"]);
  });

  it("asks for an emailed code before linking Apple or Google to an existing account", async () => {
    await signUp(EMAIL, PASSWORD);
    const r = await google();
    expect([r.status, await body(r)]).toEqual([202, { linkRequired: true }]);
    await mailed(2);
    const code = codeIn(sent[1]);
    expect((await account("link/confirm", "POST", { email: EMAIL, code: "000000", ...LOGIN }, undefined, { "CF-Connecting-IP": "198.51.100.9" })).status).toBe(401);
    const ok = await account("link/confirm", "POST", { email: EMAIL, code, ...LOGIN });
    expect(ok.status).toBe(200);
    expect((await body(ok)).account).toMatchObject({ providers: ["google"], password: true });
    await mailed(3);
    expect(sent[2].subject).toBe("A sign-in method was added to your Lazy account");
  });

  it("keeps only hashes of the subject and the token", async () => {
    const t = await idToken("apple");
    await account("apple", "POST", { idToken: t.idToken, nonce: t.raw, ...LOGIN });
    const stub = env.IDENTITIES.get(env.IDENTITIES.idFromName(await identityName("apple", who.apple)));
    await runInDurableObject(stub, async (_o, state) => {
      const all = JSON.stringify([...(await state.storage.list())]);
      expect(all).not.toContain(who.apple);
      expect(all).not.toContain(t.idToken.split(".")[2]);
    });
  });
});

describe("email, export and deletion over HTTP", () => {
  it("moves the account only with the password and a code from each mailbox, and tells both addresses", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    expect((await account("email", "POST", { email: who.other }, token)).status).toBe(401);
    expect((await account("email", "POST", { email: who.other, password: PASSWORD, turnstile: "" }, token)).status).toBe(403);
    expect((await account("email", "POST", { email: who.other, password: PASSWORD }, token)).status).toBe(202);
    await mailed(3);
    const oldCode = codeIn(sent.find((m, i) => i > 0 && m.to === EMAIL)!);
    const newCode = codeIn(sent.find((m) => m.to === who.other)!);
    expect((await account("email/confirm", "POST", { email: who.other, code: newCode }, token)).status).toBe(401);
    expect((await account("email/confirm", "POST", { email: who.other, code: newCode, oldCode }, token)).status).toBe(204);
    await mailed(5);
    expect(sent.some((m) => m.to === EMAIL && m.text.includes(`is being changed to ${who.other}`))).toBe(true);
    expect(sent.some((m) => m.to === who.other && m.text.includes("is now the email"))).toBe(true);
    expect((await account("signin", "POST", { email: who.other, password: PASSWORD, ...LOGIN, device: DEV_B })).status).toBe(200);
    expect((await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN, device: DEV_B })).status).toBe(401);
    expect((await account("email/undo", "POST", { email: EMAIL, code: "000000" })).status).toBe(404);
  });

  it("needs the confirmation word and removes everything the service holds", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    await apple({}, {}, { device: DEV_B });
    await mailed(2);
    await account("link/confirm", "POST", { email: EMAIL, code: codeIn(sent[1]), ...LOGIN, device: DEV_B });
    expect((await account("me", "DELETE", {}, token)).status).toBe(400);
    const r = await account("me", "DELETE", { confirm: "delete" }, token);
    expect(r.status).toBe(200);
    expect((await body(r)).deleted).toMatchObject({ account: true, sessions: 2, identities: 1 });
    await empty(accountStub(token));
    await empty(env.IDENTITIES.get(env.IDENTITIES.idFromName(await identityName("apple", who.apple))));
    await runInDurableObject(await addressStub(EMAIL), async (_o, state) => expect((await state.storage.get<any>("s")).account).toBeUndefined());
    expect((await account("me", "GET", undefined, token)).status).toBe(401);
    expect((await account("signin", "POST", { email: EMAIL, password: PASSWORD, ...LOGIN })).status).toBe(401);
  });

  it("exports what the service holds without secrets", async () => {
    const token = await signUp(EMAIL, PASSWORD);
    const b = await body(await account("export", "GET", undefined, token));
    expect(b.account).toMatchObject({ email: EMAIL, hasPassword: true });
    expect(b.apps.relay).toMatchObject({ groups: [] });
    const text = JSON.stringify(b);
    expect(text).not.toContain(token.split(".")[1]);
    expect(text).not.toContain('"hash"');
  });
});
