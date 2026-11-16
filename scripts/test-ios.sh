#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
device="${1:?Pass an available Simulator UUID}"
case "${CARGO_TARGET_DIR:-target}" in
    /*) rust_root="$CARGO_TARGET_DIR" ;;
    *) rust_root="$PWD/${CARGO_TARGET_DIR:-target}" ;;
esac
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
cargo test --locked -j 2 -p pdfcraft-mobile --features swift --lib --test workflow
bash scripts/build-ios-rust.sh aarch64-apple-ios-sim
bash scripts/generate-ios-project.sh
mkdir -p target/ios/TestResults
results="$(mktemp -d "$PWD/target/ios/TestResults/$device.XXXXXX")"
printf 'Retained native test results: %s\n' "$results"
xcodebuild -quiet -project apps/ios/PdfCraftMobilePOC.xcodeproj -scheme PdfCraftMobilePOC \
    -destination "platform=iOS Simulator,id=$device" -derivedDataPath target/ios/DerivedData \
    -resultBundlePath "$results/hosted.xcresult" \
    -parallel-testing-enabled NO -only-testing:PdfCraftMobilePOCTests CODE_SIGNING_ALLOWED=NO \
    "RUST_LIBRARY_ROOT=$rust_root" test
for key in CFBundleDisplayName CFBundleDevelopmentRegion NSHumanReadableCopyright; do
    expected="$(plutil -extract "$key" raw -o - packaging/macos/Info.plist.in)"
    actual="$(plutil -extract "$key" raw -o - target/ios/DerivedData/Build/Products/Debug-iphonesimulator/PdfCraftMobilePOC.app/Info.plist)"
    if [[ "$actual" != "$expected" ]]; then
        echo "iOS bundle $key differs from the macOS metadata source." >&2
        exit 1
    fi
done
for key in CFBundleShortVersionString PdfCraftVersion PdfCraftBuildCommit; do
    expected="$(plutil -extract "$key" raw -o - target/ios/Info.plist)"
    actual="$(plutil -extract "$key" raw -o - target/ios/DerivedData/Build/Products/Debug-iphonesimulator/PdfCraftMobilePOC.app/Info.plist)"
    if [[ "$actual" != "$expected" ]]; then
        echo "iOS bundle $key differs from the generated build metadata." >&2
        exit 1
    fi
done
xcodebuild -quiet -project apps/ios/PdfCraftMobilePOC.xcodeproj -scheme PdfCraftMobilePOC \
    -destination "platform=iOS Simulator,id=$device" -derivedDataPath target/ios/DerivedData \
    -resultBundlePath "$results/ui.xcresult" \
    -parallel-testing-enabled NO -test-timeouts-enabled YES -maximum-test-execution-time-allowance 300 \
    -only-testing:PdfCraftMobilePOCUITests CODE_SIGNING_ALLOWED=NO \
    "RUST_LIBRARY_ROOT=$rust_root" test
