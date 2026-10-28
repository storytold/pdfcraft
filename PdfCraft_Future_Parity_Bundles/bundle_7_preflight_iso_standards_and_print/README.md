# Bundle 7: Preflight Rule Engine, ISO Standards & Print Production

## Overview
Bundle 7 implements an extensible preflight rule verification engine, automated single checks and fixups, ISO 19005 (PDF/A-1b/2b/3b/4) and ISO 15930 (PDF/X-1a/3/4) compliance profiles, PDF Optimizer image downsampling, ISO 32000-1 Annex F Fast Web View linearization, and CMYK pre-press separation inspection with Total Area Coverage (TAC) warnings, directly targeting **Milestone M11** (Preflight & Standards).

---

## Parity Gaps Addressed

### Milestone M11 (Preflight & Standards)
* `[P1]` **Preflight Rule Engine & Single Fixups:** Customizable preflight profiles with automated fixups (inserting missing OutputIntents, correcting box geometries, stripping disallowed actions).
* `[P1]` **ISO 19005 (PDF/A-1b, 2b, 3b, 4):** Standard compliance verification (OutputIntents `/GTS_PDFA1`, font embedding enforcement, encryption prohibition, XMP `pdfaid` synchronization).
* `[P1]` **ISO 15930 (PDF/X-1a, 3, 4):** Pre-press exchange conformance requiring TrimBox/BleedBox and CMYK color enforcement.
* `[P1]` **PDF Optimizer:** Bicubic area-averaging image downsampling, Flate recompression, and unused object pruning.
* `[P1]` **Linearized Save (Fast Web View):** ISO 32000-1 Annex F primary hint stream generation and first-page byte-range streaming layout.
* `[P1]` **CMYK Separation Previews & TAC Limits:** Cyan, Magenta, Yellow, Black plate extraction and Total Area Coverage ink limit calculation (>300% warning).

---

## Crate Layout

```
bundle_7_preflight_iso_standards_and_print/
├── Cargo.toml
├── README.md
├── crates/
│   ├── preflight_engine/  # Rule execution framework, fixups & audit reports
│   ├── iso_standards/     # ISO 19005 (PDF/A) and ISO 15930 (PDF/X) profiles
│   ├── optimizer/         # Image downsampler, stream recompression & pruning
│   ├── linearizer/        # ISO 32000 Annex F Fast Web View linearizer & hint streams
│   └── print_production/  # CMYK separations preview & TAC calculation
└── tests/
    └── integration_tests.rs
```
