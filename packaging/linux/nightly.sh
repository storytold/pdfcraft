#!/usr/bin/env bash
# App-owned GUI-only nightly tar: no FUSE/AppImage, CLI, association or install hooks.
set -euo pipefail
ARCH=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --arch) [[ $# -ge 2 ]] || { echo 'missing --arch value' >&2; exit 2; }; ARCH="$2"; shift 2 ;;
    -h|--help) echo 'Usage: nightly.sh --arch x86_64'; exit 0 ;;
    *) echo "unsupported nightly argument: $1" >&2; exit 2 ;;
  esac
done
[[ "$ARCH" == x86_64 && "$(uname -s)" == Linux ]] || { echo 'only native Linux x86_64 nightly packaging is supported' >&2; exit 2; }
# shellcheck source=../nightly-common.sh
. "$(dirname "${BASH_SOURCE[0]}")/../nightly-common.sh"
nightly_init linux "$ARCH" x86_64-unknown-linux-gnu
OUTPUT="$DIST/pdfcraft-$VERSION-linux-x86_64-nightly.tar.gz"
[[ ! -e "$OUTPUT" && ! -L "$OUTPUT" ]] || nightly_die 'refusing to replace an existing nightly archive'
nightly_build_gui
STAGE="$NIGHTLY_WORK/stage"
mkdir -p "$STAGE"
cp "$NIGHTLY_GUI" "$STAGE/pdfcraft"
chmod 755 "$STAGE/pdfcraft"
# An unpacked tar is isolated too: the launcher starts the GUI through its launch profile, and the
# bare binary refuses to run without one.
cat >"$STAGE/pdfcraft-nightly" <<'LAUNCHER'
#!/bin/sh
# PdfCraft Nightly. Its own data root: the stable app's settings, recovery and socket are never touched.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
if [ -z "${XDG_DATA_HOME:-}" ] && [ -z "${HOME:-}" ]; then
  echo "pdfcraft-nightly: XDG_DATA_HOME and HOME are both unset, so there is no Nightly data folder" >&2
  exit 2
fi
root="${XDG_DATA_HOME:-$HOME/.local/share}/pdfcraft-nightly"
# A version query is answered in the Launcher's form; everything else is a file, after `--`.
case "${1:-}" in
  --version | -V) exec "$here/pdfcraft" --launch-profile nightly-v1 --profile-root "$root" "$1" ;;
esac
exec "$here/pdfcraft" --launch-profile nightly-v1 --profile-root "$root" -- "$@"
LAUNCHER
chmod 755 "$STAGE/pdfcraft-nightly"
if [[ -f "$ROOT/assets/app-icon/pdfcraft-nightly.png" ]]; then
  cp "$ROOT/assets/app-icon/pdfcraft-nightly.png" "$STAGE/pdfcraft-nightly.png"
else
  cp "$ROOT/assets/app-icon/hicolor/256x256/apps/ai.storyteller.pdfcraft.png" "$STAGE/pdfcraft-nightly.png"
fi
nightly_copy_notices "$STAGE"
# The private mktemp cwd is the helper authority root, not a CLI path parent.
(cd "$NIGHTLY_WORK" && python3 "$ROOT/packaging/nightly-archive.py" linux stage archive.tar.gz)
nightly_publish_archive "$NIGHTLY_WORK/archive.tar.gz" "$OUTPUT"
