import { APPLE_ROOT, fromBase64 } from "./appattest";
import type { IntegrityPayload } from "./integrity";

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
  // for this group: Apple's appAccountToken or Google's obfuscatedAccountId
  // is set to the group id at purchase time.
  verify(store: Store, receipt: string, group: string): Promise<{ until: number; purchase: string }>;
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
};

let current = notConfigured;

export const services = (): Services => current;

export function useServices(s?: Partial<Services>): void {
  current = { ...notConfigured, ...s };
}
