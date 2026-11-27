import { DecodeError, cborDecode, children, ecPoint, parseCert, readTlv, verifyCertSignature, type Cert } from "./asn1";

// Apple App Attestation Root CA, valid until 2045-03-15; SHA-256 1CB9823BA28BA6AD2D33A006941DE2AE4F513EF1D4E831B9F7E0FA7B6242C932.
export const APPLE_ROOT = `MIICITCCAaegAwIBAgIQC/O+DvHN0uD7jG5yH2IXmDAKBggqhkjOPQQDAzBSMSYwJAYDVQQDDB1BcHBsZSBBcHAgQXR0ZXN0YXRpb24gUm9vdCBDQTETMBEGA1UECgwKQXBwbGUgSW5jLjETMBEGA1UECAwKQ2FsaWZvcm5pYTAeFw0yMDAzMTgxODMyNTNaFw00NTAzMTUwMDAwMDBaMFIxJjAkBgNVBAMMHUFwcGxlIEFwcCBBdHRlc3RhdGlvbiBSb290IENBMRMwEQYDVQQKDApBcHBsZSBJbmMuMRMwEQYDVQQIDApDYWxpZm9ybmlhMHYwEAYHKoZIzj0CAQYFK4EEACIDYgAERTHhmLW07ATaFQIEVwTtT4dyctdhNbJhFs/Ii2FdCgAHGbpphY3+d8qjuDngIN3WVhQUBHAoMeQ/cLiP1sOUtgjqK9auYen1mMEvRq9Sk3Jm5X8U62H+xTD3FE9TgS41o0IwQDAPBgNVHRMBAf8EBTADAQH/MB0GA1UdDgQWBBSskRBTM72+aEH/pwyp5frq5eWKoTAOBgNVHQ8BAf8EBAMCAQYwCgYIKoZIzj0EAwMDaAAwZQIwQgFGnByvsiVbpTKwSga0kP0e8EeDS4+sQmTvb7vn53O5+FRXgeLhpJ06ysC5PrOyAjEAp5U4xDgEgllF7En3VcE3iexZZtKeYnpqtijVoyFraWVIyd/dganmrduC1bmTBGwD`;

const NONCE_EXTENSION = "1.2.840.113635.100.8.2";
const AAGUID = { production: "appattest\0\0\0\0\0\0\0", development: "appattestdevelop" };

export class AttestationError extends Error {
  name = "AttestationError";
}

export interface AppAttestConfig {
  // "<team id>.<bundle id>" of every app that may use the relay.
  appIds: string[];
  environment: "production" | "development";
  roots: Uint8Array[];
}

export const fromBase64 = (s: string): Uint8Array => {
  const std = s.replaceAll("-", "+").replaceAll("_", "/");
  return Uint8Array.from(atob(std.padEnd(Math.ceil(std.length / 4) * 4, "=")), (c) => c.charCodeAt(0));
};

export const toBase64Url = (b: Uint8Array): string => btoa(String.fromCharCode(...b)).replaceAll("+", "-").replaceAll("/", "_").replaceAll("=", "");

const sha256 = async (...parts: Uint8Array[]): Promise<Uint8Array> => {
  const all = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    all.set(p, at);
    at += p.length;
  }
  return new Uint8Array(await crypto.subtle.digest("SHA-256", all));
};

const same = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((v, i) => v === b[i]);

async function checkChain(chain: Cert[], roots: Cert[], now: number): Promise<void> {
  const valid = (c: Cert) => now >= c.notBefore && now <= c.notAfter;
  for (let i = 0; i < chain.length; i++) {
    if (i > 0 && !chain[i].ca) throw new AttestationError("an intermediate is not a CA");
    const last = i + 1 === chain.length;
    const issuer = last ? roots.find((r) => same(r.subject, chain[i].issuer)) : chain[i + 1];
    if (!issuer || !same(issuer.subject, chain[i].issuer) || !valid(chain[i]) || (last && !valid(issuer))) {
      throw new AttestationError("certificate chain does not lead to the Apple root");
    }
    if (!(await verifyCertSignature(chain[i], issuer.spki))) throw new AttestationError("certificate signature is wrong");
  }
}

// Checks an App Attest attestation as Apple documents it: the chain to Apple's root, the nonce
// made from the challenge, the key id, the app id, the counter and the environment. The
// `challenge` is what the relay issued; it is hashed here to make Apple's clientDataHash.
export async function verifyAppAttest(
  config: AppAttestConfig,
  challenge: Uint8Array,
  keyId: Uint8Array,
  attestation: Uint8Array,
  now = Date.now(),
): Promise<{ appId: string }> {
  try {
    return await check(config, challenge, keyId, attestation, now);
  } catch (e) {
    if (e instanceof DecodeError) throw new AttestationError(`unreadable attestation: ${e.message}`);
    throw e;
  }
}

async function check(config: AppAttestConfig, challenge: Uint8Array, keyId: Uint8Array, attestation: Uint8Array, now: number) {
  const obj = cborDecode(attestation);
  if (!(obj instanceof Map) || obj.get("fmt") !== "apple-appattest") throw new AttestationError("not an App Attest object");
  const stmt = obj.get("attStmt");
  const authData = obj.get("authData");
  const x5c = stmt instanceof Map ? stmt.get("x5c") : undefined;
  if (!(authData instanceof Uint8Array) || !Array.isArray(x5c) || x5c.length < 2 || x5c.length > 4 || !x5c.every((c) => c instanceof Uint8Array)) {
    throw new AttestationError("missing parts");
  }
  const chain = (x5c as Uint8Array[]).map(parseCert);
  await checkChain(chain, config.roots.map(parseCert), now);

  const leaf = chain[0];
  const nonce = await sha256(authData, await sha256(challenge));
  const ext = leaf.extensions.get(NONCE_EXTENSION);
  const tagged = ext && children(readTlv(ext).body)[0];
  const inner = tagged && tagged.tag === 0xa1 ? children(tagged.body)[0] : undefined;
  if (!inner || inner.tag !== 0x04 || !same(inner.body, nonce)) throw new AttestationError("nonce does not match the challenge");

  if (!same(await sha256(ecPoint(leaf.spki)), keyId)) throw new AttestationError("key id does not match the certificate");

  if (authData.length < 55) throw new AttestationError("authenticator data too short");
  const rpIdHash = authData.subarray(0, 32);
  const counter = new DataView(authData.buffer, authData.byteOffset + 33, 4).getUint32(0);
  const aaguid = new TextDecoder().decode(authData.subarray(37, 53));
  const idLength = (authData[53] << 8) | authData[54];
  const credentialId = authData.subarray(55, 55 + idLength);
  if (counter !== 0) throw new AttestationError("counter is not zero");
  if (aaguid !== AAGUID[config.environment]) throw new AttestationError("wrong App Attest environment");
  if (!same(credentialId, keyId)) throw new AttestationError("credential id does not match the key id");
  for (const appId of config.appIds) {
    if (same(rpIdHash, await sha256(new TextEncoder().encode(appId)))) return { appId };
  }
  throw new AttestationError("app is not one of ours");
}
