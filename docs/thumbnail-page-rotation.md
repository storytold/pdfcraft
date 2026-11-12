# Rotate pages from thumbnails

> **Last updated:** 2026-11-12 · **Change:** minor · **Status:** implemented and covered by a UI regression (#7052)

In the Pages panel, right-click a thumbnail and choose Rotate clockwise or Rotate counterclockwise. The action changes the document's page rotation by 90 degrees, rather than rotating only the reader's view. It uses the existing page-assembly permissions and undo path.

A right-click inside the selected pages keeps that selection and rotates all of it. A right-click outside it targets the clicked page. Each choice is one Undo step. The header's Page tools menu remains available.

Evidence: `crates/ui-egui/tests/it/editing.rs::rotating_pages_from_the_thumbnail_menu_is_undoable`, exercised against PdfCraft 0.21.0. It covers the multi-selection, Undo and clicking a different thumbnail; without the new items it fails because the clockwise action cannot be found.

## Revision history

| Date | Change | Description |
|---|---|---|
| 2026-11-12 | minor | Add thumbnail context rotation and the regression evidence |
