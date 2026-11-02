# PdfCraft Android Security Architecture and Threat Model

This document outlines the security architecture, threat model, and defense-in-depth principles implemented in **PdfCraft Android** (`io.github.dikshant363.pdfcraft`).

---

## 1. Core Security Guiding Principles

1. **Zero Network Attack Surface:**
   - The application does NOT request or declare `android.permission.INTERNET`.
   - No analytics, telemetry, crash reporting, ad SDKs, or cloud backends are included.
   - All PDF parsing, rendering, text recognition (OCR), form calculation, and digital signing run completely offline on the user's device.

2. **Strict Storage Confinement (SAF):**
   - Direct broad storage permissions (`READ_EXTERNAL_STORAGE`, `MANAGE_EXTERNAL_STORAGE`) are NOT requested.
   - All document access occurs through the Android Storage Access Framework (`ACTION_OPEN_DOCUMENT`, `ACTION_CREATE_DOCUMENT`, and `ACTION_OPEN_DOCUMENT_TREE`).
   - The app operates on content URIs and transient scoped file descriptors passed directly into memory-safe native Rust abstractions.

3. **Memory Safety & Sandbox Boundaries:**
   - The PDF engine is written in 100% memory-safe Rust with `#![forbid(unsafe_code)]` enabled across the codebase.
   - No untrusted C/C++ libraries (such as libpdfium, poppler, or mupdf) are linked.
   - Input parsing vulnerabilities (buffer overflows, use-after-free, double-free) inherent to legacy C/C++ PDF engines are structurally eliminated.

---

## 2. Threat Model

| Threat | Target / Vector | Impact | Mitigation Strategy |
| :--- | :--- | :--- | :--- |
| **Malicious PDF Exploit** | Complex PDF parser structures, streams, fonts | Denial of Service, potential code execution | 100% safe Rust (`#![forbid(unsafe_code)]`); `catch_unwind` guards around all top-level native entry points; checked integer arithmetic; bounded recursion. |
| **Network Data Exfiltration** | Malicious document actions, URI launching | Private document leakage over network | `android.permission.INTERNET` is omitted from `AndroidManifest.xml`. External URL actions prompt the user with clear intent confirmation before opening the system browser. |
| **JavaScript Sandbox Escape** | Form calculation scripts, Acrobat JS actions | Host device compromise, arbitrary execution | JavaScript runs inside Boa (a sandboxed ECMAScript engine implemented in safe Rust). No OS syscalls, network APIs, or filesystem primitives are exposed to the JS runtime. |
| **Path Traversal / Overwrite** | Attachment export, file extract tools | Arbitrary file overwrite outside app sandbox | Filename sanitization rejects `..`, path separators, and absolute paths. Files are exported strictly through user-chosen SAF destinations with atomic writing. |
| **Memory Exhaustion (OOM DoS)** | Gigapixel render bombs, decompression bombs | System memory pressure, application crash | Multi-scale tile rendering bounds single render buffers to 512×512 pixels; LRU tile cache is strictly memory-budgeted; image decompression bounds are capped. |
| **Hostile Form Input** | Extreme character sets, XFA overflow scripts | UI freezing or memory bloat | Input length caps; truncation warnings returned cleanly; non-blocking coroutine dispatch. |

---

## 3. Native Interface Security (UniFFI & JNA)

1. **Panic Boundary Containment:**
   - In Rust, panics across the FFI boundary are undefined behavior in C ABI.
   - All UniFFI bridge functions in `apps/pdfcraft-android/src/lib.rs` are wrapped in `catch_unwind(AssertUnwindSafe(...))` converting any unforeseen panic into a structured, typed `Result::Err(PdfCraftError)`.
   - A corrupt PDF cannot bring down the Android host process.

2. **Safe Resource Lifetime Management:**
   - `DocumentId` handles are managed through atomic thread-safe counters.
   - Document closing verifies unsaved state and guarantees timely release of native resources.
   - FFI pointers are never exposed directly to Kotlin code; UniFFI handles handle serialization and deserialization across the JNA boundary safely.

---

## 4. Digital Signatures and Encryption

1. **PKCS#12 Keystores:**
   - Digital certificates are imported exclusively through SAF file pickers and decrypted in-memory.
   - Passwords for protected documents and digital IDs are handled as transient character sequences, never persisted to disk or logs.

2. **Cryptographic Primitives:**
   - Digital signatures and PDF encryption conform to ISO 32000-2 (AES-128, AES-256, RSA-PSS, ECDSA).
   - Signatures are verified mathematically against embedded trust chains without external online OCSP/CRL queries to preserve privacy.

---

## 5. ProGuard & Build Obfuscation

Release builds enable full R8 code shrinking and resource optimization (`isMinifyEnabled = true`, `isShrinkResources = true`).
`proguard-rules.pro` preserves:
- UniFFI JNA interface bindings (`io.github.dikshant363.pdfcraft.core.**`).
- AndroidX Lifecycle and Compose view models.
- Native library entry points.

All unused Android APIs and dead code paths are stripped from the resulting production binary.
