import { env } from "cloudflare:workers";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { account, fakes, tweak, who } from "./helpers";

// A real mailer and bot check, so only the dev switches can make the token "dev" count.
beforeEach(() => {
  fakes();
  tweak({ mailer: undefined, turnstile: undefined });
});

afterEach(() => {
  vi.restoreAllMocks();
  delete env.DEV_MAIL_URL;
  delete env.DEV_TURNSTILE;
});

const setDev = (url: string) => {
  env.DEV_MAIL_URL = url;
  env.DEV_TURNSTILE = "1";
};

const asked = () => account("signup", "POST", { email: who.email, turnstile: "dev" });

describe("the dev switches", () => {
  it("do nothing when the mail sink is not on this machine", async () => {
    const f = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("ok"));
    for (const url of ["http://example.com/mail", "https://127.0.0.1/mail", "http://127.0.0.1.evil.test/", "not a url"]) {
      setDev(url);
      expect([403, 501], url).toContain((await asked()).status);
    }
    expect(f.mock.calls.filter(([u]) => String(u).includes("example.com") || String(u).includes("evil.test"))).toEqual([]);
  });

  it("take the token dev and send mail to a sink on this machine", async () => {
    const f = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("ok"));
    setDev("http://127.0.0.1:8790/");
    expect((await asked()).status).toBe(202);
    for (let i = 0; i < 100 && f.mock.calls.length === 0; i++) await new Promise((r) => setTimeout(r, 10));
    expect(String(f.mock.calls[0][0])).toBe("http://127.0.0.1:8790/");
  });

  it("need the bot check switch as well as the sink", async () => {
    vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("ok"));
    env.DEV_MAIL_URL = "http://127.0.0.1:8790/";
    expect([403, 501]).toContain((await asked()).status);
  });
});
