import { describe, expect, it } from "vitest";
import { accountName } from "../../src/account/core/account";
import { codeIn, kit, memoryService } from "./memory";

const LOGIN = { app: "lazysend", device: "a".repeat(32), label: "x" };
const now = () => Math.floor(Date.now() / 1000);

describe("review findings", () => {
  it.fails("P1 a later sign-up for the same address swaps the password the victim confirms", async () => {
    const rig = memoryService();
    const email = "victim@example.com";
    expect(await rig.service.signup(email, "victim password 1")).toBe("sent");
    expect(await rig.service.signup(email, "attacker password")).toBe("sent");
    const r = await rig.service.confirm(email, codeIn(rig.mail[1]), LOGIN);
    expect(r.ok).toBe(true);
    expect((await rig.service.signinPassword(email, "attacker password", { ...LOGIN, device: "b".repeat(32) })).ok).toBe(false);
  });

  it.fails("P2 parallel code requests pass the three-per-window cap", async () => {
    const rig = memoryService();
    const email = "v2@example.com";
    await rig.service.signup(email, "victim password 1");
    await rig.service.confirm(email, codeIn(rig.mail[0]), LOGIN);
    const acct = rig.accounts.get(await accountName(email))!.core as any;
    const sent: unknown[] = [];
    acct.deps.mailer = () => ({ send: async (m: unknown) => { await new Promise((r) => setTimeout(r, 20)); sent.push(m); } });
    await Promise.all(Array.from({ length: 30 }, () => rig.service.code(email)));
    expect(sent.length).toBeLessThanOrEqual(3);
  });

  it.fails("P3 /code: the fourth request is limited only when the account exists", async () => {
    const rig = memoryService();
    const email = "v3@example.com";
    await rig.service.signup(email, "victim password 1");
    await rig.service.confirm(email, codeIn(rig.mail[0]), LOGIN);
    const k = [], u = [];
    for (let i = 0; i < 4; i++) k.push(await rig.service.code(email));
    for (let i = 0; i < 4; i++) u.push(await rig.service.code("nobody3@example.com"));
    expect(k[3]).toBe(u[3]);
  });

  it.fails("P3b sign-up: the fourth request is limited only when no account exists", async () => {
    const rig = memoryService();
    const email = "v3b@example.com";
    await rig.service.signup(email, "victim password 1");
    await rig.service.confirm(email, codeIn(rig.mail[0]), LOGIN);
    const k = [], u = [];
    for (let i = 0; i < 4; i++) k.push(await rig.service.signup(email, "whatever password"));
    for (let i = 0; i < 4; i++) u.push(await rig.service.signup("nobody3b@example.com", "whatever password"));
    expect(k[3]).toBe(u[3]);
  });

  it.fails("P3c password sign-in says locked only for a real account", async () => {
    const rig = memoryService();
    const email = "v3c@example.com";
    await rig.service.signup(email, "victim password 1");
    await rig.service.confirm(email, codeIn(rig.mail[0]), LOGIN);
    let k, u;
    for (let i = 0; i < 11; i++) k = await rig.service.signinPassword(email, "wrong password!", LOGIN);
    for (let i = 0; i < 11; i++) u = await rig.service.signinPassword("nobody3c@example.com", "wrong password!", LOGIN);
    expect(k).toEqual(u);
  }, 60000);

  it.fails("P4 the nonce check passes with the nonce read out of the token", async () => {
    const rig = memoryService();
    const raw = "r".repeat(24);
    const hashed = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(raw))), (b) => b.toString(16).padStart(2, "0")).join("");
    const tok = await kit.sign({ iss: "https://appleid.apple.com", aud: "com.lazyscan.lazyscan", sub: "s4", email: "v4@example.com", email_verified: "true", nonce: hashed, iat: now(), exp: now() + 600 });
    const claims = JSON.parse(atob(tok.split(".")[1].replace(/-/g, "+").replace(/_/g, "/")));
    expect((await rig.service.social("apple", tok, claims.nonce, LOGIN)).ok).toBe(false);
  });

  it.fails("P5 a 20-day-old session deletes the account with no re-check", async () => {
    const rig = memoryService();
    const email = "v5@example.com";
    await rig.service.signup(email, "victim password 1");
    const r = await rig.service.confirm(email, codeIn(rig.mail[0]), LOGIN);
    rig.clock.now += 20 * 86_400_000;
    const s = await rig.service.session(`Bearer ${(r as any).token}`);
    expect(s!.fresh).toBe(false);
    await rig.service.delete(s!).catch(() => undefined);
    expect((await rig.service.signinPassword(email, "victim password 1", LOGIN)).ok).toBe(true);
  });

  it.fails("P6 a stranger keeps the victim's email-code sign-in shut for the day", async () => {
    const rig = memoryService();
    const email = "v6@example.com";
    await rig.service.signup(email, "victim password 1");
    await rig.service.confirm(email, codeIn(rig.mail[0]), LOGIN);
    for (let round = 0; round < 4; round++) {
      await rig.service.code(email);
      for (let i = 0; i < 3; i++) await rig.service.signinCode(email, "000000", LOGIN);
      rig.clock.now += 11 * 60_000;
    }
    const before = rig.mail.length;
    await rig.service.code(email);
    expect((await rig.service.signinCode(email, codeIn(rig.mail[before]), LOGIN)).ok).toBe(true);
  });
});

describe("review finding: stale session", () => {
  it.fails("P7 an old session of an Apple-only account sets a password and signs everyone else out", async () => {
    const rig = memoryService();
    const t = Math.floor(Date.now() / 1000);
    const tok = await kit.sign({ iss: "https://appleid.apple.com", aud: "com.lazyscan.lazyscan", sub: "s7", email: "v7@example.com", email_verified: "true", nonce: "n".repeat(24), iat: t, exp: t + 600 });
    const r: any = await rig.service.social("apple", tok, "n".repeat(24), LOGIN);
    rig.clock.now += 20 * 86_400_000;
    const s = await rig.service.session(`Bearer ${r.token}`);
    expect(s!.fresh).toBe(false);
    expect(await rig.service.setPassword(s!, "attacker password", undefined)).not.toBe("ok");
  });
});
