import { describe, expect, it } from "vitest";
import { APPLE_ROOT, AttestationError, fromBase64, verifyAppAttest, type AppAttestConfig } from "../src/appattest";
import { parseCert, verifyCertSignature } from "../src/asn1";
import { makeAttestation, makePki, newKey, sha256, type Pki } from "./pki";

const APP_ID = "8XT2U634Z4.com.lazysend.lazysend";
const challenge = () => crypto.getRandomValues(new Uint8Array(32));

const config = (pki: Pki, over: Partial<AppAttestConfig> = {}): AppAttestConfig => ({
  appIds: [APP_ID],
  environment: "production",
  roots: [pki.root],
  ...over,
});

describe("App Attest", () => {
  it("accepts an attestation made for the challenge and names the app", async () => {
    const pki = await makePki();
    const c = challenge();
    const { keyId, attestation } = await makeAttestation({ pki, appId: APP_ID, challenge: c });
    expect(await verifyAppAttest(config(pki), c, keyId, attestation)).toEqual({ appId: APP_ID });
  });

  it("refuses certificates with missing fields instead of throwing a type error", async () => {
    const pki = await makePki();
    const enc = new TextEncoder();
    const bad = [
      new Uint8Array([0x30, 0x06, 0x30, 0x00, 0x30, 0x00, 0x03, 0x00]),
      new Uint8Array([0x30, 0x03, 0x30, 0x00, 0x30, 0x00]),
      new Uint8Array([0x30, 0x00]),
    ];
    for (const cert of bad) {
      const item = (b: Uint8Array) => [0x40 + b.length, ...b];
      const parts = [0xa3, 0x63, ...enc.encode("fmt"), 0x6f, ...enc.encode("apple-appattest"), 0x67, ...enc.encode("attStmt"), 0xa1, 0x63, ...enc.encode("x5c"), 0x82, ...item(cert), ...item(cert), 0x68, ...enc.encode("authData"), 0x40];
      await expect(verifyAppAttest(config(pki), new Uint8Array(1), new Uint8Array(32), new Uint8Array(parts))).rejects.toBeInstanceOf(AttestationError);
    }
  });

  type Made = { keyId: Uint8Array; attestation: Uint8Array };
  const refused = async (reason: RegExp, make: (pki: Pki, c: Uint8Array) => Promise<Made>, over?: (pki: Pki) => Partial<AppAttestConfig>, asked?: Uint8Array) => {
    const pki = await makePki();
    const c = challenge();
    const { keyId, attestation } = await make(pki, c);
    await expect(verifyAppAttest(config(pki, over?.(pki)), asked ?? c, keyId, attestation)).rejects.toThrow(reason);
  };

  it("refuses another challenge", () => refused(/nonce/, (pki, c) => makeAttestation({ pki, appId: APP_ID, challenge: c }), undefined, challenge()));
  it("refuses a nonce made from another challenge", () =>
    refused(/nonce/, (pki, c) => makeAttestation({ pki, appId: APP_ID, challenge: c, nonceChallenge: challenge() })));
  it("refuses an app that is not listed", () =>
    refused(/not one of ours/, (pki, c) => makeAttestation({ pki, appId: "8XT2U634Z4.com.example.other", challenge: c })));
  it("refuses a counter above zero", () => refused(/counter/, (pki, c) => makeAttestation({ pki, appId: APP_ID, challenge: c, counter: 1 })));
  it("refuses the development environment in production", () =>
    refused(/environment/, (pki, c) => makeAttestation({ pki, appId: APP_ID, challenge: c, environment: "development" })));
  it("refuses a key id that is not the certificate's key", () =>
    refused(/key id/, async (pki, c) => ({ ...(await makeAttestation({ pki, appId: APP_ID, challenge: c })), keyId: new Uint8Array(32) })));
  it("refuses a key id of another key", () =>
    refused(/key id/, async (pki, c) => makeAttestation({ pki, appId: APP_ID, challenge: c, keyIdOf: await newKey("P-256") })));
  it("refuses a chain that leads to another root", async () => {
    const other = await makePki();
    await refused(/signature/, (pki, c) => makeAttestation({ pki, appId: APP_ID, challenge: c }), () => ({ roots: [other.root] }));
  });
  it("refuses a leaf that the intermediate did not sign", () =>
    refused(/signature/, async (pki, c) => makeAttestation({ pki, appId: APP_ID, challenge: c, leafSigner: await newKey("P-384") })));
  it("refuses an expired certificate", () =>
    refused(/root/, (pki, c) => makeAttestation({ pki, appId: APP_ID, challenge: c, notAfter: Date.now() - 1000 })));
  it("refuses an intermediate that is not a CA", async () => {
    const pki = await makePki({ intermediateCa: false });
    const c = challenge();
    const { keyId, attestation } = await makeAttestation({ pki, appId: APP_ID, challenge: c });
    await expect(verifyAppAttest(config(pki), c, keyId, attestation)).rejects.toThrow(/CA/);
  });
  it("accepts the development environment when it is the configured one", async () => {
    const pki = await makePki();
    const c = challenge();
    const { keyId, attestation } = await makeAttestation({ pki, appId: APP_ID, challenge: c, environment: "development" });
    await verifyAppAttest(config(pki, { environment: "development" }), c, keyId, attestation);
  });
  it("refuses what is not an attestation", async () => {
    const pki = await makePki();
    for (const junk of [new Uint8Array(), new Uint8Array([0xa0]), new Uint8Array([0xff, 0xff]), crypto.getRandomValues(new Uint8Array(300))]) {
      await expect(verifyAppAttest(config(pki), challenge(), new Uint8Array(32), junk)).rejects.toThrow(AttestationError);
    }
  });

  it("carries Apple's root, which signs itself", async () => {
    const root = fromBase64(APPLE_ROOT);
    const cert = parseCert(root);
    const hex = Array.from(await sha256(root), (b) => b.toString(16).padStart(2, "0")).join("");
    expect(hex).toBe("1cb9823ba28ba6ad2d33a006941de2ae4f513ef1d4e831b9f7e0fa7b6242c932");
    expect(new Date(cert.notAfter).toISOString()).toBe("2045-03-15T00:00:00.000Z");
    expect(cert.ca).toBe(true);
    expect(await verifyCertSignature(cert, cert.spki)).toBe(true);
  });
});
