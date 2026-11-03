# PdfCraft Android v0.17.0-rc.1 Release Notes

**Version Name:** `0.17.0-rc.1`  
**Version Code:** `1700010` (Multi-ABI) / `1700012` (arm64-v8a) / `1700013` (x86_64)  
**Package Name:** `io.github.dikshant363.pdfcraft`  
**Minimum Android:** Android 12 (API level 31)  
**Target Android:** Android 15 (API level 36)  
**Release Date:** 2026-11-03  

---

## 1. Overview

PdfCraft Android v0.17.0-rc.1 is the first official release candidate of the native Android port of PdfCraft.

Built completely from scratch using Kotlin and Jetpack Compose on top of the open-source Rust PDF engine via high-performance UniFFI bindings, PdfCraft Android delivers a fast, privacy-first, fully offline PDF viewing and editing workstation for mobile devices without using any WebViews, Flutter, or third-party cloud backends.

---

## 2. Key Features

- **Native High-Fidelity PDF Viewing:**
  - Continuous vertical page scrolling and single-page presentation modes.
  - Tiled 256x256 rendering pipeline with viewport culling for ultra-smooth pinch-to-zoom (up to 5.0x) on large engineering drawings and publications.
  - Native dark mode and color inversion.
  - Password-protected PDF opening.
- **Fast In-Memory Search & Navigation:**
  - Instant word-by-word search with hit highlighting and sequential navigation across pages.
  - Document outline / Table of Contents hierarchy navigator.
  - Page grid thumbnail viewer and direct jump bar.
- **Annotations & Markup Tools:**
  - Standard text markup: Highlight, Underline, Strikeout, and Squiggly.
  - Sticky notes, free-text annotations, and comment replies.
  - Freehand ink drawings with pressure simulation, smoothing, and geometric shape recognition (rectangles, ellipses, arrows, lines).
  - Voice comments / sound annotations powered by a bounded clean-room RIFF/WAVE parser.
  - Document attachments viewer and extractor.
- **Interactive Forms & Signatures:**
  - Fill text fields, checkboxes, radio groups, and drop-down choice lists.
  - Client-side JavaScript calculations and automatic recalculation cascades.
  - Surface modal calculation alerts and prompt requests.
  - Digital signatures (PKCS#7 PAdES) and custom drawn stamp signatures.
- **Page Organization:**
  - Reorder, rotate (90°, 180°, 270°), extract, and delete pages.
  - Transactional undo and redo for all modifications with atomic `doc_save`.
- **Accessibility & Speech:**
  - Integrated Read Aloud text-to-speech with word-level highlight synchronization.
- **Privacy & Security First:**
  - Zero network permissions (`android.permission.INTERNET` is completely absent).
  - 100% offline document workflows integrated with Android Storage Access Framework (SAF).

---

## 3. Release Artifacts & Checksums

| File | Target Architecture | Size | SHA-256 Digest |
| :--- | :--- | :--- | :--- |
| **`app-release-unsigned.apk`** | Universal (`arm64-v8a`, `x86_64`) | 70 MiB | `7707e5f13c2551d97f01795ec0876a32763d04ac3c4a351eccfbee67719d9224` |
| **`app-release.aab`** | Google Play App Bundle | 67 MiB | `f86952c0fdff09e541baad0eb32a3c8adc17750107fe73ced06cc7230a6095f1` |
| **`app-debug.apk`** | Universal (Debug) | 135 MiB | `5eee54dfc33a02d4f3fe19ebdb780b0009b99c43478cd46301ca5f7e449f2434` |
| **`app-debug-androidTest.apk`** | On-Device Test Runner | 2.3 MiB | `f5bf09718ad6b338c91c4b394fe22ac0b75cc071d80efab9c387b21f258ab5bc` |

---

## 4. Installation and Verification

### 4.1 F-Droid / Manual Signing Instructions

To sign the unsigned release candidate APK with your own release keystore:

```bash
apksigner sign --ks release.keystore \
  --ks-key-alias my-key-alias \
  --in app-release-unsigned.apk \
  --out app-release-signed.apk

# Verify signature
apksigner verify --verbose app-release-signed.apk
```

### 4.2 Sideloading via ADB

```bash
adb install -r app-release-signed.apk
```
