import { equal } from "./password";
import type { Deps, Store } from "./store";
import { CallerCounts, DAY, MINUTE, bytes, emptyWindow, hmac, randomDigits, windowAdd, windowFull, windowIdle, type Window } from "./util";

const CODE_MS = 10 * MINUTE;
const SENDS_PER_WINDOW = 3;
const SENDS_PER_DAY = 10;
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

// A new-email code is kept per account that asked, so one account cannot crowd out another at an address.
export type Purpose = "signup" | "signin" | "link" | "emailold" | "setpw" | `email:${string}`;
const MAX_PENDING_EMAIL = 5;
export type Sent = "sent" | "limited" | "busy" | "not-configured";

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
    const next = this.nextWake();
    if (next !== undefined) await this.store.wakeBy(next);
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
  // The whole service has a day's allowance of mail too, charged only when a mail will really go; when
  // it is spent the request leaves no mark on this address.
  private async reserveSend(now: number, mails: boolean): Promise<"ok" | "limited" | "busy"> {
    // While the day's mail is spent every request gets the same answer, whatever the address.
    if (this.deps.mailBudgetOpen && !(await this.deps.mailBudgetOpen())) return "busy";
    this.s.sends = this.s.sends.filter((t) => now - t < DAY);
    if (this.s.sends.length >= SENDS_PER_DAY || this.s.sends.filter((t) => now - t < CODE_MS).length >= SENDS_PER_WINDOW) return "limited";
    this.s.sends.push(now);
    // A request that will send nothing (an address without an account) makes the same call without
    // charging, so how long it takes does not tell whether the address has an account.
    if (this.deps.mailBudget && !(await this.deps.mailBudget(mails)) && mails) {
      this.s.sends.splice(this.s.sends.indexOf(now), 1);
      return "busy";
    }
    return "ok";
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
      : purpose === "setpw" ? `Your Lazy code is ${digits}. Setting a first password on your Lazy account needs it, and it works for 10 minutes. If you did not ask for that, ignore this email and nothing changes.`
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
    const slot = await this.reserveSend(now, true);
    if (slot !== "ok") return slot;
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
    const slot = await this.reserveSend(now, Boolean(this.s.account && this.s.mailTo));
    if (slot !== "ok") return slot;
    if (this.s.account && this.s.mailTo) await this.issue("signin", canonical, this.s.mailTo, now);
    else await this.hash("signin", canonical, "000000");
    await this.save();
    return "sent";
  }

  // A provider vouched for this address, which already has an account: the owner has to say yes by code.
  async requestLink(canonical: string, identity: { id: string; provider: string; app: string; device: string }, now = this.clock()): Promise<Sent | "none"> {
    await this.repair();
    if (!this.s.account || !this.s.mailTo) return "none";
    if (!this.ready()) return "not-configured";
    const slot = await this.reserveSend(now, true);
    if (slot !== "ok") return slot;
    await this.issue("link", canonical, this.s.mailTo, now, identity);
    await this.save();
    return "sent";
  }

  // The new address of an account that is changing its email.
  // What the asking account may do is limited by its own daily count, not by this address's window,
  // so another account cannot use up the mail this address's owner is allowed. An address that has an
  // account of its own is never written to for someone else's change.
  async requestEmail(typed: string, canonical: string, account: string, now = this.clock()): Promise<Sent> {
    if (!this.ready()) return "not-configured";
    await this.repair();
    if (this.deps.mailBudgetOpen && !(await this.deps.mailBudgetOpen())) return "busy";
    // An address with an account of its own is never written to, but takes the same budget call.
    if (this.s.account) {
      await this.deps.mailBudget?.(false);
      return "sent";
    }
    if (this.deps.mailBudget && !(await this.deps.mailBudget())) return "busy";
    const pending = Object.keys(this.s.codes).filter((k) => k.startsWith("email:"));
    if (pending.length >= MAX_PENDING_EMAIL && !pending.includes(`email:${account}`)) {
      const oldest = pending.sort((x, y) => this.s.codes[x as Purpose]!.expires - this.s.codes[y as Purpose]!.expires)[0];
      delete this.s.codes[oldest as Purpose];
    }
    await this.issue(`email:${account}`, canonical, typed, now, account);
    await this.save();
    return "sent";
  }

  // The current address of an account is asked as well before its email changes.
  // A change of email names its new address, in the mail and in what the code is good for.
  async requestOldCheck(canonical: string, account: string, purpose: "emailold" | "setpw" = "emailold", now = this.clock(), change?: { typed: string; canonical: string }): Promise<Sent | "none"> {
    if (this.s.account !== account || !this.s.mailTo) return "none";
    if (!this.ready()) return "not-configured";
    const slot = await this.reserveSend(now, true);
    if (slot !== "ok") return slot;
    const text = change ? (digits: string) => `Your Lazy code is ${digits}. Changing the email of your Lazy account to ${change.typed} needs it, and it works for 10 minutes. If you did not ask for that, ignore this email and nothing changes.` : undefined;
    await this.issue(purpose, canonical, this.s.mailTo, now, change ? { account, to: change.canonical } : account, text);
    await this.save();
    return "sent";
  }

  // Tells an address about a change of email, and says whether the mail went; asked twice more if it did not.
  async notifyChange(to: string, text: string): Promise<boolean> {
    for (let attempt = 0; attempt < 3; attempt++) {
      try {
        await this.deps.mailer().send({ to, subject: "The email of your Lazy account changed", text });
        return true;
      } catch {}
    }
    return false;
  }

  // Each caller gets three tries per code and ten wrong codes a day; the code itself has thirty tries
  // and the address three hundred wrong codes a day. Tries are counted before the compare. A guess at
  // an address with no live code does the same hashing and the same two writes, so that is not a way to
  // tell which addresses have codes or accounts.
  // `only` names what the code was made for (the device that asked, say): a right code from anywhere else counts as a wrong try and stays unspent.
  async takeCode(purpose: Purpose, code: unknown, canonical: string, caller: string, only?: Record<string, string>, now = this.clock(), keep = false): Promise<Taken> {
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
    const fits = !only || Object.entries(only).every(([k, v]) => (c?.extra as Record<string, unknown> | undefined)?.[k] === v);
    const good = Boolean(live) && same && guess === code && fits;
    // `keep` checks the code without spending it, for a step that needs two codes to be right together.
    if (good) {
      if (!keep) delete this.s.codes[purpose];
    } else {
      this.codeCallers.add(caller, now);
      windowAdd(this.s.code, now, DAY);
    }
    await this.save();
    return good ? { ok: true, account: this.s.account, mailTo: c!.mailTo, extra: c!.extra } : { ok: false };
  }

  // Spends a code that `takeCode` checked with `keep`.
  async dropCode(purpose: Purpose): Promise<void> {
    delete this.s.codes[purpose];
    await this.save();
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
    this.s.sends = this.s.sends.filter((t) => now - t < DAY);
    for (const k of Object.keys(this.s.codes) as Purpose[]) if (this.s.codes[k]!.expires <= now) delete this.s.codes[k];
    if (windowIdle(this.s.code, now, DAY)) this.s.misses = undefined;
    this.codeCallers.prune(now);
    this.passwordCallers.prune(now);
    if (this.nextWake() === undefined && !this.s.account) {
      this.s = fresh();
      await this.store.erase();
      return;
    }
    await this.store.put(this.s);
    const next = this.nextWake();
    if (next !== undefined) await this.store.wakeBy(next);
  }

  // The first moment something kept here runs out; nothing to wake for when nothing will.
  private nextWake(): number | undefined {
    const times: number[] = [];
    for (const c of Object.values(this.s.codes)) times.push(c!.expires);
    if (this.s.sends.length > 0) times.push(Math.min(...this.s.sends) + DAY);
    if (this.s.code.count > 0) times.push(this.s.code.since + DAY);
    if (this.s.password.count > 0) times.push(this.s.password.since + PASSWORD_WINDOW_MS);
    return times.length > 0 ? Math.min(...times) + 1 : undefined;
  }
}
