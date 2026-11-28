#!/bin/bash
# Builds lazy-link-ffi for Android, generates its Kotlin bindings and runs the relay client tests on the connected device
# or emulator against a development relay on the host (relay/scripts/dev-tls.sh 8788 0.0.0.0). The relay's development
# authority goes into the device's user certificate store, which the platform verifier reads (AndroidCAStore); that
# needs a device that allows adb root, such as an emulator image with Google APIs. Needs cargo-ndk, the NDK
# (ANDROID_NDK_HOME) and a device.
#   link-android.sh [relay url as the device sees it, default https://10.0.2.2:8788/relay/v1]
set -euo pipefail

root="$(cd "$(dirname "$0")/../../.." && pwd)"
out="${LINK_OUT:-$root/.scratch/link-android}"
rm -rf "$out"
mkdir -p "$out"

cd "$root/rust"
cargo ndk -t arm64-v8a -t x86_64 -P 26 -o "$out/jniLibs" build --locked -p lazy-link-ffi
cargo run --locked -q -p uniffi-bindgen -- generate --library "$out/jniLibs/arm64-v8a/liblazy_link_ffi.so" --language kotlin --out-dir "$out/kotlin" --no-format

cd "$root/native/link-tests/android"
ca="$root/relay/.dev-tls/ca.pem"
name="$(openssl x509 -in "$ca" -noout -subject_hash_old).0"
adb root
adb wait-for-device
adb push "$ca" "/data/local/tmp/$name"
adb shell "mkdir -p /data/misc/user/0/cacerts-added && cp /data/local/tmp/$name /data/misc/user/0/cacerts-added/$name && chmod 644 /data/misc/user/0/cacerts-added/$name && chown system:system /data/misc/user/0/cacerts-added/$name && restorecon /data/misc/user/0/cacerts-added/$name"
status=0
./gradlew :linktests:connectedDebugAndroidTest -PlinkOut="$out" \
  -Pandroid.testInstrumentationRunnerArguments.relayUrl="${1:-https://10.0.2.2:8788/relay/v1}" --console=plain || status=$?
# Gradle only says that tests failed; the reasons are in the result files.
if [ "$status" != 0 ]; then
  find linktests/build/outputs/androidTest-results -name '*.xml' -exec grep -h -A12 '<failure' {} + || true
fi
exit "$status"
