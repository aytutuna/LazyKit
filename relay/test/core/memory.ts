import { AccountCore, type AccountState } from "../../src/account/core/account";
import { AddressCore, type AddressState } from "../../src/account/core/address";
import type { Mail, Mailer } from "../../src/account/core/email";
import { IdentityCore } from "../../src/account/core/identity";
import { AccountService, type AccountPort, type AddressPort, type AppData, type Directory, type IdentityPort } from "../../src/account/core/service";
import type { Deps, Store } from "../../src/account/core/store";
import { jwtKit } from "./jwt";

export class MemoryStore<T> implements Store<T> {
  value?: T;
  alarm?: number;
  async get() {
    return this.value === undefined ? undefined : structuredClone(this.value);
  }
  async put(v: T) {
    this.value = structuredClone(v);
  }
  async erase() {
    this.value = undefined;
  }
  async wakeBy(at: number) {
    if (this.alarm === undefined || this.alarm > at) this.alarm = at;
  }
  async cancel() {
    this.alarm = undefined;
  }
}

export const kit = await jwtKit();

interface Options {
  codeKey?: string;
  idKey?: string;
  mailReady?: boolean;
  data?: Record<string, AppData>;
}

// A whole account service held in memory: a clock the test moves, a mailbox it reads, mail that can
// be made to fail, and `flush` to let the mail that was sent after the answer finish.
export function memoryService(over: Options = {}) {
  const clock = { now: Date.now() };
  const mail: Mail[] = [];
  const pending: Promise<unknown>[] = [];
  const mailer = { fail: false, delay: 0 };
  const sender: Mailer = {
    ready: () => over.mailReady !== false,
    send: async (m) => {
      if (mailer.delay) await new Promise((r) => setTimeout(r, mailer.delay));
      if (mailer.fail) throw new Error("refused");
      mail.push(m);
    },
  };
  const deps: Deps = { codeKey: () => ("codeKey" in over ? over.codeKey : "test-key"), mailer: () => sender, defer: (p) => void pending.push(p), stages: () => 1, mailBudget: async () => budget.left-- > 0, accountExists: async (id) => accounts.get(id)?.store.value !== undefined, now: () => clock.now };
  const addresses = new Map<string, { core: AddressCore; store: MemoryStore<AddressState> }>();
  const accounts = new Map<string, { core: AccountCore; store: MemoryStore<AccountState> }>();
  const identities = new Map<string, { core: IdentityCore; store: MemoryStore<any> }>();
  const spent = new Set<string>();
  const budget = { left: Infinity };
  const dir: Directory = {
    once: (name) => ({ take: async () => (spent.has(name) ? false : (spent.add(name), true)) }),
    address(name) {
      if (!addresses.has(name)) {
        const store = new MemoryStore<AddressState>();
        addresses.set(name, { store, core: new AddressCore(store, deps) });
      }
      return addresses.get(name)!.core as unknown as AddressPort;
    },
    account(id) {
      if (!accounts.has(id)) {
        const store = new MemoryStore<AccountState>();
        accounts.set(id, { store, core: new AccountCore(store, deps) });
      }
      return accounts.get(id)!.core as unknown as AccountPort;
    },
    identity(name) {
      if (!identities.has(name)) {
        const store = new MemoryStore<any>();
        identities.set(name, { store, core: new IdentityCore(store, () => clock.now) });
      }
      return identities.get(name)!.core as unknown as IdentityPort;
    },
  };
  const service = new AccountService(
    dir,
    { apps: ["lazysend", "lazyprint"], appleAudiences: "com.lazyscan.lazyscan", googleAudiences: "google-client", keys: kit.keys, idKey: "idKey" in over ? over.idKey : "test-id-key", nonceKey: "nonce-key", stages: 1, sleep: async () => {}, now: () => clock.now },
    over.data,
  );
  const flush = async () => {
    while (pending.length) await Promise.all(pending.splice(0));
  };
  return { service, clock, mail, mailer, budget, flush, addresses, accounts, identities };
}

export const codeIn = (m: Mail) => /\b(\d{6})\b/.exec(m.text)![1];
