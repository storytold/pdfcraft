#!/usr/bin/env bash
# Fetches the text-recognition models (Recognize text) into assets/models/, checked by SHA-256,
# with their licence and an attribution note: what `cargo xtask models` does in the full PdfCraft
# workspace. The public repository (artcraft-android/pdfcraft-android) commits them, so its builds
# (F-Droid's too) download nothing; this fetches them again. Without them the app still builds, and
# Recognize text says it isn't installed.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
out="$root/assets/models"
mkdir -p "$out"

licence_url="https://creativecommons.org/licenses/by-sa/4.0/legalcode.txt"
licence_sha="28a9529c7d0bb4dc51f4bf5c116a3d16ef247a052f7591466768ddf563fd1cf5"
source="https://github.com/robertknight/ocrs-models (the models ocrs downloads; model card: https://huggingface.co/robertknight/ocrs, cc-by-sa-4.0)"
author="Robert Knight (ocrs-models; trained on HierText, CC-BY-SA-4.0)"

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

# fetch URL FILE SHA256: download unless the file is already there with the right hash
fetch() {
  local url="$1" file="$2" want="$3"
  if [ -f "$file" ] && [ "$(sha256 "$file")" = "$want" ]; then return; fi
  curl -fsSL --retry 3 -o "$file.part" "$url"
  local got
  got="$(sha256 "$file.part")"
  if [ "$got" != "$want" ]; then
    rm -f "$file.part"
    echo "error: $url has SHA-256 $got, expected $want" >&2
    exit 1
  fi
  mv "$file.part" "$file"
}

fetch "$licence_url" "$out/LICENCE.part.txt" "$licence_sha"

{
  echo "PdfCraft's text recognition (Scan & OCR) uses these models, unmodified."
  for entry in \
    "text-detection.rten|Ocrs text detection model|f15cfb56bd02c4bf478a20343986504a1f01e1665c2b3a0ad66340f054b1b5ca" \
    "text-recognition.rten|Ocrs text recognition model|e484866d4cce403175bd8d00b128feb08ab42e208de30e42cd9889d8f1735a6e"; do
    IFS='|' read -r file title sha <<<"$entry"
    fetch "https://ocrs-models.s3-accelerate.amazonaws.com/$file" "$out/$file" "$sha"
    cp "$out/LICENCE.part.txt" "$out/$file.LICENCE.txt"
    echo
    echo "$file"
    echo "  $title"
    echo "  by $author"
    echo "  from $source"
    echo "  licence: CC-BY-SA-4.0 ($licence_url; full text in $file.LICENCE.txt)"
  done
} >"$out/ATTRIBUTION.txt.part"
mv "$out/ATTRIBUTION.txt.part" "$out/ATTRIBUTION.txt"
rm -f "$out/LICENCE.part.txt"
echo "models in $out"
