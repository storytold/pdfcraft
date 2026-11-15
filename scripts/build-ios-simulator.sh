#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
bash scripts/build-ios-rust.sh aarch64-apple-ios-sim
bash scripts/generate-ios-project.sh
xcodebuild -project apps/ios/PdfCraftMobilePOC.xcodeproj -scheme PdfCraftMobilePOC \
    -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' \
    -derivedDataPath target/ios/DerivedData CODE_SIGNING_ALLOWED=NO build
