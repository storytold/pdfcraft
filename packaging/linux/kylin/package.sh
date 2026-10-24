#!/usr/bin/env bash
# Build the Kylin-targeted .deb packages for Linux. <osarch> is amd64 or aarch64:
#
#   $DIST/pdfcraft-<version>-kylin-<osarch>.deb          regular package
#   $DIST/pdfcraft-<version>-kylin-<osarch>-offline.deb  + bundled runtime dependency closure
#
# Run this inside a Debian bullseye (glibc 2.31) container, or on a host with that glibc. The
# regular distro packages (packaging/linux/package.sh) are built on Ubuntu 22.04 and declare
# libc6 >= 2.35, which Kylin V20 / Ubuntu Kylin (Ubuntu 20.04 base, glibc 2.31-0kylin), UOS and
# Debian 11 cannot provide. This path builds against glibc 2.31 and packages without the runtime
# library Depends, following the deepin-screenshot packaging
# (https://github.com/himaps/deepin-screenshot, README "离线环境安装" and 踩坑记录).
#
# Usage: packaging/linux/kylin/package.sh [--skip-build]
#
# Needs inside the container:
#   - rustup's cargo (Debian bullseye's rustc 1.63 is far below the workspace rust-version),
#   - git (craft-libs is a pinned git dependency of the workspace), curl, ca-certificates,
#   - build-essential, cmake, perl (aws-lc-sys builds its C from source; on amd64/arm64 it uses
#     its pregenerated bindings, so libclang is not needed — install libclang-dev if bindgen
#     ever asks for it), nasm (unused with the prebuilt-nasm feature, kept as insurance),
#   - dpkg-dev (dpkg-deb) and working apt sources for the offline closure.
#
# In a fresh bullseye container, point the security suite at archive.debian.org first
# (.github/workflows/kylin-deb.yml does this): the bullseye-security index is still served on
# deb.debian.org, but its .debs have been moved to the archive, so `apt-get update` succeeds and
# installs then 404. Only the security suite belongs on the archive; main stays on the fast mirror.
#
# Two hard boundaries, learned on the deepin-screenshot packaging:
#   - glibc can never be bundled or replaced: the target system needs glibc >= 2.31.
#   - dpkg holds its lock for the whole run, so no maintainer script may call `dpkg -i`.
#     The offline postinst only detects missing libraries and schedules install-deps.sh
#     through systemd once dpkg is done.
set -euo pipefail
# shellcheck source=../../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../../env.sh"
HERE="$ROOT/packaging/linux"
APP_ID=ai.storyteller.pdfcraft
PACKAGE=pdfcraft

SKIP_BUILD=0
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    -h | --help) sed -n '2,33p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

ARCH="$(uname -m)"
case "$ARCH" in
  x86_64) OSARCH=amd64; DEB_ARCH=amd64 ;;
  aarch64 | arm64) OSARCH=aarch64; DEB_ARCH=arm64 ;;
  *) echo "unsupported architecture $ARCH (this path targets the amd64/aarch64 Kylin V20 images)" >&2; exit 2 ;;
esac
# A distinct label, so `--version` ("0.7.1 (kylin)") and the file names can't be mistaken for the
# regular distro build, which links a newer glibc.
export PDFCRAFT_BUILD_LABEL="${PDFCRAFT_BUILD_LABEL:-kylin}"
export PDFCRAFT_MAINTAINER="${PDFCRAFT_MAINTAINER:-PdfCraft maintainers <pdfcraft@storyteller.ai>}"
DEB_VERSION="${VERSION}+kylin"
BASENAME="pdfcraft-$VERSION-kylin-$OSARCH"

if [ -z "${CRAFT_FONTS_DIR:-}" ]; then
  warn "CRAFT_FONTS_DIR is not set: the build embeds no Japanese face (CJK interface text then falls back to a system font at run time, as on every build without it)"
fi
for tool in cargo git strip dpkg-deb ldd; do
  command -v "$tool" >/dev/null 2>&1 || { echo "error: $tool not found (see the header of this script)" >&2; exit 1; }
done

echo "==> PdfCraft $VERSION (+kylin) for Linux $OSARCH, glibc 2.31 baseline"

if [ "$SKIP_BUILD" = 0 ]; then
  (cd "$ROOT" && cargo build --release --locked -p pdfcraft -p pdfcraft-cli)
fi
BIN="$CARGO_TARGET_DIR/release"
WORK="$CARGO_TARGET_DIR/kylin-package"
STAGE="$WORK/root"
rm -rf "$WORK"

# ---- stage an FHS tree (mirrors packaging/linux/package.sh; the two differ in the glibc baseline
# and the packaging backend — nfpm with strict Depends there, plain dpkg-deb here — so they stay
# separate on purpose) -----------------------------------------------------------
install -Dm755 "$BIN/pdfcraft" "$STAGE/usr/bin/pdfcraft"
install -Dm755 "$BIN/pdfcraft-cli" "$STAGE/usr/bin/pdfcraft-cli"
strip "$STAGE/usr/bin/pdfcraft" "$STAGE/usr/bin/pdfcraft-cli" 2>/dev/null || true
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
install -Dm644 "$HERE/$APP_ID.mime.xml" "$STAGE/usr/share/mime/packages/$APP_ID.xml"
mkdir -p "$STAGE/usr/share/metainfo"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@DATE@/$PDFCRAFT_BUILD_DATE/g" \
  "$HERE/$APP_ID.metainfo.xml.in" >"$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
mkdir -p "$STAGE/usr/share/icons"
cp -R "$ROOT/assets/app-icon/hicolor" "$STAGE/usr/share/icons/"
mkdir -p "$STAGE/usr/share/doc/pdfcraft"
copy_docs "$STAGE/usr/share/doc/pdfcraft"
# OCR models: the app finds them at <bin>/../share/pdfcraft/models. Offline machines have no
# network to fetch them later, so they are not optional here.
stage_models "$STAGE/usr/share/pdfcraft/models"

if command -v desktop-file-validate >/dev/null 2>&1; then
  desktop-file-validate "$STAGE/usr/share/applications/$APP_ID.desktop"
fi
if command -v appstreamcli >/dev/null 2>&1; then
  appstreamcli validate --no-net --explain "$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
fi

# Only the true ELF requirements are Depends (glibc and libgcc_s, exactly what the binaries link).
# Windowing (X11/Wayland, xkbcommon), the GPU (Vulkan/EGL/GL) and CUPS printing are loaded at run
# time, so — unlike the nfpm deb, which can name them for modern distros — they are not listed
# here: Kylin's and Debian 11's package sets differ too much for strict names to hold, and the
# offline variant carries them instead.
write_control() {
  local dir="$1" flavour="$2"
  mkdir -p "$dir/DEBIAN"
  cat >"$dir/DEBIAN/control" <<EOF
Package: $PACKAGE
Version: $DEB_VERSION
Architecture: $DEB_ARCH
Maintainer: $PDFCRAFT_MAINTAINER
Homepage: https://getartcraft.com/apps/pdfcraft
Section: graphics
Priority: optional
Depends: libc6 (>= 2.31), libgcc-s1
Description: Open-source, native PDF workbench ($flavour, glibc 2.31 build)
 Read, comment, fill and sign forms, organize, combine, split, edit,
 redact, protect and export PDFs. Includes pdfcraft-cli, a headless
 command runner and opt-in MCP server.
 .
 Built inside Debian bullseye for systems whose glibc is older than the
 regular pdfcraft deb requires (2.35): Kylin V20 / Ubuntu Kylin (Ubuntu
 20.04 base), UOS/deepin desktops and Debian 11.
 .
 Only libc6 and libgcc_s are Depends. The X11/Wayland, xkbcommon and
 Vulkan/GL libraries are loaded at run time and a desktop system has
 them; on a minimal or offline system install the -offline deb instead,
 which bundles them.
EOF
}

# ---- regular deb --------------------------------------------------------------------------------
PKGDIR="$WORK/${PACKAGE}_${DEB_VERSION}_${DEB_ARCH}"
mkdir -p "$PKGDIR"
cp -R "$STAGE/." "$PKGDIR/"
write_control "$PKGDIR" "Kylin V20 build"
install -m755 "$HERE/postinst.sh" "$PKGDIR/DEBIAN/postinst"
dpkg-deb --build --root-owner-group "$PKGDIR" "$DIST/$BASENAME.deb"
echo "wrote $DIST/$BASENAME.deb"

# ---- offline deb --------------------------------------------------------------------------------
# Bundle the runtime dependency closure so `dpkg -i` alone leaves a working install on a machine
# without network. The closure is expanded recursively from Depends: target distros package some
# libraries under different names (Kylin V20's libicu66 against Debian's libicu63), differently
# named packages coexist, and a single `ldd` round only catches direct needs.
PKGDIR="$WORK/${PACKAGE}_${DEB_VERSION}_${DEB_ARCH}-offline"
DEPS_DIR="$PKGDIR/usr/share/pdfcraft/deps"
CACHE="$WORK/offline-cache"
mkdir -p "$PKGDIR" "$DEPS_DIR" "$CACHE"

LIBS="$(ldd "$STAGE/usr/bin/pdfcraft" "$STAGE/usr/bin/pdfcraft-cli" | awk '{print $1}' | sort -u)"
echo "linked libraries: $LIBS"
TOP=""
# Package names are plain tokens; word splitting is the point.
# shellcheck disable=SC2086
for lib in $LIBS; do
  # dpkg -S exits 1 for loader-internal names (linux-vdso, the loader itself); || true keeps
  # set -e from treating the lookup as a failure
  p="$(dpkg -S "$lib" 2>/dev/null | head -1 | cut -d: -f1 || true)"
  [ -n "$p" ] && TOP="$TOP $p"
done
# shellcheck disable=SC2086
TOP="$(printf '%s\n' $TOP | sed '/^$/d' | sort -u | tr '\n' ' ')"
if [ -z "${TOP// /}" ]; then
  echo "error: could not map any linked library to a Debian package" >&2
  exit 1
fi
echo "offline deps direct closure:$TOP"

PKGS="$TOP"
for round in 1 2 3 4 5; do
  rm -rf "$CACHE"
  mkdir -p "$CACHE"
  # shellcheck disable=SC2086
  apt-get install --reinstall --download-only --no-install-recommends -y \
    -o Dir::Cache::archives="$CACHE" $PKGS >/dev/null
  MORE=""
  for f in "$CACHE"/*.deb; do
    [ -e "$f" ] || continue
    # One candidate per alternative (drop "pkg (relation version)" and "| alternatives"): the
    # bundle needs at least one provider of each, and the first is the preferred one.
    deps="$(dpkg-deb -f "$f" Depends 2>/dev/null | sed 's/ ([^)]*)//g' | tr ',' '\n' | sed 's/|.*//; s/^ *//; s/ *$//' | sed '/^$/d' || true)"
    MORE="$MORE $deps"
  done
  # shellcheck disable=SC2086
  NEW="$(printf '%s\n' $PKGS $MORE | sed '/^$/d' | sort -u | tr '\n' ' ')"
  # shellcheck disable=SC2086
  CUR="$(printf '%s\n' $PKGS | sed '/^$/d' | sort -u | tr '\n' ' ')"
  if [ "$NEW" = "$CUR" ]; then
    # the closure is a plain token list; word splitting is the point
    # shellcheck disable=SC2086
    echo "closure stable after round $round ($(printf '%s\n' $NEW | sed '/^$/d' | wc -l) packages)"
    break
  fi
  PKGS="$NEW"
  # shellcheck disable=SC2086
  echo "closure round $round grew to $(printf '%s\n' $NEW | sed '/^$/d' | wc -l) packages"
done

COUNT=0
for f in "$CACHE"/*.deb; do
  [ -e "$f" ] || continue
  COUNT=$((COUNT + 1))
  p="$(dpkg-deb -f "$f" Package)"
  # glibc must never be replaced by a bundled copy; Essential packages are on every system.
  [ "$(dpkg-deb -f "$f" Essential)" = "yes" ] && continue
  [ "$p" = "libc6" ] && continue
  cp "$f" "$DEPS_DIR/"
done
if [ "$COUNT" -eq 0 ]; then
  echo "error: the offline closure downloaded no packages — are the apt sources working? (see the header of this script)" >&2
  exit 1
fi
echo "bundled $(find "$DEPS_DIR" -maxdepth 1 -name '*.deb' | wc -l) dependency packages into $DEPS_DIR"

cat >"$PKGDIR/usr/share/pdfcraft/install-deps.sh" <<'SH'
#!/bin/sh
# Install or upgrade the runtime dependencies bundled in ./deps: a package the system already
# has at an equal or newer version is kept (the system copy is used); a missing or older one is
# installed from the bundle. Qt-era ABI worries do not apply here — PdfCraft bundles no Qt —
# but the version check still keeps a newer system library over the bundled one.
set -e
BINS="/usr/bin/pdfcraft /usr/bin/pdfcraft-cli"
DIR="$(cd "$(dirname "$0")" && pwd)/deps"
cd "$DIR"
MISSING=""
for f in *.deb; do
  pkg="$(dpkg-deb -f "$f" Package)"
  ver="$(dpkg-deb -f "$f" Version)"
  stat="$(dpkg-query -W -f='${db:Status-Abbrev}' "$pkg" 2>/dev/null || true)"
  case "$stat" in
    ii*)
      inst="$(dpkg-query -W -f='${Version}' "$pkg" 2>/dev/null || true)"
      if dpkg --compare-versions "${inst:-0}" ge "$ver"; then
        echo "$pkg: system $inst is up to date, using it"
        continue
      fi
      echo "$pkg: system $inst is older, upgrading with $f ($ver)"
      ;;
    *)
      echo "$pkg: missing, will install $f ($ver)"
      ;;
  esac
  MISSING="$MISSING $f"
done
if [ -n "$MISSING" ]; then
  # a single dpkg call, so the packages configure in dependency order
  # shellcheck disable=SC2086
  dpkg -i $MISSING
else
  echo "nothing to install"
fi
echo ""
STILL=""
for b in $BINS; do
  [ -f "$b" ] || continue
  STILL="$STILL $(ldd "$b" 2>/dev/null | awk '/not found/ {print $1}')"
done
# shellcheck disable=SC2086
if [ -n "$(printf '%s' $STILL | tr -d '[:space:]')" ]; then
  echo "WARNING: these libraries are still missing, the app will not start:"
  # shellcheck disable=SC2086
  printf '%s\n' $STILL | sed '/^$/d'
  exit 1
fi
echo "OK: all runtime libraries resolved. pdfcraft is ready."
SH
chmod 755 "$PKGDIR/usr/share/pdfcraft/install-deps.sh"

cat >"$PKGDIR/DEBIAN/postinst" <<'EOF'
#!/bin/sh
# dpkg holds its lock while this script runs, so the bundled dependency debs cannot be installed
# here (a nested dpkg -i fails on the lock). Refresh the desktop caches, detect missing runtime
# libraries, and schedule the installer through systemd once dpkg is done.
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q /usr/share/applications || true
fi
if command -v update-mime-database >/dev/null 2>&1; then
  update-mime-database /usr/share/mime || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
MISSING=""
for b in /usr/bin/pdfcraft /usr/bin/pdfcraft-cli; do
  [ -f "$b" ] || continue
  MISSING="$MISSING $(ldd "$b" 2>/dev/null | awk '/not found/ {print $1}')"
done
if [ -z "$(printf '%s' "$MISSING" | tr -d '[:space:]')" ]; then
  echo "pdfcraft: all runtime libraries present, nothing to do."
  exit 0
fi
echo "pdfcraft: missing runtime libraries:"
printf '%s\n' $MISSING | sed '/^$/d; s/^/  /'
if command -v systemd-run >/dev/null 2>&1; then
  echo "pdfcraft: auto-installing them from the bundled offline deps in 10s"
  echo "  (same as running: sudo /usr/share/pdfcraft/install-deps.sh)"
  echo "  (result: journalctl -u pdfcraft-deps)"
  systemd-run --on-active=10 --quiet --unit=pdfcraft-deps \
    /usr/share/pdfcraft/install-deps.sh 2>/dev/null \
    || echo "pdfcraft: auto-install could not be scheduled; run the command above manually."
else
  echo "pdfcraft: run this once to install them from the bundled offline deps:"
  echo "  sudo /usr/share/pdfcraft/install-deps.sh"
fi
exit 0
EOF
chmod 755 "$PKGDIR/DEBIAN/postinst"
write_control "$PKGDIR" "Kylin V20 offline build"
dpkg-deb --build --root-owner-group "$PKGDIR" "$DIST/$BASENAME-offline.deb"
echo "wrote $DIST/$BASENAME-offline.deb"

"$STAGE/usr/bin/pdfcraft-cli" --version
echo "==> done"
ls -lh "$DIST"
