#!/usr/bin/env bash
# Genera dist/form-pdf-reader-<versión>-x86_64.tar.gz con todo lo necesario
# (binario, libpdfium.so, licencias, .desktop, icono e instalador).
set -euo pipefail
cd "$(dirname "$0")/.."
VER=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
NAME="form-pdf-reader-$VER-x86_64"
# SKIP_BUILD=1 reutiliza target/release y los avisos de terceros ya
# generados (lo usa la integración continua).
NOTICES=target/THIRD-PARTY-NOTICES.txt
if [[ -z "${SKIP_BUILD:-}" ]]; then
  cargo build --release
  python3 scripts/third_party_notices.py --package form-pdf-reader --target x86_64-unknown-linux-gnu \
    --pdfium vendor/pdfium --out "$NOTICES"
fi
rm -rf "dist/$NAME" && mkdir -p "dist/$NAME/lib" "dist/$NAME/licenses"
cp target/release/form-pdf-reader "dist/$NAME/"
cp vendor/pdfium/lib/libpdfium.so "dist/$NAME/lib/"
# Licencia del programa y avisos del software de terceros que incluye.
cp LICENSE "$NOTICES" "dist/$NAME/"
cp -r vendor/pdfium/licenses/. "dist/$NAME/licenses/"
cp assets/form-pdf-reader.desktop assets/form-pdf-reader.svg README.md "dist/$NAME/"
cp -r assets/icons "dist/$NAME/"
cat > "dist/$NAME/instalar.sh" <<'INNER'
#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
PREFIX="${PREFIX:-$HOME/.local}"
D="$PREFIX/lib/form-pdf-reader"
install -Dm755 form-pdf-reader "$D/form-pdf-reader"
install -Dm644 lib/libpdfium.so "$D/lib/libpdfium.so"
mkdir -p "$D/licenses" && cp -r licenses/. "$D/licenses/"
cp LICENSE THIRD-PARTY-NOTICES.txt "$D/"
mkdir -p "$PREFIX/bin" && ln -sf "$D/form-pdf-reader" "$PREFIX/bin/form-pdf-reader"
install -Dm644 form-pdf-reader.desktop "$PREFIX/share/applications/form-pdf-reader.desktop"
install -Dm644 form-pdf-reader.svg "$PREFIX/share/icons/hicolor/scalable/apps/form-pdf-reader.svg"
for s in 16 24 32 48 64 128 256; do
  install -Dm644 "icons/$s/form-pdf-reader.png" "$PREFIX/share/icons/hicolor/${s}x${s}/apps/form-pdf-reader.png"
done
echo "Instalado en $D"
INNER
chmod +x "dist/$NAME/instalar.sh"
tar -C dist -czf "dist/$NAME.tar.gz" "$NAME"
echo "Creado dist/$NAME.tar.gz ($(du -h "dist/$NAME.tar.gz" | cut -f1))"
