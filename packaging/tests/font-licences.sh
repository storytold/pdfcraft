#!/usr/bin/env bash
# Every manifest licence reaches the package, regardless of its upstream filename.
set -euo pipefail
FONT_TEST_DIR=$(mktemp -d)
trap 'rm -rf "$FONT_TEST_DIR"' EXIT
mkdir -p "$FONT_TEST_DIR/fonts/japanese" "$FONT_TEST_DIR/fonts/chinese" "$FONT_TEST_DIR/package"
printf 'OFL test licence\n' > "$FONT_TEST_DIR/fonts/japanese/OFL.txt"
printf 'Apache test notice\n' > "$FONT_TEST_DIR/fonts/chinese/NOTICE"
cat > "$FONT_TEST_DIR/fonts/manifest.txt" <<'MANIFEST'
# family | style | file | scripts | licence | licence file | sha256 | source
Japanese | Regular | fonts/japanese/font.ttf | Jpan | OFL-1.1 | fonts/japanese/OFL.txt | unused | unused
Chinese | Regular | fonts/chinese/font.ttf | Hans,Hant | Apache-2.0 | fonts/chinese/NOTICE | unused | unused
MANIFEST
export CRAFT_FONTS_DIR="$FONT_TEST_DIR" DIST="$FONT_TEST_DIR/dist"
. "$(dirname "$0")/../env.sh"
copy_font_licences "$FONT_TEST_DIR/package"
cmp "$FONT_TEST_DIR/fonts/japanese/OFL.txt" "$FONT_TEST_DIR/package/OFL-japanese.txt"
cmp "$FONT_TEST_DIR/fonts/chinese/NOTICE" "$FONT_TEST_DIR/package/NOTICE-chinese.txt"
# An older checkout using NOTICE.txt produces the same package filename.
mv "$FONT_TEST_DIR/fonts/chinese/NOTICE" "$FONT_TEST_DIR/fonts/chinese/NOTICE.txt"
sed 's@fonts/chinese/NOTICE |@fonts/chinese/NOTICE.txt |@' "$FONT_TEST_DIR/fonts/manifest.txt" > "$FONT_TEST_DIR/manifest"
mv "$FONT_TEST_DIR/manifest" "$FONT_TEST_DIR/fonts/manifest.txt"
copy_font_licences "$FONT_TEST_DIR/package"
cmp "$FONT_TEST_DIR/fonts/chinese/NOTICE.txt" "$FONT_TEST_DIR/package/NOTICE-chinese.txt"
rm "$FONT_TEST_DIR/fonts/chinese/NOTICE.txt"
if copy_font_licences "$FONT_TEST_DIR/package" 2>/dev/null; then
  echo 'missing font licence was silently omitted' >&2
  exit 1
fi
printf 'malformed manifest row\n' > "$FONT_TEST_DIR/fonts/manifest.txt"
if copy_font_licences "$FONT_TEST_DIR/package" 2>/dev/null; then
  echo 'malformed manifest was silently accepted' >&2
  exit 1
fi
unset CRAFT_FONTS_DIR
copy_font_licences "$FONT_TEST_DIR/package"
