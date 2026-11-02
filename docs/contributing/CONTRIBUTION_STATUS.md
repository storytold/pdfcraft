# Upstream Contribution Status Dashboard

This live status tracker records all upstream contributions, branches, commits, test results, and Pull Requests between `storytold/pdfcraft` and `dikshant363/pdfcraft-android`.

---

## 1. Branch and Repository Map

- **Upstream:** `https://github.com/storytold/pdfcraft` (`main` @ `0cba87ae`)
- **Origin Fork:** `https://github.com/dikshant363/pdfcraft-android`
- **Active Branches:**
  - `main`: Clean sync with `upstream/main`
  - `android/development`: Working branch for Android product development
  - `android-port`: Integration commit `fa9984a4`
  - `feature/android-reader`: Reader implementation
  - `feature/android-rendering`: Rendering implementation
  - `upstream/automation-apply-alerts`: Dedicated branch for PR 1
  - `upstream/sound-annotations`: Dedicated branch for PR 2

---

## 2. Contribution Items Status

| Contribution / Topic | Type | Topic Branch | Commit | Tests & Status | PR Status | Blocker / Next Action |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **PR 1: Automation Apply Alerts**<br>Surface AcroForm calculation alerts & requests in `apply` | `fix(automation)` | `upstream/automation-apply-alerts` | Prepared from `android-port` | • `cargo test -p pdfcraft-automation`: 318 passed<br>• Regression test passed | **DRAFT READY** | Ready to create branch from `upstream/main` and stage commit |
| **PR 2: ISO 32000-2 Sound Annotations**<br>`Shape::Sound`, appearance stream vector mic icon, WAV parsing | `feat(annot)` | `upstream/sound-annotations` | Prepared from `android-port` | • `cargo test -p pdfcraft-annot`: passed<br>• `cargo test -p pdfcraft-automation`: 318 passed<br>• `cargo xtask assets`: passed | **DRAFT READY** | Ready to create branch from `upstream/main` and stage commit |
| **RFC Proposal: Native Android Support**<br>Architecture proposal issue for upstream discussion | `rfc(platform)` | N/A | Documented in `PROPOSAL_ISSUE_DRAFT.md` | Complete architectural review | **DRAFT READY** | Awaiting user authorization before publishing to GitHub |
| **Track A: Native Android Application**<br>Complete Kotlin + Compose + UniFFI Android App | Product | `android/development` | `fa9984a4` | • 54 Android JVM unit tests passed<br>• 17 Native host tests passed<br>• Multi-ABI APK generated (`app-debug.apk`: 63MB) | **VERIFIED (Fork)** | Independent product track running in fork |

---

## 3. Pull Request Submission Log

| PR # | Title | Branch | Target | Status | Verification Date |
| :--- | :--- | :--- | :--- | :--- | :--- |
| *TBD* | `fix(automation): surface form calculation alerts and requests in apply` | `upstream/automation-apply-alerts` | `storytold/pdfcraft:main` | Staged locally | 2026-11-02 |
| *TBD* | `feat(annot): implement sound annotations (voice comments) per ISO 32000-2` | `upstream/sound-annotations` | `storytold/pdfcraft:main` | Staged locally | 2026-11-02 |

---

## 4. Current Blockers and Required User Authorizations

1. **GitHub Issue Publishing:**
   - Maintainer RFC proposal is drafted at `docs/contributing/PROPOSAL_ISSUE_DRAFT.md`.
   - **Action Required:** User authorization needed to publish this issue on `storytold/pdfcraft`.
2. **Upstream PR Creation:**
   - Local topic branches `upstream/automation-apply-alerts` and `upstream/sound-annotations` are prepared and isolated from Android-specific code.
   - Pushing and opening PRs on GitHub requires network push and user confirmation.
