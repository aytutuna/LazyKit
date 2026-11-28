import { AccountCore, type State, type Store } from "../../src/account/core/account";
import { IdentityCore } from "../../src/account/core/identity";
import type { Mail } from "../../src/account/core/email";
import { AccountService, type AccountPort, type AppData, type Directory, type IdentityPort } from "../../src/account/core/service";
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
  async scheduleIfNone(at: number) {
    this.alarm ??= at;
  }
  async cancel() {
    this.alarm = undefined;
  }
}

export const kit = await jwtKit();

// A whole account service held in memory, with a clock the test moves and a mailbox it reads.
export function memoryService(over: { codeKey?: string; apps?: string[]; data?: Record<string, AppData> } = {}) {
  const clock = { now: Date.now() };
  const mail: Mail[] = [];
  const accounts = new Map<string, { core: AccountCore; store: MemoryStore<State> }>();
  const identities = new Map<string, { core: IdentityCore; store: MemoryStore<any> }>();
  const codeKey = "codeKey" in over ? over.codeKey : "test-key";
  const dir: Directory = {
    account(name) {
      if (!accounts.has(name)) {
        const store = new MemoryStore<State>();
        accounts.set(name, { store, core: new AccountCore(store, { codeKey: () => codeKey, mailer: () => ({ send: async (m) => void mail.push(m) }), now: () => clock.now }) });
      }
      return accounts.get(name)!.core as unknown as AccountPort;
    },
    identity(name) {
      if (!identities.has(name)) {
        const store = new MemoryStore<any>();
        identities.set(name, { store, core: new IdentityCore(store) });
      }
      return identities.get(name)!.core as unknown as IdentityPort;
    },
  };
  const service = new AccountService(dir, { apps: over.apps ?? ["lazysend", "lazyprint"], appleAudiences: "com.lazyscan.lazyscan", googleAudiences: "google-client", keys: kit.keys, stages: 1 }, over.data);
  return { service, clock, mail, accounts, identities };
}

export const codeIn = (m: Mail) => /\b(\d{6})\b/.exec(m.text)![1];
