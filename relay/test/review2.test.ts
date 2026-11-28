import { afterEach, describe, expect, it } from "vitest";
import { services, useServices } from "../src/services";
import { account, codeIn, fakes, mailed, sent, signUp, who , tweak } from "./helpers";

afterEach(() => useServices());
const PW = "correct horse battery";
const ME = { app: "lazysend", device: "a".repeat(32) };

describe("second review findings in workerd", () => {
  it("parallel code requests on one address send at most three mails", async () => {
    fakes();
    await signUp(who.email, PW);
    const mails: unknown[] = [];
    tweak({ mailer: { send: async (m) => { await new Promise((r) => setTimeout(r, 30)); mails.push(m); } } });
    const rs = await Promise.all(Array.from({ length: 20 }, () => account("code", "POST", { email: who.email })));
    await new Promise((r) => setTimeout(r, 300));
    expect(mails.length).toBeLessThanOrEqual(3);
    expect(rs.filter((r) => r.status === 202).length).toBeLessThanOrEqual(3);
  });

  it("without CF-Connecting-IP a stranger and the owner are one caller", async () => {
    fakes();
    await signUp(who.email, PW);
    for (let i = 0; i < 10; i++) await account("signin", "POST", { email: who.email, password: "wrong password!!", ...ME });
    const owner = await account("signin", "POST", { email: who.email, password: PW, ...ME });
    expect(owner.status).toBe(429);
  });

  it("with CF-Connecting-IP the owner on another network still gets in", async () => {
    fakes();
    await signUp(who.email, PW);
    for (let i = 0; i < 10; i++) await account("signin", "POST", { email: who.email, password: "wrong password!!", ...ME }, undefined, { "CF-Connecting-IP": "2001:db8:1:2::1" });
    const owner = await account("signin", "POST", { email: who.email, password: PW, ...ME }, undefined, { "CF-Connecting-IP": "198.51.100.9" });
    expect(owner.status).toBe(200);
  });

  it("email change tells the old address", async () => {
    fakes();
    const t = await signUp(who.email, PW);
    const before = sent.length;
    expect((await account("email", "POST", { email: who.other, password: PW }, t)).status).toBe(202);
    await mailed(before + 1);
    expect((await account("email/confirm", "POST", { email: who.other, code: codeIn(sent[before]) }, t)).status).toBe(204);
    await new Promise((r) => setTimeout(r, 200));
    expect(sent.slice(before).map((m) => m.to)).toContain(who.email);
  });
});
