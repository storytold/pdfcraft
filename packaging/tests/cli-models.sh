#!/usr/bin/env bash
# Run the actual release scripts without compiling, fetching models or signing anything.
# The ZIP/tar tools are real: model payloads, licences and credits must survive archive extraction.
set -euo pipefail

PLATFORM="${1:-all}"
case "$PLATFORM" in all | linux | macos) ;; *) echo "unknown platform: $PLATFORM" >&2; exit 2 ;; esac
if [ "$PLATFORM" = macos ] && ! command -v ditto >/dev/null 2>&1; then
  echo "macOS archive regression requires ditto" >&2
  exit 2
fi
SOURCE_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pdfcraft cli archives.XXXXXX")"
trap 'rm -rf "$TEST_DIR"' EXIT
FIXTURE="$TEST_DIR/source"
SHIMS="$TEST_DIR/shims"
MODELS="$TEST_DIR/model inputs"
mkdir -p "$FIXTURE/packaging/macos/dmg" "$FIXTURE/packaging/linux" "$SHIMS" "$MODELS"
cp "$SOURCE_ROOT/packaging/env.sh" "$FIXTURE/packaging/"
cp "$SOURCE_ROOT/packaging/macos/package.sh" "$SOURCE_ROOT/packaging/macos/Info.plist.in" "$FIXTURE/packaging/macos/"
cp "$SOURCE_ROOT/packaging/linux/package.sh" "$SOURCE_ROOT/packaging/linux/ai.storyteller.pdfcraft."* "$FIXTURE/packaging/linux/"
printf '[workspace.package]\nversion = "0.0.0"\n' >"$FIXTURE/Cargo.toml"
printf 'CLI archive fixture\n' >"$FIXTURE/README.md"
printf 'Original synthetic test inputs, MIT OR Apache-2.0\n' >"$FIXTURE/LICENSE-MIT"
mkdir -p "$FIXTURE/assets/app-icon/hicolor"
printf 'synthetic icon\n' >"$FIXTURE/assets/app-icon/pdfcraft.icns"
printf 'synthetic background\n' >"$FIXTURE/packaging/macos/dmg/background.tiff"
printf 'synthetic layout\n' >"$FIXTURE/packaging/macos/dmg/dmg-layout.DS_Store"
for model in text-detection.rten text-recognition.rten future-recognition.bin; do
  printf 'synthetic model payload: %s\n' "$model" >"$MODELS/$model"
  printf 'synthetic licence notice: %s\n' "$model" >"$MODELS/$model.LICENCE.txt"
done
printf 'original test model credits\n' >"$MODELS/ATTRIBUTION.txt"

# Only the model-fetch boundary is mocked; stage_models and the package assembly stay real.
cat >"$SHIMS/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[ "$#" = 3 ] && [ "$1" = xtask ] && [ "$2" = models ] || exit 91
cp -R "$TEST_MODELS/." "$3/"
EOF
cat >"$SHIMS/install" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[ "$#" = 3 ] || exit 92
mkdir -p "$(dirname "$3")"
cp "$2" "$3"
chmod "${1#-Dm}" "$3"
EOF
cat >"$SHIMS/uname" <<'EOF'
#!/usr/bin/env bash
printf 'x86_64\n'
EOF
cat >"$SHIMS/lipo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  -info) exit 0 ;;
  -create) cp "$4" "$3" ;;
  *) exit 93 ;;
esac
EOF
cat >"$SHIMS/hdiutil" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  create) touch "${@: -1}" ;;
  attach)
    while [ "$1" != -mountpoint ]; do shift; done
    mkdir -p "$2"
    cp -R "$CARGO_TARGET_DIR/macos-package/PdfCraft.app" "$2/"
    ;;
  detach) : ;;
  *) exit 94 ;;
esac
EOF
for tool in strip codesign plutil desktop-file-validate appstreamcli; do
  printf '#!/usr/bin/env bash\nexit 0\n' >"$SHIMS/$tool"
done
chmod +x "$SHIMS/"*

export PATH="$SHIMS:$PATH" TEST_MODELS="$MODELS"
export PDFCRAFT_VERSION=0.0.0 PDFCRAFT_BUILD_SHA=fixture PDFCRAFT_BUILD_DATE=2026-01-01
export MACOS_SIGN_IDENTITY=- APPLE_ID= APPLE_PASSWORD= APPLE_TEAM_ID=
unset CRAFT_FONTS_DIR

make_binary() {
  mkdir -p "$(dirname "$1")"
  printf '#!/usr/bin/env bash\nprintf "PdfCraft fixture\\n"\n' >"$1"
  chmod +x "$1"
}

check_archive() {
  local unpacked="$1" file
  [ -x "$unpacked/pdfcraft-cli" ]
  [ -f "$unpacked/README.md" ] && [ -f "$unpacked/LICENSE-MIT" ]
  for file in "$MODELS/"*; do
    cmp "$file" "$unpacked/models/$(basename "$file")"
  done
}

if [ "$PLATFORM" != macos ]; then
  export CARGO_TARGET_DIR="$TEST_DIR/linux target" DIST="$TEST_DIR/linux dist"
  make_binary "$CARGO_TARGET_DIR/release/pdfcraft"
  make_binary "$CARGO_TARGET_DIR/release/pdfcraft-cli"
  bash "$FIXTURE/packaging/linux/package.sh" --skip-build --formats cli
  mkdir -p "$TEST_DIR/linux unpacked"
  tar -xzf "$DIST/pdfcraft-cli-0.0.0-linux-x86_64.tar.gz" -C "$TEST_DIR/linux unpacked"
  check_archive "$TEST_DIR/linux unpacked/pdfcraft-cli-0.0.0-linux-x86_64"
fi

# ditto is macOS's real release ZIP writer. Linux/FreeBSD still run the tarball regression.
if [ "$PLATFORM" != linux ] && command -v ditto >/dev/null 2>&1; then
  export CARGO_TARGET_DIR="$TEST_DIR/macos target" DIST="$TEST_DIR/macos dist"
  make_binary "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/pdfcraft"
  make_binary "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/pdfcraft-cli"
  bash "$FIXTURE/packaging/macos/package.sh" --skip-build --arch aarch64
  ditto -x -k "$DIST/pdfcraft-cli-0.0.0-macos-aarch64.zip" "$TEST_DIR/macos unpacked"
  check_archive "$TEST_DIR/macos unpacked/pdfcraft-cli-0.0.0-macos-aarch64"
fi
