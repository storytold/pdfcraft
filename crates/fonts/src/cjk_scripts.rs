//! The rule `build.rs` follows for CJK faces: which manifest script tags are CJK, which families
//! are embedded whole, and the size cap for the others. The build script includes this file with
//! `#[path]` and the crate's tests use it as a module, so the two can't drift.

/// The CJK families embedded whole: the Japanese faces documents and the renderer use.
pub const WHOLE_CJK_FAMILIES: [&str; 3] = ["Shippori Mincho", "BIZ UDPGothic", "BIZ UDMincho"];

/// Any other CJK face over this size is left out.
pub const MAX_OTHER_CJK_BYTES: u64 = 4 << 20;

/// Reviewed native-only UI fallback from the separate Chinese manifest.
/// It is never included in the browser payload. Other CJK families keep the 4 MiB cap.
pub const NATIVE_UI_CJK_FAMILIES: [&str; 1] = ["LXGW WenKai Lite"];
pub const MAX_NATIVE_UI_CJK_BYTES: u64 = 16 << 20;

/// ISO 15924 codes of the Han, Japanese and Korean scripts, Hangul on its own included.
const CJK_SCRIPTS: [&str; 6] = ["Hani", "Hans", "Hant", "Jpan", "Kore", "Hang"];

/// Whether a craft-fonts manifest script tag is CJK. A tag can carry a region after the code
/// (craft-fonts tags Noto Sans HK UI `Hant-HK`), so only the part before `-` is compared.
pub fn is_cjk_script(tag: &str) -> bool {
    let code = tag.trim().split('-').next().unwrap_or_default();
    CJK_SCRIPTS.contains(&code)
}

/// Whether a manifest's comma-separated script list contains `script` exactly, ignoring spaces.
/// The interface and document lookups use exact tags (`CraftFont::covers`), so a regional tag
/// such as `Hans-HK` is not a `Hans` entry.
pub fn has_script_tag(scripts: &str, script: &str) -> bool {
    scripts.split(',').any(|tag| tag.trim() == script)
}

/// What `build.rs` does with one CJK face from a manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CjkSizeDecision {
    /// The face fits its cap (or is a Japanese family embedded whole).
    Embed,
    /// An optional face over its cap: leave it out and keep the rest.
    Skip,
    /// A required face over its cap: fail the build rather than silently omit it.
    Reject,
}

/// The size cap that applies to `family` in this build.
pub fn cjk_cap_bytes(family: &str, wasm: bool) -> u64 {
    if !wasm && NATIVE_UI_CJK_FAMILIES.contains(&family) { MAX_NATIVE_UI_CJK_BYTES } else { MAX_OTHER_CJK_BYTES }
}

/// Decide whether a CJK face is embedded, skipped, or rejected.
///
/// The Japanese document/UI families are embedded whole. Every other CJK family uses its cap;
/// `required` marks the native Simplified Chinese manifest of a `CRAFT_FONTS_REQUIRED` build,
/// where an over-cap face must fail instead of quietly disappearing.
pub fn cjk_size_decision(family: &str, bytes: u64, wasm: bool, required: bool) -> CjkSizeDecision {
    if WHOLE_CJK_FAMILIES.contains(&family) {
        return CjkSizeDecision::Embed;
    }
    if bytes <= cjk_cap_bytes(family, wasm) {
        CjkSizeDecision::Embed
    } else if required {
        CjkSizeDecision::Reject
    } else {
        CjkSizeDecision::Skip
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Check that a required Chinese manifest needs an exact `Hans` script tag and does not accept
    /// Traditional or region-suffixed tags.
    #[test]
    fn a_required_chinese_manifest_needs_an_exact_hans_entry() {
        assert!(has_script_tag("Hans,Latn", "Hans"));
        assert!(has_script_tag(" Latn , Hans ", "Hans"));
        assert!(!has_script_tag("Hant,Latn", "Hans"), "Traditional Chinese is not the required Simplified face");
        assert!(!has_script_tag("Hans-HK,Latn", "Hans"), "the UI looks for the exact Hans tag");
        assert!(!has_script_tag("Hani,Jpan,Kore", "Hans"));
    }

    /// Check that a native Chinese face over its cap is rejected when required, skipped when optional, and
    /// uses the web cap on the web.
    #[test]
    fn a_required_native_chinese_face_over_its_cap_is_rejected_not_skipped() {
        let at_cap = MAX_NATIVE_UI_CJK_BYTES;
        assert_eq!(cjk_size_decision("LXGW WenKai Lite", at_cap, false, true), CjkSizeDecision::Embed);
        assert_eq!(cjk_size_decision("LXGW WenKai Lite", at_cap + 1, false, true), CjkSizeDecision::Reject);
        assert_eq!(cjk_size_decision("LXGW WenKai Lite", at_cap + 1, false, false), CjkSizeDecision::Skip);
        // The web build keeps the smaller general cap even for the native Chinese family.
        assert_eq!(cjk_size_decision("LXGW WenKai Lite", MAX_OTHER_CJK_BYTES + 1, true, true), CjkSizeDecision::Reject);
        // Japanese document/UI families are embedded whole.
        assert_eq!(cjk_size_decision("BIZ UDPGothic", u64::MAX, false, true), CjkSizeDecision::Embed);
        assert_eq!(cjk_cap_bytes("LXGW WenKai Lite", false), MAX_NATIVE_UI_CJK_BYTES);
        assert_eq!(cjk_cap_bytes("LXGW WenKai Lite", true), MAX_OTHER_CJK_BYTES);
    }
}
