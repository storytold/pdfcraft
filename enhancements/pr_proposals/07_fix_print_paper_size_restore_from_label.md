# Pull Request: fix(print): restore previous paper size after leaving Label mode (#3564)

## Summary
In `crates/ui-egui/src/print_ui.rs`, selecting "Label" in "Page Sizing & Handling" automatically sets `d.paper = print::LABEL_PAPER` (4x6 in). However, when the user switched away from Label back to "Size", "Poster", "Multiple", or "Booklet", the paper size remained stuck on 4x6 in instead of reverting to their previously selected paper (e.g. Letter or A4).

## Changes
- `crates/ui-egui/src/print_ui.rs`:
  - Added `pub saved_paper: Option<usize>` to `PrintDraft`.
  - When entering `Handling::Label`, save previous `d.paper` to `d.saved_paper`.
  - When switching away from `Handling::Label`, restore `d.paper = d.saved_paper.take()`.
  - Added test `leaving_label_mode_restores_previous_paper_size`.
