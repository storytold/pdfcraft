#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
if ! command -v xcodegen >/dev/null; then
    echo "Install XcodeGen 2.46.0 (MIT), then rerun this script." >&2
    exit 1
fi
if [ "$(xcodegen --version)" != "Version: 2.46.0" ]; then
    echo "This POC is validated with XcodeGen 2.46.0; found $(xcodegen --version)." >&2
    exit 1
fi
. packaging/env.sh
export PDFCRAFT_IOS_SHORT_VERSION="${VERSION%%[-+]*}"
if [[ ! "$PDFCRAFT_IOS_SHORT_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "iOS requires a numeric X.Y.Z marketing version; found $VERSION." >&2
    exit 1
fi
mkdir -p target/ios
cp apps/ios/Config/Info.plist target/ios/Info.plist
# Inherit only compatible desktop fields, not macOS handlers, localizations or signing identity.
for key in CFBundleDisplayName CFBundleDevelopmentRegion NSHumanReadableCopyright \
    CFBundleShortVersionString PdfCraftVersion PdfCraftBuildCommit; do
    if ! value="$(plutil -extract "$key" raw -o - packaging/macos/Info.plist.in)"; then
        echo "Cannot inherit $key from packaging/macos/Info.plist.in." >&2
        exit 1
    fi
    value="${value//@SHORT_VERSION@/$PDFCRAFT_IOS_SHORT_VERSION}"
    value="${value//@VERSION@/$VERSION}"
    value="${value//@BUILD_SHA@/${PDFCRAFT_BUILD_SHA:-unknown}}"
    plutil -insert "$key" -string "$value" target/ios/Info.plist
done
if [[ -z "$PDFCRAFT_BUILD_SHA" ]]; then
    warn "iOS build commit is unknown; set PDFCRAFT_BUILD_SHA when building outside a Git checkout"
fi
PDFCRAFT_APP_DISPLAY_NAME="$(plutil -extract CFBundleDisplayName raw -o - target/ios/Info.plist)"
PDFCRAFT_APP_LANGUAGE="$(plutil -extract CFBundleDevelopmentRegion raw -o - target/ios/Info.plist)"
export PDFCRAFT_APP_DISPLAY_NAME PDFCRAFT_APP_LANGUAGE
xcodegen generate --spec apps/ios/project.yml
