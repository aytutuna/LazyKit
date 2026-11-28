import type { PublishedKey } from "../../src/account/core/oidc";

const b64u = (b: Uint8Array | string) =>
  btoa(typeof b === "string" ? b : String.fromCharCode(...b)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

const rsa = () => crypto.subtle.generateKey({ name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" }, true, ["sign", "verify"]);

// A pretend identity provider: one published key, k1, and a second key that is not published.
export async function jwtKit() {
  const good = await rsa();
  const other = await rsa();
  const published: PublishedKey = { ...(await crypto.subtle.exportKey("jwk", good.publicKey)), kid: "k1" };
  const keys = { get: async (_p: "apple" | "google", kid: string) => (kid === "k1" ? published : undefined) };

  async function sign(claims: Record<string, unknown>, opts: { kid?: string; wrongKey?: boolean; alg?: string } = {}): Promise<string> {
    const head = b64u(JSON.stringify({ alg: opts.alg ?? "RS256", kid: opts.kid ?? "k1" }));
    const body = b64u(JSON.stringify(claims));
    const key = (opts.wrongKey ? other : good).privateKey;
    const sig = new Uint8Array(await crypto.subtle.sign("RSASSA-PKCS1-v1_5", key, new TextEncoder().encode(`${head}.${body}`)));
    return `${head}.${body}.${b64u(sig)}`;
  }
  return { keys, sign };
}
