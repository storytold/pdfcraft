# Continuous page display

> **Last updated:** 2026-10-18 · Viewer: optional page gaps.

To read pages as a continuous strip, open **View → Page display**, choose **Single page**,
turn **Continuous scrolling** on, and turn **Show gaps between pages** off. The Page display
button on the viewer rail opens the same choices. The command palette also offers
**Show gaps between pages** as a toggle.

Hiding gaps joins the displayed page edges and removes page shadows. It does not crop or
change the PDF: white margins already in a page remain. Pages of different widths stay
centered. In two-page view, it removes both the gutter within a row and the gap between rows.

For future documents, open **Preferences → Documents and view** and uncheck
**Show gaps between pages**. This default survives a restart. Existing documents retain their
current view; the per-document toggle does not change the preference for future documents.

Desktop launch options:

```sh
pdfcraft --layout continuous --page-gaps off document.pdf
pdfcraft --default-page-gaps off
```

The opt-in UI control channel accepts `set key=page-gaps value=off` for the active document
and `set key=default-page-gaps value=off` for future documents. Its `command` method accepts
`id=view.page_gaps`, and `state` reports `active.page_gaps` and `active.continuous`.

Revision history: 2026-10-17 — added local gapless-display controls and documentation.

In a side-by-side comparison, the gap setting applies to both panes. Changing it in
Organize Pages keeps your document scroll position when you return to the viewer.
