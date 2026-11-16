//! Embeds the fonts of the optional craft-fonts build input (`CRAFT_FONTS_DIR`) as `CRAFT_FONTS`.
//! The recipe is craft-fonts' `docs/integration.md`; unset, `CRAFT_FONTS` is empty. It only reads
//! the local checkout (no network). On wasm32 it embeds only the UI faces (BIZ UDPGothic Regular
//! and any `Arab`, `Telu`, `Hebr`, `Thai` or `Deva` face), to keep the web build within hosting
//! limits (Cloudflare Pages: 25 MiB per file). Native builds also read the optional
//! `fonts/simplified-chinese-manifest.txt` (LXGW WenKai Lite); it is never embedded on wasm.
//! `CRAFT_FONTS_REQUIRED` makes both the checkout and that native manifest required.
//! Native builds also read the opt-in Thai document manifest `fonts/thai-manifest.txt` (Sarabun)
//! into a separate `CRAFT_DOCUMENT_FONTS`: content faces, never interface fallbacks. It stays
//! optional even when required, since craft-fonts revisions before it have none.

use std::fmt::Write as _;
use std::path::PathBuf;

#[path = "src/cjk_scripts.rs"]
mod cjk_scripts;

/// Build-script entry point: embed craft-fonts from `CRAFT_FONTS_DIR`, make a missing or invalid input fatal
/// when `CRAFT_FONTS_REQUIRED` is set, and write `craft_fonts.rs` to `OUT_DIR`.
fn main() {
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_DIR");
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_REQUIRED");
    let mut src = String::from("pub static CRAFT_FONTS: &[CraftFont] = &[\n");
    let mut documents = String::new();
    let required = std::env::var_os("CRAFT_FONTS_REQUIRED").is_some();
    if let Some(dir) = std::env::var_os("CRAFT_FONTS_DIR").map(PathBuf::from) {
        match craft_fonts(&dir, required) {
            Ok(entries) => {
                src.push_str(&entries);
                let wasm = std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|a| a == "wasm32");
                match document_fonts(&dir, required, wasm) {
                    Ok(entries) => documents = entries,
                    Err(e) => println!("cargo::error=CRAFT_FONTS_DIR={}: {e}", dir.display()),
                }
            }
            Err(e) if required => println!("cargo::error=CRAFT_FONTS_DIR={}: {e}", dir.display()),
            Err(e) => println!("cargo::warning=building without craft-fonts: CRAFT_FONTS_DIR={}: {e}", dir.display()),
        }
    } else if required {
        println!("cargo::error=CRAFT_FONTS_REQUIRED is set but CRAFT_FONTS_DIR is missing");
    }
    src.push_str("];\n/// Faces of craft-fonts' opt-in document manifests, for text written into PDFs only.\n");
    src.push_str("pub static CRAFT_DOCUMENT_FONTS: &[CraftFont] = &[\n");
    src.push_str(&documents);
    src.push_str("];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default()).join("craft_fonts.rs");
    if let Err(e) = std::fs::write(&out, src) {
        println!("cargo::error=writing {}: {e}", out.display());
    }
}

/// The main manifest, plus the native-only Simplified Chinese manifest. The latter is optional
/// for ordinary builds and required when `CRAFT_FONTS_REQUIRED` is set, so a required build can
/// never pass the Chinese UI tests only by skipping them.
fn craft_fonts(dir: &std::path::Path, required: bool) -> Result<String, String> {
    let wasm = std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|a| a == "wasm32");
    let mut out = manifest(dir, &dir.join("fonts/manifest.txt"), wasm, false)?;
    if !wasm {
        let chinese = dir.join("fonts/simplified-chinese-manifest.txt");
        println!("cargo::rerun-if-changed={}", chinese.display());
        match chinese.try_exists() {
            Ok(false) if required => {
                return Err(format!("{}: CRAFT_FONTS_REQUIRED is set but the simplified-Chinese font manifest is missing", chinese.display()));
            }
            Ok(false) => {}
            Ok(true) => match manifest(dir, &chinese, false, required) {
                Ok(entries) => out.push_str(&entries),
                // A broken optional addition must never discard the main Japanese/symbol/RTL faces.
                Err(e) if required => return Err(e),
                Err(e) => println!("cargo::warning=skipping optional Chinese fonts: {e}"),
            },
            Err(e) if required => return Err(format!("{}: {e}", chinese.display())),
            Err(e) => println!("cargo::warning=skipping optional Chinese fonts: {}: {e}", chinese.display()),
        }
    }
    Ok(out)
}

/// The faces of the opt-in Thai document manifest (`fonts/thai-manifest.txt`, Sarabun): for text
/// written into PDFs only, never interface fallbacks, so they go into `CRAFT_DOCUMENT_FONTS`, not
/// `CRAFT_FONTS`. Native only (the web build stays within its size limits). A missing manifest is
/// normal even when `required` (craft-fonts revisions before it have none); a broken one fails a
/// required build and is skipped with a warning otherwise.
fn document_fonts(dir: &std::path::Path, required: bool, wasm: bool) -> Result<String, String> {
    if wasm {
        return Ok(String::new());
    }
    let thai = dir.join("fonts/thai-manifest.txt");
    println!("cargo::rerun-if-changed={}", thai.display());
    match thai.try_exists() {
        Ok(false) => Ok(String::new()),
        Ok(true) => match manifest(dir, &thai, false, false) {
            Ok(entries) => Ok(entries),
            Err(e) if required => Err(e),
            Err(e) => {
                println!("cargo::warning=skipping optional Thai document fonts: {e}");
                Ok(String::new())
            }
        },
        Err(e) if required => Err(format!("{}: {e}", thai.display())),
        Err(e) => {
            println!("cargo::warning=skipping optional Thai document fonts: {}: {e}", thai.display());
            Ok(String::new())
        }
    }
}

/// One `CraftFont { .. }` initialiser per manifest line.
///
/// `strict_cap` is set for the required native Simplified Chinese manifest: a face that does not
/// fit its cap must fail the build rather than be silently left out, so a required build cannot
/// pass without the Chinese face it promises.
fn manifest(dir: &std::path::Path, manifest: &std::path::Path, wasm: bool, strict_cap: bool) -> Result<String, String> {
    println!("cargo::rerun-if-changed={}", manifest.display());
    let text = std::fs::read_to_string(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let mut out = String::new();
    // A required native Chinese manifest must add the Hans face itself, not merely some other
    // CJK face that happens to fit: otherwise CRAFT_FONTS_REQUIRED would pass while the promised
    // Simplified Chinese fallback is still absent.
    let mut has_hans = false;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = line.split(" | ").map(str::trim).collect();
        let [family, style, file, scripts, ..] = f.as_slice() else {
            return Err(format!("malformed manifest line: {line}"));
        };
        // Keep the upstream general CJK size cap. The separately pinned native Chinese UI
        // face has its own bound; web builds retain the smaller general limit. A required
        // Chinese face over its cap fails the build instead of being silently omitted.
        let cjk = scripts.split(',').any(cjk_scripts::is_cjk_script);
        if cjk && let Ok(metadata) = std::fs::metadata(dir.join(file)) {
            match cjk_scripts::cjk_size_decision(family, metadata.len(), wasm, strict_cap) {
                cjk_scripts::CjkSizeDecision::Embed => {}
                cjk_scripts::CjkSizeDecision::Skip => continue,
                cjk_scripts::CjkSizeDecision::Reject => {
                    let cap = cjk_scripts::cjk_cap_bytes(family, wasm);
                    return Err(format!(
                        "{file}: the {family} face is larger than the {cap}-byte native Chinese UI cap; CRAFT_FONTS_REQUIRED cannot embed it"
                    ));
                }
            }
        }
        // Arabic interface faces are small, so the web build keeps them too.
        let arabic = scripts.split(',').any(|s| s.trim() == "Arab");
        // Telugu faces too, for the Telugu interface.
        let telugu = scripts.split(',').any(|s| s.trim() == "Telu");
        // And the Hebrew ones (small), for Hebrew text and Edit text on the web.
        let hebrew = scripts.split(',').any(|s| s.trim() == "Hebr");
        // Devanagari too, for the Hindi interface (one small face).
        let devanagari = scripts.split(',').any(|s| s.trim() == "Deva");
        let web_face = (*family == "BIZ UDPGothic" && *style == "Regular")
            || arabic
            || telugu
            || hebrew
            || scripts.split(',').any(|s| s.trim() == "Thai")
            || devanagari;
        if wasm && !web_face {
            continue;
        }
        has_hans |= cjk_scripts::has_script_tag(scripts, "Hans");
        let path = dir.join(file).canonicalize().map_err(|e| format!("{file}: {e}"))?;
        println!("cargo::rerun-if-changed={}", path.display());
        let scripts: Vec<String> = scripts.split(',').map(|s| format!("{:?}", s.trim())).collect();
        let _ = writeln!(
            out,
            "    CraftFont {{ family: {family:?}, style: {style:?}, scripts: &[{}], bytes: include_bytes!({:?}) }},",
            scripts.join(", "),
            path.display().to_string(),
        );
    }
    if strict_cap && !has_hans {
        return Err(format!(
            "{}: CRAFT_FONTS_REQUIRED is set but the manifest emits no Hans face (a Simplified Chinese face is required)",
            manifest.display()
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod manifest_tests {
    use super::*;

    /// Regression test: a required native-font manifest must declare at least one Hans face, while optional
    /// input accepts another script and a missing asset still fails.
    #[test]
    fn required_native_manifest_rejects_non_hans_faces_after_parsing() {
        let root = std::env::temp_dir().join(format!("pdfcraft-required-hans-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("face.ttf"), b"synthetic manifest fixture").unwrap();
        let path = root.join("manifest.txt");
        for scripts in ["Hant,Latn", "Hans-HK,Latn", "Latn"] {
            std::fs::write(&path, format!("Fixture | Regular | face.ttf | {scripts}\n")).unwrap();
            assert!(manifest(&root, &path, false, true).unwrap_err().contains("no Hans face"), "{scripts}");
            assert!(manifest(&root, &path, false, false).unwrap().contains("CraftFont {"), "optional input stays usable");
        }
        std::fs::write(&path, "Fixture | Regular | face.ttf | Latn, Hans\n").unwrap();
        assert!(manifest(&root, &path, false, true).unwrap().contains("CraftFont {"));
        std::fs::remove_file(root.join("face.ttf")).unwrap();
        assert!(manifest(&root, &path, false, true).is_err(), "a missing Hans asset cannot satisfy required input");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The Thai document manifest is optional even for a required build (the pinned craft-fonts
    /// predates it), native-only, and never breaks the build unless it is present and broken
    /// while `CRAFT_FONTS_REQUIRED` is set.
    #[test]
    fn document_manifest_is_optional_and_native_only() {
        let root = std::env::temp_dir().join(format!("pdfcraft-document-fonts-{}", std::process::id()));
        std::fs::create_dir_all(root.join("fonts")).unwrap();
        for required in [false, true] {
            assert_eq!(document_fonts(&root, required, false), Ok(String::new()), "missing manifest, required={required}");
        }
        std::fs::write(root.join("face.ttf"), b"synthetic manifest fixture").unwrap();
        let path = root.join("fonts/thai-manifest.txt");
        std::fs::write(&path, "# comment\nFixture | Regular | face.ttf | Thai,Latn\n").unwrap();
        for required in [false, true] {
            assert!(document_fonts(&root, required, false).unwrap().contains("family: \"Fixture\""), "required={required}");
            assert_eq!(document_fonts(&root, required, true), Ok(String::new()), "never on wasm, required={required}");
        }
        std::fs::write(&path, "not a manifest line\n").unwrap();
        assert_eq!(document_fonts(&root, false, false), Ok(String::new()), "a broken optional manifest is skipped");
        assert!(document_fonts(&root, true, false).is_err(), "a broken manifest fails a required build");
        std::fs::remove_dir_all(root).unwrap();
    }
}
