# Where PdfCraft falls short of Acrobat Pro

> **Last reviewed:** 2026-10-15 · **Last updated:** 2026-10-15 · **Change:** minor (gap 12: reading order is scored, not just word F1) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)
> **Last reviewed:** 2026-10-14 · **Last updated:** 2026-10-14 · **Change:** minor (M1: raw-deflate fallback preserves decoding-limit errors; estimates unchanged) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)
> **Last reviewed:** 2026-10-12 · **Last updated:** 2026-10-13 · **Change:** minor (gap 14: Vietnamese catalog shipped; Hindi/Indonesian/Korean and Arabic RTL remain) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)
> **Last reviewed:** 2026-10-13 · **Last updated:** 2026-10-13 · **Change:** minor (gap 6: #1910 Organize Pages opens on the current page; estimates unchanged) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)
> **Last reviewed:** 2026-10-12 · **Last updated:** 2026-10-12 · **Change:** minor (gap 6: #1398 stamps read upright on rotated pages; estimates unchanged) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)
> **Last reviewed:** 2026-10-12 · **Last updated:** 2026-10-12 · **Change:** minor (gap 6: #1317, #1327, #1302, #1315 and #1340; estimates unchanged) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)
> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-12 · **Change:** minor (#1316: replacing a line keeps a neighbouring line whose glyph box overlaps; gap 3 remains) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)
> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-12 · **Change:** gap 4 closed: the web build prints through the browser's print dialog; exclusive image-crop resize follow-up; minor (Word invoice regression coverage, #773; Office fidelity still partial; gap 2: hayro patch counts after the native-gradient backport) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)
> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-12 · **Change:** minor (gap 13: large-file memory measured, edits keep two copies instead of three, file-backed input blocked on gap 2) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)
> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-11 · **Change:** minor (gap 1 closed by #836; fixed issues removed from gaps 3, 6 and 16) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)
> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-11 · **Change:** minor (document-font subset completion and curved/shared image clips in the contributor fork; gap 2: hayro patch counts after the native-gradient backport) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

Every known shortfall, one entry each, ranked by what it costs users. This is the work list: agents
choose from the top unless the owner says otherwise, and prefer these over new P2/P3 checklist
features. The numbers behind it are in [target-app-parity.md](target-app-parity.md); the per-feature
detail is in `parity/acrobat-features.toml` (`cargo xtask parity --partial` lists what each partial
feature lacks).

No gap blocks the [alpha gate](roadmap.md#alpha-gate); gap 8 holds the convert-and-share
workflow's partial sub-cases and is the first beta item. Gap numbers are kept when a gap closes, so
references elsewhere stay valid (gap 1, encrypted output Acrobat rejected, closed with #836; gap 4,
no printing on the web, closed by printing through the browser's print dialog).

Hours are Opus 5.5 agent wall-clock hours. "Doc" names the parity document the gap belongs to.
When a gap closes, delete its entry, note it in the [ROADMAP.md](../ROADMAP.md) progress log and
update the numbers.

## Summary

| # | Gap | Kind | Hours | Doc |
|---|---|---|---|---|
| 2 | Rendering is borrowed and its fidelity unmeasured | feature, spec | 110–190 | [pdf-spec-parity.md](pdf-spec-parity.md) |
| 3 | Editing existing text is fragile on real files | feature | 100–180 | [target-app-parity.md](target-app-parity.md) |
| 5 | Chinese UI shows missing-glyph boxes in releases | localization | 6–12 | [localization-parity.md](localization-parity.md) |
| 6 | Open-issue backlog of wrong results in shipped features | stability | 45–80 | this file |
| 7 | OCR reads only unaccented Latin | feature | 40–75 | [file-format-parity.md](file-format-parity.md) |
| 8 | Office export weak, Office import absent | file format | 55–100 | [file-format-parity.md](file-format-parity.md) |
| 9 | No Preflight; PDF/A partial; PDF/X and PDF/UA absent | standards | 65–115 | [pdf-spec-parity.md](pdf-spec-parity.md) |
| 10 | Signatures: no PKCS #11, timestamp servers or online revocation | feature, hardware | 40–70 | [pdf-spec-parity.md](pdf-spec-parity.md) |
| 11 | Accessibility: checker only, no tagging tools | feature | 40–70 | [target-app-parity.md](target-app-parity.md) |
| 12 | No measured fidelity harness against Acrobat | quality | (in 2) | [target-app-parity.md](target-app-parity.md) |
| 13 | Performance budgets and lazy loading of very large files | performance | 25–45 | this file |
| 14 | Arabic not mirrored; no Hindi, Indonesian, Korean | localization | 25–45 | [localization-parity.md](localization-parity.md) |
| 15 | Forms: JavaScript object model, actions, submit, XFA depth | feature | 40–75 | [pdf-spec-parity.md](pdf-spec-parity.md) |
| 16 | UI precision gaps (multi-select move, rulers, accelerators, windows) | UI/UX | 30–60 | [ui-parity.md](ui-parity.md) |
| 17 | Certificate security and other protect gaps | feature, spec | 15–30 | [pdf-spec-parity.md](pdf-spec-parity.md) |
| 18 | Print production tools absent | feature | 25–45 | [target-app-parity.md](target-app-parity.md) |
| 19 | Preferences mostly missing | UI/UX | 15–25 | [ui-parity.md](ui-parity.md) |
| 20 | Scanners, GPU page rendering, pen pressure, Read Out Loud | hardware | 25–50 | [hardware-parity.md](hardware-parity.md) |
| 21 | Compare: no side-by-side view or filters | feature | 10–20 | [target-app-parity.md](target-app-parity.md) |
| 22 | PDF 2.0 extras, linearization, Arlington validation | spec | 20–35 | [pdf-spec-parity.md](pdf-spec-parity.md) |
| 23 | Local AI provider (summarize, ask, alt text, redaction suggestions) | AI | 20–40 | [target-app-parity.md](target-app-parity.md) |
| 24 | Checklist not diffed against Acrobat's own menus | measurement | 4–8 | [target-app-parity.md](target-app-parity.md) |

## The gaps

### 2. Rendering is borrowed and its fidelity unmeasured

- **Missing:** our own renderer (M2: `model` crate, font engine, DisplayList devices, ADR-0004), or
  an owner decision to adopt another engine; and any measurement of rendering against Acrobat.
- **Evidence:** pages are drawn by vendored `hayro` 0.7 with 26 local patches to `hayro-interpret`,
  15 to `hayro`, 3 to `hayro-jbig2` and 17 to `hayro-syntax` (`vendor/README.md`); `lopdf` still
  backs document inspection. All 7 rendering P0 features are `partial`. User reports: TikZ pattern
  fills drawn wrong ([#794](https://github.com/storytold/pdfcraft/issues/794)), blurry text at
  1080p/1440p ([#730](https://github.com/storytold/pdfcraft/issues/730)), fixed zoom sizes differ
  from Acrobat ([#739](https://github.com/storytold/pdfcraft/issues/739)). An outside author
  proposes his MIT/Apache renderer as the bridge
  ([#841](https://github.com/storytold/pdfcraft/issues/841)).
- **Impact:** every page a user sees. Wrong glyphs, shadings or blend modes erode trust faster than
  any missing feature.
- **Fix:** first a fidelity harness (gap 12), then the renderer: own devices (150–250 h in the old
  plan) or an adopted engine plus our own font and text layers.
- **Estimate:** 110–190 h (includes the harness). **Owner decision needed** on #841 vs M2.
  **Doc:** [pdf-spec-parity.md](pdf-spec-parity.md).

### 3. Editing existing text is fragile on real files

- **Fork progress (2026-10-11):** compatible words and subset-font spaces form lines;
  source fonts lacking a space glyph remain reusable; inline editing previews remove source
  glyphs on a clone. Aligned visual rows now form mixed-style regions (including matrix-scaled headings),
  with source-run styles preserved during editing and delete/save/reopen regressions. A private
  real-file acceptance check and opt-in UI regression supplement synthetic coverage.
  Embedded sfnt faces now render in the editor, including Mac format-6 subsets adapted in memory
  through the PDF Unicode map. Source baselines remain stable; draft frames expand/shrink with
  visible text, and `text_edit_layout` measures saved draft bounds without applying them.
  Page/path-clipped artwork bounds and text-first hit testing prevent large cropped images from
  blocking text. A separate private four-page regression opens all 73 detected text regions;
  curve/hole hit testing now evaluates clipping paths and fill rules. A clip the image owns
  moves with it across nested states and content streams; shared path clips stay where they are.
  Full matching document fonts complete absent subset glyphs; explicit installed/file font
  selection, TrueType/OpenType-CFF embedding, disjoint style ranges, marked paragraph boundaries
  and single-face OpenType shaping/bidi have regression coverage. Actual Chinese keyboard
  insertion/deletion and save/reopen renders are checked on all four private pages.
  Legacy non-SFNT previews, mixed-font/style contextual runs, structure-tree fidelity and reliable
  semantic grouping on every untagged PDF remain open. Replacing one line or paragraph no longer
  removes a distinct neighbour whose glyph box overlaps (#1316); a second drawing on the same
  baseline is still replaced. Estimates are unchanged.

- **Missing:** robust in-place editing of text PdfCraft didn't write: subset fonts without the
  needed glyphs, CJK and right-to-left text, rotated text, lists, superscript/subscript, keeping
  structure tags, find and replace in edit mode; vector and object editing; arrange and align.
- **Evidence:** `edit.text-edit` is partial (font reused or Helvetica substituted);
  `edit.rtl-cjk-editing`, `edit.rotated-text-editing`, `edit.keep-tags-on-edit`,
  `edit.vector-edit`, `edit.edit-object-tool` are planned. Users:
  [#766](https://github.com/storytold/pdfcraft/issues/766) (editing Arabic text),
  [#844](https://github.com/storytold/pdfcraft/issues/844) (several elements now move together on
  one page, not across pages),
  [#787](https://github.com/storytold/pdfcraft/issues/787) (move images between pages),
  [#791](https://github.com/storytold/pdfcraft/issues/791).
- **Impact:** "Edit PDF" is one of Acrobat Pro's top reasons to buy. Substituting Helvetica changes
  how a document looks.
- **Estimate:** 100–180 h (the longest feature pole, M7). **Doc:** [target-app-parity.md](target-app-parity.md).

### 5. Chinese UI shows missing-glyph boxes in releases

- **Missing:** a Simplified Chinese (`Hans`) UI face in release builds and a working fallback.
- **Evidence:** [#826](https://github.com/storytold/pdfcraft/issues/826),
  [#688](https://github.com/storytold/pdfcraft/issues/688),
  [#728](https://github.com/storytold/pdfcraft/issues/728),
  [#758](https://github.com/storytold/pdfcraft/issues/758),
  [#689](https://github.com/storytold/pdfcraft/issues/689) (Flatpak). `AGENTS.md` §1.1 bars Noto
  CJK (Source Han rebranded), so craft-fonts needs another openly licensed Hans face.
- **Impact:** the second-largest language group sees a broken interface although the catalog is 97%
  translated.
- **Estimate:** 6–12 h (font sourcing in craft-fonts plus the fallback chain). **Doc:**
  [localization-parity.md](localization-parity.md).

### 6. Open-issue backlog of wrong results in shipped features

- **Missing:** fixes for the ≈ 13 of ≈ 25 filed reports of features marked `shipped` that give wrong results still open,
  plus triage of ≈ 220 other open issues.
- **Evidence:** 245 open issues on 2026-10-10. The first batch of `fix(...)` reports was fixed by
  2026-10-10 (#799, #800, #807, #809, #810, #811, #815, #816, #817, #818, #819, #821). A later
  batch keeps a cloudy comment flatten cannot draw (#1317), leaves a refused text-box restyle or
  resize unchanged (#1327), keeps arrow defaults off plain lines (#1302), stops a custom text
  match on the separator instead of the next glyph (#1315), and states the vertical-text
  redaction refusal without claiming an overlap (#1340). Organize Pages opens on the page being
  read, selected and scrolled into view (#1910). Stamps placed on a rotated page read upright
  (#1398). Another batch keeps a spaced XFA radio value in the datasets (#1319), replaces only the
  selected pages when they are not adjacent (#1305), and leaves unrelated page streams in place
  when a figure is marked decorative (#1324). The rest of the backlog is untriaged.
- **Fix verified (2026-10-14):** raw-deflate fallback returns a decoding-limit error instead of
  accepting an empty partial stream when a misleading zlib header triggers a retry. Synthetic
  filter and COS stream regressions cover refusal at the limit and decoding within it.
- **Impact:** every one is a user who lost trust. Crash reports outrank feature work
  (`AGENTS.md` §4).
- **Estimate:** 45–80 h (≈ 0.2–0.4 h per issue). **Doc:** this file.

### 7. OCR still has language and scan-cleanup gaps

- **Now available:** English, German, French, Spanish and horizontal Japanese, Unicode search/copy,
  automatic quarter-turn correction and deskew up to 12 degrees, including correction without OCR.
  Clear paper edges can now define an automatic crop with safety padding; ambiguous white borders
  stay unchanged, and original image pixels remain in the PDF.
- **Missing:** additional language choices, vertical Japanese, editable-text output, deskew on pages
  with annotations, despeckle, background/camera cleanup, MRC compression, browser OCR and scanner input.
- **Evidence:** `crates/engine/tests/scan_ocr.rs` and the OCR automation/UI tests; model tests require
  `cargo xtask models`, Japanese fixture generation requires the release `CRAFT_FONTS_DIR` input.
- **Impact:** scanned documents remain a core Pro workflow with wider language and layout demands.
- **Estimate:** previous 40–75 h estimate needs reassessment after this narrower feature slice.
### 8. Office export weak, Office import absent

- **Missing:** Word export that keeps layout; Excel and PowerPoint export; creating PDFs from Office,
  HTML, web pages and PostScript; SVG/XML/EPS/PS/JPEG 2000 export.
- **Evidence:** `create.export-docx` partial; the synthetic invoice regression for
  [#773](https://github.com/storytold/pdfcraft/issues/773) now covers absent inferred borders,
  floating watermark text and per-page section sizes. Exact font matching, vector rules and
  broader layout fidelity remain incomplete;
  `create.export-xlsx`, `create.export-pptx`, `create.from-office`, `create.from-html` planned.
- **Impact:** "Export PDF" and "Create PDF" are two of Acrobat's most used tools.
- **Gate:** lossy or missing sub-cases of the convert-and-share workflow (partial, not blocking).
  First beta items: Create PDF from Office (LibreOffice sidecar, 10–20 h) and a Word export that
  keeps layout (20–35 h).
- **Estimate:** 55–100 h. **Doc:** [file-format-parity.md](file-format-parity.md).

### 9. No Preflight; PDF/A partial; PDF/X and PDF/UA absent

- **Missing:** Preflight (profiles, single checks and fixups, results tree, reports, droplets);
  PDF/A-1, PDF/A-4 and conformance levels a/u; PDF/X verify and convert; PDF/UA-1/2 verify; PDF/E,
  PDF/VT; output-intent choice; transparency flattening.
- **Evidence:** 12 Preflight features and 9 standards features planned; `optimize.pdfa-validate`,
  `optimize.pdfa-convert` partial (2b/3b structural rules; CMYK documents get no output intent).
  Acrobat ships hundreds of built-in Preflight profiles.
- **Impact:** blocks print shops, archives and accessibility compliance work.
- **Estimate:** 65–115 h. **Doc:** [pdf-spec-parity.md](pdf-spec-parity.md).

### 10. Signatures: no PKCS #11, timestamp servers or online revocation

- **Missing:** PKCS #11 tokens and smart cards on macOS and Linux; timestamp-server preferences;
  fetching OCSP/CRL online for LTV; FieldMDP and locking fields on signing; the OS trust store; EU
  Trusted Lists; signature graphics and saved appearances; compare signed version; signing in the
  browser.
- **Evidence:** `sign.pkcs11`, `sign.timestamp-servers`, `sign.field-mdp`, `sign.os-trust`,
  `sign.eutl` planned; `sign.pades-bt`, `sign.ltv`, `sign.ocsp-crl` partial (caller-supplied
  evidence). Users ask for Acrobat-like validation
  ([#824](https://github.com/storytold/pdfcraft/issues/824),
  [#775](https://github.com/storytold/pdfcraft/issues/775),
  [#718](https://github.com/storytold/pdfcraft/issues/718)).
- **Impact:** government and legal users sign with tokens and need B-LT/B-LTA.
- **Estimate:** 40–70 h; tokens need hardware to test. **Doc:** [pdf-spec-parity.md](pdf-spec-parity.md).

### 11. Accessibility: only text-only automatic structure creation

- **Missing:** apply auto-tag proposals to marked content and a valid parent tree, tag fields
  into structure trees with unusual layouts (only a direct Document root with a direct
  ParentTree Nums array is extended),
  visual Tags, Order and Content panels, Reading Order tool, table editor, Make Accessible action,
  Read Out Loud; full keyboard operation of PdfCraft itself and a screen-reader audit.
- **Evidence:** all 32 checker rules, figure alt text, tag property and sibling order edits, and
  a bounded auto-tag preview exist. Apply writes valid structure and parent-tree links for
  text-only files; it refuses mixed content and existing tags;
  `a11y.keyboard-only` (P0) planned, `a11y.app-screen-reader` partial.
- **Impact:** most untagged files still need another editor to build their structure; PdfCraft itself is
  not yet usable without a mouse.
- **Estimate:** 40–70 h. **Doc:** [target-app-parity.md](target-app-parity.md).

### 12. No measured fidelity harness against Acrobat

- **Missing:** side-by-side comparison of rendering, text extraction, form calculation and
  signature validation against Acrobat on synthetic fixtures; an interop suite that opens every
  kind of output we write (encrypted ones above all, after #774) in pdf.js, MuPDF and Poppler as
  oracle processes.
- **Progress (2026-10-14):** rendering now has an oracle too. `cargo xtask render-oracle` compares
  page 1 of every corpus file against Poppler's `pdftoppm` and reports a disagreement rate: the
  share of pixels with no near match in the other renderer's 3×3 neighbourhood, which forgives
  antialiasing and subpixel placement but not missing or wrong content (`crates/oracle`, module
  `pixels`). **First reading, 250 files at 72 dpi, tolerance 8: 229 compared, median 0.89%, mean
  2.16%, p90 5.35%, 25 within 0.1%.** It reports rather than gates — the corpus is not fetched on
  CI, and a first reading of a borrowed renderer is a baseline to improve on, not a bar to clear.
  The worst results cluster on JBIG2 `bitmap-*` files (`hayro-jbig2` carries 3 local patches) and
  on ZapfDingbats, which points the renderer work at something specific for the first time.
- **Read the score as a disagreement, not a verdict (2026-10-14).** The worst file of the 250,
  `bug1552113.pdf` at 84%, turned out to be Poppler's defect: it paints the link's `/Border [0 0
  112]` as a 112-unit band over the two lines of text, and the fixture is named "this text should
  be visible". Ours is the readable rendering. Anything at the top of the list needs a person to
  look at both images before it is treated as a bug of ours.
- **The JBIG2 cluster was Poppler too, and the suite checks itself (2026-10-14).** The next four
  worst files were JBIG2, which looked like a defect in `hayro-jbig2` and its local patches. It is
  the reverse. The 96 `bitmap-*.pdf` fixtures are a conformance suite: one picture encoded through
  every corner of JBIG2, so a correct decoder renders all 96 identically — a real test needing no
  reference image and no oracle. **We render 1 distinct image across all 96; Poppler renders 8**,
  drawing a blank page for the seven halftone-skip and symbol fixtures and mis-decoding TPGRON
  refinement (its output changes when TPGRON is switched on, which a correct decoder's cannot).
  Pinned as `crates/render/tests/jbig2_invariants.rs`. **Poppler is not a usable oracle for JBIG2**,
  and roughly a fifth of its fixtures should be read as its bugs rather than ours.
- **Annotations are shown, and that was measured rather than argued.** Poppler synthesises an
  appearance for an annotation that has none, where we draw only what the document carries (link
  borders without `/AP` are recorded as not covered under `view.render-annotation-appearances`), so
  showing them scores us for a known gap. Hiding them is worse, because it then scores Poppler for
  every appearance we *do* draw: over the same 250 files, shown gives mean 2.16% / p90 5.35% / 25
  files within 0.1%, hidden gives 2.18% / 5.55% / 22, with `annotation-tx3.pdf` jumping to 45.64%.
  The biases nearly cancel; `--hide-annotations` tells the two apart for one file at a time.
- **The text score was blind to reading order (2026-10-15).** Word F1 is a bag of words, so a page
  emitted back to front scores a perfect 1.000 — and reading order is most of what text extraction
  has left to get wrong (#2387, #2586). `cargo xtask text-oracle` now reports a second score: the
  longest common subsequence of the two word sequences over the longer one, both cut to 3,000
  words. The hole was not hypothetical. Measuring #2431, a change that moves text and alters
  nothing else, **word F1 called 26 of the 29 files whose output changed identical**, where the
  order score separated 18 improvements from 6 regressions and named their shape: forms, where a
  checkbox's number was parted from its label (`prefilled_f1040.pdf` 0.672 → 0.481). Read it as a
  disagreement, like the pixel score: the same split scored as a win on one file and a loss on
  another purely because `pdftotext` makes it in one place and not the other. The corpus-wide
  first reading has not been taken yet.
- **Evidence:** text extraction (word-F1 0.98 against `pdftotext`) and now rendering (above); qpdf
  checks written output on every CI run since `PDFCRAFT_ORACLES_REQUIRED`. Acrobat captures must
  stay local in `plan/acrobat/` (clean-room, `AGENTS.md` §1.1).
- **Still missing:** Acrobat itself as the reference rather than other open renderers; form
  calculation and signature validation; the interop suite that opens every kind of output we write
  (encrypted ones above all, after #774) in pdf.js and MuPDF; and 15 of those 250 files could not
  be compared at all because the two renderers round the page box to different pixel sizes.
- **Impact:** until it is complete, quality numbers outside rendering and text are judgements.
- **Estimate:** counted in gap 2 (≈ 20–30 h of it; roughly a third done).
  **Doc:** [target-app-parity.md](target-app-parity.md).

### 13. Performance budgets and lazy loading of very large files

- **Missing:** `misc.performance-budgets` (P0) and `core.lazy-loading` (P0): open GB files within a
  memory budget, with budgets checked on Tier-1 platforms.
- **Evidence:** #307 brought a 9,156-page file from > 10 GiB to ≈ 500 MiB inspection, but
  file-backed input and global memory budgets remain open; blurry fonts at common resolutions
  (#730). Measured 2026-10-12 on synthetic files (release build, Linux): a 1 GiB file opens at
  1.0 GiB peak in 0.7 s, so viewing holds the whole file once; after edits it held two copies and
  peaked at three (3.1 GiB after five edits), because the previous working file stayed alive while
  the next was written. Edits now let go of it first: five edits peak at 2.1 GiB (8.5 s, was 16 s).
  Saving still peaks at three copies (the saved file is assembled in memory beside the working
  one), and display appearances for comments without one add a copy. A 100,000-page file (28 MB)
  opens at 403 MiB in 5.5 s and each edit re-inspects it for ≈ 5 s.
- **Blocked part:** reading a GB file from disk on demand (`ByteSource`) needs every reader to
  take ranges, but the `hayro` renderer parses one contiguous buffer (`Pdf::new(PdfData)`), so it
  waits on gap 2's renderer decision. Until then the budget is one copy of the file to view and two
  to edit.
- **Estimate:** 25–45 h. **Doc:** this file.

### 14. Arabic not mirrored; no Hindi, Indonesian, Korean

- **Missing:** a right-to-left layout (panels, menus, alignment), bidirectional text in the text
  layout itself (wrapping, typed text); catalogs for Hindi, Indonesian and Korean (Acrobat ships
  Korean; it ships Arabic in its Middle East edition; it ships none of Hindi or Indonesian).
  Vietnamese ships as a catalog (`vi`, 100% of the current localizable union) but stays `partial`
  like other non-English languages (no translated help; engine/OS details stay English).
- **Evidence:** [localization.md](localization.md) §Arabic; `misc.rtl-ui` planned.
- **Estimate:** 25–45 h (Arabic RTL 20–35, three catalogs 8–12 plus review). **Doc:**
  [localization-parity.md](localization-parity.md).

### 15. Forms: JavaScript object model, actions, submit, XFA depth

- **Missing:** the wider Acrobat JavaScript object model (annotations, layers, dialogs), document
  actions, the Actions tab triggers that run JavaScript, form submission, the debugger, barcode
  fields, rich-text values; XFA flattening and complete dynamic-form fidelity.
- **Evidence:** `form.js-host` partial, `form.document-actions`, `form.submit-form`,
  `edit.action-javascript`, `form.js-debugger`, `form.xfa-flatten` planned; XFA layout, FormCalc
  and data partial.
- **Estimate:** 40–75 h. **Doc:** [pdf-spec-parity.md](pdf-spec-parity.md).

### 16. UI precision gaps

- **Missing:** moving several selected objects across pages (same-page moves shipped, #844), single-key tool accelerators,
  rulers, grids and guides, loupe and dynamic zoom, multiple windows and split view, comment
  pop-ups that can be moved, connector lines, customisable quick tools; shortcut display and tool-mode bugs (#746, #721), macOS window controls
  (#789).
- **Estimate:** 30–60 h. **Doc:** [ui-parity.md](ui-parity.md).

### 17. Certificate security and other protect gaps

- **Missing:** encrypting for certificate recipients (and opening such files), security policies,
  attachment-only encryption, redaction pattern locales and partial-word redaction, folder
  redaction, protected view, URL and attachment trust.
- **Estimate:** 15–30 h. **Doc:** [pdf-spec-parity.md](pdf-spec-parity.md).

### 18. Print production tools absent

- **Missing:** Output Preview, separations, total area coverage, overprint preview, ink manager,
  colour conversion, printer marks, fix hairlines, flattener preview, colour management settings;
  print as image; PostScript output.
- **Evidence:** 18 print-production features planned (L area).
- **Estimate:** 25–45 h (part of the L area's 30–55). **Doc:** [target-app-parity.md](target-app-parity.md).

### 19. Preferences mostly missing

- **Missing:** most of Acrobat's twenty or so preference pages (General, Documents, Commenting, Forms, Identity,
  Signatures, Security, Measuring, Units and guides, Convert, …). Today: Interface language,
  Documents and view, Identity, Fill & Sign, JavaScript.
- **Estimate:** 15–25 h. **Doc:** [ui-parity.md](ui-parity.md).

### 20. Scanners, GPU page rendering, pen pressure, Read Out Loud

- **Missing:** TWAIN/WIA/ICA/SANE scanning; GPU-accelerated page rendering (pages are rasterized on
  the CPU and uploaded); pressure-sensitive ink; platform text-to-speech.
- **Estimate:** 25–50 h; scanners need hardware. **Doc:** [hardware-parity.md](hardware-parity.md).

### 21. Compare: no side-by-side view or filters

- **Missing:** synchronised side-by-side view, filters by change type, page ranges, scanned
  documents.
- **Estimate:** 10–20 h. **Doc:** [target-app-parity.md](target-app-parity.md).

### 22. PDF 2.0 extras, linearization, Arlington validation

- **Missing:** UTF-8 text strings, associated files (`/AF`), document parts, structure namespaces;
  linearized save (Fast Web View); deterministic save; structural validation against the Arlington
  model.
- **Estimate:** 20–35 h. **Doc:** [pdf-spec-parity.md](pdf-spec-parity.md).

### 23. Local AI provider

- **Missing:** the opt-in provider interface (local model or the user's own endpoint) and the
  features on it: summarize, ask with page citations, alternate-text and redaction suggestions.
  Acrobat's AI Assistant is a cloud service and out of scope.
- **Estimate:** 20–40 h. **Doc:** [target-app-parity.md](target-app-parity.md).

### 24. Checklist not diffed against Acrobat's own menus

- **Missing:** a menu and tool listing captured from Acrobat (names only, `plan/acrobat/`, never
  committed) diffed against the checklist, so that features the checklist never listed show up.
  `misc.localization` and `misc.installers` are now `partial` (Chinese, Telugu and Arabic are not
  fully usable yet; Linux packages are unsigned).
- **Estimate:** 4–8 h. **Doc:** [target-app-parity.md](target-app-parity.md).

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-15 | minor | Gap 12: the text oracle scores reading order as well as word F1, which is a bag of words and cannot see order; the first measurement separated 18 improvements from 6 regressions in a change F1 called unchanged on 26 of 29 files. Estimates unchanged. |
| 2026-10-14 | minor | M1: raw-deflate fallback preserves decoding-limit errors in strict and tolerant decoding; synthetic filter and COS stream regressions. Estimates unchanged. |
| 2026-10-11 | minor | M10: multilingual OCR, orientation/deskew, and conservative scanner-border crop with padding. Ambiguous edges and camera cleanup remain open. |

| 2026-10-15 | patch | Gap 24: record localization and installers as partial (unusable Chinese, Telugu and Arabic; unsigned Linux packages) |
| 2026-10-13 | minor | Gap 6: Organize Pages selects and reveals the current page on open, including after an earlier grid selection elsewhere (#1910). Estimates unchanged. |
| 2026-10-12 | minor | Gap 6: rubber stamps, explicit image rectangles and PDF-page stamps read upright on rotated pages (#1398). Estimates unchanged. |
| 2026-10-12 | minor | #1316: replacing a line or paragraph keeps a neighbouring line whose glyph box overlaps. Gap 3 remains. Estimates unchanged. |
| 2026-10-12 | minor | Exclusive image crops resize with their pixels across nested states, intervening operators and content streams; shared clips remain fixed. Synthetic engine/tool regressions and private image save/reopen checks; estimates unchanged. |
| 2026-10-12 | minor | Gap 13: measured large-file memory; edits no longer keep the previous working file while writing the next (peak 3 → 2 copies); file-backed input recorded as blocked on gap 2 |
| 2026-10-11 | minor | Closed gap 4: the web build prints through the browser's print dialog (sheets drawn by PdfCraft at 150–300 dpi, one per printed page); number kept |
| 2026-10-11 | minor | Word invoice failure #773 has synthetic regression coverage; broader Office conversion remains gap 8. |
| 2026-10-11 | minor | Contributor fork: mixed-style visual regions, source-font space reuse and a read-only textless background. Embedded sfnt source faces and Mac subset Unicode mappings keep edit baselines stable; live frames follow Apply/Save geometry as text grows or shrinks. Engine/tool/UI regressions and private all-page checks cover the changes; full matching font import, CFF, range styles, shaping and private Chinese typing checks added; legacy programs and general fidelity remain open; estimates unchanged |
| 2026-10-11 | patch | Gap 2: 23 hayro-interpret and 10 hayro patches after axial and radial shadings became native gradients (backport of hayro 0.8) |
| 2026-10-11 | minor | Closed gap 1 (#774, fixed by #836; numbers kept, its interop suite moved to gap 12); gap 6 lists the fixed #799–#821 batch; gap 3 and 16 updated for #844 (same-page move), #740 (column select) and #759 (1:1 touchpad scrolling) |
| 2026-10-10 | minor | Removed the alpha markers: with the cross-app gate rule, gaps 1 and 8 are partial sub-cases, not blockers; stage alpha |
| 2026-10-10 | minor | Marked the alpha blockers (gap 1 and the Office part of gap 8) after the core-workflow gate put the stage at pre-alpha |
| 2026-10-10 | major | Created. Ranked 24 gaps from the 2026-10-10 re-measure, the open GitHub issues and the former ROADMAP.md §Where we're lacking and where we're going (renderer, hardening, fidelity, editing, Pro workflows, 1.0 polish), which this file replaces |
