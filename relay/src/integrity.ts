import { AttestationError } from "./appattest";

// The relevant part of what Google's decodeIntegrityToken returns.
export interface IntegrityPayload {
  requestDetails?: { requestPackageName?: string; nonce?: string; timestampMillis?: string };
  appIntegrity?: { appRecognitionVerdict?: string; packageName?: string };
  deviceIntegrity?: { deviceRecognitionVerdict?: string[] };
}

const MAX_AGE_MS = 5 * 60_000;

// The app gives Play Integrity the challenge string itself as the nonce, so the verdict is bound to one challenge.
export function checkIntegrity(payload: IntegrityPayload, packages: string[], challenge: string, now = Date.now()): void {
  const req = payload.requestDetails;
  const app = payload.appIntegrity;
  if (req?.nonce !== challenge) throw new AttestationError("nonce does not match the challenge");
  const age = now - Number(req.timestampMillis);
  if (!(age >= -MAX_AGE_MS && age <= MAX_AGE_MS)) throw new AttestationError("verdict is not recent");
  if (!app?.packageName || !packages.includes(app.packageName) || req.requestPackageName !== app.packageName) {
    throw new AttestationError("app is not one of ours");
  }
  if (app.appRecognitionVerdict !== "PLAY_RECOGNIZED") throw new AttestationError("app is not recognized by Google Play");
  if (!payload.deviceIntegrity?.deviceRecognitionVerdict?.includes("MEETS_DEVICE_INTEGRITY")) {
    throw new AttestationError("device does not meet integrity");
  }
}
