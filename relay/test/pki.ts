// Builds certificates and App Attest objects for the attestation tests, since Apple publishes no
// sample that chains to its root.
import { toBase64Url } from "../src/appattest";

const enc = new TextEncoder();

export function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

export function der(tag: number, ...parts: Uint8Array[]): Uint8Array {
  const body = concat(...parts);
  const len = body.length < 128 ? [body.length] : body.length < 256 ? [0x81, body.length] : [0x82, body.length >> 8, body.length & 0xff];
  return concat(new Uint8Array([tag, ...len]), body);
}

export const sha256 = async (...parts: Uint8Array[]) => new Uint8Array(await crypto.subtle.digest("SHA-256", concat(...parts) as Uint8Array<ArrayBuffer>));

function oid(dotted: string): Uint8Array {
  const n = dotted.split(".").map(Number);
  const out = [n[0] * 40 + n[1]];
  for (const v of n.slice(2)) {
    const bytes = [v & 0x7f];
    for (let r = v >> 7; r > 0; r >>= 7) bytes.unshift((r & 0x7f) | 0x80);
    out.push(...bytes);
  }
  return der(0x06, new Uint8Array(out));
}

const name = (cn: string) => der(0x30, der(0x31, der(0x30, oid("2.5.4.3"), der(0x0c, enc.encode(cn)))));
const utc = (t: number) => {
  const iso = new Date(t).toISOString();
  return der(0x17, enc.encode(`${iso.slice(2, 4)}${iso.slice(5, 7)}${iso.slice(8, 10)}${iso.slice(11, 13)}${iso.slice(14, 16)}${iso.slice(17, 19)}Z`));
};

const SIG_ALGS = { "SHA-256": "1.2.840.10045.4.3.2", "SHA-384": "1.2.840.10045.4.3.3" } as const;

export interface Key {
  pair: CryptoKeyPair;
  spki: Uint8Array;
  point: Uint8Array;
  curve: "P-256" | "P-384";
}

export async function newKey(curve: "P-256" | "P-384" = "P-256"): Promise<Key> {
  const pair = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: curve }, true, ["sign", "verify"])) as CryptoKeyPair;
  return {
    pair,
    curve,
    spki: new Uint8Array(await crypto.subtle.exportKey("spki", pair.publicKey)),
    point: new Uint8Array(await crypto.subtle.exportKey("raw", pair.publicKey)),
  };
}

function derSignature(raw: Uint8Array): Uint8Array {
  const half = raw.length / 2;
  const int = (v: Uint8Array) => {
    let i = 0;
    while (i < v.length - 1 && v[i] === 0) i++;
    const t = v.subarray(i);
    return der(0x02, t[0] & 0x80 ? concat(new Uint8Array([0]), t) : t);
  };
  return der(0x30, int(raw.subarray(0, half)), int(raw.subarray(half)));
}

export interface CertOptions {
  subject: string;
  issuer: string;
  key: Key;
  signer: Key;
  ca?: boolean;
  notAfter?: number;
  extensions?: { oid: string; value: Uint8Array }[];
}

export async function makeCert(o: CertOptions): Promise<Uint8Array> {
  const hash = o.signer.curve === "P-384" ? "SHA-384" : "SHA-256";
  const alg = der(0x30, oid(SIG_ALGS[hash]));
  const constraints = o.ca === undefined ? [] : [{ oid: "2.5.29.19", value: der(0x30, ...(o.ca ? [der(0x01, new Uint8Array([0xff]))] : [])) }];
  const exts = [...constraints, ...(o.extensions ?? [])];
  const tbs = der(
    0x30,
    der(0xa0, der(0x02, new Uint8Array([2]))),
    der(0x02, new Uint8Array([1])),
    alg,
    name(o.issuer),
    der(0x30, utc(Date.now() - 86_400_000), utc(o.notAfter ?? Date.now() + 86_400_000)),
    name(o.subject),
    o.key.spki,
    der(0xa3, der(0x30, ...exts.map((e) => der(0x30, oid(e.oid), der(0x04, e.value))))),
  );
  const raw = new Uint8Array(await crypto.subtle.sign({ name: "ECDSA", hash }, o.signer.pair.privateKey, tbs as Uint8Array<ArrayBuffer>));
  return der(0x30, tbs, alg, der(0x03, concat(new Uint8Array([0]), derSignature(raw))));
}

export type Cbor = number | string | Uint8Array | Cbor[] | Map<string, Cbor>;

export function cbor(v: Cbor): Uint8Array {
  const head = (major: number, n: number) =>
    n < 24 ? new Uint8Array([(major << 5) | n]) : n < 256 ? new Uint8Array([(major << 5) | 24, n]) : new Uint8Array([(major << 5) | 25, n >> 8, n & 0xff]);
  if (typeof v === "number") return head(0, v);
  if (typeof v === "string") return concat(head(3, enc.encode(v).length), enc.encode(v));
  if (v instanceof Uint8Array) return concat(head(2, v.length), v);
  if (Array.isArray(v)) return concat(head(4, v.length), ...v.map(cbor));
  return concat(head(5, v.size), ...[...v].flatMap(([k, x]) => [cbor(k), cbor(x)]));
}

export interface Pki {
  root: Uint8Array;
  intermediate: Uint8Array;
  intermediateKey: Key;
}

export async function makePki(opts: { intermediateCa?: boolean } = {}): Promise<Pki> {
  const rootKey = await newKey("P-384");
  const intermediateKey = await newKey("P-384");
  const root = await makeCert({ subject: "Test Attestation Root", issuer: "Test Attestation Root", key: rootKey, signer: rootKey, ca: true });
  const intermediate = await makeCert({
    subject: "Test Attestation CA",
    issuer: "Test Attestation Root",
    key: intermediateKey,
    signer: rootKey,
    ca: opts.intermediateCa ?? true,
  });
  return { root, intermediate, intermediateKey };
}

export interface AttestOptions {
  pki: Pki;
  appId: string;
  challenge: Uint8Array;
  counter?: number;
  environment?: "production" | "development";
  nonceChallenge?: Uint8Array;
  keyIdOf?: Key;
  notAfter?: number;
  leafSigner?: Key;
}

// The object the app sends: `keyId` is the SHA-256 of the leaf's public key, as App Attest makes it.
export async function makeAttestation(o: AttestOptions): Promise<{ keyId: Uint8Array; attestation: Uint8Array }> {
  const key = await newKey("P-256");
  const keyId = await sha256((o.keyIdOf ?? key).point);
  const aaguid = enc.encode(o.environment === "development" ? "appattestdevelop" : "appattest\0\0\0\0\0\0\0");
  const counter = new Uint8Array(4);
  new DataView(counter.buffer).setUint32(0, o.counter ?? 0);
  const authData = concat(await sha256(enc.encode(o.appId)), new Uint8Array([0x40]), counter, aaguid, new Uint8Array([0, keyId.length]), keyId);
  const nonce = await sha256(authData, await sha256(o.nonceChallenge ?? o.challenge));
  const leaf = await makeCert({
    subject: "Test Credential",
    issuer: "Test Attestation CA",
    key,
    signer: o.leafSigner ?? o.pki.intermediateKey,
    notAfter: o.notAfter,
    extensions: [{ oid: "1.2.840.113635.100.8.2", value: der(0x30, der(0xa1, der(0x04, nonce))) }],
  });
  const attestation = cbor(
    new Map<string, Cbor>([
      ["fmt", "apple-appattest"],
      ["attStmt", new Map<string, Cbor>([["x5c", [leaf, o.pki.intermediate]], ["receipt", new Uint8Array(8)]])],
      ["authData", authData],
    ]),
  );
  return { keyId, attestation };
}

export const b64 = toBase64Url;
