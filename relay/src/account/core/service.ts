import { accountName, identityName, type AccountCore, type Checked, type Login, type Sent } from "./account";
import { sha256 } from "./b64";
import { normalizeEmail } from "./email";
import type { IdentityCore } from "./identity";
import { IdentityRejectedError, ProviderNotConfiguredError, verifyIdToken, type KeySource, type Provider } from "./oidc";
import { hashPassword, passwordOk } from "./password";

type Async<T> = { [K in keyof T]: T[K] extends (...a: infer A) => infer R ? (...a: A) => Promise<Awaited<R>> : never };

// What the service calls on one account or identity; a Durable Object stub, a database row or the
// object itself all fit.
export type AccountPort = Async<Pick<AccountCore, "startSignup" | "startSignin" | "confirmSignup" | "signinWithCode" | "signinWithPassword" | "signinWithIdentity" | "check" | "sessions" | "revoke" | "setPassword" | "me" | "identityIds" | "export" | "erase">>;
export type IdentityPort = Async<Pick<IdentityCore, "account" | "bind" | "use" | "erase">>;

export interface Directory {
  account(name: string): AccountPort;
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
  stages?: number;
}

export type Entered =
  | { ok: true; token: string; session: string; account: unknown }
  | { ok: false; why: "bad" | "locked" | "refused" | "no-email" | "not-configured" | "bad-email" | "bad-password" | "bad-input" };

const ID = /^[0-9a-f]{32}$/;
const TOKEN = /^Bearer ([0-9a-f]{64})\.([A-Za-z0-9_-]{43})$/;

// Sign-in, sessions and deletion for the Lazy account, with no idea where anything is stored.
export class AccountService {
  constructor(
    private dir: Directory,
    private cfg: ServiceConfig,
    private data: Record<string, AppData> = {},
  ) {}

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

  private async entered(r: Awaited<ReturnType<AccountPort["signinWithPassword"]>>, name: string): Promise<Entered> {
    if (!r.ok) return { ok: false, why: r.why };
    return { ok: true, token: r.token, session: r.session, account: await this.dir.account(name).me() };
  }

  // An address that already has an account is answered like a new one and nothing is sent.
  async signup(email: unknown, password: unknown): Promise<Sent | "bad-email" | "bad-password"> {
    const address = normalizeEmail(email);
    if (!address) return "bad-email";
    if (!passwordOk(password)) return "bad-password";
    // Hashed before the lookup, so an address with an account takes as long as a new one.
    const record = await hashPassword(password, this.cfg.stages);
    const r = await this.dir.account(await accountName(address)).startSignup(address, record);
    return r === "exists" ? "sent" : r;
  }

  async code(email: unknown): Promise<Sent | "bad-email"> {
    const address = normalizeEmail(email);
    if (!address) return "bad-email";
    const r = await this.dir.account(await accountName(address)).startSignin();
    return r === "none" ? "sent" : r;
  }

  async confirm(email: unknown, code: unknown, login: Login): Promise<Entered> {
    return this.withCode(email, code, login, (a, c) => a.confirmSignup(c, login));
  }

  async signinCode(email: unknown, code: unknown, login: Login): Promise<Entered> {
    return this.withCode(email, code, login, (a, c) => a.signinWithCode(c, login));
  }

  private async withCode(email: unknown, code: unknown, _login: Login, go: (a: AccountPort, c: string) => ReturnType<AccountPort["signinWithCode"]>): Promise<Entered> {
    const address = normalizeEmail(email);
    if (!address) return { ok: false, why: "bad-email" };
    const name = await accountName(address);
    return this.entered(await go(this.dir.account(name), typeof code === "string" ? code.trim() : ""), name);
  }

  async signinPassword(email: unknown, password: unknown, login: Login): Promise<Entered> {
    const address = normalizeEmail(email);
    if (!address) return { ok: false, why: "bad-email" };
    if (typeof password !== "string" || password.length > 1024) return { ok: false, why: "bad-password" };
    const name = await accountName(address);
    return this.entered(await this.dir.account(name).signinWithPassword(password, login), name);
  }

  // Both providers hand the app an ID token for a nonce the app made. A token is good for one sign-in.
  async social(provider: Provider, idToken: unknown, nonce: unknown, login: Login): Promise<Entered> {
    if (typeof idToken !== "string" || typeof nonce !== "string") return { ok: false, why: "bad-input" };
    let ident;
    try {
      ident = await verifyIdToken(provider, idToken, nonce, provider === "apple" ? this.cfg.appleAudiences : this.cfg.googleAudiences, this.cfg.keys);
    } catch (e) {
      if (e instanceof ProviderNotConfiguredError) return { ok: false, why: "not-configured" };
      if (e instanceof IdentityRejectedError) return { ok: false, why: "refused" };
      throw e;
    }
    const id = await identityName(provider, ident.sub);
    const identity = this.dir.identity(id);
    if (!(await identity.use(await sha256(idToken), ident.exp))) return { ok: false, why: "refused" };

    let name = await identity.account();
    const email = name ? "" : normalizeEmail(ident.email);
    if (!name) {
      if (!email) return { ok: false, why: "no-email" };
      name = await accountName(email);
    }
    const r = await this.dir.account(name).signinWithIdentity(email || "", { id, provider }, login);
    if (r.ok) await identity.bind(name);
    return this.entered(r, name);
  }

  me = (s: Checked) => this.dir.account(s.account).me();
  sessions = (s: Checked) => this.dir.account(s.account).sessions(s.session);
  revoke = (s: Checked, id: string) => this.dir.account(s.account).revoke(id);
  signout = (s: Checked) => this.dir.account(s.account).revoke(s.session);

  async setPassword(s: Checked, password: unknown, current: unknown): Promise<"ok" | "bad" | "locked" | "bad-password"> {
    if (!passwordOk(password)) return "bad-password";
    const record = await hashPassword(password, this.cfg.stages);
    return this.dir.account(s.account).setPassword(record, typeof current === "string" ? current : undefined, s.fresh, s.session);
  }

  async export(s: Checked) {
    const apps: Record<string, unknown> = {};
    for (const [k, d] of Object.entries(this.data)) apps[k] = await d.export(s.account);
    return { account: await this.dir.account(s.account).export(), apps };
  }

  // Everything that hangs off the account goes first, and each step can be run again; the account itself goes last.
  async delete(s: Checked) {
    const acct = this.dir.account(s.account);
    for (const id of await acct.identityIds()) await this.dir.identity(id).erase();
    const apps: Record<string, unknown> = {};
    for (const [k, d] of Object.entries(this.data)) apps[k] = await d.erase(s.account);
    const own = await acct.erase();
    return { account: true, ...own, apps };
  }
}
