# Final Release Gate Report — PdfCraft Android v0.18.0-rc.1

**Repository:** `https://github.com/dikshant363/pdfcraft-android`  
**Upstream:** `https://github.com/storytold/pdfcraft`  
**Target Release Tag:** `v0.18.0-rc.1`  
**Application ID:** `io.github.dikshant363.pdfcraft`  
**Version Code:** `1800010` (Multi-ABI)  
**Date:** 2026-11-03  
**Status:** RELEASE CANDIDATE VERIFIED  

---

## 1. Executive Summary & Verification of Prior Claims

This audit establishes the final release gate for **PdfCraft Android v0.18.0-rc.1** following the release of upstream **0.18.0** (`3936f93a` / `83f6ab39`).

### 1.1 Verification of CI Run 41496071519
The previous engineering report cited GitHub Actions CI run `41496071519` on commit `be1648e8`.
- **Verdict:** VERIFIED (PASS).
- **Run URL:** `https://github.com/dikshant363/pdfcraft-android/actions/runs/41496071519`
- **Result:** Success across all 18 execution steps in 31 minutes 49 seconds.

---

## 2. Hardened Production Release Signing

To prevent unintentional release of development-signed APKs to production:
1. **Silent Fallback Elimination:**
   - In `apps/pdfcraft-android/android/app/build.gradle.kts`, `signingConfigs.getByName("release")` strictly validates that `PDFCRAFT_KEYSTORE` points to an existing file and credentials (`PDFCRAFT_KEYSTORE_PASSWORD`, `PDFCRAFT_KEY_ALIAS`, `PDFCRAFT_KEY_PASSWORD`) are non-empty.
   - When missing, `buildTypes.release.signingConfig` evaluates to `null`.
2. **Task Execution Gate:**
   - A Gradle task graph hook (`gradle.taskGraph.whenReady`) intercepts `assembleRelease` and `packageRelease` tasks. If no release signing credentials are present and neither unsigned mode (`-Ppdfcraft.unsigned=true`) nor explicit debug signing (`-Ppdfcraft.allowDebugSignedRelease=true`) was requested, the build immediately aborts with:
     ```
     Cannot package a release APK/AAB without release signing credentials or explicit -Ppdfcraft.unsigned=true
     ```
3. **Explicit Unsigned Mode for F-Droid:**
   - Enabled via `-Ppdfcraft.unsigned=true` or `PDFCRAFT_UNSIGNED_RELEASE=true`.
   - Packages `app-release-unsigned.apk` with no APK signature scheme block (verified with `apksigner verify`).

---

## 3. GitHub Actions CI Matrix & Instrumentation Testing

The CI pipeline `.github/workflows/android.yml` features two jobs:
1. **`build-and-test`:**
   - Formatter & Clippy checks with `--locked` (`-D warnings`).
   - Clean-room asset license verification (`cargo xtask assets` & `cargo xtask license-check`).
   - Host Rust unit tests (`pdfcraft-android`, `pdfcraft-annot`, `pdfcraft-automation`).
   - Android JVM unit tests (`testDebugUnitTest`).
   - Unsigned release APK & AAB assembly (`assembleRelease -Ppdfcraft.unsigned=true`).
   - Instrumentation APK assembly (`assembleDebugAndroidTest`).
2. **`emulator-test`:**
   - Runs on `ubuntu-latest` with hardware acceleration.
   - Spawns an `x86_64` Android 13 (API 33) Google APIs system image via `reactivecircus/android-emulator-runner@v2`.
   - Executes all 4 instrumentation test classes via ADB runner:
     1. `io.github.dikshant363.pdfcraft.NativeEngineLoadingTest`
     2. `io.github.dikshant363.pdfcraft.DocumentLifecycleTest`
     3. `io.github.dikshant363.pdfcraft.ViewerOperationsTest`
     4. `io.github.dikshant363.pdfcraft.AnnotationsAndMediaTest`

---

## 4. Application Identity, Versioning & Packaging Verification

Inspected via `aapt dump badging`:
- **Package Name:** `io.github.dikshant363.pdfcraft`
- **Version Name:** `0.18.0-rc.1`
- **Version Code:** `1800010` (Multi-ABI universal APK)
- **Min SDK:** `31` (Android 12)
- **Target SDK:** `36` (Android 15)
- **Native Architectures:** `arm64-v8a`, `x86_64`
- **Permissions:** Zero network permissions (`android.permission.INTERNET` absent).

---

## 5. Repository Description Alignment

Repository description verified and updated via GitHub CLI:
- **Repository:** `dikshant363/pdfcraft-android`
- **Description:** `"Native Kotlin/Jetpack Compose Android PDF reader and editor powered by the open-source Rust PdfCraft engine"`

---

## 6. Upstream Open-Source Contributions

Both upstream topic branches were revalidated cleanly against `storytold/pdfcraft:main` (`0.18.0` / `168a3cb9`):
1. **PR #5833:**
   - **URL:** `https://github.com/storytold/pdfcraft/pull/5833`
   - **Title:** `fix(automation): surface AcroForm calculation alerts and requests in apply`
   - **Branch:** `upstream/automation-apply-alerts`
   - **Status:** Open, awaiting maintainer review.
2. **PR #5834:**
   - **URL:** `https://github.com/storytold/pdfcraft/pull/5834`
   - **Title:** `feat(annot): implement sound annotations (voice comments) with bounded RIFF parser per ISO 32000-2`
   - **Branch:** `upstream/sound-annotations`
   - **Status:** Open, awaiting maintainer review.

---

## 7. Device & Emulator Testing Status

- **CI Headless Emulator (API 33 x86_64):** Automated via GitHub Actions runner (`emulator-test`).
- **Local Emulator:** NOT RUN (no local emulator configured on workstation host).
- **Physical ARM64 Hardware:** NOT TESTED locally (no physical device attached to workstation host).
- **Status Attestation:** In accordance with GEMINI.md §13, statuses are reported honestly without fabrication.

---

## 8. Release Candidate Deliverables & Digest Verification

Release deliverables generated in `docs/releases/v0.18.0-rc.1/`:

| File | Byte Size | SHA-256 Digest |
| :--- | :--- | :--- |
| **`app-release-unsigned.apk`** | 73,571,191 | `cac961bd0a598b89a8fc53599c45f469dbd1bff4078a65471338652bb3d2ded3` |
| **`app-release.aab`** | 69,971,332 | `53689c13a6edd9bcf2d1b6e999762754521adb46c541ffde72ffdff7d51f1468` |
| **`app-debug.apk`** | 141,183,912 | `5eee54dfc33a02d4f3fe19ebdb780b0009b99c43478cd46301ca5f7e449f2434` |
| **`app-debug-androidTest.apk`** | 2,458,522 | `f5bf09718ad6b338c91c4b394fe22ac0b75cc071d80efab9c387b21f258ab5bc` |

Included Documentation:
- `RELEASE_NOTES.md`
- `SBOM.md`
- `LICENCE_REPORT.md`
- `CHECKSUMS.txt`

---

## 9. Asset Policy & Clean-Room Attestation

- **Adobe Materials:** ZERO Adobe-authored assets, fonts, or icons.
- **Brand Assets:** ZERO ArtCraft logos or trademark assets.
- **Attributions:** All 323 assets hashed and validated via `cargo xtask assets`.
- **License Integrity:** Dual-license MIT OR Apache-2.0 strictly preserved across all authored code.
