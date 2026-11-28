import { equal } from "./password";
import type { Deps, Store } from "./store";
import { DAY, MINUTE, bytes, failsAdd, failsForgive, failsFull, failsIdle, hmac, noFails, randomDigits, type Fails } from "./util";

const CODE_MS = 10 * MINUTE;
const SENDS_PER_WINDOW = 3;
const TRIES_PER_CODE_CALLER = 3;
const TRIES_PER_CODE = 10;
const CODE_FAILS_CALLER = 10;
const CODE_FAILS_TOTAL = 100;
const PASSWORD_WINDOW_MS = 15 * MINUTE;
const PASSWORD_FAILS_CALLER = 10;
const PASSWORD_FAILS_TOTAL = 40;
const SWEEP_MS = 3_600_000;

export type Purpose = "signup" | "signin" | "link" | "email";
export type Sent = "sent" | "limited" | "not-configured";

interface Code {
  hash: string;
  expires: number;
  tries: number;
  callers: Record<string, number>;
  mailTo: string;
  extra?: unknown;
}

export interface AddressState {
  account?: string;
  // Where mail for the account goes: the address as it was registered.
  mailTo?: string;
  sends: number[];
  codes: Partial<Record<Purpose, Code>>;
  password: Fails;
  code: Fails;
}

export type Taken = { ok: true; account?: string; mailTo: string; extra?: unknown } | { ok: false };

const fresh = (): AddressState => ({ sends: [], codes: {}, password: noFails(), code: noFails() });

// What the service keeps about one email address, whether or not an account stands behind it: how
// many mails it has been sent, the codes waiting for it, and how many wrong tries it has seen. Named
// by a keyed hash of the address in its canonical spelling, so every address answers alike and an
// account can change its email without changing who it is.
export class AddressCore {
  private s: AddressState = fresh();

  constructor(
    private store: Store<AddressState>,
    private deps: Deps,
  ) {}

  async load(): Promise<void> {
    this.s = (await this.store.get()) ?? fresh();
  }

  private clock = () => this.deps.now?.() ?? Date.now();

  private async save(): Promise<void> {
    await this.store.put(this.s);
    await this.store.scheduleIfNone(this.clock() + SWEEP_MS);
  }

  account(): string | undefined {
    return this.s.account;
  }

  private ready(): boolean {
    return Boolean(this.deps.codeKey()) && this.deps.mailer().ready?.() !== false;
  }

  // The window is recorded before the mail goes, so parallel requests cannot all pass it.
  private reserveSend(now: number): boolean {
    this.s.sends = this.s.sends.filter((t) => now - t < CODE_MS);
    if (this.s.sends.length >= SENDS_PER_WINDOW) return false;
    this.s.sends.push(now);
    return true;
  }

  private later(to: string, subject: string, text: string, undo: () => void): void {
    this.deps.defer(
      this.deps.mailer().send({ to, subject, text }).catch(async () => {
        undo();
        await this.save();
      }),
    );
  }

  private unsend(now: number): void {
    const i = this.s.sends.indexOf(now);
    if (i >= 0) this.s.sends.splice(i, 1);
  }

  private hash(purpose: Purpose, canonical: string, code: string): Promise<string> {
    return hmac(this.deps.codeKey()!, `${canonical}\0${purpose}\0${code}`);
  }

  // Makes a code and mails it; if the mail is refused later, the code and the send are taken back.
  private async issue(purpose: Purpose, canonical: string, mailTo: string, now: number, extra?: unknown): Promise<void> {
    const digits = randomDigits();
    const code: Code = { hash: await this.hash(purpose, canonical, digits), expires: now + CODE_MS, tries: 0, callers: {}, mailTo, extra };
    const previous = this.s.codes[purpose];
    this.s.codes[purpose] = code;
    const text =
      purpose === "signup" ? `Your Lazy code is ${digits}. It confirms your new account and works for 10 minutes. If you did not ask for it, ignore this email.`
      : purpose === "signin" ? `Your Lazy sign-in code is ${digits}. It works for 10 minutes. If you did not ask for it, ignore this email and nobody gets in.`
      : purpose === "link" ? `Your Lazy code is ${digits}. It lets Apple or Google sign in to your Lazy account and works for 10 minutes. If that was not you, ignore this email and nothing is linked.`
      : `Your Lazy code is ${digits}. It confirms this address as the new email of a Lazy account and works for 10 minutes. If you did not ask for it, ignore this email.`;
    this.later(mailTo, `Your Lazy code: ${digits}`, text, () => {
      if (this.s.codes[purpose] === code) {
        if (previous) this.s.codes[purpose] = previous;
        else delete this.s.codes[purpose];
      }
      this.unsend(now);
    });
  }

  // An address that has an account is told so by mail; one that has none gets a code. The caller
  // sees the same answer either way.
  async requestSignup(typed: string, canonical: string, now = this.clock()): Promise<Sent> {
    if (!this.ready()) return "not-configured";
    if (!this.reserveSend(now)) return "limited";
    if (this.s.account && this.s.mailTo) {
      this.later(this.s.mailTo, "You already have a Lazy account", "Someone asked to create a Lazy account with this address, which already has one. Sign in instead; if you forgot your password, ask for a sign-in code. If it was not you, you can ignore this email.", () => this.unsend(now));
    } else {
      await this.issue("signup", canonical, typed, now);
    }
    await this.save();
    return "sent";
  }

  async requestSignin(canonical: string, now = this.clock()): Promise<Sent> {
    if (!this.ready()) return "not-configured";
    if (!this.reserveSend(now)) return "limited";
    if (this.s.account && this.s.mailTo) await this.issue("signin", canonical, this.s.mailTo, now);
    else await this.hash("signin", canonical, "000000");
    await this.save();
    return "sent";
  }

  // A provider vouched for this address, which already has an account: the owner has to say yes by code.
  async requestLink(canonical: string, identity: { id: string; provider: string }, now = this.clock()): Promise<Sent | "none"> {
    if (!this.s.account || !this.s.mailTo) return "none";
    if (!this.ready()) return "not-configured";
    if (!this.reserveSend(now)) return "limited";
    await this.issue("link", canonical, this.s.mailTo, now, identity);
    await this.save();
    return "sent";
  }

  // The new address of an account that is changing its email.
  async requestEmail(typed: string, canonical: string, account: string, now = this.clock()): Promise<Sent> {
    if (!this.ready()) return "not-configured";
    if (!this.reserveSend(now)) return "limited";
    if (this.s.account && this.s.mailTo) {
      this.later(this.s.mailTo, "A Lazy account tried to use this address", "Someone asked to use this address for another Lazy account, which already has one here. Nothing changed. If it was not you, you can ignore this email.", () => this.unsend(now));
    } else {
      await this.issue("email", canonical, typed, now, account);
    }
    await this.save();
    return "sent";
  }

  // Each caller gets three tries per code, the code ten, and a caller ten wrong codes a day (the
  // address a hundred), counted before the compare. A stranger's misses close the door to him, not to the owner.
  async takeCode(purpose: Purpose, code: unknown, canonical: string, caller: string, now = this.clock()): Promise<Taken> {
    const c = this.s.codes[purpose];
    if (!c || !this.deps.codeKey()) return { ok: false };
    if (c.expires <= now || c.tries >= TRIES_PER_CODE || (c.callers[caller] ?? 0) >= TRIES_PER_CODE_CALLER || Object.keys(c.callers).length > 20) return { ok: false };
    if (failsFull(this.s.code, caller, now, DAY, CODE_FAILS_CALLER, CODE_FAILS_TOTAL)) return { ok: false };
    c.tries++;
    c.callers[caller] = (c.callers[caller] ?? 0) + 1;
    await this.save();
    const good = typeof code === "string" && /^\d{6}$/.test(code) && equal(bytes(await this.hash(purpose, canonical, code)), bytes(c.hash));
    if (good) delete this.s.codes[purpose];
    else failsAdd(this.s.code, caller, now, DAY);
    await this.save();
    return good ? { ok: true, account: this.s.account, mailTo: c.mailTo, extra: c.extra } : { ok: false };
  }

  // True once, for the first account to claim the address.
  async bindAccount(id: string, mailTo: string): Promise<boolean> {
    if (this.s.account && this.s.account !== id) return false;
    this.s.account = id;
    this.s.mailTo = mailTo;
    delete this.s.codes.signup;
    await this.save();
    return true;
  }

  async unbind(id: string): Promise<void> {
    if (this.s.account !== id) return;
    this.s.account = undefined;
    this.s.mailTo = undefined;
    await this.save();
  }

  // The try is counted before the password is looked at; the answer is the same whether or not an account exists.
  async passwordTry(caller: string, now = this.clock()): Promise<"ok" | "locked"> {
    if (failsFull(this.s.password, caller, now, PASSWORD_WINDOW_MS, PASSWORD_FAILS_CALLER, PASSWORD_FAILS_TOTAL)) return "locked";
    failsAdd(this.s.password, caller, now, PASSWORD_WINDOW_MS);
    await this.save();
    return "ok";
  }

  async passwordGood(caller: string): Promise<void> {
    failsForgive(this.s.password, caller);
    await this.save();
  }

  // Forgets what has run out; an address with nothing left and no account is deleted. A newer code is not touched.
  async sweep(): Promise<void> {
    const now = this.clock();
    this.s.sends = this.s.sends.filter((t) => now - t < CODE_MS);
    for (const k of Object.keys(this.s.codes) as Purpose[]) if (this.s.codes[k]!.expires <= now) delete this.s.codes[k];
    const quiet = this.s.sends.length === 0 && Object.keys(this.s.codes).length === 0 && failsIdle(this.s.code, now, DAY) && failsIdle(this.s.password, now, PASSWORD_WINDOW_MS);
    if (quiet && !this.s.account) {
      this.s = fresh();
      await this.store.erase();
      return;
    }
    await this.store.put(this.s);
    await this.store.scheduleIfNone(now + SWEEP_MS);
  }
}
