# MPMB character-sheet compatibility

> **Last reviewed:** 2026-10-31 · **Last updated:** 2026-10-31 · **Status:** partial; full-sheet calculations and advanced controls are not qualified.

The supplied MPMB v13.2.3+241220 Colorful A4 sheet is an AcroForm application,
with six visible pages, thousands of fields, and 25 document scripts totaling
about 3 MiB. Filling a text field alone does not establish compatibility with
its character builder. The supplied PDF and its scripts are local diagnostic
inputs, not committed test fixtures or redistributable assets.

## Implemented foundations

- Ordinary AcroForm document libraries run on opening when JavaScript is enabled,
  before the field snapshot and renderer are created. Startup field changes and
  reset requests respect form-filling permissions. This lets the sheet hide its
  own unsupported-viewer badge; errors from unsupported later initialization
  calls are still reported. Active XFA initialization remains separate.
- Document libraries can be up to 1 MiB each, with an 8 MiB combined limit.
  Field scripts and code compiled from strings retain their existing limits;
  loop, recursion and nesting checks still apply. An oversized library is
  reported and skipped while other eligible scripts run.
- Repeated field lookups and the same field's `event.target` share one wrapper
  within a script context. Helpers and temporary properties attached by a
  document library are available to its calculation scripts. Native properties
  continue reading and updating the current field state. These JavaScript
  properties do not persist between separate script contexts.
- Custom document metadata keeps its original key spelling, allowing a script
  to read configuration such as `info.SheetVersion`. Standard metadata retains
  its existing scripting names such as `info.title`.

Synthetic regression coverage lives in `pdfcraft-js`, the engine's
`js::review_tests`, and the automation test
`character_calculations_with_large_libraries_survive_reopen`. The automation
case changes a score, calculates its modifier through a library-defined field
helper, saves, reopens, and changes the score again.

## Remaining compatibility work

- Initialization depends on dynamic `eval`/`Function` and legacy `toSource`
  serialization. AcroForm currently refuses code compiled from strings;
  supporting it requires preserving resource and parsing bounds.
- Dialogs, popup menus, dynamically populated choice lists, field metadata and
  rich-text styling need a wider scripting object model and viewer integration.
- Page-template spawning and deleting, extra spell sheets, bookmarks, buttons
  and custom content import/export need separate functional coverage.
- Each calculation currently initializes its own script context and libraries.
  Large calculation chains need performance work and persistent-state semantics
  before the full sheet can be called responsive or compatible.

The original PDF was observed opening as editable, with six pages and 4,352
fields read by the viewer, without a password. Startup execution was verified
on this sheet: the `d20warning` widget changes from visible to hidden when
opening and saving a local copy, while the original PDF remains byte-for-byte
unchanged. Synthetic raster coverage verifies that hidden startup badges are
absent from the rendered page, and that JavaScript-off keeps them visible. After the initial scripting
fixes, a `form_fill` of `Str` to `18` updated both `Str Mod` and `Str ST Mod`
to `4`. This full-sheet calculation pass took several minutes. Other character
systems and save/reopen on the supplied sheet remain unqualified; the synthetic
regression checks save/reopen independently. These results do not establish
that all calculations, dialogs, page creation or saved state work.

## Revision history

- 2026-10-31: Initialize ordinary AcroForm document libraries on opening; cover
  rendered warning visibility, disabled JavaScript, later script errors, reset
  requests and form-filling permissions with synthetic regressions.
- 2026-10-30: Record the supplied sheet's scope, initial scripting fixes, test
  entry points and remaining limitations. No full-sheet compatibility claim.
