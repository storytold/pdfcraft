#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
target="${1:-aarch64-apple-ios-sim}"
profile="${2:-debug}"
if [[ $# -gt 2 ]]; then
    echo "Usage: bash scripts/build-ios-rust.sh [target] [debug|release]" >&2
    exit 2
fi
case "$target" in
    aarch64-apple-ios|aarch64-apple-ios-sim) ;;
    *) echo "Use aarch64-apple-ios or aarch64-apple-ios-sim" >&2; exit 1 ;;
esac
case "$profile" in
    debug) profile_args=(--profile dev) ;;
    release) profile_args=(--release) ;;
    *) echo "Use the Rust profile debug or release" >&2; exit 2 ;;
esac
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 IPHONEOS_DEPLOYMENT_TARGET=17.0
mkdir -p target/ios
cargo build --locked -j "${CARGO_BUILD_JOBS:-2}" -p pdfcraft-mobile --features swift \
    "${profile_args[@]}" --target "$target" --message-format=json-render-diagnostics |
    tee "target/ios/build-${target}.json" |
    python3 -c 'import json, sys
for line in sys.stdin:
    event = json.loads(line)
    if event.get("reason") == "compiler-message":
        print(event["message"]["rendered"], end="", flush=True)
    elif event.get("reason") == "build-finished":
        print("Rust build success:", event["success"], flush=True)'
bash scripts/generate-swift-bindings.sh "$target"
