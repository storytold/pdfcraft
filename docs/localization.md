# Interface language

Choose **Menu > Edit > Preferences…** (Command-comma on macOS, Ctrl-comma elsewhere) **> Interface language** and select **Auto**, **English**, **日本語**, **简体中文**, **繁體中文**, **Русский**, **Български**, **Čeština**, **Português (Brasil)**, **Deutsch**, **Español**, **Français**, **తెలుగు**, **Magyar** or **Українська**. The change applies immediately and persists between launches. Command ids, document contents and file names are unchanged.

**Auto** (the default) follows the system language: `LC_ALL`, `LC_MESSAGES` or `LANG`, then the preferred-languages list on macOS. Any Portuguese locale (`pt_BR`, `pt_PT`) uses the Brazilian catalog, any French locale (`fr`, `fr_FR`, `fr_CA`, `fr_BE`) the French one, any German locale (`de`, `de_DE`, `de_AT`, `de_CH`) the German one, and Chinese locales for Taiwan, Hong Kong and Macau (`zh_TW`, `zh_HK`, `zh_MO`, `zh-Hant`) the Traditional Chinese one; other Chinese locales (`zh`, `zh_CN`, `zh_SG`, `zh-Hans`) the Simplified Chinese one. Telugu locales (`te`, `te_IN`) use the Telugu catalog, Bulgarian locales (`bg`, `bg_BG`) the Bulgarian one, and Hungarian locales (`hu`, `hu_HU`) the Hungarian one, and Ukrainian locales (`uk`, `uk_UA`) the Ukrainian one. A system language without a catalog shows English. On Windows, **Auto** uses the Windows display languages, in preference order, when no supported locale is selected by those environment variables. The query asks Windows directly (`GetUserPreferredUILanguages`, through the `sys-locale` crate) without starting a process, and is cached for the process. If detection fails or the display language has no catalog, PdfCraft shows English.

The control channel exposes the setting through `ui.set`:

```json
{"method":"ui.set","params":{"key":"language","value":"ja"}}
```

The value is `auto` or a language code (`en`, `ja`, `zh-hans`, `zh-hant`, `ru`, `bg`, `cs`, `pt-br`, `de`, `es`, `fr`, `te`, `hu`, `uk`), in any case. `ui.state` reports `language` as written in the preferences (`auto`, `en`, `ja`, `zh-hans`, `zh-hant`, `ru`, `bg`, `cs`, `pt-br`, `de`, `es`, `fr`, `te`, `hu` or `uk`). Unknown values return an error without changing the current setting. Preferences saved before this setting existed follow the system language.

Dialogs, panels, menus, notices and history labels go through the catalog (`tl!`). Japanese, Traditional Chinese, Simplified Chinese, Russian, Bulgarian, German, Spanish, French, Telugu, Hungarian and Ukrainian translate them (about 1,800 entries each); Czech and Brazilian Portuguese cover the menus so far. Untranslated labels use English. Error details that come from the engine or the operating system are shown as they are, inside a translated frame ("操作「…」に失敗しました: …"). The command palette matches the translated label, the English label and the command id. Vertical Japanese PDF rendering is an existing viewer feature; this change does not add vertical text editing.

The Ukrainian catalog has 2,047 entries, written from the meaning of the English labels using Ukrainian computing vocabulary. Commands use infinitives; fixed-count messages use count-neutral wording. Plural-aware messages use the three [CLDR Ukrainian integer forms](https://www.unicode.org/cldr/charts/48/supplemental/language_plural_rules.html#uk): `1 сторінка`, `2 сторінки`, `5 сторінок`; 11–14 take the third form, while 21 and 22 return to the first and second. Calendar dates use genitive month names (`9 жовтня 2026`), while headings use standalone names (`Жовтень 2026`). Home recommendation cards grow to fit translated text; the toolbar search hint truncates before the shortcut, with the full label available on hover.

Japanese interface text uses BIZ UDPGothic from [craft-fonts](https://github.com/storytold/craft-fonts), an optional build input that every release includes (`CRAFT_FONTS_DIR`; see the README). A build made without it has no Japanese face, so Japanese labels show replacement boxes. Chinese is drawn with the same craft-fonts CJK faces (in Simplified Chinese the `Hans` faces come first, so a line never mixes faces with different baselines), so a character none of them has shows a replacement box (and every Chinese character does in a build without craft-fonts). Russian, Bulgarian, Czech, Brazilian Portuguese, German, Spanish, French and Hungarian need only the bundled Latin faces (tested in `tests/fonts.rs`). Hungarian uses the same one/other plural rule as English: 1 is singular, and every other count — including 0 — is the other form. After a numeral the noun stays singular (`1 oldal`, `2 oldal`). Telugu is drawn with a craft-fonts `Telu` face (Noto Sans Telugu, kept in the web build too), whose conjuncts egui shapes; a build without one shows Telugu labels as replacement boxes, and the release-pinned craft-fonts input has no Telugu face yet.

There is no right-to-left interface language yet. File names and document titles in Arabic or Hebrew are drawn with a craft-fonts `Arab` face when the build input has one; on desktop, a character no embedded face has is drawn with one font already installed on the machine (`PDFCRAFT_SYSTEM_FONTS=0` turns this off). The tab strip, the recent-files list and Properties show such names in display order; other places (bookmarks, comments, search results) have the glyphs but may order mixed-direction text wrongly.

Simplified Chinese, French and German coverage tests, following PhotoCraft's complete-catalog approach, check every registered command, All tools label and `tl!("literal")` in the UI source. Simplified Chinese also checks direct `i18n::t` calls and multiline literals, plus the About tabs, contributor name modes, sort options, table headers and model columns. Generated history labels, contributor summaries and diagnostics are tested with user-supplied names and error details. Catalog coverage does not guarantee glyph coverage: the release-pinned craft-fonts input currently has no `Hans` face, and the web build embeds only BIZ UDPGothic Regular. Chinese font delivery remains separate work, as discussed in [#112](https://github.com/storytold/pdfcraft/pull/112) and [#140](https://github.com/storytold/pdfcraft/pull/140).

German labels keep nouns capitalized, including tool names inserted into notices. The certificate viewer uses the `certificate` context for `Subject` (the certificate holder), while document properties keep the plain translation (the topic). Search hints reserve room for the shortcut, and Home cards limit their title and description to the available rows; hovering shows the full text when it is shortened.

## How translations work

PdfCraft uses the same system as PhotoCraft (`crates/ui-egui/src/i18n/`):

- Strings in the code stay in English and serve as lookup keys. At draw time, `tl!("Save")` returns the text in the current language, and falls back to English when there is no entry.
- Each language has a catalog, `crates/ui-egui/src/i18n/<code>.tsv`, with one entry per line: `context<TAB>English<TAB>translation`. An empty context marks a plain string. `@id` keys an entry by command id (for example `file.saveAs`), so one menu item can read differently from another with the same English label. `@plural` holds plural forms. Any other context disambiguates an English word with several meanings (`tl_ctx!` in UI code, for example the signature pad's "Type" button against the "Type" column). The header of `ja.tsv` documents the format and escapes.
- Placeholders (`{name}`) must appear in both columns. A trailing `…` must be kept. `i18n::fmt` fills placeholders in one pass, so a value such as a file name containing `{n}` is inserted as written.
- Catalogs are validated strictly: unknown escapes, unknown `@` contexts, placeholder or ellipsis mismatches, wrong plural form counts and duplicates are errors. The tests require the bundled catalogs to have none; at run time a bad line is skipped (and logged) and that string shows in English.
- Catalogs are assets: each `.tsv` has an `ATTRIBUTION.toml` entry with `kind = "translation"` and its SHA-256, checked by `cargo xtask assets`.
- Translations are clean-room: write them from the meaning of the English text, never from another product's string tables.

## Adding a language

1. Copy the header of `ja.tsv` into `xx.tsv` and translate entries.
2. Add one row to `LANGUAGES` in `crates/ui-egui/src/i18n/mod.rs`: code, native name, catalog and plural rule.
3. Add an `ATTRIBUTION.toml` entry for the catalog (`kind = "translation"`), then run `cargo xtask assets --write` and `cargo xtask assets`.

The Preferences dropdown, the system-language match and the catalog tests (format, duplicates, placeholders, plural forms, command ids) then pick it up.
