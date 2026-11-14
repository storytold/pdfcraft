//! Copy with formatting's standard CJK order rule (hayro-interpret patch 107,
//! `StandardCjkGlyphOrder`): Traditional Chinese read from the glyph ids of Type 0 subsets that
//! keep their original font's order, found from the document's text. Every document here is
//! synthetic ([`super::cjk_fixture`]).

use super::cjk_fixture::{Doc, Font, Program, Show};
use super::*;
use hayro::hayro_interpret::font::RecoveryDocument;
use hayro::hayro_interpret::font::test_hooks::{
    cjk_common, cjk_group_reading, cjk_guards, cjk_order_char, cjk_output, cjk_output_counts_agree, cjk_outputs, cjk_placeholder_name, cjk_search,
    set_survey_glyph_cap,
};
use std::sync::Arc;

/// Common characters only (each one of the 750), 31 different ones.
const TEXT: &str = "本公司依證券交易法第三十六條規定編製年報並經董事會通過後公告於主管機關指定之網站";
/// More common characters.
const MORE: &str = "公司應依法辦理各項工作並於期限內完成相關事項";
/// Characters that are not common.
const RARE: &str = "麒麟鸚鵡龜鯨齒蠶";

fn parse(bytes: Vec<u8>) -> Pdf {
    Pdf::new(Arc::new(bytes)).unwrap()
}

/// Every page's recovered text (whitespace dropped), read with one document as Copy with
/// formatting reads it, in page order.
fn recovered_pages(doc: &Doc) -> Vec<String> {
    let pdf = parse(doc.build());
    let settings = InterpreterSettings { recovery_document: Some(Arc::new(RecoveryDocument::new())), ..InterpreterSettings::default() };
    (0..doc.pages.len()).map(|p| recovered_on(&pdf, p, &settings)).collect()
}

fn recovered_on(pdf: &Pdf, page: usize, settings: &InterpreterSettings) -> String {
    let (_, layer) = extract_page_recovering(pdf, page, settings).unwrap();
    layer.map(|l| l.text.plain_text()).unwrap_or_default().chars().filter(|c| !c.is_whitespace()).collect()
}

fn recovered(doc: &Doc) -> String {
    recovered_pages(doc).concat()
}

/// The first position of `c` in `order` (its code point in the uni order).
fn position(order: &str, c: char) -> u32 {
    if order == "uni" {
        return u32::from(c);
    }
    (0..20_000).find(|&p| cjk_order_char(order, p) == Some(c)).unwrap_or_else(|| panic!("{c} is not in the {order} order"))
}

/// The glyph numbers `text` has in `order` with `offset`.
fn glyphs(order: &str, offset: i64, text: &str) -> Vec<u16> {
    text.chars().map(|c| u16::try_from(i64::from(position(order, c)) + offset).unwrap()).collect()
}

/// A document of one TrueType subset (`glyphs` glyphs) showing `codes`.
fn one_font(codes: Vec<u16>) -> Doc {
    Doc { fonts: vec![Font::truetype("ABCDEF+DFKaiShu-SB-Estd-BF-ETen-B5-H-Identity-H", 30_000)], pages: vec![vec![Show::Text(0, codes)]] }
}

#[test]
fn the_sample_texts_are_common_and_the_rare_ones_not() {
    assert!(TEXT.chars().chain(MORE.chars()).all(cjk_common));
    assert!(!RARE.chars().any(cjk_common));
}

/// Each of the four orders is read, at the offset its glyph ids carry.
#[test]
fn each_order_reads_at_its_offset() {
    for (order, offset) in [("big5", 499), ("big5", 320), ("uni", -12_376), ("uni_big5", 356), ("cns1", 0), ("big5", 915)] {
        let codes = [glyphs(order, offset, TEXT), glyphs(order, offset, MORE)].concat();
        assert_eq!(recovered(&one_font(codes)), format!("{TEXT}{MORE}"), "{order} {offset}");
    }
}

/// A CID-keyed CFF subset is read by CID, and a TrueType subset with a `/CIDToGIDMap` stream by
/// the glyph id it maps each CID to.
#[test]
fn cff_and_mapped_truetype_subsets_are_read_by_their_numbers() {
    let numbers = [glyphs("big5", 499, TEXT), glyphs("big5", 499, MORE)].concat();
    let doc = Doc { fonts: vec![Font::cff("ABCDEF+DFKaiShu", numbers.clone())], pages: vec![vec![Show::Text(0, numbers.clone())]] };
    assert_eq!(recovered(&doc), format!("{TEXT}{MORE}"));

    // CIDs 1, 2, 3 … draw the glyphs the Big5 order puts the text at.
    let mut mapped = Font::truetype("ABCDEF+DFKaiShu", 30_000);
    let mut map = vec![0_u16];
    map.extend(&numbers);
    mapped.cid_to_gid = Some(map);
    let cids: Vec<u16> = (1..=u16::try_from(numbers.len()).unwrap()).collect();
    let doc = Doc { fonts: vec![mapped], pages: vec![vec![Show::Text(0, cids)]] };
    assert_eq!(recovered(&doc), format!("{TEXT}{MORE}"));
}

/// An offset one off is a rival like any other: a text whose characters' neighbours in the
/// order are common too reads at neither offset.
#[test]
fn an_offset_one_off_that_reads_as_well_blocks_the_font() {
    // Code points whose next code point is common too: the uni order at -12,376 and at
    // -12,375 read them equally well.
    let pairs: Vec<char> = (0x4E00_u32..0x9FFF)
        .filter_map(char::from_u32)
        .filter(|&c| cjk_common(c) && char::from_u32(u32::from(c) + 1).is_some_and(cjk_common))
        .take(20)
        .collect();
    assert!(pairs.len() >= 10, "{pairs:?}");
    let text: String = pairs.iter().collect();
    let codes = glyphs("uni", -12_376, &text);
    let (_, _, best, rival, _, _, passes) = cjk_search(&{
        let mut v: Vec<(u32, u64)> = codes.iter().map(|&g| (u32::from(g), 1)).collect();
        v.sort_unstable();
        v
    })
    .unwrap();
    assert!(!passes && rival > 0.6 * best, "{best} {rival}");
    assert_eq!(recovered(&one_font(codes)), "");
}

/// The guards, at each threshold: the best reading's score of 0.6, a rival at 0.6 times the
/// best, a close reading's ratio of 0.6, and ten different glyphs.
#[test]
fn each_guard_holds_at_its_threshold() {
    assert!(cjk_guards(0.6, 0.36, 0.6, 10));
    assert!(!cjk_guards(0.599_999_999, 0.0, 0.0, 10), "best");
    assert!(!cjk_guards(0.8, 0.480_000_001, 0.0, 10), "rival");
    assert!(cjk_guards(0.8, 0.48, 0.0, 10), "rival at 0.6 times");
    assert!(!cjk_guards(0.8, 0.0, 0.600_000_001, 10), "close");
    assert!(!cjk_guards(0.8, 0.0, 0.0, 9), "glyphs");
}

/// Rare characters among the glyphs a font shows lower its score: a text with a few reads, one
/// where rare characters outnumber the common ones doesn't.
#[test]
fn a_font_whose_glyphs_are_mostly_rare_reads_nothing() {
    let text = |rare: usize| -> String { TEXT.chars().chain(MORE.chars()).chain(RARE.chars().cycle().take(rare)).collect() };
    assert!(!recovered(&one_font(glyphs("big5", 499, &text(4)))).is_empty());
    assert_eq!(recovered(&one_font(glyphs("big5", 499, &text(120)))), "");
}

/// Fewer than ten different glyphs never read, however clear their reading.
#[test]
fn nine_glyphs_read_nothing() {
    let nine: String = TEXT.chars().take(9).collect();
    let (.., glyphs_seen, passes) = cjk_search(&ones("uni", -12_376, &nine)).unwrap();
    assert_eq!((glyphs_seen, passes), (9, false));
    assert_eq!(recovered(&one_font(glyphs("uni", -12_376, &nine))), "");
}

/// A reading that agrees with the best on most glyphs (the Big5 order and Adobe-CNS1, 187 apart
/// for most ideographs) but where they differ reads common characters more than 0.6 times as
/// often blocks the font; at 0.6 times or less it doesn't.
#[test]
fn a_close_reading_common_where_they_differ_blocks_the_font() {
    const CNS1_SHIFT: u32 = 187;
    let reads = |order: &str, p: u32| cjk_order_char(order, p);
    let output =
        |p: u32| reads("big5", p).is_some_and(|c| cjk_outputs("big5", c)) && reads("cns1", p + CNS1_SHIFT).is_some_and(|c| cjk_outputs("cns1", c));
    // Positions where the two readings agree, where only Big5 reads a common character, and
    // where only Adobe-CNS1 does, all read as characters the rule outputs.
    let agree: Vec<u32> = (0..13_502)
        .filter(|&p| output(p) && reads("big5", p).is_some_and(cjk_common) && reads("big5", p) == reads("cns1", p + CNS1_SHIFT))
        .take(40)
        .collect();
    let big5_only: Vec<u32> = (0..13_502)
        .filter(|&p| output(p) && reads("big5", p).is_some_and(cjk_common) && reads("cns1", p + CNS1_SHIFT).is_some_and(|c| !cjk_common(c)))
        .take(5)
        .collect();
    let cns1_only: Vec<u32> = (0..13_502)
        .filter(|&p| output(p) && reads("cns1", p + CNS1_SHIFT).is_some_and(cjk_common) && reads("big5", p).is_some_and(|c| !cjk_common(c)))
        .take(5)
        .collect();
    assert_eq!((agree.len(), big5_only.len(), cns1_only.len()), (40, 5, 5));
    let search = |cns1_weight: u64| {
        let mut v: Vec<(u32, u64)> = agree
            .iter()
            .map(|&p| (p + 499, 1))
            .chain(big5_only.iter().map(|&p| (p + 499, 3)))
            .chain(cns1_only.iter().map(|&p| (p + 499, cns1_weight)))
            .collect();
        v.sort_unstable();
        cjk_search(&v).unwrap()
    };
    // Where they differ, Big5 reads 15 common glyphs, Adobe-CNS1 5: a ratio of 1/3.
    let (order, offset, _, _, close, _, passes) = search(1);
    assert_eq!((order, offset, passes), ("big5", 499, true), "{close}");
    assert!((close - 1.0 / 3.0).abs() < 1e-9, "{close}");
    // Adobe-CNS1 10 of 15: 2/3, over 0.6.
    let (order, offset, _, _, close, _, passes) = search(2);
    assert_eq!((order, offset, passes), ("big5", 499, false), "{close}");
    assert!((close - 2.0 / 3.0).abs() < 1e-9, "{close}");
}

/// Subsets of one font in a document are searched together: three subsets too small to read
/// alone (nine glyphs each) read together, with the group's reading. A font named otherwise, or
/// with another glyph count, is no part of the group (and too small to read alone), and a producer's placeholder name
/// (`CIDFont+F1`) makes no group, since unrelated fonts share it.
#[test]
fn subsets_of_one_font_read_together() {
    let text: Vec<char> = TEXT.chars().chain(MORE.chars()).collect();
    let parts: Vec<String> = text.chunks(9).take(3).map(|p| p.iter().collect()).collect();
    let doc = |names: [(&str, u16); 3]| Doc {
        fonts: names.iter().map(|&(name, glyphs)| Font::truetype(name, glyphs)).collect(),
        pages: vec![parts.iter().enumerate().map(|(i, part)| Show::Text(i, glyphs("uni", -12_376, part))).collect()],
    };
    let all: String = parts.concat();
    assert_eq!(recovered(&doc([("AAAAAA+Kai", 30_000), ("BBBBBB+Kai", 30_000), ("CCCCCC+Kai", 30_000)])), all);
    assert!(!recovered(&doc([("AAAAAA+Kai", 30_000), ("BBBBBB+Kai", 30_000), ("CCCCCC+Hei", 30_000)])).contains(parts[2].as_str()));
    assert!(!recovered(&doc([("AAAAAA+Kai", 30_000), ("BBBBBB+Kai", 30_000), ("CCCCCC+Kai", 30_001)])).contains(parts[2].as_str()));
    assert_eq!(recovered(&doc([("CIDFont+F1", 30_000), ("CIDFont+F1", 30_000), ("CIDFont+F1", 30_000)])), "");
    for (name, placeholder) in [
        ("CIDFont+F2-ETen-B5-H", true),
        ("F12", true),
        ("", true),
        ("123", true),
        ("TT58E809A6tCID", false),
        ("CIDFont+F", false),
        ("DFKaiShu-SB-Estd-BF-ETen-B5-H", false),
        ("F12abc", false),
    ] {
        assert_eq!(cjk_placeholder_name(name), placeholder, "{name:?}");
    }
}

/// A group whose reading reads a member that reads by itself otherwise, on half its glyphs or
/// more, isn't read together: that member is another font under the same name.
#[test]
fn a_group_that_contradicts_a_member_is_not_read_together() {
    let text: Vec<char> = TEXT.chars().chain(MORE.chars()).collect();
    let small: Vec<Vec<(u32, u64)>> = text.chunks(9).take(3).map(|part| ones("uni", -12_376, &part.iter().collect::<String>())).collect();
    assert_eq!(cjk_group_reading("Kai", &small), Some(("uni", -12_376)));
    // One member reads alone in the Big5 order, the other alone in the uni order; the group's
    // reading (the Big5 one, which has more glyphs) reads the second member otherwise.
    let mut big = ones("big5", 499, &text.iter().collect::<String>());
    for g in &mut big {
        g.1 = 4;
    }
    let mixed = [big, ones("uni", -12_376, &text.iter().collect::<String>())];
    assert_eq!(cjk_search(&mixed[1]).map(|s| (s.0, s.1, s.6)), Some(("uni", -12_376, true)));
    assert_eq!(cjk_group_reading("Kai", &mixed), None);
}

/// When the group's reading fails the guards, each subset reads by its own when that passes,
/// and not otherwise.
#[test]
fn a_group_that_fails_leaves_each_subset_to_itself() {
    let rare: String = RARE.chars().cycle().take(40).collect();
    let doc = Doc {
        fonts: vec![Font::truetype("AAAAAA+Kai", 30_000), Font::truetype("BBBBBB+Kai", 30_000)],
        pages: vec![vec![Show::Text(0, glyphs("big5", 499, TEXT)), Show::Text(1, glyphs("big5", 499, &rare))]],
    };
    assert_eq!(recovered(&doc), TEXT);
}

/// The rule only fills: a font with a `/ToUnicode` map, or of an Adobe character collection,
/// is never read, and nothing it shows changes.
#[test]
fn fonts_with_text_of_their_own_are_never_read() {
    // Adobe-CNS1 order at offset 0, and CID 129 (`A15E`, punctuation), whose code an identity
    // `/ToUnicode` reads as a control character, so that glyph has no strict text.
    let codes = [glyphs("cns1", 0, TEXT), glyphs("cns1", 0, MORE), vec![129]].concat();
    let mut named = Font::truetype("ABCDEF+Kai", 30_000);
    named.to_unicode = true;
    let mut adobe = Font::truetype("ABCDEF+Kai", 30_000);
    adobe.ordering = "CNS1";
    // `/ToUnicode /Identity-H` names every code's text (the code itself): the survey counts such
    // a font with its group, but the rule never reads it.
    let mut identity = Font::truetype("ABCDEF+Kai", 30_000);
    identity.to_unicode_name = Some("Identity-H");
    for font in [named, adobe, identity] {
        let doc = Doc { fonts: vec![font], pages: vec![vec![Show::Text(0, codes.clone())]] };
        let pdf = parse(doc.build());
        let settings = InterpreterSettings { recovery_document: Some(Arc::new(RecoveryDocument::new())), ..InterpreterSettings::default() };
        let (strict, layer) = extract_page_recovering(&pdf, 0, &settings).unwrap();
        assert!(layer.is_none_or(|l| l.recovery.iter().all(Option::is_none)));
        assert_eq!(strict, extract_page(&pdf, 0, &settings).unwrap());
    }
    // Without a program there is nothing to read either.
    let mut bare = Font::truetype("Kai", 30_000);
    bare.program = Program::None;
    assert_eq!(recovered(&Doc { fonts: vec![bare], pages: vec![vec![Show::Text(0, codes)]] }), "");
}

/// Only ideographs (Big5 `A440` on) and the common punctuation row (`A141`–`A15E`) are output;
/// the uni order outputs CJK unified ideographs of Big5 only.
#[test]
fn only_ideographs_and_common_punctuation_are_output() {
    let big5 = |p: u32| cjk_order_char("big5", p).unwrap();
    // Positions 1 (A141, `，`), 31 (A15F, past the row) and 157 (A1DD) of the Big5 order.
    assert!(cjk_outputs("big5", big5(1)));
    assert!(!cjk_outputs("big5", big5(31)));
    assert!(!cjk_outputs("big5", big5(157)));
    assert!(cjk_outputs("big5", '中'));
    assert!(cjk_outputs("uni", '中'));
    assert!(!cjk_outputs("uni", '，'), "punctuation isn't read in the uni order");
    assert!(!cjk_outputs("uni", '\u{4E28}'), "an ideograph Big5 doesn't have");
    // A glyph past the scope reads nothing among glyphs that read.
    let mut codes = glyphs("big5", 499, TEXT);
    codes.push(u16::try_from(157 + 499).unwrap());
    assert_eq!(recovered(&one_font(codes)), TEXT);
}

/// The position of Big5 code `code` in the Big5 order (`A140` on, without `A3C0`–`A3FE` and
/// `C6A1`–`C8FE`).
fn big5_position(code: u32) -> u32 {
    let mut p = 0;
    for lead in 0xA1..=0xF9_u32 {
        for trail in (0x40..=0x7E_u32).chain(0xA1..=0xFE) {
            let c = (lead << 8) | trail;
            if (0xA3C0..=0xA3FE).contains(&c) || (0xC6A1..=0xC8FE).contains(&c) {
                continue;
            }
            if c == code {
                return p;
            }
            p += 1;
        }
    }
    panic!("{code:X} is not in the order")
}

/// The Big5 order's characters, at the codes where the Adobe CMaps and code page 950 differ, are
/// code page 950's, and the order ends at `F9FE`.
#[test]
fn the_big5_order_reads_as_code_page_950() {
    let at = |code| cjk_order_char("big5", big5_position(code));
    assert_eq!(at(0xA140), Some('\u{3000}'));
    assert_eq!(at(0xA2CC), Some('十'));
    assert_eq!(at(0xA2CE), Some('卅'));
    assert_eq!(at(0xA440), Some('一'));
    assert_eq!(at(0xC94A), Some('\u{FA0C}'));
    assert_eq!(at(0xF9FE), Some('\u{2593}'));
    assert_eq!(big5_position(0xF9FE), 13_501);
    assert_eq!(cjk_order_char("big5", 13_502), None);
    // 兀 read through its second code is written in its canonical form.
    let codes = [glyphs("big5", 499, TEXT), vec![u16::try_from(big5_position(0xC94A) + 499).unwrap()]].concat();
    assert_eq!(recovered(&one_font(codes)), format!("{TEXT}兀"));
}

/// The evidence is the whole document's: a page showing too few glyphs to read alone reads
/// with the rest of the document's, whichever page is read first; a font shown only past page
/// 400 has no evidence.
#[test]
fn the_evidence_is_the_whole_documents_whatever_page_is_read_first() {
    let first: String = TEXT.chars().take(5).collect();
    let rest: String = TEXT.chars().skip(5).collect();
    let doc = Doc {
        fonts: vec![Font::truetype("ABCDEF+Kai", 30_000)],
        pages: vec![vec![Show::Text(0, glyphs("big5", 499, &first))], vec![Show::Text(0, glyphs("big5", 499, &rest))]],
    };
    let pdf = parse(doc.build());
    for order in [[0, 1], [1, 0]] {
        let settings = InterpreterSettings { recovery_document: Some(Arc::new(RecoveryDocument::new())), ..InterpreterSettings::default() };
        let mut read = [String::new(), String::new()];
        for p in order {
            read[p] = recovered_on(&pdf, p, &settings);
        }
        assert_eq!(read, [first.clone(), rest.clone()], "order {order:?}");
    }
    // Without a document (each page alone) nothing is read: the rule needs the evidence.
    assert_eq!(recovered_on(&pdf, 0, &InterpreterSettings::default()), "");

    let mut pages = vec![Vec::new(); 401];
    pages[400] = vec![Show::Text(0, glyphs("big5", 499, TEXT))];
    let late = Doc { fonts: vec![Font::truetype("ABCDEF+Kai", 30_000)], pages };
    let pdf = parse(late.build());
    let settings = InterpreterSettings { recovery_document: Some(Arc::new(RecoveryDocument::new())), ..InterpreterSettings::default() };
    assert_eq!(recovered_on(&pdf, 400, &settings), "");
}

/// A document past the survey's cap reads nothing: no reading rather than one from part of it.
#[test]
fn a_document_past_the_surveys_cap_reads_nothing() {
    let doc = one_font(glyphs("big5", 499, TEXT));
    let n = TEXT.chars().count() as u64;
    set_survey_glyph_cap(Some(n));
    let within = recovered(&doc);
    set_survey_glyph_cap(Some(n - 1));
    let past = recovered(&doc);
    set_survey_glyph_cap(None);
    assert_eq!((within.as_str(), past.as_str()), (TEXT, ""));
}

/// The strict text never changes, whatever the rule reads.
#[test]
fn the_strict_text_is_unchanged() {
    let doc = Doc {
        fonts: vec![Font::truetype("ABCDEF+Kai", 30_000), {
            let mut named = Font::truetype("ABCDEF+Named", 30_000);
            named.to_unicode = true;
            named
        }],
        pages: vec![vec![Show::Text(0, glyphs("big5", 499, TEXT)), Show::Text(1, vec![0, 1, 2])]],
    };
    let pdf = parse(doc.build());
    let settings = InterpreterSettings { recovery_document: Some(Arc::new(RecoveryDocument::new())), ..InterpreterSettings::default() };
    let (strict, layer) = extract_page_recovering(&pdf, 0, &settings).unwrap();
    let plain = extract_page(&pdf, 0, &settings).unwrap();
    assert_eq!(strict, plain);
    assert_eq!(plain.plain_text().trim(), "XYZ");
    assert!(layer.is_some_and(|l| l.text.plain_text().contains(TEXT)));
}

// The alternative policy (`output-share`), which the rule can be switched to: each of its
// choices against the shipped one's.

/// (glyph number, 1) for each glyph of `text` in `order` at `offset`, sorted, each glyph once.
fn ones(order: &str, offset: i64, text: &str) -> Vec<(u32, u64)> {
    let mut v: Vec<(u32, u64)> = glyphs(order, offset, text).into_iter().map(|g| (u32::from(g), 1)).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// An offset is scored on the glyphs it reads as characters the rule outputs: glyphs read
/// outside them (here the symbol rows before the ideographs, as a font's digits and Latin letters
/// read) don't dilute the score, as long as at least half the glyphs read as output; with more of
/// them, the offset isn't scored at all.
#[test]
fn an_offset_is_scored_on_what_it_outputs() {
    let mut numbers = ones("big5", 499, TEXT);
    // Positions 160 to 189: symbols of rows A1 and A2, no character the rule outputs.
    numbers.extend((160..190).map(|p| (p + 499, 1)));
    numbers.sort_unstable();
    let (order, offset, best, ..) = cjk_search(&numbers).unwrap();
    assert_eq!((order, offset), ("big5", 499));
    assert!((best - 1.0).abs() < 1e-9, "{best}");
    let mut diluted = ones("big5", 499, TEXT);
    diluted.extend((160..230).map(|p| (p + 499, 1)));
    diluted.sort_unstable();
    let (order, offset, best, _, _, _, passes) = cjk_search(&diluted).unwrap();
    assert!(!passes && (order, offset) != ("big5", 499), "{order} {offset} {best}");
}

/// A score is a share: a common character counts only where the order outputs it. The
/// ideographic space is common but never output, and the uni order outputs ideographs only, so
/// neither the space nor the uni order's punctuation raises a score above 1.
#[test]
fn a_common_character_counts_only_where_it_is_output() {
    let mut numbers = ones("uni", 0, TEXT);
    numbers.push((0x3000, 31));
    numbers.push((u32::from('，'), 1));
    numbers.sort_unstable();
    let (_, _, best, rival, ..) = cjk_search(&numbers).unwrap();
    assert!(best <= 1.0 && rival <= 1.0, "{best} {rival}");
    for order in ["big5", "uni", "uni_big5", "cns1"] {
        assert!(!cjk_outputs(order, '\u{3000}'), "{order}");
    }
}

/// The punctuation row is read only at the orders and offsets whose punctuation was measured
/// right; ideographs are read at every offset.
#[test]
fn punctuation_is_read_at_measured_offsets_only() {
    let comma = position("big5", '，');
    for (offset, measured) in [(499, true), (915, true), (514, false), (4_439, false), (320, false)] {
        let number = comma + u32::try_from(offset).unwrap();
        assert_eq!(cjk_output("big5", offset, number), measured.then_some('，'), "{offset}");
        let ideograph = position("big5", '中') + u32::try_from(offset).unwrap();
        assert_eq!(cjk_output("big5", offset, ideograph), Some('中'));
    }
    assert_eq!(cjk_output("cns1", 0, position("cns1", '，')), Some('，'));
}

/// A code whose glyph the embedded program doesn't have reads nothing, whatever its number reads
/// as: a CID outside a CFF program's charset, a glyph id past a TrueType program's glyphs. The
/// page draws nothing there. The codes still count as shown for the font's reading.
#[test]
fn a_code_without_a_glyph_reads_nothing() {
    let drawn = [glyphs("big5", 499, TEXT), glyphs("big5", 499, MORE)].concat();
    // Common characters too, in the same order, but not in the program.
    let missing: Vec<u16> = glyphs("big5", 499, "營業收入合併財務報告").into_iter().filter(|g| !drawn.contains(g)).collect();
    assert!(!missing.is_empty());
    let codes = [drawn.clone(), missing.clone()].concat();
    let doc = Doc { fonts: vec![Font::cff("ABCDEF+Kai", drawn.clone())], pages: vec![vec![Show::Text(0, codes.clone())]] };
    assert_eq!(recovered(&doc), format!("{TEXT}{MORE}"));

    // A TrueType program of `glyphs` glyphs: codes from there on draw nothing.
    let glyph_count = drawn.iter().copied().max().unwrap() + 1;
    let past: Vec<u16> = (0..10).map(|k| glyph_count + 2 * k).collect();
    let doc = Doc { fonts: vec![Font::truetype("ABCDEF+Kai", glyph_count)], pages: vec![vec![Show::Text(0, [drawn.clone(), past].concat())]] };
    assert_eq!(recovered(&doc), format!("{TEXT}{MORE}"));
}

/// The output counts the score uses are the same counted run by run (small fonts) and by the
/// Fourier transform (large ones), at every offset of every order.
#[test]
fn both_ways_of_counting_output_glyphs_agree() {
    let mut state = 0x2545_F491_u64;
    let mut next = move |m: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % m
    };
    for size in [3, 40, 700] {
        let mut numbers: Vec<(u32, u64)> = (0..size).map(|_| (u32::try_from(next(30_000)).unwrap(), 1 + next(9))).collect();
        numbers.sort_unstable();
        numbers.dedup_by_key(|x| x.0);
        assert!(cjk_output_counts_agree(&numbers), "{size}");
    }
    assert!(cjk_output_counts_agree(&ones("uni", -12_376, TEXT)));
    assert!(cjk_output_counts_agree(&[(0, 1), (65_535, 3)]));
}
