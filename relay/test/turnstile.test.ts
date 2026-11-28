import { afterEach, describe, expect, it, vi } from "vitest";
import { cloudflareTurnstile } from "../src/account/turnstile";
import { NotConfiguredError } from "../src/services";

const answer = (body: object, status = 200) => vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response(JSON.stringify(body), { status }));
const env = { TURNSTILE_SECRET: "secret" };

afterEach(() => vi.restoreAllMocks());

describe("Turnstile check", () => {
  it("accepts a token made on our hostname for the action asked, and sends the address along", async () => {
    const f = answer({ success: true, hostname: "alazy.app", action: "signup" });
    expect(await cloudflareTurnstile(env).verify("tok", "203.0.113.1", "signup")).toBe(true);
    const form = f.mock.calls[0][1]!.body as FormData;
    expect([form.get("secret"), form.get("response"), form.get("remoteip")]).toEqual(["secret", "tok", "203.0.113.1"]);
  });

  it("refuses another hostname, another action, a failure and a bad answer", async () => {
    for (const body of [{ success: true, hostname: "evil.example", action: "signup" }, { success: true, hostname: "alazy.app", action: "code" }, { success: true, hostname: "alazy.app" }, { success: true, action: "signup" }, { success: false, hostname: "alazy.app", action: "signup" }]) {
      answer(body);
      expect(await cloudflareTurnstile(env).verify("tok", null, "signup"), JSON.stringify(body)).toBe(false);
    }
    answer({ success: true, hostname: "alazy.app", action: "signup" }, 500);
    expect(await cloudflareTurnstile(env).verify("tok", null, "signup")).toBe(false);
  });

  it("takes the hostnames from the config, and says when there is no secret", async () => {
    answer({ success: true, hostname: "alazy.test", action: "signup" });
    expect(await cloudflareTurnstile({ ...env, TURNSTILE_HOSTNAMES: "alazy.app, alazy.test" }).verify("tok", null, "signup")).toBe(true);
    await expect(cloudflareTurnstile({}).verify("tok", null, "signup")).rejects.toBeInstanceOf(NotConfiguredError);
  });
});
