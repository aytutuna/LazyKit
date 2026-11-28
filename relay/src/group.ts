import { DurableObject } from "cloudflare:workers";
import { toBase64Url } from "./appattest";
import { TokenGoneError, services, type Platform } from "./services";

export { WEEKLY_CAP } from "./quota";
export const MAX_DEVICES = 32;
export const MAX_TOKEN = 4096;
const CHALLENGE_BYTES = 32;
const WAKE_GAP_MS = 10_000;
const DEVICE_KEEP_MS = 90 * 86_400_000;
const VERIFIED_KEEP_MS = 90 * 86_400_000;
const SEEN_REFRESH_MS = 86_400_000;
const SWEEP_MS = 86_400_000;
const CHALLENGE_MS = 5 * 60_000;
const MAX_CHALLENGES = 8;

interface Device {
  platform: Platform;
  token: string;
  seen: number;
}

// One set of a person's paired devices, tied to the account that made it: a push token per device
// and whether the app check has passed. The group is a random key the devices made when they first
// paired; the weekly count and the paid time belong to the account, not to the group.
export class Group extends DurableObject<Cloudflare.Env> {
  private account?: string;
  private id = "";
  private appCheck = false;
  private devices: Record<string, Device> = {};
  private woken = new Map<string, number>();
  private verifiedAt = 0;
  private seenAt = 0;
  // Kept in memory only: a group nobody has verified writes nothing, and a lost challenge costs one retry.
  private challenges = new Map<string, number>();

  constructor(ctx: DurableObjectState, env: Cloudflare.Env) {
    super(ctx, env);
    ctx.blockConcurrencyWhile(async () => {
      const kept = await ctx.storage.get(["account", "id", "appCheck", "devices", "verifiedAt", "seenAt"]);
      this.account = kept.get("account") as string | undefined;
      this.id = (kept.get("id") as string | undefined) ?? "";
      this.appCheck = (kept.get("appCheck") as boolean | undefined) ?? false;
      this.devices = (kept.get("devices") as Record<string, Device> | undefined) ?? {};
      this.verifiedAt = (kept.get("verifiedAt") as number | undefined) ?? 0;
      this.seenAt = (kept.get("seenAt") as number | undefined) ?? 0;
    });
  }

  private quota() {
    return this.env.QUOTAS.get(this.env.QUOTAS.idFromName(this.account!));
  }

  // A group belongs to the first account that claims it. Apps that can prove themselves to the
  // platform (iPhone, iPad, Android) say so, and then the group also needs the app check.
  async claim(id: string, account: string, platform: "apple" | "google" | "desktop", now = Date.now()): Promise<"ok" | "other" | "limit"> {
    if (this.account !== undefined && this.account !== account) return "other";
    if (!(await this.env.QUOTAS.get(this.env.QUOTAS.idFromName(account)).addGroup(id))) return "limit";
    this.account = account;
    this.id = id;
    this.appCheck ||= platform !== "desktop";
    this.seenAt = now;
    await this.ctx.storage.put({ account, id, appCheck: this.appCheck, seenAt: now });
    await this.sweepSoon();
    return "ok";
  }

  owner(): string | undefined {
    return this.account;
  }

  // Passing the app check is only asked of groups made by the phone apps; a group that keeps asking
  // stays known, so one that moves no bytes and has no push token is not forgotten by the sweep.
  async admit(now = Date.now()): Promise<"ok" | "attest"> {
    if (now - this.seenAt >= SEEN_REFRESH_MS) {
      this.seenAt = now;
      await this.ctx.storage.put("seenAt", now);
    }
    return !this.appCheck || now - this.verifiedAt < VERIFIED_KEEP_MS ? "ok" : "attest";
  }

  // Adds forwarded bytes to the account's week. Returns false once the account is over its cap,
  // so the room can close its streams.
  async record(bytes: number, now = Date.now()): Promise<boolean> {
    if (!this.account) return false;
    return this.quota().record(bytes, now);
  }

  async status(now = Date.now()) {
    if (!this.account) return { week: "", used: 0, cap: 0, paidUntil: 0, paid: false, capped: true, accountToken: "" };
    return this.quota().status(now);
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

  // Forgets old devices; a group nobody has used for a long time is deleted and the account lets go of it.
  async alarm(): Promise<void> {
    const now = Date.now();
    await this.pruneDevices(now);
    if (Object.keys(this.devices).length === 0 && now - this.seenAt >= VERIFIED_KEEP_MS) {
      if (this.account) await this.quota().removeGroup(this.id);
      await this.ctx.storage.deleteAll();
      this.account = undefined;
      this.appCheck = false;
      this.verifiedAt = 0;
      return;
    }
    await this.ctx.storage.setAlarm(now + SWEEP_MS);
  }

  // A one-time string the app has to bind its proof to; a few may be outstanding at once, for
  // a person's devices attesting together.
  newChallenge(now = Date.now()): string {
    for (const [c, until] of this.challenges) if (until <= now) this.challenges.delete(c);
    if (this.challenges.size >= MAX_CHALLENGES) this.challenges.delete(this.challenges.keys().next().value!);
    const challenge = toBase64Url(crypto.getRandomValues(new Uint8Array(CHALLENGE_BYTES)));
    this.challenges.set(challenge, now + CHALLENGE_MS);
    return challenge;
  }

  // True once for a challenge this group issued and has not yet taken.
  takeChallenge(challenge: string, now = Date.now()): boolean {
    const until = this.challenges.get(challenge);
    this.challenges.delete(challenge);
    return until !== undefined && until > now;
  }

  async setVerified(now = Date.now()): Promise<void> {
    this.verifiedAt = now;
    await this.ctx.storage.put("verifiedAt", now);
    await this.sweepSoon();
  }

  // A host or guest connecting shows the device is in use.
  async touch(device: string, now = Date.now()): Promise<void> {
    const d = this.devices[device];
    if (!d || now - d.seen < SEEN_REFRESH_MS) return;
    d.seen = now;
    await this.ctx.storage.put("devices", this.devices);
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

  export(id: string) {
    return { id, appCheck: this.appCheck, devices: Object.entries(this.devices).map(([device, d]) => ({ device, platform: d.platform, token: d.token, seen: d.seen })) };
  }

  // Returns how many push tokens went with it.
  async erase(): Promise<number> {
    const tokens = Object.keys(this.devices).length;
    await this.ctx.storage.deleteAlarm();
    await this.ctx.storage.deleteAll();
    this.account = undefined;
    this.devices = {};
    this.appCheck = false;
    this.verifiedAt = 0;
    this.seenAt = 0;
    return tokens;
  }
}
