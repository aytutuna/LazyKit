import { DurableObject } from "cloudflare:workers";
import { NotConfiguredError, ReceiptRejectedError, services, type Store } from "./services";
import { purchaseKey } from "./purchase";
import { weekOf } from "./week";

export const WEEKLY_CAP = 5_000_000_000;
export const MAX_GROUPS = 16;
const KEEP_WEEKS = 8;

export interface QuotaStatus {
  week: string;
  used: number;
  cap: number;
  paidUntil: number;
  paid: boolean;
  capped: boolean;
  // What the stores must carry as the buyer's account id (Apple's appAccountToken, Google's obfuscatedAccountId).
  accountToken: string;
}

// What the relay keeps for one Lazy account: this week's bytes across all its groups, the time a
// purchase lifts the cap until, and which groups are its own. Named by the account id.
export class Quota extends DurableObject<Cloudflare.Env> {
  private usage: Record<string, number> = {};
  private paidUntil = 0;
  private groups: string[] = [];
  private token = "";

  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    ctx.blockConcurrencyWhile(async () => {
      const kept = await ctx.storage.get(["usage", "paidUntil", "groups", "token"]);
      this.usage = (kept.get("usage") as Record<string, number> | undefined) ?? {};
      this.paidUntil = (kept.get("paidUntil") as number | undefined) ?? 0;
      this.groups = (kept.get("groups") as string[] | undefined) ?? [];
      this.token = (kept.get("token") as string | undefined) ?? "";
    });
  }

  // Adds forwarded bytes to the week they were sent in. Returns false once the account is over its cap.
  async record(bytes: number, now = Date.now()): Promise<boolean> {
    const week = weekOf(now);
    this.usage[week] = (this.usage[week] ?? 0) + bytes;
    const oldest = weekOf(now - KEEP_WEEKS * 7 * 86_400_000);
    for (const w of Object.keys(this.usage)) if (w < oldest) delete this.usage[w];
    await this.ctx.storage.put("usage", this.usage);
    return !(await this.status(now)).capped;
  }

  async status(now = Date.now()): Promise<QuotaStatus> {
    if (!this.token) {
      this.token = crypto.randomUUID();
      await this.ctx.storage.put("token", this.token);
    }
    const week = weekOf(now);
    const used = this.usage[week] ?? 0;
    const paid = this.paidUntil > now;
    return { week, used, cap: WEEKLY_CAP, paidUntil: this.paidUntil, paid, capped: !paid && used >= WEEKLY_CAP, accountToken: this.token };
  }

  async addGroup(group: string): Promise<boolean> {
    if (this.groups.includes(group)) return true;
    if (this.groups.length >= MAX_GROUPS) return false;
    this.groups.push(group);
    await this.ctx.storage.put("groups", this.groups);
    return true;
  }

  async removeGroup(group: string): Promise<void> {
    this.groups = this.groups.filter((g) => g !== group);
    await this.ctx.storage.put("groups", this.groups);
  }

  // Rejections come back as values: an error thrown inside a Durable Object is reported as
  // uncaught as well as reaching the caller. A purchase pays for one account only.
  async buy(store: Store, receipt: string, account: string): Promise<"ok" | "not-configured" | "rejected" | "bound"> {
    try {
      await this.status();
      const { until, purchase } = await services().verifier.verify(store, receipt, this.token);
      const owner = this.env.PURCHASES.get(this.env.PURCHASES.idFromName(await purchaseKey(store, purchase)));
      if (!(await owner.bind(account, until))) return "bound";
      if (until > this.paidUntil) {
        this.paidUntil = until;
        await this.ctx.storage.put("paidUntil", until);
      }
      return "ok";
    } catch (e) {
      if (e instanceof NotConfiguredError) return "not-configured";
      if (e instanceof ReceiptRejectedError) return "rejected";
      throw e;
    }
  }

  async export() {
    const groups = [];
    for (const g of this.groups) groups.push(await this.env.GROUPS.get(this.env.GROUPS.idFromName(g)).export(g));
    return { usage: this.usage, paidUntil: this.paidUntil, accountToken: this.token || undefined, groups };
  }

  // The purchase record stays elsewhere, as a hash, until its paid time ends.
  async erase(): Promise<{ groups: number; pushTokens: number; usageWeeks: number; purchaseKeptUntil?: number }> {
    let pushTokens = 0;
    for (const g of this.groups) pushTokens += await this.env.GROUPS.get(this.env.GROUPS.idFromName(g)).erase();
    const gone = { groups: this.groups.length, pushTokens, usageWeeks: Object.keys(this.usage).length, purchaseKeptUntil: this.paidUntil > Date.now() ? this.paidUntil : undefined };
    await this.ctx.storage.deleteAll();
    this.usage = {};
    this.paidUntil = 0;
    this.groups = [];
    this.token = "";
    return gone;
  }
}
