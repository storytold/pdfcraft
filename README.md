# Linkco PDF Editor

**Professional PDF tools for everyday document work.**

Linkco PDF Editor is a fast, native desktop and web PDF application developed by **Al Rawabet Commercial Services & Contracting Company W.L.L.** (`Linkco`). It provides a complete local-first PDF workspace for viewing, searching, commenting, filling and signing forms, organizing pages, creating and converting documents, redacting sensitive content, and automating document workflows.

All document processing happens locally on your machine with no telemetry, no account requirement, and no cloud uploads.

---

## Key Capabilities

### Organize & Edit
- **Organize Pages** — Reorder, rotate, duplicate, insert, extract, replace, crop, and delete pages visually.
- **Edit PDF** — Edit text paragraphs and images directly on the page, add text boxes, links, headers & footers, watermarks, backgrounds, and Bates numbering.
- **Combine & Split** — Merge multiple PDFs or images into a single document, or split documents by page count, file size, or top-level bookmarks.

### Review, Sign & Forms
- **Comments & Markup** — Sticky notes, text highlights, underlines, strikethroughs, callouts, freehand ink, shapes, and stamps.
- **Fill & Sign** — Interactive AcroForm and XFA form filling, visual signatures, and cryptographic digital signatures (`PKCS#12` / `.pfx` and system certificate stores).
- **Prepare Form** — Create and edit interactive form fields (text, checkboxes, radio buttons, dropdowns, list boxes, buttons, and signature fields).

### Convert, Protect & Verify
- **Create & Export** — Create PDFs from blank pages, text, images, or clipboard; export to PNG, JPEG, TIFF, Word (`.docx`), HTML, RTF, or plain text.
- **Scan & OCR** — Recognize text in scanned pages into a searchable text layer.
- **Redact & Protect** — Permanently remove sensitive text and graphics, sanitize metadata and hidden layers, and protect documents with AES-256 passwords and permissions.
- **Compare, Measure & Accessibility** — Side-by-side document comparison, distance/perimeter/area measurement tools, PDF/A, PDF/X, and PDF/UA inspection, and WCAG accessibility checking.

---

## Building from Source

```bash
# Run the desktop application
cargo run -p pdfcraft

# Build release binaries (GUI and headless CLI)
cargo build --release -p pdfcraft -p pdfcraft-cli

# Run workspace tests and asset verification
cargo test --workspace
cargo xtask assets
```

---

## Command-Line Interface (`pdfcraft-cli`)

Linkco PDF Editor includes `pdfcraft-cli` for scripting, batch processing, and headless automation:

```bash
pdfcraft-cli info    <file.pdf> [--password PW]
pdfcraft-cli render  <file.pdf> --page N [--dpi 96] --out page.png
pdfcraft-cli text    <file.pdf> [--page N]
pdfcraft-cli edit    <in.pdf> --out out.pdf [--rotate 1,3:90] [--delete 2,4]
pdfcraft-cli combine <a.pdf> <b.pdf> --out combined.pdf
pdfcraft-cli extract <in.pdf> --pages 1,3,5 --out extracted.pdf
pdfcraft-cli split   <in.pdf> (--every N | --before 3,7) [--out-dir DIR]
pdfcraft-cli tools
pdfcraft-cli run     <tool> [key=value ...]
```

---

## License & Attribution

Linkco PDF Editor is dual-licensed under the **MIT License** ([LICENSE-MIT](LICENSE-MIT)) and **Apache License, Version 2.0** ([LICENSE-APACHE](LICENSE-APACHE)), at your option.

© Al Rawabet Commercial Services & Contracting Company W.L.L.

See [NOTICE](NOTICE) and [ATTRIBUTION.md](ATTRIBUTION.md) for third-party component and asset notices.
