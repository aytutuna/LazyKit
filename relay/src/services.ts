import { APPLE_ROOT, fromBase64 } from "./appattest";
import type { IntegrityPayload } from "./integrity";
import type { KeySource, PublishedKey } from "./account/core/oidc";
import type { Mailer } from "./account/core/email";

export type Store = "apple" | "google" | "microsoft";
export type Platform = "apns" | "fcm";

export class NotConfiguredError extends Error {
  name = "NotConfiguredError";
}

export class ReceiptRejectedError extends Error {
  name = "ReceiptRejectedError";
}

export class TokenGoneError extends Error {
  name = "TokenGoneError";
}

export interface PurchaseVerifier {
  // Resolves to the time the purchase pays up to, in ms since the epoch, and
  // the store's own id for the purchase (Apple's original transaction id,
  // Google's purchase token, Microsoft's purchase id for the buyer, not the
  // add-on's product Store ID, which every buyer shares), which stays the same
  // across renewals. A real check also confirms that the purchase was made
  // for this account: Apple's appAccountToken or Google's obfuscatedAccountId
  // is set to the relay's account token at purchase time.
  verify(store: Store, receipt: string, accountToken: string): Promise<{ until: number; purchase: string }>;
}

export interface PushSender {
  send(platform: Platform, token: string, message: { room: string }): Promise<void>;
}

export interface IntegrityDecoder {
  // Has Google decode a Play Integrity token; needs a service account the relay does not hold yet.
  decode(token: string): Promise<IntegrityPayload>;
}

export interface Services {
  verifier: PurchaseVerifier;
  push: PushSender;
  integrity: IntegrityDecoder;
  // The roots App Attest chains must lead to; only tests use another.
  appleRoots: Uint8Array[];
  keys: KeySource;
  // Left unset, the account service sends through Resend with its own environment.
  mailer?: Mailer;
}

// The stores' receipt checks and the APNs and FCM clients need keys that are
// not set up yet; until they are, every call says so.
const notConfigured: Services = {
  verifier: {
    async verify(store) {
      throw new NotConfiguredError(`${store} receipts are not configured`);
    },
  },
  push: {
    async send(platform) {
      throw new NotConfiguredError(`${platform} is not configured`);
    },
  },
  integrity: {
    async decode() {
      throw new NotConfiguredError("Play Integrity is not configured");
    },
  },
  appleRoots: [fromBase64(APPLE_ROOT)],
  keys: publishedKeys(),
};

let current = notConfigured;

export const services = (): Services => current;

export function useServices(s?: Partial<Services>): void {
  current = { ...notConfigured, ...s };
}

const KEY_URLS = { apple: "https://appleid.apple.com/auth/keys", google: "https://www.googleapis.com/oauth2/v3/certs" };
const KEYS_KEEP_MS = 3_600_000;
const KEYS_RETRY_MS = 300_000;
const KEYS_FAILED_MS = 30_000;

// Providers rotate keys now and then, so an unknown id triggers one fetch, at most every few minutes.
function publishedKeys(): KeySource {
  const cache = new Map<string, { keys: PublishedKey[]; at: number }>();
  const failed = new Map<string, number>();
  return {
    async get(provider, kid) {
      const now = Date.now();
      const kept = cache.get(provider);
      const find = (keys: PublishedKey[]) => keys.find((k) => k.kid === kid);
      if (kept && now - kept.at < KEYS_KEEP_MS && find(kept.keys)) return find(kept.keys);
      if (kept && now - kept.at < KEYS_RETRY_MS) return undefined;
      // A provider that is down is not asked again for a little while.
      if (now - (failed.get(provider) ?? 0) < KEYS_FAILED_MS) throw new Error(`${provider} keys are unavailable`);
      try {
        const res = await fetch(KEY_URLS[provider]);
        if (!res.ok) throw new Error(`${provider} keys answered ${res.status}`);
        const keys = ((await res.json()) as { keys: PublishedKey[] }).keys;
        cache.set(provider, { keys, at: now });
        return find(keys);
      } catch (e) {
        failed.set(provider, now);
        throw e;
      }
    },
  };
}
