import { NotConfiguredError } from "../services";

export interface Turnstile {
  // `action` is what the widget on that page was made with, one per kind of request.
  verify(token: string, ip: string | null, action: string): Promise<boolean>;
}

// Cloudflare Turnstile's server check. A token made on another site or for another kind of request
// does not count: the answer has to name one of our hostnames and the action asked for. The secret is
// a Worker secret; the hostnames are a comma list in TURNSTILE_HOSTNAMES (alazy.app when unset).
export function cloudflareTurnstile(env: { TURNSTILE_SECRET?: string; TURNSTILE_HOSTNAMES?: string }): Turnstile {
  return {
    async verify(token, ip, action) {
      if (!env.TURNSTILE_SECRET) throw new NotConfiguredError("Turnstile is not configured");
      const form = new FormData();
      form.set("secret", env.TURNSTILE_SECRET);
      form.set("response", token);
      if (ip) form.set("remoteip", ip);
      const res = await fetch("https://challenges.cloudflare.com/turnstile/v0/siteverify", { method: "POST", body: form });
      if (!res.ok) return false;
      const out = (await res.json()) as { success?: boolean; hostname?: string; action?: string };
      const hostnames = (env.TURNSTILE_HOSTNAMES ?? "alazy.app").split(",").map((h) => h.trim());
      return out.success === true && typeof out.hostname === "string" && hostnames.includes(out.hostname) && out.action === action;
    },
  };
}
