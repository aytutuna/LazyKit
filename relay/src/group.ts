import { DurableObject } from "cloudflare:workers";
import { NotConfiguredError, ReceiptRejectedError, TokenGoneError, services, type Platform, type Store } from "./services";
import { weekOf } from "./week";

export const WEEKLY_CAP = 5 * 1024 ** 3;
export const MAX_DEVICES = 32;
export const MAX_TOKEN = 4096;
const KEEP_WEEKS = 8;
const WAKE_GAP_MS = 10_000;

interface Device {
  platform: Platform;
  token: string;
}

export interface GroupStatus {
  week: string;
  used: number;
  cap: number;
  paidUntil: number;
  paid: boolean;
  capped: boolean;
}

// What the relay knows about one person's paired devices: this week's bytes,
// the time a purchase lifts the cap until, and a push token per device. The
// group is a random key the devices made when they first paired; it carries
// no name or address.
export class Group extends DurableObject<Cloudflare.Env> {
  private usage: Record<string, number> = {};
  private paidUntil = 0;
  private devices: Record<string, Device> = {};
  private woken = new Map<string, number>();

  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    ctx.blockConcurrencyWhile(async () => {
      const kept = await ctx.storage.get(["usage", "paidUntil", "devices"]);
      this.usage = (kept.get("usage") as Record<string, number> | undefined) ?? {};
      this.paidUntil = (kept.get("paidUntil") as number | undefined) ?? 0;
      this.devices = (kept.get("devices") as Record<string, Device> | undefined) ?? {};
    });
  }

  // Adds forwarded bytes to the week they were sent in. Returns false once
  // the group is over its cap, so the room can close its streams.
  async record(bytes: number, now = Date.now()): Promise<boolean> {
    const week = weekOf(now);
    this.usage[week] = (this.usage[week] ?? 0) + bytes;
    const oldest = weekOf(now - KEEP_WEEKS * 7 * 86_400_000);
    for (const w of Object.keys(this.usage)) if (w < oldest) delete this.usage[w];
    await this.ctx.storage.put("usage", this.usage);
    return !this.status(now).capped;
  }

  status(now = Date.now()): GroupStatus {
    const week = weekOf(now);
    const used = this.usage[week] ?? 0;
    const paid = this.paidUntil > now;
    return { week, used, cap: WEEKLY_CAP, paidUntil: this.paidUntil, paid, capped: !paid && used >= WEEKLY_CAP };
  }

  // Rejections come back as values: an error thrown inside a Durable Object
  // is reported as uncaught as well as reaching the caller.
  async buy(store: Store, receipt: string): Promise<"ok" | "not-configured" | "rejected"> {
    try {
      const { until } = await services().verifier.verify(store, receipt);
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

  async setPush(device: string, platform: Platform, token: string): Promise<boolean> {
    if (!(device in this.devices) && Object.keys(this.devices).length >= MAX_DEVICES) return false;
    this.devices[device] = { platform, token };
    await this.ctx.storage.put("devices", this.devices);
    return true;
  }

  async dropPush(device: string): Promise<void> {
    delete this.devices[device];
    await this.ctx.storage.put("devices", this.devices);
  }

  // Wakes a closed phone app so it connects to the room. True when a push
  // went out; a device is not woken twice within a few seconds.
  async wake(device: string, room: string, now = Date.now()): Promise<boolean> {
    const d = this.devices[device];
    if (!d) return false;
    if (now - (this.woken.get(device) ?? 0) < WAKE_GAP_MS) return true;
    try {
      await services().push.send(d.platform, d.token, { room });
    } catch (e) {
      if (e instanceof TokenGoneError) await this.dropPush(device);
      return false;
    }
    this.woken.set(device, now);
    return true;
  }
}
