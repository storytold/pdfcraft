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
xcodegen generate --spec apps/ios/project.yml
