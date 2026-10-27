#!/usr/bin/env bash
# Limpia lo que se acumula al compilar, sin tocar nada que no se regenere
# solo (PDFium y los modelos de vendor/, dist/ y el código).
#
#   scripts/limpiar.sh          # ligera: cachés incrementales, ejemplos,
#                               # restos de makepkg; y todo target/ si aun
#                               # así pasa del límite
#   scripts/limpiar.sh --todo   # todo target/ (cargo clean) y la build de
#                               # Android
#
# El límite (en GB) se cambia con LIMPIAR_LIMITE_GB (por defecto, 20).
# Cargo nunca borra las versiones viejas de lo que compila: target/ solo
# crece, y la única limpieza segura sin herramientas aparte es esta.
set -euo pipefail
cd "$(dirname "$0")/.."

LIMITE=${LIMPIAR_LIMITE_GB:-20}
gb() { du -s --block-size=1G "$1" 2>/dev/null | cut -f1 || echo 0; }

# Cargo bloquea target/<perfil>/.cargo-lock mientras compila: si otra sesión
# está compilando, no se toca nada (borrarle los ficheros la rompería).
for lock in target/*/.cargo-lock target/*/*/.cargo-lock; do
  if [[ -e "$lock" ]] && ! flock -n "$lock" true; then
    echo "Cargo está compilando ($lock): no se limpia ahora."
    exit 0
  fi
done

antes=$(gb target)

if [[ "${1:-}" == --todo ]]; then
  cargo clean -q
  rm -rf android/app/build android/build
else
  # Lo que cargo vuelve a generar en la próxima compilación sin recompilar
  # las dependencias.
  rm -rf target/*/incremental target/*/*/incremental target/*/examples target/*/*/examples
  # Restos de makepkg (los PKGBUILD de packaging/).
  rm -rf packaging/src packaging/pkg
  if [[ -d target && $(gb target) -ge $LIMITE ]]; then
    echo "target/ sigue por encima de $LIMITE GB: cargo clean"
    cargo clean -q
  fi
fi

echo "target/: ${antes} GB → $(gb target) GB"
