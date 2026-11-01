#!/usr/bin/env bash
# PdfCraft checks the shared packaging lint does not cover: the macOS Hot Folder installer
# scripts (no .sh suffix), the product Distribution template and the server packaging checks.
set -euo pipefail
shellcheck packaging/macos/hot-folder-scripts/preinstall packaging/macos/hot-folder-scripts/postinstall
xmllint --noout packaging/macos/Distribution.xml.in
pwsh -NoProfile -File packaging/server-tests/check.ps1
