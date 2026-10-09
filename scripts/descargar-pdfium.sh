#!/usr/bin/env bash
# Downloads PDFium built with V8 and XFA (bblanchon/pdfium-binaries,
# chromium/8086) into vendor/pdfium, checking its SHA-256.
#
#   scripts/descargar-pdfium.sh            download if missing
#   scripts/descargar-pdfium.sh --force    download again
#   PDFIUM_TGZ=/path/file.tgz scripts/descargar-pdfium.sh   use a local copy
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=8086
URL="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${VERSION}/pdfium-v8-linux-x64.tgz"
SHA256=8e3efa4fe6784ed80fd58ef3c839b072ddcd79b16880706892a008f8a2da730b
DEST=vendor/pdfium

if [[ -f "$DEST/lib/libpdfium.so" && "${1:-}" != "--force" ]]; then
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
echo "PDFium chromium/$VERSION installed in $DEST"
