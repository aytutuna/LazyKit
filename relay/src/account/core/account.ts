import { sha256, toBase64Url } from "./b64";
import { burnPasswordTime, equal, verifyPassword, type PasswordRecord } from "./password";
import type { Deps, Store } from "./store";
import { DAY, MINUTE, bytes, emptyWindow, windowAdd, windowFull, type Window } from "./util";

const SESSION_IDLE_MS = 30 * DAY;
const SESSION_MAX_MS = 180 * DAY;
const SEEN_REFRESH_MS = 3_600_000;
const FRESH_MS = 10 * MINUTE;
const MAX_SESSIONS = 20;
const REAUTH_WINDOW_MS = 15 * MINUTE;
const REAUTH_FAILS = 10;

export type { Store };

export const identityName = (provider: string, sub: string) => sha256(`lazy identity v1\0${provider}\0${sub}`);

interface Session {
  id: string;
  hash: string;
  app: string;
  device: string;
  label: string;
  created: number;
  seen: number;
}

export interface AccountState {
  id: string;
  email: string;
  created: number;
  password?: PasswordRecord;
  identities: { id: string; provider: string }[];
  sessions: Session[];
  reauth: Window;
}

export interface Login {
  app: string;
  device: string;
  label: string;
}

export type Signed = { ok: true; token: string; session: string } | { ok: false; why: "bad" };

export interface Checked {
  account: string;
  session: string;
  app: string;
  device: string;
  fresh: boolean;
}

export type Reauth = "ok" | "bad" | "locked" | "need-fresh";

// One person's sign-in methods and sessions, under a random id that nothing else is derived from.
// It knows nothing of any app's data; apps ask it who a session belongs to. Counting wrong tries at
// the front door is the address object's job; this one counts only the checks made with a live
// session (changing the password, deleting the account).
export class AccountCore {
  private s?: AccountState;

  constructor(
    private store: Store<AccountState>,
    private deps: Deps,
  ) {}

  async load(): Promise<void> {
    this.s = await this.store.get();
  }

  private clock = () => this.deps.now?.() ?? Date.now();

  private async save(): Promise<void> {
    await this.store.put(this.s!);
    await this.store.scheduleIfNone(this.clock() + DAY);
  }

  // Tells the person by mail that something about their sign-in changed; a failure to send is ignored.
  private notify(subject: string, text: string): void {
    this.deps.defer(this.deps.mailer().send({ to: this.s!.email, subject, text }).catch(() => undefined));
  }

  // False when the id is taken.
  async create(id: string, email: string, password?: PasswordRecord, identity?: { id: string; provider: string }, now = this.clock()): Promise<boolean> {
    if (this.s) return false;
    this.s = { id, email, created: now, password, identities: identity ? [identity] : [], sessions: [], reauth: emptyWindow() };
    await this.save();
    return true;
  }

  async addIdentity(identity: { id: string; provider: string }): Promise<"ok" | "gone"> {
    const s = this.s;
    if (!s) return "gone";
    if (!s.identities.some((i) => i.id === identity.id)) {
      s.identities.push(identity);
      await this.save();
      this.notify("A sign-in method was added to your Lazy account", `${identity.provider === "apple" ? "Sign in with Apple" : "Sign in with Google"} can now sign in to your Lazy account. If that was not you, change your password and remove the sessions you do not know in the app.`);
    }
    return "ok";
  }

  async open(login: Login, now = this.clock()): Promise<Signed> {
    const s = this.s;
    if (!s) return { ok: false, why: "bad" };
    const secret = toBase64Url(crypto.getRandomValues(new Uint8Array(32)));
    const id = toBase64Url(crypto.getRandomValues(new Uint8Array(9)));
    // A device that signs in again replaces its own earlier session for the same app.
    s.sessions = s.sessions.filter((x) => !(x.app === login.app && x.device === login.device));
    s.sessions.push({ id, hash: await sha256(secret), app: login.app, device: login.device, label: login.label, created: now, seen: now });
    if (s.sessions.length > MAX_SESSIONS) s.sessions.sort((a, b) => a.seen - b.seen).splice(0, s.sessions.length - MAX_SESSIONS);
    await this.save();
    return { ok: true, token: `${s.id}.${secret}`, session: id };
  }

  // Takes as long for an account without a password as for one with it.
  async verifyPassword(password: string): Promise<boolean> {
    const rec = this.s?.password;
    if (!rec) {
      await burnPasswordTime(this.deps.stages?.());
      return false;
    }
    return verifyPassword(password, rec);
  }

  async check(secret: string, now = this.clock()): Promise<Checked | undefined> {
    const s = this.s;
    if (!s) return undefined;
    const hash = bytes(await sha256(secret));
    const found = s.sessions.find((x) => equal(bytes(x.hash), hash));
    if (!found) return undefined;
    if (now - found.seen > SESSION_IDLE_MS || now - found.created > SESSION_MAX_MS) {
      s.sessions = s.sessions.filter((x) => x !== found);
      await this.save();
      return undefined;
    }
    if (now - found.seen >= SEEN_REFRESH_MS) {
      found.seen = now;
      await this.save();
    }
    return { account: s.id, session: found.id, app: found.app, device: found.device, fresh: now - found.created < FRESH_MS };
  }

  exists(): boolean {
    return this.s !== undefined;
  }

  // Whether a session still stands, for open connections to ask now and then.
  alive(session: string): boolean {
    return Boolean(this.s?.sessions.some((x) => x.id === session));
  }

  sessions(current: string) {
    return (this.s?.sessions ?? []).map(({ hash, ...x }) => ({ ...x, current: x.id === current }));
  }

  async revoke(id: string): Promise<boolean> {
    const s = this.s;
    if (!s || !s.sessions.some((x) => x.id === id)) return false;
    s.sessions = s.sessions.filter((x) => x.id !== id);
    await this.save();
    return true;
  }

  // A session made in the last minutes may act on its own; an older one has to show the password,
  // and where there is none it has to sign in again.
  private async reauth(fresh: boolean, password: string | undefined, now: number): Promise<Reauth> {
    const s = this.s;
    if (!s) return "bad";
    if (fresh) return "ok";
    if (!s.password) return "need-fresh";
    if (typeof password !== "string") return "bad";
    if (windowFull(s.reauth, now, REAUTH_WINDOW_MS, REAUTH_FAILS)) return "locked";
    windowAdd(s.reauth, now, REAUTH_WINDOW_MS);
    await this.save();
    if (!(await verifyPassword(password, s.password))) return "bad";
    s.reauth.count = 0;
    return "ok";
  }

  async setPassword(next: PasswordRecord, current: string | undefined, fresh: boolean, session: string, now = this.clock()): Promise<Reauth> {
    const r = await this.reauth(fresh, current, now);
    if (r !== "ok") return r;
    const s = this.s!;
    s.password = next;
    s.sessions = s.sessions.filter((x) => x.id === session);
    await this.save();
    this.notify("Your Lazy password was changed", "The password of your Lazy account was changed and your other sessions were signed out. If that was not you, ask for a sign-in code and change it again.");
    return "ok";
  }

  authorizeDelete(fresh: boolean, password: string | undefined, now = this.clock()): Promise<Reauth> {
    return this.reauth(fresh, password, now);
  }

  mailAddress(): string | undefined {
    return this.s?.email;
  }

  async setEmail(email: string): Promise<void> {
    if (!this.s) return;
    this.s.email = email;
    await this.save();
    this.notify("The email of your Lazy account changed", "This address is now the email of your Lazy account.");
  }

  me() {
    const s = this.s;
    if (!s) return undefined;
    return { email: s.email, created: s.created, providers: [...new Set(s.identities.map((i) => i.provider))], password: Boolean(s.password), sessions: s.sessions.length };
  }

  identityIds(): string[] {
    return (this.s?.identities ?? []).map((i) => i.id);
  }

  export() {
    const s = this.s;
    if (!s) return undefined;
    return {
      email: s.email,
      created: s.created,
      hasPassword: Boolean(s.password),
      identities: s.identities.map((i) => ({ provider: i.provider, subjectHash: i.id })),
      sessions: s.sessions.map(({ hash, ...x }) => x),
    };
  }

  async erase(): Promise<{ sessions: number; identities: number }> {
    const gone = { sessions: this.s?.sessions.length ?? 0, identities: this.s?.identities.length ?? 0 };
    await this.store.cancel();
    await this.store.erase();
    this.s = undefined;
    return gone;
  }

  // Forgets sessions that have lapsed.
  async sweep(): Promise<void> {
    const s = this.s;
    if (!s) return;
    const now = this.clock();
    s.sessions = s.sessions.filter((x) => now - x.seen <= SESSION_IDLE_MS && now - x.created <= SESSION_MAX_MS);
    await this.store.put(s);
    await this.store.scheduleIfNone(now + DAY);
  }
}
