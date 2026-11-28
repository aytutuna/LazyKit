import type { Store } from "./store";

interface State {
  account?: string;
  used: Record<string, number>;
}

// One per Apple or Google subject, named by a hash of provider and subject: which account it signs
// into, and the hashes of tokens already used, so a stolen token cannot be played twice.
export class IdentityCore {
  private s: State = { used: {} };

  constructor(
    private store: Store<State>,
    private now: () => number = Date.now,
  ) {}

  async load(): Promise<void> {
    this.s = (await this.store.get()) ?? { used: {} };
  }

  account(): string | undefined {
    return this.s.account;
  }

  async bind(account: string): Promise<void> {
    this.s.account = account;
    await this.store.put(this.s);
  }

  // True once for a token; the entry lives until the token would have expired anyway.
  async use(tokenHash: string, exp: number, now = this.now()): Promise<boolean> {
    for (const [h, until] of Object.entries(this.s.used)) if (until <= now) delete this.s.used[h];
    if (this.s.used[tokenHash]) return false;
    this.s.used[tokenHash] = Math.max(exp * 1000 + 120_000, now + 120_000);
    await this.store.put(this.s);
    return true;
  }

  // Binds the subject to this account unless it is already bound; answers whichever account holds it.
  async claim(account: string): Promise<string> {
    if (this.s.account) return this.s.account;
    this.s.account = account;
    await this.store.put(this.s);
    return account;
  }

  // Gives the subject up, but only if it is still this account's.
  async release(account: string): Promise<void> {
    if (this.s.account !== account) return;
    this.s.account = undefined;
    await this.store.put(this.s);
  }

  async erase(): Promise<void> {
    this.s = { used: {} };
    await this.store.erase();
  }
}
