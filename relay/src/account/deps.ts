import { services } from "../services";
import type { Deps } from "./core/store";
import { DEFAULT_STAGES } from "./core/password";
import { resendMailer } from "./resend";

export const stagesOf = (env: Cloudflare.Env) => Math.min(12, Math.max(1, Math.floor(Number(env.PASSWORD_STAGES)) || DEFAULT_STAGES));

// What the sign-in objects take from a Durable Object and the Worker's environment.
export const depsOf = (ctx: DurableObjectState, env: Cloudflare.Env): Deps => ({
  codeKey: () => env.ACCOUNT_KEY,
  mailer: () => services().mailer ?? resendMailer(env),
  defer: (task) => ctx.waitUntil(task),
  accountExists: (id) => env.ACCOUNTS.get(env.ACCOUNTS.idFromName(id)).exists(),
  stages: () => stagesOf(env),
});
