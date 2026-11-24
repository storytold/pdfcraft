#!/bin/sh
set -eu

if [ -z "${PDFCRAFT_CONFIG_DIR:-}" ]; then
  config_home="${XDG_CONFIG_HOME:-${HOME:-}/.config}"
  if [ -n "$config_home" ] && [ "$config_home" != "/.config" ]; then
    export PDFCRAFT_CONFIG_DIR="$config_home/pdfcraft-nightly"
  fi
fi

real_bin="$(dirname "$0")/.pdfcraft-nightly/pdfcraft"
exec "$real_bin" "$@"
