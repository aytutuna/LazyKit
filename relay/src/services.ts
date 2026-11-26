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
  // Resolves to the time the purchase pays up to, in ms since the epoch.
  verify(store: Store, receipt: string): Promise<{ until: number }>;
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
