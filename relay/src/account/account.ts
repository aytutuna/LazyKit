import { DurableObject } from "cloudflare:workers";
import { AccountCore, type AccountState, type Store } from "./core/account";
import { AddressCore, type AddressState } from "./core/address";
import { depsOf } from "./deps";

export const durableStore = <T>(ctx: DurableObjectState): Store<T> => ({
  get: () => ctx.storage.get<T>("s"),
  put: (v) => ctx.storage.put("s", v),
  erase: () => ctx.storage.deleteAll(),
  scheduleIfNone: async (at) => {
    if ((await ctx.storage.getAlarm()) === null) await ctx.storage.setAlarm(at);
  },
  cancel: () => ctx.storage.deleteAlarm(),
});

// One Durable Object per account, named by its random id: it runs the calls one at a time, and
// erasing an account is one deleteAll.
export class Account extends DurableObject<Cloudflare.Env> {
  private core: AccountCore;

  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    this.core = new AccountCore(durableStore<AccountState>(ctx), depsOf(ctx, env));
    ctx.blockConcurrencyWhile(() => this.core.load());
  }

  create(...a: Parameters<AccountCore["create"]>) {
    return this.core.create(...a);
  }

  exists() {
    return this.core.exists();
  }

  addIdentity(...a: Parameters<AccountCore["addIdentity"]>) {
    return this.core.addIdentity(...a);
  }

  open(...a: Parameters<AccountCore["open"]>) {
    return this.core.open(...a);
  }

  verifyPassword(...a: Parameters<AccountCore["verifyPassword"]>) {
    return this.core.verifyPassword(...a);
  }

  check(...a: Parameters<AccountCore["check"]>) {
    return this.core.check(...a);
  }

  alive(...a: Parameters<AccountCore["alive"]>) {
    return this.core.alive(...a);
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

  authorizeEmailChange(...a: Parameters<AccountCore["authorizeEmailChange"]>) {
    return this.core.authorizeEmailChange(...a);
  }

  authorizeDelete(...a: Parameters<AccountCore["authorizeDelete"]>) {
    return this.core.authorizeDelete(...a);
  }

  mailAddress() {
    return this.core.mailAddress();
  }

  setEmail(...a: Parameters<AccountCore["setEmail"]>) {
    return this.core.setEmail(...a);
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

// One per email address, named by a keyed hash of its canonical spelling.
export class Address extends DurableObject<Cloudflare.Env> {
  private core: AddressCore;

  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    this.core = new AddressCore(durableStore<AddressState>(ctx), depsOf(ctx, env));
    ctx.blockConcurrencyWhile(() => this.core.load());
  }

  requestSignup(...a: Parameters<AddressCore["requestSignup"]>) {
    return this.core.requestSignup(...a);
  }

  requestSignin(...a: Parameters<AddressCore["requestSignin"]>) {
    return this.core.requestSignin(...a);
  }

  requestLink(...a: Parameters<AddressCore["requestLink"]>) {
    return this.core.requestLink(...a);
  }

  requestEmail(...a: Parameters<AddressCore["requestEmail"]>) {
    return this.core.requestEmail(...a);
  }

  requestOldCheck(...a: Parameters<AddressCore["requestOldCheck"]>) {
    return this.core.requestOldCheck(...a);
  }

  issueUndo(...a: Parameters<AddressCore["issueUndo"]>) {
    return this.core.issueUndo(...a);
  }

  takeCode(...a: Parameters<AddressCore["takeCode"]>) {
    return this.core.takeCode(...a);
  }

  bindAccount(...a: Parameters<AddressCore["bindAccount"]>) {
    return this.core.bindAccount(...a);
  }

  unbind(...a: Parameters<AddressCore["unbind"]>) {
    return this.core.unbind(...a);
  }

  account() {
    return this.core.account();
  }

  passwordTry(...a: Parameters<AddressCore["passwordTry"]>) {
    return this.core.passwordTry(...a);
  }

  passwordGood(...a: Parameters<AddressCore["passwordGood"]>) {
    return this.core.passwordGood(...a);
  }

  async alarm(): Promise<void> {
    await this.core.sweep();
  }
}

// Counts the mails the whole service has been asked to send today.
export class Budget extends DurableObject<Cloudflare.Env> {
  async take(limit: number, now = Date.now()): Promise<boolean> {
    const day = new Date(now).toISOString().slice(0, 10);
    const kept = await this.ctx.storage.get<{ day: string; count: number }>("s");
    const count = kept?.day === day ? kept.count : 0;
    if (count >= limit) return false;
    await this.ctx.storage.put("s", { day, count: count + 1 });
    if ((await this.ctx.storage.getAlarm()) === null) await this.ctx.storage.setAlarm(now + 2 * 86_400_000);
    return true;
  }

  async alarm(): Promise<void> {
    await this.ctx.storage.deleteAll();
  }
}

// A value that may be used once, until the time it would have lapsed anyway.
export class Spent extends DurableObject<Cloudflare.Env> {
  async take(until: number): Promise<boolean> {
    if ((await this.ctx.storage.get("s")) !== undefined) return false;
    await this.ctx.storage.put("s", until);
    await this.ctx.storage.setAlarm(until);
    return true;
  }

  async alarm(): Promise<void> {
    await this.ctx.storage.deleteAll();
  }
}
