# Interface language

Choose **Menu > Edit > Preferences…** (Command-comma on macOS, Ctrl-comma elsewhere) **> Interface language** and select **Auto**, **English**, **日本語**, **简体中文**, **繁體中文**, **Русский**, **Български**, **Čeština**, **Português (Brasil)**, **Deutsch**, **Español**, **Français** or **తెలుగు**. The change applies immediately and persists between launches. Command ids, document contents and file names are unchanged.

**Auto** (the default) follows the system language: `LC_ALL`, `LC_MESSAGES` or `LANG`, then the preferred-languages list on macOS. Any Portuguese locale (`pt_BR`, `pt_PT`) uses the Brazilian catalog, any French locale (`fr`, `fr_FR`, `fr_CA`, `fr_BE`) the French one, any German locale (`de`, `de_DE`, `de_AT`, `de_CH`) the German one, and Chinese locales for Taiwan, Hong Kong and Macau (`zh_TW`, `zh_HK`, `zh_MO`, `zh-Hant`) the Traditional Chinese one; other Chinese locales (`zh`, `zh_CN`, `zh_SG`, `zh-Hans`) the Simplified Chinese one. Telugu locales (`te`, `te_IN`) use the Telugu catalog, and Bulgarian locales (`bg`, `bg_BG`) the Bulgarian one. A system language without a catalog shows English. On Windows, **Auto** uses the Windows display languages, in preference order, when no supported locale is selected by those environment variables. The query asks Windows directly (`GetUserPreferredUILanguages`, through the `sys-locale` crate) without starting a process, and is cached for the process. If detection fails or the display language has no catalog, PdfCraft shows English.

The control channel exposes the setting through `ui.set`:

```json
{"method":"ui.set","params":{"key":"language","value":"ja"}}
```

The value is `auto` or a language code (`en`, `ja`, `zh-hans`, `zh-hant`, `ru`, `bg`, `cs`, `pt-br`, `de`, `es`, `fr`, `te`), in any case. `ui.state` reports `language` as written in the preferences (`auto`, `en`, `ja`, `zh-hans`, `zh-hant`, `ru`, `bg`, `cs`, `pt-br`, `de`, `es`, `fr` or `te`). Unknown values return an error without changing the current setting. Preferences saved before this setting existed follow the system language.

Dialogs, panels, menus, notices and history labels go through the catalog (`tl!`). Japanese, Traditional Chinese, Simplified Chinese, Russian, Bulgarian, German, Spanish, French and Telugu translate them (about 1,800 entries each); Czech and Brazilian Portuguese cover the menus so far. Untranslated labels use English. Error details that come from the engine or the operating system are shown as they are, inside a translated frame ("操作「…」に失敗しました: …"). The command palette matches the translated label, the English label and the command id. Vertical Japanese PDF rendering is an existing viewer feature; this change does not add vertical text editing.

Whole sentences that the layers below word — what the accessibility checker found, what the PDF/A verifier reports, what a signature's details say, what is wrong with the pages to print, what an edit is called in the history — are translated too, because those layers hand the sentence over apart from the document's own words. See [Sentences the engine words](#sentences-the-engine-words).

Japanese interface text uses BIZ UDPGothic from [craft-fonts](https://github.com/storytold/craft-fonts), an optional build input that every release includes (`CRAFT_FONTS_DIR`; see the README). A build made without it has no Japanese face, so Japanese labels show replacement boxes. Chinese is drawn with the same craft-fonts CJK faces (in Simplified Chinese the `Hans` faces come first, so a line never mixes faces with different baselines), so a character none of them has shows a replacement box (and every Chinese character does in a build without craft-fonts). Russian, Bulgarian, Czech, Brazilian Portuguese, German, Spanish and French need only the bundled Latin faces (tested in `tests/fonts.rs`). Telugu is drawn with a craft-fonts `Telu` face (Noto Sans Telugu, kept in the web build too), whose conjuncts egui shapes; a build without one shows Telugu labels as replacement boxes, and the release-pinned craft-fonts input has no Telugu face yet.

There is no right-to-left interface language yet. File names and document titles in Arabic or Hebrew are drawn with a craft-fonts `Arab` face when the build input has one; on desktop, a character no embedded face has is drawn with one font already installed on the machine (`PDFCRAFT_SYSTEM_FONTS=0` turns this off). The tab strip, the recent-files list and Properties show such names in display order; other places (bookmarks, comments, search results) have the glyphs but may order mixed-direction text wrongly.

Simplified Chinese, French and German coverage tests, following PhotoCraft's complete-catalog approach, check every registered command, All tools label and `tl!("literal")` in the UI source. Simplified Chinese also checks direct `i18n::t` calls and multiline literals, plus the About tabs, contributor name modes, sort options, table headers and model columns. Generated history labels, contributor summaries and diagnostics are tested with user-supplied names and error details. Catalog coverage does not guarantee glyph coverage: the release-pinned craft-fonts input currently has no `Hans` face, and the web build embeds only BIZ UDPGothic Regular. Chinese font delivery remains separate work, as discussed in [#112](https://github.com/storytold/pdfcraft/pull/112) and [#140](https://github.com/storytold/pdfcraft/pull/140).

## How translations work

PdfCraft uses the same system as PhotoCraft (`crates/ui-egui/src/i18n/`):

- Strings in the code stay in English and serve as lookup keys. At draw time, `tl!("Save")` returns the text in the current language, and falls back to English when there is no entry.
- Each language has a catalog, `crates/ui-egui/src/i18n/<code>.tsv`, with one entry per line: `context<TAB>English<TAB>translation`. An empty context marks a plain string. `@id` keys an entry by command id (for example `file.saveAs`), so one menu item can read differently from another with the same English label. `@plural` holds plural forms. Any other context disambiguates an English word with several meanings (`tl_ctx!` in UI code, for example the signature pad's "Type" button against the "Type" column). The header of `ja.tsv` documents the format and escapes.
- Placeholders (`{name}`) must appear in both columns. A trailing `…` must be kept. `i18n::fmt` fills placeholders in one pass, so a value such as a file name containing `{n}` is inserted as written.
- Catalogs are validated strictly: unknown escapes, unknown `@` contexts, placeholder or ellipsis mismatches, wrong plural form counts and duplicates are errors. The tests require the bundled catalogs to have none; at run time a bad line is skipped (and logged) and that string shows in English.
- Catalogs are assets: each `.tsv` has an `ATTRIBUTION.toml` entry with `kind = "translation"` and its SHA-256, checked by `cargo xtask assets`.
- Translations are clean-room: write them from the meaning of the English text, never from another product's string tables.

## Sentences the engine words

Some sentences are not the interface's own. The accessibility checker, the PDF/A verifier, the signature reader, the page-range parser and the edit history all word their own findings, and matching that wording in the UI with a `tl!` literal would break silently whenever the engine changed it. So those layers hand the wording over instead, apart from the document's own words:

- Each of them has a `WORDINGS` constant listing every sentence it can say, as `&'static str` with holes in it (`{n}`, `{p}`, `{}`). The sentence is written once, in the layer that means it.
- `said(&words)` says one of them: the layer looks its own wording up through the closure it is given, then fills the holes with the document's values — a page number, a font name, what the person typed — in one pass, so a value containing `{n}` is inserted as written.
- The UI passes `&crate::i18n::words`, which is a catalog lookup, so the sentence reads in the interface language. `in_english` is the same thing in English, and a wording with no entry comes back as it stands, which is the English — a catalog missing one shows an English sentence rather than a gap.
- The lists are `a11y::{WORDINGS, REPORT_WORDS}`, `preflight::WORDINGS`, `sign::pdf::{WORDINGS, kind::ALL}`, `print::WORDINGS` and the engine's `{EDIT_LABELS, BATCH_LABELS}`. `a11y::report_html` takes a language tag and a `words` closure, so a saved report reads the way the panel read, down to its `lang` attribute.
- `spanish_covers_what_the_engine_says` requires an entry for every wording in those lists, and `spanish_keeps_the_holes_in_what_the_engine_says` requires each translation to keep the same holes, so the catalog cannot fall behind the engine without a test saying so. Other languages are free to add the entries when they are translated; until then those sentences show in English as before.

Error details that come from the operating system, and faults rather than mistakes (a file that cannot be read, a lock that was poisoned), still carry their own text and are shown as they are.

## Adding a language

1. Copy the header of `ja.tsv` into `xx.tsv` and translate entries.
2. Add one row to `LANGUAGES` in `crates/ui-egui/src/i18n/mod.rs`: code, native name, catalog and plural rule.
3. Add an `ATTRIBUTION.toml` entry for the catalog (`kind = "translation"`), then run `cargo xtask assets --write` and `cargo xtask assets`.

The Preferences dropdown, the system-language match and the catalog tests (format, duplicates, placeholders, plural forms, command ids) then pick it up.
