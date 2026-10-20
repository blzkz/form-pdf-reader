#!/usr/bin/env bash
# Descarga los modelos de visión que usa la app de Android en vendor/models,
# comprobando su SHA-256. Son de PaddleOCR (https://github.com/PaddlePaddle/PaddleOCR,
# Apache 2.0), en la conversión a ONNX de https://huggingface.co/monkt/paddleocr-onnx
# (Apache 2.0, revisión fija):
#
#   doc_ori.onnx   PP-LCNet_x1_0_doc_ori: si una página escaneada está girada
#                  0, 90, 180 o 270 grados.
#   det.onnx       PP-OCRv3 mobile det: dónde hay líneas de texto (OCR).
#   rec.onnx       PP-OCRv5 latin mobile rec: el texto de cada línea, en los
#   rec_dict.txt   idiomas de alfabeto latino, y sus caracteres.
set -euo pipefail
cd "$(dirname "$0")/.."

DEST=vendor/models
REV=7b02d0a30a07ba2b92ad1ff5a8941ae2c633de65
BASE="https://huggingface.co/monkt/paddleocr-onnx/resolve/$REV"

fetch() { # nombre ruta sha256
  local out="$DEST/$1"
  if [[ -f "$out" ]] && echo "$3  $out" | sha256sum -c --quiet - 2>/dev/null; then
    return
  fi
  local tmp
  tmp=$(mktemp)
  echo "Downloading $BASE/$2"
  curl -fsSL --retry 3 -o "$tmp" "$BASE/$2"
  echo "$3  $tmp" | sha256sum -c --quiet - || { rm -f "$tmp"; exit 1; }
  chmod 644 "$tmp"
  mv "$tmp" "$out"
}

mkdir -p "$DEST"
fetch doc_ori.onnx preprocessing/doc-orientation/PP-LCNet_x1_0_doc_ori.onnx f5516822af9262711e197ff224a8a9d884f8046a6321b762e34f8cbf082c45ef
fetch det.onnx detection/v3/det.onnx ee40e80071ba3a320d4efda75f3e22047a7d049e9bf7bcaaf9daea23fc21b935
fetch rec.onnx languages/latin/rec.onnx 614ffc2d6d3902d360fad7f1b0dd455ee45e877069d14c4e51a99dc4ef144409
fetch rec_dict.txt languages/latin/dict.txt 3c0a8a79b612653c25f765271714f71281e4e955962c153e272b7b8c1d2b13ff
