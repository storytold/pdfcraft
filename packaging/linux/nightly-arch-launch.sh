#!/bin/sh
set -eu

if [ -z "${XDG_DATA_HOME:-}" ] && [ -z "${HOME:-}" ]; then
  echo "pdfcraft-nightly: XDG_DATA_HOME and HOME are both unset, so there is no Nightly data folder" >&2
  exit 2
fi

real_bin="$(dirname "$0")/.pdfcraft-nightly/pdfcraft"
root="${XDG_DATA_HOME:-$HOME/.local/share}/pdfcraft-nightly"
# A version query is answered in the Launcher's form; everything else is a file, after `--`.
case "${1:-}" in
  --version | -V) exec "$real_bin" --launch-profile nightly-v1 --profile-root "$root" "$1" ;;
esac
exec "$real_bin" --launch-profile nightly-v1 --profile-root "$root" -- "$@"
