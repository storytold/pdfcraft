//! Explorer's MUIVerb resources use Windows' UI language, independently of install-time locale.
//! Labels come from the craft-i18n checkout the interface embeds (`CRAFT_I18N_DIR`,
//! `locales/<code>/pdfcraft.tsv`); a neutral English table is the fallback for a locale without
//! all three labels, and for a build without the checkout.

pub const LABELS: [&str; 3] = ["Convert to PDF with PdfCraft", "Convert to PDF and email…", "Combine files in PdfCraft…"];
/// Every UI language with a menu table when its catalog has all three labels: code, LANGID.
pub const LOCALES: [(&str, u16); 34] = [
    ("ar", 0x0401),
    ("bg", 0x0402),
    ("ca", 0x0403),
    ("ckb", 0x0492),
    ("cs", 0x0405),
    ("de", 0x0407),
    ("el", 0x0408),
    ("es", 0x0c0a),
    ("fa", 0x0429),
    ("fi", 0x040b),
    ("fr", 0x040c),
    ("hi", 0x0439),
    ("hu", 0x040e),
    ("id", 0x0421),
    ("it", 0x0410),
    ("ja", 0x0411),
    ("kk", 0x043f),
    ("ko", 0x0412),
    ("lo", 0x0454),
    ("lt", 0x0427),
    ("nb", 0x0414),
    ("pl", 0x0415),
    ("pt-br", 0x0416),
    ("ro", 0x0418),
    ("ru", 0x0419),
    ("sl", 0x0424),
    ("sq", 0x041c),
    ("te", 0x044a),
    ("th", 0x041e),
    ("tr", 0x041f),
    ("uk", 0x0422),
    ("vi", 0x042a),
    ("zh-hans", 0x0804),
    ("zh-hant", 0x0404),
];

/// The `STRINGTABLE`s for `craft_i18n`, a craft-i18n checkout. `None` gives English only; a
/// directory without `locales/` is an error, while a locale whose catalog lacks a label keeps
/// Windows' English fallback.
pub fn string_tables(craft_i18n: Option<&std::path::Path>) -> Result<String, String> {
    let mut out = String::from("\n#pragma code_page(65001)\n");
    table(&mut out, 0, &LABELS);
    table(&mut out, 0x0409, &LABELS);
    let Some(directory) = craft_i18n else { return Ok(out) };
    let locales = directory.join("locales");
    if !locales.is_dir() {
        return Err(format!("{} is not a craft-i18n checkout", directory.display()));
    }
    for (locale, id) in LOCALES {
        let Ok(catalog) = std::fs::read_to_string(catalog(directory, locale)) else { continue };
        let translated: Option<Vec<&str>> = LABELS
            .iter()
            .map(|key| {
                catalog.lines().filter_map(|line| line.strip_prefix('\t')).find_map(|line| {
                    let (source, value) = line.split_once('\t')?;
                    (source == *key && !value.trim().is_empty()).then_some(value)
                })
            })
            .collect();
        if let Some(translated) = translated {
            table(&mut out, id, &translated);
        }
    }
    Ok(out)
}

/// PdfCraft's catalog for `locale` in a craft-i18n checkout.
pub fn catalog(craft_i18n: &std::path::Path, locale: &str) -> std::path::PathBuf {
    craft_i18n.join("locales").join(locale).join("pdfcraft.tsv")
}

fn table(out: &mut String, language: u16, labels: &[&str]) {
    out.push_str(&format!("LANGUAGE {}, {}\nSTRINGTABLE\nBEGIN\n", language & 0x3ff, language >> 10));
    for (i, label) in labels.iter().enumerate() {
        // Catalog text is resource data, never interpreted as preprocessor directives.
        let label = label.replace('\\', "\\\\").replace('"', "\"\"");
        out.push_str(&format!("  {} L\"{label}\"\n", 1001 + i));
    }
    out.push_str("END\n");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkout(name: &str, catalogs: &[(&str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("pdfcraft-shell-labels-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (locale, text) in catalogs {
            std::fs::create_dir_all(root.join("locales").join(locale)).unwrap();
            std::fs::write(catalog(&root, locale), text).unwrap();
        }
        std::fs::create_dir_all(root.join("locales")).unwrap();
        root
    }

    #[test]
    fn complete_catalogs_get_a_table_and_partial_ones_keep_english() {
        let german = "# comment\n\tConvert to PDF with PdfCraft\tMit PdfCraft in PDF umwandeln\n\tConvert to PDF and email…\tIn PDF umwandeln und per E-Mail senden…\n\tCombine files in PdfCraft…\tDateien in PdfCraft kombinieren…\n";
        let partial = "\tConvert to PDF with PdfCraft\tConvertir en PDF avec PdfCraft\n\tCombine files in PdfCraft…\t \n";
        let root = checkout("partial", &[("de", german), ("fr", partial)]);
        let tables = string_tables(Some(&root)).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(tables.matches("STRINGTABLE").count(), 3, "neutral, en-US and German");
        assert!(tables.contains("LANGUAGE 7, 1\n"), "German (0x0407)");
        assert!(tables.contains("Mit PdfCraft in PDF umwandeln"));
        assert!(!tables.contains("Convertir"), "a catalog missing a label keeps Windows' English fallback");
        for id in 1001..=1003 {
            assert_eq!(tables.matches(&format!("  {id} L\"")).count(), 3);
        }
    }

    #[test]
    fn without_a_checkout_the_menu_is_english_and_a_wrong_directory_is_an_error() {
        let tables = string_tables(None).unwrap();
        assert_eq!(tables.matches("STRINGTABLE").count(), 2);
        assert!(tables.contains("LANGUAGE 0, 0\n") && tables.contains("Convert to PDF with PdfCraft"));
        let missing = std::env::temp_dir().join(format!("pdfcraft-shell-labels-none-{}", std::process::id()));
        assert!(string_tables(Some(&missing)).is_err());
    }

    /// With the pinned checkout (CI sets `CRAFT_I18N_DIR`), every registered language has all
    /// three labels.
    #[test]
    fn the_pinned_catalogs_supply_every_registered_menu_language() {
        let Some(root) = std::env::var_os("CRAFT_I18N_DIR").filter(|d| !d.is_empty()).map(std::path::PathBuf::from) else { return };
        let tables = string_tables(Some(&root)).unwrap();
        assert_eq!(tables.matches("STRINGTABLE").count(), LOCALES.len() + 2, "a registered language lacks an Explorer label");
    }

    #[cfg(all(windows, pdfcraft_shell_resources))]
    #[test]
    fn windows_can_load_the_embedded_menu_strings() {
        let executable = winsafe::HINSTANCE::GetModuleHandle(None).unwrap();
        for id in 1001..=1003 {
            assert!(!executable.LoadString(id).unwrap().trim().is_empty());
        }
    }
}
