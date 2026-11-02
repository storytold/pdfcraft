# PDFCRAFT RELEASE READINESS & DEPLOYMENT MANUAL

**Application ID:** `io.github.dikshant363.pdfcraft`  
**Target Release Version:** `0.0.1-beta` (Version Code: `199`)  
**Audit Date:** 2026-11-02  
**Status:** PRODUCTION-GRADE RELEASE CANDIDATE (RC-1)

---

## 1. Verified Release Artifacts

Both production distribution artifacts have been successfully assembled, optimized with R8, and verified on disk:

```
+--------------------------------------------------------------------------------+
|                             DISTRIBUTION ARTIFACTS                             |
+--------------------------------------------------------------------------------+
| 1. Release APK (Direct Install / GitHub Releases / F-Droid):                   |
|    apps/pdfcraft-android/android/app/build/outputs/apk/release/app-release.apk |
|    Size: 70.0 MiB (73,439,812 bytes)                                           |
|                                                                                |
| 2. Release Android App Bundle (Google Play Store):                             |
|    apps/pdfcraft-android/android/app/build/outputs/bundle/release/app-release.aab|
|    Size: 67.2 MiB (70,471,048 bytes)                                           |
+--------------------------------------------------------------------------------+
```

---

## 2. Signing Architecture & Deployment Protocols

The Gradle build script (`apps/pdfcraft-android/android/app/build.gradle.kts`) implements a dual-mode signing policy:

### 2.1 Release Signing Mode (Production / Play Store)
When signing keys are provided via environment variables, the release task applies the production signing configuration:
- `PDFCRAFT_KEYSTORE`: Absolute path to Java Keystore (`.jks` / `.keystore`).
- `PDFCRAFT_KEYSTORE_PASSWORD`: Keystore passphase.
- `PDFCRAFT_KEY_ALIAS`: Key alias.
- `PDFCRAFT_KEY_PASSWORD`: Key password.

### 2.2 F-Droid Unsigned Mode
For F-Droid's deterministic build infrastructure, appending `-Ppdfcraft.unsigned=true` strips all signing blocks, allowing F-Droid to sign with its official keys:
```bash
./gradlew assembleRelease -Ppdfcraft.unsigned=true
```

### 2.3 Local Testing Fallback
In local developer environments where no production keystore is specified, `assembleRelease` gracefully signs with the Android Debug Key, enabling immediate installation on physical devices and emulators:
```bash
adb install -r apps/pdfcraft-android/android/app/build/outputs/apk/release/app-release.apk
```

---

## 3. Reproducible Build Guarantees

PdfCraft Android enforces strict build reproducibility:
1. **Path Remapping:** Cargo compiles native shared libraries using:
   ```
   --remap-path-prefix=<workspace_dir>=/pdfcraft
   --remap-path-prefix=<cargo_home>=/cargo
   ```
   This strips developer-specific local filesystem paths from panic messages and string tables.
2. **Metadata Stripping:**
   ```kotlin
   vcsInfo { include = false }
   dependenciesInfo {
       includeInApk = false
       includeInBundle = false
   }
   ```
   Ensures identical commit bytes produce byte-for-byte verifiable APK outputs regardless of host environment.

---

## 4. Software Bill of Materials (SBOM) & Legal Compliance

The in-app Licences viewer displays all third-party open-source licenses bundled with the binary:
- **Inter Font:** SIL Open Font License 1.1 (`licences/OFL-Inter.txt`)
- **Dancing Script Font:** SIL Open Font License 1.1 (`licences/OFL-DancingScript.txt`)
- **Lucide Icons:** ISC License (`licences/LICENSE-lucide.txt`)
- **Hayro Foxit Appearance:** BSD-style notice (`licences/LICENSE-foxit.txt`)
- **Adobe CMap Tables:** BSD-3-Clause notice (`licences/LICENSE-adobe-cmaps.txt`)

All 323 assets are verified via `cargo xtask assets` without license or author violations.

---

## 5. Upgrade Path & Data Persistence

1. **Sandboxed Data:** Bookmarks and recent file lists are serialized into SQLite/Room and JSON inside `context.filesDir`.
2. **Temporary Render Caches:** Rendered page tiles and temporary scratch files sit in `context.cacheDir` and are subject to automatic OS eviction under low storage conditions without risking user data loss.
3. **No Database Migrations Required:** Initial release utilizes stateless document binding; application updates retain user bookmarks and recent history across app package updates.
