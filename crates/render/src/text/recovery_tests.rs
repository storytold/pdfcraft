//! Copy with formatting's recovered layer (hayro-interpret patch 104): one interpretation reads
//! the strict text layer, unchanged, and what the recovery rules read; rules fill glyphs without
//! strict text, or override strict text they judge untrustworthy.

use super::*;
use pdfcraft_testkit::truetype::{Cmap, mac_gid, subset};

fn parse(bytes: Vec<u8>) -> Pdf {
    Pdf::new(std::sync::Arc::new(bytes)).unwrap()
}

/// Both layers of page 0, and the strict layer as the plain extraction reads it.
fn layers(pdf: &Pdf) -> (PageText, Option<RecoveredText>, PageText) {
    let settings = InterpreterSettings::default();
    let (strict, recovered) = extract_page_recovering(pdf, 0, &settings).unwrap();
    (strict, recovered, extract_page(pdf, 0, &settings).unwrap())
}

/// A one-page PDF: `Hi` in Helvetica (when `strict_words`), then `shown` in font `F3`, whose
/// dictionary is `font` (objects from 6 on are `extra`).
fn page_with_font(font: &str, shown: &str, strict_words: bool, extra: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut content = format!("BT /F3 20 Tf 60 250 Td {shown} Tj ET");
    if strict_words {
        content = format!("BT /F1 20 Tf 20 250 Td (Hi) Tj ET {content}");
    }
    let mut pdf = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 300] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F3 6 0 R >> >> >>")
        .stream("", content)
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .obj(font);
    for (dict, data) in extra {
        pdf = if data.is_empty() { pdf.obj(dict) } else { pdf.stream(dict, data) };
    }
    pdf.build()
}

/// A simple Type 1 font (embedding the vendored Type 1 program as /Type1C) whose `/Differences` from code 1 are `names`, shown with
/// codes 1 to n.
fn names_page(names: &str, base: &str, strict_words: bool) -> Vec<u8> {
    let codes: Vec<u8> = (1..=u8::try_from(names.split_whitespace().count()).unwrap_or(u8::MAX)).collect();
    names_page_showing(names, &codes, base, strict_words)
}

/// [`names_page`] showing `codes`.
fn names_page_showing(names: &str, codes: &[u8], base: &str, strict_words: bool) -> Vec<u8> {
    let codes: String = codes.iter().map(|c| format!("{c:02x}")).collect();
    const CFF: &[u8] = include_bytes!("../../../../vendor/hayro-interpret/assets/FoxitFixed.pfb");
    let n = names.split_whitespace().count();
    let widths = vec!["600"; n].join(" ");
    let font = format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /{base} /FirstChar 1 /LastChar {n} /Widths [{widths}] /FontDescriptor 7 0 R /Encoding << /Type /Encoding /Differences [1 {names}] >> >>"
    );
    let descriptor = format!(
        "<< /Type /FontDescriptor /FontName /{base} /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 8 0 R >>"
    );
    page_with_font(&font, &format!("<{codes}>"), strict_words, &[(&descriptor, Vec::new()), ("/Subtype /Type1C", CFF.to_vec())])
}

/// A Type 0 font (Identity-H) over an embedded TrueType program, without `/ToUnicode`, showing
/// the glyph ids `gids` as CIDs; `to_unicode` adds a `/ToUnicode` CMap.
fn type0_page(program: Vec<u8>, gids: &[u16], base: &str, to_unicode: bool) -> Vec<u8> {
    let shown: String = gids.iter().map(|g| format!("{g:04X}")).collect();
    let tu = if to_unicode { " /ToUnicode 10 0 R" } else { "" };
    let font = format!("<< /Type /Font /Subtype /Type0 /BaseFont /ABCDEF+{base} /Encoding /Identity-H /DescendantFonts [7 0 R]{tu} >>");
    let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /T def 1 begincodespacerange <0000> <FFFF> endcodespacerange 1 beginbfrange <0000> <00FF> <0058> endbfrange endcmap CMapName currentdict /CMap defineresource pop end end".to_vec();
    page_with_font(
        &font,
        &format!("<{shown}>"),
        false,
        &[
            (
                &format!(
                    "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /ABCDEF+{base} /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /CIDToGIDMap /Identity /DW 600 /FontDescriptor 8 0 R >>"
                ),
                Vec::new(),
            ),
            (
                &format!(
                    "<< /Type /FontDescriptor /FontName /ABCDEF+{base} /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 9 0 R >>"
                ),
                Vec::new(),
            ),
            ("", program),
            ("", cmap),
        ],
    )
}

/// A simple TrueType font marked symbolic, without `/Encoding` or `/ToUnicode`, over an
/// embedded program, shown with `codes`.
fn simple_truetype_page(program: Vec<u8>, codes: &[u8]) -> Vec<u8> {
    let shown: String = codes.iter().map(|c| format!("{c:02X}")).collect();
    page_with_font(
        "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+TTE1A2B3C4t00 /FirstChar 1 /LastChar 255 /FontDescriptor 7 0 R >>",
        &format!("<{shown}>"),
        false,
        &[
            (
                "<< /Type /FontDescriptor /FontName /ABCDEF+TTE1A2B3C4t00 /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /MissingWidth 600 /FontFile2 8 0 R >>",
                Vec::new(),
            ),
            ("", program),
        ],
    )
}

/// The glyph ids the standard Macintosh order gives `text`.
fn gids(text: &str) -> Vec<u16> {
    text.chars().map(mac_gid).collect()
}

/// A program of 300 glyphs (a subset that kept the original ids) drawing `text`'s glyphs.
fn kept_ids(text: &str, cmap: Cmap<'_>) -> Vec<u8> {
    subset(300, &gids(text), cmap, None)
}

const SENTENCE: &str = "Besluitenlijst van de vergadering";

/// Rule 1: a `C`, `c` or `MT` glyph name whose decimal number is a Latin-1 character fills the
/// recovered layer whatever code shows it, when every numbered name in the font numbers one;
/// the strict layer still reads nothing (ISO 32000-2 §9.10.2 gives such a name no Unicode
/// value). A font with one numbered name below 32 (`/C3`, a subset counting its glyphs: review
/// on #3239), above 255, in hexadecimal or with another prefix recovers nothing.
#[test]
fn numbered_latin1_names_fill_the_recovered_layer_only() {
    let names = "/C83 /C111 /C99 /C246 /c105 /MT97 /C0108 /C32 /C174";
    let (strict, recovered, plain) = layers(&parse(names_page(names, "ABCDEF+AdvTTSynthetic", false)));
    assert_eq!(strict, plain);
    assert!(strict.glyphs.is_empty());
    let r = recovered.expect("recovered");
    assert_eq!(r.text.plain_text(), "Soc\u{f6}ial \u{ae}");
    assert!(r.recovery.iter().all(|x| x.as_ref().is_some_and(|x| x.rule == RecoveryRule::NumberedLatin1Names && x.mode == RecoveryMode::Fill)));
    assert!((0..r.text.glyphs.len()).all(|i| r.strict_index_of(i).is_none()));
    for odd in ["/C3", "/G5F", "/g3", "/C1111"] {
        let (strict, recovered, _) = layers(&parse(names_page(&format!("/C83 /C111 {odd}"), "ABCDEF+AdvTTSynthetic", false)));
        assert!(strict.glyphs.is_empty() && recovered.is_none(), "{odd}");
    }
    // A symbol font's numbered names are not its characters (a ZapfDingbats `/c110` is a square).
    let (_, recovered, _) = layers(&parse(names_page("/c110 /C83", "ABCDEF+ZapfDingbatsBT", false)));
    assert!(recovered.is_none());
}

/// Rule 1's numbers are codes of the font's own encoding, not always Latin-1. A copy of Symbol
/// named with a `Sym` ending reads them through the Symbol font's built-in encoding (ISO
/// 32000-2 Annex D.5): `/C211` is `©` (copyrightserif), not `Ó`; a font named `…Symbols` still
/// recovers nothing. A further encoding plane of a font
/// (`AdvTT5843c571+20`) recovers nothing: its `/C172` is `€`, not `¬`, while the text font of
/// the same name reads Latin-1. An Advent 3B2 PostScript text font (`AdvPSTim`) recovers its
/// ASCII codes only: its `/C177` is an en dash and `/C174` the `fi` ligature, not `±` and `®`.
#[test]
fn numbered_names_read_through_the_fonts_own_encoding() {
    let read = |names: &str, base: &str| layers(&parse(names_page(names, base, false))).1.map(|r| r.text.plain_text());
    assert_eq!(read("/C211 /C176 /C97", "ODNMDG+AdvPSSym").as_deref(), Some("\u{a9}\u{b0}\u{3b1}"));
    assert_eq!(read("/C211 /C176 /C97", "ABCDEF+GreekMathSymbols"), None);
    assert_eq!(read("/C172", "PLKBAG+AdvTT5843c571+20"), None);
    assert_eq!(read("/C83 /C172", "PLKBAG+AdvTT5843c571+20"), None);
    assert_eq!(read("/C83 /C172", "PLKBAE+AdvTT5843c571").as_deref(), Some("S\u{ac}"));
    assert_eq!(read("/C83 /C111 /C177 /C174", "LBPQCR+AdvPSTim").as_deref(), Some("So"));
}

/// One pass gives the strict layer exactly as the plain extraction reads it, and the recovered
/// layer holds every strict glyph (same text and box, pointing at its strict index) plus the
/// recovered ones.
#[test]
fn one_pass_keeps_the_strict_layer_and_points_into_it() {
    let names = "/C83 /C111 /C99 /C246 /c105 /MT97 /C0108";
    let (strict, recovered, plain) = layers(&parse(names_page(names, "ABCDEF+AdvTTSynthetic", true)));
    assert_eq!(strict, plain);
    assert_eq!(strict.plain_text(), "Hi");
    let r = recovered.expect("recovered");
    assert_eq!(r.text.plain_text(), "Hi Soc\u{f6}ial");
    for (i, g) in r.text.glyphs.iter().enumerate() {
        match r.strict_index_of(i) {
            Some(k) => {
                assert_eq!(strict.glyphs.get(k), Some(g));
                assert!(r.recovery_of(i).is_none());
            }
            None => assert!(r.recovery_of(i).is_some()),
        }
    }
    assert_eq!(r.strict_glyphs(&(0..r.text.glyphs.len()).collect::<Vec<_>>()), vec![0, 1]);
    // The inverse map leads each strict glyph back to its place in the layer.
    for k in 0..strict.glyphs.len() {
        let c = r.layer_index_of(k).expect("every strict glyph is in the layer");
        assert_eq!(r.strict_index_of(c), Some(k));
    }
    assert_eq!(r.layer_index_of(strict.glyphs.len()), None);
    assert!(r.any_recovered(&[0, 1, 2]) && !r.any_recovered(&[0, 1]) && !r.any_recovered(&[usize::MAX]));
}

/// A page nothing is recovered from has no recovered layer, and its strict text is the same.
#[test]
fn without_recovery_there_is_no_recovered_layer() {
    let pdf = parse(page_with_font("<< /Type /Font /Subtype /Type1 /BaseFont /Times-Roman /Encoding /WinAnsiEncoding >>", "(world)", true, &[]));
    let (strict, recovered, plain) = layers(&pdf);
    assert_eq!(strict.plain_text(), "Hi world");
    assert_eq!(strict, plain);
    assert!(recovered.is_none());
}

/// The fill/override policy every rule's reading passes through, with a synthetic rule's
/// readings: a fill never replaces strict text, an override does (with its reason) unless the
/// text is the same, and over a glyph without strict text either is a fill.
#[test]
fn rules_fill_by_default_and_override_only_when_they_say_why() {
    use hayro::hayro_interpret::hayro_cmap::BfString;
    let rule = RecoveryRule::MacGlyphOrder;
    let strict = BfString::Char('a');
    let x = || BfString::Char('X');
    let over = RecoveryMode::Override { reason: "synthetic" };
    assert_eq!(recovery_policy(rule, Some(&strict), x(), RecoveryMode::Fill), None);
    let filled = recovery_policy(rule, None, x(), RecoveryMode::Fill).unwrap();
    assert_eq!((filled.text, filled.mode, filled.rule), (x(), RecoveryMode::Fill, rule));
    let replaced = recovery_policy(rule, Some(&strict), x(), over).unwrap();
    assert_eq!((replaced.text, replaced.mode), (x(), over));
    assert_eq!(recovery_policy(rule, Some(&x()), x(), over), None, "same text, no override");
    assert_eq!(recovery_policy(rule, None, x(), over).unwrap().mode, RecoveryMode::Fill);
}

/// Rule K, glyph ids in the standard Macintosh glyph order: a Type 0 font over an embedded
/// TrueType subset that kept its original glyph ids (300 glyphs, few drawn), with no
/// `/ToUnicode`, `cmap` or glyph names, reads its glyph ids through the standard order (Apple
/// TrueType Reference Manual, `post` table) and the Adobe Glyph List, in the recovered layer
/// only.
#[test]
fn glyph_ids_in_the_mac_order_fill_a_type0_font() {
    let text = "Form 4 Clauses modificatrices";
    let (strict, recovered, plain) = layers(&parse(type0_page(kept_ids(text, Cmap::None), &gids(text), "ArialMT", false)));
    assert_eq!(strict, plain);
    assert!(strict.glyphs.is_empty());
    let r = recovered.expect("recovered");
    assert_eq!(r.text.plain_text(), text);
    assert!(r.recovery.iter().all(|x| x.as_ref().is_some_and(|x| x.rule == RecoveryRule::MacGlyphOrder && x.mode == RecoveryMode::Fill)));
    // Each recovered glyph carries its style as drawn: font, size (20 pt) and colour.
    let style = r.style_of(0).expect("style");
    assert_eq!(style.font.as_deref(), Some("ABCDEF+ArialMT"));
    assert!((style.size - 20.0).abs() < 0.5, "{style:?}");
    assert_eq!(style.color, [0, 0, 0]);
    assert!(r.style.iter().all(Option::is_some));
}

/// Rule K in a simple symbolic TrueType font without `/Encoding`, as Windows printer drivers
/// write (`TTE…t00`, a (3, 0) `cmap` from codes to glyph ids): the strict text of a code is the
/// code read as WinAnsi, a last resort, so the rule overrides it (`A` at 0x41 draws `b`), and
/// fills the codes below 32 the strict text leaves empty. The strict layer is unchanged.
#[test]
fn glyph_ids_in_the_mac_order_override_a_symbolic_truetype_fonts_guess() {
    let text = "Besluitenlijst";
    let mut pairs: Vec<(u8, u16)> = gids(text).into_iter().enumerate().map(|(i, g)| (u8::try_from(i + 1).unwrap(), g)).collect();
    // Code 0x41 draws `b`.
    pairs.push((0x41, mac_gid('b')));
    let mut codes: Vec<u8> = (1..=u8::try_from(text.len()).unwrap()).collect();
    codes.push(0x41);
    let program = subset(300, &gids(&format!("{text}b")), Cmap::Symbolic(&pairs), Some(0x0003_0000));
    let (strict, recovered, plain) = layers(&parse(simple_truetype_page(program, &codes)));
    assert_eq!(strict, plain);
    assert_eq!(strict.plain_text(), "A", "the strict guess stays in the strict layer");
    let r = recovered.expect("recovered");
    assert_eq!(r.text.plain_text(), format!("{text}b"));
    let last = r.text.glyphs.len() - 1;
    let over = r.recovery_of(last).expect("override");
    assert!(matches!(over.mode, RecoveryMode::Override { .. }), "{over:?}");
    assert_eq!(r.strict_index_of(last), Some(0));
    assert_eq!(r.recovery_of(0).map(|x| x.mode), Some(RecoveryMode::Fill));
}

/// Rule K through glyph names that only number the glyph id (`/g39` for glyph 39, `D`): every
/// numbered name must be `g` and a decimal number, and the glyphs must read as text.
#[test]
fn g_names_read_their_glyph_ids_in_the_mac_order() {
    // Each glyph once in the encoding, as converters write it.
    let text = "Disclosure Statement";
    let mut unique: Vec<char> = text.chars().collect();
    unique.sort_unstable();
    unique.dedup();
    let names: Vec<String> = unique.iter().map(|&c| format!("/g{}", mac_gid(c))).collect();
    let codes: Vec<u8> = text.chars().filter_map(|c| unique.iter().position(|&u| u == c)).map(|i| u8::try_from(i + 1).unwrap()).collect();
    let (strict, recovered, _) = layers(&parse(names_page_showing(&names.join(" "), &codes, "ABCDEF+Arial-Black", false)));
    assert!(strict.glyphs.is_empty());
    assert_eq!(recovered.expect("recovered").text.plain_text(), "Disclosure Statement");
    // A symbol font's order is its own; a name of another kind disqualifies the font.
    let (_, recovered, _) = layers(&parse(names_page(&names.join(" "), "ABCDEF+Wingdings-Regular", false)));
    assert!(recovered.is_none());
    let (_, recovered, _) = layers(&parse(names_page(&format!("{} /C70", names.join(" ")), "ABCDEF+Arial-Black", false)));
    assert!(recovered.is_none());
    // Names renumbered from some start (`/g31` … `/g47`, `<` to `L`) give themselves away.
    let block: Vec<String> = (31..48).map(|g| format!("/g{g}")).collect();
    let (_, recovered, _) = layers(&parse(names_page(&block.join(" "), "ABCDEF+Arial", false)));
    assert!(recovered.is_none());
}

/// Fonts rule K must not read, each failing one guard, recover nothing: a subset whose glyphs
/// were renumbered from 1 (class J: dense, a few dozen glyphs), a program that draws most of
/// its 300 glyphs, one whose drawn glyphs read as punctuation and digits, one whose drawn
/// glyphs are one gapless block (renumbered from some start), one whose glyphs lie among the
/// order's accented letters (a symbol font under another name), one with a Unicode
/// `cmap`, one with a `post` table naming glyphs its own way, one with `/ToUnicode` (it is the
/// strict text's business), and one named as a symbol font.
#[test]
fn fonts_without_the_mac_order_evidence_recover_nothing() {
    let shown = gids(SENTENCE);
    let renumbered: Vec<u16> = (1..=20).collect();
    // 134 of 258 glyphs drawn (more than half), reading as text otherwise: plain Latin with a
    // few gaps and some accented letters.
    let mostly_drawn: Vec<u16> = (3..98).filter(|g| ![10, 20, 30].contains(g)).chain(98..140).collect();
    // Punctuation and digits, every other glyph id (not one block).
    let punctuation: Vec<u16> = (4..36).step_by(2).collect();
    // `A` to `Y`: a subset renumbered from 36, with no gap.
    let block: Vec<u16> = (36..61).collect();
    // Glyph ids among the order's accented letters, where a symbol font read in this order
    // lands (`Ä` for a β), every other one.
    let accented: Vec<u16> = (98..160).step_by(2).collect();
    // (case, program, glyph ids shown, base font, with /ToUnicode)
    type Case<'a> = (&'a str, Vec<u8>, Vec<u16>, &'a str, bool);
    let cases: Vec<Case<'_>> = vec![
        ("renumbered", subset(24, &renumbered, Cmap::None, None), renumbered.clone(), "Calibri", false),
        ("mostly drawn", subset(258, &mostly_drawn, Cmap::None, None), shown.clone(), "ArialMT", false),
        ("punctuation", subset(300, &punctuation, Cmap::None, None), punctuation.clone(), "ArialMT", false),
        ("gapless block", subset(300, &block, Cmap::None, None), block.clone(), "ArialMT", false),
        ("accented range", subset(300, &accented, Cmap::None, None), accented.clone(), "TT24Fo00", false),
        ("unicode cmap", kept_ids(SENTENCE, Cmap::Unicode(&[('B', 37)])), shown.clone(), "ArialMT", false),
        ("own glyph names", subset(300, &shown, Cmap::None, Some(0x0002_0000)), shown.clone(), "ArialMT", false),
        ("to unicode", kept_ids(SENTENCE, Cmap::None), shown.clone(), "ArialMT", true),
        ("symbol font", kept_ids(SENTENCE, Cmap::None), shown.clone(), "Wingdings-Regular", false),
    ];
    for (case, program, gids, base, to_unicode) in cases {
        let (strict, recovered, plain) = layers(&parse(type0_page(program, &gids, base, to_unicode)));
        assert_eq!(strict, plain, "{case}");
        assert!(recovered.is_none(), "{case}: {:?}", recovered.map(|r| r.text.plain_text()));
    }
}

/// Finding on the review: the recovery rules run only for a recovered-text request. A text
/// request (Find, search, plain copy, text extraction) reads the strict layer alone, with no
/// recovered layer, and the same strict layer as the recovered-text request of the same page.
#[test]
fn only_a_recovered_text_request_runs_the_recovery_rules() {
    use crate::{PageRenderer, RenderRequest, RequestKind};
    let names = "/C83 /C111 /C99 /C246 /c105 /MT97 /C0108";
    let mut r = PageRenderer::new(std::sync::Arc::new(names_page(names, "ABCDEF+AdvTTSynthetic", true)), Default::default());
    let strict = r.render(RenderRequest { page: 0, kind: RequestKind::Text, ..Default::default() });
    assert!(strict.error.is_none(), "{:?}", strict.error);
    assert!(strict.recovered.is_none(), "a text request read the recovered layer");
    let both = r.render(RenderRequest { page: 0, kind: RequestKind::RecoveredText, ..Default::default() });
    assert_eq!(both.recovered.as_ref().map(|l| l.text.plain_text()).as_deref(), Some("Hi Soc\u{f6}ial"));
    assert_eq!(strict.text, both.text);
    assert_eq!(strict.text.as_ref().map(|t| t.plain_text()).as_deref(), Some("Hi"));
}

/// Review finding: a recovered glyph's size is the em box's height (1000 glyph units, the font
/// size), also in vertical writing (`Identity-V`), where the box's other side is the advance
/// (600 units here), not the size.
#[test]
fn a_vertical_recovered_glyph_is_sized_by_its_em_box() {
    let text = "Articles of Amendment";
    let mut vertical = type0_page(kept_ids(text, Cmap::None), &gids(text), "ArialMT", false);
    let at = vertical.windows(11).position(|w| w == b"/Identity-H").expect("the encoding");
    vertical[at + 10] = b'V';
    let (_, recovered, _) = layers(&parse(vertical));
    let r = recovered.expect("recovered");
    let style = r.style_of(0).expect("style");
    assert!((style.size - 20.0).abs() < 0.5, "{style:?}");
}

/// Review finding: a font's style for recovered text is worked out once per font, not per glyph:
/// every recovered glyph of the font shares one name (the same `Arc`) and the same flags.
#[test]
fn recovered_glyphs_share_their_fonts_style() {
    let text = "Articles of Amendment";
    let (_, recovered, _) = layers(&parse(type0_page(kept_ids(text, Cmap::None), &gids(text), "Arial-BoldMT", false)));
    let r = recovered.expect("recovered");
    let styles: Vec<&GlyphStyle> = r.style.iter().flatten().collect();
    assert!(styles.len() > 10);
    let first = styles[0].font.as_ref().expect("a name");
    assert_eq!(&**first, "ABCDEF+Arial-BoldMT");
    assert!(styles.iter().all(|s| s.font.as_ref().is_some_and(|f| std::sync::Arc::ptr_eq(f, first))), "one name for the font");
}

/// Review finding: a font rule 1 reads, drawn once inside an `/ActualText` span and again
/// unmarked at the same places (Canva's outline copy). The unmarked redraw takes the span's
/// replacement, and a glyph of it that only the recovered layer reads goes with it: the
/// recovered layer reads the word once, not the replacement and the recovered glyph again.
#[test]
fn a_recovered_glyph_inside_an_actual_text_redraw_goes_with_it() {
    const CFF: &[u8] = include_bytes!("../../../../vendor/hayro-interpret/assets/FoxitFixed.pfb");
    let show = "BT /F3 20 Tf 60 250 Td <010203> Tj ET";
    let pdf = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 300] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F3 5 0 R >> >> >>")
        .stream("", format!("/Span << /ActualText (Soc) >> BDC {show} EMC {show}"))
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+AdvTTSynthetic /FirstChar 1 /LastChar 3 /Widths [600 600 600] /FontDescriptor 6 0 R /Encoding << /Type /Encoding /Differences [1 /S /C111 /c] >> >>")
        .obj("<< /Type /FontDescriptor /FontName /ABCDEF+AdvTTSynthetic /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 7 0 R >>")
        .stream("/Subtype /Type1C", CFF)
        .build();
    let (strict, recovered, plain) = layers(&parse(pdf));
    assert_eq!(strict, plain);
    assert_eq!(strict.plain_text(), "Soc");
    // Nothing recovered is left over, so there is no recovered layer at all.
    assert_eq!(recovered.map(|r| r.text.plain_text()), None);
}

/// Review finding: rule K replaces the strict text only in a font the descriptor marks
/// Symbolic (and not Nonsymbolic). The same program and codes in a font marked Nonsymbolic keep
/// their strict text, with nothing overridden.
#[test]
fn a_nonsymbolic_truetype_font_keeps_its_strict_text() {
    // A Macintosh (1, 0) `cmap` from codes `A`, `B`, … to the glyphs, which read as text in the
    // Macintosh order: a nonsymbolic font may use it (ISO 32000-2 §9.6.5.4).
    let text = "Besluitenlijst";
    let pairs: Vec<(u8, u16)> = gids(text).into_iter().enumerate().map(|(i, g)| (u8::try_from(i + 0x41).unwrap(), g)).collect();
    let codes: Vec<u8> = pairs.iter().map(|p| p.0).collect();
    let program = kept_ids(text, Cmap::Mac(&pairs));
    let read = |flags: &[u8]| {
        let mut page = simple_truetype_page(program.clone(), &codes);
        let at = page.windows(8).position(|w| w == b"/Flags 4").expect("the flags");
        page.splice(at..at + 8, flags.iter().copied());
        let (strict, recovered, plain) = layers(&parse(page));
        assert_eq!(strict, plain);
        let overridden = recovered.as_ref().is_some_and(|r| r.recovery.iter().flatten().any(|x| matches!(x.mode, RecoveryMode::Override { .. })));
        (strict.plain_text(), overridden)
    };
    let (symbolic, overridden) = read(b"/Flags 4");
    assert!(overridden, "the symbolic font's guess {symbolic:?} is overridden");
    for flags in [b"/Flags 32".as_slice(), b"/Flags 36", b"/Flags 0"] {
        let (strict, overridden) = read(flags);
        assert!(!overridden, "{}: {strict:?} overridden", String::from_utf8_lossy(flags));
    }
}

/// A one-page PDF in which `content` shows codes of `F3`, a Type 1 font (the vendored CFF
/// program) whose `/Differences` from code 1 are `names`, and `F1`, Helvetica.
fn page_of(names: &str, content: &str) -> Vec<u8> {
    page_of_flags(names, content, 4)
}

/// [`page_of`] with the font descriptor's `/Flags` set to `flags`.
fn page_of_flags(names: &str, content: &str, flags: impl std::fmt::Display) -> Vec<u8> {
    const CFF: &[u8] = include_bytes!("../../../../vendor/hayro-interpret/assets/FoxitFixed.pfb");
    let n = names.split_whitespace().count();
    let widths = vec!["600"; n].join(" ");
    pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R /F3 6 0 R >> >> >>")
        .stream("", content)
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")
        .obj(format!("<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+AdvTTSynthetic /FirstChar 1 /LastChar {n} /Widths [{widths}] /FontDescriptor 7 0 R /Encoding << /Type /Encoding /Differences [1 {names}] >> >>"))
        .obj(format!("<< /Type /FontDescriptor /FontName /ABCDEF+AdvTTSynthetic /Flags {flags} /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 8 0 R >>"))
        .stream("/Subtype /Type1C", CFF)
        .build()
}

/// Review finding: dropping recovered glyphs drawn inside merged `/ActualText` redraws walks
/// the two lists once, however many spans and recovered glyphs a page has (a hostile page has
/// 10^5 of each), and a panic in the recovery rules costs the page only its recovered layer:
/// the strict text is read as a text request reads it.
#[test]
fn recovery_work_is_bounded_and_cant_cost_the_strict_text() {
    let spans = 400;
    let mut content = String::new();
    for k in 0..spans {
        let y = 780 - (k % 70) * 11;
        let x = 20 + (k / 70) * 90;
        let show = format!("BT /F3 10 Tf {x} {y} Td <010203> Tj ET");
        content.push_str(&format!("/Span << /ActualText (Soc) >> BDC {show} EMC {show} "));
    }
    let pdf = parse(page_of("/S /C111 /c", &content));
    REDRAW_WALK_STEPS.with(|c| c.set(0));
    let (strict, recovered, plain) = layers(&pdf);
    assert_eq!(strict, plain);
    assert!(recovered.is_none(), "every recovered glyph was inside a redraw");
    let steps = REDRAW_WALK_STEPS.with(std::cell::Cell::get);
    // Each run end read is counted, whatever the walk: a pairwise test reads 2 × 400 × 400.
    assert!(steps <= 5 * spans, "{steps} reads of run ends for {spans} spans and as many recovered glyphs");

    PANIC_IN_RECOVERY.with(|p| p.set(true));
    let names = parse(names_page("/C83 /C111 /C99", "ABCDEF+AdvTTSynthetic", true));
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| layers(&names)));
    PANIC_IN_RECOVERY.with(|p| p.set(false));
    let (strict, recovered, plain) = read.expect("a panic in the recovery rules stays inside them");
    assert_eq!(strict, plain);
    assert_eq!(strict.plain_text(), "Hi");
    assert!(recovered.is_none());
}

/// Review finding: a text request never calls the recovery rules at all (not only drops what
/// they read); a recovered-text request does.
#[test]
fn a_text_request_never_calls_the_recovery_rules() {
    use crate::{PageRenderer, RenderRequest, RequestKind};
    use hayro::hayro_interpret::font::recovery_rule_calls;
    let mut r = PageRenderer::new(std::sync::Arc::new(names_page("/C83 /C111 /C99", "ABCDEF+AdvTTSynthetic", true)), Default::default());
    let before = recovery_rule_calls();
    let strict = r.render(RenderRequest { page: 0, kind: RequestKind::Text, ..Default::default() });
    assert!(strict.text.is_some());
    assert_eq!(recovery_rule_calls(), before, "a text request asked a recovery rule");
    r.render(RenderRequest { page: 0, kind: RequestKind::RecoveredText, ..Default::default() });
    assert!(recovery_rule_calls() > before);
}

/// Review finding: glyphs only the recovered layer reads, drawn where an invisible strict text
/// layer is drawn later with the same text (rule-1 glyphs under an OCR layer), never cost a
/// strict glyph its place in the recovered layer: the recovered doubles are dropped, so every
/// strict glyph can be selected and copied there.
#[test]
fn a_recovered_double_never_displaces_a_strict_glyph() {
    for word in ["Hi", "Soc"] {
        let names: Vec<String> = word.chars().map(|c| format!("/C{}", u32::from(c))).collect();
        // Each glyph placed on its own, the two layers glyph for glyph at the same places.
        let at = |k: usize| 60 + 12 * k;
        let mut content: String = (0..word.len()).map(|k| format!("BT /F3 20 Tf {} 250 Td <{:02x}> Tj ET ", at(k), k + 1)).collect();
        content.extend(word.chars().enumerate().map(|(k, c)| format!("BT 3 Tr /F1 20 Tf {} 250 Td ({c}) Tj ET ", at(k))));
        let (strict, recovered, plain) = layers(&parse(page_of(&names.join(" "), &content)));
        assert_eq!(strict, plain);
        assert_eq!(strict.plain_text(), word);
        let r = recovered.expect("recovered");
        for k in 0..strict.glyphs.len() {
            assert!(r.layer_index_of(k).is_some(), "{word}: strict glyph {k} has no place in {:?}", r.text.plain_text());
        }
        assert_eq!(r.text.plain_text(), word, "the recovered doubles are dropped");
    }
}

/// Gate review: a symbolic TrueType font whose codes are WinAnsi codes (a Windows driver's
/// font: `A` at 0x41 draws the Macintosh order's `A`) has strict text that reads right; rule K
/// leaves it, even where one glyph's place in the Macintosh order says otherwise (a curly
/// quote, 000016.pdf, drawn as `’` while the order reads `‘`). Only a font whose codes
/// mostly don't read as WinAnsi (a subset counting 1, 2, 3 …) has its strict text overridden.
#[test]
fn a_font_whose_codes_read_as_winansi_keeps_its_strict_text() {
    let text = "Besluitenlijst";
    let mut pairs: Vec<(u8, u16)> = text.chars().map(|c| (u8::try_from(u32::from(c)).unwrap(), mac_gid(c))).collect();
    // Code 0x92, WinAnsi's right quote, draws the glyph the Macintosh order calls `b`.
    pairs.push((0x92, mac_gid('b')));
    let codes: Vec<u8> = pairs.iter().map(|p| p.0).collect();
    let program = subset(300, &gids(&format!("{text}b")), Cmap::Symbolic(&pairs), Some(0x0003_0000));
    let (strict, recovered, plain) = layers(&parse(simple_truetype_page(program, &codes)));
    assert_eq!(strict, plain);
    assert_eq!(strict.plain_text(), format!("{text}\u{2019}"));
    let overridden = recovered.as_ref().is_some_and(|r| r.recovery.iter().flatten().any(|x| matches!(x.mode, RecoveryMode::Override { .. })));
    assert!(!overridden, "{:?}", recovered.map(|r| r.text.plain_text()));
}

/// Review finding: a word longer than the fake-bold window, mixing strict glyphs and glyphs
/// rule 1 reads, drawn twice at the same place (fake bold). The recovered layer keeps every
/// strict glyph the strict layer keeps, at its place, and reads the word once, every character
/// of it.
#[test]
fn a_mixed_word_drawn_twice_keeps_its_strict_glyphs_and_reads_once() {
    let show = "BT /F3 20 Tf 60 250 Td <010203040506> Tj ET";
    let (strict, recovered, plain) = layers(&parse(page_of("/S /C111 /c /C105 /a /l", &format!("{show} {show}"))));
    assert_eq!(strict, plain);
    assert_eq!(strict.plain_text(), "Scal");
    let r = recovered.expect("recovered");
    for k in 0..strict.glyphs.len() {
        assert!(r.layer_index_of(k).is_some(), "strict glyph {k} has no place in {:?}", r.text.plain_text());
    }
    assert_eq!(r.text.plain_text(), "Social");
}

/// Found by the property tests: a strict glyph drawn twice on one line (fake bold) and a word
/// below mixing strict and recovered glyphs of the same letter. Every strict glyph keeps its
/// place in the recovered layer.
#[test]
fn fake_bold_above_a_mixed_word_keeps_every_strict_glyph() {
    let line1 = "BT /F3 12 Tf 20 760 Td <01> Tj ET";
    let content = format!("{line1} {line1} BT /F3 12 Tf 20 736 Td <01010201> Tj ET");
    let (strict, recovered, plain) = layers(&parse(page_of("/a /C97", &content)));
    assert_eq!(strict, plain);
    let r = recovered.expect("recovered");
    for k in 0..strict.glyphs.len() {
        assert!(r.layer_index_of(k).is_some(), "strict glyph {k} of {:?} has no place in {:?}", strict.plain_text(), r.text.plain_text());
    }
}

/// Review finding: many strict glyphs stacked at one spot, cycling through more letters than
/// the fake-bold window (so the strict layout keeps them all), then as many rule-1 glyphs at the
/// same spot. Dropping the recovered doubles stays within its work budget.
#[test]
fn dropping_recovered_doubles_stays_within_its_budget() {
    let n = 3000;
    let codes: String = (0..n).map(|k| format!("{:02x}", 1 + k % 5)).chain((0..n).map(|k| format!("{:02x}", 6 + k % 5))).collect();
    const CFF: &[u8] = include_bytes!("../../../../vendor/hayro-interpret/assets/FoxitFixed.pfb");
    let pdf = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F3 5 0 R >> >> >>")
        .stream("", format!("BT /F3 12 Tf 60 400 Td <{codes}> Tj ET"))
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+AdvTTSynthetic /FirstChar 1 /LastChar 10 /Widths [0 0 0 0 0 0 0 0 0 0] /FontDescriptor 6 0 R /Encoding << /Type /Encoding /Differences [1 /a /b /c /d /e /C102 /C103 /C104 /C105 /C106] >> >>")
        .obj("<< /Type /FontDescriptor /FontName /ABCDEF+AdvTTSynthetic /Flags 32 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 7 0 R >>")
        .stream("/Subtype /Type1C", CFF)
        .build();
    DOUBLES_WORK.with(|c| c.set(0));
    let (strict, _, plain) = layers(&parse(pdf));
    assert_eq!(strict, plain);
    let work = DOUBLES_WORK.with(std::cell::Cell::get);
    assert!(work <= MAX_REDRAW_WORK, "{work} steps");
}

/// Review finding: the cells around a coordinate at the ends of the grid (where `cell`
/// saturates to `i64::MIN` or `i64::MAX`) are found without stepping past them. (A page keeps
/// its glyphs within its box, so text doesn't reach there today; this guards the arithmetic.)
#[test]
fn cells_around_the_grids_ends_stay_in_range() {
    assert_eq!(device::cell_span(f32::MAX, 3.0), (i64::MAX, i64::MAX));
    assert_eq!(device::cell_span(f32::MIN, 3.0), (i64::MIN, i64::MIN));
    // 13 / 8 and 19 / 8, floored.
    assert_eq!(device::cell_span(16.0, 3.0), (1, 2));
    assert_eq!(device::cell_span(-1.0, 0.5), (-1, -1));
}

/// Review finding: a 72 pt word mixing strict and rule-1 glyphs with a shadow 12 pt off (within
/// a fifth of its height, two grid cells away) reads its recovered glyph once.
#[test]
fn a_big_word_with_a_shadow_reads_its_recovered_glyphs_once() {
    // Longer than the fake-bold window, so only the recovered layer's own dedupe sees it.
    let content = "BT /F3 72 Tf 60 500 Td <010203040506> Tj ET BT /F3 72 Tf 72 488 Td <010203040506> Tj ET";
    let (strict, recovered, plain) = layers(&parse(page_of("/S /C111 /c /C105 /a /l", content)));
    assert_eq!(strict, plain);
    assert_eq!(strict.plain_text(), "Scal");
    assert_eq!(recovered.map(|r| r.text.plain_text()).as_deref(), Some("Social"));
}

/// Review finding: `/Flags` with a bit ISO 32000-2 leaves undefined (here 16, beside ForceBold:
/// 262160) keeps its defined bits, as the editor reads them: a recovered glyph outside any
/// block is bold, as the strict text of the font is.
#[test]
fn undefined_flag_bits_keep_force_bold() {
    let (_, recovered, _) = layers(&parse(page_of_flags("/C83 /C111 /C99", "BT /F3 20 Tf 60 250 Td <010203> Tj ET", 262_160)));
    let r = recovered.expect("recovered");
    assert!(r.style_of(0).is_some_and(|s| s.bold), "{:?}", r.style_of(0));
}

/// Review finding: the page's dedupe budget is one, shared by the redraw dedupe and then the
/// recovered layer's: when the redraws spend it all, dropping recovered doubles does no work.
#[test]
fn the_redraw_dedupe_and_drop_doubles_share_one_budget() {
    let mut content = String::new();
    for k in 0..40 {
        let show = format!("BT /F3 10 Tf {} {} Td <010203040506> Tj ET", 20 + (k / 30) * 120, 780 - (k % 30) * 11);
        content.push_str(&format!("/Span << /ActualText (Social) >> BDC {show} EMC {show} "));
    }
    // A mixed word and its shadow, whose recovered glyphs `drop_doubles` would compare.
    content.push_str("BT /F3 72 Tf 60 300 Td <010203040506> Tj ET BT /F3 72 Tf 72 288 Td <010203040506> Tj ET");
    let pdf = parse(page_of("/S /C111 /c /C105 /a /l", &content));
    PAGE_WORK.with(|w| w.set(Some(50)));
    DOUBLES_WORK.with(|c| c.set(0));
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| layers(&pdf)));
    PAGE_WORK.with(|w| w.set(None));
    let (strict, _, plain) = read.expect("no panic");
    assert_eq!(strict, plain);
    assert_eq!(DOUBLES_WORK.with(std::cell::Cell::get), 0, "the redraws spent the page's budget");
    // With the full budget the same page does compare.
    DOUBLES_WORK.with(|c| c.set(0));
    let _ = layers(&pdf);
    assert!(DOUBLES_WORK.with(std::cell::Cell::get) > 0);
}
