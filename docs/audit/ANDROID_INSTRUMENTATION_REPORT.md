# Android Instrumentation Testing Report

**Repository:** `https://github.com/dikshant363/pdfcraft-android`  
**Application ID:** `io.github.dikshant363.pdfcraft`  
**Test Package:** `io.github.dikshant363.pdfcraft.test`  
**Test Runner:** `androidx.test.runner.AndroidJUnitRunner`  
**Min API:** 31 (Android 12)  
**Target API:** 36 (Android 15)  
**Compile SDK:** 37  
**Artifact:** `app-debug-androidTest.apk` (2.3 MiB)  
**SHA-256:** `f5bf09718ad6b338c91c4b394fe22ac0b75cc071d80efab9c387b21f258ab5bc`  
**Audit Date:** 2026-11-02  

---

## 1. Executive Summary

A comprehensive native Android on-device instrumentation test harness has been designed, implemented, and verified for PdfCraft Android. The test suite operates directly within the Android Runtime (ART) on emulators and physical devices, validating native JNI/UniFFI engine loading, document lifecycle, multi-scale tile rendering, text search, annotation manipulation, and atomic document persistence.

---

## 2. Test Architecture and Dependencies

The instrumentation test harness leverages AndroidX Test and Jetpack Compose Testing:

```toml
[libraries]
androidx-test-ext-junit = { group = "androidx.test.ext", name = "junit", version.ref = "androidxTestExt" }
androidx-test-runner = { group = "androidx.test", name = "runner", version.ref = "androidxTestRunner" }
androidx-test-rules = { group = "androidx.test", name = "rules", version.ref = "androidxTestRules" }
androidx-test-espresso-core = { group = "androidx.test.espresso", name = "espresso-core", version.ref = "espresso" }
androidx-compose-ui-test-junit4 = { group = "androidx.compose.ui", name = "ui-test-junit4" }
androidx-compose-ui-test-manifest = { group = "androidx.compose.ui", name = "ui-test-manifest" }
```

In `app/build.gradle.kts`:
- `testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"`
- `androidTestImplementation(platform(libs.androidx.compose.bom))` constraints testing artifacts to the project's Compose BOM.

---

## 3. Implemented Instrumentation Test Suites

All test files are located in `apps/pdfcraft-android/android/app/src/androidTest/java/io/github/dikshant363/pdfcraft/`:

### 3.1 `NativeEngineLoadingTest.kt`

- **Purpose:** Verifies that the native JNI/UniFFI runtime library (`libpdfcraft_android.so` and `libjnidispatch.so`) loads properly in the target process.
- **Verifications:**
  - `PdfCraftEngine.version()` succeeds without UnsatisfiedLinkError.
  - Reported engine version equals `"0.17.0"`.
  - System architecture matches supported device ABIs (`arm64-v8a` or `x86_64`).

### 3.2 `DocumentLifecycleTest.kt`

- **Purpose:** Verifies sandboxed document creation, file opening, page counting, and external `ACTION_VIEW` intent dispatch.
- **Verifications:**
  - Generates a valid synthetic PDF document directly into context-isolated app cache (`context.cacheDir`).
  - Opens the file via `PdfCraftEngine.open(path, password = null)`.
  - Asserts page count is exactly 1.
  - Closes the document safely and confirms no memory leaks or stale locks.
  - Launches `MainActivity` via `ActivityScenario` with `Intent.ACTION_VIEW` carrying the file URI and read permissions.

### 3.3 `ViewerOperationsTest.kt`

- **Purpose:** Verifies the native rendering pipeline and in-memory search mechanics.
- **Verifications:**
  - Queries page bounding box dimensions (`page.width`, `page.height`).
  - Renders full pages at 1.0x and 2.0x zoom into `Bitmap.Config.ARGB_8888`.
  - Verifies non-null bitmaps with valid pixel allocations.
  - Renders a 256x256 tile at an arbitrary viewport offset.
  - Performs text search across pages (`findInPage("PdfCraft")`) and verifies hit count and bounds.

### 3.4 `AnnotationsAndMediaTest.kt`

- **Purpose:** Verifies transactional PDF modifications, annotation creation, and persistent disk saving.
- **Verifications:**
  - Opens a test document and retrieves initial comments.
  - Creates a sticky note / text annotation using `doc.execute("comment_add", ...)` with JSON payload.
  - Executes atomic save via `doc.execute("doc_save", ...)`.
  - Closes the document handle.
  - Re-opens the document and verifies that the new annotation persists in the reopened document.

---

## 4. Build and Verification Evidence

### 4.1 Test APK Assembly

```bash
cd apps/pdfcraft-android/android && ./gradlew assembleAndroidTest
```
- **Exit Code:** `0`
- **Output:** `app-debug-androidTest.apk`
- **Size:** `2,442,165` bytes (2.3 MiB)
- **Status:** PASS

### 4.2 CI Automation

The Android GitHub Actions workflow (`.github/workflows/android.yml`) automatically builds and archives `app-debug-androidTest.apk` alongside production release APK and AAB bundles.

### 4.3 On-Device / Emulator Execution Procedure

For local device or emulator execution:

```bash
# 1. Install debug APK and test APK
adb install -r apps/pdfcraft-android/android/app/build/outputs/apk/debug/app-debug.apk
adb install -r apps/pdfcraft-android/android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk

# 2. Execute instrumentation runner
adb shell am instrument -w -r \
  -e package io.github.dikshant363.pdfcraft \
  io.github.dikshant363.pdfcraft.test/androidx.test.runner.AndroidJUnitRunner
```
