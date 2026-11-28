#!/bin/bash
# Builds lazy-link-ffi for Android, generates its Kotlin bindings and runs the relay client tests on the connected device
# or emulator against a development relay on the host (relay/scripts/dev-tls.sh 8788 0.0.0.0). The relay's development
# authority goes into the test app's assets, and the test adds it to the client's roots. Needs cargo-ndk, the NDK
# (ANDROID_NDK_HOME) and a device.
#   link-android.sh [relay url as the device sees it, default https://10.0.2.2:8788/relay/v1]
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
out="${LINK_OUT:-$root/.scratch/link-android}"
rm -rf "$out"
mkdir -p "$out"

cd "$root/rust"
cargo ndk -t arm64-v8a -t x86_64 -P 26 -o "$out/jniLibs" build --locked -p lazy-link-ffi --features dev-roots
cargo run --locked -q -p uniffi-bindgen -- generate --library "$out/jniLibs/arm64-v8a/liblazy_link_ffi.so" --language kotlin --out-dir "$out/kotlin" --no-format

cd "$root/native/link-tests/android"
mkdir -p linktests/src/androidTest/assets
cp "$root/relay/.dev-tls/ca.pem" linktests/src/androidTest/assets/dev_ca.pem
status=0
./gradlew :linktests:connectedDebugAndroidTest -PlinkOut="$out" \
  -Pandroid.testInstrumentationRunnerArguments.relayUrl="${1:-https://10.0.2.2:8788/relay/v1}" \
  -Pandroid.testInstrumentationRunnerArguments.mailSink="${MAIL_SINK:-http://10.0.2.2:8790}" --console=plain || status=$?
# Gradle only says that tests failed; the reasons are in the result files.
if [ "$status" != 0 ]; then
  find linktests/build/outputs/androidTest-results -name '*.xml' -exec grep -h -A12 '<failure' {} + || true
fi
exit "$status"
