#!/usr/bin/env bash
# Crea una AppImage. Necesita appimagetool en el PATH (no se descarga solo):
#   https://github.com/AppImage/appimagetool/releases
set -euo pipefail
cd "$(dirname "$0")/.."
command -v appimagetool >/dev/null || { echo "Falta appimagetool en el PATH. Descárgalo de https://github.com/AppImage/appimagetool/releases"; exit 1; }
cargo build --release
APPDIR=dist/AppDir
rm -rf "$APPDIR" && mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/lib"
cp target/release/form-pdf-reader "$APPDIR/usr/bin/"
cp vendor/pdfium/lib/libpdfium.so "$APPDIR/usr/bin/"   # junto al binario: lo encuentra el rpath $ORIGIN
cp assets/form-pdf-reader.desktop "$APPDIR/"
cp assets/form-pdf-reader.svg "$APPDIR/"
cat > "$APPDIR/AppRun" <<'INNER'
#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/bin/form-pdf-reader" "$@"
INNER
chmod +x "$APPDIR/AppRun"
ARCH=x86_64 appimagetool "$APPDIR" dist/PDF_Reader_Editor-x86_64.AppImage
