#!/bin/bash
# Runs run-android.sh against a pretend adb whose device is a folder, and checks which files it pushes again: the models and
# the library only when one of them changed, testdata every time.
set -u
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin" "$work/device" "$work/models/android/arm64-v8a"
export FAKE_DEVICE="$work/device" FAKE_LOG="$work/log"
cat > "$work/bin/adb" <<'ADB'
#!/bin/bash
real() { echo "${1/\/data\/local\/tmp/$FAKE_DEVICE}"; }
case "$1" in
  shell)
    shift
    case "$*" in
      "getprop ro.product.cpu.abi") echo arm64-v8a ;;
      *) bash -c "$(sed "s#/data/local/tmp#$FAKE_DEVICE#g" <<<"$*")" ;;
    esac ;;
  push)
    dest="$(real "$3")"
    echo "push $(basename "$2")" >> "$FAKE_LOG"
    mkdir -p "$(dirname "$dest")"
    if [ -d "$2" ]; then rm -rf "$dest"; cp -r "$2" "$dest"; else cp "$2" "$dest"; fi ;;
esac
ADB
chmod +x "$work/bin/adb"
echo model > "$work/models/fastvit_t8_h_e_bifpn_256_fp32.onnx"
echo patched > "$work/models/fastvit_t8_h_e_bifpn_256_fp32-tract.onnx"
echo library > "$work/models/android/arm64-v8a/libonnxruntime.so"
printf '#!/bin/sh\n' > "$work/test-binary"
export PATH="$work/bin:$PATH" LAZY_SCAN_MODELS="$work/models"
failed=0
fail() { echo "FAIL: $1" >&2; failed=1; }

run() {
  : > "$FAKE_LOG"
  bash "$here/run-android.sh" "$work/test-binary" >/dev/null 2>&1 || fail "$1: the run failed"
}
pushed() { grep -c "push $1" "$FAKE_LOG"; }

run first
for f in fastvit_t8_h_e_bifpn_256_fp32.onnx fastvit_t8_h_e_bifpn_256_fp32-tract.onnx libonnxruntime.so scan; do
  [ "$(pushed "$f")" = 1 ] || fail "first run: $f was not pushed once"
done
run same
for f in fastvit_t8_h_e_bifpn_256_fp32.onnx fastvit_t8_h_e_bifpn_256_fp32-tract.onnx libonnxruntime.so; do
  [ "$(pushed "$f")" = 0 ] || fail "second run: $f was pushed again"
done
[ "$(pushed scan)" = 1 ] || fail "second run: testdata was not pushed"
echo "changed" > "$work/models/fastvit_t8_h_e_bifpn_256_fp32-tract.onnx"
run edited
for f in fastvit_t8_h_e_bifpn_256_fp32.onnx fastvit_t8_h_e_bifpn_256_fp32-tract.onnx libonnxruntime.so; do
  [ "$(pushed "$f")" = 1 ] || fail "after a model changed: $f was not pushed"
done
[ "$(cat "$FAKE_DEVICE/lazykit/fastvit_t8_h_e_bifpn_256_fp32-tract.onnx")" = changed ] || fail "the device kept the old model"

[ "$failed" = 0 ] && echo "run-android.sh stamp ok"
exit "$failed"
