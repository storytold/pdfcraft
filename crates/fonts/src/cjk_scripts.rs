//! The rule `build.rs` follows for CJK faces: which manifest script tags are CJK, which families
//! are embedded whole, and the size cap for the others. The build script includes this file with
//! `#[path]` and the crate's tests use it as a module, so the two can't drift.

/// The CJK families embedded whole: the Japanese faces documents and the renderer use.
pub const WHOLE_CJK_FAMILIES: [&str; 3] = ["Shippori Mincho", "BIZ UDPGothic", "BIZ UDMincho"];

/// Any other CJK face over this size is left out.
pub const MAX_OTHER_CJK_BYTES: u64 = 4 << 20;

/// ISO 15924 codes of the Han, Japanese and Korean scripts, Hangul on its own included.
const CJK_SCRIPTS: [&str; 6] = ["Hani", "Hans", "Hant", "Jpan", "Kore", "Hang"];

/// Whether a craft-fonts manifest script tag is CJK. A tag can carry a region after the code
/// (craft-fonts tags Noto Sans HK UI `Hant-HK`), so only the part before `-` is compared.
pub fn is_cjk_script(tag: &str) -> bool {
    let code = tag.trim().split('-').next().unwrap_or_default();
    CJK_SCRIPTS.contains(&code)
}
