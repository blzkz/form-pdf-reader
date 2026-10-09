#!/usr/bin/env bash
# Crea la clave de subida a Google Play (upload key) con la que la integración
# continua firma el APK y el App Bundle, y la guarda como secretos del
# repositorio de GitHub.
#
#   scripts/android/crear-clave-de-subida.sh
#
# La clave (PKCS#12) y su contraseña quedan en ~/.config/form-pdf-reader
# (o en $FORM_PDF_READER_KEY_DIR), fuera del repositorio. Haz una copia de
# seguridad: si se pierde, Google Play permite cambiar la clave de subida,
# pero los APK publicados en GitHub ya no se podrán actualizar encima de los
# instalados.
#
# Necesita openssl, y gh con sesión iniciada para guardar los secretos.
set -euo pipefail
cd "$(dirname "$0")/../.."

DIR="${FORM_PDF_READER_KEY_DIR:-$HOME/.config/form-pdf-reader}"
KEY="$DIR/upload-key.p12"
PASSFILE="$DIR/upload-key.password"
ALIAS=upload

if [[ -e "$KEY" ]]; then
  echo "Ya existe $KEY: no se sobrescribe."
else
  mkdir -p "$DIR"
  chmod 700 "$DIR"
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  PASS="$(openssl rand -base64 32 | tr -d '/+=\n')"
  export PASS
  # RSA de 4096 bits, válida 27 años (Google Play pide más de 25).
  openssl req -x509 -newkey rsa:4096 -sha256 -days 10000 -nodes \
    -subj "/CN=Form PDF Reader/O=blzkz" -keyout "$tmp/key.pem" -out "$tmp/cert.pem" 2>/dev/null
  openssl pkcs12 -export -name "$ALIAS" -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
    -out "$KEY" -passout env:PASS
  printf '%s\n' "$PASS" >"$PASSFILE"
  chmod 600 "$KEY" "$PASSFILE"
  echo "Clave creada: $KEY"
  echo "Contraseña:   $PASSFILE"
fi

PASS="$(cat "$PASSFILE")"
export PASS
echo
echo "Huella SHA-256 del certificado (la verás en Google Play Console):"
openssl pkcs12 -in "$KEY" -passin env:PASS -nokeys 2>/dev/null | openssl x509 -noout -fingerprint -sha256

echo
read -r -p "¿Guardar la clave como secretos del repositorio de GitHub con gh? [s/N] " ok
if [[ "$ok" == [sS]* ]]; then
  base64 -w0 "$KEY" | gh secret set ANDROID_KEYSTORE_BASE64
  printf '%s' "$PASS" | gh secret set ANDROID_KEYSTORE_PASSWORD
  printf '%s' "$ALIAS" | gh secret set ANDROID_KEY_ALIAS
  echo "Secretos guardados: ANDROID_KEYSTORE_BASE64, ANDROID_KEYSTORE_PASSWORD, ANDROID_KEY_ALIAS."
else
  echo "No se han guardado. Puedes volver a ejecutar este script cuando quieras."
fi
echo
echo "Guarda una copia de seguridad de $DIR en un lugar seguro."
