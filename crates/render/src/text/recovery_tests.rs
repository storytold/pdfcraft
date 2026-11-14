//! Copy with formatting's recovered layer (hayro-interpret patch 107): one interpretation reads
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

/// Review on #6733: when the strict part of a recovered-text request has used half the render
/// pool's watchdog, the page keeps its strict layer and has no recovered one. A failed
/// recovered-text request fails the page's text, so the recovered layout must never be what
/// pushes a page past the watchdog.
#[test]
fn a_page_slow_to_read_keeps_its_strict_text_and_skips_the_recovered_layout() {
    assert!(RECOVERY_DEADLINE.saturating_mul(2) <= crate::raster::STUCK_AFTER);
    let pdf = parse(names_page("/C83 /C111 /C99", "ABCDEF+AdvTTSynthetic", true));
    let (_, recovered, _) = layers(&pdf);
    assert!(recovered.is_some(), "a quick page has its recovered layer");
    let millis = u64::try_from(RECOVERY_DEADLINE.as_millis()).unwrap_or(u64::MAX);
    STRICT_MILLIS.with(|m| m.set(Some(millis)));
    let _reset = Reset(|| STRICT_MILLIS.with(|m| m.set(None)));
    let (strict, recovered, plain) = layers(&pdf);
    assert_eq!(strict, plain);
    assert_eq!(strict.plain_text(), "Hi");
    assert!(recovered.is_none(), "past the deadline the recovered layout is skipped");
}

/// Runs its function when dropped, also while a failing test unwinds: resets a thread-local
/// override so it can't leak into the next test on the thread.
struct Reset(fn());

impl Drop for Reset {
    fn drop(&mut self) {
        (self.0)();
    }
}

/// [`Reset`] runs while a test unwinds.
#[test]
fn reset_runs_while_unwinding() {
    let unwound = std::panic::catch_unwind(|| {
        STRICT_MILLIS.with(|m| m.set(Some(1)));
        let _reset = Reset(|| STRICT_MILLIS.with(|m| m.set(None)));
        panic!("a failing test");
    });
    assert!(unwound.is_err());
    assert_eq!(STRICT_MILLIS.with(std::cell::Cell::get), None);
}

/// Review on #6733: a recovered layout still running when three quarters of the render pool's
/// watchdog have passed since the page's text extraction started gives up and is discarded;
/// the page keeps its strict layer. The strict layout never runs under that deadline, so it is
/// the same whether or not the recovered one gave up.
#[test]
fn a_recovered_layout_out_of_time_is_discarded_and_the_strict_layer_kept() {
    use super::device::RECOVERY_LAYOUT_END;
    assert!(RECOVERY_DEADLINE < RECOVERY_LAYOUT_END && RECOVERY_LAYOUT_END < crate::raster::STUCK_AFTER);
    let pdf = parse(names_page("/C83 /C111 /C99", "ABCDEF+AdvTTSynthetic", true));
    let (before, recovered, _) = layers(&pdf);
    assert!(recovered.is_some(), "in time, the page has its recovered layer");
    super::layout::GIVE_UP_NOW.with(|g| g.set(true));
    let _reset = Reset(|| super::layout::GIVE_UP_NOW.with(|g| g.set(false)));
    let (strict, recovered, plain) = layers(&pdf);
    assert_eq!(strict, before, "the strict layer is the same");
    assert_eq!(strict, plain);
    assert!(recovered.is_none(), "a recovered layout out of time is discarded");
}

/// Glyph-order drift: fonts made for Windows keep the standard Macintosh order only so far
/// (past about glyph 171 a glyph is often its neighbour's: Times New Roman's right quote where
/// the order has the left one), so rule K reads glyph ids up to 171 and nothing past them.
/// `Straße – it’s`: `ß` is glyph 137 and reads; the en dash (178) and the right quote (183) don't.
#[test]
fn the_macintosh_order_is_read_up_to_glyph_171_only() {
    let mut ids = gids("Stra");
    ids.push(137);
    ids.extend(gids("e "));
    ids.push(178);
    ids.extend(gids(" it"));
    ids.push(183);
    ids.extend(gids("s and some more words"));
    let page = type0_page(subset(300, &ids, Cmap::None, None), &ids, "TimesNewRomanPSMT", false);
    let (_, recovered, _) = layers(&parse(page));
    let text = recovered.map(|r| r.text.plain_text()).unwrap_or_default();
    assert!(text.contains("Stra\u{df}e") && text.contains("some more words"), "{text:?}");
    assert!(!text.contains('\u{2013}') && !text.contains('\u{2019}') && !text.contains('\u{201c}'), "{text:?}");
}

/// Glyph-order drift: an accented letter is as wide as its base letter in a Latin typeface, so a
/// font whose glyphs read as accented letters in the Macintosh order have other widths than the
/// glyphs read as their base letters doesn't follow that order (an Apple font such as Skia
/// numbers its glyphs from further on: its glyph 104, the order's `Udieresis`, is an `h`) and
/// reads nothing. With the widths of their `e` the accents read, `ï` uncompared (accented `i`
/// letters are often wider than the `i`).
#[test]
fn accented_letters_wider_than_their_base_letters_reject_the_font() {
    let mut ids = gids("caf");
    ids.push(112);
    ids.extend(gids(" cr"));
    ids.push(113);
    ids.extend(gids("me na"));
    ids.push(119);
    ids.extend(gids("ve plus"));
    let program = |accented: u16| {
        let width = move |gid: u16| if gid == 112 || gid == 113 { accented } else { 500 };
        pdfcraft_testkit::truetype::subset_with_widths(300, &ids, Cmap::None, None, Some(&width))
    };
    let read = |accented: u16| layers(&parse(type0_page(program(accented), &ids, "ArialMT", false))).1.map(|r| r.text.plain_text());
    let same = read(500);
    assert!(same.as_deref().is_some_and(|t| t.contains("caf\u{e9}") && t.contains("cr\u{e8}me") && t.contains("na\u{ef}ve")), "{same:?}");
    assert_eq!(read(800), None, "accents wider than their `e`: not the Macintosh order");
}

/// The two thresholds of the drift checks, at every value around them: a glyph id beside a text
/// reads one more character exactly when it is 171 or less (172, a no-break space, is blank) and
/// not one of the ids where some Windows fonts differ from the order (≠, π, Ω, ∆), and an accent
/// reads exactly when its width is within 1% of its base letter's (500).
#[test]
fn the_drift_thresholds_hold_at_every_value_around_them() {
    let letters = gids("some more words here");
    let visible = |ids: &[u16], program: Vec<u8>| {
        layers(&parse(type0_page(program, ids, "ArialMT", false))).1.map_or(0, |r| r.text.plain_text().chars().filter(|c| !c.is_whitespace()).count())
    };
    let base = visible(&letters, subset(300, &letters, Cmap::None, None));
    assert_eq!(base, 17);
    for id in (140..=185).filter(|id| *id != 172) {
        let mut ids = letters.clone();
        ids.push(id);
        let read = visible(&ids, subset(300, &ids, Cmap::None, None));
        assert_eq!(read, base + usize::from(id <= 171 && ![143, 155, 159, 168].contains(&id)), "glyph {id}");
    }
    let mut ids = letters.clone();
    ids.push(112);
    for accented in 490..=510u16 {
        let width = move |gid: u16| if gid == 112 { accented } else { 500 };
        let read = visible(&ids, pdfcraft_testkit::truetype::subset_with_widths(300, &ids, Cmap::None, None, Some(&width)));
        let within = f32::from(accented.abs_diff(500)) <= f32::from(accented.max(500)) * 0.01;
        assert_eq!(read, if within { base + 1 } else { 0 }, "an accent {accented} wide");
    }
}

/// A CFF INDEX of `items` (offsets one byte wide when they fit, else two).
fn cff_index(items: &[Vec<u8>]) -> Vec<u8> {
    let count = u16::try_from(items.len()).unwrap();
    let mut out = count.to_be_bytes().to_vec();
    if items.is_empty() {
        return out;
    }
    let total: usize = items.iter().map(Vec::len).sum::<usize>() + 1;
    let wide = total > 255;
    out.push(if wide { 2 } else { 1 });
    let mut at = 1usize;
    let mut offsets = vec![at];
    for item in items {
        at += item.len();
        offsets.push(at);
    }
    for o in offsets {
        if wide {
            out.extend_from_slice(&u16::try_from(o).unwrap().to_be_bytes());
        } else {
            out.push(u8::try_from(o).unwrap());
        }
    }
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

/// A bare CFF program whose glyphs 1, 2, 3 … are named `names` (custom strings) and drawn by
/// codes 1, 2, 3 … of its built-in encoding (format 0); every glyph is empty.
pub(super) fn cff_with_names(names: &[&str]) -> Vec<u8> {
    // Operands as five-byte integers, so the offsets don't change the Top DICT's size.
    let int = |v: usize| {
        let mut b = vec![29];
        b.extend_from_slice(&i32::try_from(v).unwrap().to_be_bytes());
        b
    };
    let n = names.len();
    let strings = cff_index(&names.iter().map(|s| s.as_bytes().to_vec()).collect::<Vec<_>>());
    let header = vec![1, 0, 4, 1];
    let name = cff_index(&[b"Synthetic".to_vec()]);
    let global_subrs = cff_index(&[]);
    let encoding: Vec<u8> = [vec![0, u8::try_from(n).unwrap()], (1..=n).map(|c| u8::try_from(c).unwrap()).collect()].concat();
    let charset: Vec<u8> = std::iter::once(0u8).chain((0..n).flat_map(|k| u16::try_from(391 + k).unwrap().to_be_bytes())).collect();
    let charstrings = cff_index(&vec![vec![14u8]; n + 1]);
    let private = vec![139, 20];
    let top_len = |dict: &[u8]| cff_index(&[dict.to_vec()]).len();
    let top = |charset_at: usize, encoding_at: usize, charstrings_at: usize, private_at: usize| {
        [int(charset_at), vec![15], int(encoding_at), vec![16], int(charstrings_at), vec![17], int(private.len()), int(private_at), vec![18]].concat()
    };
    let start = header.len() + name.len() + top_len(&top(0, 0, 0, 0)) + strings.len() + global_subrs.len();
    let (encoding_at, charset_at) = (start, start + encoding.len());
    let charstrings_at = charset_at + charset.len();
    let private_at = charstrings_at + charstrings.len();
    let top_index = cff_index(&[top(charset_at, encoding_at, charstrings_at, private_at)]);
    [header, name, top_index, strings, global_subrs, encoding, charset, charstrings, private].concat()
}

/// Review on #6733: rule 1 reads the glyph names of a CFF program's own charset when the font
/// has no `/Encoding` (its built-in encoding draws codes 1, 2, 3 as `C83`, `C111`, `C99`), as
/// it reads `/Differences`; the strict layer still reads nothing.
#[test]
fn numbered_names_of_a_cff_programs_own_charset_fill_the_recovered_layer() {
    let page = page_with_font(
        "<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+AdvTTSynthetic /FirstChar 1 /LastChar 3 /Widths [600 600 600] /FontDescriptor 7 0 R >>",
        "<010203>",
        false,
        &[
            (
                "<< /Type /FontDescriptor /FontName /ABCDEF+AdvTTSynthetic /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 8 0 R >>",
                Vec::new(),
            ),
            ("/Subtype /Type1C", cff_with_names(&["C83", "C111", "C99"])),
        ],
    );
    let (strict, recovered, _) = layers(&parse(page));
    assert!(strict.glyphs.is_empty(), "{:?}", strict.plain_text());
    assert_eq!(recovered.map(|r| r.text.plain_text()).as_deref(), Some("Soc"));
}

/// Review on #6733: a font of an Adobe character collection names its characters by CID (its
/// UCS2 CMap), so its glyph ids are never read in the Macintosh order, even for CIDs the map
/// lacks (here past Adobe-GB1's last CID); the same font as Adobe-Identity is read.
#[test]
fn a_font_of_an_adobe_collection_is_not_read_in_the_macintosh_order() {
    let text = "Articles of Amendment";
    let g = gids(text);
    let first = 40_000usize;
    let mut map = vec![0u8; (first + g.len()) * 2];
    for (k, gid) in g.iter().enumerate() {
        let at = (first + k) * 2;
        map[at..at + 2].copy_from_slice(&gid.to_be_bytes());
    }
    let shown: String = (0..g.len()).map(|k| format!("{:04X}", first + k)).collect();
    let page = |ordering: &str| {
        page_with_font(
            "<< /Type /Font /Subtype /Type0 /BaseFont /ABCDEF+ArialMT /Encoding /Identity-H /DescendantFonts [7 0 R] >>",
            &format!("<{shown}>"),
            false,
            &[
                (
                    &format!(
                        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /ABCDEF+ArialMT /CIDSystemInfo << /Registry (Adobe) /Ordering ({ordering}) /Supplement 0 >> /CIDToGIDMap 10 0 R /DW 600 /FontDescriptor 8 0 R >>"
                    ),
                    Vec::new(),
                ),
                (
                    "<< /Type /FontDescriptor /FontName /ABCDEF+ArialMT /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 9 0 R >>",
                    Vec::new(),
                ),
                ("", kept_ids(text, Cmap::None)),
                ("", map.clone()),
            ],
        )
    };
    let (_, identity, _) = layers(&parse(page("Identity")));
    assert_eq!(identity.map(|r| r.text.plain_text()).as_deref(), Some(text), "the fixture reads as Adobe-Identity");
    let (strict, gb1, _) = layers(&parse(page("GB1")));
    assert!(strict.glyphs.is_empty(), "{:?}", strict.plain_text());
    assert!(gb1.is_none(), "{:?}", gb1.map(|r| r.text.plain_text()));
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

/// The candidate policy, through `recovery_policy`: whatever a rule reads, a private use
/// character (U+E000–U+F8FF, planes 15 and 16), a C0 or C1 control, a noncharacter or nothing at
/// all is never recovered, alone or inside a string.
#[test]
fn the_candidate_policy_refuses_what_is_not_text() {
    use hayro::hayro_interpret::hayro_cmap::BfString;
    let read = |text: BfString| recovery_policy(RecoveryRule::MacGlyphOrder, None, text, RecoveryMode::Fill);
    for c in [
        '\u{E000}',
        '\u{F8FF}',
        '\u{F0041}',
        '\u{10FFFD}',
        '\u{0}',
        '\u{1F}',
        '\u{7F}',
        '\u{80}',
        '\u{9F}',
        '\u{FDD0}',
        '\u{FDEF}',
        '\u{FFFE}',
        '\u{FFFF}',
        '\u{1FFFE}',
    ] {
        assert_eq!(read(BfString::Char(c)), None, "{c:?}");
        assert_eq!(read(BfString::String(format!("a{c}"))), None, "a{c:?}");
    }
    assert_eq!(read(BfString::String(String::new())), None);
    for c in ['A', ' ', '\u{A0}', '\u{AD}', '\u{2019}', '\u{D7FF}', '\u{FDCF}', '\u{FFFD}', '\u{1F600}'] {
        assert!(read(BfString::Char(c)).is_some(), "{c:?}");
    }
    assert!(read(BfString::String("fi".into())).is_some());
    // An override is refused the same way, and replaces strict text only with other text.
    let strict = BfString::Char('%');
    let over = RecoveryMode::Override { reason: "test" };
    assert_eq!(recovery_policy(RecoveryRule::MacGlyphOrder, Some(&strict), BfString::Char('\u{E000}'), over), None);
    assert!(recovery_policy(RecoveryRule::MacGlyphOrder, Some(&strict), BfString::Char('D'), over).is_some());
}

/// The candidate policy's choice among several names for one glyph: the acceptable ones, one
/// distinct character, or a single ASCII one among several; nothing when it stays ambiguous,
/// whatever order the names come in.
#[test]
fn the_candidate_choice_is_deterministic() {
    use hayro::hayro_interpret::font::test_hooks::choose;
    assert_eq!(choose(&['a']), Some('a'));
    assert_eq!(choose(&['a', 'a']), Some('a'));
    assert_eq!(choose(&['-', '\u{2010}']), Some('-'));
    assert_eq!(choose(&['\u{2010}', '-']), Some('-'));
    assert_eq!(choose(&['\u{A0}', ' ']), Some(' '));
    assert_eq!(choose(&['a', 'b']), None);
    assert_eq!(choose(&['\u{2010}', '\u{2011}']), None);
    assert_eq!(choose(&['\u{E000}', 'x']), Some('x'));
    assert_eq!(choose(&['\u{E000}']), None);
    assert_eq!(choose(&[]), None);
    assert_eq!(choose(&['x'; 17]), None, "a list longer than any glyph's names is no evidence");
}

/// The standard orders, in one place: the Macintosh order as Apple's TrueType Reference Manual
/// lists it (`space` 3, `A` 36, `a` 68, `quoteleft` 182), with nothing for `.notdef`, `.null`,
/// the private use `apple` (210) or past 257; and the CFF standard strings as Adobe Technical
/// Note #5176 Appendix A lists them (`space` 1, `A` 34, `a` 66, `quoteright` 8, `quoteleft` 65,
/// `bullet` 116), with nothing for `.notdef`, the expert set's private use small capitals
/// (`Asmall`, 229 + …) or past 390.
#[test]
fn the_standard_orders_read_as_their_specifications_list_them() {
    use hayro::hayro_interpret::font::test_hooks::{mac_glyph_char, standard_string_char};
    for (gid, c) in [(3, Some(' ')), (36, Some('A')), (68, Some('a')), (98, Some('\u{C4}')), (182, Some('\u{2018}')), (183, Some('\u{2019}'))] {
        assert_eq!(mac_glyph_char(gid), c, "glyph {gid}");
    }
    for gid in [0, 1, 210, 258, u32::MAX] {
        assert_eq!(mac_glyph_char(gid), None, "glyph {gid}");
    }
    for (sid, c) in [(1, ' '), (34, 'A'), (66, 'a'), (8, '\u{2019}'), (65, '\u{2018}'), (116, '\u{2022}')] {
        assert_eq!(standard_string_char(sid), Some(c), "SID {sid}");
    }
    for c in ('!'..='~').filter(|c| *c != '\'' && *c != '`') {
        assert_eq!(standard_string_char(u32::from(pdfcraft_testkit::cff::standard_string_id(c))), Some(c), "{c}");
    }
    for sid in [0, 391, u32::MAX] {
        assert_eq!(standard_string_char(sid), None, "SID {sid}");
    }
    let small_caps = (229..=390).filter_map(standard_string_char).filter(|c| matches!(u32::from(*c), 0xE000..=0xF8FF)).count();
    assert_eq!(small_caps, 0, "private use small capitals read as nothing");
}

/// The accent check every rule reading the Macintosh order shares: an accented letter as wide
/// as its base letter (to 1%) agrees, a wider one doesn't, a width the font doesn't give
/// decides nothing, and the accented `i` letters are left out.
#[test]
fn the_shared_accent_check_compares_accents_with_their_base_letters() {
    use hayro::hayro_interpret::font::test_hooks::accents_as_wide_as_bases;
    let width = |eacute: f32, iacute: f32| {
        move |gid: u16| match gid {
            112 => Some(eacute),
            72 => Some(556.0),
            118 => Some(iacute),
            76 => Some(222.0),
            _ => None,
        }
    };
    assert!(accents_as_wide_as_bases(&[72, 112], &width(556.0, 278.0)));
    assert!(accents_as_wide_as_bases(&[72, 112], &width(561.0, 278.0)), "within 1%");
    assert!(!accents_as_wide_as_bases(&[72, 112], &width(600.0, 278.0)));
    assert!(accents_as_wide_as_bases(&[76, 118], &width(556.0, 278.0)), "the accented i is left out");
    assert!(accents_as_wide_as_bases(&[112], &|_| None), "no widths, no evidence against");
}

/// The width evidence's characters for `text` read `shift` places along printable ASCII from
/// what each glyph is (a glyph whose reading leaves the range reads nothing), each with the
/// Helvetica width of the letter the glyph really is.
fn shifted_widths(text: &str, shift: i32) -> Vec<(char, f32)> {
    let mut out: Vec<(char, f32)> = Vec::new();
    for c in text.chars() {
        let read = u32::try_from(i64::from(u32::from(c)) + i64::from(shift)).ok().and_then(char::from_u32).filter(|r| ('!'..='~').contains(r));
        if let Some(r) = read
            && !out.iter().any(|(k, _)| *k == r)
        {
            out.push((r, pdfcraft_testkit::cff::helvetica_width(c) as f32));
        }
    }
    out
}

/// A small deterministic generator for the width properties.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        usize::try_from((self.0 >> 33) % u64::try_from(n).unwrap()).unwrap()
    }
}

/// Review on the standard-order rules (H1, M3): the width evidence never proves a reading off
/// by a glyph or more, nor one in capitals only. A subset of capitals (a form's labels, a name)
/// read a glyph or more off gives `J` as `I` and every other capital wider, which held three
/// comparisons before; texts are generated in capitals only, as small subsets of two to six
/// letters, and as mixed sentences, and read with shifts of one to three places, 29 to 33 (a
/// numbering by ASCII or Unicode against one by standard string ids) and between the cases.
#[test]
fn the_width_evidence_never_proves_a_shifted_or_one_case_reading() {
    use hayro::hayro_interpret::font::test_hooks::widths_prove;
    // The review's own counter-examples, and the readings this test's generator found proving
    // one condition of the proof at a time.
    for (text, shift) in [
        ("JACQUES DUPRE", -1),
        ("PROJECT QUEUE", -1),
        ("SUBJECT", -1),
        ("SUBJECT MATTER", 1),
        ("PROJECT QUEUE ADMIN", 1),
        ("JACQUES DUPRE", 3),
        ("of and Residence", 1),
        ("of and Residence", 3),
        ("nil indicated", -3),
        ("isbbddls", -3),
        ("nil address", -3),
        ("nil Residence ZIP", -3),
        ("WGKVCIQJ", 32),
        ("Z PB MKYX CKDNICKEEH", 32),
        ("IIW KIOWGWG    FQ T ", 32),
    ] {
        assert!(!widths_prove(&shifted_widths(text, shift)), "{text} shifted {shift}");
    }
    let words = [
        "minimal",
        "endorsement",
        "within",
        "the",
        "human",
        "body",
        "annual",
        "report",
        "Corporate",
        "name",
        "address",
        "Residence",
        "and",
        "ZIP",
        "code",
        "are",
        "indicated",
        "Willem",
        "Jansen",
        "Quarterly",
        "Summary",
        "of",
        "Accounts",
        "Payable",
        "milk",
        "lamb",
        "mill",
        "nil",
        "limb",
    ];
    let shifts = [-33, -32, -31, -29, -3, -2, -1, 1, 2, 3, 29, 31, 32, 33];
    let mut rng = Lcg(6733);
    for case in 0..3000 {
        let text: String = match case % 3 {
            // Capitals only, as a form's labels.
            0 => (0..3 + rng.next(18)).map(|_| if rng.next(6) == 0 { ' ' } else { char::from(b'A' + u8::try_from(rng.next(26)).unwrap()) }).collect(),
            // A small subset: two to six distinct letters, either case.
            1 => {
                let base = if rng.next(2) == 0 { b'a' } else { b'A' };
                let letters: Vec<char> = (0..2 + rng.next(5)).map(|_| char::from(base + u8::try_from(rng.next(26)).unwrap())).collect();
                (0..8).map(|_| letters[rng.next(letters.len())]).collect()
            }
            // A sentence in mixed case.
            _ => (0..2 + rng.next(6)).map(|_| words[rng.next(words.len())]).collect::<Vec<_>>().join(" "),
        };
        for shift in shifts {
            assert!(!widths_prove(&shifted_widths(&text, shift)), "{text:?} shifted {shift}");
        }
        if case % 3 == 0 {
            assert!(!widths_prove(&shifted_widths(&text, 0)), "capitals only prove nothing: {text:?}");
        }
    }
    // Read right, sentences with lowercase letters of different widths prove their reading, in
    // Helvetica's widths and in Times's (its AFM: rounder letters closer to the narrow ones).
    let times = |c: char| -> f32 {
        const LOWER: [u16; 26] =
            [444, 500, 444, 500, 444, 333, 500, 500, 278, 278, 500, 278, 778, 500, 500, 500, 500, 333, 389, 278, 500, 500, 722, 500, 500, 444];
        const UPPER: [u16; 26] =
            [722, 667, 667, 722, 611, 556, 722, 722, 333, 389, 722, 611, 889, 722, 722, 556, 722, 667, 556, 611, 722, 722, 944, 722, 722, 611];
        f32::from(match c {
            'a'..='z' => LOWER[(c as usize) - ('a' as usize)],
            'A'..='Z' => UPPER[(c as usize) - ('A' as usize)],
            _ => 250,
        })
    };
    for text in ["minimal endorsement within the human body", "Quarterly Summary of Accounts Payable in limbo", "the annual report, filed in July"] {
        assert!(widths_prove(&shifted_widths(text, 0)), "{text}");
        let mut in_times: Vec<(char, f32)> = Vec::new();
        for c in text.chars().filter(|c| *c != ' ') {
            if !in_times.iter().any(|(k, _)| *k == c) {
                in_times.push((c, times(c)));
            }
        }
        assert!(widths_prove(&in_times), "{text} in Times");
    }
}

/// Each condition of the width proof on its own, with a reading built so that only it fails:
/// widths that agree with every comparison made, but a reading with no comparison between two
/// lowercase letters (capitals against `i` and `l`), with two wide characters only, with a full
/// stop as its second narrow character, or with seven comparisons. Each of these is a handful of
/// glyphs whose widths fall right by chance as easily as by being read right.
#[test]
fn each_condition_of_the_width_proof_is_needed() {
    use hayro::hayro_interpret::font::test_hooks::widths_prove;
    let enough = [('a', 556.0), ('b', 556.0), ('d', 556.0), ('e', 556.0), ('i', 222.0), ('l', 222.0)];
    assert!(widths_prove(&enough), "four round letters against `i` and `l` prove");
    let capitals_only = [('A', 667.0), ('B', 667.0), ('C', 722.0), ('D', 722.0), ('i', 222.0), ('l', 222.0)];
    assert!(!widths_prove(&capitals_only), "no lowercase pair");
    let two_wide = [('a', 556.0), ('b', 556.0), ('i', 222.0), ('l', 222.0), ('j', 222.0), ('.', 278.0), (',', 278.0)];
    assert!(!widths_prove(&two_wide), "two wide characters");
    let stop_as_narrow = [('a', 556.0), ('b', 556.0), ('d', 556.0), ('e', 556.0), ('i', 222.0), ('.', 278.0)];
    assert!(!widths_prove(&stop_as_narrow), "a full stop is no narrow letter");
    let seven = [('A', 667.0), ('B', 667.0), ('a', 556.0), ('b', 556.0), ('i', 222.0), ('r', 333.0)];
    assert!(!widths_prove(&seven), "seven comparisons");
}

/// A page of `type3` Type 3 fonts, then `/C83 /C111 /C99` (`Soc`) in a numbered-names font
/// (rule 1).
fn type3_fonts_then_names(type3: usize) -> Vec<u8> {
    const CFF: &[u8] = include_bytes!("../../../../vendor/hayro-interpret/assets/FoxitFixed.pfb");
    let resources: String = (0..type3).map(|k| format!("/T{k} {} 0 R ", k + 5)).collect();
    let names = type3 + 5;
    let mut content: String = (0..type3).map(|k| format!("BT /T{k} 10 Tf 20 {} Td <41> Tj ET ", 700 - (k % 60) * 11)).collect();
    content.push_str("BT /F3 20 Tf 300 100 Td <010203> Tj ET");
    let mut pdf = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [4 0 R] /Count 1 /MediaBox [0 0 600 800] >>")
        .stream("", b"500 0 0 0 500 700 d1 0 0 500 700 re f")
        .obj(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << {resources}/F3 {names} 0 R >> >> >>", names + 3));
    // Each font its own dictionary (`/Name`): fonts are told apart by their bytes.
    for k in 0..type3 {
        pdf = pdf.obj(format!(
            "<< /Type /Font /Subtype /Type3 /Name /T{k} /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 65 /Widths [500] /Encoding << /Type /Encoding /Differences [65 /A] >> /CharProcs << /A 3 0 R >> >>"
        ));
    }
    pdf.obj(format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+AdvTTSynthetic /FirstChar 1 /LastChar 3 /Widths [600 600 600] /FontDescriptor {} 0 R /Encoding << /Type /Encoding /Differences [1 /C83 /C111 /C99] >> >>",
        names + 1
    ))
    .obj(format!(
        "<< /Type /FontDescriptor /FontName /ABCDEF+AdvTTSynthetic /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 {} 0 R >>",
        names + 2
    ))
    .stream("/Subtype /Type1C", CFF)
    .stream("", content)
    .build()
}

/// Type 3 glyphs go to the recovery rules as outline glyphs do (asked once per glyph, under the
/// rules' font-level guards), and their fonts count toward the page's budget of fonts asked: a
/// page showing more Type 3 fonts than the budget allows recovers nothing from the font after
/// them, which a few Type 3 fonts leave to rule 1. The strict text is the same either way.
#[test]
fn type3_glyphs_reach_the_rules_and_count_toward_the_font_budget() {
    use hayro::hayro_interpret::font::recovery_rule_calls;
    let pdf = parse(type3_fonts_then_names(3));
    let before = recovery_rule_calls();
    let (strict, recovered, plain) = layers(&pdf);
    // Three Type 3 glyphs and three numbered-names glyphs asked; the plain extraction asks none.
    assert_eq!(recovery_rule_calls() - before, 6);
    assert_eq!(strict, plain);
    assert_eq!(recovered.map(|r| r.text.plain_text()).as_deref().map(|t| t.contains("Soc")), Some(true));
    let (strict, recovered, plain) = layers(&parse(type3_fonts_then_names(1025)));
    assert_eq!(strict, plain);
    assert!(!strict.glyphs.is_empty());
    assert!(recovered.is_none(), "past the font budget nothing is recovered");
}

/// `Hi` in Helvetica, then `text` in a Type 0 font over an embedded CID-keyed CFF program
/// (`CIDFontType0`, `Identity-H`, no `/ToUnicode`) whose CIDs are the CFF standard strings of
/// `text`'s characters.
fn cid_cff_page(text: &str) -> Vec<u8> {
    let cids: Vec<u16> = text.chars().map(pdfcraft_testkit::cff::standard_string_id).collect();
    let w: String =
        text.chars().map(|c| format!("{} [{}] ", pdfcraft_testkit::cff::standard_string_id(c), pdfcraft_testkit::cff::helvetica_width(c))).collect();
    let shown: String = cids.iter().map(|c| format!("{c:04X}")).collect();
    page_with_font(
        "<< /Type /Font /Subtype /Type0 /BaseFont /ABCDEF+Meta /Encoding /Identity-H /DescendantFonts [7 0 R] >>",
        &format!("<{shown}>"),
        true,
        &[
            (
                &format!(
                    "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /ABCDEF+Meta /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /W [{w}] /FontDescriptor 8 0 R >>"
                ),
                Vec::new(),
            ),
            (
                "<< /Type /FontDescriptor /FontName /ABCDEF+Meta /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 9 0 R >>",
                Vec::new(),
            ),
            ("/Subtype /CIDFontType0C", pdfcraft_testkit::cff::cid_keyed(&cids)),
        ],
    )
}

/// A Type 0 font over an embedded CID-keyed CFF program (`CIDFontType0`) goes to the rules as a
/// font of its own kind, with its charset's CIDs; no rule here reads such a font, so it keeps
/// its strict text (nothing, without `/ToUnicode`: ISO 32000-2 §9.10.2) and recovers nothing.
#[test]
fn a_cid_keyed_cff_font_is_asked_of_the_rules_and_keeps_its_strict_text() {
    use hayro::hayro_interpret::font::recovery_rule_calls;
    let text = "Indicated";
    let page = cid_cff_page(text);
    let before = recovery_rule_calls();
    let (strict, recovered, plain) = layers(&parse(page));
    assert!(recovery_rule_calls() - before >= text.len(), "its glyphs are asked of the rules");
    assert_eq!(strict, plain);
    assert_eq!(strict.plain_text(), "Hi");
    assert!(recovered.is_none());
}

/// A page of `shown` glyphs, 60 to a line, of a Type 0 font (`Identity-H`, no `/ToUnicode`, an
/// identity `/CIDSystemInfo`) over an embedded TrueType program of `glyphs` glyphs that all
/// draw: a dense CJK font, whose glyph ids no rule can read.
fn dense_cjk_page(glyphs: u16, shown: usize) -> Vec<u8> {
    let drawn: Vec<u16> = (1..glyphs).collect();
    let program = subset(glyphs, &drawn, Cmap::None, None);
    let span = usize::from(glyphs.saturating_sub(1)).max(1);
    let lines: Vec<String> = (0..shown)
        .collect::<Vec<_>>()
        .chunks(60)
        .map(|line| format!("<{}> Tj T*", line.iter().map(|k| format!("{:04X}", 1 + (k * 7919) % span)).collect::<String>()))
        .collect();
    let content = format!("BT /F3 6 Tf 7 TL 20 780 Td {} ET", lines.join(" "));
    pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>")
        .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F3 5 0 R >> >> >>")
        .stream("", content)
        .obj("<< /Type /Font /Subtype /Type0 /BaseFont /ABCDEF+DenseMing /Encoding /Identity-H /DescendantFonts [6 0 R] >>")
        .obj("<< /Type /Font /Subtype /CIDFontType2 /BaseFont /ABCDEF+DenseMing /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /CIDToGIDMap /Identity /DW 1000 /FontDescriptor 7 0 R >>")
        .obj("<< /Type /FontDescriptor /FontName /ABCDEF+DenseMing /Flags 4 /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 /FontFile2 8 0 R >>")
        .stream("", program)
        .build()
}

/// Both layers of `pdf`'s first page, the strict layer as the plain extraction reads it, and
/// what reading both layers cost the recovery layer.
fn recovering_costs(pdf: &Pdf) -> (PageText, Option<RecoveredText>, PageText, hayro::hayro_interpret::font::RecoveryCounters) {
    use hayro::hayro_interpret::font::recovery_counters;
    let settings = InterpreterSettings::default();
    let before = recovery_counters();
    let (strict, recovered) = extract_page_recovering(pdf, 0, &settings).unwrap();
    let cost = recovery_counters().since(&before);
    (strict, recovered, extract_page(pdf, 0, &settings).unwrap(), cost)
}

/// A font of a kind no rule reads (Type 3) costs the recovered layer one check per glyph and
/// nothing else: its guards are never decided, its facts never built. A font a rule can read (a
/// Type 1 font, the standard Helvetica too, a CID-keyed CFF program) is decided once, and one no
/// rule takes costs nothing more.
#[test]
fn a_font_kind_no_rule_reads_costs_one_check_per_glyph() {
    let (strict, recovered, plain, cost) = recovering_costs(&parse(type3_fonts_then_names(3)));
    assert_eq!(strict, plain);
    assert!(recovered.is_some());
    // Three Type 3 glyphs and three numbered-names glyphs; only the numbered-names font is
    // decided, and only its three glyphs read (its facts once for the guards, once a glyph).
    assert_eq!(cost.rule_calls, 6);
    assert_eq!((cost.guard_decisions, cost.facts_built, cost.glyph_facts_built), (1, 4, 3), "{cost:?}");

    let text = "Indicated";
    let (strict, recovered, plain, cost) = recovering_costs(&parse(cid_cff_page(text)));
    assert_eq!(strict, plain);
    assert!(recovered.is_none());
    assert_eq!(cost.rule_calls, 2 + text.len());
    // Helvetica's `Hi`, and the CID-keyed CFF font (whose rule, without a document's evidence,
    // takes nothing).
    assert_eq!((cost.guard_decisions, cost.facts_built, cost.glyph_facts_built, cost.program_walks), (2, 2, 0, 0), "{cost:?}");
}

/// A Type 3 font's cache key is worked out once per font, however many of its glyphs a page
/// shows: the recovered layer asks for it with every glyph (its budget of fonts per page).
#[test]
fn a_type3_fonts_key_is_hashed_once_however_many_glyphs_it_shows() {
    let page = |glyphs: usize| {
        let codes = "41".repeat(glyphs);
        pdfcraft_testkit::Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] >>")
            .obj("<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /T0 5 0 R >> >> >>")
            .stream("", format!("BT /T0 1 Tf 20 700 Td <{codes}> Tj ET"))
            .obj("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /FirstChar 65 /LastChar 65 /Widths [500] /Encoding << /Type /Encoding /Differences [65 /A] >> /CharProcs << /A 6 0 R >> >>")
            .stream("", b"500 0 0 0 500 700 d1 0 0 500 700 re f")
            .build()
    };
    let (_, _, _, few) = recovering_costs(&parse(page(10)));
    let (strict, _, plain, many) = recovering_costs(&parse(page(500)));
    assert_eq!(strict, plain);
    assert_eq!(many.rule_calls, 500);
    assert!(few.type3_key_hashes > 0);
    assert_eq!(many.type3_key_hashes, few.type3_key_hashes, "the key is hashed per font, not per glyph");
}

/// A dense CJK page (3,000 glyphs of a Type 0 font over a program of 20,000 glyphs, no
/// `/ToUnicode`): the font's guards walk its program once, whatever its size (2,000 glyphs
/// cost the same walks and decisions as 20,000); no rule takes it, so each glyph costs the one
/// check and no more; and the strict text is the plain extraction's.
#[test]
fn a_dense_cjk_page_costs_the_same_per_font_whatever_the_fonts_size() {
    let shown = 3000;
    let mut costs = Vec::new();
    for glyphs in [2000, 20_000] {
        let (strict, recovered, plain, cost) = recovering_costs(&parse(dense_cjk_page(glyphs, shown)));
        assert_eq!(strict, plain, "{glyphs} glyphs");
        // Without `/ToUnicode` the glyphs have no strict text (ISO 32000-2 §9.10.2).
        assert!(strict.glyphs.is_empty(), "{glyphs} glyphs");
        assert!(recovered.is_none(), "{glyphs} glyphs");
        assert_eq!(cost.rule_calls, shown, "one check a glyph");
        assert_eq!((cost.guard_decisions, cost.facts_built, cost.glyph_facts_built), (1, 1, 0), "{glyphs} glyphs: {cost:?}");
        costs.push(cost);
    }
    assert_eq!(costs.first(), costs.last(), "per-font work grew with the font's glyphs");
    assert!(costs.first().is_some_and(|c| c.program_walks > 0), "the guard read the program");
}

/// A dense page past the recovered-glyph budget has no recovered layer, and its strict text as
/// the plain extraction reads it; past the dedupe budget, recovered doubles stop being dropped
/// (the recovered text reads them twice) and the work stops at the budget. Neither panics.
#[test]
fn dense_pages_stop_at_the_recovery_budgets() {
    // 1,500 `S` (strict) and 1,500 `o` (rule 1) glyphs, 60 to a line.
    let line = format!("<{}> Tj T*", "0102".repeat(30));
    let lines = vec![line; 50].join(" ");
    let content = format!("BT /F3 6 Tf 7 TL 20 780 Td {lines} ET");
    let pdf = parse(page_of("/S /C111", &content));
    let count = |text: Option<RecoveredText>, c: char| text.map(|r| r.text.plain_text().matches(c).count());

    RECOVERED_GLYPHS.with(|n| n.set(Some(1000)));
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| layers(&pdf)));
    RECOVERED_GLYPHS.with(|n| n.set(None));
    let (strict, recovered, plain) = read.expect("no panic past the recovered-glyph budget");
    assert_eq!(strict, plain);
    assert_eq!(strict.plain_text().matches('S').count(), 1500);
    assert!(recovered.is_none(), "past the recovered-glyph budget the page has no recovered layer");
    let (_, recovered, _) = layers(&pdf);
    assert_eq!(count(recovered, 'o'), Some(1500), "within it, every glyph is read");

    // The same lines drawn twice at the same place: 1,500 recovered doubles to drop.
    let twice = parse(page_of("/S /C111", &format!("{content} {content}")));
    let budget = 500;
    PAGE_WORK.with(|w| w.set(Some(budget)));
    DOUBLES_WORK.with(|c| c.set(0));
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| layers(&twice)));
    PAGE_WORK.with(|w| w.set(None));
    let (strict, recovered, plain) = read.expect("no panic past the dedupe budget");
    assert_eq!(strict, plain);
    let work = DOUBLES_WORK.with(std::cell::Cell::get);
    assert!(work <= budget, "the dedupe stopped at its budget: {work} steps");
    let os = count(recovered, 'o');
    assert!(os.is_some_and(|n| n > 1500), "past the budget the doubles are kept as they are: {os:?}");
    let (_, recovered, _) = layers(&twice);
    assert_eq!(count(recovered, 'o'), Some(1500), "within it, the doubles are dropped");
}
