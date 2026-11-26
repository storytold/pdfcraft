use std::fmt::Write as _;

fn main() -> Result<(), String> {
    arabic_fonts()
}

/// Embed only Arabic fallback faces from the optional, separately licensed font bundle.
fn arabic_fonts() -> Result<(), String> {
    use std::path::PathBuf;
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_DIR");
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_REQUIRED");
    let required = std::env::var_os("CRAFT_FONTS_REQUIRED").is_some();
    let entries = match std::env::var_os("CRAFT_FONTS_DIR").map(PathBuf::from) {
        Some(dir) => arabic_font_entries(&dir),
        None if required => Err("CRAFT_FONTS_REQUIRED needs CRAFT_FONTS_DIR".into()),
        None => Ok(String::new()),
    };
    let entries = match entries {
        Ok(entries) => entries,
        Err(e) if required => return Err(e),
        Err(e) => {
            println!("cargo::warning=building without bundled Arabic fonts: {e}");
            String::new()
        }
    };
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default()).join("arabic_fonts.rs");
    std::fs::write(&out, format!("pub(crate) static ARABIC_FONTS: &[(&str, &[u8])] = &[\n{entries}];\n"))
        .map_err(|e| format!("writing {}: {e}", out.display()))
}

fn arabic_font_entries(dir: &std::path::Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use skrifa::MetadataProvider;
    let root = dir.canonicalize().map_err(|e| format!("craft-fonts directory: {e}"))?;
    let manifest = root.join("fonts/manifest.txt");
    println!("cargo::rerun-if-changed={}", manifest.display());
    let text = String::from_utf8(read_font_input(&manifest, 1 << 20)?).map_err(|e| e.to_string())?;
    let mut out = String::new();
    let mut has_regular = false;
    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split(" | ").map(str::trim).collect();
        let [family, style, file, scripts, _licence, licence_file, checksum, _source] = fields.as_slice() else {
            return Err(format!("malformed craft-fonts manifest line {}", line_number + 1));
        };
        if !scripts.split(',').any(|script| script.trim() == "Arab") {
            continue;
        }
        let path = root.join(file).canonicalize().map_err(|e| format!("{file}: {e}"))?;
        let licence = root.join(licence_file).canonicalize().map_err(|e| format!("{licence_file}: {e}"))?;
        if !path.starts_with(&root) || !licence.starts_with(&root) {
            return Err("craft-fonts paths must stay inside the checkout".into());
        }
        println!("cargo::rerun-if-changed={}", path.display());
        println!("cargo::rerun-if-changed={}", licence.display());
        let bytes = read_font_input(&path, 64 << 20)?;
        if Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>() != *checksum {
            return Err(format!("checksum mismatch for {file}"));
        }
        let font = skrifa::FontRef::from_index(&bytes, 0).map_err(|e| format!("invalid Arabic font {file}: {e}"))?;
        if !"العربية١٢٣".chars().all(|ch| font.charmap().map(ch).is_some()) {
            return Err(format!("{file} lacks Arabic glyphs"));
        }
        let _ = writeln!(out, "    ({:?}, include_bytes!({:?})),", format!("craft:{family}:{style}"), path);
        has_regular |= *style != "Bold";
    }
    if out.is_empty() {
        return Err("craft-fonts manifest contains no Arabic face; update the checkout".into());
    }
    if !has_regular {
        return Err("craft-fonts manifest has only bold Arabic faces; regular text needs a regular or variable face".into());
    }
    Ok(out)
}

fn read_font_input(path: &std::path::Path, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{} exceeds the font input size limit", path.display()));
    }
    Ok(bytes)
}
