#!/bin/bash
# Builds lazy-link-ffi, generates its Swift bindings and runs apple/main.swift (the relay client, TLS in Rust) against a
# development relay that is already listening with TLS (relay/scripts/dev-tls.sh). Needs a Mac with cargo and swiftc.
#   RELAY_URL=https://127.0.0.1:8788/relay/v1 link-swift.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
out="${LINK_OUT:-$root/.scratch/link-swift}"
rm -rf "$out"
mkdir -p "$out"

cd "$root/rust"
cargo build --locked -p lazy-link-ffi
cargo run --locked -q -p uniffi-bindgen -- generate --library target/debug/liblazy_link_ffi.dylib --language swift --out-dir "$out/gen"

swiftc -swift-version 5 -O \
  "$out/gen/lazy_link_ffi.swift" "$root/native/link-tests/apple/main.swift" \
  -Xcc -fmodule-map-file="$out/gen/lazy_link_ffiFFI.modulemap" -I "$out/gen" \
  -L target/debug -llazy_link_ffi -Xlinker -rpath -Xlinker "$PWD/target/debug" \
  -o "$out/check"
"$out/check"
