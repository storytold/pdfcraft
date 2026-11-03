# Software Licence and Attribution Report — PdfCraft Android v0.18.0-rc.1

**Application Identifier:** `io.github.dikshant363.pdfcraft`  
**Licence:** MIT OR Apache-2.0 (Dual License)  
**Audit Verification:** `cargo xtask assets` (PASS), `cargo xtask license-check` (PASS)  

---

## 1. Project Licensing Overview

PdfCraft Android is licensed under the terms of either:
- **The MIT License** (`LICENSE-MIT`), or
- **The Apache License, Version 2.0** (`LICENSE-APACHE`).

All source code authored for the Android port in `apps/pdfcraft-android/` and related crates conforms strictly to these licensing obligations.

---

## 2. Third-Party Licenses and Attributions

### 2.1 Kotlin and Android Dependencies
- **AndroidX (`core-ktx`, `compose`, `lifecycle`, `activity-compose`, `exifinterface`):** Licensed under **Apache License 2.0**.
- **Kotlin Standard Library & Coroutines (`kotlinx-coroutines-android`):** Licensed under **Apache License 2.0**.
- **Java Native Access (`net.java.dev.jna:jna`):** Licensed under **Apache License 2.0**.

### 2.2 Native Rust Dependencies
- **UniFFI (`uniffi`, `uniffi_core`, `uniffi_bindgen`):** Licensed under **Mozilla Public License 2.0 (MPL-2.0)**.
- **`lopdf`:** Licensed under **MIT License**.
- **`serde_json`, `thiserror`, `log`:** Licensed under **MIT OR Apache-2.0**.
- **`hayro`, `hayro-interpret`, `hayro-syntax`, `hayro-jbig2`:** Clean-room PDF engines licensed under **MIT OR Apache-2.0**.
- **`boa_engine` (JavaScript calculations):** Licensed under **MIT OR Apache-2.0**.

### 2.3 Bundled Assets, Icons & Typography
- **Lucide Icons:** Dual-licensed under **ISC AND MIT** (Lucide Contributors, portions Cole Bemis, Feather).
- **Inter Font:** Licensed under the **SIL Open Font License 1.1 (OFL-1.1)**.
- **Dancing Script Font:** Licensed under the **SIL Open Font License 1.1 (OFL-1.1)**.
- **Foxit / CMaps Data:** Licensed under **BSD-3-Clause**.

---

## 3. Clean-Room and Trademark Compliance Attestation

Per repository policy (`AGENTS.md` §1):
1. **Zero Adobe Materials:** No icons, images, artwork, fonts, cursors, or sample documents from Adobe products are included or referenced.
2. **Zero ArtCraft Trademarks:** No proprietary trademarks, brand graphics, or logos from ArtCraft are packaged within the Android APK or AAB.
3. **Provable Licenses:** All 323 repository assets are recorded with verified SHA-256 hashes in `ATTRIBUTION.toml`.
