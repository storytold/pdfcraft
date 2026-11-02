# Localization parity

> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-13 · **Change:** minor (Vietnamese 100% of current key union; native-speaker community review; twelve-language mean ≈68%) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)
> **Last reviewed:** 2026-10-10 · **Last updated:** 2026-10-14 · **Change:** minor (Italian count after completing its catalog) · **Target:** Adobe Acrobat Pro (Acrobat DC, continuous track 26.002.21931, macOS)

Per-language status of PdfCraft's interface against Acrobat Pro's. How translations work and how to
add a language: [localization.md](localization.md). Summary in [ROADMAP.md](../ROADMAP.md).

## How this was measured

- **Strings translated** is measured: the catalogs in `crates/ui-egui/src/i18n/<code>.tsv`, counted
  as distinct `(context, English)` keys with a non-empty translation. The **current** denominator is
  the union of keys across all catalogs (**2,590** plain keys as of 2026-10-13, plus language-specific
  context rows such as Vietnamese `theme`/`Appearance`). The twelve-language percentage table below
  still quotes the **historical 2,143-key snapshot** from 2026-10-10 for every language except
  Vietnamese (re-measured against the current union). English is built in.
- **Not counted:** error details from the engine and the operating system, which are shown in
  English inside a translated frame; there is no translated user documentation or help yet (Acrobat
  ships localized help).
- **Script support** comes from [localization.md](localization.md), the vendored winit IME patches
  (`vendor/README.md`) and user reports.
- **Native review**: "community" means a native speaker contributed or corrected the catalog in a
  pull request or issue; "no" means no record of review. No catalog has had a formal review.
- **Acrobat** ships 24 interface localizations on macOS (`.lproj`: cs, da, de, en, es, fi, fr, hu,
  it, ja, ko, nl, no, pl, pt, ru, sv, tr, uk, zh_CN, zh_TW, plus en_AE, en_IL, fr_MA) and Middle
  East / North Africa editions (MEA Arabic, MEH Hebrew, NAF French; `Resources/Sequences/`).

## The twelve key languages

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status | Acrobat ships it | To `full` (h) |
|---|---|---|---|---|---|---|---|---|
| English | `en` | built in (100%) | all; README and docs | Latin | yes | **full** | yes | 0 |
| Simplified Chinese | `zh-hans` | 2,088 (97.4%) | dialogs and tooltips through `tl!`; no help | CJK text and IME work, but **release builds have no Hans face**: many characters show as boxes (#826, #688, #728, #758; Flatpak #689) | no | partial | yes | 6–12 |
| Spanish | `es` | 2,046 (95.5%) | dialogs and tooltips; no help | Latin | no | partial | yes | 2–4 |
| Hindi | `hi` | 0 | none | Devanagari not tested (egui shapes Telugu conjuncts, so shaping is likely workable); no Devanagari face in craft-fonts | no | none | no | 10–18 |
| Arabic | `ar` | 2,143 (100%) | dialogs and tooltips; no help | Letters joined and lines reordered at load, but **layout not mirrored**, wrapped lines break at the wrong end, typed text runs left to right (`localization.md` §Arabic) | no | partial | yes (Middle East edition) | 25–45 |
| French | `fr` | 2,128 (99.3%) | dialogs and tooltips; no help | Latin | no | partial | yes | 1–2 |
| Portuguese (Brazil) | `pt-br` | 2,130 (99.4%) | dialogs and tooltips; no help | Latin | community (#288) | partial | yes | 1–2 |
| Indonesian | `id` | 0 | none | Latin | no | none | no | 3–5 |
| Japanese | `ja` | 2,123 (99.1%) | dialogs and tooltips; no help | BIZ UDPGothic from craft-fonts; IME patched on macOS, Windows, X11, Wayland; vertical text renders but can't be edited | no | partial | yes | 4–8 |
| German | `de` | 2,128 (99.3%) | dialogs and tooltips; no help | Latin | community, in progress (#828: terminology, clipping) | partial | yes | 2–4 |
| Korean | `ko` | 0 | none | Hangul IME patched (Microsoft and Apple Korean, `vendor/README.md` winit 3, 6, 9, 10); no Hangul UI face in craft-fonts | no | none | yes | 6–10 |
| Vietnamese | `vi` | 2,591 (100% of current union + theme Appearance) | dialogs and tooltips; no help | Latin with stacked diacritics; bundled Inter covers them (fonts test) | community | partial | no | 2–4 |

**Localization dimension ≈ 68%**: the mean over the twelve, counting a catalog's percentage but only
60% for Chinese and Arabic until their script problems are fixed, and 0 for Hindi, Indonesian and
Korean (still missing). Vietnamese moved from 0% to 100% in that mean (59% → 68%). **≈ 55–110 h**
to bring all twelve to `full`, of which ≈ 35–60 h is engineering (Hans face, RTL layout, bidi text)
and the rest catalogs and fixes; native-speaker review needs humans.

No language is `full` except English: every catalog leaves engine error details in English, and
none has translated help. Vietnamese's **To `full`** hours are therefore residual help/docs polish
(catalog and script work are already done), not missing UI strings.

## Other shipped languages

| Language | Code | UI strings translated | Status | Acrobat ships it |
|---|---|---|---|---|
| Traditional Chinese | `zh-hant` | 2,016 (94.1%) | partial (same Hans/Hant face caveat) | yes |
| Russian | `ru` | 2,043 (95.3%) | partial | yes |
| Bulgarian | `bg` | 2,094 (97.7%) | partial | no |
| Czech | `cs` | 135 (6.3%) | menus only | yes |
| Telugu | `te` | 2,058 (96.0%) | partial (the release-pinned craft-fonts has no Telugu face yet) | no |
| Hungarian | `hu` | 2,126 (99.2%) | partial | yes |
| Ukrainian | `uk` | 2,142 (100%) | partial | yes |
| Italian | `it` | 2,528 entries (every key of the other catalogs that the code looks up) | partial | yes |
| Kazakh | `kk` | 2,327 entries | partial | no |
| Polish | `pl` | 2,287 entries | partial | yes |
| Slovenian | `sl` | 2,324 entries | partial | no |
| Greek | `el` | 2,341 entries | partial | no |
| Turkish | `tr` | 2,313 entries | partial | yes |
| Lithuanian | `lt` | 2,585 entries | partial; community native-speaker review ([#1401](https://github.com/storytold/pdfcraft/pull/1401)) | no |

**14 other languages** ship (22 catalogs plus English = **23** interface languages in Preferences).
The entry counts without percentages were measured on 2026-10-12–13; they are not percentages of the older 2,143-key snapshot above. Their presence does not imply a detailed linguistic or runtime review.
Acrobat languages PdfCraft lacks: Danish, Dutch, Finnish, Norwegian, Swedish, Hebrew. Vietnamese is shipped here but not by Acrobat.
**14 other languages** ship (21 catalogs plus English, 22 interface languages in Preferences).
The entry counts without percentages were measured on 2026-10-12 (Italian on 2026-10-14); they are not percentages of the older 2,143-key snapshot above. Their presence does not imply a detailed linguistic or runtime review.
Acrobat languages PdfCraft lacks: Danish, Dutch, Finnish, Norwegian, Swedish, Hebrew.

## Revision history

| Date | Change | Summary |
|---|---|---|
| 2026-10-13 | minor | Vietnamese (`vi`) catalog: 2,591 entries (100% of current key union + `theme`/`Appearance`); twelve-language mean ≈ 68%; native-speaker community review; registered with `plural_none` |
| 2026-10-14 | minor | Italian: 2,528 entries after adding the 201 keys the other catalogs had and the code looks up |
| 2026-10-12 | minor | Added current counts for Kazakh, Polish, Slovenian, Greek, Turkish and Lithuanian; retained existing language assessments and the original percentage snapshot |
| 2026-10-10 | major | Created. Catalog counts measured from `crates/ui-egui/src/i18n/*.tsv` against the 2,143-key union; Acrobat's localizations from its bundle's `.lproj` and Sequences folder names |
