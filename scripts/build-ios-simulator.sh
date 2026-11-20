#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
exec bash scripts/build-apple.sh build ios-simulator "${1:-Debug}"
