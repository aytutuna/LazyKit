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

test("peers with a zone count apart and a bad peer throws", () => {
  const session = new link.PairingSession(0, new Uint8Array(16).fill(9), Uint8Array.from([7, 3, 21, 0, 9, 31, 12, 5]));
  for (let i = 0; i < 5; i++) assert.equal(session.redeem("wrong", 1, "fe80::1%12"), "wrongToken");
  assert.equal(session.redeem("wrong", 1, "fe80::1%12"), "closed");
  assert.equal(session.redeem("wrong", 1, "fe80::1%wlan0"), "wrongToken");
  assert.equal(session.redeem("wrong", 1, undefined), "wrongToken");
  assert.throws(() => session.redeem("wrong", 1, "nope"), { message: "the peer is not an address" });
  assert.equal(session.failedAttempts(), 7);
  session.free();
});

test("times must be finite numbers", () => {
  const token = new Uint8Array(16).fill(9);
  const entropy = new Uint8Array(8);
  for (const bad of [NaN, Infinity, -Infinity, 1e300]) {
    assert.throws(() => new link.PairingSession(bad, token, entropy));
  }
  const session = new link.PairingSession(0, token, entropy);
  assert.throws(() => session.redeem("x", NaN, undefined));
  assert.throws(() => session.redeem("x", undefined, undefined));
  assert.throws(() => session.stateAt(NaN));
  assert.throws(() => session.remainingMs(Infinity));
  session.free();
});

test("the countdown can be read", () => {
  const session = new link.PairingSession(1000, new Uint8Array(16), new Uint8Array(8));
  assert.equal(session.expiresMs(), 301000);
  assert.equal(session.stateAt(1000), "open");
  assert.equal(session.remainingMs(101000), 200000);
  assert.equal(session.stateAt(301000), "expired");
  assert.equal(session.remainingMs(301000), 0);
  session.end();
  assert.equal(session.stateAt(1000), "closed");
  session.free();
});

test("a pairing code can be built", () => {
  const code = new link.PairingCode("id", "Desk", "fp", ["10.0.0.1"], 52734, "tok");
  assert.equal(code.desktopId, "id");
  assert.deepEqual(code.addresses, ["10.0.0.1"]);
  assert.equal(code.port, 52734);
});

test("an expired session stays expired when the clock goes back", () => {
  const session = new link.PairingSession(0, new Uint8Array(16), new Uint8Array(8));
  assert.equal(session.redeem(session.token(), 300000, undefined), "expired");
  assert.equal(session.redeem(session.token(), 1000, undefined), "expired");
  session.free();
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

test("signers must return bytes", async () => {
  const data = Uint8Array.from([1]);
  const message = { message: "the signer must return a Uint8Array" };
  assert.throws(() => link.signData({ sign: () => undefined }, data), message);
  assert.throws(() => link.signData({ sign: () => [1, 2] }, data), message);
  await assert.rejects(link.signDataAsync({ sign: async () => undefined }, data), message);
  await assert.rejects(link.signDataAsync({ sign: async () => "text" }, data), message);
  await assert.rejects(link.signDataAsync({ sign: () => undefined }, data), message);
  await assert.rejects(link.signDataAsync({ sign: () => "text" }, data), message);
});

test("async signing accepts a plain value and a thenable", async () => {
  const data = Uint8Array.from([4, 5]);
  const plain = await link.signDataAsync({ sign: (hash) => new Uint8Array(sign(hash)) }, data);
  assert.deepEqual(Buffer.from(plain), expected(data));
  const thenable = { sign: (hash) => ({ then: (done) => done(new Uint8Array(sign(hash))) }) };
  assert.deepEqual(Buffer.from(await link.signDataAsync(thenable, data)), expected(data));
  await assert.rejects(link.signDataAsync({ sign: () => { throw new Error("locked"); } }, data), { message: "locked" });
});

test("an async signer that reads the hash late still sees its own copy", async () => {
  const data = Uint8Array.from([3, 1, 4, 1, 5]);
  const late = {
    sign: async (hash) => {
      await new Promise((r) => setTimeout(r, 10));
      return new Uint8Array(sign(hash));
    },
  };
  const first = link.signDataAsync(late, data);
  link.patternBytes(64 << 20);
  link.hashBytes(Uint8Array.from([7, 7, 7]));
  const second = link.signDataAsync(late, Uint8Array.from([2, 7, 1]));
  assert.deepEqual(Buffer.from(await first), expected(data));
  assert.deepEqual(Buffer.from(await second), expected(Uint8Array.from([2, 7, 1])));
});

test("a pairing code cannot be changed from JavaScript", () => {
  const code = new link.PairingCode("id", "n", "fp", ["10.0.0.1"], 443, "t");
  const change = (field, value) => { try { code[field] = value; } catch {} };
  change("name", 5);
  change("port", 70000);
  change("port", -1);
  change("addresses", "abc");
  change("token", {});
  assert.equal(code.name, "n");
  assert.equal(code.port, 443);
  assert.deepEqual(code.addresses, ["10.0.0.1"]);
  assert.equal(code.token, "t");
});

test("arguments of the wrong kind throw instead of trapping", () => {
  const entropy = new Uint8Array(8);
  assert.throws(() => new link.PairingSession(0, "0123456789abcdef", entropy), { message: "the pairing token must be a Uint8Array" });
  assert.throws(() => new link.PairingSession(0, new Uint8Array(16), { length: 8 }), { message: "the code entropy must be a Uint8Array" });
  const session = new link.PairingSession(0, new Uint8Array(16), entropy);
  assert.throws(() => session.redeem(123, 1, undefined), { message: "the token must be a string" });
  assert.throws(() => session.redeem("x", 1, 42), { message: "the peer must be a string" });
  assert.throws(() => session.redeem("x", 1, {}), { message: "the peer must be a string" });
  assert.equal(session.failedAttempts(), 0);
  session.free();
  assert.throws(() => link.normalizeShortCode(12345678), { message: "the short code must be a string" });
  assert.throws(() => link.parsePairingCode(5), { message: "the pairing code must be a string" });
  assert.throws(() => link.matchCode(new Uint8Array(32), 1), { message: "the short code must be a string" });
  for (const port of [0, 70000, 1.5, NaN, -1]) {
    assert.throws(() => new link.PairingCode("id", "n", "fp", ["10.0.0.1"], port, "t"), { message: /port/ }, String(port));
  }
  assert.throws(() => new link.PairingCode("id", "n", "fp", "10.0.0.1", 1, "t"), { message: /addresses/ });
  assert.throws(() => new link.PairingCode("id", "n", "fp", [1], 1, "t"), { message: "an address must be a string" });
  assert.equal(new link.PairingCode("id", "n", "fp", [], 65535, "t").port, 65535);
  assert.equal(link.normalizeShortCode(undefined) ?? null, null);
});

test("byte arguments must be a Uint8Array", async () => {
  const signer = { sign: (hash) => new Uint8Array(sign(hash)) };
  const message = /must be a Uint8Array/;
  for (const bad of ["abcd", [1, 2, 3], { length: 4 }, new ArrayBuffer(4), undefined, null, 5]) {
    const what = String(bad);
    assert.throws(() => link.checkCode(bad), message, what);
    assert.throws(() => link.matchCode(bad, "ABCDEFGH"), message, what);
    assert.throws(() => link.spkiFingerprint(bad), message, what);
    assert.throws(() => link.hashBytes(bad), message, what);
    assert.throws(() => link.signData(signer, bad), message, what);
    await assert.rejects(link.signDataAsync(signer, bad), message, what);
  }
  assert.equal(link.hashBytes(new Uint8Array(0)), crypto.createHash("sha256").digest("hex"));
});

test("pattern length must be a whole number in range", () => {
  for (const bad of [-1, 1.5, NaN, Infinity, -Infinity, 2 ** 32, 2 ** 32 + 3, undefined]) {
    assert.throws(() => link.patternBytes(bad), { message: /whole number/ }, String(bad));
  }
  assert.equal(link.patternBytes(0).length, 0);
});

test("pattern bytes have a cap", () => {
  assert.throws(() => link.patternBytes(256 * 1024 * 1024 + 1), { message: "at most 256 MiB" });
  assert.deepEqual(Array.from(link.patternBytes(3)), [0, 1, 2]);
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
