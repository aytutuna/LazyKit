import { DurableObject } from "cloudflare:workers";
import { services } from "../services";
import { AccountCore, type State, type Store } from "./core/account";
import { resendMailer } from "./resend";

export const durableStore = <T>(ctx: DurableObjectState): Store<T> => ({
  get: () => ctx.storage.get<T>("s"),
  put: (v) => ctx.storage.put("s", v),
  erase: () => ctx.storage.deleteAll(),
  scheduleIfNone: async (at) => {
    if ((await ctx.storage.getAlarm()) === null) await ctx.storage.setAlarm(at);
  },
  cancel: () => ctx.storage.deleteAlarm(),
});

// One Durable Object per account: it runs the calls one at a time, which is what the code and
// password try limits rely on, and erasing an account is one deleteAll.
export class Account extends DurableObject<Cloudflare.Env> {
  private core: AccountCore;

  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    this.core = new AccountCore(durableStore<State>(ctx), {
      codeKey: () => env.ACCOUNT_KEY,
      mailer: () => services().mailer ?? resendMailer(env),
    });
    ctx.blockConcurrencyWhile(() => this.core.load());
  }

  startSignup(...a: Parameters<AccountCore["startSignup"]>) {
    return this.core.startSignup(...a);
  }

  startSignin(...a: Parameters<AccountCore["startSignin"]>) {
    return this.core.startSignin(...a);
  }

  confirmSignup(...a: Parameters<AccountCore["confirmSignup"]>) {
    return this.core.confirmSignup(...a);
  }

  signinWithCode(...a: Parameters<AccountCore["signinWithCode"]>) {
    return this.core.signinWithCode(...a);
  }

  signinWithPassword(...a: Parameters<AccountCore["signinWithPassword"]>) {
    return this.core.signinWithPassword(...a);
  }

  signinWithIdentity(...a: Parameters<AccountCore["signinWithIdentity"]>) {
    return this.core.signinWithIdentity(...a);
  }

  check(...a: Parameters<AccountCore["check"]>) {
    return this.core.check(...a);
  }

  sessions(...a: Parameters<AccountCore["sessions"]>) {
    return this.core.sessions(...a);
  }

  revoke(...a: Parameters<AccountCore["revoke"]>) {
    return this.core.revoke(...a);
  }

  setPassword(...a: Parameters<AccountCore["setPassword"]>) {
    return this.core.setPassword(...a);
  }

  me() {
    return this.core.me();
  }

  identityIds() {
    return this.core.identityIds();
  }

  export() {
    return this.core.export();
  }

  erase() {
    return this.core.erase();
  }


  async alarm(): Promise<void> {
    await this.core.sweep();
  }
}
