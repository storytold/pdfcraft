# Syriac PDF content

This contribution is made in honor of the Maronite Church and its cultural history.

This package adds Syriac document text through the existing engine and automation
tools. It does not translate the interface or qualify Windows/browser keyboard
composition. Content editors receive the approved Syriac glyph fallback so their
text can be displayed; no interface language or layout is added.

## Font input and shaping

Build with `CRAFT_FONTS_DIR` pointing to the companion craft-fonts contribution.
The companion is [craft-fonts #61](https://github.com/storytold/craft-fonts/pull/61),
qualified at `758bdc0ea05af952d66ded15572d127f877866f1`.
That contribution contains the unmodified, static Noto Sans Syriac Regular faces
from the original [Noto Syriac releases](https://github.com/notofonts/syriac/releases):
Estrangela 3.000, Western/Serto 3.001 and Eastern 3.001. Their OFL notices,
source revisions and SHA-256 records live in craft-fonts. No font files are added
to PdfCraft. The default generated face is Estrangela. The `syriac_family` argument
selects `estrangela`, `eastern` or `western` for added page text, form properties,
FreeText/callouts, Fill & Sign text or typed signatures, headers/footers and text
watermarks. Existing paragraphs select an explicitly loaded full face through
`text_font_import` and `font_resource`.

Added text, field/widget properties and FreeText annotations store the chosen
family so unrelated edits and appearance regeneration retain it. `content_list`,
`form_fields` and `comment_list` report the stored choice where it applies.
Omitting the option during an edit preserves that choice. Typed signature outlines
use the selected family when created; changing them requires recreating the
signature. Picture-only marks and unrelated annotation/field kinds reject this
text-only option. Other scripts retain their existing font selection.

Joining and contextual substitutions come from the face's OpenType GSUB rules;
mark placement comes from its shaping data. Mixed Syriac, Latin and numbers use
UAX #9 visual runs while retaining logical Unicode. Generated cluster fonts have
ToUnicode mappings and line ActualText. Typed signatures are drawn as outlines.

Without a Syrc build input, new page text is refused with an actionable font error.
Form and comment values retain their existing undrawn-character warning policy;
retaining a value does not mean its appearance could be drawn. The selected font
releases lack the Syriac Supplement U+0860–U+086A; it is not claimed as supported.

## Content surfaces

- Page Add Text and updates: `page_add_text`, `content_update`, text extraction,
  search, history and saving/reopening.
- Existing upright paragraphs: `text_edit` and `text_edit_layout`, with a complete
  embedded source face, an approved full fallback or an explicit `font_resource`.
  Existing single-line replacement cannot safely preserve contextual geometry:
  use the paragraph path. Turned baselines and complex contextual style changes
  remain restricted.
- Text and choice fields, comb values and push-button captions through the existing
  form tools. Password fields retain masking. Comb cells keep marks with their base
  and shape cells independently, without joining letters across cell gaps.
- FreeText text boxes, callouts and Fill & Sign text, including regenerated and
  missing appearances; typed signatures use the same shaped glyph outlines.
- Header/footer and Bates template text, plus text watermarks, through the existing
  document mark tools.

Other annotation caption writers, rich-text markup, arbitrary malformed source
fonts and other vendors' editing interoperability are not qualified by this package.
There are no new network, MCP or control-channel defaults.

## Limits and verification

Shared generated layout caps text at 262,144 UTF-8 bytes and 1,024 rows. Lines are
wrapped at spaces; an overlong joining word is not split through its letters or
marks and can overflow the available width. Choice appearance layout caps options
at 1,024, comb values at 4,096 cells, and typed signatures retain the existing
256-character limit. Unsupported input follows the surface's existing error or
warning policy. Glyph-outline and font-resource budgets also apply.

The original feature batch at `df4de1d9` passed on 2026-11-02 with the companion revision above,
repository optimization defaults, at most two compiler jobs and
`PDFCRAFT_REQUIRE_SYRIAC=1` so the Syriac tests could not skip missing fonts:

- `cargo test -p pdfcraft-fonts --test syriac_content --test cluster_budget -j 2`:
  three shaping/source/wrapping cases and one repeated-glyph output-budget case.
- The two focused forms units: checkbox/radio state names retain their non-text
  appearance path, and sparse comb auto-size uses MaxLen and independent-cell
  geometry. Each failed with its relevant guard or cell-count division removed and
  passed after restoration. The comb geometry unit is not an end-to-end red proof.
- `cargo test -p pdfcraft-automation --test syriac_content -j 2`: all four cases
  passed, covering existing paragraphs, added content/history/search, generated
  appearances and selected-family persistence/refusal. Each family's isolated
  content regions render differently, and their readback/pixels survive save/reopen.
- All-target strict Clippy with `-D warnings` passed for fonts, edit, annot, forms,
  engine, automation and xfdf; `cargo fmt --all --check` passed.

- Strict UI-library Clippy passed. Seven existing upstream lint diagnostics in five
  UI files received equivalent mechanical corrections; no GUI executable was built.
- The WASM check passed for the seven content crates above, with five existing
  render-crate unused/dead-code warnings. Worktree-rooted layers, assets and parity
  checks passed; attribution was generated from the manifest.
- A single 13-step CLI replay created all three families, a form field, FreeText,
  a typed signature and page marks, then saved and rendered the synthetic document.
  The resulting image was inspected for visible joining/marks and placement, with
  no obvious clipping. The output is local verification material, not a committed
  PDF or a claim of native-reader typography acceptance.

### Review 5740207590 corrections

Default FreeText retains its standard-font placeholder appearance and logical
contents when Syriac drawing is unavailable. Restyling uses that same fallback.
A malformed or unbuildable Syriac comment no longer discards other comments'
display-only appearances; the input document remains unchanged.

Form fitting uses Syriac measurements when successful, and otherwise uses the
existing field/Helvetica placeholder metrics. Short unsupported insertions remain
stored with their normal undrawn-text warning. Actual overflow, including a long
Latin insertion beside an unsupported glyph, still uses the existing trimming
policy. This is not an unconditional bypass of field size limits.

Two annotation regressions and the refined form regression passed after witnessed
pre-correction failures. The supported three-family integration case also passed;
its successful shaping path was unchanged by the final error-only fit refinement.
Only the affected form test was rerun for that refinement. Earlier font assets,
source-shaping and CLI checks were not repeated without a relevant change.

Ordinary builds without the optional companion retain their no-font behavior.
Native-reader judgment and desktop/browser keyboard composition are separate from
these content tests. Official release and CI workflows currently pin an older
craft-fonts revision; the companion
must be accepted and the build-input pin advanced before release availability can
be claimed. Per repository rules, CI changes are not part of this feature PR.

Shaping behavior follows the public [Syriac OpenType guidance](https://learn.microsoft.com/en-us/typography/script-development/syriac)
and [Unicode script specification](https://www.unicode.org/versions/Unicode16.0.0/core-spec/chapter-9/).
