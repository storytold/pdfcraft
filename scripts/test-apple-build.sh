#!/bin/bash
set -euo pipefail
source_root="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
fixture="$(mktemp -d "${TMPDIR:-/tmp}/pdfcraft-apple-tests.XXXXXX")"
fixture="$(cd "$fixture" && pwd)"
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/bin" "$fixture/workspace with spaces/scripts" "$fixture/out/PdfCraftMobile"
root="$fixture/workspace with spaces"
cp "$source_root/scripts/"{build-apple,build-ios-rust,generate-swift-bindings}.sh "$root/scripts/"
mkdir -p "$root/packaging/macos"
cp "$source_root/packaging/env.sh" "$root/packaging/"
cp "$source_root/packaging/macos/package.sh" "$root/packaging/macos/actual-package.sh"
cp "$source_root/scripts/test-ios.sh" "$root/scripts/actual-test-ios.sh"
cp "$source_root/packaging/macos/Info.plist.in" "$root/packaging/macos/"
printf '[workspace.package]\nversion = "0.21.1"\n' >"$root/Cargo.toml"
export PATH="$fixture/bin:$PATH" TEST_LOG="$fixture/commands" TEST_OUTPUT="$fixture/out"
export CARGO_TARGET_DIR="$root/target"
for file in SwiftBridgeCore.swift SwiftBridgeCore.h PdfCraftMobile/PdfCraftMobile.swift PdfCraftMobile/PdfCraftMobile.h; do
    printf 'synthetic binding\n' >"$fixture/out/$file"
done
cat >"$fixture/bin/cargo" <<'SH'
#!/bin/bash
printf 'cargo %s\n' "$*" >>"$TEST_LOG"
[[ "${TEST_CARGO_FAILURE:-0}" = 0 ]] || exit 17
printf '{"reason":"build-script-executed","package_id":"pdfcraft-mobile","out_dir":"%s"}\n' "$TEST_OUTPUT"
printf '{"reason":"build-finished","success":true}\n'
SH
cat >"$fixture/bin/uname" <<'SH'
#!/bin/bash
case "$1" in -s) echo Darwin ;; -m) echo arm64 ;; *) exit 2 ;; esac
SH
chmod +x "$fixture/bin/"*
bash "$root/scripts/build-ios-rust.sh" aarch64-apple-ios-sim
if grep -F -- --release "$TEST_LOG"; then echo "Debug unexpectedly used Release." >&2; exit 1; fi
bash "$root/scripts/build-ios-rust.sh" aarch64-apple-ios release
grep -F -- '--release --target aarch64-apple-ios' "$TEST_LOG"
[[ -s "$root/target/ios/Generated/PdfCraftMobile.swift" ]]
if TEST_CARGO_FAILURE=1 bash "$root/scripts/build-ios-rust.sh" aarch64-apple-ios release; then
    echo "Cargo failure was swallowed." >&2; exit 1
fi

cat >"$root/scripts/build-ios-rust.sh" <<'SH'
#!/bin/bash
printf 'rust %s\n' "$*" >>"$TEST_LOG"
SH
cat >"$root/scripts/generate-ios-project.sh" <<'SH'
#!/bin/bash
printf 'generate\n' >>"$TEST_LOG"
SH
cat >"$root/scripts/test-ios.sh" <<'SH'
#!/bin/bash
printf 'test-device %s\n' "$1" >>"$TEST_LOG"
SH
cat >"$root/packaging/macos/package.sh" <<'SH'
#!/bin/bash
printf 'mac-package identity=%s %s\n' "$MACOS_SIGN_IDENTITY" "$*" >>"$TEST_LOG"
mkdir -p "$DIST" "$CARGO_TARGET_DIR/macos-package/PdfCraft.app"
SH
cat >"$fixture/bin/xcodebuild" <<'SH'
#!/bin/bash
printf 'xcode %s\n' "$*" >>"$TEST_LOG"
while [[ $# -gt 0 ]]; do
    if [[ "$1" = -archivePath ]]; then
        app="$2/Products/Applications/PdfCraftMobilePOC.app"
        mkdir -p "$app"
        printf '<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleSupportedPlatforms</key><array><string>%s</string></array></dict></plist>' \
            "${TEST_ARCHIVE_PLATFORM:-iPhoneOS}" >"$app/Info.plist"
    fi
    shift
done
SH
cat >"$fixture/bin/ditto" <<'SH'
#!/bin/bash
if [[ "$1" != -c ]]; then
    mkdir -p "$2/Contents"
    printf 'synthetic plist\n' >"$2/Contents/Info.plist"
    exit 0
fi
for last in "$@"; do :; done
printf 'synthetic zip\n' >"$last"
SH
cat >"$fixture/bin/codesign" <<'SH'
#!/bin/bash
printf 'codesign %s\n' "$*" >>"$TEST_LOG"
SH
chmod +x "$fixture/bin/"*
: >"$TEST_LOG"
bash "$root/scripts/build-apple.sh" build all Release
grep -F 'rust aarch64-apple-ios-sim release' "$TEST_LOG"
grep -F 'rust aarch64-apple-ios release' "$TEST_LOG"
[[ "$(grep -c -- '-configuration Release' "$TEST_LOG")" = 2 ]]
: >"$TEST_LOG"
CARGO_TARGET_DIR='relative target' bash "$root/scripts/build-apple.sh" build ios-device Debug
grep -F "RUST_LIBRARY_ROOT=$root/relative target" "$TEST_LOG"
mkdir -p "$root/target/ios/DerivedData/Build/Products/Debug-iphonesimulator/PdfCraftMobilePOC.app"
cp "$root/packaging/macos/Info.plist.in" \
    "$root/target/ios/DerivedData/Build/Products/Debug-iphonesimulator/PdfCraftMobilePOC.app/Info.plist"
: >"$TEST_LOG"
CARGO_TARGET_DIR='relative target' bash "$root/scripts/actual-test-ios.sh" phone
[[ "$(grep -cF "RUST_LIBRARY_ROOT=$root/relative target" "$TEST_LOG")" = 2 ]]
MACOS_SIGN_IDENTITY=never-use-an-inherited-identity bash "$root/scripts/build-apple.sh" package all Debug
grep -F 'mac-package identity=- --arch aarch64 --profile debug --skip-build' "$TEST_LOG"
[[ -s "$root/target/apple/Debug/macos/PdfCraft.app.zip" ]]
[[ -s "$root/target/apple/Debug/macos/PdfCraft.app/Contents/Info.plist" ]]
[[ -s "$root/target/apple/Debug/ios-simulator/PdfCraftMobilePOC.app.zip" ]]
[[ -s "$root/target/apple/Debug/ios-device/PdfCraftMobilePOC.xcarchive/Products/Applications/PdfCraftMobilePOC.app/Info.plist" ]]
if TEST_ARCHIVE_PLATFORM=iPhoneSimulator bash "$root/scripts/build-apple.sh" package ios-device Debug; then
    echo "A Simulator archive was accepted as an iOS-device archive." >&2; exit 1
fi
: >"$TEST_LOG"
CRAFT_IPHONE_SIMULATOR_UDID=phone CRAFT_IPAD_SIMULATOR_UDID=tablet \
    bash "$root/scripts/build-apple.sh" test ios-simulator Debug
[[ "$(grep 'test-device' "$TEST_LOG")" = "$(printf 'test-device phone\ntest-device tablet')" ]]
for arguments in 'test ios-device Debug' 'test all Release' 'build invalid Debug' 'build all invalid'; do
    # These are fixed test cases, not commands evaluated from environment input.
    read -r phase platform configuration <<<"$arguments"
    if bash "$root/scripts/build-apple.sh" "$phase" "$platform" "$configuration"; then
        echo "Invalid request succeeded: $arguments" >&2; exit 1
    fi
done
if CRAFT_IPHONE_SIMULATOR_UDID=same CRAFT_IPAD_SIMULATOR_UDID=same \
    bash "$root/scripts/build-apple.sh" test ios-simulator Debug; then
    echo "One device was accepted for both families." >&2; exit 1
fi

cat >"$fixture/bin/lipo" <<'SH'
#!/bin/bash
printf 'lipo %s\n' "$*" >>"$TEST_LOG"
exit 78
SH
chmod +x "$fixture/bin/lipo"
for profile in debug release; do
    : >"$TEST_LOG"
    result=0
    MACOS_SIGN_IDENTITY=- bash "$root/packaging/macos/actual-package.sh" \
        --arch aarch64 --profile "$profile" --skip-build || result=$?
    [[ "$result" = 78 ]]
    grep -F "/aarch64-apple-darwin/$profile/pdfcraft" "$TEST_LOG"
done
echo "Apple build contract, profile, failure, unsigned packaging and sequential-device regressions passed."
