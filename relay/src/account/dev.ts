import { MailFailedError, type Mailer } from "./core/email";
import type { Turnstile } from "./turnstile";

// For a local `wrangler dev` only, switched on by --var on its command line and never in wrangler.jsonc:
// mail goes to a sink (scripts/dev-mail.mjs) that the tests read their codes from, and the bot check
// accepts the token "dev" and the per-address limits are off, so tests can make as many accounts as they like.
// The sink has to be on this machine: DEV_MAIL_URL counts only when its host is loopback, and the bot check and
// limits are loosened only when it does, so a stray variable on a deployed Worker changes nothing.
const LOOPBACK = new Set(["127.0.0.1", "localhost", "[::1]"]);

export function devMailUrl(env: { DEV_MAIL_URL?: string }): string | undefined {
  if (!env.DEV_MAIL_URL) return undefined;
  try {
    const url = new URL(env.DEV_MAIL_URL);
    return url.protocol === "http:" && LOOPBACK.has(url.hostname) ? env.DEV_MAIL_URL : undefined;
  } catch {
    return undefined;
  }
}

export const devMode = (env: { DEV_MAIL_URL?: string; DEV_TURNSTILE?: string }) => Boolean(env.DEV_TURNSTILE) && devMailUrl(env) !== undefined;

export const devMailer = (url: string): Mailer => ({
  ready: () => true,
  async send(mail) {
    const res = await fetch(url, { method: "POST", body: JSON.stringify(mail) });
    if (!res.ok) throw new MailFailedError(`mail sink answered ${res.status}`);
  },
});

export const devTurnstile: Turnstile = { verify: async (token) => token === "dev" };
