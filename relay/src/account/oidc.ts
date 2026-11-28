import { fromBase64Url, sha256 as sha256hex } from "./b64";
import { services } from "../services";

export type Provider = "apple" | "google";

export class IdentityRejectedError extends Error {
  name = "IdentityRejectedError";
}

export class ProviderNotConfiguredError extends Error {
  name = "ProviderNotConfiguredError";
}

export interface Identity {
  provider: Provider;
  sub: string;
  email?: string;
  exp: number;
}

const ISSUERS: Record<Provider, string[]> = {
  apple: ["https://appleid.apple.com"],
  google: ["https://accounts.google.com", "accounts.google.com"],
};
const MAX_TOKEN = 8192;
const LEEWAY_S = 60;

const list = (s?: string) => (s ?? "").split(",").map((x) => x.trim()).filter(Boolean);


function part(s: string): Record<string, unknown> {
  try {
    const v = JSON.parse(new TextDecoder().decode(fromBase64Url(s)));
    if (v && typeof v === "object" && !Array.isArray(v)) return v;
  } catch {}
  throw new IdentityRejectedError("token is not well formed");
}

// Checks an ID token as the provider documents it: signature against the published keys, issuer,
// audience (one of our client ids), expiry, and the nonce the app made for this sign-in (given
// as is or as its SHA-256, which is how Apple's apps pass it).
export async function verifyIdToken(provider: Provider, token: string, nonce: string, audiences: string | undefined, now = Date.now()): Promise<Identity> {
  const wanted = list(audiences);
  if (wanted.length === 0) throw new ProviderNotConfiguredError(`${provider} sign-in is not configured`);
  if (token.length > MAX_TOKEN) throw new IdentityRejectedError("token is too long");
  const pieces = token.split(".");
  if (pieces.length !== 3) throw new IdentityRejectedError("token is not well formed");
  const header = part(pieces[0]);
  if (header.alg !== "RS256" || typeof header.kid !== "string") throw new IdentityRejectedError("token is not signed with RS256");

  const jwk = await services().keys.get(provider, header.kid);
  if (!jwk) throw new IdentityRejectedError("unknown signing key");
  let signed: boolean;
  try {
    const key = await crypto.subtle.importKey("jwk", { kty: jwk.kty, n: jwk.n, e: jwk.e, alg: "RS256", ext: true }, { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }, false, ["verify"]);
    signed = await crypto.subtle.verify("RSASSA-PKCS1-v1_5", key, fromBase64Url(pieces[2]) as Uint8Array<ArrayBuffer>, new TextEncoder().encode(`${pieces[0]}.${pieces[1]}`));
  } catch {
    throw new IdentityRejectedError("bad signature");
  }
  if (!signed) throw new IdentityRejectedError("bad signature");

  const c = part(pieces[1]);
  if (typeof c.iss !== "string" || !ISSUERS[provider].includes(c.iss)) throw new IdentityRejectedError("wrong issuer");
  const aud = typeof c.aud === "string" ? [c.aud] : Array.isArray(c.aud) ? c.aud : [];
  if (!aud.some((a) => typeof a === "string" && wanted.includes(a))) throw new IdentityRejectedError("not made for this app");
  const nowS = Math.floor(now / 1000);
  if (typeof c.exp !== "number" || c.exp + LEEWAY_S < nowS) throw new IdentityRejectedError("token has expired");
  if (typeof c.iat === "number" && c.iat > nowS + LEEWAY_S) throw new IdentityRejectedError("token is from the future");
  if (typeof c.nonce !== "string" || nonce.length < 16 || (c.nonce !== nonce && c.nonce !== (await sha256hex(nonce)))) throw new IdentityRejectedError("nonce does not match");
  if (typeof c.sub !== "string" || c.sub.length === 0 || c.sub.length > 255) throw new IdentityRejectedError("no subject");

  // Apple sends the address only the first time, and as the string "true" for its flag.
  const verified = c.email_verified === true || c.email_verified === "true";
  const email = typeof c.email === "string" && verified ? c.email : undefined;
  return { provider, sub: c.sub, email, exp: c.exp };
}
