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

# the iOS cache counts only while its stamp matches the files on disk
ios_state() { LAZY_FETCH_DRY=1 bash "$fetch" "$out/cache" ios 2>"$err"; }
eval "$(sed -n '/^sha256() {/,/^}/p;/^tree_sha() {/,/^}/p' "$fetch")"
stamp() { echo "$(grep '^xcframework_sha=' "$fetch" | cut -d = -f 2) $(tree_sha "$out/cache/ios")" > "$out/cache/ios/stamp"; }
mkdir -p "$out/cache/ios/onnxruntime.xcframework"
echo framework > "$out/cache/ios/onnxruntime.xcframework/lib"
[ "$(ios_state)" = stale ] || fail "an iOS cache with no stamp was trusted"
stamp
[ "$(ios_state)" = cached ] || fail "a matching iOS cache was refused"
echo tampered > "$out/cache/ios/onnxruntime.xcframework/lib"
[ "$(ios_state)" = stale ] || fail "an edited iOS cache was trusted"
echo framework > "$out/cache/ios/onnxruntime.xcframework/lib"
echo extra > "$out/cache/ios/onnxruntime.xcframework/extra"
[ "$(ios_state)" = stale ] || fail "an iOS cache with an added file was trusted"
rm "$out/cache/ios/onnxruntime.xcframework/extra"
echo "$(grep '^xcframework_sha=' "$fetch" | cut -d = -f 2)" > "$out/cache/ios/stamp"
[ "$(ios_state)" = stale ] || fail "an iOS cache with the old plain stamp was trusted"

# a nested file called stamp is part of the tree; a symlink counts by where it points
stamp
echo one > "$out/cache/ios/onnxruntime.xcframework/stamp"
stamp
[ "$(ios_state)" = cached ] || fail "a nested file called stamp broke a matching iOS cache"
echo two > "$out/cache/ios/onnxruntime.xcframework/stamp"
[ "$(ios_state)" = stale ] || fail "an edited nested file called stamp was ignored"
rm "$out/cache/ios/onnxruntime.xcframework/stamp"
if ln -s lib "$out/cache/ios/onnxruntime.xcframework/link" 2>/dev/null && [ -L "$out/cache/ios/onnxruntime.xcframework/link" ]; then
  stamp
  [ "$(ios_state)" = cached ] || fail "an iOS cache with a symlink was refused"
  rm "$out/cache/ios/onnxruntime.xcframework/link"
  ln -s elsewhere "$out/cache/ios/onnxruntime.xcframework/link"
  [ "$(ios_state)" = stale ] || fail "a retargeted symlink was trusted"
  rm "$out/cache/ios/onnxruntime.xcframework/link"
else
  echo "no symlinks here; symlink check skipped"
fi

[ "$failed" = 0 ] && echo "fetch.sh decisions ok"
exit "$failed"
