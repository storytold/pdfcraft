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
| **PR 1: Automation Apply Alerts**<br>Surface AcroForm calculation alerts & requests in `apply` | `fix(automation)` | `upstream/automation-apply-alerts` | `8246d8ce` | • `cargo test -p pdfcraft-automation`: 318 passed<br>• Regression test passed | **READY TO SUBMIT** | Pushed to origin; awaiting authorization to open PR against `storytold/pdfcraft:main` |
| **PR 2: ISO 32000-2 Sound Annotations**<br>`Shape::Sound`, appearance stream vector mic icon, WAV parsing | `feat(annot)` | `upstream/sound-annotations` | `58dbec31` | • `cargo test -p pdfcraft-annot`: passed<br>• `cargo test -p pdfcraft-automation`: 318 passed<br>• `cargo xtask assets`: passed | **READY TO SUBMIT** | Pushed to origin; awaiting authorization to open PR against `storytold/pdfcraft:main` |
| **RFC Proposal: Native Android Support**<br>Architecture proposal issue for upstream discussion | `rfc(platform)` | N/A | Documented in `PROPOSAL_ISSUE_DRAFT.md` | Complete architectural review | **DRAFT READY** | Awaiting user authorization before publishing issue to `storytold/pdfcraft` |
| **Track A: Native Android Application**<br>Complete Kotlin + Compose + UniFFI Android App | Product | `android/development` | `f35a18fe` | • 54 Android JVM unit tests passed<br>• 17 Native host tests passed<br>• Multi-ABI APK generated (`app-debug.apk`: 63MB) | **VERIFIED (Fork)** | Independent product track running in fork (`origin/android/development`) |

---

## 3. Pull Request Submission Log

| PR # | Title | Branch | Target | Status | Verification Date |
| :--- | :--- | :--- | :--- | :--- | :--- |
| *TBD* | `fix(automation): surface form calculation alerts and requests in apply` | `upstream/automation-apply-alerts` | `storytold/pdfcraft:main` | Staged locally | 2026-11-02 |
| *TBD* | `feat(annot): implement sound annotations (voice comments) per ISO 32000-2` | `upstream/sound-annotations` | `storytold/pdfcraft:main` | Staged locally | 2026-11-02 |

---

## 4. Current Status and Required User Authorizations

1. **Fork Synchronization:**
   - Remote `origin` (`https://github.com/dikshant363/pdfcraft-android`) created and synchronized.
   - All topic and integration branches (`android/development`, `android-port`, `feature/android-reader`, `feature/android-rendering`, `upstream/automation-apply-alerts`, `upstream/sound-annotations`) are pushed to `origin`.

2. **Upstream PR Submission:**
   - PR 1 (`upstream/automation-apply-alerts` -> `storytold/pdfcraft:main`) is verified and ready.
   - PR 2 (`upstream/sound-annotations` -> `storytold/pdfcraft:main`) is verified and ready.
   - **Action Required:** User authorization to open the PRs against `storytold/pdfcraft:main` via GitHub CLI or web UI.

3. **RFC Proposal Publishing:**
   - RFC proposal is drafted at `docs/contributing/PROPOSAL_ISSUE_DRAFT.md`.
   - **Action Required:** User authorization needed to publish this discussion issue on `storytold/pdfcraft`.
