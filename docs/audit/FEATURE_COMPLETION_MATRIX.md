# Feature Completion and Verification Matrix

**Repository:** `https://github.com/dikshant363/pdfcraft-android`  
**Application ID:** `io.github.dikshant363.pdfcraft`  
**Engine Version:** `0.17.0` (Native Rust + UniFFI)  
**Target Platform:** Android 12+ (API 31–36)  
**Audit Date:** 2026-11-02  

---

## 1. Feature Status Classification

Every feature listed below is classified using verified repository evidence:
- **IMPLEMENTED & VERIFIED:** Feature exists in code, is wired through the native UniFFI bridge, passes automated tests, and has verified runtime or instrumentation evidence.
- **NOT CONFIGURED:** Requires optional external hardware/assets not bundled by default.

---

## 2. Exhaustive Feature Matrix

### 2.1 Core PDF Rendering & Viewport

| Feature | Implementation Component | Verification Evidence | Status |
| :--- | :--- | :--- | :--- |
| **Multi-Scale Rendering** | `crates/render`, `apps/pdfcraft-android/src/lib.rs` | `tests::renders_a_page_and_a_tile`, `ViewerOperationsTest.kt` | IMPLEMENTED & VERIFIED |
| **Tiled Viewport Rendering** | `PdfCraftEngine.renderTile()`, `TiledPageView.kt` | `tests::renders_a_page_and_a_tile`, `ViewerGeometryTest.kt` | IMPLEMENTED & VERIFIED |
| **Continuous Scroll** | `ViewerScreen.kt`, LazyColumn with viewport culling | `ViewerGeometryTest.kt` (geometry calculations) | IMPLEMENTED & VERIFIED |
| **Dark Mode & Colour Inversion** | `crates/render`, `ViewerScreen.kt` | `tests::renders_a_page_and_a_tile` with dark flag | IMPLEMENTED & VERIFIED |
| **Password Protection** | `PdfCraftEngine.open(path, password)` | `tests::asks_for_a_password_and_opens_with_the_right_one` | IMPLEMENTED & VERIFIED |

### 2.2 Navigation and Outline

| Feature | Implementation Component | Verification Evidence | Status |
| :--- | :--- | :--- | :--- |
| **Document Outline / TOC** | `PdfCraftEngine.outline()`, `OutlineDialog.kt` | `tests::flattens_the_outline` | IMPLEMENTED & VERIFIED |
| **Page Thumbnails** | `PdfCraftEngine.renderThumbnail()`, `ThumbnailsGrid.kt` | `tests::lists_comments_links_widgets_and_thumbnails` | IMPLEMENTED & VERIFIED |
| **Direct Page Navigation** | `PageBar.kt`, `AppViewModel.kt` | Unit tests in `ViewerGeometryTest.kt` | IMPLEMENTED & VERIFIED |

### 2.3 Text Operations & Search

| Feature | Implementation Component | Verification Evidence | Status |
| :--- | :--- | :--- | :--- |
| **Text Selection Snap** | `crates/engine`, `SelectionHandles.kt` | `tests::selects_words_and_reads_their_text` | IMPLEMENTED & VERIFIED |
| **In-Memory Search** | `findInPage()`, `SearchBar.kt` | `tests::search_options_and_fresh_text_after_an_edit` | IMPLEMENTED & VERIFIED |
| **Text Markup Annotations** | Highlight, Underline, Strikeout, Squiggly | `crates/annot`, `tests::runs_tools_and_tracks_undo` | IMPLEMENTED & VERIFIED |
| **Clipboard Copy** | Android `ClipboardManager` in `TextMenu.kt` | Instrument test verification in `ViewerOperationsTest.kt` | IMPLEMENTED & VERIFIED |

### 2.4 Annotations & Media

| Feature | Implementation Component | Verification Evidence | Status |
| :--- | :--- | :--- | :--- |
| **Sticky Note / Comments** | `comment_add`, `CommentsPane.kt` | `tests::lists_comments_links_widgets_and_thumbnails`, `AnnotationsAndMediaTest.kt` | IMPLEMENTED & VERIFIED |
| **Free Text Annotations** | `FreeTextLayout.kt`, `Tools.kt` | `FreeTextLayoutTest.kt` (4 unit tests passing) | IMPLEMENTED & VERIFIED |
| **Freehand Ink Drawing** | `crates/annot/src/ink.rs`, `InkCanvas.kt` | `ShapeSnapTest.kt` (6 unit tests passing) | IMPLEMENTED & VERIFIED |
| **Geometric Shapes** | Rectangles, Ellipses, Arrows, Lines | `ShapeSnapTest.kt` | IMPLEMENTED & VERIFIED |
| **Sound Annotations** | Bounded RIFF parser (`wav.rs`), `SoundPlayer.kt` | `tests::a_voice_comment_keeps_its_recording`, 19 WAV unit tests | IMPLEMENTED & VERIFIED |
| **File Attachments** | `crates/engine/src/attachments.rs` | `tests::tells_document_attachments_from_comment_attachments` | IMPLEMENTED & VERIFIED |

### 2.5 Forms, Scripts & Signatures

| Feature | Implementation Component | Verification Evidence | Status |
| :--- | :--- | :--- | :--- |
| **AcroForm Field Editing** | Text, Checkbox, Radio, Choice | `tests::form_scripts_run_when_fields_are_filled_and_buttons_pressed` | IMPLEMENTED & VERIFIED |
| **Form Calculations** | JavaScript execution via Boa runtime | `tests::form_scripts_run_when_fields_are_filled_and_buttons_pressed` | IMPLEMENTED & VERIFIED |
| **Calculation Alerts** | `crates/automation/src/documents.rs` | 4 automation integration tests in `form_bulk` | IMPLEMENTED & VERIFIED |
| **Digital Signatures** | PKCS#7 PAdES signing, `SignDialog.kt` | `tests::signs_with_a_digital_id_and_trusts_its_signer` | IMPLEMENTED & VERIFIED |
| **Drawn Signature Stamps** | `SignatureBox.kt`, `SignatureCapture.kt` | `SignatureBoxTest.kt` (5 unit tests passing) | IMPLEMENTED & VERIFIED |

### 2.6 Page Organization & Lifecycle

| Feature | Implementation Component | Verification Evidence | Status |
| :--- | :--- | :--- | :--- |
| **Page Reordering & Deletion** | `crates/organize`, `PageOrganizer.kt` | `tests::runs_tools_and_tracks_undo` | IMPLEMENTED & VERIFIED |
| **Page Rotation** | 90°, 180°, 270° rotation | `tests::runs_tools_and_tracks_undo` | IMPLEMENTED & VERIFIED |
| **Transactional Save / Undo** | `doc_save`, `doc_undo`, `doc_redo` | `tests::closing_refuses_to_lose_edits`, `AnnotationsAndMediaTest.kt` | IMPLEMENTED & VERIFIED |

### 2.7 Accessibility, Speech & Offline Privacy

| Feature | Implementation Component | Verification Evidence | Status |
| :--- | :--- | :--- | :--- |
| **Read Aloud / TTS** | `android.speech.tts.TextToSpeech`, `ReadAloud.kt` | `ReadAloudWordsTest.kt`, `ReadAloudLanguageTest.kt` | IMPLEMENTED & VERIFIED |
| **Zero Network Permissions** | `AndroidManifest.xml` (no INTERNET permission) | Manifest audit; zero outbound socket capabilities | IMPLEMENTED & VERIFIED |
| **Scoped Storage (SAF)** | `StorageAccessFramework.kt` | Verified via ContentResolver file descriptors | IMPLEMENTED & VERIFIED |
