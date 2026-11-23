#!/usr/bin/env bash
# Resolve AppImage update metadata for the binary passed as $1. No downloads or GUI launch.
set -euo pipefail
export LC_ALL=C
REPO="${PDFCRAFT_UPDATE_REPO:-${GITHUB_REPOSITORY:-}}"
if [[ -z "$REPO" ]]; then
  QUERY=("$1")
  if [[ -n "${CROSS_TARGET:-}" ]]; then
    if [[ -z "${EMULATOR:-}" ]]; then
      echo "error: cannot query a cross-compiled binary; set PDFCRAFT_UPDATE_REPO or EMULATOR" >&2
      exit 1
    fi
    QUERY=("$EMULATOR" "$1")
  fi
  # Older PdfCraft builds interpret unknown flags as view options and may open a window.
  # Check their headless help first; also bound a broken executable or emulator.
  HELP="$(timeout 10 "${QUERY[@]}" --help)"
  if [[ "$HELP" != *--print-update-repository* ]]; then
    echo "error: this binary cannot report its update source; set PDFCRAFT_UPDATE_REPO explicitly" >&2
    exit 1
  fi
  REPO="$(timeout 10 "${QUERY[@]}" --print-update-repository)"
fi
# Reject malformed overrides or output from an old/incompatible binary, including zsync's '|'.
OWNER="${REPO%%/*}"
NAME="${REPO#*/}"
if [[ ! "$REPO" =~ ^[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9._-]+$ ||
      ${#OWNER} -gt 39 || ${#NAME} -gt 100 || "$OWNER" == *- || "$OWNER" == *--* ||
      "$NAME" == . || "$NAME" == .. ]]; then
  echo "error: update source must be a GitHub owner/repo; set PDFCRAFT_UPDATE_REPO explicitly" >&2
  exit 1
fi
printf '%s\n' "$REPO"
