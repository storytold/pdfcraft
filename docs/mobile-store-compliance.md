# Mobile Store Compliance Guide (iOS & Android)

This document records store compliance declarations, privacy manifests, permissions, and binary architecture verifications for PdfCraft on iOS and Android (MOB-7.2).

---

## 1. Apple App Store Compliance

### 1.1 Privacy Manifest (`PrivacyInfo.xcprivacy`)

The privacy manifest is bundled in:
- `mobile/ios/PdfCraftKit/Sources/PdfCraftKit/Resources/PrivacyInfo.xcprivacy` (SDK framework)
- `mobile/ios/PdfCraft/PrivacyInfo.xcprivacy` (application target)

#### Declarations
- **Tracking (`NSPrivacyTracking`):** `false`. PdfCraft does not track users, link user identities across third-party apps, or share data with data brokers.
- **Tracking Domains (`NSPrivacyTrackingDomains`):** Empty (`[]`). No analytics or advertising endpoints exist in PdfCraft.
- **Collected Data Types (`NSPrivacyCollectedDataTypes`):** Empty (`[]`). PdfCraft collects zero personal data, diagnostic data, usage data, or device identifiers.
- **Required Reason APIs (`NSPrivacyAccessedAPITypes`):**
  1. `NSPrivacyAccessedAPICategoryFileTimestamp`:
     - **Declared Reason:** `C617.1` (Accessing modification timestamps of files inside the app's container sandbox for document autosave, session recovery, and cache eviction).
  2. `NSPrivacyAccessedAPICategoryDiskSpace`:
     - **Declared Reason:** `E174.1` (Checking available disk space to prevent data loss when saving PDF files or writing disk thumbnail caches).

### 1.2 Export Compliance & Cryptography
- PdfCraft uses standard encryption for PDF password protection (AES-256-CBC, AES-128, RC4) and digital signatures (RSA-PSS, ECDSA P-256/P-384, SHA-256) via pure Rust (`aws-lc-rs`).
- Under US EAR §740.17(b)(1) / French ANSSI / Apple Export Compliance rules, standard consumer document software using general-purpose cryptographic primitives qualifies for exemption (self-classification report or standard exemption indicator `ITSAppUsesNonExemptEncryption = NO` in `Info.plist`).

---

## 2. Google Play Store Compliance

### 2.1 Play Console Data Safety Form

| Section | Question | Answer | Details |
|---|---|---|---|
| **Data collection** | Does your app collect or share any user data? | **No** | PdfCraft operates 100% locally on-device. Zero telemetry, crash reporters, or analytics SDKs are embedded. |
| **Data sharing** | Is data shared with third parties? | **No** | No external APIs, trackers, or SDKs are linked. |
| **Security practices** | Is data encrypted in transit? | **N/A** | No data is transmitted over the network. |
| | Can users request data deletion? | **Yes** | All document data resides solely in local device storage and is removed immediately upon app uninstallation or document deletion by the user. |

### 2.2 Permissions Audit

The Android manifests:
- `mobile/android/app/src/main/AndroidManifest.xml`
- `mobile/android/pdfcraft-kit/src/main/AndroidManifest.xml`

Declare **zero dangerous or runtime permissions**:
- No `android.permission.INTERNET` (strictly offline by default).
- No `android.permission.READ_EXTERNAL_STORAGE` or `WRITE_EXTERNAL_STORAGE` (uses Android Storage Access Framework via `ActivityResultContracts.OpenDocument` and `CreateDocument`).
- No `android.permission.CAMERA` (document import utilizes system Photo Picker `PickVisualMedia` which requires zero camera permissions).

---

## 3. Android 16 KB Page-Size Verification

Android 15 introduces support for devices configured with 16 KB page sizes. All native `.so` libraries in Android apps must be built with 16 KB alignment.

### 3.1 Toolchain Flags
`cargo xtask mobile-android` sets linker arguments:
```bash
RUSTFLAGS="-C link-arg=-z -C link-arg=max-page-size=16384"
```

### 3.2 Verification
Run the following inspection on built native binaries:
```bash
readelf -l target/aarch64-linux-android/release/libpdfcraft_ffi.so | grep -A 1 LOAD
```
Every `LOAD` segment must show `Align 0x4000` (16384 bytes).
