#!/usr/bin/env bash
# The Nightly Linux launcher (packaging/linux/nightly-launch.sh): every format's layout finds the
# GUI, a launch adds the Launcher's private profile, the Launcher's own command line and a version
# query pass through, and a tree without the GUI refuses instead of starting something else.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# An argv recorder stands in for the GUI: it prints one `<arg>` per line and opens no window.
recorder() {
  local path="$1"
  mkdir -p "$(dirname "$path")"
  cat >"$path" <<'RECORDER'
#!/bin/sh
printf '<%s>\n' "$@"
RECORDER
  chmod 755 "$path"
}

profile="${XDG_DATA_HOME:-$HOME/.local/share}/pdfcraft-nightly"
# The app's fence accepts this root: absolute, no parent traversal, and outside every unpacked copy.
case "$profile" in
  /*) ;;
  *) echo "FAIL: the launcher profile is not absolute: $profile" >&2; exit 1 ;;
esac
case "/$profile/" in
  */../*) echo "FAIL: the launcher profile contains parent traversal" >&2; exit 1 ;;
esac

expect() {
  local launcher="$1" expected="$2"
  shift 2
  local actual
  actual=$("$launcher" "$@")
  if [[ "$actual" != "$expected" ]]; then
    printf 'FAIL: %s\nexpected:\n%s\nactual:\n%s\n' "$launcher" "$expected" "$actual" >&2
    exit 1
  fi
}

# The whole argument list, one `<arg>` per line, as the recorder prints it.
lines() {
  printf '<%s>' "$1"
  shift
  for arg in "$@"; do printf '\n<%s>' "$arg"; done
  printf '\n'
}

launched() {
  local root="$1"
  shift
  lines --launch-profile nightly-v1 --profile-root "$root" -- "$@"
}

queried() {
  lines --launch-profile nightly-v1 --profile-root "$1" "$2"
}

# 1. The plain tarball's standard layout: `pdfcraft` is this launcher, `pdfcraft-bin` the GUI.
tar_tree="$work/tar"
recorder "$tar_tree/pdfcraft-bin"
install -m755 "$HERE/nightly-launch.sh" "$tar_tree/pdfcraft"
expect "$tar_tree/pdfcraft" "$(launched "$profile")"
expect "$tar_tree/pdfcraft" "$(launched "$profile" 'document with spaces.pdf' '--other-name.pdf')" \
  'document with spaces.pdf' '--other-name.pdf'
# A version query goes in the Launcher's form (`--` would make it a file name); the Launcher's own
# command line, which already names its profile, is not rewritten.
expect "$tar_tree/pdfcraft" "$(queried "$profile" --version)" --version
expect "$tar_tree/pdfcraft" "$(queried "$profile" -V)" -V
expect "$tar_tree/pdfcraft" "$(lines --launch-profile nightly-v1 --profile-root /launcher/profile --version)" \
  --launch-profile nightly-v1 --profile-root /launcher/profile --version
expect "$tar_tree/pdfcraft" "$(lines --launch-profile nightly-v1 --profile-root /launcher/profile -- a.pdf)" \
  --launch-profile nightly-v1 --profile-root /launcher/profile -- a.pdf

# 2. The deb/rpm: /usr/bin/pdfcraft-nightly starts the GUI in /usr/lib/pdfcraft-nightly.
deb_tree="$work/deb"
recorder "$deb_tree/usr/lib/pdfcraft-nightly/pdfcraft"
install -Dm755 "$HERE/nightly-launch.sh" "$deb_tree/usr/bin/pdfcraft-nightly"
expect "$deb_tree/usr/bin/pdfcraft-nightly" "$(launched "$profile")"
expect "$deb_tree/usr/bin/pdfcraft-nightly" "$(launched "$profile" a.pdf)" a.pdf

# 3. The AppImage: AppRun at the AppDir root and usr/bin/pdfcraft-nightly beside usr/bin/pdfcraft.
appdir="$work/AppDir"
recorder "$appdir/usr/bin/pdfcraft"
# appimagetool links .DirIcon at the AppDir root; it is what tells this tree from an installed
# package, where usr/bin/pdfcraft is the stable binary.
: >"$appdir/.DirIcon"
install -m755 "$HERE/nightly-launch.sh" "$appdir/AppRun"
install -Dm755 "$HERE/nightly-launch.sh" "$appdir/usr/bin/pdfcraft-nightly"
expect "$appdir/AppRun" "$(launched "$profile" a.pdf)" a.pdf
expect "$appdir/usr/bin/pdfcraft-nightly" "$(launched "$profile" a.pdf)" a.pdf

# 4. The Arch handoff: bin/pdfcraft with the GUI in bin/.pdfcraft-nightly, renamed by the packager.
arch_tree="$work/arch/bin"
recorder "$arch_tree/.pdfcraft-nightly/pdfcraft"
install -m755 "$HERE/nightly-launch.sh" "$arch_tree/pdfcraft"
mv "$arch_tree/pdfcraft" "$arch_tree/pdfcraft-nightly"
expect "$arch_tree/pdfcraft-nightly" "$(launched "$profile" a.pdf)" a.pdf

# 5. An installed tree (no AppDir marker) never runs a sibling `pdfcraft`, which is stable's binary.
installed="$work/installed/usr/bin"
recorder "$installed/pdfcraft"
install -m755 "$HERE/nightly-launch.sh" "$installed/pdfcraft-nightly"
if "$installed/pdfcraft-nightly" a.pdf >/dev/null 2>&1; then
  echo "FAIL: a package launcher ran the binary beside it" >&2
  exit 1
fi

# 6. A tree whose GUI is missing refuses, and never falls back to another binary.
mkdir -p "$work/empty"
install -m755 "$HERE/nightly-launch.sh" "$work/empty/pdfcraft"
if "$work/empty/pdfcraft" a.pdf >/dev/null 2>&1; then
  echo "FAIL: a launcher with no GUI started something" >&2
  exit 1
fi

# 7. Neither data-folder base: refuse rather than write somewhere unknown.
redirected=$(env -u XDG_DATA_HOME -u HOME "$tar_tree/pdfcraft" a.pdf 2>&1) && {
  printf 'FAIL: a launcher without XDG_DATA_HOME or HOME started: %s\n' "$redirected" >&2
  exit 1
}

echo "PASS: the Nightly Linux launcher starts the GUI with the Launcher profile in every format"
