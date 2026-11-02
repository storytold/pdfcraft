//! Explorer's MUIVerb resources use Windows' UI language, independently of install-time locale.
//! Labels share the UI catalogs; a neutral English table is the fallback for unsupported locales.

pub const LABELS: [&str; 3] = ["Convert to PDF with PdfCraft", "Convert to PDF and email…", "Combine files in PdfCraft…"];
const LOCALES: [(&str, u16); 34] = [
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

pub fn string_tables() -> Result<String, String> {
    let mut out = String::from("\n#pragma code_page(65001)\n");
    table(&mut out, 0, &LABELS);
    table(&mut out, 0x0409, &LABELS);
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/ui-egui/src/i18n");
    for (locale, id) in LOCALES {
        let path = directory.join(format!("{locale}.tsv"));
        let catalog = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut translated = Vec::new();
        for key in LABELS {
            let value = catalog
                .lines()
                .filter_map(|line| line.strip_prefix('\t'))
                .find_map(|line| {
                    let (source, value) = line.split_once('\t')?;
                    (source == key && !value.trim().is_empty()).then_some(value)
                })
                .ok_or_else(|| format!("{locale}: missing Explorer label {key}"))?;
            translated.push(value);
        }
        table(&mut out, id, &translated);
    }
    Ok(out)
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

    #[test]
    fn every_shipped_catalog_supplies_all_three_menu_labels() {
        let tables = string_tables().unwrap();
        let catalogs = std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/ui-egui/src/i18n"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "tsv"))
            .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(catalogs, LOCALES.iter().map(|(locale, _)| (*locale).to_owned()).collect(), "register every shipped UI language");
        assert_eq!(tables.matches("STRINGTABLE").count(), LOCALES.len() + 2);
        assert!(tables.contains("LANGUAGE 0, 0\n"));
        assert!(tables.contains("Mit PdfCraft in PDF umwandeln"));
        for id in 1001..=1003 {
            assert_eq!(tables.matches(&format!("  {id} L\"")).count(), LOCALES.len() + 2);
        }
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
