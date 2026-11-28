import { DurableObject } from "cloudflare:workers";

// One per Apple or Google subject, named by a hash of provider and subject: which account it signs
// into, and the hashes of tokens already used, so a stolen token cannot be played twice.
export class Identity extends DurableObject<Cloudflare.Env> {
  async account(): Promise<string | undefined> {
    return this.ctx.storage.get<string>("account");
  }

  async bind(account: string): Promise<void> {
    await this.ctx.storage.put("account", account);
  }

  // True once for a token; the entry lives until the token would have expired anyway.
  async use(tokenHash: string, exp: number, now = Date.now()): Promise<boolean> {
    const used = (await this.ctx.storage.get<Record<string, number>>("used")) ?? {};
    for (const [h, until] of Object.entries(used)) if (until <= now) delete used[h];
    if (used[tokenHash]) return false;
    used[tokenHash] = Math.max(exp * 1000 + 120_000, now + 120_000);
    await this.ctx.storage.put("used", used);
    return true;
  }

  async erase(): Promise<void> {
    await this.ctx.storage.deleteAll();
  }
}
