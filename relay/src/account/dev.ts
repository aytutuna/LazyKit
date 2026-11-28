import { MailFailedError, type Mailer } from "./core/email";
import type { Turnstile } from "./turnstile";

// For a local `wrangler dev` only, switched on by --var on its command line and never in wrangler.jsonc:
// mail goes to a sink (scripts/dev-mail.mjs) that the tests read their codes from, and the bot check
// accepts the token "dev".
export const devMailer = (url: string): Mailer => ({
  ready: () => true,
  async send(mail) {
    const res = await fetch(url, { method: "POST", body: JSON.stringify(mail) });
    if (!res.ok) throw new MailFailedError(`mail sink answered ${res.status}`);
  },
});

export const devTurnstile: Turnstile = { verify: async (token) => token === "dev" };
