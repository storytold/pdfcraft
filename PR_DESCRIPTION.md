# Vector line editing — Session 1–3

### What this branch does
Adds vector line selection, property editing, and endpoint resizing to the Edit a PDF tool.
- **Session 1 (Batch 1):** `LineSegment` type, `editable_lines()` enumeration skeleton with collinear merging, `hit_test_line` and `paint_line_highlight`, `vector_edit_mode` toggle in the toolbar.
- **Session 2 (Batch 2):** `set_line_properties()` mutation function with pre-validation, Line Properties panel (color, width, dash) in the Edit a PDF sidebar.
- **Session 3 (Batch 3):** Endpoint handles on selected lines (`SelectionHandle::LineStart`, `SelectionHandle::LineEnd`), live drag preview, `update_line_endpoints()` on release.

### What is fully working
- **Line Primitives & Geometry:** `LineSegment` struct, bounding box computation, Euclidean length, and point-to-segment distance calculations (`crates/edit/src/objects.rs`).
- **Collinear Segment Merging:** Collinear reduction algorithm joining adjacent segments sharing width and slope within angular/distance thresholds.
- **Selection & Interaction:** `vector_edit_mode` toggle in the Edit a PDF toolbar, line hover (yellow) and selection (blue) highlights on canvas (`crates/ui-egui/src/canvas.rs`).
- **Endpoint Resize Handles:** Two endpoint handles render at segment start/end (`paint_line_endpoint_handles`), with crosshair cursor on hover/drag (`crates/ui-egui/src/canvas_selection.rs`).
- **Drag Preview:** Endpoint dragging dynamically tracks the pointer in screen space, inverting to PDF user space coordinates via `screen_to_user_point` to display a real-time live preview of the dragged endpoint on the canvas.
- **Line Properties UI:** Color picker, stroke width slider (0.5–20.0 pt), and dash pattern selector in the sidebar panel (`crates/ui-egui/src/editing.rs`).
- **Strict Input Validation:** Both `set_line_properties` and `update_line_endpoints` validate coordinate bounds (`abs <= 1e9`), finite numbers, valid stroke widths, dash lengths (`<= 8`), target kinds, and zero-length prevention.

### What is deferred
- **Content Stream Mutation Path:** Both `set_line_properties()` (line 171) and `update_line_endpoints()` (line 211) in `crates/edit/src/objects.rs` are safely stubbed with `Err(EditError::Invalid(...))` because the `edit` crate currently lacks a content-stream operator splice helper for vector stroke graphics state.
- **Line Segment Enumeration on Real PDFs:** `editable_lines()` currently returns `Vec::new()` (stubbed at line 506 with `TODO(session-1)`) pending content-stream parser stroke operator walking.

### Known limitations
- `editable_lines()` returns empty on real documents until the operator walker is connected; the selection and handle infrastructure is architecturally and visually complete and tested via synthetic line segments.
- `set_line_properties()` and `update_line_endpoints()` return descriptive errors (`"the content stream mutation path is not implemented"`), safely surfaced in the sidebar UI rather than modifying PDF bytes.
- i18n: German translations added in `crates/ui-egui/src/i18n/de.tsv`; other language catalogs remain pending.

### Relationship to PR #1990
This branch overlaps in scope with PR #1990 (*M7: select and edit vector paths*). It should **NOT** be merged while #1990 is open and actively landing. Recommendation:
- Open as a draft PR for architectural review and record-keeping, marking mutation as deferred to follow-up Session 2.5 / rebase onto #1990 once landed.

### Test results
- `cargo check -p pdfcraft-edit -p pdfcraft-ui-egui --release`: **Passed (0 errors)**
- `cargo test -p pdfcraft-edit --release`: **Passed (207 passed, 0 failed)**
- `cargo test -p pdfcraft-ui-egui --release --test vector_line_selection`: **Passed (2 passed, 2 ignored for PageXform fixture)**
- `cargo clippy`: Clean across all whitelisted files
- `cargo fmt --check`: Clean on all whitelisted files

### Files changed across all 3 commits
- `Cargo.lock`
- `crates/edit/src/lib.rs`
- `crates/edit/src/objects.rs`
- `crates/engine/src/lib.rs`
- `crates/ui-egui/Cargo.toml`
- `crates/ui-egui/src/canvas.rs`
- `crates/ui-egui/src/canvas_selection.rs`
- `crates/ui-egui/src/editing.rs`
- `crates/ui-egui/src/i18n/de.tsv`
- `crates/ui-egui/src/panels.rs`
- `crates/ui-egui/tests/vector_line_selection.rs`

