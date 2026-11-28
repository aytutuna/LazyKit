import type { AccountCore, Checked, Login, Reauth } from "./account";
import { identityName } from "./account";
import type { AddressCore, Sent } from "./address";
import { sha256, toBase64Url } from "./b64";
import { canonicalEmail, normalizeEmail } from "./email";
import type { IdentityCore } from "./identity";
import { IdentityRejectedError, KeysUnavailableError, ProviderNotConfiguredError, verifyIdToken, type KeySource, type Provider } from "./oidc";
import { burnPasswordTime, equal, hashPassword, passwordOk } from "./password";
import { MINUTE, bytes, hmac, randomId } from "./util";

type Async<T> = { [K in keyof T]: T[K] extends (...a: infer A) => infer R ? (...a: A) => Promise<Awaited<R>> : never };

// What the service calls on one address, account or identity; a Durable Object stub, a database
// row or the object itself all fit.
export type AddressPort = Async<Pick<AddressCore, "requestSignup" | "requestSignin" | "requestLink" | "requestEmail" | "requestOldCheck" | "notifyChange" | "takeCode" | "bindAccount" | "unbind" | "account" | "passwordTry" | "passwordGood">>;
export type AccountPort = Async<Pick<AccountCore, "create" | "exists" | "addIdentity" | "open" | "verifyPassword" | "check" | "alive" | "sessions" | "revoke" | "setPassword" | "authorizeDelete" | "authorizeEmailChange" | "ask" | "mailAddress" | "setEmail" | "me" | "identityIds" | "export" | "erase">>;
export type IdentityPort = Async<Pick<IdentityCore, "account" | "bind" | "claim" | "release" | "use" | "erase">>;

export type OncePort = Async<{ take(until: number): boolean }>;

export interface Directory {
  address(name: string): AddressPort;
  // A value that may be spent once, named by a hash of it.
  once(name: string): OncePort;
  account(id: string): AccountPort;
  identity(name: string): IdentityPort;
}

// What an app keeps about an account that deletion and export must reach.
export interface AppData {
  erase(account: string): Promise<unknown>;
  export(account: string): Promise<unknown>;
}

export interface ServiceConfig {
  // Apps that may start a session.
  apps: string[];
  appleAudiences?: string;
  googleAudiences?: string;
  keys: KeySource;
  // Key for the address names; without it nothing can be looked up.
  idKey?: string;
  // Key for the sign-in nonces.
  nonceKey?: string;
  stages?: number;
  // How a slowed answer waits; real time unless a test says otherwise.
  sleep?(ms: number): Promise<void>;
  now?(): number;
}

export type Why = "bad" | "locked" | "refused" | "no-email" | "not-configured" | "bad-email" | "bad-password" | "bad-input" | "limited" | "busy" | "link-code" | "unavailable" | "need-fresh";
export type Entered = { ok: true; token: string; session: string; account: unknown } | { ok: false; why: Why };

const ID = /^[0-9a-f]{32}$/;
const TOKEN = /^Bearer ([0-9a-f]{32})\.([A-Za-z0-9_-]{43})$/;
const NONCE_MS = 10 * MINUTE;

// Sign-in, sessions and deletion for the Lazy account, with no idea where anything is stored.
export class AccountService {
  constructor(
    private dir: Directory,
    private cfg: ServiceConfig,
    private data: Record<string, AppData> = {},
  ) {}

  private clock = () => this.cfg.now?.() ?? Date.now();

  // The app and device a session is being made for, or nothing if the body does not say.
  login(body: Record<string, unknown>): Login | undefined {
    const { app, device, label } = body;
    if (typeof app !== "string" || !this.cfg.apps.includes(app)) return undefined;
    if (typeof device !== "string" || !ID.test(device)) return undefined;
    if (label !== undefined && (typeof label !== "string" || label.length > 64)) return undefined;
    return { app, device, label: ((label as string | undefined) ?? "").replace(/\p{C}/gu, "") };
  }

  // The signed-in account of an Authorization header, or nothing. Other Lazy services call this too.
  async session(authorization: string | null): Promise<Checked | undefined> {
    const m = TOKEN.exec(authorization ?? "");
    if (!m) return undefined;
    const found = await this.dir.account(m[1]).check(m[2]);
    return found && found.account === m[1] ? found : undefined;
  }

  // Whether a session still stands; open connections ask now and then.
  alive(account: string, session: string): Promise<boolean> {
    return this.dir.account(account).alive(session);
  }

  private async address(email: unknown) {
    const typed = normalizeEmail(email);
    if (!typed || !this.cfg.idKey) return undefined;
    const canonical = canonicalEmail(typed);
    return { typed, canonical, port: this.dir.address(await hmac(this.cfg.idKey, `lazy address v1\0${canonical}`)) };
  }

  private async entered(r: { ok: true; token: string; session: string } | { ok: false }, id: string): Promise<Entered> {
    if (!r.ok) return { ok: false, why: "bad" };
    return { ok: true, token: r.token, session: r.session, account: await this.dir.account(id).me() };
  }

  async signup(email: unknown): Promise<Sent | "bad-email"> {
    const a = await this.address(email);
    if (!a) return normalizeEmail(email) ? "not-configured" : "bad-email";
    return a.port.requestSignup(a.typed, a.canonical);
  }

  async code(email: unknown): Promise<Sent | "bad-email"> {
    const a = await this.address(email);
    if (!a) return normalizeEmail(email) ? "not-configured" : "bad-email";
    return a.port.requestSignin(a.canonical);
  }

  // The password arrives with the code, so what is confirmed is what this request holds.
  async confirm(email: unknown, code: unknown, password: unknown, login: Login, caller = ""): Promise<Entered> {
    const a = await this.address(email);
    if (!a) return { ok: false, why: normalizeEmail(email) ? "not-configured" : "bad-email" };
    if (!passwordOk(password)) return { ok: false, why: "bad-password" };
    const taken = await a.port.takeCode("signup", code, a.canonical, caller);
    if (!taken.ok) return { ok: false, why: "bad" };
    const id = randomId();
    const account = this.dir.account(id);
    await account.create(id, taken.mailTo, await hashPassword(password, this.cfg.stages));
    if (!(await a.port.bindAccount(id, taken.mailTo))) {
      await account.erase();
      return { ok: false, why: "bad" };
    }
    return this.entered(await account.open(login), id);
  }

  async signinCode(email: unknown, code: unknown, login: Login, caller = ""): Promise<Entered> {
    const a = await this.address(email);
    if (!a) return { ok: false, why: normalizeEmail(email) ? "not-configured" : "bad-email" };
    const taken = await a.port.takeCode("signin", code, a.canonical, caller);
    if (!taken.ok || !taken.account) return { ok: false, why: "bad" };
    return this.entered(await this.dir.account(taken.account).open(login), taken.account);
  }

  async signinPassword(email: unknown, password: unknown, login: Login, caller = ""): Promise<Entered> {
    const a = await this.address(email);
    if (!a) return { ok: false, why: normalizeEmail(email) ? "not-configured" : "bad-email" };
    if (typeof password !== "string" || password.length > 1024) return { ok: false, why: "bad-password" };
    const turn = await a.port.passwordTry(caller);
    if (turn === "locked") return { ok: false, why: "locked" };
    if (turn === "slow") await (this.cfg.sleep ?? ((ms) => new Promise<void>((r) => setTimeout(r, ms))))(1000);
    const id = await a.port.account();
    if (!id) {
      await burnPasswordTime(this.cfg.stages);
      return { ok: false, why: "bad" };
    }
    const account = this.dir.account(id);
    if (!(await account.verifyPassword(password))) return { ok: false, why: "bad" };
    await a.port.passwordGood(caller);
    return this.entered(await account.open(login), id);
  }

  // A nonce the app takes to Apple or Google: random, stamped with its expiry and signed, so the
  // sign-in that uses it is one this server asked for.
  async nonce(): Promise<string | undefined> {
    if (!this.cfg.nonceKey) return undefined;
    const body = `${toBase64Url(crypto.getRandomValues(new Uint8Array(18)))}.${this.clock() + NONCE_MS}`;
    return `${body}.${await hmac(this.cfg.nonceKey, `lazy nonce v1\0${body}`)}`;
  }

  // Whether a nonce is one this server issued and has not let lapse; nothing is spent.
  async nonceValid(nonce: unknown): Promise<boolean> {
    return typeof nonce === "string" && this.nonceGood(nonce);
  }

  // True once for a nonce this server issued and has not seen spent; for bot checks that ask for a proof bound to one.
  async spendNonce(nonce: unknown): Promise<boolean> {
    if (typeof nonce !== "string" || !(await this.nonceGood(nonce))) return false;
    return this.dir.once(await sha256(`nonce:${nonce}`)).take(this.clock() + NONCE_MS + MINUTE);
  }

  private async nonceGood(nonce: string): Promise<boolean> {
    const m = /^([A-Za-z0-9_-]{24})\.(\d{10,16})\.([0-9a-f]{64})$/.exec(nonce);
    if (!m || !this.cfg.nonceKey || Number(m[2]) < this.clock()) return false;
    return equal(bytes(await hmac(this.cfg.nonceKey, `lazy nonce v1\0${m[1]}.${m[2]}`)), bytes(m[3]));
  }

  // Both providers hand the app an ID token for the nonce this server issued (Apple's carries its
  // SHA-256, Google's the nonce itself). A token and a nonce are good for one sign-in each.
  async social(provider: Provider, idToken: unknown, nonce: unknown, login: Login, caller = ""): Promise<Entered> {
    if (typeof idToken !== "string" || typeof nonce !== "string") return { ok: false, why: "bad-input" };
    if (!(await this.nonceGood(nonce))) return { ok: false, why: "refused" };
    let ident;
    try {
      ident = await verifyIdToken(provider, idToken, provider === "apple" ? await sha256(nonce) : nonce, provider === "apple" ? this.cfg.appleAudiences : this.cfg.googleAudiences, this.cfg.keys, this.clock());
    } catch (e) {
      if (e instanceof ProviderNotConfiguredError) return { ok: false, why: "not-configured" };
      if (e instanceof KeysUnavailableError) return { ok: false, why: "unavailable" };
      if (e instanceof IdentityRejectedError) return { ok: false, why: "refused" };
      throw e;
    }
    const id = await identityName(provider, ident.sub);
    const identity = this.dir.identity(id);
    // The nonce is spent for the whole service, not per subject: one sign-in each.
    if (!(await identity.use(await sha256(idToken), ident.exp)) || !(await this.spendNonce(nonce))) return { ok: false, why: "refused" };

    const known = await identity.account();
    if (known) {
      const r = await this.dir.account(known).open(login);
      if (!r.ok) await identity.erase();
      return r.ok ? this.entered(r, known) : { ok: false, why: "refused" };
    }
    const a = await this.address(ident.email);
    if (!a) return { ok: false, why: ident.email ? (normalizeEmail(ident.email) ? "not-configured" : "no-email") : "no-email" };

    // The address already has an account: the owner says yes by code before anything is linked.
    if (await a.port.account()) {
      const sent = await a.port.requestLink(a.canonical, { id, provider, app: login.app, device: login.device });
      return sent === "sent" ? { ok: false, why: "link-code" } : { ok: false, why: sent === "none" ? "refused" : sent };
    }
    const fresh = randomId();
    const account = this.dir.account(fresh);
    // The link is written before the account that holds it, so deleting the account cannot miss it.
    // If another first sign-in of this subject got there first, this one signs into that account.
    const holder = await identity.claim(fresh);
    if (holder !== fresh) {
      const r = await this.dir.account(holder).open(login);
      return r.ok ? this.entered(r, holder) : { ok: false, why: "refused" };
    }
    await account.create(fresh, a.typed, undefined, { id, provider });
    if (!(await a.port.bindAccount(fresh, a.typed))) {
      await account.erase();
      await identity.release(fresh);
      return { ok: false, why: "refused" };
    }
    return this.entered(await account.open(login), fresh);
  }

  async confirmLink(email: unknown, code: unknown, login: Login, caller = ""): Promise<Entered> {
    const a = await this.address(email);
    if (!a) return { ok: false, why: normalizeEmail(email) ? "not-configured" : "bad-email" };
    const taken = await a.port.takeCode("link", code, a.canonical, caller);
    const link = taken.ok ? (taken.extra as { id: string; provider: string; app: string; device: string } | undefined) : undefined;
    // The code finishes the sign-in that asked for it, on that device and nowhere else.
    if (!taken.ok || !taken.account || !link || link.device !== login.device || link.app !== login.app) return { ok: false, why: "bad" };
    const identity = this.dir.identity(link.id);
    const account = this.dir.account(taken.account);
    await identity.bind(taken.account);
    if ((await account.addIdentity({ id: link.id, provider: link.provider })) === "gone") {
      await identity.erase();
      return { ok: false, why: "bad" };
    }
    return this.entered(await account.open(login), taken.account);
  }

  me = (s: Checked) => this.dir.account(s.account).me();
  sessions = (s: Checked) => this.dir.account(s.account).sessions(s.session);
  revoke = (s: Checked, id: string) => this.dir.account(s.account).revoke(id);
  signout = (s: Checked) => this.dir.account(s.account).revoke(s.session);

  // A first password, on an account that has none, needs a code from the current address, whatever
  // the age of the session; a change needs the old password or a new session.
  async setPassword(s: Checked, password: unknown, current: unknown, code?: unknown, caller = ""): Promise<Reauth | "bad-password"> {
    if (!passwordOk(password)) return "bad-password";
    const account = this.dir.account(s.account);
    let fresh = s.fresh;
    if (!(await account.me())?.password) {
      const email = await account.mailAddress();
      const a = email ? await this.address(email) : undefined;
      const taken = a ? await a.port.takeCode("setpw", code, a.canonical, caller) : undefined;
      if (!taken?.ok || taken.extra !== s.account) return "bad";
      fresh = true;
    }
    return account.setPassword(await hashPassword(password, this.cfg.stages), typeof current === "string" ? current : undefined, fresh, s.session);
  }

  // Mails that code to the current address of an account without a password.
  async passwordCode(s: Checked): Promise<Sent | "bad"> {
    const account = this.dir.account(s.account);
    if ((await account.me())?.password) return "bad";
    if (!(await account.ask("password"))) return "limited";
    const email = await account.mailAddress();
    const a = email ? await this.address(email) : undefined;
    if (!a) return "bad";
    const r = await a.port.requestOldCheck(a.canonical, s.account, "setpw");
    return r === "none" ? "bad" : r;
  }

  // The email is the way back into the account, so a change takes a new session, the password if
  // there is one, and a code from each mailbox: the current address and the new one.
  async changeEmail(s: Checked, email: unknown, password?: unknown): Promise<Sent | "bad-email" | "need-fresh" | "bad" | "locked"> {
    if (!s.fresh) return "need-fresh";
    const a = await this.address(email);
    if (!a) return normalizeEmail(email) ? "not-configured" : "bad-email";
    const account = this.dir.account(s.account);
    const ok = await account.authorizeEmailChange(typeof password === "string" ? password : undefined);
    if (ok !== "ok") return ok;
    if (!(await account.ask("email"))) return "limited";
    const old = await account.mailAddress();
    const previous = old ? await this.address(old) : undefined;
    if (!previous) return "bad";
    const asked = await previous.port.requestOldCheck(previous.canonical, s.account);
    if (asked !== "sent") return asked === "none" ? "bad" : asked;
    return a.port.requestEmail(a.typed, a.canonical, s.account);
  }

  // The old address is told before the account moves, and the change waits until that mail has gone;
  // then every other session ends.
  async confirmEmail(s: Checked, email: unknown, code: unknown, caller = "", oldCode?: unknown): Promise<"ok" | "bad" | "bad-email" | "need-fresh" | "mail-failed"> {
    if (!s.fresh) return "need-fresh";
    const a = await this.address(email);
    if (!a) return "bad-email";
    const account = this.dir.account(s.account);
    const old = await account.mailAddress();
    const previous = old ? await this.address(old) : undefined;
    if (!previous) return "bad";
    const proof = await previous.port.takeCode("emailold", oldCode, previous.canonical, caller);
    if (!proof.ok || proof.extra !== s.account) return "bad";
    const taken = await a.port.takeCode(`email:${s.account}`, code, a.canonical, caller);
    if (!taken.ok || taken.extra !== s.account) return "bad";
    if (!(await previous.port.notifyChange(old!, `The email of your Lazy account is being changed to ${taken.mailTo}. If that was not you, change your password now and sign the other sessions out.`))) return "mail-failed";
    // The old address is let go first, so a stop half way leaves an address unbound, never one bound to nothing.
    if (previous.canonical !== a.canonical) await previous.port.unbind(s.account);
    if (!(await a.port.bindAccount(s.account, taken.mailTo))) {
      await previous.port.bindAccount(s.account, old!);
      return "bad";
    }
    await account.setEmail(taken.mailTo, s.session);
    await a.port.notifyChange(taken.mailTo, `This address is now the email of your Lazy account, which was ${old}.`);
    return "ok";
  }

  async export(s: Checked) {
    const apps: Record<string, unknown> = {};
    for (const [k, d] of Object.entries(this.data)) apps[k] = await d.export(s.account);
    return { account: await this.dir.account(s.account).export(), apps };
  }

  // A session that is not new has to show the password. Everything that hangs off the account goes
  // first, and each step can be run again; the account itself goes last.
  async delete(s: Checked, password?: unknown) {
    const acct = this.dir.account(s.account);
    const ok = await acct.authorizeDelete(s.fresh, typeof password === "string" ? password : undefined);
    if (ok !== "ok") return { ok: false as const, why: ok };
    // The address goes first: a stop half way leaves an account nobody can reach by email, which a
    // second delete finishes, never an address bound to an account that is gone.
    const email = await acct.mailAddress();
    const a = email ? await this.address(email) : undefined;
    if (a) await a.port.unbind(s.account);
    for (const id of await acct.identityIds()) await this.dir.identity(id).erase();
    const apps: Record<string, unknown> = {};
    for (const [k, d] of Object.entries(this.data)) apps[k] = await d.erase(s.account);
    const own = await acct.erase();
    return { ok: true as const, deleted: { account: true, ...own, apps } };
  }
}

