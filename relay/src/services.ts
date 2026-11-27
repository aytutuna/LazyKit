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

export interface Services {
  verifier: PurchaseVerifier;
  push: PushSender;
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
};

let current = notConfigured;

export const services = (): Services => current;

export function useServices(s?: Partial<Services>): void {
  current = { ...notConfigured, ...s };
}
