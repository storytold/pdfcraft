# Bundle 6: Multilingual OCR, Scan Cleanup & High-Fidelity Office Export

## Overview
Bundle 6 implements intelligent scan cleanup (Hough deskewing, Otsu adaptive binarization, despeckling), multilingual OCR routing (Latin, CJK, Cyrillic, Arabic), searchable sandwich PDF creation with invisible text (`3 Tr`), and ISO/IEC 29500 OpenXML DOCX, PPTX, and HTML5 document export, directly targeting **Milestone M10** (OCR, Scan & Conversion).

---

## Parity Gaps Addressed

### Milestone M10 (OCR, Scan & Conversion)
* `[P0]` **OCR Document Language Selection:** Multi-model routing covering Latin (English, German, French, Spanish), CJK (Simplified & Traditional Chinese, Japanese, Korean), Cyrillic (Russian), and Arabic.
* `[P0]` **Searchable Image Output (Invisible Text Layer `3 Tr`):** Sandwich PDF synthesizer emitting `3 Tr` invisible text glyphs perfectly positioned and scaled over raster scan words.
* `[P1]` **Scan Preprocessing (Deskew, Despeckle & Binarize):** Automated skew detection via projection profile variance, 3x3 median despeckling, and Otsu global thresholding.
* `[P1]` **High-Fidelity DOCX & PPTX Export:** Clean-room generation of ISO/IEC 29500 WordprocessingML (`w:p`, `w:r`, `w:tbl`) and PresentationML slides preserving headings, styles, and tables.
* `[P1]` **Semantic HTML5 Export:** Structured HTML5 output with accessible tables, headings, and styling.

---

## Crate Layout

```
bundle_6_multilingual_ocr_and_office_export/
├── Cargo.toml
├── README.md
├── crates/
│   ├── scan_cleanup/     # Hough deskewing, Otsu binarization, median despeckle
│   ├── multilingual_ocr/ # Multilingual models, sandwich generator (3 Tr), OCR pipeline
│   └── office_export/    # ISO/IEC 29500 DOCX, PPTX, and semantic HTML5 builders
└── tests/
    └── integration_tests.rs
```
