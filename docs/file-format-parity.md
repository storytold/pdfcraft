# File format parity

> **Last reviewed:** 2026-10-16 · **Last updated:** 2026-10-16 · **Change:** minor (comic book archive export) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

Every format Acrobat Pro reads or writes, and PdfCraft's support for it. Acrobat's list comes from
its bundle's `Info.plist` `CFBundleDocumentTypes` (what it opens) and its public documentation
(Create PDF and Export PDF). PdfCraft's support comes from the code and
`parity/acrobat-features.toml`. Summary in [target-app-parity.md](target-app-parity.md); gaps
in [gaps.md](gaps.md).

Legend: **R** read/import, **W** write/export. Fidelity: *high* (tested against an external oracle
or round-trips exactly), *good* (tested, known limits), *basic* (works, visibly lossy), *none*.

**File formats and conversion ≈ 40% ready** (estimated): PDF itself is strong except encrypted
output interoperability; the conversion formats users reach for most (Word, Excel, PowerPoint,
Office to PDF, OCR beyond English) are weak or missing. **120–220 h** to parity (within the
features total).

## PDF

| Format | Acrobat | PdfCraft | Fidelity | Tests and evidence |
|---|---|---|---|---|
| PDF 1.0–1.7, ISO 32000-1 | R W | R W | high for reading and unencrypted writing | 958-file pdf.js corpus open/edit/save; `qpdf --check` on saved outputs; `xtask check` baselines |
| PDF 2.0 (ISO 32000-2) | R W | R W (2.0-only features ignored but preserved) | good | No UTF-8 strings, `/AF`, DPart, namespaces ([pdf-spec-parity.md](pdf-spec-parity.md)) |
| Encrypted PDF (standard security R2–R6) | R W | R W | reading high; writing opens in Acrobat DC since #836 (#774; AES-256 and RC4-128 checked by hand, no automated interop suite) | `crates/crypt`, `crates/cos` tests (`the_catalog_of_an_encrypted_document_stays_outside_object_streams`); all 7 corpus encrypted files open |
| Certificate-encrypted PDF (public-key handler) | R W | none (clear message) | none | `core.unsupported-handlers` |
| Linearized PDF (Fast Web View) | R W | R (as normal PDF); no W | none for W | `core.linearization` planned |
| Damaged PDF | R (repair) | R (repair, logged) | high | `core.repair-broken-files`, nightly fuzzing |
| PDF/A-1, 2, 3, 4 | R W, verify | R; verify/convert 2b and 3b (partial) | basic | `optimize.pdfa-*`; veraPDF not yet used as oracle |
| PDF/X, PDF/UA, PDF/E, PDF/VT | R W, verify | R only | none | [pdf-spec-parity.md](pdf-spec-parity.md) |
| PDF Portfolio (collection) | R W | R as a PDF with attachments; no portfolio view | none | `organize.portfolio-*` planned |
| XFA forms (in PDF) | R W (fill) | R, lay out dynamic forms, fill, save datasets (partial) | basic | `form.xfa-*` partial; `crates/xfa` tests |

## Data and interchange formats

| Format | Acrobat | PdfCraft | Fidelity | Tests and evidence |
|---|---|---|---|---|
| FDF (`.fdf`) comments and form data | R W | R W | good | `crates/xfdf`; comment-exchange fixes for popups, polygon vertices and callouts (#807, #809, #819) |
| XFDF (`.xfdf`, ISO 19444-1) | R W | R W | good | as above |
| XML form data | R W | R W | good | `form.data-xml-csv-txt` |
| XDP data (`.xdp`) for XFA forms | R W | R W: Export Data writes the whole datasets as an XDP; Import Data takes an XDP, datasets or data XML | good | `form.xfa-data` (`data_exports_as_datasets_xml_or_an_xdp_and_imports_from_any_of_them`, `xfa_data_files_export_and_import_through_tools`) |
| XDP opened as a document | R | none | none | Acrobat opens `.xdp` as a document type |
| CSV / tab-delimited form data | W (merge to spreadsheet) | R W; merge to spreadsheet | good | `form.merge-to-spreadsheet` |
| Comment summary (PDF) | W | W (comments only layout) | basic | `comment.summarize` partial |
| Measurement CSV | W | W | good | `comment.measure-export` |
| Compare report (PDF) | W | W | good | `misc.compare-report` |
| Action Wizard sequences (`.sequ`) | R W | own JSON actions; no `.sequ` | none for `.sequ` | `misc.action-wizard-*` |
| Security settings (`.acrobatsecuritysettings`), `.apf` profiles, `.secstore` | R W | none | none | out of scope unless asked |
| Search index (`.pdx`) | R W | none | none | `view.search-index` planned |
| JDF / MJD job definitions | R | none | none | `print.jdf` planned (P3) |
| Preflight profiles (`.kfp`) and reports | R W | none | none | gap 9 |
| PDF Optimizer settings (`.optimize`) | R W | none (dialog settings not saved as presets) | none | `optimize.optimizer-presets` planned |

## Certificates and keys

| Format | Acrobat | PdfCraft | Fidelity | Tests and evidence |
|---|---|---|---|---|
| PKCS #12 digital IDs (`.p12`, `.pfx`) | R W | R W (self-signed IDs) | high | OpenSSL-made fixtures (`crates/sign/tests/data/README.md`) |
| Certificates (`.cer`, `.der`, `.pem`, `.p7c`, `.p7b`) | R W | R (`.cer/.pem/.der/.p12` trust import); W public certificate | good; `.p7c/.p7b` bundles not read | `sign.trust-import` partial, `sign.export-certificate` |
| CMS / PKCS #7 signatures, RFC 3161 tokens, OCSP, CRL | R W | R W | high | `pdfsig` and OpenSSL oracles |

## Create PDF from

| Source | Acrobat | PdfCraft | Fidelity | Tests and evidence |
|---|---|---|---|---|
| JPEG, PNG, TIFF (multipage, and BigTIFF), GIF, BMP, JPEG 2000 | yes | yes (ICC profiles kept; DPI choices) | good | `create.from-images`, `create.from-multipage-tiff`, `bigtiff_images_become_pages` |
| PCX | yes | no | none | in Acrobat's document types |
| Plain text | yes | yes | good | `create.from-text` |
| Clipboard (image or text) | yes | yes | good | `create.from-clipboard` |
| Screenshot | yes | no | none | `create.from-screenshot` planned |
| Word, Excel, PowerPoint (and Visio, Publisher on Windows) | yes | no | none | `create.from-office` planned (LibreOffice sidecar) |
| HTML file, web page URL | yes | no | none | `create.from-html`, `create.from-web-page` planned |
| RTF | yes | no | none | |
| PostScript / EPS (Distiller) | yes | no | none | `create.distiller` planned (P3) |
| Markdown | no | no | none | `create.from-markdown` planned (P2; not an Acrobat feature) |
| Scanner | yes | no | none | `ocr.scanner-acquire` planned |
| Several files at once | yes | yes: combine PDFs, images and text into one document, or create a separate PDF for each non-PDF source (GUI and `doc_create_multiple mode=separate`); existing outputs get numbered names | good for supported source types | `create.create-multiple`, `separate_batch_creates_one_pdf_per_source_without_replacing_existing_files` |

## Export PDF to

| Target | Acrobat | PdfCraft | Fidelity | Tests and evidence |
|---|---|---|---|---|
| Word (`.docx`) | yes, layout-preserving | flowing paragraphs and borderless inferred tables; positioned text/artwork only for pages with rotated text or text overlapping an image/watermark, with per-page sizes | **basic** (#773 synthetic invoice regression covered; exact fonts, vector rules and general layout fidelity remain incomplete) | `create.export-docx` partial |
| Word 97–2003 (`.doc`) | yes | no | none | |
| Excel (`.xlsx`), XML Spreadsheet 2003 | yes | no | none | `create.export-xlsx` planned |
| PowerPoint (`.pptx`) | yes | slides per page: title from largest text, body text, images, tables (no slide overflow reflow / normAutofit; first page sets presentation dimensions) | basic | `create.export-pptx` partial |
| RTF | yes | yes | basic | `create.export-rtf` |
| HTML (single page, multiple pages) | yes | single file with headings, paragraphs, images, tables | basic | `create.export-html` partial |
| Plain text | yes | yes (reading order) | good (word-F1 0.98 vs `pdftotext`; reading order is scored separately, because F1 cannot see it) | `create.export-text`, `xtask text-oracle` |
| Accessible text (from tags) | yes | no | none | `create.export-accessible-text` planned |
| PNG, JPEG, TIFF | yes | yes | good | `create.export-png`, `create.export-jpeg-tiff` |
| JPEG 2000 | yes | no | none | `create.export-jpeg2000` planned |
| All embedded images | yes | yes | good | `create.export-all-images` |
| Comic book archive (`.cbz`) | no | one image per page with `ComicInfo.xml` (title, author, subject, language, right-to-left, bookmarks); a page that is a single JPEG image is copied without recompressing, other pages are rendered as JPEG or PNG; zip64 past 4 GiB. Not imported | good | `export.cbz`, `doc_export_cbz`; not in the Acrobat checklist (Acrobat has no such export) |
| XML 1.0 | yes | no | none | `create.export-xml` planned |
| SVG | no (dropped by Adobe) | no | none | `create.export-svg` planned |
| PostScript, EPS | yes | no | none | `create.export-ps`, `create.export-eps` planned |

## OCR (scans to searchable PDF)

| Capability | Acrobat | PdfCraft | Fidelity |
|---|---|---|---|
| Searchable image (invisible text layer) | yes | yes (Tr 3 text fitted to word boxes) | good for English |
| Editable text and images output | yes | no | none |
| Languages | dozens (Latin with accents, Cyrillic, Greek, CJK, Arabic, Hebrew, Indic) | Latin without accents only (ocrs models) | **basic** (#744) |
| Deskew, rotation, despeckle, background and halo removal, descreen | yes | no | none |
| Camera images (edges, perspective, whiteboard, card) | yes | no | none |
| MRC (adaptive) compression, PDF/A output of scans | yes | no | none |

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-16 | minor | Export to comic book archive (`.cbz`), a PdfCraft extra outside Acrobat parity: pages that are a single JPEG image are copied without recompressing, other pages are rendered; streamed to disk, zip64 when large. |
| 2026-10-12 | minor | XDP split into data (read and write for XFA forms, already shipped under `form.xfa-data`) and opening `.xdp` as a document (none). Create PDF reads BigTIFF; malformed GIF headers are no longer detected as images. |
| 2026-10-11 | minor | #773: inferred tables have no invented borders; rotated or overlapping invoice pages use floating shapes and page sections. Full Office layout fidelity remains partial. |
| 2026-10-11 | minor | Encrypted PDF row: written files open in Acrobat DC (#774, fixed by #836). FDF row: #807, #809 and #819 fixed |
| 2026-10-10 | major | Created from Acrobat 26.002.21931's `CFBundleDocumentTypes`, Acrobat's public Create/Export documentation, and PdfCraft's code and checklist |
