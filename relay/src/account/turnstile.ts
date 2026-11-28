import { NotConfiguredError } from "../services";

export interface Turnstile {
  verify(token: string, ip: string | null): Promise<boolean>;
}

// Cloudflare Turnstile's server check; the secret is a Worker secret.
export function cloudflareTurnstile(env: { TURNSTILE_SECRET?: string }): Turnstile {
  return {
    async verify(token, ip) {
      if (!env.TURNSTILE_SECRET) throw new NotConfiguredError("Turnstile is not configured");
      const form = new FormData();
      form.set("secret", env.TURNSTILE_SECRET);
      form.set("response", token);
      if (ip) form.set("remoteip", ip);
      const res = await fetch("https://challenges.cloudflare.com/turnstile/v0/siteverify", { method: "POST", body: form });
      return res.ok && ((await res.json()) as { success?: boolean }).success === true;
    },
  };
}
