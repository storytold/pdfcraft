# Building a modified working tree on Windows

`packaging/windows/build-dev.ps1` compiles the current source checkout, including uncommitted
changes. It does not extract a pristine release over the checkout, reset Git, download tools,
install software, create an MSI, sign binaries or publish a release. It is for developing and
testing changes before the distribution work described in `packaging/windows/INSTALLATION.md`.

## Prerequisites

- PowerShell 7.4 or later.
- Git and Rust MSVC at least as new as `[workspace.package].rust-version` in this checkout
  (1.95 for PdfCraft 0.21.1).
- Visual Studio 2022 Build Tools, Desktop development with C++, and Windows SDK. Run inside
  Developer PowerShell so `cl.exe` and `rc.exe` are available.
- Clean Git checkouts of [craft-i18n](https://github.com/storytold/craft-i18n) and
  [craft-fonts](https://github.com/storytold/craft-fonts), at the commits this source pins.

The translation commit is the contents of `craft-i18n.rev`; the font commit is
`craft-fonts-ref` in `.github/workflows/release.yml`. For the 0.21.1 starting point:

```powershell
git clone https://github.com/storytold/craft-i18n ../craft-i18n
git -C ../craft-i18n checkout --detach 866aff71dd63d8369eb100b9c432b5af1400c22a
git clone https://github.com/storytold/craft-fonts ../craft-fonts
git -C ../craft-fonts checkout --detach b076417b5880bd4154bd22f00695b1d28c6bea83
rustup target add x86_64-pc-windows-msvc
```

Use separate checkouts for translation/font editing. This script refuses a changed, untracked
or ignored file in a pinned input: locale discovery also embeds files that Git HEAD alone does
not describe. **The application source itself may have changes**; that is what the script builds.

### Enter the installed MSVC environment

An ordinary PowerShell window may not expose `cl.exe` or `rc.exe` even when the C++ workload
and SDK are installed. Discover and enter that installation rather than installing a second
copy of Visual Studio:

```powershell
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$vsRoot = & $vswhere -latest -products '*' `
  -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vsRoot) { throw 'Install the Visual Studio C++ desktop workload and Windows SDK first.' }
Import-Module (Join-Path $vsRoot 'Common7/Tools/Microsoft.VisualStudio.DevShell.dll')
Enter-VsDevShell -VsInstallPath $vsRoot -SkipAutomaticLocation `
  -DevCmdArguments '-arch=x64 -host_arch=x64'
$env:Path = (Join-Path $env:USERPROFILE '.cargo/bin') + ';' + $env:Path
Get-Command cargo, rustc, rustup, cl.exe, rc.exe
rustc -Vv
```

If Rust is absent, use the [official Windows rustup installer](https://rust-lang.org/tools/install/).
Select an MSVC toolchain meeting the source's minimum version. A fresh terminal sees changes
to the user PATH; the current terminal may need the `.cargo/bin` entry above.

## Validate and build

From the PdfCraft source folder:

```powershell
pwsh -NoProfile -File ./packaging/windows/build-dev.ps1 `
  -I18nDirectory ../craft-i18n -FontsDirectory ../craft-fonts -CheckInputsOnly

pwsh -NoProfile -File ./packaging/windows/build-dev.ps1 `
  -I18nDirectory ../craft-i18n -FontsDirectory ../craft-fonts -RunTests
```

For the corrected DXGI file-output development build, explicitly add `-DxgiFileOutput`:

```powershell
pwsh -NoProfile -File ./packaging/windows/build-dev.ps1 `
  -I18nDirectory ../craft-i18n -FontsDirectory ../craft-fonts -DxgiFileOutput -RunTests
```

This mode runs `packaging/windows/prepare-dxgi-deps.ps1`. It copies the locked
MIT/Apache-2.0 craft-print and BSD-3-Clause winprint sources from clean Cargo checkouts
into `target/native-deps/`, verifies the reviewed patch hashes, and applies the explicit
file-stream and multithreaded Direct2D factory fixes there. Missing locked sources must
first be fetched with `cargo fetch --locked`. No shared Cargo checkout, global Cargo
configuration, root manifest or lockfile is modified. The generated configuration selects
the private craft-print copy with a Cargo path override and replaces the exact winprint source revision and enables
`pdfcraft-print/dxgi-file-output` for both tests and binaries. An override dependency-list
advisory may be emitted; it is recorded separately from compiler/clippy results.

Without the switch, dependencies remain stock and DXGI `printer_output` returns a clear
unavailable-feature error. Stock binaries do not contain these native fixes. `-CheckInputsOnly`
does not prepare dependencies or test DXGI. Reuse verifies every prepared source hash;
an incomplete or changed preparation fails instead of overwriting it. These are local
development patches. Explicit DXGI file output accepts 1–64 imposed sheets.
GDI accepts one imposed sheet.

For scoped lint checks with the same validated `CRAFT_*` inputs in your shell:

```powershell
$prepared = & ./packaging/windows/prepare-dxgi-deps.ps1
cargo clippy --config $prepared.ConfigPath --locked `
  -p pdfcraft-print -p pdfcraft-automation --features pdfcraft-print/dxgi-file-output `
  --all-targets -- -D warnings
```

Keep `--config` after `clippy` so that the external subcommand forwards it to its
internal Cargo invocation. The ordinary `test` and `build` commands also accept the
prefix form used by the development helper. An incorrect lint invocation selecting
stock dependencies cannot validate the prepared feature-enabled build.

The two paths may instead come from `CRAFT_I18N_DIR` and `CRAFT_FONTS_DIR`. They are resolved to
absolute paths. The script verifies their Git revisions, the Turkish `common.tsv` and
`pdfcraft.tsv` catalogs, and the native font license notices. Required-input variables make a
missing translation/font input fail the build instead of silently producing an English-only
or incomplete-font application. This validates build inputs; it does not prove Turkish PDF
editing or OCR correctness.

`-RunTests` runs the `pdfcraft-edit`, `pdfcraft-ocr` and `pdfcraft-automation` test packages for
the host. The build uses `cargo build --locked` for `pdfcraft` and `pdfcraft-cli` with the
requested Windows MSVC target. x64 is the default; `-Architecture x86` or `arm64` also requires
the corresponding Rust target and MSVC target tools. Cross-compiling is not evidence that the
result works on that architecture.

Default x64 outputs:

```text
target/x86_64-pc-windows-msvc/debug/pdfcraft.exe
target/x86_64-pc-windows-msvc/debug/pdfcraft-cli.exe
```

Use `-Release` for `release/` binaries. `-TargetDirectory` overrides the build output folder;
otherwise the script honors `CARGO_TARGET_DIR` or uses this source tree's `target/`. An explicit
relative target directory is resolved against the caller's current folder. Build environment
variables and the working directory are restored even when Cargo fails. Existing build flags
are left alone; these development binaries are not a portable distribution and are not
promised to contain a statically linked CRT.

## Prepare OCR for local testing

The script does not fetch OCR models. Fetch the attributed built-in models separately; xtask
checks their pinned SHA-256 values and downloads their license notices:

```powershell
cargo xtask models
$env:PDFCRAFT_MODELS = Join-Path (Get-Location) 'assets/models'
```

These built-in models do not read Turkish. Turkish recognition needs an installed external
Tesseract and its `tur` language model; see the [Tesseract installation guide](https://tesseract-ocr.github.io/tessdoc/Installation.html).
Keep this installation and downloaded language data outside the source checkout. Installing
the engine alone does not prove that Turkish language data is present.

For repeatable development measurements, the official
[`tessdata_fast` revision `87416418657359cb625c412a48b6e1d6d41c29bd`](https://github.com/tesseract-ocr/tessdata_fast/tree/87416418657359cb625c412a48b6e1d6d41c29bd)
has these Apache-2.0 files. Retain its `LICENSE` beside the downloaded models and verify each
file against the hash before use:

| File | SHA-256 |
|---|---|
| `tur.traineddata` | `7393381111e1152420fc4092cb44eef4237580d21b92bf30d7d221aad192c6b7` |
| `eng.traineddata` | `7d4322bd2a7749724879683fc3912cb542f19906c83bcc1a52132556427170b2` |
| `osd.traineddata` | `9cf5d576fcc47564f11265841e5ca839001e7e6f38ff7f7aacf46d15a96b00ff` |
| `LICENSE` | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |

Set `PDFCRAFT_TESSERACT` to the installed `tesseract.exe`'s absolute path and `TESSDATA_PREFIX`
to the directory holding the verified language files. These are process environment settings,
not PDF input or automatic downloads by the application. Check both the engine and the models:

```powershell
& $env:PDFCRAFT_TESSERACT --version
& $env:PDFCRAFT_TESSERACT --list-langs
```

### Explicit extended Turkish alphabet

The optional `turkish_extended` profile uses the fixed `script/Latin` pack instead of
Standard's `tur` pack. It requires Turkish and Tesseract (or Automatic); it does not change
the default profile, download data or fall back when the required pack is missing.
Its alphabet can emit `î`, `Î`, `û` and `Û`, which the two measured `tur` alphabets lack.
Review recognized words before applying them.

For a repeatable development setup, use the official Apache-2.0
[`tessdata_fast` commit `87416418657359cb625c412a48b6e1d6d41c29bd`](https://github.com/tesseract-ocr/tessdata_fast/tree/87416418657359cb625c412a48b6e1d6d41c29bd).
Keep these files and their license outside the source checkout:

| Relative file | Bytes | SHA-256 |
|---|---:|---|
| `script/Latin.traineddata` | 89,384,811 | `6dbdaf8ecc6c40f025c2648bf3b3f3fbffe073e1fd2df2047fde2e2b2f020d53` |
| `LICENSE` | 11,358 | `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30` |

The directory structure matters: `TESSDATA_PREFIX` names the directory containing
`script/`, and `--list-langs` must list `script/Latin`. Do not rename the pack to `tur`.
Standard Turkish still requires a separate `tur.traineddata`. A fresh process avoids
reusing an earlier installed-model discovery cache.

After verifying hashes, set the data prefix in the development shell and explicitly
select the new profile:

```powershell
$env:TESSDATA_PREFIX = '<absolute external model directory>'
& $env:PDFCRAFT_TESSERACT --list-langs
& ./target/x86_64-pc-windows-msvc/debug/pdfcraft-cli.exe run ocr_status `
  language=tr engine=auto model_profile=turkish_extended
```

Check `required_models`, `installed_models`, `requested_model_profile` and
`effective_model_profile`. An unavailable selection has a null effective profile;
an inventory listing alone is not a recognition-accuracy acceptance result.

Require `tur` in the second command's output. Once the CLI is built, `pdfcraft-cli run ocr_status`
checks the application's discovery too. Use `language=tr engine=auto` or `engine=tesseract` for
Turkish recognition; explicitly choosing `ocrs` is an error. A model-only `TESSDATA_PREFIX` is
supported: no `configs/tsv` file is needed.

The model paths are read at runtime; building does not embed these environment values in the
EXE. Launch from the same configured PowerShell session. A later Explorer launch does not
inherit variables from a closed development shell:

```powershell
./target/x86_64-pc-windows-msvc/debug/pdfcraft-cli.exe run ocr_status language=tr engine=auto
./target/x86_64-pc-windows-msvc/debug/pdfcraft.exe --language tr
```

A development build is not
ready for redistribution: modified versions must satisfy the brand/license requirements in
`docs/brand/LICENSE-brand.txt`, carry their font/model notices, and pass the Windows GUI and
installer/portable acceptance tests.

The offline training-data helper also supports an explicit `--mode specialization-v2`
(512 training and 128 internal-validation lines). Its default remains `original-v1`
(128/32). Training data and experimental models stay outside the checkout.
The command's own help lists the required pins and verification options.

`--mode counterbalanced-v3` is a separate controlled research mode. It requires an
external, previously frozen V3 design (`--v3-design` and `--v3-design-sha256`) and the
pinned V2 dataset referenced by that design; it does not synthesize or silently refresh
the design. It retains 272 V2 training samples and the same 128 validation samples,
and adds 240 contrast controls. These data checks measure dataset construction.

## Helper regression checks

```powershell
pwsh -NoProfile -File ./packaging/windows/test-build-dev.ps1
```

The checks use temporary synthetic Git inputs and a substituted command runner to exercise pin
validation, source preservation and environment restoration. They do not compile Rust or run
Windows executables. `-CheckInputsOnly` can run on another platform; a real build requires
Windows. Passing helper checks on Linux does not establish Windows compilation or GUI results.
