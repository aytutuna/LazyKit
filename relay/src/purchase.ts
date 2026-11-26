import { DurableObject } from "cloudflare:workers";
import type { Store } from "./services";

// The store's purchase id is hashed before it names an object, so the relay
// never keeps it in the clear.
export async function purchaseKey(store: Store, purchase: string): Promise<string> {
  const bytes = new TextEncoder().encode(`lazy relay purchase v1\0${store}\0${purchase}`);
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return Array.from(hash, (b) => b.toString(16).padStart(2, "0")).join("");
}

// One per store purchase: remembers which group it paid for, so a receipt
// that is shared or copied cannot lift the cap of a second group.
export class Purchase extends DurableObject<Cloudflare.Env> {
  async bind(group: string): Promise<boolean> {
    const bound = await this.ctx.storage.get<string>("group");
    if (bound === undefined) {
      await this.ctx.storage.put("group", group);
      return true;
    }
    return bound === group;
  }
}
