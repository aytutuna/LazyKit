#!/bin/bash
# Builds lazy-link-ffi, generates its C# bindings and runs the relay client tests against a development relay
# that is already listening with TLS (relay/scripts/dev-tls.sh). Needs cargo, uniffi-bindgen-cs (version in
# rust/Cargo.toml) and dotnet.
#   RELAY_URL=https://127.0.0.1:8788/relay/v1 link-cs.sh
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
out="${LINK_OUT:-$root/.scratch/link-cs}"
rm -rf "$out"
mkdir -p "$out"

case "$(uname -s)" in
  MINGW*|MSYS*) lib=lazy_link_ffi.dll ;;
  Darwin) lib=liblazy_link_ffi.dylib ;;
  *) lib=liblazy_link_ffi.so ;;
esac

cd "$root/rust"
cargo build --locked -p lazy-link-ffi --features dev-roots
cp "target/debug/$lib" "$out/"
uniffi-bindgen-cs --library "$out/$lib" --out-dir "$out"

cd "$root/native/link-tests/windows"
dotnet test LazyLink.RelayTests -p:RustLinkDir="$(cygpath -m "$out" 2>/dev/null || echo "$out")"
