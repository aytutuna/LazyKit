import { env, exports } from "cloudflare:workers";
import { afterEach, describe, expect, it } from "vitest";
import { fromBase64 } from "../src/appattest";
import { INSTALL_GROUPS_PER_WEEK, desktopMessage, verifyAttestation } from "../src/attest";
import type { IntegrityPayload } from "../src/integrity";
import { useServices } from "../src/services";
import { b64, makeAttestation, makePki, newKey } from "./pki";

const APP_ID = "8XT2U634Z4.com.lazysend.lazysend";
const challenge = () => crypto.getRandomValues(new Uint8Array(32));

// The whole exchange through the worker.
const newGroup = () => Array.from(crypto.getRandomValues(new Uint8Array(16)), (b) => b.toString(16).padStart(2, "0")).join("");
const api = (g: string, path: string, body?: unknown, headers: Record<string, string> = {}) =>
  exports.default.fetch(`https://alazy.app/relay/v1/group/${g}/${path}`, { method: "POST", body: body === undefined ? undefined : JSON.stringify(body), headers });
const challengeOf = async (g: string) => ((await (await api(g, "challenge")).json()) as { challenge: string }).challenge;
const wantsRoom = (g: string) =>
  exports.default.fetch(`https://alazy.app/relay/v1/${g}${g}?role=host&group=${g}&device=${"d".repeat(32)}`, { headers: { Upgrade: "websocket" } });

async function desktop(g: string, key?: Awaited<ReturnType<typeof newKey>>) {
  const k = key ?? (await newKey());
  const c = await challengeOf(g);
  const signature = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, k.pair.privateKey, desktopMessage(g, c) as Uint8Array<ArrayBuffer>);
  return { challenge: c, platform: "desktop", publicKey: b64(k.point), signature: b64(new Uint8Array(signature)) };
}

describe("app check at the relay", () => {
  afterEach(() => useServices());

  it("refuses rooms and push tokens until the group is verified, and says why", async () => {
    const g = newGroup();
    const res = await wantsRoom(g);
    expect(res.status).toBe(403);
    expect(res.headers.get("x-relay-reason")).toBe("attestation-required");
    const push = await api(g, "push", { device: "d".repeat(32), platform: "fcm", token: "t" });
    expect(push.status).toBe(403);
    expect(push.headers.get("x-relay-reason")).toBe("attestation-required");
    expect((await exports.default.fetch(`https://alazy.app/relay/v1/group/${g}`)).status).toBe(200);
  });

  it("verifies a group with a good App Attest attestation", async () => {
    const pki = await makePki();
    useServices({ appleRoots: [pki.root] });
    const g = newGroup();
    const c = await challengeOf(g);
    const { keyId, attestation } = await makeAttestation({ pki, appId: APP_ID, challenge: fromBase64(c) });
    const res = await api(g, "attest", { challenge: c, platform: "apple", keyId: b64(keyId), attestation: b64(attestation) });
    expect(res.status).toBe(204);
    expect((await wantsRoom(g)).status).toBe(101);
  });

  it("rejects an app that is not ours, and the group stays unverified", async () => {
    const pki = await makePki();
    useServices({ appleRoots: [pki.root] });
    const g = newGroup();
    const c = await challengeOf(g);
    const { keyId, attestation } = await makeAttestation({ pki, appId: "8XT2U634Z4.com.example.other", challenge: fromBase64(c) });
    expect((await api(g, "attest", { challenge: c, platform: "apple", keyId: b64(keyId), attestation: b64(attestation) })).status).toBe(422);
    expect((await wantsRoom(g)).status).toBe(403);
  });

  it("takes each challenge once and refuses one it did not issue", async () => {
    const g = newGroup();
    const body = await desktop(g);
    expect((await api(g, "attest", body)).status).toBe(204);
    expect((await api(g, "attest", body)).status).toBe(400);
    expect((await api(newGroup(), "attest", body)).status).toBe(400);
    expect((await api(g, "attest", { ...body, challenge: b64(challenge()) })).status).toBe(400);
  });

  it("refuses malformed attestation bodies", async () => {
    const g = newGroup();
    const c = await challengeOf(g);
    for (const body of [{ challenge: c }, { challenge: c, platform: "tpm" }, { challenge: c, platform: "apple", keyId: "x" }, { platform: "google", token: "t" }]) {
      expect((await api(g, "attest", body)).status).toBe(400);
    }
    expect((await exports.default.fetch(`https://alazy.app/relay/v1/group/${g}/challenge`)).status).toBe(405);
  });

  it("verifies a computer that proves it holds its key, and ties the proof to the group", async () => {
    const g = newGroup();
    expect((await api(g, "attest", await desktop(g))).status).toBe(204);
    expect((await wantsRoom(g)).status).toBe(101);

    // A proof made for one group does not verify another.
    const other = newGroup();
    const mine = await desktop(g);
    expect((await api(other, "attest", { ...mine, challenge: await challengeOf(other) })).status).toBe(422);
    expect((await wantsRoom(other)).status).toBe(403);
  });

  it("refuses a computer's wrong signature and a key that is not on the curve", async () => {
    const g = newGroup();
    const body = await desktop(g);
    expect((await api(g, "attest", { ...body, signature: b64(new Uint8Array(64)) })).status).toBe(422);
    const c = await challengeOf(g);
    expect((await api(g, "attest", { ...body, challenge: c, publicKey: b64(new Uint8Array(65).fill(4)) })).status).toBe(422);
  });

  it("limits the new groups one install key may verify in a week", async () => {
    const key = await newKey();
    for (let i = 0; i < INSTALL_GROUPS_PER_WEEK; i++) {
      const g = newGroup();
      expect((await api(g, "attest", await desktop(g, key))).status).toBe(204);
    }
    const g = newGroup();
    expect((await api(g, "attest", await desktop(g, key))).status).toBe(429);
    expect((await wantsRoom(g)).status).toBe(403);
    const fresh = newGroup();
    expect((await api(fresh, "attest", await desktop(fresh))).status).toBe(204);
  });

  it("limits how many fresh installs one network starts in an hour", async () => {
    const ip = { "CF-Connecting-IP": "198.51.100.77" };
    let last = 0;
    for (let i = 0; i < 12; i++) {
      const g = newGroup();
      last = (await api(g, "attest", await desktop(g), ip)).status;
      if (last === 503) break;
    }
    expect(last).toBe(503);
  });

  const payload = (c: string, over: Partial<IntegrityPayload> = {}): IntegrityPayload => ({
    requestDetails: { requestPackageName: "com.lazysend.lazysend", nonce: c, timestampMillis: String(Date.now()) },
    appIntegrity: { appRecognitionVerdict: "PLAY_RECOGNIZED", packageName: "com.lazysend.lazysend" },
    deviceIntegrity: { deviceRecognitionVerdict: ["MEETS_DEVICE_INTEGRITY"] },
    ...over,
  });

  it("says Play Integrity is not configured until it is", async () => {
    const g = newGroup();
    const c = await challengeOf(g);
    expect((await api(g, "attest", { challenge: c, platform: "google", token: "t" })).status).toBe(501);
    expect((await wantsRoom(g)).status).toBe(403);
  });

  it("verifies a Play Integrity verdict for the challenge, from an app of ours on a sound device", async () => {
    const seen: string[] = [];
    let make: (c: string) => IntegrityPayload = payload;
    let current = "";
    useServices({ integrity: { decode: async (t) => (seen.push(t), make(current)) } });
    const attempt = async (m: (c: string) => IntegrityPayload) => {
      const g = newGroup();
      current = await challengeOf(g);
      make = m;
      const res = await api(g, "attest", { challenge: current, platform: "google", token: "tok" });
      return [res.status, (await wantsRoom(g)).status];
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
    const out = await verifyAttestation({ ...env, APPLE_APP_IDS: "" }, newGroup(), b64(challenge()), { platform: "apple", keyId: "a", attestation: "a" });
    expect(out).toBe("not-configured");
  });
});
