import { equal } from "./password";
import type { Deps, Store } from "./store";
import { CallerCounts, DAY, MINUTE, bytes, emptyWindow, hmac, randomDigits, windowAdd, windowFull, windowIdle, type Window } from "./util";

const CODE_MS = 10 * MINUTE;
const SENDS_PER_WINDOW = 3;
// Each caller gets a few tries, kept in memory; what is written down counts everyone together and
// sits well above what a few networks could spend, so strangers cannot close the door on the owner.
const TRIES_PER_CODE_CALLER = 3;
const TRIES_PER_CODE = 30;
const CODE_FAILS_CALLER = 10;
const CODE_FAILS_TOTAL = 300;
const PASSWORD_WINDOW_MS = 15 * MINUTE;
const PASSWORD_FAILS_CALLER = 10;
const PASSWORD_SLOW_AFTER = 40;
const PASSWORD_FAILS_TOTAL = 400;
const SWEEP_MS = 3_600_000;
const UNDO_MS = 7 * DAY;

export type Purpose = "signup" | "signin" | "link" | "email" | "emailold" | "undo";
export type Sent = "sent" | "limited" | "not-configured";

interface Code {
  hash: string;
  expires: number;
  tries: number;
  mailTo: string;
  extra?: unknown;
}

export interface AddressState {
  account?: string;
  // Where mail for the account goes: the address as it was registered.
  mailTo?: string;
  sends: number[];
  codes: Partial<Record<Purpose, Code>>;
  // Wrong passwords and wrong codes by everyone, without saying who.
  password: Window;
  code: Window;
  // Guesses that found no live code, counted so they cost what any other guess does.
  misses?: number;
}

export type Taken = { ok: true; account?: string; mailTo: string; extra?: unknown } | { ok: false };

const fresh = (): AddressState => ({ sends: [], codes: {}, password: emptyWindow(), code: emptyWindow() });

// What the service keeps about one email address, whether or not an account stands behind it: how
// many mails it has been sent, the codes waiting for it, and how many wrong tries it has seen. Named
// by a keyed hash of the address in its canonical spelling, so every address answers alike and an
// account can change its email without changing who it is.
export class AddressCore {
  private s: AddressState = fresh();
  private codeCallers = new CallerCounts(DAY);
  private passwordCallers = new CallerCounts(PASSWORD_WINDOW_MS);
  // Tries per caller on the code now waiting, by code hash.
  private tried = new Map<string, Map<string, number>>();

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

  // An address bound to an account that is gone (a deletion that stopped half way) is unbound again.
  private async repair(): Promise<void> {
    const id = this.s.account;
    if (id && this.deps.accountExists && !(await this.deps.accountExists(id))) {
      this.s.account = undefined;
      this.s.mailTo = undefined;
    }
  }

  private ready(): boolean {
    return Boolean(this.deps.codeKey()) && this.deps.mailer().ready?.() !== false;
  }

  // The window counts requests, not deliveries, and is recorded before the mail goes: parallel
  // requests cannot all pass it, and a refused mail cannot be told apart by a refund.
  private reserveSend(now: number): boolean {
    this.s.sends = this.s.sends.filter((t) => now - t < CODE_MS);
    if (this.s.sends.length >= SENDS_PER_WINDOW) return false;
    this.s.sends.push(now);
    return true;
  }

  private later(to: string, subject: string, text: string, undo: () => void = () => {}): void {
    this.deps.defer(
      this.deps.mailer().send({ to, subject, text }).catch(async () => {
        undo();
        await this.save();
      }),
    );
  }

  private hash(purpose: Purpose, canonical: string, code: string): Promise<string> {
    return hmac(this.deps.codeKey()!, `${canonical}\0${purpose}\0${code}`);
  }

  // Makes a code and mails it; if the mail is refused later, the code and the send are taken back.
  private async issue(purpose: Purpose, canonical: string, mailTo: string, now: number, extra?: unknown, text?: (digits: string) => string, ttl = CODE_MS): Promise<void> {
    const digits = randomDigits();
    const code: Code = { hash: await this.hash(purpose, canonical, digits), expires: now + ttl, tries: 0, mailTo, extra };
    const previous = this.s.codes[purpose];
    this.s.codes[purpose] = code;
    const body =
      text ? text(digits)
      : purpose === "signup" ? `Your Lazy code is ${digits}. It confirms your new account and works for 10 minutes. If you did not ask for it, ignore this email.`
      : purpose === "signin" ? `Your Lazy sign-in code is ${digits}. It works for 10 minutes. If you did not ask for it, ignore this email and nobody gets in.`
      : purpose === "link" ? `Your Lazy code is ${digits}. Entering it adds Apple or Google as a way to sign in to your Lazy account, and that sign-in will open the account. It works for 10 minutes. If that was not you, ignore this email and nothing is added.`
      : purpose === "emailold" ? `Your Lazy code is ${digits}. A change of the email of your Lazy account needs it, and it works for 10 minutes. If you did not ask for that, ignore this email and nothing changes.`
      : `Your Lazy code is ${digits}. It confirms this address as the new email of a Lazy account and works for 10 minutes. If you did not ask for it, ignore this email.`;
    this.later(mailTo, `Your Lazy code: ${digits}`, body, () => {
      if (this.s.codes[purpose] === code) {
        if (previous) this.s.codes[purpose] = previous;
        else delete this.s.codes[purpose];
      }
    });
  }

  // An address that has an account is told so by mail; one that has none gets a code. The caller
  // sees the same answer either way.
  async requestSignup(typed: string, canonical: string, now = this.clock()): Promise<Sent> {
    if (!this.ready()) return "not-configured";
    await this.repair();
    if (!this.reserveSend(now)) return "limited";
    if (this.s.account && this.s.mailTo) {
      this.later(this.s.mailTo, "You already have a Lazy account", "Someone asked to create a Lazy account with this address, which already has one. Sign in instead; if you forgot your password, ask for a sign-in code. If it was not you, you can ignore this email.");
    } else {
      await this.issue("signup", canonical, typed, now);
    }
    await this.save();
    return "sent";
  }

  async requestSignin(canonical: string, now = this.clock()): Promise<Sent> {
    if (!this.ready()) return "not-configured";
    await this.repair();
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
      this.later(this.s.mailTo, "A Lazy account tried to use this address", "Someone asked to use this address for another Lazy account, which already has one here. Nothing changed. If it was not you, you can ignore this email.");
    } else {
      // A change already waiting here for another account is not overwritten.
      const waiting = this.s.codes.email;
      if (!waiting || waiting.expires <= now || waiting.extra === account) await this.issue("email", canonical, typed, now, account);
    }
    await this.save();
    return "sent";
  }

  // The current address of an account without a password is asked as well before the email changes.
  async requestOldCheck(canonical: string, account: string, now = this.clock()): Promise<Sent | "none"> {
    if (this.s.account !== account || !this.s.mailTo) return "none";
    if (!this.ready()) return "not-configured";
    if (!this.reserveSend(now)) return "limited";
    await this.issue("emailold", canonical, this.s.mailTo, now, account);
    await this.save();
    return "sent";
  }

  // Mailed to the address an account is leaving, with a code that puts it back for a week.
  async issueUndo(canonical: string, account: string, mailTo: string, moved: string, now = this.clock()): Promise<void> {
    await this.issue("undo", canonical, mailTo, now, account, (digits) => `The email of your Lazy account was changed to ${moved}. If that was you, ignore this email. If it was not, enter the code ${digits} within 7 days to put this address back and sign every session out.`, UNDO_MS);
    await this.save();
  }

  // Each caller gets three tries per code and ten wrong codes a day; the code itself has thirty tries
  // and the address three hundred wrong codes a day. Tries are counted before the compare. A guess at
  // an address with no live code does the same hashing and the same two writes, so that is not a way to
  // tell which addresses have codes or accounts.
  async takeCode(purpose: Purpose, code: unknown, canonical: string, caller: string, now = this.clock()): Promise<Taken> {
    if (!this.deps.codeKey()) return { ok: false };
    if (this.codeCallers.full(caller, now, CODE_FAILS_CALLER) || windowFull(this.s.code, now, DAY, CODE_FAILS_TOTAL)) return { ok: false };
    const c = this.s.codes[purpose];
    const mine = c ? (this.tried.get(c.hash) ?? new Map<string, number>()) : undefined;
    const live = c && mine && c.expires > now && c.tries < TRIES_PER_CODE && (mine.get(caller) ?? 0) < TRIES_PER_CODE_CALLER;
    if (live) {
      c.tries++;
      mine.set(caller, (mine.get(caller) ?? 0) + 1);
      if (this.tried.size >= 50) this.tried.delete(this.tried.keys().next().value!);
      this.tried.set(c.hash, mine);
    } else {
      this.s.misses = (this.s.misses ?? 0) + 1;
    }
    await this.save();
    const guess = typeof code === "string" && /^[0-9]{6}$/.test(code) ? code : "000000";
    const same = equal(bytes(await this.hash(purpose, canonical, guess)), bytes(live ? c.hash : "-"));
    const good = Boolean(live) && same && guess === code;
    if (good) delete this.s.codes[purpose];
    else {
      this.codeCallers.add(caller, now);
      windowAdd(this.s.code, now, DAY);
    }
    await this.save();
    return good ? { ok: true, account: this.s.account, mailTo: c!.mailTo, extra: c!.extra } : { ok: false };
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

  // The try is counted before the password is looked at; the answer is the same whether or not an
  // account exists. A caller has ten tries in 15 minutes; past forty by everyone together the
  // answer is slowed, and past four hundred it is refused to all.
  async passwordTry(caller: string, now = this.clock()): Promise<"ok" | "slow" | "locked"> {
    if (this.passwordCallers.full(caller, now, PASSWORD_FAILS_CALLER) || windowFull(this.s.password, now, PASSWORD_WINDOW_MS, PASSWORD_FAILS_TOTAL)) return "locked";
    this.passwordCallers.add(caller, now);
    windowAdd(this.s.password, now, PASSWORD_WINDOW_MS);
    await this.save();
    return this.s.password.count > PASSWORD_SLOW_AFTER ? "slow" : "ok";
  }

  async passwordGood(caller: string): Promise<void> {
    this.passwordCallers.forgive(caller);
    this.s.password.count = Math.max(0, this.s.password.count - 1);
    await this.save();
  }

  // Forgets what has run out; an address with nothing left and no account is deleted. A newer code is not touched.
  async sweep(): Promise<void> {
    const now = this.clock();
    await this.repair();
    this.s.sends = this.s.sends.filter((t) => now - t < CODE_MS);
    for (const k of Object.keys(this.s.codes) as Purpose[]) if (this.s.codes[k]!.expires <= now) delete this.s.codes[k];
    if (windowIdle(this.s.code, now, DAY)) this.s.misses = undefined;
    this.codeCallers.prune(now);
    this.passwordCallers.prune(now);
    const quiet = this.s.sends.length === 0 && Object.keys(this.s.codes).length === 0 && windowIdle(this.s.code, now, DAY) && windowIdle(this.s.password, now, PASSWORD_WINDOW_MS);
    if (quiet && !this.s.account) {
      this.s = fresh();
      await this.store.erase();
      return;
    }
    await this.store.put(this.s);
    await this.store.scheduleIfNone(now + SWEEP_MS);
  }
}
