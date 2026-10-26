# Korean text support

> **Last reviewed:** 2026-10-22 · **Last updated:** 2026-10-22 · **Change:** created · **Scope:** rendering, interface fallback and editing of Korean (Hangul) text

What PdfCraft can and cannot do with Korean text today, which font carries it, and where each
behaviour is pinned by a test. Korean interface *translation* (a `ko` catalog) is separate work:
see [localization-parity.md](localization-parity.md) and gap #14 in [gaps.md](gaps.md).

## The font

**학교안심 나무 / Hakgyoansim Namu** (KERIS, designed by Design210, OFL-1.1), lives in
[storytold/craft-fonts](https://github.com/storytold/craft-fonts) like every shared font
(`AGENTS.md` §1.4): Regular and Light TrueType, the full 11 172-glyph U+AC00–U+D7A3 syllable
block plus modern and compatibility jamo. Its licence file travels with the package and with
`CRAFT_FONTS_DIR` releases (`packaging/env.sh`, `copy_font_licences`).

**Provenance.** The committed bytes (SHA-256 in `fonts/manifest.txt`) were downloaded on
2026-10-22 from KERIS's own official release flow — `copyright.keris.or.kr/wft/fntDwnld`
(font id `GFT202312110000000000013`, government CDN `font-real.cdn.gov-ntruss.com`, folders
`2023-12-11/1049` and `1054`) — and compared byte-for-byte: identical. The craft-fonts
ATTRIBUTION row records this; re-verify against a fresh official download if the file is ever
replaced.

The script tag is `Kore` (ISO 15924). `crates/fonts/build.rs` embeds it with every other manifest
face under `CRAFT_FONTS_DIR`; builds without the checkout keep working and every consumer below
degrades to a clear message, never to wrong glyphs.

## What works (each line cites its test)

| Path | Behaviour | Evidence |
|---|---|---|
| Interface fallback | `ui_korean_fonts()` joins the egui family chain after CJK/Arabic/Telugu/Hebrew/Thai, so file names and titles in Hangul draw real glyphs | `crates/ui-egui/tests/fonts.rs::korean_ui_text_uses_craft_fonts` |
| Viewing a PDF | a non-embedded CID font whose `/CIDSystemInfo /Ordering` is `Korea1`/`KR` substitutes the Korean face and says so (`Why::ScriptFace`); rasterization of such pages draws with it | `crates/fonts/src/fallback.rs::korean_depends_on_craft_fonts_and_says_which_it_got`, `crates/render/src/raster.rs::non_embedded_korean_cid_fonts_draw_with_a_craft_fonts_face` |
| Editing existing text | Hangul replacement text keeps its own face on the line (Latin and digits too, one look per line), generated as one-glyph-per-character Type 3 fonts named `PCKo…` / `/Name PCKorean`; characters the face lacks are taken from the Japanese faces; longer runs split across fonts and still read as one line; the source font is reused whole when it already covers every character | `crates/edit/src/tests.rs::korean_replacement_text_uses_the_korean_face`, `crates/edit/src/tests.rs::korean_replacement_without_a_korean_face_says_so`, `crates/automation/tests/automation.rs::editing_existing_korean_text_through_tools` |
| Embedding | any full Unicode face including Hangul round-trips through `pdfcraft_fonts::embedding::embed` (identity `PCFullFont` program, UCS-2 CIDs, ToUnicode) | `crates/fonts/src/embedding.rs::korean_text_round_trips_through_the_korean_craft_face` |
| CLI demo | `cargo run -p pdfcraft-fonts --example korean -- out.pdf` writes a Hangul page with the embedded face | manual; covered programmatically by the embedding test above |

Parity entries updated: `view.render-cid-fonts` and `edit.text-reflow` in
`parity/acrobat-features.toml`.

## Limits and open ends

- **Add content ▸ text**: a new Added-text item containing Hangul is still refused by
  `first_undrawable` (Japanese text has the same limit; Arabic/Hebrew/Thai each have their own
  shaped writer). Wiring Korean into Added-text is open work.
- **Decomposed jamo**: replacement text draws one Type 3 glyph per character, so a decomposed
  jamo sequence (e.g. U+110B U+110B) draws as individual jamo instead of composing; precomposed
  syllables — every normal Korean input method's output — are exact.
- **One design**: Namu is a display-weight text face with only Regular and Light. Missing-bold
  requests resolve by NFSS to those faces; no synthetic bold is drawn. Korean releases get no
  serif Korean face (none is licensed in craft-fonts).
- **GB1 / CNS1** (Chinese orderings) still substitute nothing: the only openly licensed faces
  that would cover them are Source Han / Noto CJK, excluded by `AGENTS.md` §1.1.
- **A Korean UI catalog** (`ko`) does not exist yet; nothing here translates interface strings.
- NAMU's name table leaves the licence field empty (KERIS ships it that way), which is fine for
  craft-fonts embedding and `validate()`; the user-picks-a-font document-input validator
  (`validate_document_input`) requires the name table to declare its licence, so Namu chosen
  *there* would be refused — an upstream reporting item, not a blocker for the paths above.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-22 | major | Created. Korean wired into the interface chain, CID substitution, renderer, editor Type 3 replacement and embedding; KERIS byte-match recorded |
