#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
    echo "Usage: bash scripts/build-apple.sh <build|test|package> <all|macos|ios-simulator|ios-device> <Debug|Release>"
}

if [[ "${1:-}" = --help || "${1:-}" = -h ]]; then usage; exit 0; fi
phase="${1:-build}"
platform="${2:-all}"
configuration="${3:-Debug}"
[[ $# -le 3 ]] || { usage >&2; exit 2; }
case "$phase" in build|test|package) ;; *) usage >&2; exit 2 ;; esac
case "$platform" in all|macos|ios-simulator|ios-device) ;; *) usage >&2; exit 2 ;; esac
case "$configuration" in
    Debug) profile=debug; profile_args=(--profile dev) ;;
    Release) profile=release; profile_args=(--release) ;;
    *) usage >&2; exit 2 ;;
esac
if [[ "$phase" = test ]]; then
    [[ "$configuration" = Debug ]] || { echo "Tests currently require Debug configuration." >&2; exit 2; }
    [[ "$platform" != ios-device ]] || { echo "Unsigned device builds cannot run physical-device tests." >&2; exit 2; }
    if [[ "$platform" != macos ]]; then
        : "${CRAFT_IPHONE_SIMULATOR_UDID:?Select an available iPhone Simulator UUID}"
        : "${CRAFT_IPAD_SIMULATOR_UDID:?Select an available iPad Simulator UUID}"
        [[ "$CRAFT_IPHONE_SIMULATOR_UDID" != "$CRAFT_IPAD_SIMULATOR_UDID" ]] ||
            { echo "iPhone and iPad tests require different devices." >&2; exit 2; }
    fi
fi
[[ "$(uname -s)" = Darwin ]] || { echo "Apple builds require macOS and Xcode." >&2; exit 1; }
# shellcheck source=../packaging/env.sh
. packaging/env.sh
case "$CARGO_TARGET_DIR" in
    /*) ;;
    *) export CARGO_TARGET_DIR="$ROOT/$CARGO_TARGET_DIR" ;;
esac
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
artifacts="$ROOT/target/apple/$configuration"
case "$(uname -m)" in
    arm64) mac_target=aarch64-apple-darwin; mac_arch=aarch64 ;;
    x86_64)
        [[ "$platform" = macos ]] || { echo "The iOS POC requires an Apple Silicon host." >&2; exit 1; }
        mac_target=x86_64-apple-darwin; mac_arch=x86_64
        ;;
    *) echo "Unsupported macOS host architecture." >&2; exit 1 ;;
esac
if [[ "$platform" = all ]]; then
    platforms=(macos ios-simulator ios-device)
    [[ "$phase" != test ]] || platforms=(macos ios-simulator)
else
    platforms=("$platform")
fi

build_macos() {
    cargo build --locked -j "${CARGO_BUILD_JOBS:-2}" "${profile_args[@]}" \
        --target "$mac_target" -p pdfcraft -p pdfcraft-cli
}

build_ios() {
    local kind="$1" action="$2" target sdk destination
    if [[ "$kind" = ios-device ]]; then
        target=aarch64-apple-ios; sdk=iphoneos; destination='generic/platform=iOS'
    else
        target=aarch64-apple-ios-sim; sdk=iphonesimulator; destination='generic/platform=iOS Simulator'
    fi
    bash scripts/build-ios-rust.sh "$target" "$profile"
    bash scripts/generate-ios-project.sh
    local args=(-project apps/ios/PdfCraftMobilePOC.xcodeproj -scheme PdfCraftMobilePOC
        -configuration "$configuration" -sdk "$sdk" -destination "$destination"
        -derivedDataPath target/ios/DerivedData CODE_SIGNING_ALLOWED=NO
        "RUST_LIBRARY_ROOT=$CARGO_TARGET_DIR")
    if [[ "$action" = archive ]]; then
        args+=(-archivePath "$artifacts/ios-device/PdfCraftMobilePOC.xcarchive")
    fi
    xcodebuild "${args[@]}" "$action"
}

for current in "${platforms[@]}"; do
    echo "==> Apple POC: $phase $current $configuration (no Apple distribution signing/upload)"
    case "$phase:$current" in
        build:macos) build_macos ;;
        build:ios-*) build_ios "$current" build ;;
        test:macos) cargo test --locked -j "${CARGO_BUILD_JOBS:-2}" -p pdfcraft-platform --lib ;;
        test:ios-simulator)
            # Bindings and DerivedData are shared locally; do not run the families concurrently.
            bash scripts/test-ios.sh "$CRAFT_IPHONE_SIMULATOR_UDID"
            bash scripts/test-ios.sh "$CRAFT_IPAD_SIMULATOR_UDID"
            ;;
        package:macos)
            build_macos
            DIST="$artifacts/macos" MACOS_SIGN_IDENTITY=- \
                bash packaging/macos/package.sh --arch "$mac_arch" --profile "$profile" --skip-build
            mkdir -p "$artifacts/macos"
            ditto "$CARGO_TARGET_DIR/macos-package/PdfCraft.app" "$artifacts/macos/PdfCraft.app"
            codesign --verify --strict --deep "$artifacts/macos/PdfCraft.app"
            ditto -c -k --keepParent "$artifacts/macos/PdfCraft.app" \
                "$artifacts/macos/PdfCraft.app.zip"
            echo "macOS artifacts are ad-hoc signed, NOT Developer ID signed/notarized or Mac App Store packages."
            ;;
        package:ios-simulator)
            build_ios "$current" build
            mkdir -p "$artifacts/ios-simulator"
            ditto -c -k --keepParent \
                "target/ios/DerivedData/Build/Products/$configuration-iphonesimulator/PdfCraftMobilePOC.app" \
                "$artifacts/ios-simulator/PdfCraftMobilePOC.app.zip"
            ;;
        package:ios-device)
            mkdir -p "$artifacts/ios-device"
            build_ios "$current" archive
            archive="$artifacts/ios-device/PdfCraftMobilePOC.xcarchive"
            app="$archive/Products/Applications/PdfCraftMobilePOC.app"
            [[ "$(plutil -extract CFBundleSupportedPlatforms.0 raw -o - "$app/Info.plist")" = iPhoneOS ]] ||
                { echo "Archive is not an iOS-device application." >&2; exit 1; }
            echo "Unsigned device archive: $archive (not an installable IPA or eligible for TestFlight)."
            ;;
    esac
done
echo "==> Apple POC $phase complete; package artifacts: $artifacts"
