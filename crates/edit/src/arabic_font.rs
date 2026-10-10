//! The face Arabic text is drawn with, for added and edited text.
//!
//! In order of preference:
//! 1. the family the user picked, when it is installed;
//! 2. a font program embedded in the document that has the characters and Arabic shaping rules
//!    (subsets usually lack the letters new text needs, or the rules, and are passed over);
//! 3. the installed font closest to the document's own fonts by name ("ABCDEF+TraditionalArabic"
//!    → Traditional Arabic), then PdfCraft's defaults for serif or sans text;
//! 4. the craft-fonts `Arab` face, when the build has one.
//!
//! The first that has every character wins; when none has them all, the best one is returned, so
//! drawing names the character it lacks.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, Object};
use pdfcraft_fonts::pdf::Metrics;
use pdfcraft_fonts::{ArabicFace, FaceShaper, GlyphError, ShapedCluster, arabic_candidates};
use unicode_bidi::{Level, ParagraphBidiInfo};

/// Largest embedded font program looked at.
const MAX_PROGRAM: usize = 32 << 20;
/// Installed faces tried for coverage before giving up on the installed fonts.
const MAX_TRIED: usize = 12;
/// Embedded programs looked at on one page.
const MAX_EMBEDDED: usize = 32;

/// What the text asks for.
#[derive(Clone, Debug, Default)]
pub(crate) struct Want<'a> {
    /// A family the user picked.
    pub requested: Option<&'a str>,
    /// Font names that hint at the document's own Arabic font, best first.
    pub hints: Vec<String>,
    /// Faces embedded in the document, best first.
    pub embedded: Vec<ArabicFace>,
    pub serif: bool,
    pub bold: bool,
}

/// The face for text needing `chars`, or `None` when no Arabic face exists at all.
pub(crate) fn choose(want: &Want, chars: &[char]) -> Option<ArabicFace> {
    let hints: Vec<&str> = want.hints.iter().map(String::as_str).collect();
    let installed = arabic_candidates(want.requested, &hints, want.serif, want.bold);
    let requested = want.requested.and_then(|r| installed.iter().find(|f| f.family.eq_ignore_ascii_case(r)).copied());
    let mut first: Option<ArabicFace> = None;
    let mut consider = |face: ArabicFace| -> Option<ArabicFace> {
        if face.covers(chars) {
            return Some(face);
        }
        first.get_or_insert(face);
        None
    };
    if let Some(face) = requested.and_then(|f| f.load()).and_then(&mut consider) {
        return Some(face);
    }
    for face in &want.embedded {
        let usable = face.shaper().is_ok_and(|s| s.can_join());
        if usable && face.covers(chars) {
            return Some(face.clone());
        }
    }
    for f in installed.iter().take(MAX_TRIED) {
        if let Some(face) = f.load().and_then(&mut consider) {
            return Some(face);
        }
    }
    if let Some(face) = ArabicFace::craft().and_then(&mut consider) {
        return Some(face);
    }
    first
}

/// The characters of `text` a face must have to draw it all: everything but direction marks,
/// tabs and line breaks.
pub(crate) fn needed(text: &str) -> Vec<char> {
    let mut out: Vec<char> = text.chars().filter(|c| !is_bidi_control(*c) && !matches!(c, '\t' | '\n' | '\r')).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// A line of text in drawing order, all of it shaped with `face`: the Unicode bidirectional
/// algorithm (UAX #9) orders its runs, the shaper joins and mirrors within each. `rtl` is the
/// paragraph's direction.
pub(crate) fn layout(face: &FaceShaper, line: &str, rtl: bool) -> Result<Vec<ShapedCluster>, GlyphError> {
    // unicode-bidi indexes the first level of a line, so an empty one would panic.
    if line.is_empty() {
        return Ok(Vec::new());
    }
    let base = if rtl { Level::rtl() } else { Level::ltr() };
    let info = ParagraphBidiInfo::new(line, Some(base));
    let (levels, runs) = info.visual_runs(0..line.len());
    let mut out = Vec::new();
    for run in runs {
        let run_rtl = levels.get(run.start).is_some_and(Level::is_rtl);
        // Runs lie on character boundaries; a missing slice is skipped, never a panic.
        let Some(text) = line.get(run) else { continue };
        let text: String = text.chars().filter(|c| !is_bidi_control(*c)).collect();
        if !text.is_empty() {
            out.extend(face.shape(&text, run_rtl)?);
        }
    }
    Ok(out)
}

/// Whether `text` reads right to left (its first strong character does).
pub(crate) fn base_rtl(text: &str) -> bool {
    unicode_bidi::get_base_direction(text) == unicode_bidi::Direction::Rtl
}

/// Arabic letters (not digits or punctuation): a strong right-to-left character.
pub(crate) fn is_arabic_letter(c: char) -> bool {
    matches!(u32::from(c), 0x0620..=0x065F | 0x066E..=0x06D3 | 0x06D5..=0x06FF | 0x0750..=0x077F | 0x0870..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFC)
}

/// Text read from a PDF in drawing order (`units` are the glyphs' texts, left to right) put back in
/// reading order, with Arabic presentation forms turned back into letters. Lines without Arabic
/// come back as drawn. A glyph's own text (a ligature's letters) is never reversed.
///
/// This undoes the Unicode bidirectional algorithm for one line without knowing its paragraph:
/// the line reads right to left when it starts and ends (on screen) with Arabic, left to right when
/// it starts and ends with Latin, and otherwise by which script has the larger share. Inside a
/// right-to-left line, Latin words (with the numbers and spaces between them) and numbers keep
/// their own order; inside a left-to-right line, Arabic words (with the numbers between them) turn
/// around and their numbers don't.
pub(crate) fn logical(units: &[String]) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum K {
        /// Arabic letters.
        R,
        /// Other letters.
        L,
        /// Digits of any script.
        D,
        /// Spaces and punctuation.
        N,
    }
    let kind = |u: &str| {
        if u.chars().any(is_arabic_letter) {
            K::R
        } else if u.chars().any(char::is_alphabetic) {
            K::L
        } else if u.chars().any(|c| c.is_numeric()) {
            K::D
        } else {
            K::N
        }
    };
    let kinds: Vec<K> = units.iter().map(|u| kind(u)).collect();
    if !kinds.contains(&K::R) {
        return units.concat();
    }
    let strong = |k: &&K| matches!(k, K::R | K::L);
    let first = kinds.iter().find(strong).copied();
    let last = kinds.iter().rev().find(strong).copied();
    let rtl = match (first, last) {
        (Some(K::R), Some(K::R)) => true,
        (Some(K::L), Some(K::L)) => false,
        _ => {
            let letters = |want: K| units.iter().zip(&kinds).filter(|(_, k)| **k == want).map(|(u, _)| u.chars().count()).sum::<usize>();
            letters(K::R) * 2 >= letters(K::L)
        }
    };
    // In reading order: a right-to-left line read from its right end.
    let mut order: Vec<usize> = (0..units.len()).collect();
    if rtl {
        order.reverse();
    }
    let kind_at = |order: &[usize], pos: usize| order.get(pos).and_then(|i| kinds.get(*i)).copied().unwrap_or(K::N);
    // Runs of the other direction: from one of its letters to its last letter before a letter of
    // the paragraph's own direction, with what lies between.
    let other = if rtl { K::L } else { K::R };
    let own = if rtl { K::R } else { K::L };
    let mut inside = vec![false; order.len()];
    let mut i = 0;
    while i < order.len() {
        if kind_at(&order, i) != other {
            i += 1;
            continue;
        }
        let mut end = i;
        let mut j = i + 1;
        while j < order.len() && kind_at(&order, j) != own {
            if kind_at(&order, j) == other {
                end = j;
            }
            j += 1;
        }
        if let Some(run) = order.get_mut(i..=end) {
            run.reverse();
        }
        for flag in inside.iter_mut().take(end + 1).skip(i) {
            *flag = true;
        }
        i = end + 1;
    }
    // Numbers read left to right: turn back each run of digits (with the separators inside it) that
    // now reads backwards, those in right-to-left stretches.
    let rtl_at = |pos: usize, inside: &[bool]| inside.get(pos).copied().unwrap_or(false) != rtl;
    let mut i = 0;
    while i < order.len() {
        if kind_at(&order, i) != K::D || !rtl_at(i, &inside) {
            i += 1;
            continue;
        }
        let mut end = i;
        let mut j = i + 1;
        while j < order.len() && rtl_at(j, &inside) {
            match kind_at(&order, j) {
                K::D => {
                    end = j;
                    j += 1;
                }
                // One separator between digits: 1,250.75 or 2026/10.
                K::N if order.get(j).and_then(|k| units.get(*k)).is_some_and(|u| matches!(u.as_str(), "," | "." | ":" | "/" | "-" | "٫" | "٬"))
                    && kind_at(&order, j + 1) == K::D =>
                {
                    j += 1
                }
                _ => break,
            }
        }
        if let Some(run) = order.get_mut(i..=end) {
            run.reverse();
        }
        i = end + 1;
    }
    // Brackets drawn in a right-to-left stretch are mirrored glyphs: their text is the other bracket.
    let mut out = String::new();
    for (pos, i) in order.iter().enumerate() {
        let Some(u) = units.get(*i) else { continue };
        if kinds.get(*i) == Some(&K::N) && rtl_at(pos, &inside) {
            out.extend(u.chars().map(mirror));
        } else {
            out.push_str(u);
        }
    }
    let text = unpresent(&out);
    // A right-to-left line whose reading starts with a Latin word or a number: a leading
    // right-to-left mark keeps its direction when it is retyped or restyled (the mark isn't drawn).
    let first_strong = text.chars().find(|c| c.is_alphabetic());
    if rtl && first_strong.is_some_and(|c| !is_arabic_letter(c)) { format!("\u{200F}{text}") } else { text }
}

fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        other => other,
    }
}

/// Arabic presentation forms (fixed joining forms and ligatures some PDFs map their glyphs to) as
/// plain letters, so retyped text joins again: their Unicode compatibility mapping (NFKC).
pub(crate) fn unpresent(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization as _;
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match u32::from(c) {
            // Initial and medial yeh: fonts share the dotted glyph between Arabic yeh and Farsi yeh,
            // and Arabic text means yeh.
            0xFBFE | 0xFBFF => out.push('\u{064A}'),
            // Isolated harakat map to a space and the mark; the space isn't text.
            0xFB50..=0xFDFF | 0xFE70..=0xFEFF => out.extend(c.to_string().nfkc().filter(|c| *c != ' ')),
            _ => out.push(c),
        }
    }
    out
}

/// Direction marks, embeddings, overrides and isolates: they steer the order and are not drawn.
pub(crate) fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// The document's fonts in `fonts` (a page's `/Font` resources) as hints and embedded faces. Only
/// fonts that show Arabic count (their embedded program has Arabic letters, or their ToUnicode map
/// gives Arabic text): a page's Courier or Arial headings must not choose the Arabic font. Fonts
/// with an embedded Arabic program come first.
pub(crate) fn from_page_fonts(doc: &Document, fonts: &Dict, want: &mut Want) {
    let mut embedded_names = Vec::new();
    let mut mapped_names = Vec::new();
    for (_, f) in fonts.iter().take(256) {
        let f = doc.resolve(f);
        let Some(dict) = f.as_dict() else { continue };
        let name = base_font(doc, dict);
        match (want.embedded.len() < MAX_EMBEDDED).then(|| embedded_face(doc, dict, &name)).flatten() {
            Some(face) => {
                want.embedded.push(face);
                embedded_names.push(name);
            }
            None if Metrics::from_dict(doc, dict).maps_arabic() => mapped_names.push(name),
            None => {}
        }
    }
    for name in embedded_names.into_iter().chain(mapped_names) {
        if !name.is_empty() && !want.hints.contains(&name) {
            want.hints.push(name);
        }
    }
}

/// The text of each glyph of the page font `font`, read from the installed font it is a subset
/// of, when its codes are that font's glyph ids: a Type 0 font in Identity order (no glyph map)
/// whose embedded program has as many glyphs as the installed font of its name and weight. Word
/// keeps glyph ids when it subsets, and its ToUnicode maps can give several Arabic glyphs one
/// wrong letter; the installed font's character map and substitutions name them right. `None`
/// otherwise.
pub(crate) fn glyph_texts(doc: &Document, font: &Dict) -> Option<Arc<std::collections::HashMap<u32, String>>> {
    if font.name(b"Subtype") != Some(b"Type0") || !matches!(font.name(b"Encoding"), Some(b"Identity-H" | b"Identity-V")) {
        return None;
    }
    let cid = descendant(doc, font)?;
    if cid.get(b"CIDToGIDMap").is_some_and(|m| doc.resolve(m).as_name() != Some(b"Identity")) {
        return None;
    }
    let name = base_font(doc, font);
    let family = pdfcraft_fonts::arabic_font_family(&name)?;
    let descriptor = doc.resolve(cid.get(b"FontDescriptor")?);
    let file = doc.resolve(descriptor.as_dict()?.get(b"FontFile2")?);
    let Object::Stream(s) = &*file else { return None };
    let embedded = pdfcraft_fonts::program_glyph_count(&s.decoded_within(MAX_PROGRAM).ok()?)?;
    let bold = ["bold", "black", "heavy", "semibold", "demi"].iter().any(|w| name.to_lowercase().contains(w));
    let installed = arabic_candidates(Some(family), &[], false, bold).into_iter().next()?.load()?;
    if installed.glyph_count()? != embedded {
        return None;
    }
    // Naming a face's glyphs shapes thousands of letter pairs: once per installed face.
    type Texts = Arc<std::collections::HashMap<u32, String>>;
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Vec<(String, String, Texts)>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    // A poisoned lock only means another thread panicked mid-update; the list is still usable.
    let mut cache = cache.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, _, texts)) = cache.iter().find(|(f, s, _)| *f == installed.family && *s == installed.style) {
        return Some(texts.clone());
    }
    let texts: Texts = Arc::new(installed.glyph_texts());
    if cache.len() >= 16 {
        cache.remove(0);
    }
    cache.push((installed.family.clone(), installed.style.clone(), texts.clone()));
    Some(texts)
}

/// A font's `/BaseFont` (for a Type 0 font, its descendant's), or "".
pub(crate) fn base_font(doc: &Document, font: &Dict) -> String {
    if let Some(name) = font.name(b"BaseFont").filter(|n| !n.is_empty()) {
        return String::from_utf8_lossy(name).into_owned();
    }
    descendant(doc, font).and_then(|d| d.name(b"BaseFont").map(|n| String::from_utf8_lossy(n).into_owned())).unwrap_or_default()
}

fn descendant(doc: &Document, font: &Dict) -> Option<Dict> {
    let list = doc.resolve(font.get(b"DescendantFonts")?);
    let first = list.as_array()?.first()?.clone();
    doc.resolve(&first).as_dict().cloned()
}

/// The font program embedded for `font` as an Arabic face, when it has Arabic letters.
pub(crate) fn embedded_face(doc: &Document, font: &Dict, name: &str) -> Option<ArabicFace> {
    // A subset (`ABCDEF+Name`) keeps only the glyphs and shaping rules its text used, so new text
    // may lose its joins; the installed font of that name looks the same and has them all.
    let subset = name.split_once('+').is_some_and(|(tag, _)| tag.len() == 6 && tag.chars().all(|c| c.is_ascii_uppercase()));
    if subset && pdfcraft_fonts::arabic_font_family(name).is_some() {
        return None;
    }
    let holder = if font.name(b"Subtype") == Some(b"Type0") { descendant(doc, font)? } else { font.clone() };
    let descriptor = doc.resolve(holder.get(b"FontDescriptor")?);
    let descriptor = descriptor.as_dict()?;
    let file = match (descriptor.get(b"FontFile2"), descriptor.get(b"FontFile3")) {
        (Some(f), _) => doc.resolve(f),
        (None, Some(f)) => {
            let f = doc.resolve(f);
            // CFF without an OpenType wrapper has no cmap or shaping rules to use.
            if f.as_dict().and_then(|d| d.name(b"Subtype")) != Some(b"OpenType") {
                return None;
            }
            f
        }
        _ => return None,
    };
    let Object::Stream(s) = &*file else { return None };
    let bytes: Arc<[u8]> = s.decoded_within(MAX_PROGRAM).ok()?.into();
    let bold = ["bold", "black", "heavy", "semibold", "demi"].iter().any(|w| name.to_lowercase().contains(w))
        || descriptor.get(b"FontWeight").and_then(Object::as_f64).is_some_and(|w| w >= 600.0);
    let family = name.split_once('+').filter(|(tag, _)| tag.len() == 6).map_or(name, |(_, rest)| rest).to_string();
    ArabicFace::from_shared(family, if bold { "Bold".into() } else { "Regular".into() }, bold, bytes, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needed_characters_skip_controls_and_repeat_once() {
        assert_eq!(needed("ب\u{200F}ب a\n"), vec![' ', 'a', 'ب']);
        assert!(needed("").is_empty());
    }

    #[test]
    fn junk_font_programs_are_passed_over() {
        let mut doc = Document::new_empty();
        let mut file = pdfcraft_cos::Stream::from_raw(Dict::new(), b"not a font".to_vec());
        file.dict.set(b"Length".to_vec(), Object::Int(10));
        let file = doc.add(Object::Stream(file));
        let mut descriptor = Dict::new();
        descriptor.set(b"FontFile2".to_vec(), Object::Ref(file));
        let mut font = Dict::new();
        font.set(b"Subtype".to_vec(), Object::name("TrueType"));
        font.set(b"BaseFont".to_vec(), Object::name("ABCDEF+TraditionalArabic"));
        font.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
        let mut fonts = Dict::new();
        fonts.set(b"F1".to_vec(), Object::Dict(font));
        // A Type 0 font whose descendant list is broken.
        let mut type0 = Dict::new();
        type0.set(b"Subtype".to_vec(), Object::name("Type0"));
        type0.set(b"DescendantFonts".to_vec(), Object::Int(3));
        fonts.set(b"F2".to_vec(), Object::Dict(type0));
        fonts.set(b"F3".to_vec(), Object::Int(7));
        let mut want = Want::default();
        from_page_fonts(&doc, &fonts, &mut want);
        assert!(want.embedded.is_empty());
        assert!(want.hints.is_empty(), "nothing shows Arabic: {:?}", want.hints);
        // A font whose ToUnicode map gives Arabic names the document's Arabic font; a Latin one doesn't.
        let cmap = b"begincmap\n1 begincodespacerange\n<00> <FF>\nendcodespacerange\n1 beginbfchar\n<41> <0627>\nendbfchar\nendcmap\n";
        let mut cmap_stream = pdfcraft_cos::Stream::from_raw(Dict::new(), cmap.to_vec());
        cmap_stream.dict.set(b"Length".to_vec(), Object::Int(cmap.len() as i64));
        let cmap = doc.add(Object::Stream(cmap_stream));
        let mut arabic = Dict::new();
        arabic.set(b"Subtype".to_vec(), Object::name("TrueType"));
        arabic.set(b"BaseFont".to_vec(), Object::name("SimplifiedArabic,Bold"));
        arabic.set(b"ToUnicode".to_vec(), Object::Ref(cmap));
        fonts.set(b"F4".to_vec(), Object::Dict(arabic));
        let mut courier = Dict::new();
        courier.set(b"Subtype".to_vec(), Object::name("Type1"));
        courier.set(b"BaseFont".to_vec(), Object::name("Courier"));
        fonts.set(b"F5".to_vec(), Object::Dict(courier));
        let mut want = Want::default();
        from_page_fonts(&doc, &fonts, &mut want);
        assert_eq!(want.hints, vec!["SimplifiedArabic,Bold".to_string()]);
    }

    /// Glyph texts in drawing order, one per character of `visual`.
    fn units(visual: &str) -> Vec<String> {
        visual.chars().map(String::from).collect()
    }

    #[test]
    fn drawn_arabic_reads_in_logical_order() {
        // "مرحبا 2026" drawn left to right: the number, a space, then the word from its last letter.
        assert_eq!(logical(&units("2026 ابحرم")), "مرحبا 2026");
        // Latin words and their spaces stay in their own order inside a right-to-left line.
        assert_eq!(logical(&units("PDF report - 2026 ريرقت")), "تقرير 2026 - PDF report");
        // Mirrored brackets read as the brackets they are.
        assert_eq!(logical(&units(".(USD) رالودلاب")), "بالدولار (USD).");
        // A left-to-right line keeps its order; only its Arabic word turns around.
        assert_eq!(logical(&units("Total: ابحرم and more")), "Total: مرحبا and more");
        // A ligature's own letters are never reversed.
        assert_eq!(logical(&["ه".to_string(), "لل".to_string(), "ا".to_string()]), "الله");
        // A right-to-left line whose reading starts with a Latin word keeps its direction.
        let text = logical(&units("نع ريرقت PDF"));
        assert_eq!(text, "\u{200F}PDF تقرير عن");
        assert!(base_rtl(&text));
        assert_eq!(logical(&[]), "");
        assert_eq!(logical(&units("plain text")), "plain text");
    }

    #[test]
    fn presentation_forms_turn_back_into_letters() {
        // Initial meem, final reh, medial hah... as some PDFs map their glyphs.
        assert_eq!(unpresent("\u{FEE3}\u{FEAE}\u{FEA3}\u{FE92}\u{FE8E}"), "مرحبا");
        // Lam-alef and the Allah ligature.
        assert_eq!(unpresent("\u{FEFB}"), "\u{0644}\u{0627}");
        assert_eq!(unpresent("\u{FDF2}"), "\u{0627}\u{0644}\u{0644}\u{0647}");
        // A ligature from Forms-A (lam with meem, initial).
        assert_eq!(unpresent("\u{FCCC}"), "\u{0644}\u{0645}");
        // Isolated fatha: the mark, without the space its compatibility mapping adds.
        assert_eq!(unpresent("\u{FE76}"), "\u{064E}");
        // Initial yeh shared with Farsi yeh.
        assert_eq!(unpresent("\u{FBFE}"), "\u{064A}");
        assert_eq!(unpresent("abc ١٢٣"), "abc ١٢٣");
    }

    #[test]
    fn layout_orders_runs_and_survives_odd_input() {
        let Some(face) = choose(&Want::default(), &needed("مرحبا PDF")) else {
            eprintln!("skipping: no Arabic face on this machine or in this build");
            return;
        };
        let shaper = face.shaper().unwrap();
        let drawn: Vec<String> = layout(&shaper, "مرحبا PDF", true).unwrap().into_iter().map(|c| c.text).collect();
        // Right to left: "PDF" is drawn first (leftmost), then the Arabic word from its last letter.
        assert_eq!(drawn.first().map(String::as_str), Some("P"));
        assert_eq!(drawn.last().map(String::as_str), Some("م"));
        assert!(layout(&shaper, "", true).unwrap().is_empty());
        for odd in ["\u{202E}\u{200F}", "\u{064B}\u{064B}", " ", "(]"] {
            let _ = layout(&shaper, odd, true);
            let _ = layout(&shaper, odd, false);
        }
    }
}
