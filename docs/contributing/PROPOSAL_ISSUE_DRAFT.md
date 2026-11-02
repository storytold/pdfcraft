# [RFC Proposal] Native Android Support for PdfCraft via UniFFI and Jetpack Compose

> **Note for Contributor:** This is a draft RFC issue prepared for `storytold/pdfcraft`. **DO NOT PUBLISH** on GitHub without explicit user authorization.

---

### Title:
`[RFC] Native Android Support for PdfCraft via UniFFI and Jetpack Compose`

### Issue Body:

#### 1. Why Android Support Belongs in PdfCraft
PdfCraft's mission is to provide an open-source, clean-room, memory-safe PDF workstation targeting Adobe Acrobat Pro parity. Today, mobile PDF readers on Android are dominated by proprietary, closed-source viewers (often laden with cloud subscriptions, telemetry, and tracking) or outdated C/C++ rendering engines (MuPDF, PDFium, Poppler) susceptible to memory corruption vulnerabilities.

A native Android port of PdfCraft provides:
- **100% Offline-First Security:** Zero network permissions (`android.permission.INTERNET` omitted), providing verifiable privacy for sensitive documents.
- **Memory Safety:** Leveraging PdfCraft's `#![forbid(unsafe_code)]` Rust engine prevents remote code execution vulnerabilities common in legacy mobile PDF engines.
- **Acrobat Parity on Mobile:** Advanced AcroForm interactive form filling with sandboxed Boa JavaScript calculation, digital signatures, vector annotations, and offline OCR.

---

#### 2. Architecture & Reuse of the Rust Engine
The proposed architecture keeps all PDF parsing, editing, rendering, and cryptographic logic strictly inside the existing Rust engine crates:

```
┌────────────────────────────────────────────────────────┐
│     Native Android UI (Kotlin + Jetpack Compose M3)    │
│    (SAF Pickers, Gesture Controller, Coroutines, M3)   │
└───────────────────────────▲────────────────────────────┘
                            │ JNA / Kotlin Coroutines
┌───────────────────────────▼────────────────────────────┐
│      UniFFI Mobile Bridge (`apps/pdfcraft-android`)    │
│    (MobileSession, Tile Canvas, Panic Isolation Guard) │
└───────────────────────────▲────────────────────────────┘
                            │ Rust Internal APIs
┌───────────────────────────▼────────────────────────────┐
│              Existing PdfCraft Engine Crates           │
│  (pdfcraft-engine, render, annot, js, crypt, filters)  │
└────────────────────────────────────────────────────────┘
```

- **Zero Kotlin PDF Logic:** Kotlin handles only Android UI, touch gestures, and Storage Access Framework interactions. All PDF parsing, form recalculations, and rasterization remain in Rust.
- **No WebViews or Cross-Platform Frameworks:** Built with Jetpack Compose and Material 3 for native Android responsiveness and accessibility.

---

#### 3. Native Bridge: UniFFI vs Direct JNI
We evaluated direct JNI (`jni` crate) versus UniFFI:
- **Selected: UniFFI (Library Mode):**
  - Generates type-safe Kotlin data classes, enums, and coroutine-friendly interfaces from Rust UDL/Proc-macros.
  - Automatic memory management and thread-safe reference counting.
  - Every bridge method is wrapped in `std::panic::catch_unwind(AssertUnwindSafe(...))` so an engine panic can never crash the host Android process.

---

#### 4. Android-Specific Storage & Rendering Architecture
- **Storage Access Framework (SAF):** Android does not grant direct file paths. Documents are opened via scoped file descriptors (`ParcelFileDescriptor`), copied to private sandbox cache, and saved back atomically upon user request.
- **Multi-Scale Tile Rendering:**
  - Viewports are divided into 512×512 pixel tiles rendered asynchronously on worker threads.
  - Octave-based zoom levels prevent full-document memory explosion.
  - LRU memory-budgeted caching automatically frees off-screen tiles under memory pressure.

---

#### 5. Supported Platforms & CPU Architectures
- **Minimum Android Version:** Android 12 (API 31).
- **Target Android Version:** Android 15 (API 35/37).
- **CPU Architectures (ABIs):**
  - Primary: `arm64-v8a` (standard modern Android devices).
  - Secondary: `x86_64` (Android emulators and ChromeOS tablets).
  - Optional: `armeabi-v7a` (legacy 32-bit devices).

---

#### 6. Testing, CI & Maintenance Responsibilities
- **Automated Verification:**
  - `cargo test -p pdfcraft-android`: 17 host tests verifying tile rendering, voice comments, JS forms, and CMS signatures.
  - Android JVM unit tests (`./gradlew testDebugUnitTest`): 54 tests covering tile geometry, speech locale resolution, and comment drag models.
- **CI Considerations:**
  - Native cross-compilation is handled via `cargo-ndk`.
  - Android builds can be isolated in a dedicated workflow or kept in the fork if maintainers prefer not to expand root CI runtimes.
- **Maintenance Commitment:**
  - As contributor `@dikshant363`, I will commit to ongoing maintenance of the Android bridge, keeping it synchronized with upstream engine releases.

---

#### 7. Maintainer Feedback Requested
We welcome the maintainers' feedback on:
1. Would you prefer in-tree support for the `apps/pdfcraft-android` UniFFI bridge crate, or should the mobile application remain in an independent fork while upstreaming only reusable engine/automation improvements?
2. Are there any specific API or layering boundaries you would prefer between `crates/engine` and mobile consumers?

Thank you for your guidance and for building PdfCraft!
