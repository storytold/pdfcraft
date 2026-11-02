# Building PdfCraft Android

This guide explains how to build the native Android application for **PdfCraft Android** (`io.github.dikshant363.pdfcraft`) from source.

---

## 1. Prerequisites and Toolchain

### 1.1 Host Environment
- **Operating System:** macOS (Apple Silicon or Intel), Linux (x86_64 or aarch64).
- **Git:** 2.30+

### 1.2 Java Development Kit (JDK)
- **Required:** OpenJDK 17 (or newer, compatible with AGP 9.2+).
- **Environment Variable:**
  ```bash
  export JAVA_HOME=/path/to/jdk-17
  export PATH=$JAVA_HOME/bin:$PATH
  ```

### 1.3 Android SDK & NDK
- **Android SDK:** Platform 37 (`platforms;android-37.0`) and Platform 35 (`platforms;android-35`).
- **Build Tools:** `build-tools;36.0.0` or `build-tools;35.0.0`.
- **Android NDK:** Version `29.0.14206865`.
- **Environment Variables:**
  ```bash
  export ANDROID_HOME=/opt/homebrew/share/android-commandlinetools # or ~/Library/Android/sdk
  export ANDROID_NDK_ROOT=$ANDROID_HOME/ndk/29.0.14206865
  ```

### 1.4 Rust Toolchain
- **Rust:** Rust 1.95+ (tested on Rust 1.97.1).
- **Android Targets:**
  ```bash
  rustup target add aarch64-linux-android
  rustup target add x86_64-linux-android
  ```
- **cargo-ndk:**
  ```bash
  cargo install cargo-ndk
  ```

---

## 2. Configuration (`local.properties`)

Create or update `apps/pdfcraft-android/android/local.properties`:

```properties
sdk.dir=/opt/homebrew/share/android-commandlinetools
ndk.dir=/opt/homebrew/share/android-commandlinetools/ndk/29.0.14206865
```

---

## 3. Building the Project

All Gradle commands must be run from the `apps/pdfcraft-android/android` directory.

### 3.1 Build the Native Rust Library (`libpdfcraft_android.so`)
To compile the native library with `cargo-ndk`:

```bash
cd apps/pdfcraft-android/android
./gradlew cargoBuildDebug -Ppdfcraft.abis=arm64-v8a
```
For both ARM64 and x86_64:
```bash
./gradlew cargoBuildDebug
```

The native shared object is placed into:
`app/build/generated/jniLibs/cargoBuildDebug/<abi>/libpdfcraft_android.so`

### 3.2 Generate UniFFI Kotlin Bindings
Generates `pdfcraft_android.kt` in package `io.github.dikshant363.pdfcraft.core`:

```bash
./gradlew uniffiBindgenDebug
```

### 3.3 Build the Complete Debug APK
To assemble the debug APK:

```bash
./gradlew assembleDebug
```

Output APK location:
```text
apps/pdfcraft-android/android/app/build/outputs/apk/debug/app-debug.apk
```

To build targeting only a specific ABI during rapid local development:
```bash
./gradlew assembleDebug -Ppdfcraft.abis=arm64-v8a
```

### 3.4 Build Release APK
A signed or unsigned release APK can be built with:

```bash
./gradlew assembleRelease -Ppdfcraft.unsigned=true
```

Output APK location:
```text
apps/pdfcraft-android/android/app/build/outputs/apk/release/app-release-unsigned.apk
```

---

## 4. Running Tests

### 4.1 Native Host Unit Tests (Rust)
Tests the core mobile bridge, forms, JS evaluation, sound attachments, signatures, and tile rendering:

```bash
cargo test -p pdfcraft-android
```

### 4.2 Android Unit Tests (Kotlin / Compose)
Tests viewer geometry, tile layout, speech synthesizer locale resolution, comment grouping, and shape snapping:

```bash
cd apps/pdfcraft-android/android
./gradlew testDebugUnitTest
```

### 4.3 Asset and License Compliance
Verifies that all bundled icons, fonts, and assets have valid SPDX attribution and matching SHA-256 hashes:

```bash
cargo xtask assets
```

---

## 5. Reproducible Builds

The Gradle build script enforces build reproducibility:
- Paths are remapped via `--remap-path-prefix=/pdfcraft` and `--remap-path-prefix=/cargo` in Rust compilation.
- Version control information and dependency signing metadata are excluded from the output APKs.
- Baseline profile non-deterministic artifacts are disabled.
