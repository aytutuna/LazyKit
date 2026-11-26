// Run through `node --test` against the package wasm-bindgen writes for the nodejs target; the folder is passed in WASM_PKG.
const test = require("node:test");
const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs");
const path = require("node:path");

const link = require(path.resolve(process.env.WASM_PKG, "lazy_link_wasm.js"));
const dataDir = path.join(__dirname, "..", "..", "..", "testdata", "link");
const load = (file) => JSON.parse(fs.readFileSync(path.join(dataDir, file), "utf8"));
const fromBase64Url = (text) => Buffer.from(text, "base64url");

test("pairing codes are read as the data says", () => {
  for (const c of load("pairing-uris.json").cases) {
    if (c.result !== "ok") {
      assert.throws(() => link.parsePairingCode(c.uri), { message: c.result }, c.name);
      continue;
    }
    const code = link.parsePairingCode(c.uri);
    const want = c.pairing;
    assert.equal(code.desktopId, want.desktopId, c.name);
    assert.equal(code.name, want.name, c.name);
    assert.equal(code.fingerprint, want.fingerprint, c.name);
    assert.deepEqual(code.addresses, want.addresses, c.name);
    assert.equal(code.port, want.port, c.name);
    assert.equal(code.token, want.token, c.name);
    assert.equal(link.checkCode(fromBase64Url(code.fingerprint)), want.checkCode, c.name);
    const written = link.formatPairingCode(code);
    assert.equal(link.formatPairingCode(link.parsePairingCode(written)), written, c.name);
  }
});

test("check and match codes are as the data says", () => {
  for (const c of load("check-codes.json").cases) {
    const fingerprint = Uint8Array.from(c.fingerprint);
    assert.equal(link.checkCode(fingerprint), c.checkCode);
    assert.equal(link.matchCode(fingerprint, c.shortCode), c.matchCode);
  }
});

test("short codes are read and redeemed as the data says", () => {
  const data = load("short-codes.json");
  for (const n of data.normalize) {
    if (n.typed === null) continue;
    assert.equal(link.normalizeShortCode(n.typed) ?? null, n.canonical, JSON.stringify(n.typed));
  }
  const token = Uint8Array.from(data.tokenBytes);
  for (const r of data.redeem) {
    const session = new link.PairingSession(0, token, Uint8Array.from([7, 3, 21, 0, 9, 31, 12, 5]));
    const code = session.shortCode();
    const typed = `${code.slice(0, 4)}-${code.slice(4)}`.toLowerCase();
    const presentable = { $token: session.token(), $shortCode: code, $shortCodeTyped: typed };
    r.attempts.forEach((a, i) => {
      const presented = a.token === null ? undefined : (presentable[a.token] ?? a.token);
      const got = session.redeem(presented, a.at * 1000, a.peer);
      assert.equal(got.toLowerCase(), a.expect.toLowerCase(), `${r.name}, attempt ${i + 1}`);
    });
    session.free();
  }
});

// The key stays in these objects: Rust only sees the hash going out and the signature coming back.
const key = crypto.randomBytes(32);
const sign = (hash) => crypto.createHmac("sha256", key).update(hash).digest();
const expected = (data) => sign(crypto.createHash("sha256").update(data).digest());

test("Rust hands the hash to the native signer", () => {
  const data = Uint8Array.from([1, 2, 3, 4, 5]);
  const signature = link.signData({ sign: (hash) => new Uint8Array(sign(hash)) }, data);
  assert.deepEqual(Buffer.from(signature), expected(data));
});

test("a signer that throws comes back as the same error", () => {
  const locked = { sign: () => { throw new Error("the key is locked"); } };
  assert.throws(() => link.signData(locked, Uint8Array.from([1])), { message: "the key is locked" });
});

test("async signing round trips", async () => {
  const data = Uint8Array.from([9, 8, 7]);
  const later = { sign: async (hash) => { await new Promise((r) => setTimeout(r, 20)); return new Uint8Array(sign(hash)); } };
  const signature = await link.signDataAsync(later, data);
  assert.deepEqual(Buffer.from(signature), expected(data));
});

test("fifty megabytes cross both ways", () => {
  const size = 50 * 1024 * 1024;
  const data = new Uint8Array(size);
  for (let i = 0; i < size; i++) data[i] = i % 251;
  assert.equal(link.hashBytes(data), crypto.createHash("sha256").update(data).digest("hex"));
  const back = link.patternBytes(size);
  assert.equal(back.length, size);
  assert.ok(Buffer.from(back).equals(data));
});

// Last, because a trap leaves the instance in an unknown state.
test("a Rust panic is an exception", () => {
  assert.throws(() => link.panicNow("boom"), (e) => e instanceof WebAssembly.RuntimeError);
});
