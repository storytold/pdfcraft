#!/usr/bin/env bash
# Validate the staged Arch launcher without building or opening PdfCraft.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
stage="$work/source"
dst="$work/dist"
base=pdfcraft-0.0.1-nightly.1-linux-x86_64
mkdir -p "$stage/bin" "$dst" "$work/extract"

# A local argv recorder stands in for the compiled binaries. It never opens a UI.
cat >"$stage/bin/pdfcraft" <<'RECORDER'
#!/bin/sh
printf '<%s>\n' "$@"
RECORDER
chmod 755 "$stage/bin/pdfcraft"
cp "$stage/bin/pdfcraft" "$stage/bin/pdfcraft-cli"

bash "$HERE/nightly-arch-tar.sh" "$stage" "$work/stage" "$dst" "$base"
tar -xzf "$dst/$base.tar.gz" -C "$work/extract"
bin="$work/extract/$base/bin"
# Simulate the existing Arch bin-tree copy and public launcher rename.
mkdir -p "$work/installed"
cp -R "$bin" "$work/installed/bin"
bin="$work/installed/bin"
mv "$bin/pdfcraft" "$bin/pdfcraft-nightly"
mv "$bin/pdfcraft-cli" "$bin/pdfcraft-nightly-cli"

[[ ! -e "$stage/bin/.pdfcraft-nightly" ]]
cmp "$stage/bin/pdfcraft" "$bin/.pdfcraft-nightly/pdfcraft"
cmp "$stage/bin/pdfcraft-cli" "$bin/pdfcraft-nightly-cli"
root="${XDG_DATA_HOME:-$HOME/.local/share}/pdfcraft-nightly"
# The recorder prints one `<arg>` per line; command substitution strips the last newline, so this
# joins the same way.
lines() {
  printf '<%s>' "$1"
  shift
  for arg in "$@"; do printf '\n<%s>' "$arg"; done
}
expect() {
  expected="$1"
  shift
  actual=$(PATH="$bin:$PATH" pdfcraft-nightly "$@")
  if [[ "$actual" != "$expected" ]]; then
    printf 'expected:\n%s\nactual:\n%s\n' "$expected" "$actual" >&2
    exit 1
  fi
}
# The launcher starts the GUI through its launch profile (files after `--`); a version query keeps
# the Launcher's own form, without the delimiter.
expect "$(lines --launch-profile nightly-v1 --profile-root "$root" --)"
expect "$(lines --launch-profile nightly-v1 --profile-root "$root" -- 'document with spaces.pdf' '--other-name.pdf')" \
  'document with spaces.pdf' '--other-name.pdf'
expect "$(lines --launch-profile nightly-v1 --profile-root "$root" --version)" --version
expect "$(lines --launch-profile nightly-v1 --profile-root "$root" -V)" -V

echo "PASS: Arch Nightly handoff runs the GUI through its launch profile"
