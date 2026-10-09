#!/usr/bin/env bash
# Instala form-pdf-reader para el usuario actual en ~/.local (sin sudo).
# Uso: scripts/instalar-local.sh            (instala)
#      scripts/instalar-local.sh --quitar   (desinstala)
set -euo pipefail
cd "$(dirname "$0")/.."
PREFIX="${PREFIX:-$HOME/.local}"
LIBDIR="$PREFIX/lib/form-pdf-reader"
APPS="$PREFIX/share/applications"
ICONS="$PREFIX/share/icons/hicolor/scalable/apps"

if [[ "${1:-}" == "--quitar" ]]; then
  rm -rf "$LIBDIR" "$PREFIX/bin/form-pdf-reader" "$APPS/form-pdf-reader.desktop" "$ICONS/form-pdf-reader.svg"
  rm -f "$PREFIX"/share/icons/hicolor/*x*/apps/form-pdf-reader.png
  command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" || true
  echo "Desinstalado."
  exit 0
fi

BIN="target/release/form-pdf-reader"
if [[ ! -x "$BIN" ]]; then
  echo "Compilando en modo release..."
  cargo build --release
fi
install -Dm755 "$BIN" "$LIBDIR/form-pdf-reader"
install -Dm644 vendor/pdfium/lib/libpdfium.so "$LIBDIR/libpdfium.so"
mkdir -p "$LIBDIR/licenses" && cp -r vendor/pdfium/LICENSE vendor/pdfium/licenses/. "$LIBDIR/licenses/" 2>/dev/null || true
mkdir -p "$PREFIX/bin"
ln -sf "$LIBDIR/form-pdf-reader" "$PREFIX/bin/form-pdf-reader"
install -Dm644 assets/form-pdf-reader.desktop "$APPS/form-pdf-reader.desktop"
install -Dm644 assets/form-pdf-reader.svg "$ICONS/form-pdf-reader.svg"
for s in 16 24 32 48 64 128 256; do
  install -Dm644 "assets/icons/$s/form-pdf-reader.png" "$PREFIX/share/icons/hicolor/${s}x${s}/apps/form-pdf-reader.png"
done
command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" || true
echo "Instalado en $LIBDIR"
echo "Ejecuta: form-pdf-reader fichero.pdf   (asegúrate de tener $PREFIX/bin en el PATH)"
echo "Para que sea el visor de PDF por defecto: xdg-mime default form-pdf-reader.desktop application/pdf"
