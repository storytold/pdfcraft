use super::layout::{
    Piece, arabic_error, hebrew_error, is_arabic, is_hebrew, is_thai, latin_width, line_extents, norm, shaped_layout_bidi, text_rect, thai_error,
    wrapped,
};
use super::types::{AddedText, Align, Family, Ink};
use super::{
    Content, Dict, Document, EditError, GlyphError, GlyphOutline, Level, Object, ParagraphBidiInfo, PdfString, ShapedScript, align_code, literal,
    matrix_number, n, shaped_glyph, win_ansi,
};
use pdfcraft_fonts::shaped_text::{self, ShapedFonts};

fn font_name(base: &str) -> String {
    format!("PCF{}", base.replace('-', ""))
}

#[cfg(test)]
pub(super) use pdfcraft_fonts::shaped_text::GLYPHS_PER_FONT;

/// A line of a shaped-text item after layout: its text, whether its paragraph runs right to left,
/// its paragraph, its pieces in drawing order and its width at the item's size.
pub(super) type LaidLine = (String, bool, usize, Vec<Piece>, f64);

/// Where line `i` (`w` wide unstretched, with `spaces` spaces) starts in the box `r`, and its word
/// spacing. `paragraphs` holds each line's paragraph: a line takes its paragraph's alignment, and
/// every line of a justified paragraph but its last is stretched to the box's width.
fn placed(t: &AddedText, r: [f64; 4], paragraphs: &[usize], i: usize, w: f64, spaces: usize, rtl: bool) -> (f64, f64) {
    let paragraph = paragraphs.get(i).copied().unwrap_or(0);
    let align = t.paragraph_align(paragraph);
    let ends_paragraph = paragraphs.get(i + 1) != Some(&paragraph);
    let tw = if align == Align::Justify && !ends_paragraph && spaces > 0 { ((r[2] - r[0]) - w).max(0.0) / spaces as f64 } else { 0.0 };
    let x = match align {
        // A right-to-left paragraph's unstretched last line ends at the right edge.
        Align::Justify if rtl => r[2] - w - tw * spaces as f64,
        Align::Left | Align::Justify => r[0],
        Align::Center => r[0] + ((r[2] - r[0]) - w) / 2.0,
        Align::Right => r[2] - w,
    };
    (x, tw)
}

pub(super) fn laid_lines(t: &AddedText) -> Result<Vec<LaidLine>, EditError> {
    let mut fragments = wrapped(t).into_iter();
    let mut laid = Vec::new();
    for (paragraph_index, paragraph) in t.text.split('\n').enumerate() {
        let rtl =
            paragraph.chars().any(|c| is_arabic(c) || is_hebrew(c)) && unicode_bidi::get_base_direction(paragraph) == unicode_bidi::Direction::Rtl;
        let bidi = ParagraphBidiInfo::new(paragraph, Some(if rtl { Level::rtl() } else { Level::ltr() }));
        let mut start = 0usize;
        loop {
            let Some((line, rtl, index)) = fragments.next() else { return Err(EditError::Invalid("text line is missing".into())) };
            if index != paragraph_index {
                return Err(EditError::Invalid("text line has the wrong paragraph".into()));
            }
            let end = start.saturating_add(line.len());
            if paragraph.get(start..end) != Some(line.as_str()) {
                return Err(EditError::Invalid("text line is outside its paragraph".into()));
            }
            let rect = norm(t.rect);
            let width = (rect[2] - rect[0]).max(t.size);
            let display = pdfcraft_fonts::appearance_text::fit_display_line(&line, end < paragraph.len(), width, |display| {
                pdfcraft_fonts::appearance_text::line_bidi(&bidi, start..end, display)
                    .ok()
                    .and_then(|info| shaped_layout_bidi(t, display, &info).ok())
                    .map_or(f64::INFINITY, |(_, width)| width)
            });
            let info = pdfcraft_fonts::appearance_text::line_bidi(&bidi, start..end, &display)
                .map_err(|_| EditError::Invalid("text line has no paragraph direction".into()))?;
            let (pieces, width) = shaped_layout_bidi(t, &display, &info)?;
            laid.push((line, rtl, index, pieces, width));
            start = end;
            if end >= paragraph.len() {
                break;
            }
        }
    }
    Ok(laid)
}

/// Draw a text item that has Arabic, Hebrew or Thai in it. Each line is laid out in display order; their glyphs
/// come from Type 3 fonts drawn from the craft-fonts face's outlines (no font file is embedded),
/// with a ToUnicode map for copy and search. Other text uses the item's standard font.
pub(super) fn draw_shaped(doc: &mut Document, t: &AddedText, taken: &Dict, fonts: &mut Dict, out: &mut Vec<u8>) -> Result<(), EditError> {
    let laid = laid_lines(t)?;
    draw_laid_shaped(doc, t, &laid, taken, fonts, out, shaped_glyph)
}

/// [`draw_shaped`] for lines that are already laid out. `outline` is the source of glyph outlines
/// for a script's face (the craft-fonts faces, or a stand-in in tests).
pub(super) fn draw_laid_shaped(
    doc: &mut Document,
    t: &AddedText,
    laid: &[LaidLine],
    taken: &Dict,
    fonts: &mut Dict,
    out: &mut Vec<u8>,
    outline: impl Fn(ShapedScript, u32) -> Result<GlyphOutline, GlyphError>,
) -> Result<(), EditError> {
    let r = norm(t.rect);
    let latin = font_name(t.family.base_font(t.bold, t.italic));
    // Each outline is read once: for the fonts, then for each line's ink extents.
    let read: std::cell::RefCell<std::collections::HashMap<(ShapedScript, u32), Result<GlyphOutline, GlyphError>>> = Default::default();
    let outline = |script: ShapedScript, id: u32| read.borrow_mut().entry((script, id)).or_insert_with(|| outline(script, id)).clone();
    let shaped =
        ShapedFonts::new(shaped_text::clusters(laid.iter().map(|(_, _, _, pieces, _)| pieces.as_slice())), outline, taken, fonts, "PCAr", &mut |o| {
            Object::Ref(doc.add(o))
        })
        .map_err(|(e, c)| match c.script {
            ShapedScript::Arabic | ShapedScript::ArabicBold => arabic_error(e, &c.text),
            ShapedScript::Hebrew => hebrew_error(e, &c.text),
            ShapedScript::Thai => thai_error(e, &c.text),
        })?;
    out.extend(format!("BT {}\n", t.fill_op()).bytes());
    if t.char_spacing != 0.0 {
        out.extend(format!("{} Tc\n", n(t.char_spacing)).bytes());
    }
    let paragraphs: Vec<usize> = laid.iter().map(|(_, _, p, ..)| *p).collect();
    let mut top = r[3];
    for (i, (line, rtl, _, pieces, w)) in laid.iter().enumerate() {
        // Shaped glyphs are painted in visual order. Keep the original line's typing order
        // explicitly, including marks, ligatures, digits and mixed-script punctuation.
        let logical: String = line.encode_utf16().map(|u| format!("{u:04X}")).collect();
        out.extend(format!("/Span <</ActualText <FEFF{logical}>>> BDC\n").bytes());
        // Logical text retains hanging separators that the displayed pieces omit. Only the
        // spaces the writer actually advances may share the line's justification.
        let spaces = pieces
            .iter()
            .map(|piece| match piece {
                Piece::Latin(text) => text.matches(' ').count(),
                Piece::Shaped(clusters) => clusters.iter().filter(|cluster| cluster.text == " ").count(),
            })
            .sum();
        let (x, tw) = placed(t, r, &paragraphs, i, *w, spaces, *rtl);
        let extents = line_extents(pieces, |cluster| {
            let mut bounds = [0.0f64; 4];
            for (id, [dx, dy]) in &cluster.glyphs {
                let glyph = outline(cluster.script, *id).ok()?;
                bounds = [
                    bounds[0].min(glyph.bbox[0] + dx),
                    bounds[1].min(glyph.bbox[1] + dy),
                    bounds[2].max(glyph.bbox[2] + dx),
                    bounds[3].max(glyph.bbox[3] + dy),
                ];
            }
            Some(bounds)
        });
        let y = top - extents[0] * t.size;
        top = y - extents[1] * t.size;
        let mut pen = x;
        let mut current = None;
        for piece in pieces {
            match piece {
                Piece::Latin(s) => {
                    out.extend(format!("/{latin} {} Tf {} Tw 1 0 0 1 {} {} Tm ", n(t.size), n(tw), n(pen), n(y)).bytes());
                    out.extend(literal(&win_ansi(s)));
                    out.extend_from_slice(b" Tj\n");
                    current = None;
                    pen += latin_width(t, s) + tw * s.matches(' ').count() as f64;
                }
                Piece::Shaped(clusters) => pen = shaped.show(out, clusters, pen, y, t.size, t.char_spacing, tw, &mut current),
            }
        }
        out.extend_from_slice(b"EMC\n");
    }
    out.extend_from_slice(b"ET\n");
    Ok(())
}

/// Content and resources for an item; `view` maps display space to user space. `taken` holds the
/// page's font names. `existing`: the item is being replaced, so a build without the Arabic or
/// Hebrew face keeps drawing that text as `?`, as before it was supported, rather than refusing to
/// move it.
pub(super) fn draw(doc: &mut Document, c: &Content, view: [f64; 6], taken: &Dict, existing: bool) -> Result<(Vec<u8>, Dict), EditError> {
    let mut res = Dict::new();
    let mut out = format!(
        "q {} {} {} {} {} {} cm\n",
        matrix_number(view[0]),
        matrix_number(view[1]),
        matrix_number(view[2]),
        matrix_number(view[3]),
        matrix_number(view[4]),
        matrix_number(view[5])
    )
    .into_bytes();
    match c {
        Content::Text(t) => {
            let base = t.family.base_font(t.bold, t.italic);
            let name = font_name(base);
            let mut font = Dict::new();
            font.set(b"Type".to_vec(), Object::name("Font"));
            font.set(b"Subtype".to_vec(), Object::name("Type1"));
            font.set(b"BaseFont".to_vec(), Object::name(base));
            if t.family != Family::Courier || !t.text.is_ascii() {
                font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
            }
            let mut fonts = Dict::new();
            fonts.set(name.clone().into_bytes(), Object::Dict(font));
            let face_or_new = |has: fn(char) -> bool, face: bool| t.text.chars().any(has) && (face || !existing);
            if t.text.chars().any(is_thai)
                || face_or_new(is_arabic, pdfcraft_fonts::document_arabic_font().is_some())
                || face_or_new(is_hebrew, pdfcraft_fonts::document_hebrew_font(false).is_some())
                || (t.text.chars().any(pdfcraft_fonts::embedding::default_ignorable) && pdfcraft_fonts::appearance_text::can_shape(&t.text))
            {
                draw_shaped(doc, t, taken, &mut fonts, &mut out)?;
                res.set(b"Font".to_vec(), Object::Dict(fonts));
                out.extend_from_slice(b"Q\n");
                return Ok((out, res));
            }
            res.set(b"Font".to_vec(), Object::Dict(fonts));
            let r = text_rect(t);
            out.extend(format!("BT /{name} {} Tf {}\n", n(t.size), t.fill_op()).bytes());
            if t.char_spacing != 0.0 {
                out.extend(format!("{} Tc\n", n(t.char_spacing)).bytes());
            }
            let all = wrapped(t);
            let paragraphs: Vec<usize> = all.iter().map(|(_, _, p)| *p).collect();
            let mut offset = 0usize;
            for (i, (line, ..)) in all.iter().enumerate() {
                offset = offset.saturating_add(line.len());
                let next = t.text.as_bytes().get(offset).copied();
                let display =
                    pdfcraft_fonts::appearance_text::fit_display_line(line, next.is_some_and(|c| c != b'\n'), (r[2] - r[0]).max(t.size), |s| {
                        latin_width(t, s)
                    });
                if next == Some(b'\n') {
                    offset = offset.saturating_add(1);
                }
                let hanging = display != line.as_str();
                if hanging {
                    let logical: String = line.encode_utf16().map(|u| format!("{u:04X}")).collect();
                    out.extend(format!("/Span <</ActualText <FEFF{logical}>>> BDC\n").bytes());
                }
                let w = latin_width(t, &display);
                // Baseline: 0.8 em below the line top.
                let y = r[3] - (i as f64 * 1.2 + 0.95) * t.size;
                let (x, tw) = placed(t, r, &paragraphs, i, w, display.matches(' ').count(), false);
                out.extend(format!("1 0 0 1 {} {} Tm {} Tw ", n(x), n(y), n(tw)).bytes());
                out.extend(literal(&win_ansi(&display)));
                out.extend_from_slice(b" Tj\n");
                if hanging {
                    out.extend_from_slice(b"EMC\n");
                }
            }
            out.extend_from_slice(b"ET\n");
        }
        Content::Image(i) => {
            if !matches!(&*doc.get(i.image), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image")) {
                return Err(EditError::Invalid("not an image".into()));
            }
            let r = norm(i.rect);
            let name = format!("PCImg{}", i.image.num);
            let mut xo = Dict::new();
            xo.set(name.clone().into_bytes(), Object::Ref(i.image));
            res.set(b"XObject".to_vec(), Object::Dict(xo));
            let [a, b, c, d, e, f] = i.matrix().0;
            // Clip to the box: the cropped-away parts stay hidden.
            out.extend(
                format!(
                    "{} {} {} {} re W n {} {} {} {} {} {} cm /{name} Do\n",
                    n(r[0]),
                    n(r[1]),
                    n(r[2] - r[0]),
                    n(r[3] - r[1]),
                    n(a),
                    n(b),
                    n(c),
                    n(d),
                    n(e),
                    n(f)
                )
                .bytes(),
            );
        }
    }
    out.extend_from_slice(b"Q\n");
    Ok((out, res))
}

pub(super) fn params(c: &Content) -> Dict {
    let mut d = Dict::new();
    let arr = |v: &[f64]| Object::Array(v.iter().map(|x| Object::Real(*x)).collect());
    match c {
        Content::Text(t) => {
            d.set(b"Kind".to_vec(), Object::name("Text"));
            d.set(b"Text".to_vec(), PdfString::text(&t.text));
            d.set(b"Font".to_vec(), Object::name(t.family.base_font(t.bold, t.italic)));
            d.set(b"Size".to_vec(), Object::Real(t.size));
            d.set(b"Color".to_vec(), arr(&t.color));
            // The ink beside its RGB look, which a reader that knows no ink keeps showing.
            match t.painted_ink() {
                Some(Ink::Gray(g)) => d.set(b"Gray".to_vec(), Object::Real(g)),
                Some(Ink::Cmyk(c)) => d.set(b"CMYK".to_vec(), arr(&c)),
                None => {}
            }
            d.set(b"Align".to_vec(), Object::Int(align_code(t.align)));
            // Each paragraph's own alignment, or null where it follows /Align.
            let aligns = crate::paragraph_align::tidy(t.aligns.clone(), t.align);
            if !aligns.is_empty() {
                d.set(b"Aligns".to_vec(), Object::Array(aligns.iter().map(|a| a.map_or(Object::Null, |a| Object::Int(align_code(a)))).collect()));
            }
            d.set(b"Rect".to_vec(), arr(&text_rect(t)));
            if t.char_spacing != 0.0 {
                d.set(b"CharSpacing".to_vec(), Object::Real(t.char_spacing));
            }
        }
        Content::Image(i) => {
            d.set(b"Kind".to_vec(), Object::name("Image"));
            d.set(b"Image".to_vec(), Object::Ref(i.image));
            d.set(b"Rect".to_vec(), arr(&norm(i.rect)));
            if i.rotation % 4 != 0 {
                d.set(b"Rotate".to_vec(), Object::Int(i64::from(i.rotation % 4) * 90));
            }
            if i.flip_h {
                d.set(b"FlipH".to_vec(), Object::Bool(true));
            }
            if i.flip_v {
                d.set(b"FlipV".to_vec(), Object::Bool(true));
            }
            if i.crop != [0.0; 4] {
                d.set(b"Crop".to_vec(), arr(&i.crop));
            }
        }
    }
    d
}

/// Existing Thai text uses absolute shaped glyph positions. Reuse Add Text's bounded outline
/// writer instead of trying to embed the matching craft-fonts variable program as a static face.
pub(crate) fn thai_glyph_resources(
    doc: &mut Document,
    fonts: &mut Dict,
    shaped: &pdfcraft_fonts::source_shaping::ShapedRun,
) -> Result<Vec<(String, Vec<u8>)>, EditError> {
    if shaped.glyphs.len() > 65_534 {
        return Err(EditError::Invalid("shaped row exceeds its geometry budget".into()));
    }
    let mut outlines = std::collections::HashMap::new();
    let mut clusters = Vec::new();
    let mut index = std::collections::HashMap::new();
    let mut indices = Vec::new();
    for glyph in &shaped.glyphs {
        let id = glyph.id.to_u32();
        let key = (ShapedScript::Thai, id);
        if let std::collections::hash_map::Entry::Vacant(entry) = outlines.entry(key) {
            entry.insert(shaped_glyph(ShapedScript::Thai, id).map_err(|e| thai_error(e, &glyph.text))?);
        }
        let at = *index.entry((id, glyph.text.as_str())).or_insert_with(|| {
            let at = clusters.len();
            let width = outlines.get(&key).map_or(0.0, |g| g.width);
            clusters.push(pdfcraft_fonts::ShapedCluster {
                script: ShapedScript::Thai,
                glyphs: vec![(id, [0.0, 0.0])],
                advance: width,
                text: glyph.text.clone(),
            });
            at
        });
        indices.push(at);
    }
    // Validate all outlines before adding resources. Type 3 character codes are single bytes;
    // shard long rows and return the appropriate resource/code for every positioned glyph.
    let mut names = Vec::new();
    for chunk in clusters.chunks(shaped_text::GLYPHS_PER_FONT) {
        let refs: Vec<_> = chunk.iter().collect();
        let font = shaped_text::add_type3_font(doc, &refs, &outlines);
        doc.update_dict(font, |f| f.set(b"Name".to_vec(), Object::name("PCThai")))?;
        let mut name = format!("PCThai{}", font.num);
        while fonts.contains(name.as_bytes()) {
            name.push('_');
        }
        fonts.set(name.as_bytes().to_vec(), Object::Ref(font));
        names.push(name);
    }
    indices
        .into_iter()
        .map(|at| {
            let name = names.get(at / shaped_text::GLYPHS_PER_FONT).ok_or_else(|| EditError::Invalid("Thai font resource is missing".into()))?;
            let code = u8::try_from(at % shaped_text::GLYPHS_PER_FONT + 1).map_err(|_| EditError::Invalid("Thai glyph limit".into()))?;
            Ok((name.clone(), vec![code]))
        })
        .collect()
}
