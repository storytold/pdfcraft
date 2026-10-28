# Bundle 3: Annotations, Portable Appearance Streams & Sandboxed JS Forms Engine

## Overview
Bundle 3 implements full appearance stream synthesis (`/AP`) for all 18 ISO 32000 markup types, XHTML rich text comments (`/RC`), AcroForm interactive field regeneration (auto-size, comb, multiline, password), and fuel-limited ECMAScript sandbox execution with Acrobat DOM bindings, directly targeting **Milestone M5** (Comments & Review) and **Milestone M6** (Forms & Scripting).

---

## Parity Gaps Addressed

### Milestone M5 (Comments & Review)
* `[P0]` **Appearance Stream (`/AP`) Generation for All 18 Markup Types:** Complete portable `/AP << /N stream >>` synthesis for Highlight, Underline, StrikeOut, Squiggly, FreeText, Line (all 10 line-ending styles), PolyLine, Polygon, Square, Circle, Caret, Stamp, Ink, Popup, FileAttachment, Sound, Redact, Link.
* `[P0]` **Text Box Comment with Rich Text (`/RC`):** XHTML-compliant rich text styling engine parsing inline formatting (bold, italic, colors, leading).
* `[P1]` **Caret Insertion Proposals & Line Endings:** Text insertion markups and OpenArrow, ClosedArrow, Circle, Diamond, Butt, Slash endings.

### Milestone M6 (Forms & Scripting)
* `[P0]` **Push Buttons & Action Execution:** Button event dispatcher handling mouse events (`/U`, `/D`, `/E`, `/X`) and executing ResetForm, SubmitForm, and JavaScript actions.
* `[P0]` **Field Appearance Regeneration:** Dynamic auto-sizing font calculator (filling cell height), Comb fixed-width formatting (`/Ff 16777216`), multi-line word wrapping, and bullet-masked password fields.
* `[P0]` **JavaScript Engine with Acrobat Object Model:** Sandboxed execution engine with fuel quota protection against infinite loops, providing `app` (alert, beep), `Doc` (getField, calculate), `event` (value, target, change, rc), and Acrobat calculation built-ins (`AFSimple_Calculate`, `AFNumber_Format`, `AFRange_Validate`).

---

## Crate Layout

```
bundle_3_annotations_appearances_and_scripted_forms/
├── Cargo.toml
├── README.md
├── crates/
│   ├── appearances/   # Appearance streams for all 18 markups & rich text
│   ├── forms_engine/  # AcroForm appearance regeneration (auto-size, comb, multiline, password)
│   └── js_sandbox/    # Fuel-limited ECMAScript sandbox with Acrobat DOM host objects
└── tests/
    └── integration_tests.rs
```
