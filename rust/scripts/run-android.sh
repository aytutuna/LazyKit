#!/bin/bash
# Cargo's test runner for the Android targets: copies the test binary, the scanner models, the ONNX Runtime library for the
# device's ABI and testdata/scan to /data/local/tmp/lazykit on the connected emulator or phone, and runs the binary there.
# Needs rust/lazy-scan-model/scripts/fetch.sh <dir> android to have run, with the same models directory.
set -euo pipefail
export MSYS_NO_PATHCONV=1

exe="$1"
shift
here="$(cd "$(dirname "$0")" && pwd)"
root="$here/../.."
models="${LAZY_SCAN_MODELS:-$here/../lazy-scan-model/models}"
dest=/data/local/tmp/lazykit
# adb is a Windows program under Git Bash and wants Windows paths for the files on this side
host() { if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else echo "$1"; fi; }
abi="$(adb shell getprop ro.product.cpu.abi | tr -d '\r')"

# the big files are pushed again whenever any of them changed (the stamp covers both models and the library); testdata is
# small and goes over every run so edits are picked up
if command -v sha256sum >/dev/null 2>&1; then sum=sha256sum; else sum="shasum -a 256"; fi
heavy=("$models/fastvit_t8_h_e_bifpn_256_fp32.onnx" "$models/fastvit_t8_h_e_bifpn_256_fp32-tract.onnx" "$models/android/$abi/libonnxruntime.so")
stamp="$(cat "${heavy[@]}" | $sum | cut -c 1-12)"
adb shell "mkdir -p $dest/testdata && rm -rf $dest/testdata/scan"
adb push "$(host "$root/testdata/scan")" "$dest/testdata/scan" >/dev/null 2>&1
if ! adb shell "[ -f $dest/ready-$abi-$stamp ]" >/dev/null 2>&1; then
  adb shell "rm -f $dest/ready-* $dest/*.onnx $dest/libonnxruntime.so"
  for file in "${heavy[@]}"; do adb push "$(host "$file")" "$dest/" >/dev/null 2>&1; done
  adb shell "touch $dest/ready-$abi-$stamp"
fi

adb push "$(host "$exe")" "$dest/test-binary" >/dev/null 2>&1
adb shell "chmod 755 $dest/test-binary"
args=""
for a in "$@"; do args="$args $(printf '%q' "$a")"; done
adb shell "cd $dest && LAZYKIT_ROOT=$dest LAZY_SCAN_MODELS=$dest ORT_DYLIB_PATH=$dest/libonnxruntime.so LAZY_SCAN_REQUIRE_MODEL=${LAZY_SCAN_REQUIRE_MODEL:-0} RUST_BACKTRACE=1 ./test-binary$args"
