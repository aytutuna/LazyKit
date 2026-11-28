// Just enough DER and CBOR to read an App Attest object: no generic parser, no encoder.

export class DecodeError extends Error {
  name = "DecodeError";
}

export interface Tlv {
  tag: number;
  body: Uint8Array;
  raw: Uint8Array;
}

export function readTlv(buf: Uint8Array, at = 0): Tlv & { next: number } {
  if (at + 2 > buf.length) throw new DecodeError("truncated");
  const tag = buf[at];
  if ((tag & 0x1f) === 0x1f) throw new DecodeError("long tags are not used");
  let len = buf[at + 1];
  let start = at + 2;
  if (len & 0x80) {
    const n = len & 0x7f;
    if (n === 0 || n > 4 || start + n > buf.length) throw new DecodeError("bad length");
    len = 0;
    for (let i = 0; i < n; i++) len = len * 256 + buf[start + i];
    start += n;
  }
  if (start + len > buf.length) throw new DecodeError("truncated");
  return { tag, body: buf.subarray(start, start + len), raw: buf.subarray(at, start + len), next: start + len };
}

export function children(body: Uint8Array): Tlv[] {
  const out: Tlv[] = [];
  for (let at = 0; at < body.length; ) {
    const t = readTlv(body, at);
    out.push(t);
    at = t.next;
  }
  return out;
}

function oid(body: Uint8Array): string {
  const parts: number[] = [];
  let n = 0;
  for (const b of body) {
    n = n * 128 + (b & 0x7f);
    if (!(b & 0x80)) {
      parts.push(n);
      n = 0;
    }
  }
  const first = parts.shift() ?? 0;
  return [first < 80 ? Math.floor(first / 40) : 2, first < 80 ? first % 40 : first - 80, ...parts].join(".");
}

function time(t: Tlv): number {
  const s = new TextDecoder().decode(t.body);
  const m = (t.tag === 0x17 ? /^(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})Z$/ : /^(\d{4})(\d{2})(\d{2})(\d{2})(\d{2})(\d{2})Z$/).exec(s);
  if (!m) throw new DecodeError("bad time");
  let year = Number(m[1]);
  if (t.tag === 0x17) year += year >= 50 ? 1900 : 2000;
  return Date.UTC(year, Number(m[2]) - 1, Number(m[3]), Number(m[4]), Number(m[5]), Number(m[6]));
}

export interface Cert {
  tbs: Uint8Array;
  sigAlg: string;
  signature: Uint8Array;
  issuer: Uint8Array;
  subject: Uint8Array;
  spki: Uint8Array;
  notBefore: number;
  notAfter: number;
  extensions: Map<string, Uint8Array>;
  ca: boolean;
}

export function parseCert(der: Uint8Array): Cert {
  const [tbs, alg, sig] = children(readTlv(der).body);
  if (!tbs || !alg || !sig || sig.tag !== 0x03) throw new DecodeError("not a certificate");
  const f = children(tbs.body);
  let i = f[0]?.tag === 0xa0 ? 1 : 0;
  i++; // serial
  i++; // signature algorithm, repeated
  const issuer = f[i++];
  const validityField = f[i++];
  const subject = f[i++];
  const spki = f[i++];
  const sigOid = children(alg.body)[0];
  if (!issuer || !validityField || !subject || !spki || !sigOid || sig.body.length === 0) throw new DecodeError("not a certificate");
  const validity = children(validityField.body);
  if (validity.length !== 2) throw new DecodeError("not a certificate");
  const extensions = new Map<string, Uint8Array>();
  let ca = false;
  const ext = f.find((x) => x.tag === 0xa3);
  for (const e of ext ? children(readTlv(ext.body).body) : []) {
    const [id, ...rest] = children(e.body);
    const value = rest[rest.length - 1];
    if (!id || !value || value.tag !== 0x04) throw new DecodeError("bad extension");
    extensions.set(oid(id.body), value.body);
  }
  const constraints = extensions.get("2.5.29.19");
  if (constraints) {
    const flag = children(readTlv(constraints).body)[0];
    ca = flag?.tag === 0x01 && flag.body[0] !== 0;
  }
  return {
    tbs: tbs.raw,
    sigAlg: oid(sigOid.body),
    signature: sig.body.subarray(1),
    issuer: issuer.raw,
    subject: subject.raw,
    spki: spki.raw,
    notBefore: time(validity[0]),
    notAfter: time(validity[1]),
    extensions,
    ca,
  };
}

// The uncompressed point of an EC public key, from its SubjectPublicKeyInfo.
export function ecPoint(spki: Uint8Array): Uint8Array {
  const key = children(readTlv(spki).body)[1];
  if (!key || key.tag !== 0x03) throw new DecodeError("not a public key");
  return key.body.subarray(1);
}

const CURVES: Record<string, { name: "P-256" | "P-384"; size: number }> = {
  "1.2.840.10045.3.1.7": { name: "P-256", size: 32 },
  "1.3.132.0.34": { name: "P-384", size: 48 },
};
const HASHES: Record<string, string> = { "1.2.840.10045.4.3.2": "SHA-256", "1.2.840.10045.4.3.3": "SHA-384" };

// Checks an X.509 signature (ECDSA, DER encoded) made by the key in `issuerSpki`.
export async function verifyCertSignature(cert: Cert, issuerSpki: Uint8Array): Promise<boolean> {
  const hash = HASHES[cert.sigAlg];
  const params = children(readTlv(issuerSpki).body)[0];
  const curve = CURVES[oid(children(params?.body ?? new Uint8Array())[1]?.body ?? new Uint8Array())];
  if (!hash || !curve) return false;
  const [r, s] = children(readTlv(cert.signature).body);
  if (!r || !s) return false;
  const raw = new Uint8Array(curve.size * 2);
  for (const [at, part] of [[0, r.body], [curve.size, s.body]] as const) {
    let v = part;
    while (v.length > curve.size && v[0] === 0) v = v.subarray(1);
    if (v.length > curve.size) return false;
    raw.set(v, at + curve.size - v.length);
  }
  const key = await crypto.subtle.importKey("spki", issuerSpki as Uint8Array<ArrayBuffer>, { name: "ECDSA", namedCurve: curve.name }, false, ["verify"]);
  return crypto.subtle.verify({ name: "ECDSA", hash }, key, raw, cert.tbs as Uint8Array<ArrayBuffer>);
}

export type Cbor = number | string | boolean | null | Uint8Array | Cbor[] | Map<string | number, Cbor>;

export function cborDecode(buf: Uint8Array): Cbor {
  let at = 0;
  const item = (depth: number): Cbor => {
    if (depth > 8 || at >= buf.length) throw new DecodeError("bad cbor");
    const head = buf[at++];
    const major = head >> 5;
    const info = head & 0x1f;
    let n = info;
    if (info >= 24 && info <= 27) {
      const size = 1 << (info - 24);
      if (at + size > buf.length) throw new DecodeError("truncated");
      n = 0;
      for (let i = 0; i < size; i++) n = n * 256 + buf[at++];
      if (!Number.isSafeInteger(n)) throw new DecodeError("too large");
    } else if (info > 27) {
      throw new DecodeError("indefinite lengths are not used");
    }
    switch (major) {
      case 0:
        return n;
      case 1:
        return -1 - n;
      case 2:
      case 3: {
        if (at + n > buf.length) throw new DecodeError("truncated");
        const bytes = buf.subarray(at, (at += n));
        if (major === 2) return bytes;
        try {
          return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
        } catch {
          throw new DecodeError("text is not UTF-8");
        }
      }
      case 4: {
        if (n > buf.length) throw new DecodeError("truncated");
        return Array.from({ length: n }, () => item(depth + 1));
      }
      case 5: {
        if (n > buf.length) throw new DecodeError("truncated");
        const map = new Map<string | number, Cbor>();
        for (let i = 0; i < n; i++) {
          const k = item(depth + 1);
          if (typeof k !== "string" && typeof k !== "number") throw new DecodeError("bad key");
          map.set(k, item(depth + 1));
        }
        return map;
      }
      case 7:
        if (info === 20 || info === 21) return info === 21;
        if (info === 22) return null;
        throw new DecodeError("unsupported");
      default:
        throw new DecodeError("unsupported");
    }
  };
  const out = item(0);
  if (at !== buf.length) throw new DecodeError("trailing bytes");
  return out;
}
