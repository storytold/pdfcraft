#!/bin/sh
# The entry point of every PdfCraft Nightly Linux format: the deb/rpm (installed as
# `/usr/bin/pdfcraft-nightly`), the AppImage (`AppRun` and `usr/bin/pdfcraft-nightly`), the Arch
# handoff (`bin/pdfcraft`, before the packager renames it) and the plain tarball (`pdfcraft`, beside
# the GUI as `pdfcraft-bin`). A Nightly build only starts with its private profile, and every
# format passes the Launcher's literal protocol:
# `--launch-profile nightly-v1 --profile-root <dir> -- <files>`.
set -eu

here="$(dirname "$(readlink -f "$0")")"
# The GUI beside the launcher (the tarball), in the package's own folder (deb/rpm, the Arch
# handoff) or in usr/bin (the AppImage). The same script serves a moved tree, so one copy can be
# checked instead of one per format.
real=
for candidate in "$here/pdfcraft-bin" "$here/../lib/pdfcraft-nightly/pdfcraft" "$here/.pdfcraft-nightly/pdfcraft" "$here/usr/bin/pdfcraft"; do
  if [ -x "$candidate" ] && [ ! "$candidate" -ef "$0" ]; then
    real="$candidate"
    break
  fi
done
# The AppImage's own `usr/bin/pdfcraft-nightly`: the GUI sits beside it. That name is the stable
# binary in an installed package, so it counts only when the tree proves it is an AppImage (the
# AppDir's `.DirIcon`, two levels up) or the runtime named the mount.
if [ -z "$real" ] && [ -x "$here/pdfcraft" ] && { [ -n "${APPDIR:-}" ] || [ -e "$here/../../.DirIcon" ]; }; then
  real="$here/pdfcraft"
fi
if [ -z "$real" ]; then
  echo "pdfcraft-nightly: no PdfCraft Nightly binary in or near $here" >&2
  exit 2
fi
if [ -z "${XDG_DATA_HOME:-}" ] && [ -z "${HOME:-}" ]; then
  echo "pdfcraft-nightly: XDG_DATA_HOME and HOME are both unset, so there is no Nightly data folder" >&2
  exit 2
fi
root="${XDG_DATA_HOME:-$HOME/.local/share}/pdfcraft-nightly"
# The Launcher's own command line passes through untouched (it names its profile), and a bare
# version query is answered in the Launcher's form without validating a root; everything else is a
# file, after `--`.
case "${1:-}" in
  --launch-profile | --profile-root) exec "$real" "$@" ;;
  --version | -V) exec "$real" --launch-profile nightly-v1 --profile-root "$root" "$1" ;;
esac
exec "$real" --launch-profile nightly-v1 --profile-root "$root" -- "$@"
