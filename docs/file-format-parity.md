# File format parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-10 · **Change:** major (first format matrix; Acrobat's formats from its bundle) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

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
| Encrypted PDF (standard security R2–R6) | R W | R W | **reading high; writing fails in Acrobat and Reader** | `crates/crypt`, `crates/cos` tests; all 7 corpus encrypted files open; **#774** |
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
| FDF (`.fdf`) comments and form data | R W | R W | good | `crates/xfdf`; bugs #807, #809, #819 in comment exchange |
| XFDF (`.xfdf`, ISO 19444-1) | R W | R W | good | as above |
| XML form data | R W | R W | good | `form.data-xml-csv-txt` |
| XDP (`.xdp`, XFA data package) | R | none | none | Acrobat opens `.xdp` as a document type |
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
| JPEG, PNG, TIFF (multipage), GIF, BMP, JPEG 2000 | yes | yes (ICC profiles kept; DPI choices) | good | `create.from-images`, `create.from-multipage-tiff` |
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
| Several files at once | yes | yes (PDFs, images and text) | good | `create.create-multiple` |

## Export PDF to

| Target | Acrobat | PdfCraft | Fidelity | Tests and evidence |
|---|---|---|---|---|
| Word (`.docx`) | yes, layout-preserving | flowing document: paragraphs, headings, bold/italic, colour, images, tables | **basic** (#773: layout lost, fake borders, duplicated pages) | `create.export-docx` partial |
| Word 97–2003 (`.doc`) | yes | no | none | |
| Excel (`.xlsx`), XML Spreadsheet 2003 | yes | no | none | `create.export-xlsx` planned |
| PowerPoint (`.pptx`) | yes | no | none | `create.export-pptx` planned |
| RTF | yes | yes | basic | `create.export-rtf` |
| HTML (single page, multiple pages) | yes | single file with headings, paragraphs, images, tables | basic | `create.export-html` partial |
| Plain text | yes | yes (reading order) | good (word-F1 0.98 vs `pdftotext`) | `create.export-text`, `xtask text-oracle` |
| Accessible text (from tags) | yes | no | none | `create.export-accessible-text` planned |
| PNG, JPEG, TIFF | yes | yes | good | `create.export-png`, `create.export-jpeg-tiff` |
| JPEG 2000 | yes | no | none | `create.export-jpeg2000` planned |
| All embedded images | yes | yes | good | `create.export-all-images` |
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
| 2026-10-10 | major | Created from Acrobat 26.002.21931's `CFBundleDocumentTypes`, Acrobat's public Create/Export documentation, and PdfCraft's code and checklist |
