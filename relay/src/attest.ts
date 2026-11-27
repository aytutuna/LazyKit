import { AttestationError, fromBase64, verifyAppAttest, type AppAttestConfig } from "./appattest";
import { checkIntegrity } from "./integrity";
import { NotConfiguredError, services } from "./services";

// New groups an install key may verify in a week.
export const INSTALL_GROUPS_PER_WEEK = 5;

export type Attestation =
  | { platform: "apple"; keyId: string; attestation: string }
  | { platform: "google"; token: string }
  | { platform: "desktop"; publicKey: string; signature: string };

export type Outcome = "ok" | "not-configured" | "rejected" | "limit";

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
  if (body.platform === "desktop") {
    const publicKey = text("publicKey");
    const signature = text("signature");
    return publicKey && signature ? { platform: "desktop", publicKey, signature } : undefined;
  }
  return undefined;
}

// What a desktop install signs: the group and the challenge, so a signature fits one request.
export const desktopMessage = (group: string, challenge: string) => new TextEncoder().encode(`lazy relay install v1\0${group}\0${challenge}`);

const installId = async (publicKey: Uint8Array) =>
  Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", new Uint8Array([...new TextEncoder().encode("lazy relay install id v1\0"), ...publicKey]))), (b) => b.toString(16).padStart(2, "0")).join("");

// Checks the proof that this request comes from the real app (or, on a computer, from an install
// that holds a key). The challenge has been consumed by the caller.
export async function verifyAttestation(env: Cloudflare.Env, group: string, challenge: string, a: Attestation): Promise<Outcome> {
  try {
    const bytes = fromBase64(challenge);
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
    const publicKey = fromBase64(a.publicKey);
    const key = await crypto.subtle.importKey("raw", publicKey as Uint8Array<ArrayBuffer>, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
    const signed = await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, key, fromBase64(a.signature) as Uint8Array<ArrayBuffer>, desktopMessage(group, challenge) as Uint8Array<ArrayBuffer>);
    if (!signed) return "rejected";
    const install = env.INSTALLS.get(env.INSTALLS.idFromName(await installId(publicKey)));
    return (await install.claim(INSTALL_GROUPS_PER_WEEK)) ? "ok" : "limit";
  } catch (e) {
    if (e instanceof NotConfiguredError) return "not-configured";
    // Bad base64 or a key that is not on the curve (DOMException), or a proof the check refuses.
    if (e instanceof AttestationError || e instanceof DOMException) return "rejected";
    throw e;
  }
}
