import { DurableObject } from "cloudflare:workers";
import { services } from "../services";
import { sha256, toBase64Url } from "./b64";
import { MailFailedError, MailNotConfiguredError, resendMailer } from "./email";
import { burnPasswordTime, equal, verifyPassword, type PasswordRecord } from "./password";

const MINUTE = 60_000;
const DAY = 86_400_000;
const CODE_MS = 10 * MINUTE;
const CODE_TRIES = 3;
const CODES_PER_WINDOW = 3;
const CODE_FAILS_PER_DAY = 10;
const PASSWORD_FAILS = 10;
const PASSWORD_WINDOW_MS = 15 * MINUTE;
const SESSION_IDLE_MS = 30 * DAY;
const SEEN_REFRESH_MS = 3_600_000;
const FRESH_MS = 10 * MINUTE;
const MAX_SESSIONS = 20;
const PENDING_KEEP_MS = 3_600_000;

export const accountName = (email: string) => sha256(`lazy account v1\0${email}`);
export const identityName = (provider: string, sub: string) => sha256(`lazy identity v1\0${provider}\0${sub}`);

const bytes = (s: string) => new TextEncoder().encode(s);

async function hmac(key: string, text: string): Promise<string> {
  const k = await crypto.subtle.importKey("raw", bytes(key), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return Array.from(new Uint8Array(await crypto.subtle.sign("HMAC", k, bytes(text))), (b) => b.toString(16).padStart(2, "0")).join("");
}

interface Code {
  hash: string;
  expires: number;
  tries: number;
}

interface Session {
  id: string;
  hash: string;
  app: string;
  device: string;
  label: string;
  created: number;
  seen: number;
}

interface State {
  email: string;
  created: number;
  active: boolean;
  password?: PasswordRecord;
  pending?: PasswordRecord;
  codes: { signup?: Code; signin?: Code };
  sends: number[];
  passwordFails: { count: number; since: number };
  codeFails: { count: number; since: number };
  identities: { id: string; provider: string }[];
  sessions: Session[];
}

export interface Login {
  app: string;
  device: string;
  label: string;
}

export type Signed = { ok: true; token: string; session: string } | { ok: false; why: "bad" | "locked" };
export type Sent = "sent" | "limited" | "not-configured" | "failed";

export interface Checked {
  account: string;
  session: string;
  app: string;
  device: string;
  fresh: boolean;
}

// Everything about one person's sign-in: named by a hash of the verified email, so one address is one
// account whichever way it was proved. It knows nothing of any app's data; apps ask it who a session
// belongs to. Kept in a Durable Object because every rule here is about one account at a time (count
// the try, then compare) and erasing it is one deleteAll.
export class Account extends DurableObject<Cloudflare.Env> {
  private s?: State;

  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    ctx.blockConcurrencyWhile(async () => {
      this.s = await ctx.storage.get<State>("s");
    });
  }

  private async save(): Promise<void> {
    await this.ctx.storage.put("s", this.s);
    if ((await this.ctx.storage.getAlarm()) === null) await this.ctx.storage.setAlarm(Date.now() + (this.s!.active ? DAY : PENDING_KEEP_MS));
  }

  private fresh(email: string, now: number): State {
    return { email, created: now, active: false, codes: {}, sends: [], passwordFails: { count: 0, since: 0 }, codeFails: { count: 0, since: 0 }, identities: [], sessions: [] };
  }

  // The code is keyed with a secret over the address, so a copy of the stored hash cannot be guessed offline.
  private codeHash(code: string): Promise<string> {
    return hmac(this.env.ACCOUNT_KEY!, `${this.s!.email}\0${code}`);
  }

  // Sends a new code; nothing stays behind if the mail does not go.
  private async mailCode(purpose: "signup" | "signin", now: number): Promise<Sent> {
    const s = this.s!;
    if (!this.env.ACCOUNT_KEY) return "not-configured";
    s.sends = s.sends.filter((t) => now - t < CODE_MS);
    if (s.sends.length >= CODES_PER_WINDOW) return "limited";
    let digits: string;
    for (;;) {
      const n = crypto.getRandomValues(new Uint32Array(1))[0];
      if (n < 4_294_000_000) {
        digits = String(n % 1_000_000).padStart(6, "0");
        break;
      }
    }
    const previous = s.codes[purpose];
    s.codes[purpose] = { hash: await this.codeHash(digits), expires: now + CODE_MS, tries: 0 };
    const text =
      purpose === "signup"
        ? `Your Lazy code is ${digits}. It confirms your new account and works for 10 minutes. If you did not ask for it, ignore this email.`
        : `Your Lazy sign-in code is ${digits}. It works for 10 minutes. If you did not ask for it, ignore this email and nobody gets in.`;
    try {
      await (services().mailer ?? resendMailer(this.env)).send({ to: s.email, subject: `Your Lazy code: ${digits}`, text });
    } catch (e) {
      s.codes[purpose] = previous;
      if (e instanceof MailNotConfiguredError) return "not-configured";
      if (e instanceof MailFailedError) return "failed";
      throw e;
    }
    s.sends.push(now);
    await this.save();
    return "sent";
  }

  // An address that already has an account gets nothing and the caller cannot tell.
  async startSignup(email: string, password: PasswordRecord, now = Date.now()): Promise<Sent | "exists"> {
    this.s ??= this.fresh(email, now);
    if (this.s.active) return "exists";
    const sent = await this.mailCode("signup", now);
    if (sent === "sent") {
      this.s.pending = password;
      await this.save();
    }
    return sent;
  }

  async startSignin(now = Date.now()): Promise<Sent | "none"> {
    if (!this.s?.active) return "none";
    return this.mailCode("signin", now);
  }

  // The try is counted before the compare, so parallel guesses cannot all slip under the limit.
  private async takeCode(purpose: "signup" | "signin", code: string, now: number): Promise<boolean> {
    const s = this.s!;
    const c = s.codes[purpose];
    if (!c) return false;
    if (now - s.codeFails.since >= DAY) s.codeFails = { count: 0, since: now };
    if (c.expires <= now || c.tries >= CODE_TRIES || s.codeFails.count >= CODE_FAILS_PER_DAY) return false;
    c.tries++;
    await this.save();
    const ok = typeof code === "string" && /^\d{6}$/.test(code) && equal(bytes(await this.codeHash(code)), bytes(c.hash));
    if (ok) delete s.codes[purpose];
    else s.codeFails.count++;
    await this.save();
    return ok;
  }

  async confirmSignup(code: string, login: Login, now = Date.now()): Promise<Signed> {
    const s = this.s;
    if (!s || s.active || !s.pending || !(await this.takeCode("signup", code, now))) return { ok: false, why: "bad" };
    s.password = s.pending;
    return this.open(login, now, true);
  }

  async signinWithCode(code: string, login: Login, now = Date.now()): Promise<Signed> {
    if (!this.s?.active || !(await this.takeCode("signin", code, now))) return { ok: false, why: "bad" };
    return this.open(login, now);
  }

  async signinWithPassword(password: string, login: Login, now = Date.now()): Promise<Signed> {
    const s = this.s;
    if (!s?.active || !s.password) {
      await burnPasswordTime();
      return { ok: false, why: "bad" };
    }
    if (!this.passwordTry(now)) return { ok: false, why: "locked" };
    await this.save();
    if (!(await verifyPassword(password, s.password))) return { ok: false, why: "bad" };
    s.passwordFails = { count: 0, since: 0 };
    return this.open(login, now);
  }

  private passwordTry(now: number): boolean {
    const f = this.s!.passwordFails;
    if (now - f.since >= PASSWORD_WINDOW_MS) {
      f.count = 0;
      f.since = now;
    }
    return ++f.count <= PASSWORD_FAILS;
  }

  // Apple or Google vouched for the address. An unconfirmed password sign-up for it is thrown away,
  // so a stranger who typed the address first cannot keep a way in.
  async signinWithIdentity(email: string, identity: { id: string; provider: string }, login: Login, now = Date.now()): Promise<Signed> {
    if (!this.s && !email) return { ok: false, why: "bad" };
    this.s ??= this.fresh(email, now);
    const s = this.s;
    if (!s.active) {
      s.pending = undefined;
      s.codes = {};
      s.created = now;
    }
    if (!s.identities.some((i) => i.id === identity.id)) s.identities.push(identity);
    return this.open(login, now, true);
  }

  private async open(login: Login, now: number, activate = false): Promise<Signed> {
    const s = this.s!;
    if (activate) {
      s.active = true;
      s.pending = undefined;
    }
    const secret = toBase64Url(crypto.getRandomValues(new Uint8Array(32)));
    const id = toBase64Url(crypto.getRandomValues(new Uint8Array(9)));
    // A device that signs in again replaces its own earlier session for the same app.
    s.sessions = s.sessions.filter((x) => !(x.app === login.app && x.device === login.device));
    s.sessions.push({ id, hash: await sha256(secret), app: login.app, device: login.device, label: login.label, created: now, seen: now });
    if (s.sessions.length > MAX_SESSIONS) s.sessions.sort((a, b) => a.seen - b.seen).splice(0, s.sessions.length - MAX_SESSIONS);
    await this.save();
    return { ok: true, token: `${await accountName(s.email)}.${secret}`, session: id };
  }

  async check(secret: string, now = Date.now()): Promise<Checked | undefined> {
    const s = this.s;
    if (!s?.active) return undefined;
    const hash = bytes(await sha256(secret));
    const found = s.sessions.find((x) => equal(bytes(x.hash), hash));
    if (!found) return undefined;
    if (now - found.seen > SESSION_IDLE_MS) {
      s.sessions = s.sessions.filter((x) => x !== found);
      await this.save();
      return undefined;
    }
    if (now - found.seen >= SEEN_REFRESH_MS) {
      found.seen = now;
      await this.save();
    }
    return { account: await accountName(s.email), session: found.id, app: found.app, device: found.device, fresh: now - found.created < FRESH_MS };
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

  // Changing a password needs the old one unless the session is a few minutes old, which is how a
  // person who forgot it sets a new one after signing in by email code. Other sessions end.
  async setPassword(next: PasswordRecord, current: string | undefined, fresh: boolean, session: string, now = Date.now()): Promise<"ok" | "bad" | "locked"> {
    const s = this.s;
    if (!s?.active) return "bad";
    if (s.password && !fresh) {
      if (typeof current !== "string") return "bad";
      if (!this.passwordTry(now)) return "locked";
      await this.save();
      if (!(await verifyPassword(current, s.password))) return "bad";
      s.passwordFails = { count: 0, since: 0 };
    }
    s.password = next;
    s.sessions = s.sessions.filter((x) => x.id === session);
    await this.save();
    return "ok";
  }

  me() {
    const s = this.s;
    if (!s?.active) return undefined;
    return { email: s.email, created: s.created, providers: [...new Set(s.identities.map((i) => i.provider))], password: Boolean(s.password), sessions: s.sessions.length };
  }

  identityIds(): string[] {
    return (this.s?.identities ?? []).map((i) => i.id);
  }

  export() {
    const s = this.s;
    if (!s?.active) return undefined;
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
    await this.ctx.storage.deleteAlarm();
    await this.ctx.storage.deleteAll();
    this.s = undefined;
    return gone;
  }

  // Forgets a sign-up nobody confirmed, and old sessions and codes of the rest.
  async alarm(): Promise<void> {
    const s = this.s;
    if (!s) return;
    const now = Date.now();
    if (!s.active) {
      await this.erase();
      return;
    }
    s.sessions = s.sessions.filter((x) => now - x.seen <= SESSION_IDLE_MS);
    for (const k of ["signup", "signin"] as const) if (s.codes[k] && s.codes[k]!.expires <= now) delete s.codes[k];
    s.sends = s.sends.filter((t) => now - t < CODE_MS);
    await this.ctx.storage.put("s", s);
    if (s.sessions.length > 0) await this.ctx.storage.setAlarm(now + DAY);
  }
}
