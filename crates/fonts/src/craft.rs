//! Fonts from the optional craft-fonts build input (<https://github.com/storytold/craft-fonts>).
//!
//! `build.rs` embeds every font in craft-fonts' manifest when PdfCraft is built with
//! `CRAFT_FONTS_DIR=<checkout>`; otherwise [`CRAFT_FONTS`] is empty and everything here returns
//! nothing. Callers must work either way.

/// A font from the optional craft-fonts build input, with the manifest line's face index and role
/// (`ui`, `mono`, `signature`), which the interface font stack reads.
pub use craft_fonts_build::EmbeddedFace;

include!(concat!(env!("OUT_DIR"), "/craft_fonts.rs"));

/// The `Jpan` craft-fonts faces in the order the interface prefers them: BIZ UDPGothic (a UI
/// face) first, then the rest in manifest order. Empty without craft-fonts.
pub fn ui_japanese_fonts() -> Vec<&'static EmbeddedFace> {
    let mut fonts: Vec<&EmbeddedFace> = CRAFT_FONTS.iter().filter(|f| f.has_script("Jpan")).collect();
    // Stable: manifest order within each group.
    fonts.sort_by_key(|f| f.family != "BIZ UDPGothic");
    fonts
}

/// The `Hans` craft-fonts faces for Simplified Chinese interface text, in manifest order.
/// Empty when built without craft-fonts (Chinese text then shows the font system's
/// replacement glyph, the same degraded mode as Japanese without craft-fonts).
pub fn ui_chinese_fonts() -> Vec<&'static EmbeddedFace> {
    CRAFT_FONTS.iter().filter(|f| f.has_script("Hans")).collect()
}

/// The `Arab` craft-fonts faces for Arabic-script interface text (file names, document titles),
/// in manifest order. Empty when built without craft-fonts or when it has no Arabic face.
pub fn ui_arabic_fonts() -> Vec<&'static EmbeddedFace> {
    arabic(CRAFT_FONTS.iter())
}

/// The face for Arabic text written into PDFs: the first `Arab` face. `None` when built without
/// craft-fonts or when its revision has no Arabic face.
pub fn document_arabic_font() -> Option<&'static EmbeddedFace> {
    ui_arabic_fonts().into_iter().next()
}

fn arabic<'a>(faces: impl IntoIterator<Item = &'a EmbeddedFace>) -> Vec<&'a EmbeddedFace> {
    faces.into_iter().filter(|f| f.has_script("Arab")).collect()
}

/// The `Telu` craft-fonts faces for Telugu interface text (the Telugu catalog, file names,
/// document titles), in manifest order. Empty when built without craft-fonts or when it has no
/// Telugu face.
pub fn ui_telugu_fonts() -> Vec<&'static EmbeddedFace> {
    CRAFT_FONTS.iter().filter(|f| f.has_script("Telu")).collect()
}

/// Thai faces shared by the interface and PDF text writer.
pub fn ui_thai_fonts() -> Vec<&'static EmbeddedFace> {
    CRAFT_FONTS.iter().filter(|f| f.has_script("Thai")).collect()
}

pub fn document_thai_font() -> Option<&'static EmbeddedFace> {
    ui_thai_fonts().into_iter().next()
}

/// The `Deva` craft-fonts faces for Devanagari interface text (the Hindi catalog, file names,
/// document titles), in manifest order. Empty when built without craft-fonts or when its revision
/// has no Devanagari face; a desktop build then falls back to one font already installed on the
/// machine (`SYSTEM_DEVANAGARI_FALLBACK`), and the web build shows replacement boxes.
pub fn ui_devanagari_fonts() -> Vec<&'static EmbeddedFace> {
    CRAFT_FONTS.iter().filter(|f| f.has_script("Deva")).collect()
}

/// The face for Devanagari text written into PDFs: the first `Deva` face. `None` when built
/// without craft-fonts or when its revision has no Devanagari face.
pub fn document_devanagari_font() -> Option<&'static EmbeddedFace> {
    ui_devanagari_fonts().into_iter().next()
}

/// The `Hebr` craft-fonts faces for Hebrew interface text (file names, document titles, text
/// being edited), in manifest order. Empty when built without craft-fonts or when it has no
/// Hebrew face.
pub fn ui_hebrew_fonts() -> Vec<&'static EmbeddedFace> {
    CRAFT_FONTS.iter().filter(|f| f.has_script("Hebr")).collect()
}

/// The face for Hebrew text written into PDFs: the `Hebr` face of the asked weight, else its
/// Regular, else any `Hebr` face. `None` without craft-fonts or without a Hebrew face.
pub fn document_hebrew_font(bold: bool) -> Option<&'static EmbeddedFace> {
    hebrew_face(CRAFT_FONTS, bold)
}

fn hebrew_face(faces: &[EmbeddedFace], bold: bool) -> Option<&EmbeddedFace> {
    let hebr = || faces.iter().filter(|f| f.has_script("Hebr"));
    let style = if bold { "Bold" } else { "Regular" };
    hebr().find(|f| f.style == style).or_else(|| hebr().find(|f| f.style == "Regular")).or_else(|| hebr().next())
}

/// Interface CJK faces in fallback order for the UI language: Simplified Chinese first when
/// `prefer_hans`, otherwise Japanese first (the historical default).
///
/// The order matters beyond glyph shapes. egui renders each character with the FIRST face that
/// has its glyph, so with Japanese first a Simplified-only character (e.g. U+6B22 欢, absent
/// from Japanese faces) lands in a different face than its neighbours; the mixed vertical
/// metrics then sink it below the line. Preferring the UI language's face keeps one line in
/// one face with one baseline.
pub fn ui_cjk_fonts(prefer_hans: bool) -> Vec<&'static EmbeddedFace> {
    order_cjk(CRAFT_FONTS.iter(), prefer_hans)
}

fn order_cjk<'a>(faces: impl IntoIterator<Item = &'a EmbeddedFace>, prefer_hans: bool) -> Vec<&'a EmbeddedFace> {
    let mut out: Vec<&'a EmbeddedFace> = faces.into_iter().filter(|f| f.has_script("Hans") || f.has_script("Jpan")).collect();
    let first = if prefer_hans { "Hans" } else { "Jpan" };
    // The preferred script's faces first (a face covering both counts as preferred); within each
    // group the Japanese UI face (BIZ UDPGothic, not a serif) leads, otherwise manifest order
    // (the sort is stable).
    out.sort_by_key(|f| (!f.has_script(first), f.family != "BIZ UDPGothic"));
    out
}

/// The face for Japanese text written into PDFs (serif document text): Shippori Mincho, then
/// BIZ UDMincho, then any other regular `Jpan` face. `None` without craft-fonts.
pub fn document_japanese_font() -> Option<&'static EmbeddedFace> {
    document_face(CRAFT_FONTS, true, false)
}

/// A real Japanese document face matching serif/sans and weight where available.
/// Sans text prefers BIZ UDPGothic Bold or Regular. Missing weights fall back to Regular;
/// serif text keeps the document Mincho preference. No synthetic weight or slant is applied.
/// `None` without craft-fonts. The small web input currently has only Gothic Regular.
pub fn document_japanese_font_for_style(serif: bool, bold: bool) -> Option<&'static EmbeddedFace> {
    document_face(CRAFT_FONTS, serif, bold)
}

/// Every `Jpan` face that can stand in for the document text, the best match for the style first
/// (the face [`document_japanese_font_for_style`] returns), then the others. Not every face has
/// every glyph (their coverage of e.g. Cyrillic differs), so a caller can move on to the next one.
/// Empty without craft-fonts.
pub fn document_japanese_fonts_for_style(serif: bool, bold: bool) -> Vec<&'static EmbeddedFace> {
    document_faces(CRAFT_FONTS, serif, bold)
}

/// The scripts the bundled Inter faces are for.
static LATIN_SCRIPTS: &[&str] = &["Latn", "Cyrl", "Grek"];

/// The bundled Latin document faces (`assets/fonts`, OFL): every build has them, unlike
/// craft-fonts. The Japanese faces carry only part of the Cyrillic (`привет` yes, Uzbek `қизил`
/// and Kazakh `әже` no), so letters like қ, ғ, ҳ, ә, ө, ү, ұ and һ have no face at all without
/// these.
static INTER_REGULAR_FACE: EmbeddedFace = EmbeddedFace {
    family: "Inter",
    style: "Regular",
    scripts: LATIN_SCRIPTS,
    index: 0,
    bytes: include_bytes!("../../../assets/fonts/Inter-Regular.ttf"),
    role: None,
};
static INTER_SEMIBOLD_FACE: EmbeddedFace = EmbeddedFace {
    family: "Inter",
    style: "SemiBold",
    scripts: LATIN_SCRIPTS,
    index: 0,
    bytes: include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"),
    role: None,
};

/// The face for Latin, Greek and Cyrillic text written into PDFs: the bundled Inter Regular, or
/// SemiBold for bold text. Present in every build, crafted ones included.
pub fn document_latin_fonts(bold: bool) -> Vec<&'static EmbeddedFace> {
    vec![if bold { &INTER_SEMIBOLD_FACE } else { &INTER_REGULAR_FACE }]
}

fn document_face(faces: &[EmbeddedFace], serif: bool, bold: bool) -> Option<&EmbeddedFace> {
    document_faces(faces, serif, bold).into_iter().next()
}

fn document_faces(faces: &[EmbeddedFace], serif: bool, bold: bool) -> Vec<&EmbeddedFace> {
    let jpan = || faces.iter().filter(|f| f.has_script("Jpan"));
    let mut preferred = Vec::new();
    if !serif {
        let style = if bold { "Bold" } else { "Regular" };
        preferred.extend(jpan().find(|f| f.family == "BIZ UDPGothic" && f.style == style));
        preferred.extend(jpan().find(|f| f.family == "BIZ UDPGothic" && f.style == "Regular"));
    }
    for family in ["Shippori Mincho", "BIZ UDMincho"] {
        preferred.extend(jpan().find(|f| f.family == family && f.style == "Regular"));
    }
    let mut out: Vec<&EmbeddedFace> = Vec::new();
    for face in preferred.into_iter().chain(jpan().filter(|f| f.style == "Regular")).chain(jpan()) {
        if !out.iter().any(|f| std::ptr::eq(*f, face)) {
            out.push(face);
        }
    }
    out
}

const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const fn find(family: &str, style: &str) -> Option<&'static [u8]> {
    let mut i = 0;
    while i < CRAFT_FONTS.len() {
        let f = &CRAFT_FONTS[i];
        if str_eq(f.family, family) && str_eq(f.style, style) {
            return Some(f.bytes);
        }
        i += 1;
    }
    None
}

/// Shippori Mincho Regular from craft-fonts, the preferred face for Japanese document text.
/// `None` when PdfCraft was built without craft-fonts.
pub static SHIPPORI_MINCHO: Option<&[u8]> = find("Shippori Mincho", "Regular");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_fallback_order_follows_the_ui_language() {
        // Synthetic faces: order_cjk must not depend on the real build input.
        static BYTES: &[u8] = b"fake";
        static LATN: &[&str] = &["Latn"];
        static JPAN_LATN: &[&str] = &["Jpan", "Latn"];
        static HANS_LATN: &[&str] = &["Hans", "Latn"];
        let biz_bold = EmbeddedFace { family: "BIZ UDPGothic", style: "Bold", scripts: JPAN_LATN, index: 0, bytes: BYTES, role: None };
        let biz = EmbeddedFace { family: "BIZ UDPGothic", style: "Regular", scripts: JPAN_LATN, index: 0, bytes: BYTES, role: None };
        let hans = EmbeddedFace { family: "FakeHans", style: "Regular", scripts: HANS_LATN, index: 0, bytes: BYTES, role: None };
        let inter = EmbeddedFace { family: "Inter", style: "Regular", scripts: LATN, index: 0, bytes: BYTES, role: None };
        let faces = [biz_bold, biz, hans, inter];
        // Chinese mode: the Hans group first (manifest order), then Japanese.
        let zh: Vec<&str> = order_cjk([&faces[0], &faces[2], &faces[1]], true).iter().map(|f| f.family).collect();
        assert_eq!(zh, ["FakeHans", "BIZ UDPGothic", "BIZ UDPGothic"]);
        // Japanese mode keeps the historical default: the Japanese UI face first.
        let ja: Vec<&str> = order_cjk([&faces[0], &faces[2], &faces[1]], false).iter().map(|f| f.family).collect();
        assert_eq!(ja, ["BIZ UDPGothic", "BIZ UDPGothic", "FakeHans"]);
        // Latin-only faces are never interface CJK fallbacks.
        assert!(order_cjk(&faces, true).iter().all(|f| f.has_script("Hans") || f.has_script("Jpan")));
        // A serif listed first in the manifest never leads the Japanese group, in either mode.
        let mincho = EmbeddedFace { family: "Shippori Mincho", style: "Regular", scripts: JPAN_LATN, index: 0, bytes: BYTES, role: None };
        let zh: Vec<&str> = order_cjk([&mincho, &faces[2], &faces[1]], true).iter().map(|f| f.family).collect();
        assert_eq!(zh, ["FakeHans", "BIZ UDPGothic", "Shippori Mincho"]);
        let ja: Vec<&str> = order_cjk([&mincho, &faces[2], &faces[1]], false).iter().map(|f| f.family).collect();
        assert_eq!(ja, ["BIZ UDPGothic", "Shippori Mincho", "FakeHans"]);
    }

    #[test]
    fn document_faces_match_style_without_inventing_missing_weights() {
        let faces = [
            EmbeddedFace { family: "Shippori Mincho", style: "Regular", scripts: &["Jpan"], index: 0, role: None, bytes: b"serif" },
            EmbeddedFace { family: "BIZ UDPGothic", style: "Regular", scripts: &["Jpan"], index: 0, role: None, bytes: b"sans" },
            EmbeddedFace { family: "BIZ UDPGothic", style: "Bold", scripts: &["Jpan"], index: 0, role: None, bytes: b"bold" },
            EmbeddedFace { family: "BIZ UDPGothic", style: "Bold", scripts: &["Latn"], index: 0, role: None, bytes: b"not-japanese" },
        ];
        assert_eq!(document_face(&faces, false, false).unwrap().bytes, b"sans");
        assert_eq!(document_face(&faces, false, true).unwrap().bytes, b"bold");
        assert_eq!(document_face(&faces, true, false).unwrap().bytes, b"serif");
        assert_eq!(document_face(&faces, true, true).unwrap().bytes, b"serif");
        assert_eq!(document_face(&faces[..2], false, true).unwrap().bytes, b"sans");
        assert_eq!(document_face(&faces[..1], false, true).unwrap().bytes, b"serif");
        // Every Japanese face is a candidate, the style's match first and none twice.
        let order: Vec<&[u8]> = document_faces(&faces, false, true).iter().map(|f| f.bytes).collect();
        assert_eq!(order, [b"bold".as_slice(), b"sans", b"serif"]);
        let order: Vec<&[u8]> = document_faces(&faces, true, false).iter().map(|f| f.bytes).collect();
        assert_eq!(order, [b"serif".as_slice(), b"sans", b"bold"]);
        assert!(document_face(&faces[3..], false, true).is_none());
        assert!(document_face(&[], false, true).is_none());
    }

    #[test]
    fn arabic_faces_are_picked_by_script_in_manifest_order() {
        // Synthetic faces: the filter must not depend on the real build input.
        static BYTES: &[u8] = b"fake";
        static ARAB_LATN: &[&str] = &["Arab", "Latn"];
        static JPAN: &[&str] = &["Jpan"];
        let naskh = EmbeddedFace { family: "FakeNaskh", style: "Regular", scripts: ARAB_LATN, index: 0, bytes: BYTES, role: None };
        let biz = EmbeddedFace { family: "BIZ UDPGothic", style: "Regular", scripts: JPAN, index: 0, bytes: BYTES, role: None };
        let kufi = EmbeddedFace { family: "FakeKufi", style: "Regular", scripts: ARAB_LATN, index: 0, bytes: BYTES, role: None };
        let faces = [naskh, biz, kufi];
        let ar: Vec<&str> = arabic(&faces).iter().map(|f| f.family).collect();
        assert_eq!(ar, ["FakeNaskh", "FakeKufi"]);
        // An Arabic face is never a CJK fallback, and the other way round.
        assert!(order_cjk(&faces, false).iter().all(|f| f.family == "BIZ UDPGothic"));
        assert_eq!(ui_arabic_fonts().len(), CRAFT_FONTS.iter().filter(|f| f.has_script("Arab")).count());
    }

    #[test]
    fn hebrew_document_faces_follow_the_weight() {
        static HEBR_LATN: &[&str] = &["Hebr", "Latn"];
        let faces = [
            EmbeddedFace { family: "BIZ UDPGothic", style: "Bold", scripts: &["Jpan"], index: 0, role: None, bytes: b"not-hebrew" },
            EmbeddedFace { family: "FakeHebrew", style: "Regular", scripts: HEBR_LATN, index: 0, role: None, bytes: b"regular" },
            EmbeddedFace { family: "FakeHebrew", style: "Bold", scripts: HEBR_LATN, index: 0, role: None, bytes: b"bold" },
        ];
        assert_eq!(hebrew_face(&faces, false).map(|f| f.bytes), Some(&b"regular"[..]));
        assert_eq!(hebrew_face(&faces, true).map(|f| f.bytes), Some(&b"bold"[..]));
        // No bold face: Regular, never a face of another script.
        assert_eq!(hebrew_face(&faces[..2], true).map(|f| f.bytes), Some(&b"regular"[..]));
        assert!(hebrew_face(&faces[..1], false).is_none());
        assert_eq!(ui_hebrew_fonts().len(), CRAFT_FONTS.iter().filter(|f| f.has_script("Hebr")).count());
        assert_eq!(document_hebrew_font(false).is_some(), CRAFT_FONTS.iter().any(|f| f.has_script("Hebr")));
    }

    /// `build.rs` embeds the Japanese CJK faces whole and any other CJK face only up to
    /// `MAX_OTHER_CJK_BYTES` (the explicitly pinned native-only LXGW UI fallback has a separate
    /// 16 MiB cap), so a checkout with a large Chinese or Korean face (craft-fonts has the
    /// 16.4 MB Noto Sans CJK SC) leaves it out. The four Japanese faces of the pinned craft-fonts
    /// are 24.1 MB.
    #[test]
    fn cjk_faces_follow_the_size_rule() {
        use crate::cjk_scripts::{MAX_NATIVE_UI_CJK_BYTES, MAX_OTHER_CJK_BYTES, NATIVE_UI_CJK_FAMILIES, WHOLE_CJK_FAMILIES, is_cjk_script};
        // A tag can carry a region after the code: craft-fonts tags Noto Sans HK UI `Hant-HK`.
        assert!(["Hant-HK", "Hans", "Jpan", "Hang"].iter().all(|s| is_cjk_script(s)));
        assert!(!["Latn", "Arab", "Hebr"].iter().any(|s| is_cjk_script(s)));
        let cjk = CRAFT_FONTS.iter().filter(|f| f.scripts.iter().any(|s| is_cjk_script(s)));
        let (whole, other): (Vec<&EmbeddedFace>, Vec<&EmbeddedFace>) = cjk.partition(|f| WHOLE_CJK_FAMILIES.contains(&f.family));
        for face in &other {
            let len = face.bytes.len();
            let native_ui = NATIVE_UI_CJK_FAMILIES.contains(&face.family);
            assert!(!cfg!(target_arch = "wasm32") || !native_ui, "native UI face in web payload");
            let cap = if native_ui { MAX_NATIVE_UI_CJK_BYTES } else { MAX_OTHER_CJK_BYTES };
            assert!(u64::try_from(len).is_ok_and(|n| n <= cap), "{} is embedded at {len} bytes", face.name());
        }
        let bytes: usize = whole.iter().map(|f| f.bytes.len()).sum();
        assert!(bytes < 25_000_000, "{bytes} bytes of Japanese faces");
    }

    /// The bundled Inter faces carry the Latin, Greek and Cyrillic letters the craft-fonts faces
    /// lack: Uzbek қ ғ ҳ, Kazakh ә ө ү ұ һ, Ukrainian ї є ґ, Greek. This one runs with or without
    /// CRAFT_FONTS_DIR (#2593).
    #[test]
    fn latin_faces_carry_the_letters_the_japanese_faces_lack() {
        for face in document_latin_fonts(false).into_iter().chain(document_latin_fonts(true)) {
            for ch in ['қ', 'ғ', 'ҳ', 'ә', 'ө', 'ү', 'ұ', 'һ', 'ї', 'є', 'ґ', 'ё', 'α', 'β', 'γ', '≥', '≈'] {
                assert!(crate::japanese_glyph_from(face, ch).is_ok(), "{} has no {ch}", face.name());
            }
        }
        assert_eq!(document_latin_fonts(false)[0].style, "Regular");
        assert_eq!(document_latin_fonts(true)[0].style, "SemiBold");
    }

    #[test]
    fn craft_fonts_are_optional_and_consistent() {
        // Holds with or without CRAFT_FONTS_DIR.
        assert_eq!(SHIPPORI_MINCHO.is_some(), CRAFT_FONTS.iter().any(|f| f.family == "Shippori Mincho" && f.style == "Regular"));
        assert_eq!(document_japanese_font().is_some(), CRAFT_FONTS.iter().any(|f| f.has_script("Jpan")));
        let ui = ui_japanese_fonts();
        assert_eq!(ui.len(), CRAFT_FONTS.iter().filter(|f| f.has_script("Jpan")).count());
        let ui_zh = ui_chinese_fonts();
        assert_eq!(ui_zh.len(), CRAFT_FONTS.iter().filter(|f| f.has_script("Hans")).count());
        if CRAFT_FONTS.is_empty() {
            eprintln!("built without craft-fonts (CRAFT_FONTS_DIR unset): no Japanese faces, as expected");
            assert!(SHIPPORI_MINCHO.is_none() && ui.is_empty() && ui_zh.is_empty());
            return;
        }
        assert!(CRAFT_FONTS.iter().all(|f| !f.bytes.is_empty()));
        if CRAFT_FONTS.iter().any(|f| f.family == "BIZ UDPGothic") {
            assert_eq!(ui.first().map(|f| f.family), Some("BIZ UDPGothic"));
        }
        assert_eq!(document_japanese_font().map(|f| f.family), Some("Shippori Mincho"));
    }
}
