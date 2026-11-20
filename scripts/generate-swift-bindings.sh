#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
target="${1:-aarch64-apple-ios-sim}"
python3 - "$target" <<'PY'
import json
import pathlib
import shutil
import sys

target = sys.argv[1]
manifest = pathlib.Path("target/ios") / f"build-{target}.json"
out = None
for line in manifest.read_text().splitlines():
    message = json.loads(line)
    if message.get("reason") == "build-script-executed" and "pdfcraft-mobile" in message.get("package_id", ""):
        out = pathlib.Path(message["out_dir"])
if out is None:
    raise SystemExit("No mobile bindings found. Run bash scripts/build-ios-rust.sh first.")
destination = pathlib.Path("target/ios/Generated")
destination.mkdir(parents=True, exist_ok=True)
for source in ("SwiftBridgeCore.swift", "SwiftBridgeCore.h", "PdfCraftMobile/PdfCraftMobile.swift", "PdfCraftMobile/PdfCraftMobile.h"):
    shutil.copyfile(out / source, destination / pathlib.Path(source).name)
print(f"Generated Swift bindings: {destination}")
PY
