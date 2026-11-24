#!/usr/bin/env bash
# Build and package PdfCraft for Linux (<arch> is x86_64 or aarch64, or riscv64 when cross-compiling):
#
#   $DIST/pdfcraft-<version>-linux-<arch>.AppImage  any distro with glibc >= the build host's
#   $DIST/pdfcraft-<version>-linux-<arch>.AppImage.zsync  delta updates (needs zsyncmake)
#   $DIST/pdfcraft-<version>-linux-<arch>.deb       Debian, Ubuntu, Mint, Pop!_OS, ...
#   $DIST/pdfcraft-<version>-linux-<arch>.rpm       Fedora, openSUSE, RHEL, ...
#   $DIST/pdfcraft-<version>-linux-<arch>.tar.gz    plain FHS-style tree (bin/, share/)
#   $DIST/pdfcraft-cli-<version>-linux-<arch>.tar.gz  the headless CLI alone (servers, CI, agents)
#
# A local or patched build: set PDFCRAFT_BUILD_LABEL (e.g. `titlebar`). `--version` and About then
# show `0.7.1 (titlebar)`, file names carry `-titlebar`, and the .rpm/.deb release becomes
# `1.titlebar`, which upgrades over the release of the same version (and the next release over it).
#
# Cross-compiling (CI: riscv64, tar only): set CROSS_ARCH (riscv64), CROSS_TARGET (the Rust target),
# CROSS_COMPILE (binutils prefix, for strip) and optionally EMULATOR (e.g. qemu-riscv64) to
# smoke-test the CLI.
#
# Usage: packaging/linux/package.sh [--skip-build] [--formats "appimage deb rpm tar cli"] [--arch-tar-dir DIRECTORY]
#
# Needs: cargo, curl and the network (the OCR models: cargo xtask models); nfpm for deb/rpm (https://nfpm.goreleaser.com); appimagetool for the AppImage
# (downloaded into $CARGO_TARGET_DIR if missing). Build on an old distro (CI: Ubuntu 22.04,
# glibc 2.35) so the binaries run on newer ones. Optional: desktop-file-validate, appstreamcli,
# zsyncmake (the zsync package) for the AppImage's .zsync.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/linux"
APP_ID=ai.storyteller.pdfcraft

SKIP_BUILD=0
FORMATS="appimage deb rpm tar cli"
ARCH_TAR_DIR=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --formats) FORMATS="$2"; shift 2 ;;
    --arch-tar-dir)
      [[ -n "${2:-}" ]] || { echo "error: --arch-tar-dir needs a directory" >&2; exit 2; }
      ARCH_TAR_DIR="$2"
      shift 2
      ;;
    -h | --help) sed -n '2,24p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

ARCH="${CROSS_ARCH:-$(uname -m)}"
case "$ARCH" in
  riscv64) DEB_ARCH=riscv64 ;;
  x86_64) DEB_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64; DEB_ARCH=arm64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
export PDFCRAFT_MAINTAINER="${PDFCRAFT_MAINTAINER:-PdfCraft maintainers <pdfcraft@storyteller.ai>}"
IS_NIGHTLY=0
if [[ "${PDFCRAFT_BUILD_CHANNEL:-}" == nightly-v1 || "$VERSION" == *-nightly* ]]; then
  IS_NIGHTLY=1
  export PDFCRAFT_BUILD_CHANNEL=nightly-v1
  export PDFCRAFT_OVERRIDE_VERSION="$VERSION"
fi
LABEL_SUFFIX="${PDFCRAFT_BUILD_LABEL:+-$PDFCRAFT_BUILD_LABEL}"
BASENAME="pdfcraft-$VERSION$LABEL_SUFFIX-linux-$ARCH"

if [[ -n "$ARCH_TAR_DIR" && "$IS_NIGHTLY" != 1 ]]; then
  echo "error: --arch-tar-dir is only for Nightly builds" >&2
  exit 2
fi

echo "==> PdfCraft $VERSION$LABEL_SUFFIX for Linux $ARCH ($FORMATS, channel=${PDFCRAFT_BUILD_CHANNEL:-stable})"

if [[ "$SKIP_BUILD" = 0 ]]; then
  if [[ -n "${CROSS_TARGET:-}" ]]; then
    (cd "$ROOT" && cargo build --release --locked -p pdfcraft -p pdfcraft-cli --target "$CROSS_TARGET")
  else
    (cd "$ROOT" && cargo build --release --locked -p pdfcraft -p pdfcraft-cli)
  fi
fi
BIN="$CARGO_TARGET_DIR/${CROSS_TARGET:+$CROSS_TARGET/}release"
WORK="$CARGO_TARGET_DIR/linux-package"
STAGE="$WORK/root"
rm -rf "$WORK"

# ---- stage an FHS tree (shared by every format) -------------------------------------------------
install -Dm755 "$BIN/pdfcraft" "$STAGE/usr/bin/pdfcraft"
install -Dm755 "$BIN/pdfcraft-cli" "$STAGE/usr/bin/pdfcraft-cli"
"${CROSS_COMPILE:-}strip" "$STAGE/usr/bin/pdfcraft" "$STAGE/usr/bin/pdfcraft-cli" 2>/dev/null || true
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
if [[ "$IS_NIGHTLY" = 1 ]]; then
  sed -e 's|^StartupWMClass=.*|StartupWMClass=ai.storyteller.pdfcraft.nightly-v1|' \
      "$STAGE/usr/share/applications/$APP_ID.desktop" >"$STAGE/usr/share/applications/$APP_ID.desktop.tmp"
  mv "$STAGE/usr/share/applications/$APP_ID.desktop.tmp" "$STAGE/usr/share/applications/$APP_ID.desktop"
fi
install -Dm644 "$HERE/$APP_ID.mime.xml" "$STAGE/usr/share/mime/packages/$APP_ID.xml"
mkdir -p "$STAGE/usr/share/metainfo"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@DATE@/$PDFCRAFT_BUILD_DATE/g" \
  "$HERE/$APP_ID.metainfo.xml.in" >"$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
mkdir -p "$STAGE/usr/share/icons"
cp -R "$ROOT/assets/app-icon/hicolor" "$STAGE/usr/share/icons/"
mkdir -p "$STAGE/usr/share/doc/pdfcraft"
copy_docs "$STAGE/usr/share/doc/pdfcraft"
# OCR models: the app finds them at <bin>/../share/pdfcraft/models, so in every format below.
stage_models "$STAGE/usr/share/pdfcraft/models"

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$STAGE/usr/share/applications/$APP_ID.desktop"
fi
if command -v appstreamcli >/dev/null; then
  appstreamcli validate --no-net --explain "$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
fi

has() {
  local format="$1"
  case " $FORMATS " in *" $format "*) return 0 ;; *) return 1 ;; esac
}

verify_sha256() {
  local expected="$1"
  local target="$2"
  printf '%s  %s\n' "$expected" "$target" | sha256sum -c -
}

# A launcher that starts the Nightly GUI through its launch profile, so no Linux format ever runs
# it against the stable profile (the app refuses a Nightly build without a profile). `real_bin` is
# where the GUI is installed; the profile root is the Nightly data folder in the user's data
# directory, which the app validates like the Launcher's.
nightly_launcher() {
  local destination="$1" real_bin="$2"
  install -d "$(dirname "$destination")"
  cat >"$destination" <<LAUNCHER
#!/bin/sh
# PdfCraft Nightly. Its own data root: the stable app's settings, recovery and socket are never touched.
set -eu
if [ -z "\${XDG_DATA_HOME:-}" ] && [ -z "\${HOME:-}" ]; then
  echo "pdfcraft-nightly: XDG_DATA_HOME and HOME are both unset, so there is no Nightly data folder" >&2
  exit 2
fi
root="\${XDG_DATA_HOME:-\$HOME/.local/share}/pdfcraft-nightly"
case "\${1:-}" in
  --version | -V) exec "$real_bin" --launch-profile nightly-v1 --profile-root "$root" "$1" ;;
esac
exec "$real_bin" --launch-profile nightly-v1 --profile-root "$root" -- "\$@"
LAUNCHER
  chmod 755 "$destination"
}

# The same launcher for a moved tree (an AppImage, an unpacked tar): it finds the GUI beside
# itself or in usr/bin, so one script serves AppRun, usr/bin/pdfcraft-nightly and the tar's copy.
nightly_relative_launcher() {
  local destination="$1"
  install -d "$(dirname "$destination")"
  cat >"$destination" <<'LAUNCHER'
#!/bin/sh
# PdfCraft Nightly. Its own data root: the stable app's settings, recovery and socket are never touched.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
real="$here/usr/bin/pdfcraft"
[ -x "$real" ] || real="$here/pdfcraft"
if [ -z "${XDG_DATA_HOME:-}" ] && [ -z "${HOME:-}" ]; then
  echo "pdfcraft-nightly: XDG_DATA_HOME and HOME are both unset, so there is no Nightly data folder" >&2
  exit 2
fi
root="${XDG_DATA_HOME:-$HOME/.local/share}/pdfcraft-nightly"
# A version query is answered in the Launcher's form; everything else is a file, after `--`.
case "${1:-}" in
  --version | -V) exec "$real" --launch-profile nightly-v1 --profile-root "$root" "$1" ;;
esac
exec "$real" --launch-profile nightly-v1 --profile-root "$root" -- "$@"
LAUNCHER
  chmod 755 "$destination"
}

# ---- .tar.gz ------------------------------------------------------------------------------------
if has tar; then
  mkdir -p "$WORK/tar"
  cp -R "$STAGE/usr" "$WORK/tar/$BASENAME"
  tar -C "$WORK/tar" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
  echo "wrote $DIST/$BASENAME.tar.gz"
fi

# The Arch handoff keeps the same basename as the plain tarball in a separate directory.
# Flatpak, OCI and direct tar consumers still get the original binary, not this launcher.
if [[ -n "$ARCH_TAR_DIR" ]]; then
  mkdir -p "$ARCH_TAR_DIR"
  if [[ "$(cd "$ARCH_TAR_DIR" && pwd -P)" == "$(cd "$DIST" && pwd -P)" ]]; then
    echo "error: the Arch tar directory must differ from the release directory" >&2
    exit 2
  fi
  bash "$HERE/nightly-arch-tar.sh" "$STAGE/usr" "$WORK/arch-tar" "$ARCH_TAR_DIR" "$BASENAME"
fi

# ---- CLI-only .tar.gz ---------------------------------------------------------------------------
# The stripped pdfcraft-cli (and its opt-in MCP server) with the licences, for machines that never
# open a window. Like the other formats it needs glibc >= the build host's.
if has cli; then
  CLI_NAME="pdfcraft-cli-$VERSION$LABEL_SUFFIX-linux-$ARCH"
  CLI_DIR="$WORK/cli/$CLI_NAME"
  mkdir -p "$CLI_DIR"
  cp "$STAGE/usr/bin/pdfcraft-cli" "$CLI_DIR/"
  copy_docs "$CLI_DIR"
  tar -C "$WORK/cli" -czf "$DIST/$CLI_NAME.tar.gz" "$CLI_NAME"
  echo "wrote $DIST/$CLI_NAME.tar.gz"
fi

# ---- .deb / .rpm --------------------------------------------------------------------------------
if has deb || has rpm; then
  command -v nfpm >/dev/null || { echo "error: nfpm not found (https://nfpm.goreleaser.com/docs/install/)" >&2; exit 1; }
  export VERSION
  export RELEASE="1${PDFCRAFT_BUILD_LABEL:+.$PDFCRAFT_BUILD_LABEL}"
  export NFPM_ARCH="$DEB_ARCH"
  NFPM_STAGE="$STAGE"
  export NFPM_NAME="pdfcraft"
  export NFPM_BIN="pdfcraft"
  export NFPM_CLI="pdfcraft-cli"
  if [[ "$IS_NIGHTLY" = 1 ]]; then
    NIGHTLY_DESK_ID="$APP_ID.nightly-v1"
    export NFPM_NAME="pdfcraft-nightly"
    export NFPM_BIN="pdfcraft-nightly"
    export NFPM_CLI="pdfcraft-nightly-cli"
    NFPM_STAGE="$WORK/nfpm-root"
    rm -rf "$NFPM_STAGE"
    cp -R "$STAGE" "$NFPM_STAGE"
    # The GUI binary moves out of /usr/bin (its name there is the launcher's), beside its models:
    # the app looks for them next to the executable before the share folder.
    install -d "$NFPM_STAGE/usr/lib/pdfcraft-nightly"
    mv "$NFPM_STAGE/usr/bin/pdfcraft" "$NFPM_STAGE/usr/lib/pdfcraft-nightly/pdfcraft"
    if [[ -d "$NFPM_STAGE/usr/share/pdfcraft/models" ]]; then
      mv "$NFPM_STAGE/usr/share/pdfcraft/models" "$NFPM_STAGE/usr/lib/pdfcraft-nightly/models"
      rmdir "$NFPM_STAGE/usr/share/pdfcraft" 2>/dev/null || true
    fi
    nightly_launcher "$NFPM_STAGE/usr/bin/$NFPM_BIN" /usr/lib/pdfcraft-nightly/pdfcraft
    mv "$NFPM_STAGE/usr/bin/pdfcraft-cli" "$NFPM_STAGE/usr/bin/$NFPM_CLI"
    mv "$NFPM_STAGE/usr/share/doc/pdfcraft" "$NFPM_STAGE/usr/share/doc/$NFPM_NAME"
    sed -e "s#^Exec=pdfcraft\([[:space:]]\|$\)#Exec=$NFPM_BIN\1#" \
        -e "s|^TryExec=pdfcraft$|TryExec=$NFPM_BIN|" \
        -e "s|^Icon=$APP_ID$|Icon=$NIGHTLY_DESK_ID|" \
        -e "s|^Name=PdfCraft$|Name=PdfCraft Nightly|" \
        -e '/^MimeType=/d' \
        "$NFPM_STAGE/usr/share/applications/$APP_ID.desktop" >"$NFPM_STAGE/usr/share/applications/$NIGHTLY_DESK_ID.desktop"
    rm -f "$NFPM_STAGE/usr/share/applications/$APP_ID.desktop"
    sed -e "s|<id>$APP_ID</id>|<id>$NIGHTLY_DESK_ID</id>|g" \
        -e "s|<name>PdfCraft</name>|<name>PdfCraft Nightly</name>|g" \
        -e "s|<launchable type=\"desktop-id\">$APP_ID.desktop</launchable>|<launchable type=\"desktop-id\">$NIGHTLY_DESK_ID.desktop</launchable>|g" \
        -e "s|<icon type=\"stock\">$APP_ID</icon>|<icon type=\"stock\">$NIGHTLY_DESK_ID</icon>|g" \
        -e "s|<binary>pdfcraft</binary>|<binary>$NFPM_BIN</binary>|g" \
        -e '/<mediatype>/d' \
        "$NFPM_STAGE/usr/share/metainfo/$APP_ID.metainfo.xml" >"$NFPM_STAGE/usr/share/metainfo/$NIGHTLY_DESK_ID.metainfo.xml"
    rm -f "$NFPM_STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
    rm -rf "$NFPM_STAGE/usr/share/mime"
    if [[ -f "$ROOT/assets/app-icon/pdfcraft-nightly.png" ]]; then
      rm -rf "$NFPM_STAGE/usr/share/icons/hicolor"
      install -Dm644 "$ROOT/assets/app-icon/pdfcraft-nightly.png" \
        "$NFPM_STAGE/usr/share/icons/hicolor/256x256/apps/$NIGHTLY_DESK_ID.png"
    else
      for icon in "$NFPM_STAGE/usr/share/icons/hicolor"/*/apps/"$APP_ID".*; do
        [[ -f "$icon" ]] || continue
        ext="${icon##*.}"
        mv "$icon" "$(dirname "$icon")/$NIGHTLY_DESK_ID.$ext"
      done
    fi
  fi
  # nfpm expands env vars in fields like `version` and `arch`, but not in `contents[].src`.
  DROP_LIB_TREE=
  # shellcheck disable=SC2016
  [[ -d "$NFPM_STAGE/usr/lib" ]] || DROP_LIB_TREE='\|^  - src: \${STAGE}/usr/lib/|,+2d;'
  # shellcheck disable=SC2016
  sed "${DROP_LIB_TREE}s|\${STAGE}|$NFPM_STAGE|g;s|\${NFPM_BIN}|$NFPM_BIN|g;s|\${NFPM_CLI}|$NFPM_CLI|g" "$HERE/nfpm.yaml" >"$WORK/nfpm.yaml"
  for fmt in deb rpm; do
    if has "$fmt"; then (cd "$ROOT" && nfpm package -f "$WORK/nfpm.yaml" -p "$fmt" -t "$DIST/$BASENAME.$fmt"); fi
  done
fi

# ---- AppImage -----------------------------------------------------------------------------------
if has appimage; then
  APPIMAGETOOL_VERSION=1.9.1
  case "$ARCH" in
    x86_64) APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0 ;;
    aarch64) APPIMAGETOOL_SHA256=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158 ;;
    *) APPIMAGETOOL_SHA256= ;;
  esac
  # appimagetool otherwise fetches the unpinned type2-runtime `continuous` build and embeds it.
  APPIMAGE_RUNTIME_VERSION=20251108
  case "$ARCH" in
    x86_64) APPIMAGE_RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
    aarch64) APPIMAGE_RUNTIME_SHA256=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
    *) APPIMAGE_RUNTIME_SHA256= ;;
  esac
  if [[ -z "$APPIMAGE_RUNTIME_SHA256" ]]; then
    echo "no pinned AppImage runtime for $ARCH; build the AppImage on x86_64 or aarch64" >&2
    exit 2
  fi
  APPDIR="$WORK/PdfCraft.AppDir"
  cp -R "$STAGE" "$APPDIR"
  if [[ "$IS_NIGHTLY" = 1 ]]; then
    # A script, not a symlink: the Nightly GUI must start through its launch profile, and the
    # desktop entry's Exec names the launcher too.
    nightly_relative_launcher "$APPDIR/AppRun"
    nightly_relative_launcher "$APPDIR/usr/bin/pdfcraft-nightly"
  else
    ln -s usr/bin/pdfcraft "$APPDIR/AppRun"
  fi
  if [[ "$IS_NIGHTLY" = 1 ]]; then
    APPIMAGE_ID="$APP_ID.nightly-v1"
    sed -e "s#^Exec=pdfcraft\([[:space:]]\|$\)#Exec=pdfcraft-nightly\1#" \
        -e "s|^TryExec=pdfcraft$|TryExec=pdfcraft-nightly|" \
        -e "s|^Icon=$APP_ID$|Icon=$APPIMAGE_ID|" \
        -e "s|^Name=PdfCraft$|Name=PdfCraft Nightly|" \
        -e '/^MimeType=/d' \
        "$APPDIR/usr/share/applications/$APP_ID.desktop" >"$APPDIR/usr/share/applications/$APPIMAGE_ID.desktop"
    rm -f "$APPDIR/usr/share/applications/$APP_ID.desktop"
    cp "$APPDIR/usr/share/applications/$APPIMAGE_ID.desktop" "$APPDIR/$APPIMAGE_ID.desktop"
    if [[ -f "$ROOT/assets/app-icon/pdfcraft-nightly.png" ]]; then
      rm -rf "$APPDIR/usr/share/icons/hicolor"
      install -Dm644 "$ROOT/assets/app-icon/pdfcraft-nightly.png" \
        "$APPDIR/usr/share/icons/hicolor/256x256/apps/$APPIMAGE_ID.png"
      cp "$ROOT/assets/app-icon/pdfcraft-nightly.png" "$APPDIR/$APPIMAGE_ID.png"
    else
      for icon in "$APPDIR/usr/share/icons/hicolor"/*/apps/"$APP_ID".*; do
        [[ -f "$icon" ]] || continue
        ext="${icon##*.}"
        mv "$icon" "$(dirname "$icon")/$APPIMAGE_ID.$ext"
      done
      cp "$APPDIR/usr/share/icons/hicolor/256x256/apps/$APPIMAGE_ID.png" "$APPDIR/$APPIMAGE_ID.png"
    fi
    ln -s "$APPIMAGE_ID.png" "$APPDIR/.DirIcon"
  else
    cp "$HERE/$APP_ID.desktop" "$APPDIR/$APP_ID.desktop"
    cp "$ROOT/assets/app-icon/hicolor/256x256/apps/$APP_ID.png" "$APPDIR/$APP_ID.png"
    ln -s "$APP_ID.png" "$APPDIR/.DirIcon"
  fi

  TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
  if [[ -z "$TOOL" ]]; then
    if [[ -z "$APPIMAGETOOL_SHA256" ]]; then
      echo "no pinned appimagetool for $ARCH; set APPIMAGETOOL or put appimagetool on PATH" >&2
      exit 2
    fi
    TOOL="$CARGO_TARGET_DIR/appimagetool-$APPIMAGETOOL_VERSION-$ARCH.AppImage"
    if [[ ! -x "$TOOL" ]]; then
      download="$TOOL.download"
      curl --proto '=https' -fsSL -o "$download" \
        "https://github.com/AppImage/appimagetool/releases/download/$APPIMAGETOOL_VERSION/appimagetool-$ARCH.AppImage"
      verify_sha256 "$APPIMAGETOOL_SHA256" "$download"
      chmod 755 "$download"
      mv "$download" "$TOOL"
    fi
    verify_sha256 "$APPIMAGETOOL_SHA256" "$TOOL"
  fi
  RUNTIME="$CARGO_TARGET_DIR/appimage-runtime-$APPIMAGE_RUNTIME_VERSION-$ARCH"
  if [[ ! -f "$RUNTIME" ]]; then
    download="$RUNTIME.download"
    curl --proto '=https' -fsSL -o "$download" \
      "https://github.com/AppImage/type2-runtime/releases/download/$APPIMAGE_RUNTIME_VERSION/runtime-$ARCH"
    verify_sha256 "$APPIMAGE_RUNTIME_SHA256" "$download"
    mv "$download" "$RUNTIME"
  fi
  verify_sha256 "$APPIMAGE_RUNTIME_SHA256" "$RUNTIME"
  # Absolute, because appimagetool runs in $DIST below (CARGO_TARGET_DIR or APPIMAGETOOL may be
  # relative, e.g. target/agent-<name>).
  OUT="$(cd "$DIST" && pwd)/$BASENAME.AppImage"
  APPDIR="$(cd "$APPDIR" && pwd)"
  TOOL="$(cd "$(dirname "$TOOL")" && pwd)/$(basename "$TOOL")"
  RUNTIME="$(cd "$(dirname "$RUNTIME")" && pwd)/$(basename "$RUNTIME")"
  # A .zsync left by an earlier run would hide a missing zsyncmake and describe another file.
  rm -f "$OUT.zsync"
  # Update information: AppImageUpdate, AppImageLauncher and the like read it from the file and
  # fetch only the blocks that changed in a newer release, through the .zsync published next to
  # each AppImage on GitHub Releases. `latest` is the newest published release that is not a
  # pre-release. A fork's builds point at its own releases through GITHUB_REPOSITORY.
  REPO="${GITHUB_REPOSITORY:-storytold/pdfcraft}"
  UPDATE_ARGS=()
  if [[ "$IS_NIGHTLY" = 0 ]]; then
    UPDATE_INFO="gh-releases-zsync|${REPO%%/*}|${REPO#*/}|latest|pdfcraft-*-linux-$ARCH.AppImage.zsync"
    UPDATE_ARGS=(-u "$UPDATE_INFO")
  else
    # Nightly releases are GitHub pre-releases (`latest` only selects stable releases), so point
    # zsync update metadata at `latest-pre` with a pattern matching both channel-only and
    # version-suffixed Nightly filenames for this architecture.
    UPDATE_INFO="gh-releases-zsync|${REPO%%/*}|${REPO#*/}|latest-pre|pdfcraft-*-linux-$ARCH.AppImage.zsync"
    UPDATE_ARGS=(-u "$UPDATE_INFO")
  fi
  # Extract-and-run: works without FUSE (containers, CI). The output embeds the static runtime,
  # so users don't need libfuse2 either. With zsyncmake on the host (CI installs the zsync
  # package) appimagetool also writes the .zsync, into its working directory, hence the cd.
  (cd "$DIST" && ARCH="$ARCH" APPIMAGE_EXTRACT_AND_RUN=1 "$TOOL" --no-appstream --runtime-file "$RUNTIME" "${UPDATE_ARGS[@]}" "$APPDIR" "$OUT")
  echo "wrote $OUT"
  if [[ -s "$OUT.zsync" ]]; then
    echo "wrote $OUT.zsync"
  else
    warn "zsyncmake not found, so $OUT.zsync was not written; AppImage delta updates need it"
  fi
fi

if [[ -z "${CROSS_TARGET:-}" ]]; then
  "$STAGE/usr/bin/pdfcraft-cli" --version
elif [[ -n "${EMULATOR:-}" ]]; then
  "$EMULATOR" "$STAGE/usr/bin/pdfcraft-cli" --version
fi
echo "==> done"
ls -lh "$DIST"
