#!/usr/bin/env bash
# Builds the Rust part of the Android app:
#   1. downloads PDFium (V8 + XFA) for Android arm64 into vendor/pdfium-android,
#   2. generates the Kotlin bindings (uniffi) into android/app/src/generated/kotlin,
#   3. cross-compiles crates/ffi for arm64-v8a with cargo-ndk,
#   4. copies libform_pdf_reader_ffi.so and libpdfium.so into jniLibs.
#
# Needs: rustup target aarch64-linux-android, cargo-ndk, and the Android NDK
# (ANDROID_NDK_HOME, or the NDK found by cargo-ndk). Then build the app with
# Gradle from android/.
set -euo pipefail
cd "$(dirname "$0")/../.."

ABI=arm64-v8a
TARGET=aarch64-linux-android
API=26
VERSION=8086
URL="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${VERSION}/pdfium-v8-android-arm64.tgz"
SHA256=8f84890e09d667b90057e2be433f86d51d3c1748de82074e5611468cb02f5589
PDFIUM=vendor/pdfium-android
APP=android/app/src/main
GEN=android/app/src/generated/kotlin

# 1. PDFium for Android.
if [[ ! -f "$PDFIUM/lib/libpdfium.so" ]]; then
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  if [[ -n "${PDFIUM_ANDROID_TGZ:-}" ]]; then
    cp "$PDFIUM_ANDROID_TGZ" "$tmp/pdfium.tgz"
  else
    echo "Downloading $URL"
    curl -fL --retry 3 -o "$tmp/pdfium.tgz" "$URL"
  fi
  echo "$SHA256  $tmp/pdfium.tgz" | sha256sum -c --quiet -
  rm -rf "$PDFIUM" && mkdir -p "$PDFIUM"
  tar -xzf "$tmp/pdfium.tgz" -C "$PDFIUM"
fi

# 2. Kotlin bindings, generated from a host build (it needs the desktop
#    PDFium only to link; the bindings do not depend on the platform). The
#    debug build keeps the uniffi metadata that release strips.
scripts/descargar-pdfium.sh
cargo build -p form-pdf-reader-ffi
rm -rf "$GEN" && mkdir -p "$GEN"
cargo run -p form-pdf-reader-ffi --features bindgen --bin uniffi-bindgen -- \
  generate --library target/debug/libform_pdf_reader_ffi.so --language kotlin --out-dir "$GEN" --no-format

# 3. The library for Android.
PDFIUM_LIB_DIR="$PWD/$PDFIUM/lib" cargo ndk -t "$ABI" --platform "$API" build --release -p form-pdf-reader-ffi

# 4. Native libraries of the app.
mkdir -p "$APP/jniLibs/$ABI"
cp "target/$TARGET/release/libform_pdf_reader_ffi.so" "$PDFIUM/lib/libpdfium.so" "$APP/jniLibs/$ABI/"
echo "Rust part ready: $APP/jniLibs/$ABI and $GEN"
