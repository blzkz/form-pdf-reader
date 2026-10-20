#!/usr/bin/env bash
# Descarga los modelos de visión que usa la app de Android en vendor/models,
# comprobando su SHA-256:
#
#   doc_ori.onnx  PP-LCNet_x1_0_doc_ori de PaddleOCR (Apache 2.0): si una página
#                 escaneada está girada 0, 90, 180 o 270 grados. Conversión a
#                 ONNX de https://huggingface.co/monkt/paddleocr-onnx (revisión
#                 fija), del modelo de https://github.com/PaddlePaddle/PaddleOCR.
set -euo pipefail
cd "$(dirname "$0")/.."

DEST=vendor/models
REV=7b02d0a30a07ba2b92ad1ff5a8941ae2c633de65
URL="https://huggingface.co/monkt/paddleocr-onnx/resolve/$REV/preprocessing/doc-orientation/PP-LCNet_x1_0_doc_ori.onnx"
SHA256=f5516822af9262711e197ff224a8a9d884f8046a6321b762e34f8cbf082c45ef

mkdir -p "$DEST"
if [[ -f "$DEST/doc_ori.onnx" ]] && echo "$SHA256  $DEST/doc_ori.onnx" | sha256sum -c --quiet - 2>/dev/null; then
  exit 0
fi
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
echo "Downloading $URL"
curl -fL --retry 3 -o "$tmp" "$URL"
echo "$SHA256  $tmp" | sha256sum -c --quiet -
chmod 644 "$tmp"
mv "$tmp" "$DEST/doc_ori.onnx"
trap - EXIT
