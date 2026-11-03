use super::layout::{
    Piece, arabic_error, hebrew_error, is_arabic, is_hebrew, is_syriac, is_thai, latin_width, norm, shaped_layout, syriac_error, text_rect,
    thai_error, wrapped,
};
use super::types::{AddedText, Align, Family};
use super::{
    Content, Dict, Document, EditError, GlyphError, GlyphOutline, HashMap, HashSet, Object, PdfString, ShapedCluster, ShapedScript, align_code,
    literal, matrix_number, n, shaped_glyph, win_ansi,
};

fn font_name(base: &str) -> String {
    format!("PCF{}", base.replace('-', ""))
}

/// Most glyphs in one Type 3 font: its codes are single bytes, 1–240. An item gets one such font
/// per 240 distinct glyphs it needs.
pub(super) const GLYPHS_PER_FONT: usize = 240;

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

/// Draw a text item that has Arabic, Hebrew or Thai in it. Each line is laid out in display order; their glyphs
/// come from Type 3 fonts drawn from the craft-fonts face's outlines (no font file is embedded),
/// with a ToUnicode map for copy and search. Other text uses the item's standard font.
fn draw_shaped(doc: &mut Document, t: &AddedText, taken: &Dict, fonts: &mut Dict, out: &mut Vec<u8>) -> Result<(), EditError> {
    let laid = wrapped(t)
        .into_iter()
        .map(|(line, rtl, paragraph)| {
            if line.len() > pdfcraft_fonts::text_direction::MAX_DIRECTION_BYTES {
                return Err(EditError::Invalid("a directional text line exceeds 262144 UTF-8 bytes; shorten it before adding text".into()));
            }
            shaped_layout(t, &line, rtl).map(|(pieces, w)| (line, rtl, paragraph, pieces, w))
        })
        .collect::<Result<Vec<_>, _>>()?;
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
    let r = text_rect(t);
    let latin = font_name(t.family.base_font(t.bold, t.italic));
    // Every distinct cluster (a letter with its dots and the text it stands for) is one glyph of
    // one of the fonts, so copy and search get each character exactly once.
    let mut keys: Vec<&ShapedCluster> = Vec::new();
    let mut index: HashMap<ClusterKey, usize> = HashMap::new();
    for c in laid.iter().flat_map(|(_, _, _, pieces, _)| pieces).flat_map(|p| match p {
        Piece::Shaped(clusters) => clusters.as_slice(),
        Piece::Latin(_) => &[],
    }) {
        index.entry(cluster_key(c)).or_insert_with(|| {
            keys.push(c);
            keys.len() - 1
        });
    }

    // Every outline is read before anything is written, so a failure leaves the document as it was.
    let mut outlines = HashMap::new();
    for c in &keys {
        for (id, _) in &c.glyphs {
            if let std::collections::hash_map::Entry::Vacant(entry) = outlines.entry((c.script, *id)) {
                entry.insert(outline(c.script, *id).map_err(|e| match c.script {
                    ShapedScript::Arabic | ShapedScript::ArabicBold => arabic_error(e, &c.text),
                    ShapedScript::Hebrew => hebrew_error(e, &c.text),
                    ShapedScript::Thai => thai_error(e, &c.text),
                    ShapedScript::Syriac | ShapedScript::SyriacEastern | ShapedScript::SyriacWestern => syriac_error(e, &c.text),
                })?);
            }
        }
    }
    // The names the page and this item already use: a new font never takes one of them.
    let mut taken_names: HashSet<Vec<u8>> = taken.iter().chain(fonts.iter()).map(|(name, _)| name.clone()).collect();
    let mut names = Vec::new();
    for chunk in keys.chunks(GLYPHS_PER_FONT) {
        let font = pdfcraft_fonts::embed_shaped_clusters(doc, chunk, &outlines).map_err(EditError::Invalid)?;
        // A name no font on the page has, so items never replace each other's fonts (object
        // numbers alone don't do: a full save renumbers objects but keeps resource names).
        let mut name = format!("PCAr{}", font.num);
        let mut suffix = 0usize;
        while taken_names.contains(name.as_bytes()) {
            suffix = suffix.saturating_add(1);
            name = format!("PCAr{}_{suffix}", font.num);
        }
        taken_names.insert(name.clone().into_bytes());
        fonts.set(name.clone().into_bytes(), Object::Ref(font));
        names.push(name);
    }
    let [cr, cg, cb] = t.color.map(|v| v.clamp(0.0, 1.0));
    out.extend(format!("BT {} {} {} rg\n", n(cr), n(cg), n(cb)).bytes());
    if t.char_spacing != 0.0 {
        out.extend(format!("{} Tc\n", n(t.char_spacing)).bytes());
    }
    let paragraphs: Vec<usize> = laid.iter().map(|(_, _, p, ..)| *p).collect();
    for (i, (line, rtl, _, pieces, w)) in laid.iter().enumerate() {
        // Shaped glyphs are painted in visual order. Keep the original line's typing order
        // explicitly, including marks, ligatures, digits and mixed-script punctuation.
        let logical: String = line.encode_utf16().map(|u| format!("{u:04X}")).collect();
        out.extend(format!("/Span <</ActualText <FEFF{logical}>>> BDC\n").bytes());
        let spaces = line.matches(' ').count();
        let (x, tw) = placed(t, r, &paragraphs, i, *w, spaces, *rtl);
        let y = r[3] - (i as f64 * 1.2 + 0.95) * t.size;
        let mut pen = x;
        let mut current: Option<&str> = None;
        for piece in pieces {
            match piece {
                Piece::Latin(s) => {
                    out.extend(format!("/{latin} {} Tf {} Tw 1 0 0 1 {} {} Tm ", n(t.size), n(tw), n(pen), n(y)).bytes());
                    out.extend(literal(&win_ansi(s)));
                    out.extend_from_slice(b" Tj\n");
                    current = Some(latin.as_str());
                    pen += latin_width(t, s) + tw * s.matches(' ').count() as f64;
                }
                Piece::Shaped(clusters) => {
                    for c in clusters {
                        let Some(&k) = index.get(&cluster_key(c)) else { continue };
                        let Some(name) = names.get(k / GLYPHS_PER_FONT) else { continue };
                        // Codes start at 1; k % 240 + 1 is at most 240.
                        let code = u8::try_from(k % GLYPHS_PER_FONT + 1).unwrap_or(1);
                        if current != Some(name.as_str()) {
                            out.extend(format!("/{name} {} Tf ", n(t.size)).bytes());
                            current = Some(name.as_str());
                        }
                        out.extend(format!("1 0 0 1 {} {} Tm ", n(pen), n(y)).bytes());
                        out.extend(literal(&[code]));
                        out.extend_from_slice(b" Tj\n");
                        pen += c.advance * t.size + t.char_spacing + if c.text == " " { tw } else { 0.0 };
                    }
                }
            }
        }
        out.extend_from_slice(b"EMC\n");
    }
    out.extend_from_slice(b"ET\n");
    Ok(())
}

/// What makes two clusters the same Type 3 glyph: text, glyphs, offsets and advance (bit for bit;
/// equal clusters come from the same font units).
type ClusterKey<'a> = (ShapedScript, &'a str, Vec<(u32, u64, u64)>, u64);

fn cluster_key(c: &ShapedCluster) -> ClusterKey<'_> {
    (c.script, c.text.as_str(), c.glyphs.iter().map(|(id, [x, y])| (*id, x.to_bits(), y.to_bits())).collect(), c.advance.to_bits())
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
            if t.direction != pdfcraft_fonts::TextDirection::Auto
                || t.text.chars().any(is_thai)
                || face_or_new(is_arabic, pdfcraft_fonts::document_arabic_font().is_some())
                || face_or_new(is_hebrew, pdfcraft_fonts::document_hebrew_font(false).is_some())
                || face_or_new(is_syriac, pdfcraft_fonts::document_syriac_font_for_family(t.syriac_family).is_some())
            {
                draw_shaped(doc, t, taken, &mut fonts, &mut out)?;
                res.set(b"Font".to_vec(), Object::Dict(fonts));
                out.extend_from_slice(b"Q\n");
                return Ok((out, res));
            }
            res.set(b"Font".to_vec(), Object::Dict(fonts));
            let r = text_rect(t);
            let [cr, cg, cb] = t.color.map(|v| v.clamp(0.0, 1.0));
            out.extend(format!("BT /{name} {} Tf {} {} {} rg\n", n(t.size), n(cr), n(cg), n(cb)).bytes());
            if t.char_spacing != 0.0 {
                out.extend(format!("{} Tc\n", n(t.char_spacing)).bytes());
            }
            let all = wrapped(t);
            let paragraphs: Vec<usize> = all.iter().map(|(_, _, p)| *p).collect();
            for (i, (line, ..)) in all.iter().enumerate() {
                let w = latin_width(t, line);
                // Baseline: 0.8 em below the line top.
                let y = r[3] - (i as f64 * 1.2 + 0.95) * t.size;
                // Justified: word spacing makes every line of a paragraph but its last fill the box.
                let (x, tw) = placed(t, r, &paragraphs, i, w, line.matches(' ').count(), false);
                out.extend(format!("1 0 0 1 {} {} Tm {} Tw ", n(x), n(y), n(tw)).bytes());
                out.extend(literal(&win_ansi(line)));
                out.extend_from_slice(b" Tj\n");
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

/// Serialize an added item's editable text/image settings into its private PDF dictionary.
pub(super) fn params(c: &Content) -> Dict {
    let mut d = Dict::new();
    let arr = |v: &[f64]| Object::Array(v.iter().map(|x| Object::Real(*x)).collect());
    match c {
        Content::Text(t) => {
            d.set(b"Kind".to_vec(), Object::name("Text"));
            d.set(b"Text".to_vec(), PdfString::text(&t.text));
            d.set(b"Font".to_vec(), Object::name(t.family.base_font(t.bold, t.italic)));
            d.set(pdfcraft_fonts::SyriacFamily::PDF_KEY.to_vec(), Object::name(t.syriac_family.name()));
            d.set(b"Size".to_vec(), Object::Real(t.size));
            d.set(b"Color".to_vec(), arr(&t.color));
            d.set(b"Align".to_vec(), Object::Int(align_code(t.align)));
            d.set(b"Direction".to_vec(), Object::name(t.direction.name()));
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
