#!/usr/bin/env bash
# Both OFL fonts and Apache-licensed fallback fonts must keep their licence texts.
set -euo pipefail
FONT_TEST_DIR=$(mktemp -d)
trap 'rm -rf "$FONT_TEST_DIR"' EXIT
mkdir -p "$FONT_TEST_DIR/fonts/japanese" "$FONT_TEST_DIR/fonts/chinese" "$FONT_TEST_DIR/package"
printf 'OFL test licence\n' > "$FONT_TEST_DIR/fonts/japanese/OFL.txt"
printf 'Apache test notice\n' > "$FONT_TEST_DIR/fonts/chinese/NOTICE.txt"
export CRAFT_FONTS_DIR="$FONT_TEST_DIR" DIST="$FONT_TEST_DIR/dist"
. "$(dirname "$0")/../env.sh"
copy_font_licences "$FONT_TEST_DIR/package"
cmp "$FONT_TEST_DIR/fonts/japanese/OFL.txt" "$FONT_TEST_DIR/package/OFL-japanese.txt"
cmp "$FONT_TEST_DIR/fonts/chinese/NOTICE.txt" "$FONT_TEST_DIR/package/NOTICE-chinese.txt"
