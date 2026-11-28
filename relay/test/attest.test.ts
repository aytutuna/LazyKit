import { env, exports } from "cloudflare:workers";
import { runDurableObjectAlarm } from "cloudflare:test";
import { afterEach, describe, expect, it } from "vitest";
import { verifyAttestation } from "../src/attest";
import type { IntegrityPayload } from "../src/integrity";
import { useServices } from "../src/services";
import { groupSession, sessionFor } from "./helpers";
import { b64, makeAttestation, makePki } from "./pki";

const APP_ID = "8XT2U634Z4.com.lazysend.lazysend";
const DEVICE = "d".repeat(32);

const newGroup = () => Array.from(crypto.getRandomValues(new Uint8Array(16)), (b) => b.toString(16).padStart(2, "0")).join("");

// A group made by a phone app, which has to pass the app check; a computer's group does not.
async function phone(platform: "apple" | "google" = "apple") {
  const g = newGroup();
  const s = await groupSession(g, DEVICE, platform);
  const api = (path: string, body?: unknown, token = s.token) =>
    exports.default.fetch(`https://alazy.app/relay/v1/group/${g}/${path}`, { method: "POST", body: body === undefined ? undefined : JSON.stringify(body), headers: { Authorization: `Bearer ${token}` } });
  const challenge = async () => ((await (await api("challenge")).json()) as { challenge: string }).challenge;
  const room = (token = s.token) =>
    exports.default.fetch(`https://alazy.app/relay/v1/${g}${g}?role=host&group=${g}&device=${DEVICE}`, { headers: { Upgrade: "websocket", Authorization: `Bearer ${token}` } });
  return { g, ...s, api, challenge, room };
}

describe("app check at the relay", () => {
  afterEach(() => useServices());

  it("refuses rooms and push tokens of a phone group until it is verified, and says why", async () => {
    const p = await phone();
    const res = await p.room();
    expect(res.status).toBe(403);
    expect(res.headers.get("x-relay-reason")).toBe("attestation-required");
    const push = await p.api("push", { device: DEVICE, platform: "fcm", token: "t" });
    expect(push.status).toBe(403);
    expect(push.headers.get("x-relay-reason")).toBe("attestation-required");
    expect((await exports.default.fetch(`https://alazy.app/relay/v1/group/${p.g}`, { headers: p.headers })).status).toBe(200);
  });

  it("asks nothing of a computer's group", async () => {
    const g = newGroup();
    const s = await groupSession(g, DEVICE, "desktop");
    const res = await exports.default.fetch(`https://alazy.app/relay/v1/${g}${g}?role=host&group=${g}&device=${DEVICE}`, { headers: { Upgrade: "websocket", ...s.headers } });
    expect(res.status).toBe(101);
  });

  it("verifies a group with a good App Attest attestation", async () => {
    const pki = await makePki();
    useServices({ appleRoots: [pki.root] });
    const p = await phone();
    const c = await p.challenge();
    const { keyId, attestation } = await makeAttestation({ pki, appId: APP_ID, challenge: new TextEncoder().encode(c) });
    const res = await p.api("attest", { challenge: c, platform: "apple", keyId: b64(keyId), attestation: b64(attestation) });
    expect(res.status).toBe(204);
    expect((await p.room()).status).toBe(101);
  });

  it("rejects an app that is not ours, and the group stays unverified", async () => {
    const pki = await makePki();
    useServices({ appleRoots: [pki.root] });
    const p = await phone();
    const c = await p.challenge();
    const { keyId, attestation } = await makeAttestation({ pki, appId: "8XT2U634Z4.com.example.other", challenge: new TextEncoder().encode(c) });
    expect((await p.api("attest", { challenge: c, platform: "apple", keyId: b64(keyId), attestation: b64(attestation) })).status).toBe(422);
    expect((await p.room()).status).toBe(403);
  });

  it("takes each challenge once and refuses one it did not issue", async () => {
    const pki = await makePki();
    useServices({ appleRoots: [pki.root] });
    const p = await phone();
    const c = await p.challenge();
    const { keyId, attestation } = await makeAttestation({ pki, appId: APP_ID, challenge: new TextEncoder().encode(c) });
    const body = { challenge: c, platform: "apple", keyId: b64(keyId), attestation: b64(attestation) };
    expect((await p.api("attest", body)).status).toBe(204);
    expect((await p.api("attest", body)).status).toBe(400);
    expect((await (await phone()).api("attest", body)).status).toBe(400);
    expect((await p.api("attest", { ...body, challenge: b64(crypto.getRandomValues(new Uint8Array(32))) })).status).toBe(400);
  });

  it("refuses malformed attestation bodies, including the retired computer proof", async () => {
    const p = await phone();
    const c = await p.challenge();
    for (const body of [{ challenge: c }, { challenge: c, platform: "tpm" }, { challenge: c, platform: "apple", keyId: "x" }, { platform: "google", token: "t" }, { challenge: c, platform: "desktop", publicKey: "a", signature: "b" }]) {
      expect((await p.api("attest", body)).status).toBe(400);
    }
    expect((await exports.default.fetch(`https://alazy.app/relay/v1/group/${p.g}/challenge`, { headers: p.headers })).status).toBe(405);
  });

  it("lets only the group's account ask for challenges and proofs", async () => {
    const p = await phone();
    const stranger = await sessionFor("stranger@test.local", DEVICE);
    expect((await p.api("challenge", undefined, stranger)).status).toBe(403);
    expect((await p.api("attest", { challenge: "x", platform: "google", token: "t" }, stranger)).status).toBe(403);
    expect((await p.api("challenge", undefined, "")).status).toBe(401);
  });

  it("does not let another account's session into a verified group", async () => {
    const pki = await makePki();
    useServices({ appleRoots: [pki.root] });
    const p = await phone();
    const c = await p.challenge();
    const { keyId, attestation } = await makeAttestation({ pki, appId: APP_ID, challenge: new TextEncoder().encode(c) });
    await p.api("attest", { challenge: c, platform: "apple", keyId: b64(keyId), attestation: b64(attestation) });
    const stranger = await sessionFor("stranger@test.local", DEVICE);
    const res = await p.room(stranger);
    expect(res.status).toBe(403);
    expect(res.headers.get("x-relay-reason")).toBe("wrong-account");
  });

  it("keeps a verified group through the daily sweep", async () => {
    const pki = await makePki();
    useServices({ appleRoots: [pki.root] });
    const p = await phone();
    const c = await p.challenge();
    const { keyId, attestation } = await makeAttestation({ pki, appId: APP_ID, challenge: new TextEncoder().encode(c) });
    await p.api("attest", { challenge: c, platform: "apple", keyId: b64(keyId), attestation: b64(attestation) });
    const stub = env.GROUPS.get(env.GROUPS.idFromName(p.g));
    await runDurableObjectAlarm(stub);
    expect(await stub.admit()).toBe("ok");
  });

  it("asks again once a verified group has been quiet for 90 days", async () => {
    const p = await phone();
    const stub = env.GROUPS.get(env.GROUPS.idFromName(p.g));
    await stub.setVerified(Date.now() - 91 * 86_400_000);
    expect(await stub.admit()).toBe("attest");
  });

  const payload = (c: string, over: Partial<IntegrityPayload> = {}): IntegrityPayload => ({
    requestDetails: { requestPackageName: "com.lazysend.lazysend", nonce: c, timestampMillis: String(Date.now()) },
    appIntegrity: { appRecognitionVerdict: "PLAY_RECOGNIZED", packageName: "com.lazysend.lazysend" },
    deviceIntegrity: { deviceRecognitionVerdict: ["MEETS_DEVICE_INTEGRITY"] },
    ...over,
  });

  it("says Play Integrity is not configured until it is", async () => {
    const p = await phone("google");
    const c = await p.challenge();
    expect((await p.api("attest", { challenge: c, platform: "google", token: "t" })).status).toBe(501);
    expect((await p.room()).status).toBe(403);
  });

  it("verifies a Play Integrity verdict for the challenge, from an app of ours on a sound device", async () => {
    const seen: string[] = [];
    let make: (c: string) => IntegrityPayload = payload;
    let current = "";
    useServices({ integrity: { decode: async (t) => (seen.push(t), make(current)) } });
    const attempt = async (m: (c: string) => IntegrityPayload) => {
      const p = await phone("google");
      current = await p.challenge();
      make = m;
      const res = await p.api("attest", { challenge: current, platform: "google", token: "tok" });
      return [res.status, (await p.room()).status];
    };
    expect(await attempt((c) => payload(c))).toEqual([204, 101]);
    expect(seen).toEqual(["tok"]);
    const stale = (c: string) =>
      payload(c, { requestDetails: { requestPackageName: "com.lazysend.lazysend", nonce: c, timestampMillis: String(Date.now() - 3_600_000) } });
    for (const bad of [
      (c: string) => payload("other" + c),
      (c: string) => payload(c, { appIntegrity: { appRecognitionVerdict: "UNRECOGNIZED_VERSION", packageName: "com.lazysend.lazysend" } }),
      (c: string) => payload(c, { appIntegrity: { appRecognitionVerdict: "PLAY_RECOGNIZED", packageName: "com.example.other" } }),
      (c: string) => payload(c, { deviceIntegrity: { deviceRecognitionVerdict: [] } }),
      stale,
    ]) {
      expect(await attempt(bad)).toEqual([422, 403]);
    }
  });

  it("says App Attest is not configured when no app ids are set", async () => {
    const out = await verifyAttestation({ ...env, APPLE_APP_IDS: "" }, b64(crypto.getRandomValues(new Uint8Array(32))), { platform: "apple", keyId: "a", attestation: "a" });
    expect(out).toBe("not-configured");
  });
});
