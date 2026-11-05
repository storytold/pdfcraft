#!/usr/bin/env bash
# Tiny real productbuild fixture; no installation, daemon, signing identity or app build.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="${1:?usage: test-localized-pkg.sh NEW_SCRATCH_DIRECTORY}"
if [ -e "$work" ]; then echo "test directory already exists: $work" >&2; exit 1; fi
mkdir -p "$work/components" "$work/empty"
for component in PdfCraft HotFolder; do
  identifier=ai.storyteller.pdfcraft.pkg
  [ "$component" = HotFolder ] && identifier=ai.storyteller.pdfcraft.hot-folder.pkg
  pkgbuild --root "$work/empty" --identifier "$identifier" --version 0.6.0 "$work/components/$component-component.pkg"
done
sed 's/@SHORT_VERSION@/0.6.0/g' "$here/Distribution.xml.in" > "$work/Distribution.xml"
bash "$here/prepare-resources.sh" "$work/resources"
productbuild --distribution "$work/Distribution.xml" --package-path "$work/components" --resources "$work/resources" "$work/localized.pkg"
pkgutil --expand "$work/localized.pkg" "$work/expanded"
for source in "$here/resources/"*.lproj; do
  cmp "$source/Localizable.strings" "$work/expanded/Resources/$(basename "$source")/Localizable.strings"
done
osascript "$here/test-localization.applescript" "$work/expanded/Resources"
# The default-off service choice must survive productbuild, not merely source parsing.
installer -showChoicesXML -pkg "$work/localized.pkg" -target / > "$work/choices.plist"
plutil -lint "$work/choices.plist"
python3 - "$work/choices.plist" <<'PY'
import plistlib, sys
with open(sys.argv[1], 'rb') as f:
    choices = plistlib.load(f)
def walk(items):
    for item in items:
        yield item
        yield from walk(item.get('childItems', []))
hot = [x for x in walk(choices) if x.get('choiceIdentifier') == 'hot-folder']
assert len(hot) == 1 and not hot[0]['choiceIsSelected'], hot
PY
