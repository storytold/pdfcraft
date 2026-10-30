# Mobile Release Checklist and Rollback Procedure

This runbook guides maintainers through releasing and rolling back mobile builds of PdfCraft (MOB-7.7).

---

## 1. Versioning Scheme

- Mobile releases are tagged as `mobile-v<semver>` (e.g., `mobile-v0.7.0`).
- The mobile version tracks the core workspace version specified in the root `Cargo.toml`.
- Both iOS (`CFBundleShortVersionString` in Xcode / SPM) and Android (`versionName` / `versionCode` in Gradle) derive from this version.

---

## 2. Pre-Release Verification Checklist

Execute these verification gates in clean checkout before cutting a release:

1. **Format and Lints:**
   ```bash
   cargo fmt --check
   cargo clippy --workspace -- -D warnings
   ```
2. **Full Workspace & Engine Test Suite:**
   ```bash
   cargo test --workspace
   ```
3. **Architecture Layering:**
   ```bash
   cargo xtask layers
   ```
4. **Mobile FFI Contract & API Surface Match:**
   ```bash
   cargo xtask mobile-bindings --check
   ```
5. **Acrobat Parity Integrity:**
   ```bash
   cargo xtask parity
   ```
6. **Binary Footprint Gate (Budget B10):**
   ```bash
   cargo xtask mobile-size
   ```
   *Requirement: Stripped library footprint must not exceed 25.00 MB.*
7. **Asset Policy & Licences (AGENTS.md §1):**
   ```bash
   cargo xtask assets
   ```
8. **Swift Tests & XCFramework Verification:**
   ```bash
   swift test --package-path mobile/ios/PdfCraftKit
   ```
9. **Store Compliance:**
   - Verify `PrivacyInfo.xcprivacy` exists in `mobile/ios/PdfCraftKit/Sources/PdfCraftKit/Resources/` and `mobile/ios/PdfCraft/`.
   - Verify Android 16 KB page-size alignment:
     ```bash
     readelf -l target/aarch64-linux-android/release/libpdfcraft_ffi.so | grep -A 1 LOAD
     ```
   - Verify zero unwanted permissions in Android manifests.

---

## 3. Cutting the Release

1. **Create and push the Git tag:**
   ```bash
   git tag -a mobile-v0.7.0 -m "Release mobile-v0.7.0"
   git push origin mobile-v0.7.0
   ```
2. **Monitor CI Workflow:**
   - GitHub Actions executes `.github/workflows/mobile-release.yml`.
   - iOS job builds `PdfCraftKit-xcframework` and archives `.ipa`.
   - Android job builds 16 KB page-aligned native libraries and produces `.aab`.

---

## 4. Rollback and Hotfix Procedure

If a critical issue or regression escapes to production:

1. **Immediate Phased Rollout Halt:**
   - **Apple App Store:** In App Store Connect, halt phased release immediately or unpublish the affected version if critical.
   - **Google Play:** In Google Play Console, halt staged rollout percentage or promote previous known-good release track.
2. **Regression Reproduction:**
   - Per AGENTS.md §4 (Never Crash), write a synthetic regression test that reproduces the defect before applying code changes.
3. **Prepare Hotfix:**
   - Branch from the release tag: `git checkout -b hotfix/mobile-v0.7.1 mobile-v0.7.0`.
   - Implement the minimal fix with `// SAFETY:` rationale if touching FFI.
   - Confirm all quality gates pass.
4. **Cut Patch Tag:**
   - Tag and push `mobile-v0.7.1` to trigger expedited store review.
