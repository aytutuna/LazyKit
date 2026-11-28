import { AttestationError, fromBase64, verifyAppAttest, type AppAttestConfig } from "./appattest";
import { checkIntegrity } from "./integrity";
import { NotConfiguredError, services } from "./services";

export type Attestation =
  | { platform: "apple"; keyId: string; attestation: string }
  | { platform: "google"; token: string };

export type Outcome = "ok" | "not-configured" | "rejected";

const list = (s?: string) => (s ?? "").split(",").map((x) => x.trim()).filter(Boolean);

export function parseAttestation(body: Record<string, unknown>): Attestation | undefined {
  const text = (k: string) => (typeof body[k] === "string" && body[k] !== "" && (body[k] as string).length <= 16_384 ? (body[k] as string) : undefined);
  if (body.platform === "apple") {
    const keyId = text("keyId");
    const attestation = text("attestation");
    return keyId && attestation ? { platform: "apple", keyId, attestation } : undefined;
  }
  if (body.platform === "google") {
    const token = text("token");
    return token ? { platform: "google", token } : undefined;
  }
  return undefined;
}

// Checks the proof that this request comes from the real app. The challenge has been consumed by the caller.
export async function verifyAttestation(env: Cloudflare.Env, challenge: string, a: Attestation): Promise<Outcome> {
  try {
    // App Attest's client data is the challenge text itself, so every platform hands the same string over.
    const bytes = new TextEncoder().encode(challenge);
    if (a.platform === "apple") {
      const appIds = list(env.APPLE_APP_IDS);
      if (appIds.length === 0) return "not-configured";
      const config: AppAttestConfig = {
        appIds,
        environment: env.APPLE_ATTEST_ENV === "development" ? "development" : "production",
        roots: services().appleRoots,
      };
      await verifyAppAttest(config, bytes, fromBase64(a.keyId), fromBase64(a.attestation));
      return "ok";
    }
    if (a.platform === "google") {
      const packages = list(env.PLAY_PACKAGES);
      if (packages.length === 0) return "not-configured";
      checkIntegrity(await services().integrity.decode(a.token), packages, challenge);
      return "ok";
    }
    return "rejected";
  } catch (e) {
    if (e instanceof NotConfiguredError) return "not-configured";
    // Bad base64 or a key that is not on the curve (DOMException), or a proof the check refuses.
    if (e instanceof AttestationError || e instanceof DOMException) return "rejected";
    throw e;
  }
}
