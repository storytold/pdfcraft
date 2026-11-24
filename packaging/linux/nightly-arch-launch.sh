#!/bin/sh
set -eu

real_bin="$(dirname "$0")/.pdfcraft-nightly/pdfcraft"
exec "$real_bin" "$@"
