//! Embeds the fonts of the optional craft-fonts build input (`CRAFT_FONTS_DIR`) as `CRAFT_FONTS`.
//! The recipe is craft-fonts' `docs/integration.md`; unset, `CRAFT_FONTS` is empty. It only reads
//! the local checkout (no network). On wasm32 it embeds only the UI faces (BIZ UDPGothic Regular
//! and any `Arab`, `Telu`, `Hebr`, `Thai` or `Deva` face), to keep the web build within hosting
//! limits (Cloudflare Pages: 25 MiB per file). Native builds also read the optional
//! `fonts/simplified-chinese-manifest.txt` (LXGW WenKai Lite); it is never embedded on wasm.
//! `CRAFT_FONTS_REQUIRED` makes both the checkout and that native manifest required.

use std::fmt::Write as _;
use std::path::PathBuf;

#[path = "src/cjk_scripts.rs"]
mod cjk_scripts;

/// Build-script entry point: embed craft-fonts from `CRAFT_FONTS_DIR`, make a missing or invalid input fatal
/// when `CRAFT_FONTS_REQUIRED` is set, and write `craft_fonts.rs` to `OUT_DIR`.
fn main() {
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_DIR");
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_REQUIRED");
    let mut src = String::from("pub static CRAFT_FONTS: &[EmbeddedFace] = &[\n");
    let required = std::env::var_os("CRAFT_FONTS_REQUIRED").is_some();
    if let Some(dir) = std::env::var_os("CRAFT_FONTS_DIR").map(PathBuf::from) {
        match craft_fonts(&dir, required) {
            Ok(entries) => src.push_str(&entries),
            Err(e) if required => println!("cargo::error=CRAFT_FONTS_DIR={}: {e}", dir.display()),
            Err(e) => println!("cargo::warning=building without craft-fonts: CRAFT_FONTS_DIR={}: {e}", dir.display()),
        }
    } else if required {
        println!("cargo::error=CRAFT_FONTS_REQUIRED is set but CRAFT_FONTS_DIR is missing");
    }
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

/// One `EmbeddedFace { .. }` initialiser per manifest line, with the line's face index and role.
///
/// `strict_cap` is set for the required native Simplified Chinese manifest: a face that does not
/// fit its cap must fail the build rather than be silently left out, so a required build cannot
/// pass without the Chinese face it promises.
fn manifest(dir: &std::path::Path, manifest: &std::path::Path, wasm: bool, strict_cap: bool) -> Result<String, String> {
    println!("cargo::rerun-if-changed={}", manifest.display());
    let text = std::fs::read_to_string(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let entries = craft_fonts_build::parse_manifest(&text).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let mut out = String::new();
    // A required native Chinese manifest must add the Hans face itself, not merely some other
    // CJK face that happens to fit: otherwise CRAFT_FONTS_REQUIRED would pass while the promised
    // Simplified Chinese fallback is still absent.
    let mut has_hans = false;
    for entry in &entries {
        let (family, style, file) = (entry.family.as_str(), entry.style.as_str(), entry.file.as_str());
        let has = |script: &str| entry.scripts.iter().any(|s| s == script);
        // Keep the upstream general CJK size cap. The separately pinned native Chinese UI
        // face has its own bound; web builds retain the smaller general limit. A required
        // Chinese face over its cap fails the build instead of being silently omitted.
        let cjk = entry.scripts.iter().any(|s| cjk_scripts::is_cjk_script(s));
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
        // Arabic interface faces are small, so the web build keeps them too. Telugu faces too, for
        // the Telugu interface; the Hebrew ones (small), for Hebrew text and Edit text on the web;
        // and Devanagari, for the Hindi interface (one small face).
        let web_face = (family == "BIZ UDPGothic" && style == "Regular") || ["Arab", "Telu", "Hebr", "Thai", "Deva"].into_iter().any(has);
        if wasm && !web_face {
            continue;
        }
        has_hans |= cjk_scripts::has_script_tag(&entry.scripts, "Hans");
        let path = dir.join(file).canonicalize().map_err(|e| format!("{file}: {e}"))?;
        println!("cargo::rerun-if-changed={}", path.display());
        let scripts: Vec<String> = entry.scripts.iter().map(|s| format!("{s:?}")).collect();
        let _ = writeln!(
            out,
            "    EmbeddedFace {{ family: {family:?}, style: {style:?}, scripts: &[{}], index: {}, bytes: include_bytes!({:?}), role: {:?} }},",
            scripts.join(", "),
            entry.index,
            path.display().to_string(),
            entry.role.as_deref(),
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
        let line = |scripts: &str| format!("Fixture | Regular | face.ttf | {scripts} | OFL-1.1 | OFL.txt | 00 | fixture\n");
        for scripts in ["Hant,Latn", "Hans-HK,Latn", "Latn"] {
            std::fs::write(&path, line(scripts)).unwrap();
            assert!(manifest(&root, &path, false, true).unwrap_err().contains("no Hans face"), "{scripts}");
            assert!(manifest(&root, &path, false, false).unwrap().contains("EmbeddedFace {"), "optional input stays usable");
        }
        std::fs::write(&path, line("Latn, Hans")).unwrap();
        assert!(manifest(&root, &path, false, true).unwrap().contains("EmbeddedFace {"));
        std::fs::remove_file(root.join("face.ttf")).unwrap();
        assert!(manifest(&root, &path, false, true).is_err(), "a missing Hans asset cannot satisfy required input");
        std::fs::remove_dir_all(root).unwrap();
    }
}
