//! The one chain PdfCraft follows when a document does not embed a font program, and a record of
//! what it chose and why.
//!
//! Substitution used to be decided per caller and never written down: `edit` picked a craft face
//! by the script of the text it was drawing, `Std14::from_base_font` matched Latin names, and the
//! renderer did its own thing. Nothing told the *user* that the glyphs they are looking at are not
//! the ones the document asked for, which is the difference between "PdfCraft shows nothing for my
//! Chinese PDF" being a mystery and being a sentence.
//!
//! This module answers the *dictionary* question — "the document does not carry this program; what
//! will stand in for it, and why?" — which is what inspection and reporting need, and the renderer
//! asks it too ([`stand_in`]), so what a report names is what is drawn. For the characters a
//! stand-in has no glyph for, [`further_faces`] gives every face in the order to try them; it
//! still does not answer the *text* question ("which face has a glyph for こ?"), because that
//! depends on the string being drawn and is handled where the drawing happens.
//!
//! The chain is tried in order, and the first step that yields a face wins:
//!
//! 1. The document embeds the program. Nothing is substituted.
//! 2. `/BaseFont` names one of the standard 14 (or a well-known alias such as Arial). Exact
//!    published metrics apply, so this is as close to right as a substitution gets.
//! 3. craft-fonts holds that exact family. Never another family — see [`crate::embedding`].
//! 4. The descendant font's `/CIDSystemInfo /Ordering` names a CJK collection (Japan1, GB1, CNS1,
//!    Korea1, KR) and craft-fonts has a Han face: the best-ranked one stands in ([`cjk_stand_in`]),
//!    a face for that script when there is one (Simplified Chinese for GB1, Hangul for Korea1),
//!    else the face of the nearest Han script (Japanese for CNS1, whose kanji keep the
//!    traditional forms), each of the Mincho/Song, Gothic/Hei or Kai design and the weight the
//!    font asks for.
//! 5. The ordering names a CJK collection and there is **no** Han face (built without craft-fonts).
//!    There is no honest substitute, so the chain reports none: the caller draws nothing and can
//!    say why.
//! 6. Otherwise, the nearest of the standard 14 by `/FontDescriptor /Flags` — serif, fixed pitch,
//!    bold and italic. This step always succeeds, so a simple font is never left without a face.

use crate::{CraftFont, Std14};

/// What a font dictionary asks for, as far as the dictionary alone can say.
#[derive(Clone, Copy, Debug, Default)]
pub struct Request<'a> {
    /// `/BaseFont` with any `ABCDEF+` subset tag already stripped.
    pub base_font: &'a str,
    /// Whether the descriptor carries `/FontFile`, `/FontFile2` or `/FontFile3` (or it is Type 3).
    pub embedded: bool,
    /// `/FontDescriptor /Flags`, or 0 when there is no descriptor.
    pub flags: u32,
    /// The descendant font's `/CIDSystemInfo /Ordering`, for a composite font.
    pub ordering: Option<&'a str>,
    /// `/FontDescriptor /FontWeight`, or 0 when there is none.
    pub weight: u32,
}

/// The face the chain picked.
///
/// `Debug`, `PartialEq` and `Eq` are written by hand rather than derived, because [`CraftFont`]
/// carries a whole font program: deriving would print megabytes and compare font files.
#[derive(Clone, Copy)]
pub enum Face {
    /// The document's own program; nothing was substituted.
    Embedded,
    /// One of the standard 14, whose metrics are exact.
    Standard14(Std14),
    /// A face from the optional craft-fonts build input.
    Craft(&'static CraftFont),
    /// No face at all. The caller must draw nothing rather than draw the wrong glyphs.
    None,
}

impl PartialEq for Face {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Embedded, Self::Embedded) | (Self::None, Self::None) => true,
            (Self::Standard14(a), Self::Standard14(b)) => a == b,
            // craft-fonts entries are unique statics, so identity is the comparison that means
            // what we want: two styles of one family are different faces and must not be equal.
            (Self::Craft(a), Self::Craft(b)) => std::ptr::eq(*a, *b),
            _ => false,
        }
    }
}

impl Eq for Face {}

impl std::fmt::Debug for Face {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Embedded => f.write_str("Embedded"),
            Self::Standard14(s) => write!(f, "Standard14({})", s.base_font()),
            // The name, never the bytes.
            Self::Craft(c) => write!(f, "Craft({})", c.name()),
            Self::None => f.write_str("None"),
        }
    }
}

/// Why the chain picked that face. The variants are in the order the chain tries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// The document carries the font program.
    Embedded,
    /// `/BaseFont` names a standard-14 face or a well-known alias of one.
    NamedStandard14,
    /// `/BaseFont` is otherwise unknown, but its name hints at a family ("Serif", "Roman",
    /// "Mono"). A guess from the name, and reported as one.
    NameHintedStandard14,
    /// craft-fonts holds this exact family.
    ExactCraftFamily,
    /// A craft-fonts face for the script named by `/CIDSystemInfo /Ordering`.
    ScriptFace,
    /// No face for the script the ordering names, so the face of the nearest Han script.
    NearestScriptFace,
    /// The ordering names a script with no licensed face available.
    NoFaceForScript,
    /// The nearest standard-14 face by descriptor flags.
    StyleMatched,
}

impl Why {
    /// A stable sentence for a report, written for whoever has to act on it.
    pub fn note(self) -> &'static str {
        match self {
            Self::Embedded => "embedded in the document; not substituted",
            Self::NamedStandard14 => "names one of the standard 14 fonts, or a well-known alias of one, whose metrics are exact",
            Self::NameHintedStandard14 => {
                "not embedded and not a known family, but its name hints at one (Serif, Roman or Mono),                  so the nearest standard-14 face was chosen from that hint"
            }
            Self::ExactCraftFamily => "this exact family is available, so no other family was chosen",
            Self::ScriptFace => "substituted by a face for the script the font declares",
            Self::NearestScriptFace => {
                "no face is available for the script this font declares, so a face for the nearest Han script stands in, \
                 whose characters may take that script's forms"
            }
            Self::NoFaceForScript => {
                "no licensed face is available for the script this font declares, so nothing is drawn for it \
                 rather than the wrong glyphs"
            }
            Self::StyleMatched => "not embedded and not a known family, so the nearest standard-14 face was chosen by its descriptor flags",
        }
    }

    /// Whether the document's own glyphs are being shown. Only [`Why::Embedded`] is faithful.
    pub fn is_faithful(self) -> bool {
        matches!(self, Self::Embedded)
    }
}

/// What the chain chose, and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Substitution {
    pub face: Face,
    pub why: Why,
}

impl Substitution {
    /// The chosen face's name, for a report. `None` when nothing was chosen.
    pub fn face_name(&self) -> Option<String> {
        match self.face {
            Face::Embedded => None,
            Face::Standard14(f) => Some(f.base_font().to_owned()),
            Face::Craft(f) => Some(f.name()),
            Face::None => None,
        }
    }
}

/// Apply the chain to `req`. Total: every request gets a [`Substitution`].
///
/// See the module documentation for the order and the reasoning behind each step.
pub fn substitute(req: &Request<'_>) -> Substitution {
    let at = |face, why| Substitution { face, why };
    if req.embedded {
        return at(Face::Embedded, Why::Embedded);
    }
    if let Some((f, how)) = Std14::recognise(req.base_font) {
        return match how {
            crate::std14::Match::Alias => at(Face::Standard14(f), Why::NamedStandard14),
            crate::std14::Match::FamilyHint => at(Face::Standard14(f), Why::NameHintedStandard14),
        };
    }
    if let Some(f) = crate::embedding::matching_faces(req.base_font).first() {
        return at(Face::Craft(f), Why::ExactCraftFamily);
    }
    let scripts = req.ordering.map(collection_scripts).unwrap_or_default();
    if !scripts.is_empty() {
        // A Han face stands in; without one there is no honest substitute to offer.
        return match cjk_stand_in(req) {
            Some(f) if scripts.iter().any(|s| f.covers(s)) => at(Face::Craft(f), Why::ScriptFace),
            Some(f) => at(Face::Craft(f), Why::NearestScriptFace),
            None => at(Face::None, Why::NoFaceForScript),
        };
    }
    at(Face::Standard14(style_matched(req.flags)), Why::StyleMatched)
}

/// The nearest standard-14 face for `/FontDescriptor /Flags` (ISO 32000-2 table 121): fixed pitch
/// is bit 1, serif bit 2, italic bit 7 and ForceBold bit 19.
fn style_matched(flags: u32) -> Std14 {
    let (bold, italic) = (flags & 0x4_0000 != 0, flags & 0x40 != 0);
    match (flags & 1 != 0, flags & 2 != 0, bold, italic) {
        (true, _, false, false) => Std14::Courier,
        (true, _, true, false) => Std14::CourierBold,
        (true, _, false, true) => Std14::CourierOblique,
        (true, _, true, true) => Std14::CourierBoldOblique,
        (false, true, false, false) => Std14::TimesRoman,
        (false, true, true, false) => Std14::TimesBold,
        (false, true, false, true) => Std14::TimesItalic,
        (false, true, true, true) => Std14::TimesBoldItalic,
        (false, false, false, false) => Std14::Helvetica,
        (false, false, true, false) => Std14::HelveticaBold,
        (false, false, false, true) => Std14::HelveticaOblique,
        (false, false, true, true) => Std14::HelveticaBoldOblique,
    }
}

/// The face that draws a composite font the document doesn't embed: the craft-fonts face of the
/// exact family it names (step 3 of the chain), else its [`cjk_stand_in`] (step 4). `None` when
/// there is neither, and the standard substitute stays.
pub fn stand_in(req: &Request<'_>) -> Option<&'static CraftFont> {
    crate::embedding::matching_faces(req.base_font).into_iter().next().or_else(|| cjk_stand_in(req))
}

/// The face that stands in for a composite font the document doesn't embed whose ordering names
/// a CJK collection: the best of [`further_faces`], when it is a face for a Han script. `None` for
/// any other font, and when there is no Han face (built without craft-fonts).
///
/// Faces are found by Unicode, never by CID: no face here has Adobe's glyph order (Noto CJK is
/// CID-keyed as Adobe-Identity-0), so a CID taken as a glyph index would draw another glyph.
pub fn cjk_stand_in(req: &Request<'_>) -> Option<&'static CraftFont> {
    stand_in_from(req, &candidates())
}

/// Every face there is, best first, for the characters a substitute for `req`'s font has no glyph
/// for. The stand-in, if any, comes first: a caller drawing with it skips it.
///
/// The order: the exact family the font names; then the script its ordering is for (or, for an
/// ordering that names none, such as Identity, the script its name suggests: `SimSun` is
/// Simplified Chinese); then, for a CJK font, the other Han scripts, nearest first, before every
/// other face, and for any other font every other face (the bundled Inter, whose Cyrillic is
/// complete) before the Han ones. Within each, the faces of the font's design (Mincho/Song,
/// Gothic/Hei or Kai, by its name or else its Serif flag) and weight come first. Never empty:
/// craft-fonts is optional, but the bundled Inter is in every build.
pub fn further_faces(req: &Request<'_>) -> Vec<&'static CraftFont> {
    ranked_from(req, &candidates())
}

/// craft-fonts' faces (none without it), then the bundled Inter faces.
fn candidates() -> Vec<&'static CraftFont> {
    crate::CRAFT_FONTS.iter().chain(crate::document_latin_fonts(false)).chain(crate::document_latin_fonts(true)).collect()
}

fn stand_in_from<'a>(req: &Request<'_>, faces: &[&'a CraftFont]) -> Option<&'a CraftFont> {
    req.ordering.map(collection_scripts).filter(|s| !s.is_empty())?;
    ranked_from(req, faces).into_iter().next().filter(|face| HAN.iter().any(|s| face.covers(s)))
}

/// The design of a face, or the one a font's name or flags ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Design {
    /// Mincho, Song, Ming, Batang/Myeongjo: serifed.
    Serif,
    /// Gothic, Hei, Dotum/Gulim: sans serif.
    Sans,
    /// Kai: brush-written.
    Brush,
}

/// The Han scripts: a face for any of them has CJK ideographs.
const HAN: [&str; 4] = ["Hans", "Hant", "Jpan", "Kore"];

/// Families that cover more of their script than the others of the same design: Shippori Mincho
/// lacks the half-width katakana (U+FF61–U+FF9F) common in Japanese documents, and a character a
/// stand-in lacks is drawn by a further face, likely of another design.
const BROAD_COVERAGE: [&str; 2] = ["BIZ UDMincho", "BIZ UDPGothic"];

/// `faces`, best first for `req` (a stable sort, so equal faces keep their order).
fn ranked_from<'a>(req: &Request<'_>, faces: &[&'a CraftFont]) -> Vec<&'a CraftFont> {
    let name = crate::embedding::face_key(req.base_font);
    let preferred = req.ordering.map(collection_scripts).filter(|s| !s.is_empty()).unwrap_or_else(|| name_scripts(&name));
    let cjk = preferred.iter().any(|s| HAN.contains(s));
    let han_order = han_order(preferred);
    let design = name_design(&name).unwrap_or(if req.flags & 0b10 != 0 { Design::Serif } else { Design::Sans });
    let bold = name.contains("bold") || req.flags & 0x4_0000 != 0 || req.weight >= 600;
    let exact: Vec<&CraftFont> = crate::embedding::matching_faces(req.base_font);
    let tier = |face: &CraftFont| {
        if exact.iter().any(|e| std::ptr::eq(*e, face)) {
            return 0;
        }
        if let Some(i) = preferred.iter().position(|s| face.covers(s)) {
            return 1 + i;
        }
        let after = 1 + preferred.len();
        match (han_order.iter().position(|s| face.covers(s)), cjk) {
            (Some(i), true) => after + i,
            (None, true) => after + HAN.len(),
            (None, false) => after,
            (Some(i), false) => after + 1 + i,
        }
    };
    let design_rank = |face: &CraftFont| match name_design(&crate::embedding::face_key(face.family)) {
        Some(d) if d == design => 0,
        None => 1,
        Some(_) => 2,
    };
    let weight_rank = |face: &CraftFont| usize::from((face.style == "Bold") != bold);
    let coverage_rank = |face: &CraftFont| usize::from(!BROAD_COVERAGE.contains(&face.family));
    let mut out = faces.to_vec();
    out.sort_by_key(|face| (tier(face), design_rank(face), weight_rank(face), coverage_rank(face)));
    out
}

/// The scripts a CJK collection's characters are for, best first; none for any other ordering.
/// `Identity` is not a script: it says the codes are glyph ids, so it says nothing about the face.
fn collection_scripts(ordering: &str) -> &'static [&'static str] {
    match ordering {
        "Japan1" => &["Jpan"],
        "GB1" => &["Hans"],
        "CNS1" => &["Hant"],
        "Korea1" | "KR" => &["Kore", "Hang"],
        _ => &[],
    }
}

/// The other Han scripts to try for text of the `preferred` scripts, nearest first: Japanese
/// kanji and Korean hanja keep the traditional forms, Simplified Chinese doesn't.
fn han_order(preferred: &[&str]) -> &'static [&'static str] {
    match preferred.first().copied() {
        Some("Jpan") => &["Hant", "Hans", "Kore"],
        Some("Hans") => &["Hant", "Jpan", "Kore"],
        Some("Hant") => &["Jpan", "Kore", "Hans"],
        Some("Kore" | "Hang") => &["Hant", "Jpan", "Hans"],
        _ => &HAN,
    }
}

/// Fragments (lowercase, letters and digits only) of the names of CJK fonts, for an ordering that
/// says no collection, and the scripts they are for. Korean first: `HYGoThic` is not Japanese.
const NAME_SCRIPTS: [(&[&str], &[&str]); 4] = [
    (
        &["batang", "gulim", "dotum", "gungsuh", "malgun", "myeongjo", "myungjo", "hygothic", "hygoth", "nanum", "applegothic", "hangul"],
        &["Kore", "Hang"],
    ),
    (&["mingliu", "msung", "mhei", "jhenghei", "dfkai", "kaiu", "pingfangtc", "pingfanghk", "lihei", "lisong", "sanscns"], &["Hant"]),
    (
        &[
            "simsun",
            "simhei",
            "simkai",
            "simfang",
            "stsong",
            "stheiti",
            "stkaiti",
            "stfangsong",
            "stxihei",
            "stzhongsong",
            "fangsong",
            "kaiti",
            "yahei",
            "dengxian",
            "pingfangsc",
            "songti",
            "sansgb",
            "fzsong",
            "fzhei",
            "fzkai",
        ],
        &["Hans"],
    ),
    (
        &[
            "mincho",
            "heisei",
            "kozmin",
            "kozgo",
            "ryumin",
            "gothicbbb",
            "midashi",
            "meiryo",
            "yugoth",
            "yumin",
            "hiragino",
            "hirakaku",
            "hiramin",
            "msgothic",
            "mspgothic",
            "msuigothic",
            "ipagothic",
            "ipamincho",
            "ipaex",
            "osaka",
        ],
        &["Jpan"],
    ),
];

/// The scripts the font name `name` (a [`face_key`](crate::embedding::face_key)) suggests: one script, or none.
fn name_scripts(name: &str) -> &'static [&'static str] {
    NAME_SCRIPTS.iter().find(|(fragments, _)| fragments.iter().any(|f| name.contains(f))).map_or(&[], |(_, scripts)| scripts)
}

/// The design the name `name` (a [`face_key`](crate::embedding::face_key)) says, if it says one. Brush and serif names are checked
/// before sans ones: `HeiseiMin` is a Mincho, though it contains "hei".
fn name_design(name: &str) -> Option<Design> {
    const BRUSH: [&str; 1] = ["kai"];
    const SERIF: [&str; 12] = ["min", "song", "sun", "ming", "sung", "batang", "myeongjo", "myungjo", "gungsuh", "serif", "times", "fangsong"];
    const SANS: [&str; 13] = ["goth", "kozgo", "kaku", "maru", "hei", "sans", "dotum", "gulim", "yahei", "dengxian", "meiryo", "arial", "helvet"];
    let has = |fragments: &[&str]| fragments.iter().any(|f| name.contains(f));
    if has(&BRUSH) {
        Some(Design::Brush)
    } else if has(&SERIF) {
        Some(Design::Serif)
    } else if has(&SANS) {
        Some(Design::Sans)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req<'a>(base: &'a str, embedded: bool, flags: u32, ordering: Option<&'a str>) -> Request<'a> {
        Request { base_font: base, embedded, flags, ordering, weight: 0 }
    }

    #[test]
    fn an_embedded_font_is_never_substituted() {
        // True even when the name and flags would otherwise pick something: embedding wins first.
        for (base, ordering) in [("Helvetica", None), ("Whatever", Some("GB1")), ("", None)] {
            let s = substitute(&req(base, true, 0, ordering));
            assert_eq!(s, Substitution { face: Face::Embedded, why: Why::Embedded });
            assert!(s.why.is_faithful());
            assert_eq!(s.face_name(), None, "nothing stands in, so there is no substitute to name");
        }
    }

    #[test]
    fn a_standard_14_name_wins_over_the_flags() {
        // Flags claiming a fixed-pitch serif must not override a name we have exact metrics for.
        let s = substitute(&req("Times-BoldItalic", false, 0b11, None));
        assert_eq!(s.face, Face::Standard14(Std14::TimesBoldItalic));
        assert_eq!(s.why, Why::NamedStandard14);
        assert_eq!(s.face_name().as_deref(), Some("Times-BoldItalic"));
        // Aliases and subset tags resolve the same way.
        for name in ["Arial", "ArialMT", "Arial-Bold", "Symbol", "ZapfDingbats", "CourierNew"] {
            assert_eq!(substitute(&req(name, false, 0, None)).why, Why::NamedStandard14, "{name}");
        }
    }

    #[test]
    fn a_name_hint_is_reported_as_a_guess_not_as_a_standard_14_name() {
        // A house font whose name merely contains "Serif" gets Times, which is a sensible guess --
        // but calling that "names one of the standard 14" would tell the reader something untrue.
        let hinted = substitute(&req("SomeHouseSerif", false, 0, None));
        assert_eq!(hinted.face, Face::Standard14(Std14::TimesRoman));
        assert_eq!(hinted.why, Why::NameHintedStandard14);
        assert!(hinted.why.note().contains("hints at one"), "{}", hinted.why.note());

        // Style words still apply to the guess.
        assert_eq!(substitute(&req("SomeHouseSerif-BoldItalic", false, 0, None)).face, Face::Standard14(Std14::TimesBoldItalic));
        // "Mono" hints at Courier the same way.
        assert_eq!(substitute(&req("HouseMono", false, 0, None)).why, Why::NameHintedStandard14);

        // A real alias is still reported as the request it is.
        for alias in ["Arial", "CourierNew", "Times-Roman", "Symbol"] {
            assert_eq!(substitute(&req(alias, false, 0, None)).why, Why::NamedStandard14, "{alias}");
        }
        // A name with no hint at all falls through to the flags instead.
        assert_eq!(substitute(&req("HouseGrotesk", false, 0b10, None)).why, Why::StyleMatched);
    }

    #[test]
    fn flags_pick_the_nearest_of_the_fourteen() {
        // Bit 1 fixed pitch, bit 2 serif, bit 7 italic, bit 19 ForceBold.
        let cases = [
            (0, Std14::Helvetica),
            (0b10, Std14::TimesRoman),
            (0b1, Std14::Courier),
            (0x40, Std14::HelveticaOblique),
            (0x4_0000, Std14::HelveticaBold),
            (0x4_0040, Std14::HelveticaBoldOblique),
            (0b10 | 0x40, Std14::TimesItalic),
            (0b10 | 0x4_0000, Std14::TimesBold),
            (0b10 | 0x4_0040, Std14::TimesBoldItalic),
            (0b1 | 0x40, Std14::CourierOblique),
            (0b1 | 0x4_0000, Std14::CourierBold),
            (0b1 | 0x4_0040, Std14::CourierBoldOblique),
            // Fixed pitch and serif together: monospace is the stronger signal for layout.
            (0b11, Std14::Courier),
        ];
        for (flags, want) in cases {
            let s = substitute(&req("SomethingUnknown", false, flags, None));
            assert_eq!(s.face, Face::Standard14(want), "flags {flags:#x}");
            assert_eq!(s.why, Why::StyleMatched);
        }
    }

    #[test]
    fn a_cjk_collection_without_a_han_face_reports_none_rather_than_latin() {
        // Without a Han face a Chinese or Korean PDF draws nothing. The point of this is that we
        // can say so, instead of quietly standing Helvetica in and showing the wrong glyphs.
        let han = crate::CRAFT_FONTS.iter().any(|f| HAN.iter().any(|s| f.covers(s)));
        for ordering in ["GB1", "CNS1", "Korea1", "KR", "Japan1"] {
            let s = substitute(&req("SomeCJKFont", false, 0, Some(ordering)));
            assert!(!s.why.is_faithful());
            if han {
                assert!(matches!(s.face, Face::Craft(f) if HAN.iter().any(|s| f.covers(s))), "{ordering}: {s:?}");
                assert!(matches!(s.why, Why::ScriptFace | Why::NearestScriptFace), "{ordering}: {s:?}");
                continue;
            }
            assert_eq!(s.face, Face::None, "{ordering}");
            assert_eq!(s.why, Why::NoFaceForScript, "{ordering}");
            assert_eq!(s.face_name(), None);
            assert!(s.why.note().contains("no licensed face"), "{ordering}");
        }
    }

    /// #5329: the chain names the face the renderer draws with ([`cjk_stand_in`]), and says
    /// whether it is a face for the declared script or for the nearest one.
    #[test]
    fn the_chain_reports_the_stand_in_the_renderer_draws_with() {
        for ordering in ["GB1", "CNS1", "Korea1", "Japan1"] {
            let r = req("SomeCJKFont-Bold", false, 0b10, Some(ordering));
            let s = substitute(&r);
            match cjk_stand_in(&r) {
                Some(f) => {
                    assert_eq!(s.face, Face::Craft(f), "{ordering}");
                    let own = collection_scripts(ordering).iter().any(|script| f.covers(script));
                    assert_eq!(s.why, if own { Why::ScriptFace } else { Why::NearestScriptFace }, "{ordering}");
                }
                None => assert_eq!(s.why, Why::NoFaceForScript, "{ordering}"),
            }
        }
    }

    #[test]
    fn identity_ordering_is_not_a_script_and_falls_through_to_the_flags() {
        // /Ordering (Identity) says the codes are glyph ids. It implies nothing about the face,
        // so treating it as a script would invent a substitution the dictionary never asked for.
        let s = substitute(&req("SomeSubsetFont", false, 0b10, Some("Identity")));
        assert_eq!(s.face, Face::Standard14(Std14::TimesRoman));
        assert_eq!(s.why, Why::StyleMatched);
        // An unregistered ordering is treated the same way, not guessed at.
        assert_eq!(substitute(&req("X", false, 0, Some("MadeUp1"))).why, Why::StyleMatched);
    }

    #[test]
    fn japanese_depends_on_craft_fonts_and_says_which_it_got() {
        // craft-fonts is an optional build input, so both outcomes are correct -- but they must be
        // distinguishable, and neither may be a silent Latin stand-in.
        let s = substitute(&req("SomeJapaneseFont", false, 0, Some("Japan1")));
        if crate::CRAFT_FONTS.iter().any(|f| f.covers("Jpan")) {
            assert_eq!(s.why, Why::ScriptFace);
            assert!(matches!(s.face, Face::Craft(_)));
            assert!(s.face_name().is_some_and(|n| !n.is_empty()));
        } else {
            assert_eq!(s.why, Why::NoFaceForScript);
            assert_eq!(s.face, Face::None);
        }
        assert!(!s.why.is_faithful(), "a substitution is never faithful");
    }

    #[test]
    fn every_why_has_a_distinct_usable_note() {
        let all = [
            Why::Embedded,
            Why::NamedStandard14,
            Why::NameHintedStandard14,
            Why::ExactCraftFamily,
            Why::ScriptFace,
            Why::NearestScriptFace,
            Why::NoFaceForScript,
            Why::StyleMatched,
        ];
        let mut notes: Vec<&str> = all.iter().map(|w| w.note()).collect();
        notes.sort_unstable();
        let before = notes.len();
        notes.dedup();
        assert_eq!(notes.len(), before, "each reason reads differently");
        assert!(all.iter().all(|w| !w.note().is_empty() && w.note().len() > 20), "notes are sentences, not labels");
        assert_eq!(all.iter().filter(|w| w.is_faithful()).count(), 1, "only an embedded program is faithful");
    }

    #[test]
    fn hostile_input_is_total_and_never_panics() {
        // Every field is attacker-controlled: a name is any bytes the document chose.
        let long = "a".repeat(10_000);
        let names = ["", " ", "+", "ABCDEF+", "\u{0}\u{1}", long.as_str(), "../../etc/passwd", "Ĥéļvéťïçà"];
        for name in names {
            for flags in [0, u32::MAX, 0x4_0040, 1] {
                for ordering in [None, Some(""), Some("Japan1"), Some("\u{0}")] {
                    let s = substitute(&req(name, false, flags, ordering));
                    // Always a decision, and the reason always reads.
                    assert!(!s.why.note().is_empty());
                    assert_eq!(s.face == Face::None, s.why == Why::NoFaceForScript, "only one step declines: {name:?}");
                }
            }
        }
    }

    const fn face(family: &'static str, style: &'static str, scripts: &'static [&'static str]) -> CraftFont {
        CraftFont { family, style, scripts, bytes: &[] }
    }

    /// The faces of the pinned craft-fonts revision in a native build (Noto Sans CJK SC is over
    /// the embedding cap), in manifest order, then the bundled Inter.
    static PINNED: [CraftFont; 15] = [
        face("Shippori Mincho", "Regular", &["Jpan", "Latn"]),
        face("BIZ UDPGothic", "Regular", &["Jpan", "Latn"]),
        face("BIZ UDPGothic", "Bold", &["Jpan", "Latn"]),
        face("BIZ UDMincho", "Regular", &["Jpan", "Latn"]),
        face("Noto Sans Arabic", "Regular", &["Arab"]),
        face("Noto Sans Thai", "Regular", &["Thai"]),
        face("Noto Sans Georgian", "Regular", &["Geor"]),
        face("NanumGothic", "Regular", &["Hang", "Kore"]),
        face("Noto Sans Symbols", "Regular", &["Zsym"]),
        face("Noto Sans Symbols 2", "Regular", &["Zsym"]),
        face("Noto Sans Hebrew", "Regular", &["Hebr"]),
        face("Noto Sans Devanagari", "Regular", &["Deva"]),
        face("LXGW WenKai Lite", "Regular", &["Hani", "Hans"]),
        face("Inter", "Regular", &["Latn", "Cyrl", "Grek"]),
        face("Inter", "SemiBold", &["Latn", "Cyrl", "Grek"]),
    ];

    fn pinned() -> Vec<&'static CraftFont> {
        PINNED.iter().collect()
    }

    /// A non-embedded font named `name` with the Serif flag `serif`, `/FontWeight` `weight` and the
    /// ordering `ordering`.
    fn cid<'a>(name: &'a str, ordering: Option<&'a str>, serif: bool, weight: u32) -> Request<'a> {
        Request { base_font: name, embedded: false, flags: if serif { 0b10 } else { 0 }, ordering, weight }
    }

    fn stand_in(r: &Request<'_>, faces: &[&'static CraftFont]) -> Option<String> {
        stand_in_from(r, faces).map(CraftFont::name)
    }

    fn ranked(r: &Request<'_>, faces: &[&'static CraftFont]) -> Vec<String> {
        ranked_from(r, faces).into_iter().map(CraftFont::name).collect()
    }

    /// The Japanese stand-ins are the ones the renderer chose before (#260): Mincho names the BIZ
    /// UDMincho face (half-width katakana), Gothic names BIZ UDPGothic of their weight, other names
    /// by the Serif flag; the web build's one face stands in for a Mincho too.
    #[test]
    fn japanese_names_keep_their_stand_ins() {
        for (font, serif, weight, expected) in [
            ("HeiseiMin-W3", false, 0, "BIZ UDMincho Regular"),
            ("KozMinPro-Regular", false, 0, "BIZ UDMincho Regular"),
            ("Ryumin-Light", false, 0, "BIZ UDMincho Regular"),
            ("MS-PMincho", false, 0, "BIZ UDMincho Regular"),
            ("HiraMinProN-W3", false, 0, "BIZ UDMincho Regular"),
            ("HeiseiKakuGo-W5", true, 0, "BIZ UDPGothic Regular"),
            ("KozGoPro-Bold", true, 0, "BIZ UDPGothic Bold"),
            ("GothicBBB-Medium", true, 0, "BIZ UDPGothic Regular"),
            ("MS-Gothic", false, 0, "BIZ UDPGothic Regular"),
            ("HiraKakuProN-W6", false, 700, "BIZ UDPGothic Bold"),
            ("Unknown-Japanese", true, 0, "BIZ UDMincho Regular"),
            ("Unknown-Japanese", false, 0, "BIZ UDPGothic Regular"),
        ] {
            assert_eq!(stand_in(&cid(font, Some("Japan1"), serif, weight), &pinned()).as_deref(), Some(expected), "{font}");
        }
        let web = [&PINNED[1]];
        assert_eq!(stand_in(&cid("HeiseiMin-W3", Some("Japan1"), true, 0), &web).as_deref(), Some("BIZ UDPGothic Regular"));
    }

    /// #5329: an Adobe-GB1 font stands in with the Simplified Chinese face; the Japanese faces come
    /// next (for ideographs it lacks), those of the font's design first, before any non-Han face.
    #[test]
    fn simplified_chinese_fonts_stand_in_with_the_hans_face() {
        for font in ["STSong-Light", "STSong-Light,Bold", "STSong-Light,Italic", "STHeiti-Regular", "SimSun"] {
            let r = cid(font, Some("GB1"), true, 0);
            assert_eq!(stand_in(&r, &pinned()).as_deref(), Some("LXGW WenKai Lite Regular"), "{font}");
            let faces = ranked(&r, &pinned());
            assert_eq!(faces.first().map(String::as_str), Some("LXGW WenKai Lite Regular"), "the stand-in comes first: {faces:?}");
            assert!(faces.iter().skip(1).take(4).all(|f| f.starts_with("BIZ UD") || f.starts_with("Shippori")), "{font}: {faces:?}");
        }
        let second = |font: &str| ranked(&cid(font, Some("GB1"), false, 0), &pinned()).into_iter().nth(1);
        assert_eq!(second("STSong-Light").as_deref(), Some("BIZ UDMincho Regular"), "a Song font: the Mincho faces next");
        assert_eq!(second("STHeiti-Regular").as_deref(), Some("BIZ UDPGothic Regular"), "a Hei font: the Gothic faces next");
    }

    /// Korean fonts stand in with the Hangul face. Traditional Chinese has no face of its own in
    /// the pinned craft-fonts, so the nearest Han face (Japanese, of the font's design) stands in,
    /// and the chain says so.
    #[test]
    fn korean_and_traditional_chinese_fonts_get_the_nearest_face() {
        for font in ["HYSMyeongJo-Medium", "HYGoThic-Medium", "Batang"] {
            assert_eq!(stand_in(&cid(font, Some("Korea1"), false, 0), &pinned()).as_deref(), Some("NanumGothic Regular"), "{font}");
        }
        assert_eq!(stand_in(&cid("MSung-Light", Some("CNS1"), false, 0), &pinned()).as_deref(), Some("BIZ UDMincho Regular"));
        assert_eq!(stand_in(&cid("MHei-Medium", Some("CNS1"), false, 0), &pinned()).as_deref(), Some("BIZ UDPGothic Regular"));
    }

    /// A font of no CJK collection has no stand-in, and every face is a further face: the non-Han
    /// ones first (Inter's Cyrillic before the Japanese faces' partial one), unless its name says
    /// a CJK script.
    #[test]
    fn other_fonts_keep_the_standard_substitute_and_rank_every_face() {
        let r = cid("ArialUnicodeMS", Some("Identity"), false, 0);
        assert_eq!(stand_in(&r, &pinned()), None);
        let faces = ranked_from(&r, &pinned());
        assert_eq!(faces.len(), PINNED.len());
        let first_han = faces.iter().position(|f| HAN.iter().any(|s| f.covers(s)));
        let inter = faces.iter().position(|f| f.family == "Inter");
        assert!(inter < first_han, "{:?}", faces.iter().map(|f| f.name()).collect::<Vec<_>>());
        assert_eq!(ranked(&cid("SimSun", Some("Identity"), false, 0), &pinned()).first().map(String::as_str), Some("LXGW WenKai Lite Regular"));
        assert_eq!(ranked(&cid("MalgunGothic", None, false, 0), &pinned()).first().map(String::as_str), Some("NanumGothic Regular"));
    }

    /// Without craft-fonts only Inter is left: no stand-in, Inter as the further faces.
    #[test]
    fn without_craft_fonts_only_inter_is_a_further_face() {
        let inter = [&PINNED[13], &PINNED[14]];
        let r = cid("STSong-Light", Some("GB1"), true, 0);
        assert_eq!(stand_in(&r, &inter), None);
        assert_eq!(ranked(&r, &inter).len(), 2);
        assert!(ranked(&r, &[]).is_empty());
    }

    #[test]
    fn names_say_their_design() {
        use Design::{Brush, Sans, Serif};
        for (font, design) in [
            ("HeiseiMin-W3", Some(Serif)),
            ("HeiseiKakuGo-W5", Some(Sans)),
            ("KozGoPro-Bold", Some(Sans)),
            ("STSong-Light", Some(Serif)),
            ("SimSun", Some(Serif)),
            ("STHeiti", Some(Sans)),
            ("SimHei", Some(Sans)),
            ("STKaiti", Some(Brush)),
            ("DFKai-SB", Some(Brush)),
            ("MSung-Light", Some(Serif)),
            ("MHei-Medium", Some(Sans)),
            ("HYSMyeongJo-Medium", Some(Serif)),
            ("HYGoThic-Medium", Some(Sans)),
            ("LXGW WenKai Lite", Some(Brush)),
            ("Noto Sans CJK SC", Some(Sans)),
            ("Inter", None),
            ("CraftCJK-W3", None),
        ] {
            assert_eq!(name_design(&crate::embedding::face_key(font)), design, "{font}");
        }
    }

    /// The ranking is total over hostile names and orderings.
    #[test]
    fn ranking_hostile_requests_is_total() {
        let long = "kai".repeat(5_000);
        for name in ["", "+", "ABCDEF+", "\u{0}", long.as_str(), "Ĥéļvéťïçà-Bold"] {
            for ordering in [None, Some(""), Some("GB1"), Some("KR"), Some("\u{0}")] {
                let r = Request { base_font: name, embedded: false, flags: u32::MAX, ordering, weight: u32::MAX };
                assert_eq!(ranked_from(&r, &pinned()).len(), PINNED.len());
                let _ = stand_in_from(&r, &pinned());
            }
        }
    }
}
