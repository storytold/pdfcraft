#!/usr/bin/env bash
# Build PdfCraft's browser app and stage its OCR models for Vercel's static output.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
export PATH="$HOME/.cargo/bin:$(npm prefix --global)/bin:$PATH"

command -v trunk >/dev/null 2>&1 || { echo "error: trunk is missing; run vercel-install.sh first" >&2; exit 1; }
command -v wasm-opt >/dev/null 2>&1 || { echo "error: wasm-opt is missing; run vercel-install.sh first" >&2; exit 1; }

cd "$HERE"
trunk build --release --dist dist

cd "$ROOT"
cargo xtask models "$HERE/dist/models"
