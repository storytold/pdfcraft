# Software Bill of Materials (SBOM) — PdfCraft Android v0.18.0-rc.1

**Application Identifier:** `io.github.dikshant363.pdfcraft`  
**Version:** `0.18.0-rc.1` (Version Code: `1800010`)  
**Specification:** SPDX 2.3 / CycloneDX Alignment  
**Target Architecture:** `arm64-v8a`, `x86_64`  
**Minimum Android API:** 31 (Android 12)  
**Target Android API:** 36 (Android 15)  

---

## 1. Top-Level Architectural Components

| Component | Language / Ecosystem | Version | License | Role |
| :--- | :--- | :--- | :--- | :--- |
| **`io.github.dikshant363.pdfcraft`** | Kotlin / Jetpack Compose | 0.18.0-rc.1 | MIT OR Apache-2.0 | Android Native User Interface & Document Controller |
| **`pdfcraft-android`** | Rust | 0.18.0 | MIT OR Apache-2.0 | Native C-ABI / UniFFI JNI bridge shared library |
| **`pdfcraft-engine`** | Rust | 0.18.0 | MIT OR Apache-2.0 | Core PDF document model, layout, and tool execution |
| **`pdfcraft-render`** | Rust | 0.18.0 | MIT OR Apache-2.0 | High-fidelity software and tiled rendering pipeline |
| **`pdfcraft-automation`** | Rust | 0.18.0 | MIT OR Apache-2.0 | Headless tool execution, scripts, and AcroForm calculations |

---

## 2. Android Runtime & JVM Dependencies

Declared in `apps/pdfcraft-android/android/gradle/libs.versions.toml`:

| Group | Artifact | Version | License | Function |
| :--- | :--- | :--- | :--- | :--- |
| `androidx.core` | `core-ktx` | 1.17.0 | Apache-2.0 | Android platform extensions & compatibility |
| `androidx.activity` | `activity-compose` | 1.11.0 | Apache-2.0 | Activity Compose integration & back handling |
| `androidx.compose.ui` | `ui` | 2026.09.00 | Apache-2.0 | Declarative Compose UI toolkit |
| `androidx.compose.ui` | `ui-graphics` | 2026.09.00 | Apache-2.0 | Hardware-accelerated graphics primitives |
| `androidx.compose.material3` | `material3` | 2026.09.00 | Apache-2.0 | Material Design 3 components & styling |
| `androidx.lifecycle` | `lifecycle-runtime-compose` | 2.9.4 | Apache-2.0 | Lifecycle-aware Compose state management |
| `androidx.lifecycle` | `lifecycle-viewmodel-compose` | 2.9.4 | Apache-2.0 | ViewModel Compose bindings |
| `org.jetbrains.kotlinx` | `kotlinx-coroutines-android` | 1.10.2 | Apache-2.0 | Asynchronous IO & background task dispatch |
| `net.java.dev.jna` | `jna` | 5.19.1 | Apache-2.0 | Java Native Access for UniFFI C-ABI dispatch |
| `androidx.exifinterface` | `exifinterface` | 1.4.1 | Apache-2.0 | Camera scan EXIF orientation parsing |

---

## 3. Rust Native Engine Dependencies

Compiled directly into `libpdfcraft_android.so`:

| Crate | Version | License | Function |
| :--- | :--- | :--- | :--- |
| `uniffi` | 0.32.2 | MPL-2.0 | Multi-language foreign function interface generator |
| `lopdf` | 0.35.0 | MIT | Low-level PDF object serialization and parser |
| `serde_json` | 1.0.140 | MIT OR Apache-2.0 | Tool payload serialization & command deserialization |
| `thiserror` | 2.0.18 | MIT OR Apache-2.0 | Strongly-typed Rust error handling |
| `log` | 0.4.26 | MIT OR Apache-2.0 | Log facade for Android logcat integration |
| `hayro` | 0.7.1 | MIT OR Apache-2.0 | PDF graphics and content stream interpreter |
| `hayro-jbig2` | 0.3.0 | MIT OR Apache-2.0 | Clean-room JBIG2 bi-level image decompressor |
| `boa_engine` | 0.22.0 | MIT OR Apache-2.0 | Embedded ECMAScript engine for AcroForm calculations |

---

## 4. Bundled Assets, Icons & Typography

Fully attributed in `ATTRIBUTION.toml` and verified via `cargo xtask assets`:

| Asset Group | Count | License | Source / Upstream |
| :--- | :--- | :--- | :--- |
| **Lucide Icons** | 34 SVGs | ISC AND MIT | Lucide Contributors (`lucide-static 1.49.0`) |
| **Inter Font** | Regular/Bold | OFL-1.1 | Rasmus Andersson |
| **Dancing Script Font** | Regular/Bold | OFL-1.1 | Impallari Type |
| **Standard-14 Metrics** | 14 tables | BSD-3-Clause | ISO 32000-2 font metric specification data |
