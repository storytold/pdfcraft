# Bundle 4: In-Place Typography Editing, Font Subsetting & Reflow

## Overview
Bundle 4 implements in-place text editing, font subset expansion, TrueType/OpenType glyph injection, `/ToUnicode` CMap synchronization, paragraph reflow with hanging bullet indents, bi-directional (RTL/CJK) layout, vector path manipulation, and legal Bates numbering, directly targeting **Milestone M7** (Content Editing).

---

## Parity Gaps Addressed

### Milestone M7 (Content Editing)
* `[P0]` **Edit Existing Text In Place:** Font subset expansion (`FontSubsetExpander`) dynamically allocating new GIDs, metrics, and `/ToUnicode` mappings when typing new characters into embedded subset fonts.
* `[P1]` **Line & Paragraph Spacing Reflow:** Word wrapping, dynamic leading calculation, and hanging indentation for bulleted lists (`•`, `-`, `*`).
* `[P1]` **Bi-Directional (RTL & CJK) Text:** Unicode Bidi run classification isolating RTL (Arabic, Hebrew) and CJK character streams.
* `[P1]` **Keep Structure Tags (/MCID):** Preserving marked content identifiers (`/Span << /MCID ... >> BDC ... EMC`) across AST content delta patches.
* `[P1]` **Vector Path Manipulation:** Select, translate, scale, recolor, and delete vector bezier paths in content streams.
* `[P1]` **Bates Numbering:** Legal multi-file sequential Bates stamping with custom prefixes, suffixes, digit padding, and position controls.

---

## Crate Layout

```
bundle_4_typography_editing_subsetting_and_reflow/
├── Cargo.toml
├── README.md
├── crates/
│   ├── subsetter/      # OpenType font subset expander, glyph injector & ToUnicode
│   ├── reflow/         # Paragraph reflow, bullet lists, Bidi RTL/CJK segmentation
│   └── content_editor/ # AST content stream patcher, vector manipulation & Bates numbering
└── tests/
    └── integration_tests.rs
```
