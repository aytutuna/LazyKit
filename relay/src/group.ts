import { DurableObject } from "cloudflare:workers";
import { NotConfiguredError, ReceiptRejectedError, TokenGoneError, services, type Platform, type Store } from "./services";
import { purchaseKey } from "./purchase";
import { weekOf } from "./week";

export const WEEKLY_CAP = 5_000_000_000;
export const MAX_DEVICES = 32;
export const MAX_TOKEN = 4096;
const KEEP_WEEKS = 8;
const WAKE_GAP_MS = 10_000;
const DEVICE_KEEP_MS = 90 * 86_400_000;
const SEEN_REFRESH_MS = 86_400_000;
const SWEEP_MS = 86_400_000;

interface Device {
  platform: Platform;
  token: string;
  seen: number;
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
    this.pruneUsage(now);
    await this.ctx.storage.put("usage", this.usage);
    await this.sweepSoon();
    return !this.status(now).capped;
  }

  private pruneUsage(now: number): boolean {
    const oldest = weekOf(now - KEEP_WEEKS * 7 * 86_400_000);
    let dropped = false;
    for (const w of Object.keys(this.usage)) {
      if (w < oldest) {
        delete this.usage[w];
        dropped = true;
      }
    }
    return dropped;
  }

  // A device that has not registered or connected for a long time is gone, so its token is
  // neither kept nor woken.
  private async pruneDevices(now: number): Promise<void> {
    let dropped = false;
    for (const [id, d] of Object.entries(this.devices)) {
      if (now - d.seen > DEVICE_KEEP_MS) {
        delete this.devices[id];
        dropped = true;
      }
    }
    if (dropped) await this.ctx.storage.put("devices", this.devices);
  }

  private async sweepSoon(): Promise<void> {
    if ((await this.ctx.storage.getAlarm()) === null) await this.ctx.storage.setAlarm(Date.now() + SWEEP_MS);
  }

  // Forgets old weeks and devices; a group with nothing left to remember is deleted.
  async alarm(): Promise<void> {
    const now = Date.now();
    if (this.pruneUsage(now)) await this.ctx.storage.put("usage", this.usage);
    await this.pruneDevices(now);
    if (Object.keys(this.devices).length === 0 && Object.keys(this.usage).length === 0 && this.paidUntil <= now) {
      await this.ctx.storage.deleteAll();
      this.paidUntil = 0;
      return;
    }
    await this.ctx.storage.setAlarm(now + SWEEP_MS);
  }

  // A host or guest connecting shows the device is in use.
  async touch(device: string, now = Date.now()): Promise<void> {
    const d = this.devices[device];
    if (!d || now - d.seen < SEEN_REFRESH_MS) return;
    d.seen = now;
    await this.ctx.storage.put("devices", this.devices);
  }

  status(now = Date.now()): GroupStatus {
    const week = weekOf(now);
    const used = this.usage[week] ?? 0;
    const paid = this.paidUntil > now;
    return { week, used, cap: WEEKLY_CAP, paidUntil: this.paidUntil, paid, capped: !paid && used >= WEEKLY_CAP };
  }

  // Rejections come back as values: an error thrown inside a Durable Object
  // is reported as uncaught as well as reaching the caller.
  // A purchase pays for one group only; the first group to present it keeps it.
  async buy(store: Store, receipt: string, group: string): Promise<"ok" | "not-configured" | "rejected" | "bound"> {
    try {
      const { until, purchase } = await services().verifier.verify(store, receipt, group);
      const owner = this.env.PURCHASES.get(this.env.PURCHASES.idFromName(await purchaseKey(store, purchase)));
      if (!(await owner.bind(group, until))) return "bound";
      if (until > this.paidUntil) {
        this.paidUntil = until;
        await this.ctx.storage.put("paidUntil", until);
        await this.sweepSoon();
      }
      return "ok";
    } catch (e) {
      if (e instanceof NotConfiguredError) return "not-configured";
      if (e instanceof ReceiptRejectedError) return "rejected";
      throw e;
    }
  }

  async setPush(device: string, platform: Platform, token: string, now = Date.now()): Promise<boolean> {
    await this.pruneDevices(now);
    if (!(device in this.devices) && Object.keys(this.devices).length >= MAX_DEVICES) return false;
    this.devices[device] = { platform, token, seen: now };
    await this.ctx.storage.put("devices", this.devices);
    await this.sweepSoon();
    return true;
  }

  async dropPush(device: string): Promise<void> {
    delete this.devices[device];
    await this.ctx.storage.put("devices", this.devices);
  }

  // Wakes the closed app that hosts the room, or the group's other devices
  // when the room has had no host yet, so one of them connects. True when a
  // push went out; a device is not woken twice within a few seconds.
  async wake(room: string, except: string, host?: string, now = Date.now()): Promise<boolean> {
    let sent = false;
    await this.pruneDevices(now);
    for (const [device, d] of Object.entries(this.devices)) {
      if (device === except || (host !== undefined && device !== host)) continue;
      if (now - (this.woken.get(device) ?? 0) < WAKE_GAP_MS) {
        sent = true;
        continue;
      }
      try {
        await services().push.send(d.platform, d.token, { room });
      } catch (e) {
        if (e instanceof TokenGoneError) await this.dropPush(device);
        continue;
      }
      this.woken.set(device, now);
      sent = true;
    }
    return sent;
  }
}
