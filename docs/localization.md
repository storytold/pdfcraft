# Interface language

Choose **Edit > Preferences > Interface language** and select **English** or **日本語**. The change applies immediately and persists between launches. Command ids, document contents and file names are unchanged.

The control channel exposes the setting through `ui.set`:

```json
{"method":"ui.set","params":{"key":"language","value":"ja"}}
```

`ui.state` reports `language` as `en` or `ja`. Unknown language codes return an error without changing the current setting. Old preferences default to English.

This first translation pass covers the main menu and core registered menu commands. Untranslated labels use English. Vertical Japanese PDF rendering is an existing viewer feature; this change does not add vertical text editing.

Japanese interface text uses BIZ UDPGothic from [craft-fonts](https://github.com/storytold/craft-fonts), an optional build input that every release includes (`CRAFT_FONTS_DIR`; see the README). A build made without it has no Japanese face, so Japanese labels show replacement boxes.

## Localization foundation

The registry in `crates/ui-egui/src/i18n/mod.rs` owns the supported codes, native display names,
catalogs and plural rules. The language dropdown, settings serialization and control-channel
validation all use that registry. The first version keeps English and the existing 49 Japanese
translations; it adds no language, translation, font or dependency. English remains the default,
and settings/control codes remain exactly `en` and `ja` (no automatic locale detection).

Each non-English language has a compiled-in UTF-8 TSV catalog. Its three columns are
`context<TAB>source<TAB>translation`; blank lines and lines starting with `#` are ignored.
Columns support `\n`, `\t` and `\\` escapes. Preserve named placeholders and trailing `…`.

| Context | Source | Lookup |
| --- | --- | --- |
| Empty | English text | `language.tr(text)` |
| A descriptive context | English text | `language.tr_ctx(context, text)` |
| `@id` | Stable engine command id | `language.tr_id(id, english_label)` |
| `@plural` | English `one\|other` templates | `language.trn(count, one, other)` |

Context and command-id lookups fall back to the plain translation, then the English text.
Registered static menu commands use the id lookup. Dynamic history labels still follow the
existing English-label path; converting them and the remaining dialogs/panels is separate work.

Use templates rather than translating the result of `format!`:

```rust,ignore
let text = i18n::fmt(language.tr_ctx("status", "Opening {file}…"), &[("file", filename)]);
let pages = language.trn(count, "{n} page", "{n} pages");
```

`fmt` permits reordered/repeated parameters, leaves unknown parameters visible, and substitutes
only once: braces in inserted filenames or other user data remain literal. Plural catalogs list
forms separated by `|`; English uses one/other, while Japanese uses a single form. Missing plural
entries fall back to the English singular/plural and still fill `{n}`. Both English forms must
have the same placeholders.

## Validation and adding languages

Run `cargo test -p printcraft-ui-egui --lib i18n` to validate every registered catalog. Tests cover
duplicate and malformed entries, escapes, matching placeholders and ellipses, plural forms,
command-id validity, settings compatibility and fallback. A catalog error is logged and uses
English at runtime instead of crashing. The UI control tests also check switching languages,
menu labels and unchanged command ids/document state.

To add a language in a later change:

1. Add its catalog and one registry entry with the code, native name and plural rule. Extend the
   plural-rule enum when the language needs a rule that is not already implemented.
2. Follow `AGENTS.md` to attribute the catalog in `ATTRIBUTION.toml` and regenerate
   `ATTRIBUTION.md` with `cargo xtask assets --write`. `cargo xtask assets` checks TSV files too.
3. Validate the catalog and test the language through `ui.set`/`ui.state`. Verify glyph coverage
   and layout; new fonts belong in craft-fonts, never this repository.

Catalog validity does not measure translation coverage. Full UI extraction/coverage gates,
broader translation, locale matching, RTL and locale-aware number/date formatting remain open.
