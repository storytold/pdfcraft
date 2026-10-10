//! Embeds the fonts of the optional craft-fonts build input (`CRAFT_FONTS_DIR`) as `CRAFT_FONTS`.
//! The recipe is craft-fonts' `docs/integration.md`; unset, `CRAFT_FONTS` is empty. It only reads
//! the local checkout (no network). Web builds retain the UI faces, including Chinese.

use std::fmt::Write as _;
use std::path::PathBuf;

#[path = "src/select.rs"]
mod select;

fn main() {
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_DIR");
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_REQUIRED");
    let mut src = String::from("pub static CRAFT_FONTS: &[CraftFont] = &[\n");
    if let Some(dir) = std::env::var_os("CRAFT_FONTS_DIR").map(PathBuf::from) {
        match craft_fonts(&dir) {
            Ok(entries) => src.push_str(&entries),
            Err(e) if std::env::var_os("CRAFT_FONTS_REQUIRED").is_some() => {
                println!("cargo::error=CRAFT_FONTS_DIR={}: {e}", dir.display());
            }
            Err(e) => println!("cargo::warning=building without craft-fonts: CRAFT_FONTS_DIR={}: {e}", dir.display()),
        }
    }
    src.push_str("];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default()).join("craft_fonts.rs");
    if let Err(e) = std::fs::write(&out, src) {
        println!("cargo::error=writing {}: {e}", out.display());
    }
}

/// One `CraftFont { .. }` initialiser per manifest line.
fn craft_fonts(dir: &std::path::Path) -> Result<String, String> {
    let manifest = dir.join("fonts/manifest.txt");
    println!("cargo::rerun-if-changed={}", manifest.display());
    let text = std::fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let wasm = std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|a| a == "wasm32");
    let mut out = String::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = line.split(" | ").map(str::trim).collect();
        let [family, style, file, scripts, ..] = f.as_slice() else {
            return Err(format!("malformed manifest line: {line}"));
        };
        // AGENTS.md §1.1 bars Adobe's type designs whatever their licence, so a checkout that
        // carries one (craft-fonts has Noto Sans CJK, which is Source Han) never embeds it.
        if !select::allowed_family(family) {
            println!("cargo::warning=craft-fonts: not embedding {family} {style} (AGENTS.md §1.1)");
            continue;
        }
        let scripts: Vec<&str> = scripts.split(',').map(str::trim).collect();
        if wasm && !select::on_web(family, style, &scripts) {
            continue;
        }
        let path = dir.join(file).canonicalize().map_err(|e| format!("{file}: {e}"))?;
        println!("cargo::rerun-if-changed={}", path.display());
        let _ = writeln!(
            out,
            "    CraftFont {{ family: {family:?}, style: {style:?}, scripts: &[{}], bytes: include_bytes!({:?}) }},",
            scripts.iter().map(|s| format!("{s:?}")).collect::<Vec<_>>().join(", "),
            path.display().to_string(),
        );
    }
    Ok(out)
}
