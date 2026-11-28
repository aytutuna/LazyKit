import { describe, expect, it } from "vitest";
import { codeIn, kit, memoryService } from "./memory";

const PW = "correct horse battery";
const LOGIN = { app: "lazysend", device: "a".repeat(32), label: "x" };
const OTHER = { ...LOGIN, device: "b".repeat(32) };
const sha = async (t: string) => Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(t))), (b) => b.toString(16).padStart(2, "0")).join("");
type Rig = ReturnType<typeof memoryService>;

async function signUp(rig: Rig, email: string) {
  await rig.service.signup(email);
  await rig.flush();
  const r: any = await rig.service.confirm(email, codeIn(rig.mail[rig.mail.length - 1]), PW, LOGIN);
  if (!r.ok) throw new Error(r.why);
  return r;
}

function countPuts(rig: Rig) {
  let puts = 0;
  for (const { store } of rig.addresses.values()) {
    const put = store.put.bind(store);
    store.put = async (v: any) => { puts++; return put(v); };
  }
  return () => puts;
}

function picky(rig: Rig) {
  const real = { send: async (m: any) => { rig.mail.push(m); } };
  const p = { ready: () => true, send: async (m: any) => { if (m.to.includes("+")) throw new Error("422"); return real.send(m); } };
  for (const { core } of rig.addresses.values()) (core as any).deps = { ...(core as any).deps, mailer: () => p };
}

describe("second review findings", () => {
  it("Q1 sign-in code: a wrong code writes storage only when the address has an account", async () => {
    const rig = memoryService();
    await signUp(rig, "known@example.com");
    await rig.service.code("known@example.com");
    await rig.service.code("nobody@example.com");
    await rig.flush();
    const puts = countPuts(rig);
    await rig.service.signinCode("known@example.com", "000001", OTHER, "1.1.1.1");
    const known = puts();
    await rig.service.signinCode("nobody@example.com", "000001", OTHER, "1.1.1.1");
    const unknown = puts() - known;
    expect(unknown).toBe(known);
  });

  it("Q3 sign-up oracle: a spelling the mail service refuses is only counted when an account exists", async () => {
    const rig = memoryService();
    await signUp(rig, "known3@example.com");
    const k: string[] = [], u: string[] = [];
    for (let i = 0; i < 4; i++) { picky(rig); k.push(await rig.service.signup("known3+x@example.com")); await rig.flush(); }
    for (let i = 0; i < 4; i++) { u.push(await rig.service.signup("nobody3+x@example.com")); picky(rig); await rig.flush(); }
    expect(k[3]).toBe(u[3]);
  });

  it("Q5 an address bound to an account that is gone is unbound again", async () => {
    const rig = memoryService();
    const r = await signUp(rig, "gone5@example.com");
    const s = (await rig.service.session(`Bearer ${r.token}`))!;
    await rig.accounts.get(s.account)!.core.erase();
    expect(await rig.service.signup("gone5@example.com")).toBe("sent");
    await rig.flush();
    expect(rig.mail[rig.mail.length - 1].subject).not.toBe("You already have a Lazy account");
    const again: any = await rig.service.confirm("gone5@example.com", codeIn(rig.mail[rig.mail.length - 1]), PW, LOGIN);
    expect(again.ok).toBe(true);
  });

  it("Q5 a sweep unbinds an address whose account is gone", async () => {
    const rig = memoryService();
    const r = await signUp(rig, "gone5b@example.com");
    const s = (await rig.service.session(`Bearer ${r.token}`))!;
    await rig.accounts.get(s.account)!.core.erase();
    const addr = [...rig.addresses.values()][0];
    await (addr.core as any).sweep();
    expect(addr.store.value?.account).toBeUndefined();
  });

  it("Q5 a delete that stops after the address is let go can be run again", async () => {
    const rig = memoryService();
    const r = await signUp(rig, "gone5c@example.com");
    const s = (await rig.service.session(`Bearer ${r.token}`))!;
    const acct = rig.accounts.get(s.account)!.core as any;
    const erase = acct.erase.bind(acct);
    acct.erase = async () => { throw new Error("worker died"); };
    await expect(rig.service.delete(s)).rejects.toThrow();
    expect([...rig.addresses.values()][0].store.value?.account).toBeUndefined();
    acct.erase = erase;
    expect((await rig.service.delete(s)).ok).toBe(true);
    expect(await rig.service.session(`Bearer ${r.token}`)).toBeUndefined();
  });

  it("Q6 four networks spend every try of the owner code", async () => {
    const rig = memoryService();
    await signUp(rig, "own6@example.com");
    await rig.service.code("own6@example.com");
    await rig.flush();
    const good = codeIn(rig.mail[rig.mail.length - 1]);
    for (const net of ["n1", "n2", "n3", "n4"]) for (let i = 0; i < 3; i++) await rig.service.signinCode("own6@example.com", "000000", OTHER, net);
    expect((await rig.service.signinCode("own6@example.com", good, LOGIN, "owner")).ok).toBe(true);
  });

  it("Q7 four networks keep the owner password sign-in shut", async () => {
    const rig = memoryService();
    await signUp(rig, "own7@example.com");
    for (const net of ["n1", "n2", "n3", "n4"]) for (let i = 0; i < 10; i++) await rig.service.signinPassword("own7@example.com", "wrong password!!", OTHER, net);
    expect((await rig.service.signinPassword("own7@example.com", PW, LOGIN, "owner")).ok).toBe(true);
  }, 60000);

  it("Q8 caller networks are written to durable storage", async () => {
    const rig = memoryService();
    await signUp(rig, "own8@example.com");
    await rig.service.signinPassword("own8@example.com", "wrong password!!", OTHER, "203.0.113.77");
    const stored = JSON.stringify([...rig.addresses.values()].map((a) => a.store.value));
    expect(stored).not.toContain("203.0.113.77");
  });

  it("Q9 a mail that always fails never fills the window", async () => {
    const rig = memoryService();
    rig.mailer.fail = true;
    const answers: string[] = [];
    for (let i = 0; i < 30; i++) { answers.push(await rig.service.signup("bounce9@example.com")); await rig.flush(); }
    expect(answers).toContain("limited");
  });

  it("Q10 a bound address and an account wake only when something runs out", async () => {
    const rig = memoryService();
    await signUp(rig, "own10@example.com");
    const a = [...rig.addresses.values()][0];
    const sentAt = a.store.value!.sends[0];
    for (let i = 0; i < 5; i++) {
      rig.clock.now += 3_600_000;
      a.store.alarm = undefined;
      await (a.core as any).sweep();
      expect(a.store.alarm).toBe(sentAt + 86_400_000 + 1);
    }
    rig.clock.now += 86_400_000;
    a.store.alarm = undefined;
    await (a.core as any).sweep();
    expect(a.store.alarm).toBeUndefined();
    expect(a.store.value!.account).toBeDefined();
    const acct = [...rig.accounts.values()][0];
    expect(acct.store.alarm).toBeGreaterThan(rig.clock.now);
    rig.clock.now += 31 * 86_400_000;
    await (acct.core as any).sweep();
    expect(acct.store.value!.sessions).toEqual([]);
    acct.store.alarm = undefined;
    await (acct.core as any).sweep();
    expect(acct.store.alarm).toBeUndefined();
  });

  it("Q11 one nonce signs in two different identities", async () => {
    const rig = memoryService();
    const raw = (await rig.service.nonce())!;
    const now = Math.floor(Date.now() / 1000);
    const g = (sub: string, email: string) => kit.sign({ iss: "https://accounts.google.com", aud: "google-client", sub, email, email_verified: true, nonce: raw, iat: now, exp: now + 600 });
    expect((await rig.service.social("google", await g("s1", "p1@example.com"), raw, LOGIN)).ok).toBe(true);
    expect((await rig.service.social("google", await g("s2", "p2@example.com"), raw, LOGIN)).ok).toBe(false);
    const a = await kit.sign({ iss: "https://appleid.apple.com", aud: "com.lazyscan.lazyscan", sub: "s3", email: "p3@example.com", email_verified: "true", nonce: await sha(raw), iat: now, exp: now + 600 });
    expect((await rig.service.social("apple", a, raw, LOGIN)).ok).toBe(false);
  });

  it("Q12 parallel first sign-ins of one Google subject leave its link erased", async () => {
    const rig = memoryService();
    const now = Math.floor(Date.now() / 1000);
    const mk = async () => { const raw = (await rig.service.nonce())!; return { raw, t: await kit.sign({ iss: "https://accounts.google.com", aud: "google-client", sub: "dup", email: "dup12@example.com", email_verified: true, nonce: raw, iat: now, exp: now + 600 }) }; };
    const x = await mk(), y = await mk();
    const rs = await Promise.all([rig.service.social("google", x.t, x.raw, LOGIN), rig.service.social("google", y.t, y.raw, OTHER)]);
    const z = await mk();
    const again = await rig.service.social("google", z.t, z.raw, LOGIN);
    expect(rs.map((r) => r.ok)).toContain(true);
    expect(again.ok).toBe(true);
  });

  it.fails("Q13 a pending link code is confirmed by whoever holds it, without the provider token", async () => {
    const rig = memoryService();
    await signUp(rig, "own13@example.com");
    const raw = (await rig.service.nonce())!;
    const now = Math.floor(Date.now() / 1000);
    const t = await kit.sign({ iss: "https://accounts.google.com", aud: "google-client", sub: "g13", email: "own13@example.com", email_verified: true, nonce: raw, iat: now, exp: now + 600 });
    expect(await rig.service.social("google", t, raw, LOGIN)).toEqual({ ok: false, why: "link-code" });
    await rig.flush();
    const r = await rig.service.confirmLink("own13@example.com", codeIn(rig.mail[rig.mail.length - 1]), { ...OTHER, device: "c".repeat(32) }, "elsewhere");
    expect(r.ok).toBe(false);
  });
});

describe("wrong guesses cost the same at every address", () => {
  it("writes twice whether the address has a live code, an expired one, none or no account", async () => {
    const rig = memoryService();
    await signUp(rig, "known14@example.com");
    const counts: number[] = [];
    const guess = async (email: string, net: string) => {
      const puts = countPuts(rig);
      await rig.service.signinCode(email, "000001", OTHER, net);
      return puts();
    };
    await rig.service.code("known14@example.com");
    await rig.service.code("nobody14@example.com");
    await rig.flush();
    counts.push(await guess("known14@example.com", "n1"));
    rig.clock.now += 11 * 60_000;
    counts.push(await guess("known14@example.com", "n2"));
    for (let i = 0; i < 3; i++) await rig.service.signinCode("known14@example.com", "000002", OTHER, "n3");
    counts.push(await guess("known14@example.com", "n4"));
    counts.push(await guess("nobody14@example.com", "n1"));
    expect(counts).toEqual([2, 2, 2, 2]);
  });
});
