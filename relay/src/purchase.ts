import { DurableObject } from "cloudflare:workers";
import type { Store } from "./services";

// The store's purchase id is hashed before it names an object, so the relay
// never keeps it in the clear.
export async function purchaseKey(store: Store, purchase: string): Promise<string> {
  return sha256(`lazy relay purchase v1\0${store}\0${purchase}`);
}

async function sha256(text: string): Promise<string> {
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)));
  return Array.from(hash, (b) => b.toString(16).padStart(2, "0")).join("");
}

// One per store purchase: remembers, as a hash, which group it paid for, so a
// receipt that is shared or copied cannot lift the cap of a second group. It
// deletes itself when the paid period ends, so nothing links a purchase to a
// group after that.
export class Purchase extends DurableObject<Cloudflare.Env> {
  async bind(group: string, until: number): Promise<boolean> {
    const mine = await sha256(`lazy relay purchase group v1\0${group}`);
    const kept = await this.ctx.storage.get<{ group: string; until: number }>("bound");
    if (kept && kept.until > Date.now() && kept.group !== mine) return false;
    const end = Math.max(until, kept?.until ?? 0);
    await this.ctx.storage.put("bound", { group: mine, until: end });
    await this.ctx.storage.setAlarm(end);
    return true;
  }

  async alarm(): Promise<void> {
    const kept = await this.ctx.storage.get<{ until: number }>("bound");
    if (kept && kept.until > Date.now()) await this.ctx.storage.setAlarm(kept.until);
    else await this.ctx.storage.deleteAll();
  }
}
