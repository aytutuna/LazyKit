// Cargo's test runner for wasm32-wasip1: runs the test binary in Node's WASI with the LazyKit checkout mounted at /kit.
import { WASI } from "node:wasi";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const [wasmPath, ...args] = process.argv.slice(2);
const root = resolve(import.meta.dirname, "../..");
const env = { LAZYKIT_ROOT: "/kit", LAZY_SCAN_MODELS: "/kit/rust/lazy-scan-model/models" };
for (const name of ["LAZY_SCAN_REQUIRE_MODEL", "RUST_BACKTRACE"]) if (process.env[name]) env[name] = process.env[name];
const wasi = new WASI({ version: "preview1", args: [wasmPath, ...args], env, preopens: { "/kit": root } });
const instance = await WebAssembly.instantiate(await WebAssembly.compile(readFileSync(wasmPath)), wasi.getImportObject());
process.exitCode = wasi.start(instance);
