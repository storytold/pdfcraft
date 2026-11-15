#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
device="${1:?Pass an available Simulator UUID}"
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
cargo test --locked -j 2 -p pdfcraft-mobile --features swift --test workflow
bash scripts/build-ios-rust.sh aarch64-apple-ios-sim
bash scripts/generate-ios-project.sh
xcodebuild -quiet -project apps/ios/PdfCraftMobilePOC.xcodeproj -scheme PdfCraftMobilePOC \
    -destination "platform=iOS Simulator,id=$device" -derivedDataPath target/ios/DerivedData \
    -parallel-testing-enabled NO -only-testing:PdfCraftMobilePOCTests CODE_SIGNING_ALLOWED=NO test
xcodebuild -quiet -project apps/ios/PdfCraftMobilePOC.xcodeproj -scheme PdfCraftMobilePOC \
    -destination "platform=iOS Simulator,id=$device" -derivedDataPath target/ios/DerivedData \
    -parallel-testing-enabled NO -test-timeouts-enabled YES -maximum-test-execution-time-allowance 300 \
    -only-testing:PdfCraftMobilePOCUITests CODE_SIGNING_ALLOWED=NO test
