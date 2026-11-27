import { DurableObject } from "cloudflare:workers";
import { weekOf } from "./week";

const WEEK_MS = 7 * 86_400_000;

// One per desktop install key, named by a hash of the key: counts the new groups the install has
// verified this week and nothing else. It deletes itself when the week is over.
export class Install extends DurableObject<Cloudflare.Env> {
  async claim(max: number, now = Date.now()): Promise<boolean> {
    const week = weekOf(now);
    const kept = await this.ctx.storage.get<{ week: string; count: number }>("groups");
    const count = kept?.week === week ? kept.count : 0;
    if (count >= max) return false;
    await this.ctx.storage.put("groups", { week, count: count + 1 });
    await this.ctx.storage.setAlarm(now + WEEK_MS);
    return true;
  }

  async alarm(): Promise<void> {
    await this.ctx.storage.deleteAll();
  }
}
