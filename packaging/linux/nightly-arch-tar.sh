#!/usr/bin/env bash
# Stage the PdfCraft Nightly-only handoff for the Arch packager.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
source_tree="$1"
work="$2"
destination="$3"
basename="$4"
stage="$work/$basename"

mkdir -p "$work"
cp -R "$source_tree" "$stage"
mkdir "$stage/bin/.pdfcraft-nightly"
mv "$stage/bin/pdfcraft" "$stage/bin/.pdfcraft-nightly/pdfcraft"
cp "$here/nightly-arch-launch.sh" "$stage/bin/pdfcraft"
chmod 755 "$stage/bin/pdfcraft"
tar -C "$work" -czf "$destination/$basename.tar.gz" "$basename"
echo "wrote $destination/$basename.tar.gz"
