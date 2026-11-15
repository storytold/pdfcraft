#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
device="${1:?Pass an available Simulator UUID from xcrun simctl list devices available}"
state="$(xcrun simctl list devices booted)"
if ! printf '%s' "$state" | grep -F "$device" >/dev/null; then
    xcrun simctl boot "$device"
fi
xcrun simctl bootstatus "$device" -b
xcrun simctl install "$device" target/ios/DerivedData/Build/Products/Debug-iphonesimulator/PdfCraftMobilePOC.app
xcrun simctl launch "$device" com.blackoutsecure.pdfcraft.poc
