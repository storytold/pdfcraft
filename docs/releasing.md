# Releasing PdfCraft

Every push to the `release` branch runs `.github/workflows/release.yml`. It builds installers for
macOS, Windows, Linux, FreeBSD and the web, signs the ones it has certificates for, and creates or
updates a **draft** GitHub Release named `PdfCraft v<version>`. Nobody sees a draft until a
maintainer publishes it.

The pipeline was ported from PhotoCraft's. User-facing names say **PdfCraft**; files, binaries and
ids stay lowercase (`pdfcraft-<version>-<platform>-<arch>.<ext>`, `ai.storyteller.pdfcraft`).

## Cutting a release

1. **Bump the version** on `main`. It lives only in `[workspace.package] version` in the root
   `Cargo.toml`:

   ```sh
   cargo xtask version                 # prints the current version
   cargo xtask version set 0.3.0       # or 0.3.0-rc.1; updates Cargo.toml and Cargo.lock
   ```

   Commit the change through the normal review flow (see the `Release: PdfCraft v0.2.1` PR).
2. **Merge `main` into `release`** (or fast-forward it) and push. The workflow starts by itself.
3. **Wait for the draft.** When every job has finished (notarization is the slow part), the
   Releases page has a draft `PdfCraft v0.3.0` targeting the pushed commit, with every artifact
   and `SHA256SUMS.txt`. The notes are generated from the merged PRs.
4. **Check it.** Download an installer or two and read the job summaries. A `::warning::` there
   means a signing secret was missing and that artifact is unsigned.
5. **Publish** the draft in the GitHub UI. Publishing creates the `v0.3.0` tag. Versions with a
   pre-release suffix (`-rc.1`) are marked as pre-releases.

Pushing to `release` again before you publish rebuilds the same draft and replaces its assets.
Once the draft is published, the workflow refuses to touch that version again, so bump it first.

**Test runs:** *Actions ▸ Release ▸ Run workflow* runs the whole pipeline by hand. The optional
`version` input (such as `0.3.0-rc.1`) overrides `Cargo.toml` for that run only; each job applies it
with `cargo xtask version set` before building, so the binaries report it too. The jobs use the
`release` environment, so pick the `release` branch in the dialog.

## What gets built

| Platform | Artifacts | Built on |
|---|---|---|
| macOS 11+ (universal: Apple silicon + Intel) | `pdfcraft-<v>-macos-universal.dmg`, `pdfcraft-cli-<v>-macos-universal.zip` | `macos-15` |
| Windows 10+ x64 | `pdfcraft-<v>-windows-x64.msi`, `pdfcraft-<v>-windows-x64-portable.zip` | `windows-latest` |
| Windows 10+ x86 (32-bit) | `pdfcraft-<v>-windows-x86.msi`, `pdfcraft-<v>-windows-x86-portable.zip` | `windows-latest` |
| Windows 11 on ARM64 | `pdfcraft-<v>-windows-arm64.msi`, `pdfcraft-<v>-windows-arm64-portable.zip` | `windows-latest` (cross-compiled) |
| Linux x86_64 | `pdfcraft-<v>-linux-x86_64.{AppImage,deb,rpm,tar.gz}` | `ubuntu-22.04` |
| Linux aarch64 | `pdfcraft-<v>-linux-aarch64.{AppImage,deb,rpm,tar.gz}` | `ubuntu-22.04-arm` |
| FreeBSD 14 x86_64 | `pdfcraft-<v>-freebsd-x86_64.tar.gz` | FreeBSD VM on `ubuntu-latest` |
| Web | `pdfcraft-web-<v>.zip` (a static site; see [`packaging/web/README.md`](../packaging/web/README.md)) | `ubuntu-latest` |

The ARM64 Windows build is cross-compiled on the x64 runner, so signing and WiX work as for the
other Windows builds; `.github/workflows/windows-arm64.yml` installs and runs it on ARM64 hardware.

Every binary reports its version (`pdfcraft --version`, `pdfcraft-cli --version`, *Help ▸ About*).
The workflow sets `PDFCRAFT_BUILD_SHA` and `PDFCRAFT_BUILD_DATE` (`packaging/env.sh` fills them in
for local builds): the commit is recorded in the macOS `Info.plist` (`PdfCraftBuildCommit`) and the
date in the AppStream metadata. The binaries don't embed the commit yet.

**Fonts:** every job checks out [craft-fonts](https://github.com/storytold/craft-fonts) at the commit
pinned in `release.yml` and builds with `CRAFT_FONTS_DIR` and `CRAFT_FONTS_REQUIRED=1`, so releases
embed its Japanese fonts and fail rather than ship without them (`AGENTS.md` §1.4). Bump the pin
deliberately.

### macOS

`packaging/macos/package.sh` builds both architectures, joins them with `lipo`, and assembles
`PdfCraft.app` from `Info.plist.in` (bundle id `ai.storyteller.pdfcraft`, macOS 11+, PDF declared
as a document type with rank Alternate, so PdfCraft is offered under Open With without taking over
from Preview). Files opened from Finder arrive as Apple events, which
`apps/pdfcraft/src/apple_events.rs` receives.

- **Signing** uses the hardened runtime and a secure timestamp, executable first, then the bundle.
  `packaging/macos/import-cert.sh` puts the certificate in a temporary keychain, deleted at the end
  of the job.
- **Notarization:** the app is zipped and sent with `xcrun notarytool submit --wait`, the ticket is
  stapled, and the result is checked with `codesign --verify`, `stapler validate` and `spctl`. The
  app ships on a drag-to-Applications DMG, which is signed and notarized too.
- **CLI:** `pdfcraft-cli` is signed and notarized as a zip. A bare executable can't hold a stapled
  ticket, so Gatekeeper looks it up online the first time a downloaded copy runs.

Without certificates (locally) the script signs ad-hoc and skips notarization:

```sh
packaging/macos/package.sh --arch aarch64     # quicker, host-only; --arch universal needs both targets
```

### Windows

`packaging/windows/package.ps1 -Arch x64|x86|arm64` builds with `-C target-feature=+crt-static`, so
neither the MSI nor the portable zip needs the Visual C++ redistributable.

- Before packaging, it reads both executables' PE headers: the machine type must match `-Arch`, and
  `pdfcraft.exe` must be a GUI-subsystem program (no console window, #57) while `pdfcraft-cli.exe`
  stays a console program.
- `pdfcraft.wxs` (WiX v5) installs per machine into Program Files with a Start Menu shortcut and an
  App Paths entry. The MSI version is the numeric `X.Y.Z` (MSI has no pre-release field), and
  same-version upgrades are allowed so release candidates replace each other. Icon ids end in `.ico`
  or `.exe` (Windows Installer requires it); packaging-lint checks this.
- The portable zip holds both executables, the README, the licences, and the OFL licence of each
  embedded craft-fonts family.
- **Signing:** `packaging/windows/sign.ps1` signs both executables and the MSI with `signtool`
  (SHA-256, RFC 3161 timestamp), from a `.pfx` (`WINDOWS_CERTIFICATE`) or Azure Trusted Signing
  (`AZURE_*`), whichever is configured. It is the one place to change when signing changes.

Locally: `dotnet tool install -g wix --version 5.0.2`, then `pwsh packaging/windows/package.ps1 -Arch x64`.

### Linux

`packaging/linux/package.sh` stages one FHS tree (both binaries, the desktop entry, hicolor icons,
AppStream metainfo) and makes every format from it: an **AppImage** (any distribution, nothing to
install), a **.deb** and an **.rpm** (built with [nfpm](https://nfpm.goreleaser.com) from
`nfpm.yaml`; they integrate with the menu, MIME and icon caches), and a **.tar.gz**.

The binaries are built on Ubuntu 22.04, the oldest GitHub-hosted image, so they need only
**glibc ≥ 2.35**: Ubuntu 22.04+, Debian 12+, Fedora 36+, RHEL 10. Windowing (X11, Wayland,
xkbcommon) and the GPU (Vulkan, EGL) are loaded at runtime from the system; the .deb and .rpm declare
them as dependencies (see `nfpm.yaml`).

Locally (on Linux, with nfpm): `packaging/linux/package.sh` or `--formats "deb tar"`.

### FreeBSD

GitHub has no FreeBSD runners, so the `freebsd` job runs `packaging/freebsd/package.sh` in a FreeBSD
VM with the packages from `.github/workflows/freebsd.yml`. The tarball is a `/usr/local`-style tree
with the same desktop entry, metainfo and icons as Linux. FreeBSD has no code signing for loose
binaries; check the tarball against `SHA256SUMS.txt`.

### Web

`packaging/web/package.sh` runs `trunk build --release` in `apps/pdfcraft-web` and zips the site
with sample `_headers` and `.htaccess` files. Hosting (MIME types, compression, caching, iframes) is
covered in [`packaging/web/README.md`](../packaging/web/README.md).

## Secrets

The signing secrets live in the repository's **`release` environment** (*Settings ▸ Environments ▸
release*), restricted to the `release` branch; every job in `release.yml` declares
`environment: release`. Each secret is optional: if one is missing, that platform's artifacts are
unsigned and the run shows a warning.

| Secret | Used for |
|---|---|
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` | base64 Developer ID Application `.p12` and its password |
| `KEYCHAIN_PASSWORD` | the temporary CI keychain (random if unset) |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | `notarytool` (an app-specific password) |
| `WINDOWS_CERTIFICATE`, `WINDOWS_CERTIFICATE_PASSWORD` | base64 `.pfx` code-signing certificate and its password |
| `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` | service principal for Azure Trusted Signing (instead of a `.pfx`) |
| `AZURE_SIGNING_ENDPOINT`, `AZURE_SIGNING_ACCOUNT`, `AZURE_CERT_PROFILE` | Trusted Signing endpoint, account and certificate profile |

`GITHUB_TOKEN` creates the release; only the final job gets `contents: write`.

## Checks

`.github/workflows/packaging-lint.yml` runs on changes to `packaging/` and the workflows: actionlint, shellcheck,
a PowerShell parse, xmllint (WiX, plist, MIME), `desktop-file-validate`, `appstreamcli validate`, and
the MSI icon-id check. The FreeBSD and Windows ARM64 workflows also exercise their packaging before a
release does.
