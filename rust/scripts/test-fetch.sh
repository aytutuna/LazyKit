#!/bin/bash
# Checks what fetch.sh decides before it downloads anything: the modes it knows and the library it picks for each system.
set -u
fetch="$(cd "$(dirname "$0")" && pwd)/../lazy-scan-model/scripts/fetch.sh"
out="$(mktemp -d)"
err="$(mktemp)"
trap 'rm -rf "$out" "$err"' EXIT
failed=0

fail() { echo "FAIL: $1" >&2; failed=1; }

if bash "$fetch" "$out" bogus 2>"$err"; then fail "an unknown mode was accepted"; fi
grep -q "unknown mode 'bogus'" "$err" || fail "the unknown mode was not named"
[ -z "$(ls "$out")" ] || fail "an unknown mode wrote files"

picks() {
  local got
  got="$(LAZY_FETCH_OS="$1" LAZY_FETCH_CPU="$2" LAZY_FETCH_DRY=1 bash "$fetch" "$out" all 2>"$err")" || { fail "$1 on $2 was refused"; return; }
  [ "${got%% *}" = "$3" ] || fail "$1 on $2 picked ${got%% *}, not $3"
}
picks Darwin arm64 runtimes/osx-arm64/native/libonnxruntime.dylib
picks Linux x86_64 runtimes/linux-x64/native/libonnxruntime.so
picks Linux aarch64 runtimes/linux-arm64/native/libonnxruntime.so
picks MINGW64_NT-10.0-26100 x86_64 runtimes/win-x64/native/onnxruntime.dll
picks MINGW64_NT-10.0-26100 aarch64 runtimes/win-arm64/native/onnxruntime.dll

refused() {
  if LAZY_FETCH_OS="$1" LAZY_FETCH_CPU="$2" LAZY_FETCH_DRY=1 bash "$fetch" "$out" all 2>"$err" >/dev/null; then
    fail "$1 on $2 was accepted"
  else
    grep -q "no ONNX Runtime" "$err" || fail "$1 on $2 was refused without saying why"
  fi
}
refused Darwin x86_64
refused Linux riscv64
refused FreeBSD amd64

[ "$failed" = 0 ] && echo "fetch.sh decisions ok"
exit "$failed"
