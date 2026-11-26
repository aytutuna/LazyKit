#!/bin/bash
# Fetches what the corner model tests need into rust/lazy-scan-model/models (git ignores it):
#   the DocAligner FastViT-T8 model (Lazy Scan's deps-docaligner-1.1.1 copy first, then the upstream release), checked
#   against its SHA-256, the same model with tract's Einsum fix (tools/patch_einsum.py, checked against its own SHA-256),
#   and the ONNX Runtime 1.24.2 library for this system, out of the Microsoft.ML.OnnxRuntime package Lazy Scan ships.
# With a second argument "models" the ONNX Runtime library is skipped (the wasm build has no use for it); with "android" it
#   is the Android build's, for x86_64 and arm64-v8a, taken out of the package's onnxruntime.aar into <out>/android/<abi>/.
# Runs in Git Bash on Windows, in macOS bash and on Linux.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
out="${1:-$here/../models}"
what="${2:-all}"
mkdir -p "$out"

copy="https://github.com/aytutuna/LazyScan/releases/download/deps-docaligner-1.1.1"
upstream="https://github.com/DocsaidLab/DocAligner/releases/download/1.1.1"
model=fastvit_t8_h_e_bifpn_256_fp32.onnx
model_sha=950070fd4a46e25d46db7c35d0ed77612d9063f70d4543c7c504f0da6a943a89
patched=fastvit_t8_h_e_bifpn_256_fp32-tract.onnx
patched_sha=613368905282e2ef7e0ad1f20517750c1abd0ad0661771b2e7041834c9c4d746
nupkg_url="https://www.nuget.org/api/v2/package/Microsoft.ML.OnnxRuntime/1.24.2"
nupkg_sha=782564d3d68e87269ea1812cc94710ad06fe014faf093f01a6f89d0bd58719d3
aar_sha=bc461499a735653dff285a6a3477d28b9cfd119a09c7753eaf003426b577f223
android_sha_x86_64=a824fa2ae49c0686b7730704ae705b03e654a1f71459939e8e0924c6a4226b11
android_sha_arm64_v8a=42f83b3c2370300821942d1f61099689d37bb5e05126fb75849d4cc9dbe356a2

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d ' ' -f 1; else shasum -a 256 "$1" | cut -d ' ' -f 1; fi
}

has() { [ -f "$1" ] && [ "$(sha256 "$1")" = "$2" ]; }

download() {
  local url="$1" file="$2" expected="$3"
  curl -fsSL --retry 3 --retry-all-errors --retry-delay 10 -o "$file.part" "$url" || { rm -f "$file.part"; return 1; }
  if [ "$(sha256 "$file.part")" != "$expected" ]; then
    rm -f "$file.part"
    echo "$url: checksum mismatch" >&2
    return 1
  fi
  mv "$file.part" "$file"
}

if ! has "$out/$model" "$model_sha"; then
  download "$copy/$model" "$out/$model" "$model_sha" ||
    download "$upstream/$model" "$out/$model" "$model_sha" ||
    { echo "$model: not fetched" >&2; exit 1; }
fi

if ! has "$out/$patched" "$patched_sha"; then
  python=python3
  "$python" --version >/dev/null 2>&1 || python=python
  "$python" -I "$here/../../../tools/patch_einsum.py" "$out/$model" "$out/$patched"
  if [ "$(sha256 "$out/$patched")" != "$patched_sha" ]; then
    echo "$patched: the patched model is not the pinned one" >&2
    exit 1
  fi
fi

if [ "$what" = models ]; then
  echo "models are in $out"
  exit 0
fi

if [ "$what" = android ]; then
  for abi in x86_64 arm64-v8a; do
    expected="android_sha_${abi//-/_}"
    expected="${!expected}"
    has "$out/android/$abi/libonnxruntime.so" "$expected" && continue
    if [ ! -f "$out/onnxruntime.aar" ]; then
      download "$nupkg_url" "$out/ort.nupkg" "$nupkg_sha" || { echo "ONNX Runtime package not fetched" >&2; exit 1; }
      unzip -j -o -q "$out/ort.nupkg" runtimes/android/native/onnxruntime.aar -d "$out"
      rm -f "$out/ort.nupkg"
      has "$out/onnxruntime.aar" "$aar_sha" || { echo "onnxruntime.aar is not the pinned archive" >&2; exit 1; }
    fi
    mkdir -p "$out/android/$abi"
    unzip -j -o -q "$out/onnxruntime.aar" "jni/$abi/libonnxruntime.so" -d "$out/android/$abi"
    has "$out/android/$abi/libonnxruntime.so" "$expected" || { echo "$abi: libonnxruntime.so is not the pinned library" >&2; exit 1; }
  done
  echo "ONNX Runtime for Android is in $out/android"
  exit 0
fi

case "$(uname -s)" in
  Darwin) entry=runtimes/osx-arm64/native/libonnxruntime.dylib; lib=libonnxruntime.dylib; lib_sha=8e88284d3b4395b1e532831ac05eb0ffa3673a62df32349e1b0fd64542cb827d ;;
  Linux) entry=runtimes/linux-x64/native/libonnxruntime.so; lib=libonnxruntime.so; lib_sha=ffc84d48e845cf0b562ba4ea5ca32aaafc0d4069019fef4f63095b307d0270ad ;;
  *) entry=runtimes/win-x64/native/onnxruntime.dll; lib=onnxruntime.dll; lib_sha=114947d633e6844ce3c4b51ef6678f776628571d08a5763859c61642c8dcca9c ;;
esac
if ! has "$out/$lib" "$lib_sha"; then
  download "$nupkg_url" "$out/ort.nupkg" "$nupkg_sha" || { echo "ONNX Runtime package not fetched" >&2; exit 1; }
  unzip -j -o -q "$out/ort.nupkg" "$entry" -d "$out"
  rm -f "$out/ort.nupkg"
  has "$out/$lib" "$lib_sha" || { echo "$lib is not the pinned library" >&2; exit 1; }
fi

echo "models and ONNX Runtime are in $out"
