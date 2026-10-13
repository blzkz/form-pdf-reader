#!/usr/bin/env bash
# Downloads PDFium built with V8 and XFA (bblanchon/pdfium-binaries,
# chromium/8086) into vendor/pdfium, checking its SHA-256.
#
#   scripts/descargar-pdfium.sh            download if missing
#   scripts/descargar-pdfium.sh --force    download again
#   PDFIUM_TGZ=/path/file.tgz scripts/descargar-pdfium.sh   use a local copy
#   PDFIUM_PLATFORM=win-x64 scripts/descargar-pdfium.sh     Windows build
#                                                           (default: linux-x64)
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=8086
PLATFORM=${PDFIUM_PLATFORM:-linux-x64}
case "$PLATFORM" in
  linux-x64)
    SHA256=8e3efa4fe6784ed80fd58ef3c839b072ddcd79b16880706892a008f8a2da730b
    CHECK=lib/libpdfium.so ;;
  win-x64)
    SHA256=cb8f1e0e0c0dd74cbb17e8ec11d8969c4b95638fd3e66fd1c7e4a684d28a0d46
    CHECK=lib/pdfium.dll.lib ;;
  *)
    echo "Unknown PDFIUM_PLATFORM: $PLATFORM (linux-x64 or win-x64)" >&2
    exit 1 ;;
esac
URL="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${VERSION}/pdfium-v8-${PLATFORM}.tgz"
DEST=vendor/pdfium

if [[ -f "$DEST/$CHECK" && "${1:-}" != "--force" ]]; then
  echo "PDFium is already in $DEST (use --force to download it again)."
  exit 0
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
if [[ -n "${PDFIUM_TGZ:-}" ]]; then
  cp "$PDFIUM_TGZ" "$tmp/pdfium.tgz"
else
  echo "Downloading $URL"
  curl -fL --retry 3 -o "$tmp/pdfium.tgz" "$URL"
fi
echo "$SHA256  $tmp/pdfium.tgz" | sha256sum -c --quiet -
rm -rf "$DEST"
mkdir -p "$DEST"
tar -xzf "$tmp/pdfium.tgz" -C "$DEST"
echo "PDFium chromium/$VERSION ($PLATFORM) installed in $DEST"
