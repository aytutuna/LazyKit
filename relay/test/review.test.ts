import { afterEach, describe, expect, it } from "vitest";
import { services, useServices } from "../src/services";
import { account, fakes, signUp, who } from "./helpers";

afterEach(() => useServices());

describe("review findings in workerd", () => {
  it.fails("parallel code requests on one account send at most three mails", async () => {
    fakes();
    await signUp(who.email);
    const mails: unknown[] = [];
    useServices({ mailer: { send: async (m) => { await new Promise((r) => setTimeout(r, 30)); mails.push(m); } }, keys: services().keys });
    await Promise.all(Array.from({ length: 20 }, () => account("code", "POST", { email: who.email })));
    expect(mails.length).toBeLessThanOrEqual(3);
  });
});
