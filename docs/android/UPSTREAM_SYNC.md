# Upstream Synchronization Strategy

This guide describes how to maintain synchronization between **PdfCraft Android** (`https://github.com/dikshant363/pdfcraft-android`) and upstream **PdfCraft** (`https://github.com/storytold/pdfcraft`).

---

## 1. Architectural Isolation

To minimize merge conflicts with upstream releases:
1. **App Isolation:** The entire Android application sits inside `apps/pdfcraft-android/`. Upstream does not touch this path.
2. **Minimal Upstream Touchpoints:** Core engine modifications are kept minimal:
   - `crates/annot/src/types.rs`, `create.rs`, `appearance.rs`: `Shape::Sound` variant for voice comment annotations.
   - `crates/automation/src/comments.rs`: PCM WAV audio sample conversion (`wav_samples`).
   - `crates/automation/src/documents.rs`: Form script alert forwarding in `Automation::apply`.
   - `crates/engine/src/edit/labels.rs`: `Shape::Sound` undo label mapping.
   - `assets/icons/`: 34 additional Lucide ISC icons for mobile navigation.
   - `ATTRIBUTION.toml`: Corresponding attribution records.

---

## 2. Synchronization Procedure

### Step 1: Fetch Upstream Changes
```bash
git checkout android-port
git fetch upstream main --tags
```

### Step 2: Merge or Rebase
Rebase is preferred to keep a clean, linear history on top of upstream releases:

```bash
git rebase upstream/main
```

Or merge if maintaining explicit merge commits:
```bash
git merge upstream/main -m "Merge upstream changes into android-port"
```

### Step 3: Resolve Upstream Conflicts
Check if any conflicts occur in:
- `Cargo.lock` or `Cargo.toml`: Re-run `cargo check` and resolve workspace version bumps.
- `crates/automation/src/documents.rs`: Verify that `take_js_output` forwarding remains in `apply`.
- `crates/annot`: Verify that `Shape::Sound` arms remain present in match blocks.
- `ATTRIBUTION.toml`: Ensure Android Lucide icon entries are preserved.

### Step 4: Verification Suite
Run the full verification battery after any upstream sync:

```bash
# 1. Verify asset compliance
cargo xtask assets

# 2. Run engine automation tests
cargo test -p pdfcraft-automation

# 3. Run Android native host tests
cargo test -p pdfcraft-android

# 4. Build and test the Android app
cd apps/pdfcraft-android/android
./gradlew testDebugUnitTest
./gradlew assembleDebug
```

### Step 5: Push Updates
```bash
git push origin android-port
```
