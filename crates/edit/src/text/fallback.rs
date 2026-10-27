use super::interp::Ts;
use super::lines::page_dict;
use super::*;

/// Text → the runs that show it: each run's font resource name and the bytes shown in that font.
pub(super) type Encoder = Box<dyn Fn(&str) -> Option<Vec<(String, Vec<u8>)>>>;

fn is_win_ansi_char(c: char) -> bool {
    c == '\t' || pdfcraft_fonts::win_ansi_byte(c).is_some()
}

pub(crate) fn needs_type3(text: &str) -> bool {
    text.chars().any(|c| !is_win_ansi_char(c))
}

/// The `/Name` of the Type 3 fonts generated for Japanese text.
const JAPANESE_FONT_NAME: &str = "PCJapanese";

/// The `/Name` of the Type 3 fonts generated for Hebrew text.
const HEBREW_FONT_NAME: &str = "PCHebrew";

/// The `/Name` of the Type 3 fonts generated from the bundled Latin (Inter) face.
const LATIN_FONT_NAME: &str = "PCLatin";

/// Whether `font` is one of those generated fonts (Latin, Japanese or Hebrew): a replacement longer
/// than one of them is split across several, which read back as one line (see [`same_font`]).
pub(super) fn is_type3_fallback(font: &Dict) -> bool {
    font.name(b"Subtype") == Some(b"Type3")
        && font
            .name(b"Name")
            .is_some_and(|n| n == JAPANESE_FONT_NAME.as_bytes() || n == HEBREW_FONT_NAME.as_bytes() || n == LATIN_FONT_NAME.as_bytes())
}

/// The generated font's `/Name` and its resource prefix: the bundled Latin face, Hebrew, or the
/// Japanese ones.
fn fallback_names(face: &CraftFont, hebrew: bool) -> (&'static str, &'static str) {
    if hebrew {
        (HEBREW_FONT_NAME, "PCHe")
    } else if face.family == "Inter" {
        (LATIN_FONT_NAME, "PCLat")
    } else {
        (JAPANESE_FONT_NAME, "PCJp")
    }
}

/// Drops the generated fallback fonts ([`is_type3_fallback`]) that nothing on `page` shows any more.
/// Each rewrite generates its own, with the outlines of every character it draws, so without this
/// an earlier rewrite's fonts stay in the page's resources and the file. Only a font dictionary the
/// page owns is changed (a rewrite gives the page its own), and nothing is dropped unless every
/// content stream that may name the page's fonts can be read: the page's own, and those of form
/// XObjects and Type 3 glyphs that have no resources of their own and so use the page's.
pub(super) fn drop_unused_fallback_fonts(doc: &mut Document, page: usize) {
    let Ok(p) = page_dict(doc, page) else { return };
    let Some(Object::Dict(mut resources)) = p.dict.get(b"Resources").cloned() else { return };
    let Some(mut fonts) = resources.get(b"Font").and_then(|f| doc.dict(f)) else { return };
    let generated: Vec<Vec<u8>> =
        fonts.iter().filter(|(_, f)| doc.dict(f).is_some_and(|d| is_type3_fallback(&d))).map(|(name, _)| name.clone()).collect();
    if generated.is_empty() {
        return;
    }
    let Some(shown) = fonts_shown(doc, &p.dict, &resources) else { return };
    let unused: Vec<&Vec<u8>> = generated.iter().filter(|name| !shown.contains(*name)).collect();
    if unused.is_empty() {
        return;
    }
    for name in unused {
        fonts.remove(name);
    }
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    // Nothing is lost if this fails: the fonts just stay.
    let _ = doc.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(resources)));
}

/// The font resource names `Tf` selects in the page's content and in the content that uses the
/// page's resources (see [`drop_unused_fallback_fonts`]), or `None` if any of it can't be read.
fn fonts_shown(doc: &Document, page: &Dict, resources: &Dict) -> Option<std::collections::HashSet<Vec<u8>>> {
    let mut streams: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    // Form XObjects without their own resources use the page's, however deeply they are nested
    // (ISO 32000-1 §7.8.3), so every form the page can reach is visited.
    let mut pending: Vec<Dict> = vec![resources.clone()];
    let mut seen = std::collections::HashSet::new();
    while let Some(res) = pending.pop() {
        for (_, x) in res.get(b"XObject").and_then(|x| doc.dict(x)).unwrap_or_default().iter() {
            let Some(r) = x.as_ref() else { continue };
            if !seen.insert(r) {
                continue;
            }
            if seen.len() > 4096 {
                return None;
            }
            let Object::Stream(s) = &*doc.get(r) else { continue };
            if s.dict.name(b"Subtype") != Some(b"Form") {
                continue;
            }
            match s.dict.get(b"Resources").and_then(|r| doc.dict(r)) {
                Some(own) => pending.push(own),
                None => streams.push(x.clone()),
            }
        }
    }
    // And so do Type 3 glyph procedures without their own (the generated fonts' are outlines only).
    for (_, f) in resources.get(b"Font").and_then(|f| doc.dict(f)).unwrap_or_default().iter() {
        if let Some(font) = doc.dict(f).filter(|d| d.name(b"Subtype") == Some(b"Type3") && !d.contains(b"Resources") && !is_type3_fallback(d)) {
            streams.extend(font.get(b"CharProcs").and_then(|c| doc.dict(c)).unwrap_or_default().iter().map(|(_, proc)| proc.clone()));
        }
    }
    let mut shown = std::collections::HashSet::new();
    for stream in streams {
        let Object::Stream(s) = &*doc.resolve(&stream) else { return None };
        // Strict: a stream only partly read might name a font in the part that wasn't.
        let data = s.decoded_strict().ok()?;
        for op in parse(&data).ops {
            if op.is("Tf")
                && let Some(name) = op.operands.first().and_then(|n| n.as_name())
            {
                shown.insert(name.to_vec());
            }
        }
    }
    Some(shown)
}

/// Whether a run shown in `font` (a resource name), with `/BaseFont` `base_font` and the Japanese
/// or Hebrew fallback face it subsets, if any, continues `line`: the same font resource, another
/// resource for the same font (a writer may split one font into several subsets, #3661), or
/// another subset of the same face.
pub(crate) fn same_font(line: &TextLine, font: &[u8], base_font: &str, face: Option<&str>) -> bool {
    line.font.as_bytes() == font || same_base_font(&line.base_font, base_font) || (line.face.is_some() && line.face.as_deref() == face)
}

/// Whether a run painted with `ts` continues `line` as it is painted: a line is rewritten in the
/// state of its first run, so a `Tr`, stroke colour or line width that changes partway starts
/// another run (see [`TextState::paint_ops`]; the `gs` parameters are compared as [`Paint`]).
/// The stroke colour, width, dash and cap only count for text that is stroked, so a rule drawn
/// between two words doesn't split their line. They are compared as values, wherever they were
/// set: two ExtGStates with the same settings under other names still continue a line.
///
/// Only the visible modes (`Tr` 0–2) that `paint_ops` restates are compared: a line that goes on
/// as invisible or clipping text stays one line, which the clipping refusal reads whole (#3447).
pub(super) fn same_paint(line: &TextLine, ts: &Ts) -> bool {
    let a = &line.origin.state;
    let visible = |m: i64| (0..=2).contains(&m);
    let same_mode = a.render_mode == ts.render_mode || !visible(a.render_mode) || !visible(ts.render_mode);
    let stroked = matches!((a.render_mode, ts.render_mode), (1 | 2, 1 | 2));
    let outline = a.stroke == ts.stroke && a.line_width == ts.line_width && a.dash == ts.dash && a.line_cap == ts.line_cap;
    same_mode && (!stroked || outline)
}

/// Whether two `/BaseFont` names name the same font: a subset's six-letter tag and `+` prefix do
/// not change the font (ISO 32000-2 §9.6.4), so `ABCDEF+Arial` and `GHIJKL+Arial` are one font.
/// A name that is missing, or only a tag (`ABCDEF+`), names no font and matches nothing.
pub(crate) fn same_base_font(a: &str, b: &str) -> bool {
    let (a, b) = (strip_subset_tag(a), strip_subset_tag(b));
    !a.is_empty() && a == b
}

/// `ABCDEF+Arial` → `Arial`. A name without a subset tag is returned unchanged.
pub(crate) fn strip_subset_tag(name: &str) -> &str {
    let bytes = name.as_bytes();
    let is_tag = bytes.get(6) == Some(&b'+') && bytes.get(..6).is_some_and(|tag| tag.iter().all(u8::is_ascii_uppercase));
    if is_tag { name.get(7..).unwrap_or(name) } else { name }
}

pub(crate) fn source_family(base_font: &str) -> crate::added::Family {
    let name = base_font.to_ascii_lowercase();
    if ["courier", "mono", "consolas", "menlo", "monaco", "lucida console"].iter().any(|s| name.contains(s)) {
        crate::added::Family::Courier
    } else if !name.contains("sans")
        && ["times", "serif", "roman", "mincho", "cambria", "georgia", "palatino", "garamond"].iter().any(|s| name.contains(s))
    {
        crate::added::Family::Times
    } else {
        crate::added::Family::Helvetica
    }
}

pub(crate) const MAX_TYPE3_GLYPHS: usize = 240;

/// The generated fonts for one replacement: one per run of at most `max_glyphs` unique characters,
/// in order of first appearance.
#[derive(Clone)]
pub(crate) struct Type3Fallback {
    /// The family its glyphs come from ("Shippori Mincho").
    pub(crate) family: &'static str,
    /// The fonts' resource names in order; the first is `PCJp` (`PCLat` for the bundled Latin
    /// face, `PCHe` for Hebrew text).
    pub(crate) names: Vec<String>,
    /// Every unique character with its advance (em), in order of first appearance.
    pub(super) widths: Vec<(char, f64)>,
    /// Where each character is shown: the index of its font in `names`, and its code there.
    pub(crate) codes: HashMap<char, (usize, u8)>,
}

/// One glyph of a generated font: its code there, its character, its glyph procedure and advance.
pub(crate) struct Type3Glyph {
    pub(crate) code: u8,
    pub(crate) ch: char,
    pub(crate) path: Vec<u8>,
    pub(crate) width: f64,
}

fn pdf_num(v: f64) -> String {
    if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v:.4}").trim_end_matches('0').trim_end_matches('.').to_string() }
}

fn type3_path(face: &CraftFont, ch: char) -> Result<(Vec<u8>, f64), EditError> {
    let glyph = japanese_glyph_from(face, ch).map_err(|e| match e {
        GlyphError::NoFont => no_japanese_font(),
        GlyphError::Missing => EditError::Invalid(format!("the {} fallback font has no glyph for U+{:04X}", face.family, ch as u32)),
        GlyphError::TooComplex => EditError::Invalid(format!("the {} fallback glyph U+{:04X} is too complex", face.family, ch as u32)),
    })?;
    let (dx, width) = proportional(ch, &glyph);
    let scale = 1000.0;
    // d1 is `wx wy llx lly urx ury` (ISO 32000-2 §9.6.4) with a box enclosing the glyph;
    // Acrobat draws a bullet in place of a glyph whose d1 is malformed.
    let b = if glyph.contours.is_empty() { [0.0; 4] } else { glyph.bbox };
    let mut out = format!(
        "{} 0 {} {} {} {} d1\n",
        pdf_num(width * scale),
        pdf_num(((b[0] + dx) * scale).floor()),
        pdf_num((b[1] * scale).floor()),
        pdf_num(((b[2] + dx) * scale).ceil()),
        pdf_num((b[3] * scale).ceil())
    )
    .into_bytes();
    for contour in glyph.contours {
        let Some(first) = contour.first() else { continue };
        out.extend_from_slice(format!("{} {} m\n", pdf_num((first[0] + dx) * scale), pdf_num(first[1] * scale)).as_bytes());
        for p in contour.iter().skip(1) {
            out.extend_from_slice(format!("{} {} l\n", pdf_num((p[0] + dx) * scale), pdf_num(p[1] * scale)).as_bytes());
        }
        out.extend_from_slice(b"h\n");
    }
    out.extend_from_slice(b"f\n");
    Ok((out, width))
}

/// Side bearing (em) given to a respaced glyph on each side.
const SIDE_BEARING: f64 = 0.05;

/// Shift and advance for a fallback glyph. Japanese faces draw Cyrillic and Greek full-width
/// (one em each, as in JIS X 0208) and have no proportional (`palt`) metrics for them, so a
/// word set that way reads as letter-spaced. Those letters get their ink width plus a side
/// bearing instead; everything else keeps the face's own advance.
fn proportional(ch: char, glyph: &pdfcraft_fonts::GlyphOutline) -> (f64, f64) {
    let respace = matches!(ch, '\u{0370}'..='\u{03FF}' | '\u{0400}'..='\u{052F}' | '\u{1F00}'..='\u{1FFF}');
    let ink = glyph.bbox[2] - glyph.bbox[0];
    if !respace || glyph.contours.is_empty() || !(ink > 0.0 && ink < glyph.width) {
        return (0.0, glyph.width);
    }
    (SIDE_BEARING - glyph.bbox[0], ink + 2.0 * SIDE_BEARING)
}

fn unicode_hex(ch: char) -> String {
    let mut units = [0u16; 2];
    let encoded = ch.encode_utf16(&mut units);
    encoded.iter().map(|u| format!("{u:04X}")).collect()
}

/// The same outline for a face that may simply lack the glyph: `None` then, so the caller can try
/// the next face (and report the character once none of them has it).
pub(crate) fn type3_glyph(face: &CraftFont, ch: char) -> Result<Option<(Vec<u8>, f64)>, EditError> {
    match japanese_glyph_from(face, ch) {
        Err(GlyphError::Missing) => Ok(None),
        _ => type3_path(face, ch).map(Some),
    }
}

/// No face this build has can draw `text`. Only Japanese text is about the Japanese face: other
/// characters are missing from the paragraph's font, and choosing a font that has them works too.
pub(crate) fn no_fallback_font(text: &str) -> EditError {
    if text.chars().any(needs_japanese) {
        return no_japanese_font();
    }
    let Some(ch) = text.chars().find(|c| !c.is_whitespace() && !is_win_ansi_char(*c)) else { return no_japanese_font() };
    EditError::Invalid(format!(
        "the paragraph's font has no glyph for {ch} (U+{:04X}) and this build has no fallback font for it; \
         choose a font that has it (Load font… or Installed fonts…), or use an official release, which includes \
         PdfCraft's fallback fonts (to build them in, set CRAFT_FONTS_DIR to a craft-fonts checkout)",
        u32::from(ch)
    ))
}

/// The character the craft-fonts Japanese faces are for: kana, CJK ideographs and their compatibility
/// and full-width forms.
fn needs_japanese(ch: char) -> bool {
    matches!(u32::from(ch), 0x3000..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

/// The chosen face has no outline for `ch`, and neither has any other: say which character it is and
/// what the writer can do about it. A CJK character without a Japanese face in the build is about
/// that build input, which is the one answer that changes it.
pub(crate) fn no_glyph_for(face: &CraftFont, ch: char) -> EditError {
    if needs_japanese(ch) && pdfcraft_fonts::document_japanese_font().is_none() {
        return no_japanese_font();
    }
    EditError::Invalid(format!(
        "the {} fallback font has no glyph for {ch} (U+{:04X}), and no fallback face covers the text; \
         choose a font that has it (Load font… or Installed fonts…), or use an official release, which includes \
         PdfCraft's fallback fonts (to build them in, set CRAFT_FONTS_DIR to a craft-fonts checkout)",
        face.family,
        u32::from(ch)
    ))
}

/// This build has no Japanese face to draw replacement text with.
pub(super) fn no_japanese_font() -> EditError {
    EditError::Invalid(
        "this text needs PdfCraft's Japanese fallback font, which this build doesn't include \
         (official releases do; to build it in, set CRAFT_FONTS_DIR to a craft-fonts checkout)"
            .into(),
    )
}

/// The first of `faces` (best match first) that has a glyph for every one of `chars`, else the first.
pub(crate) fn fallback_face<'a>(faces: &[&'a CraftFont], chars: &[char], has_glyph: impl Fn(&CraftFont, char) -> bool) -> Option<&'a CraftFont> {
    faces.iter().copied().find(|face| chars.iter().all(|ch| has_glyph(face, *ch))).or_else(|| faces.first().copied())
}

/// This build has no Hebrew face to draw replacement text with.
fn no_hebrew_font() -> EditError {
    EditError::Invalid(
        "this text needs PdfCraft's Hebrew fallback font (Noto Sans Hebrew), which this build doesn't include \
         (to build it in, set CRAFT_FONTS_DIR to a craft-fonts checkout that has it)"
            .into(),
    )
}

/// The faces a Type 3 replacement may draw `text` with, best first. Text with Hebrew takes the
/// Hebrew face first (its Latin and digits too, so a line keeps one look), then the Japanese faces
/// and the bundled Latin one for anything it lacks; other text takes the Japanese faces, and the
/// bundled Latin face behind them, which covers the Latin, Greek and Cyrillic letters they lack
/// (#2593: the Japanese faces have no қ, ғ, ҳ, ә, ө, ү, ұ or һ).
pub(crate) fn type3_faces(text: &str, family: crate::added::Family, bold: bool) -> Result<(Vec<&'static CraftFont>, bool), EditError> {
    let japanese = pdfcraft_fonts::document_japanese_fonts_for_style(family == crate::added::Family::Times, bold);
    let latin = pdfcraft_fonts::document_latin_fonts(bold);
    if !text.chars().any(crate::bidi::is_hebrew) {
        return Ok((japanese.into_iter().chain(latin).collect(), false));
    }
    // Points are drawn on their letter with the font's positioning, which a one-glyph-per-
    // character Type 3 font can't do; refuse rather than print them beside the letter.
    if text.chars().any(crate::bidi::is_hebrew_mark) {
        return Err(EditError::Invalid("Hebrew points (niqqud) can't be written into a PDF yet; type the text without them".into()));
    }
    let hebrew = pdfcraft_fonts::document_hebrew_font(bold).ok_or_else(no_hebrew_font)?;
    Ok((std::iter::once(hebrew).chain(japanese).chain(latin).collect(), true))
}

pub(super) fn type3_font(
    doc: &mut Document,
    fonts_res: &mut Dict,
    text: &str,
    family: crate::added::Family,
    bold: bool,
    max_glyphs: usize,
) -> Result<Type3Fallback, EditError> {
    let (faces, hebrew) = type3_faces(text, family, bold)?;
    // Each unique character once, in order of first appearance.
    let mut chars = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for ch in text.chars() {
        if seen.insert(ch) {
            chars.push(ch);
        }
    }
    if chars.is_empty() {
        return Err(EditError::Invalid("replacement text is empty".into()));
    }
    // The faces differ in coverage (e.g. of Cyrillic), so use the best face that has every
    // character. When none has them all, the best face reports the character it lacks. Hebrew
    // text keeps the Hebrew face and takes each character it lacks from the next face that has it.
    let face = if hebrew { faces.first().copied() } else { fallback_face(&faces, &chars, |face, ch| japanese_glyph_from(face, ch).is_ok()) }
        .ok_or_else(|| no_fallback_font(text))?;
    let others: &[&CraftFont] = if hebrew { faces.get(1..).unwrap_or_default() } else { &[] };
    // Every outline is read before the document is touched, so a character that fails leaves it
    // as it was. Each font takes the next `per_font` characters; codes restart at 1 in each.
    let per_font = max_glyphs.clamp(1, MAX_TYPE3_GLYPHS);
    let mut glyphs = Vec::with_capacity(chars.len());
    for (i, ch) in chars.into_iter().enumerate() {
        // The chosen face first, then the faces after it (Hebrew text takes a character the Hebrew
        // face lacks from the next face that has it); a character no face draws is reported once.
        let mut drawn = type3_glyph(face, ch)?;
        for f in others.iter().copied() {
            if drawn.is_some() {
                break;
            }
            drawn = type3_glyph(f, ch)?;
        }
        let Some((path, width)) = drawn else { return Err(no_glyph_for(face, ch)) };
        let code = u8::try_from((i % per_font).saturating_add(1))
            .map_err(|_| EditError::Invalid("replacement text has too many unique characters".into()))?;
        glyphs.push(Type3Glyph { code, ch, path, width });
    }
    Ok(write_type3_glyphs(doc, fonts_res, face, hebrew, glyphs, per_font))
}

/// Write already validated outlines; callers can prepare every face before adding any objects.
pub(crate) fn write_type3_glyphs(
    doc: &mut Document,
    fonts_res: &mut Dict,
    face: &CraftFont,
    hebrew: bool,
    glyphs: Vec<Type3Glyph>,
    per_font: usize,
) -> Type3Fallback {
    let per_font = per_font.clamp(1, MAX_TYPE3_GLYPHS);
    let mut codes = HashMap::with_capacity(glyphs.len());
    let mut widths = Vec::with_capacity(glyphs.len());
    let mut names = Vec::new();
    let mut glyphs = glyphs.into_iter();
    loop {
        let group: Vec<Type3Glyph> = glyphs.by_ref().take(per_font).collect();
        if group.is_empty() {
            break;
        }
        for glyph in &group {
            codes.insert(glyph.ch, (names.len(), glyph.code));
            widths.push((glyph.ch, glyph.width));
        }
        names.push(add_type3_font(doc, fonts_res, face, hebrew, group));
    }
    Type3Fallback { family: face.family, names, widths, codes }
}

/// Adds one generated font whose codes 1 to `glyphs.len()` show `glyphs`, and returns its resource
/// name (`PCLat`, `PCJp`, then `PCJp1`, ...; `PCHe`, ... for Hebrew text). Nothing here can fail: the
/// outlines were read already.
pub(super) fn add_type3_font(doc: &mut Document, fonts_res: &mut Dict, face: &CraftFont, hebrew: bool, glyphs: Vec<Type3Glyph>) -> String {
    let last_char = glyphs.len();
    let mut charprocs = Dict::new();
    let mut widths = Vec::with_capacity(last_char);
    let mut differences = vec![Object::Int(1)];
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
    );
    cmap.push_str(&format!("{last_char} beginbfchar\n"));
    for glyph in glyphs {
        let glyph_name = format!("g{:02X}", glyph.code);
        let mut pd = Dict::new();
        pd.set(b"Length".to_vec(), glyph.path.len() as i64);
        let proc_ref = doc.add(Object::Stream(Stream::from_raw(pd, glyph.path)));
        charprocs.set(glyph_name.as_bytes().to_vec(), Object::Ref(proc_ref));
        differences.push(Object::name(&glyph_name));
        widths.push(Object::Real((glyph.width * 1000.0).round()));
        cmap.push_str(&format!("<{:02X}> <{}>\n", glyph.code, unicode_hex(glyph.ch)));
    }
    cmap.push_str("endbfchar\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let mut cmap_dict = Dict::new();
    cmap_dict.set(b"Length".to_vec(), cmap.len() as i64);
    let cmap_ref = doc.add(Object::Stream(Stream::from_raw(cmap_dict, cmap.into_bytes())));
    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    let (font_name, prefix) = fallback_names(face, hebrew);
    font.set(b"Name".to_vec(), Object::name(font_name));
    let mut descriptor = Dict::new();
    descriptor.set(b"Type".to_vec(), Object::name("FontDescriptor"));
    descriptor.set(b"FontName".to_vec(), Object::name(&format!("{}-{}", face.family, face.style).replace(' ', "")));
    descriptor.set(b"FontFamily".to_vec(), Object::String(PdfString::literal(face.family.as_bytes().to_vec())));
    descriptor.set(b"Flags".to_vec(), Object::Int(if face.family.contains("Mincho") { 6 } else { 4 }));
    descriptor.set(b"ItalicAngle".to_vec(), Object::Int(0));
    // PDF 1.7 tables 5.9 and 5.19: a Type 3 descriptor is indirect; Ascent/Descent may be omitted.
    font.set(b"FontDescriptor".to_vec(), Object::Ref(doc.add(Object::Dict(descriptor))));
    font.set(b"FontBBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(-300), Object::Int(1000), Object::Int(1000)]));
    font.set(
        b"FontMatrix".to_vec(),
        Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
    );
    font.set(b"FirstChar".to_vec(), Object::Int(1));
    font.set(b"LastChar".to_vec(), Object::Int(last_char as i64));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    font.set(b"ToUnicode".to_vec(), Object::Ref(cmap_ref));
    let mut name = String::from(prefix);
    let mut suffix = 0usize;
    while fonts_res.contains(name.as_bytes()) {
        suffix = suffix.saturating_add(1);
        name = format!("{prefix}{suffix}");
    }
    fonts_res.set(name.as_bytes().to_vec(), Object::Dict(font));
    name
}

/// The runs that show `text` in the fallback: consecutive characters in one font form one run, so
/// a font is switched only where the text moves to another font. `None` if a character has no code.
pub(super) fn type3_encode(fallback: &Type3Fallback, text: &str) -> Option<Vec<(String, Vec<u8>)>> {
    let mut runs: Vec<(usize, Vec<u8>)> = Vec::new();
    for ch in text.chars() {
        let (font, code) = *fallback.codes.get(&ch)?;
        match runs.last_mut() {
            Some((last, bytes)) if *last == font => bytes.push(code),
            _ => runs.push((font, vec![code])),
        }
    }
    runs.into_iter().map(|(font, bytes)| Some((fallback.names.get(font)?.clone(), bytes))).collect()
}

/// The source font's own TrueType/OpenType program, when the document embeds a complete one whose
/// licence allows editable embedding. A subset (an `ABCDEF+` name) is left out: a character can keep
/// its mapping there after the subsetter dropped its outline.
pub(crate) fn source_program(doc: &Document, fonts: &Dict, name: &str) -> Option<Vec<u8>> {
    let font = fonts.get(name.as_bytes()).and_then(|f| doc.resolve(f).as_dict().cloned())?;
    let base = Metrics::from_dict(doc, &font).base_font;
    if base.split_once('+').is_some_and(|(tag, _)| tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase())) {
        return None;
    }
    let bytes = pdfcraft_fonts::pdf::embedded_sfnt(doc, &font)?;
    pdfcraft_fonts::embedding::validate(&bytes).is_ok().then_some(bytes)
}

/// Whether `text` has characters `line`'s font can't encode that its own program can draw.
pub(super) fn source_program_completes(doc: &Document, fonts: &Dict, line: &TextLine, text: &str) -> bool {
    let Some(metrics) = fonts.get(line.font.as_bytes()).and_then(|f| doc.resolve(f).as_dict().cloned()).map(|f| Metrics::from_dict(doc, &f)) else {
        return false;
    };
    let missing: String = text.chars().filter(|c| !c.is_whitespace() && !source_encodes(&metrics, &c.to_string())).collect();
    !missing.is_empty() && source_program(doc, fonts, &line.font).is_some_and(|bytes| pdfcraft_fonts::embedding::covers(&bytes, &missing))
}

/// A subset may contain all the letters but no space glyph: producers use another subset
/// or a positioning operator for spaces. Do the same instead of substituting the whole font.
pub(crate) fn source_encodes(metrics: &Metrics, text: &str) -> bool {
    text.split(' ').all(|word| metrics.encode(word).is_some())
}

#[cfg(test)]
thread_local! {
    /// How often [`source_space`] ran on this thread, for the test that pins it out of per-character loops.
    pub(crate) static SOURCE_SPACE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(crate) fn source_space(doc: &Document, fonts: &Dict, metrics: &Metrics) -> f64 {
    #[cfg(test)]
    SOURCE_SPACE_CALLS.with(|c| c.set(c.get().saturating_add(1)));
    let width = |m: &Metrics| m.encode(" ").map(|b| m.codes(&b).iter().map(|(c, _)| m.width(*c)).sum::<f64>());
    if let Some(w) = width(metrics) {
        return w;
    }
    let family = metrics.base_font.rsplit('+').next().unwrap_or(&metrics.base_font);
    for (_, font) in fonts.iter() {
        if let Some(dict) = doc.resolve(font).as_dict() {
            let m = Metrics::from_dict(doc, dict);
            if m.base_font.rsplit('+').next() == Some(family)
                && let Some(w) = width(&m)
            {
                return w;
            }
        }
    }
    0.33
}

pub(crate) fn source_width(metrics: &Metrics, text: &str, size: f64, space: f64, char_spacing: f64, word_spacing: f64) -> f64 {
    if let Some(bytes) = metrics.encode(text) {
        // PDF word spacing applies only to single-byte code 32, even for a Unicode space.
        return metrics
            .codes(&bytes)
            .iter()
            .map(|(c, len)| metrics.width(*c) * size + char_spacing + if metrics.is_space(*c, *len) { word_spacing } else { 0.0 })
            .sum();
    }
    let words: f64 = text
        .split(' ')
        .filter_map(|w| metrics.encode(w))
        .map(|b| metrics.codes(&b).iter().map(|(c, _)| metrics.width(*c) * size + char_spacing).sum::<f64>())
        .sum();
    words + text.matches(' ').count() as f64 * (space * size + char_spacing + word_spacing)
}

pub(crate) fn source_show(metrics: &Metrics, text: &str, size: f64, space: f64, char_spacing: f64, word_spacing: f64) -> Result<Vec<Op>, EditError> {
    if let Some(bytes) = metrics.encode(text) {
        return Ok(vec![Op::new("Tj", vec![Object::String(PdfString::literal(bytes))])]);
    }
    let adjustment = -1000.0 * (space + (char_spacing + word_spacing) / size);
    if !adjustment.is_finite() {
        return Err(EditError::Invalid("the source font's space advance is invalid".into()));
    }
    let mut out = Vec::new();
    for (i, word) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(Op::new("TJ", vec![Object::Array(vec![pdfcraft_content::num(adjustment)])]));
        }
        if !word.is_empty() {
            let bytes = metrics.encode(word).ok_or_else(|| EditError::Invalid("the source font cannot show the replacement text".into()))?;
            out.push(Op::new("Tj", vec![Object::String(PdfString::literal(bytes))]));
        }
    }
    Ok(out)
}
