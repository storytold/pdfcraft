# Interface language

Choose **Edit > Preferences > Interface language** and select **English** or **日本語**. The change applies immediately and persists between launches. Command ids, document contents and file names are unchanged.

The control channel exposes the setting through `ui.set`:

```json
{"method":"ui.set","params":{"key":"language","value":"ja"}}
```

`ui.state` reports `language` as `en` or `ja`. Unknown language codes return an error without changing the current setting. Old preferences default to English.

This first translation pass covers the main menu and core registered menu commands. Untranslated labels use English. Vertical Japanese PDF rendering is an existing viewer feature; this change does not add vertical text editing.
