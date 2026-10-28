//! A reported, shaped Arabic fallback for source subsets that cannot draw an edited paragraph.
//! The source operators are replaced in place; surrounding text and graphics remain untouched.

use super::interp::{Carry, Shown, Ts, interpret};
use super::{
    BlockStyle, Dict, Document, EditError, HashMap, LineEdit, Matrix, Metrics, Object, Op, Pieces, TextBlock, TextLine, coincident_ops,
    content_streams, page_dict, parse, reveal_ops, rewritten_contents, splice,
};
use pdfcraft_fonts::appearance_text;

/// Whether the Arabic fallback can draw this edit at all: Arabic text (or a paragraph it wrote
/// before), only the paragraph-wide formatting it supports, and upright source rows that share
/// one transform. Runs may differ in size, colour, weight and font: each keeps its own look.
pub(super) fn eligible(members: &[&TextLine], text: &str, style: &BlockStyle) -> bool {
    let Some(first) = members.first().copied() else { return false };
    let own_fallback = members.iter().all(|line| line.font.starts_with("PCEdArabic"));
    !(pdfcraft_fonts::document_arabic_font().is_none()
        || (!own_fallback
            && !text
                .chars()
                .any(|c| matches!(u32::from(c), 0x0600..=0x06ff | 0x0750..=0x077f | 0x0870..=0x08ff | 0xfb50..=0xfdff | 0xfe70..=0xfefc)))
        || !style.ranges.is_empty()
        || style.resource.is_some()
        || style.family.is_some()
        || style.bold.is_some()
        || style.char_spacing.is_some()
        || style.scale.is_some()
        || style.line_spacing.is_some()
        || style.underline == Some(true)
        || style.align == Some(crate::added::Align::Justify)
        || members.iter().any(|line| !line.upright() || line.origin.ctm.iter().zip(first.origin.ctm).any(|(a, b)| (a - b).abs() > 1e-9)))
}

/// Whether a uniform paragraph (one size, colour and face) must take the Arabic fallback up front
/// because no complete source face can draw it. A mixed paragraph first tries its source fonts,
/// run by run, and takes the fallback only when they cannot draw the edit (see
/// `rewrite_block_from`).
pub(super) fn needs_fallback(doc: &Document, fonts: &Dict, members: &[&TextLine], text: &str, style: &BlockStyle) -> bool {
    let Some(first) = members.first().copied() else { return false };
    let own_fallback = members.iter().all(|line| line.font.starts_with("PCEdArabic"));
    if !eligible(members, text, style)
        || members.iter().any(|line| {
            line.size != first.size
                || line.color != first.color
                || (!own_fallback && pdfcraft_fonts::embedding::face_key(&line.base_font) != pdfcraft_fonts::embedding::face_key(&first.base_font))
        })
    {
        return false;
    }
    // Prefer a usable complete source face. A fallback never replaces a face capable of the edit.
    let family = pdfcraft_fonts::embedding::face_key(&first.base_font);
    let reusable = |bytes: &[u8]| {
        pdfcraft_fonts::embedding::covers(bytes, text)
            && pdfcraft_fonts::embedding::validate(bytes).is_ok()
            && text.split('\n').all(|line| pdfcraft_fonts::source_shaping::shape(bytes, line).is_ok())
    };
    !fonts.iter().filter_map(|(_, font)| doc.dict(font)).any(|font| {
        pdfcraft_fonts::embedding::face_key(&Metrics::from_dict(doc, &font).base_font) == family
            && pdfcraft_fonts::pdf::embedded_sfnt(doc, &font).is_some_and(|bytes| reusable(&bytes))
    }) && !pdfcraft_fonts::embedding::matching_faces(&first.base_font).into_iter().any(|face| reusable(face.bytes))
}

pub(super) fn rewrite(
    doc: &mut Document,
    page: usize,
    block: &TextBlock,
    lines: &[TextLine],
    draft: &str,
    style: &BlockStyle,
) -> Result<LineEdit, EditError> {
    let members: Vec<_> = block.lines.iter().filter_map(|index| lines.get(*index)).collect();
    let first = members.first().copied().ok_or_else(|| EditError::Invalid("the paragraph has no source runs".into()))?;
    // The source path's refusals that don't depend on its fonts hold for the fallback too.
    members.iter().try_for_each(|line| line.ensure_editable())?;
    let page = page_dict(doc, page)?;
    let size = style.size.unwrap_or(first.size);
    let color = style.color.unwrap_or(first.color);
    // Every run keeps its source look (a size or colour asked for applies to all of them).
    // Italic is for the Latin runs: Arabic script has no italic and stays upright.
    let look = |line: &TextLine| appearance_text::SpanStyle {
        size: style.size.unwrap_or(line.size),
        color: style.color.unwrap_or(line.color),
        bold: line.bold,
        italic: line.italic,
    };
    let spans: Vec<_> = row_sources(block, draft, block.edited_spans(draft))
        .into_iter()
        .map(|(range, index)| (range, look(lines.get(index).unwrap_or(first))))
        .collect();
    if spans.iter().any(|(_, s)| !s.size.is_finite() || !(0.1..=500.0).contains(&s.size) || s.color.iter().any(|v| !v.is_finite())) {
        return Err(EditError::Invalid("the paragraph's runs must have finite sizes between 0.1 and 500 points and finite colours".into()));
    }
    if spans.iter().any(|(range, s)| s.bold && draft.get(range.clone()).is_some_and(|t| t.chars().any(pdfcraft_fonts::arabic_has)))
        && !pdfcraft_fonts::arabic_bold_available()
    {
        return Err(EditError::Invalid(
            "bold Arabic needs a variable Arabic face with a weight axis, which this build doesn't include; choose a complete bold font".into(),
        ));
    }
    let [dx, dy] = style.offset.unwrap_or([0.0, 0.0]);
    if !size.is_finite() || !(0.1..=500.0).contains(&size) || [dx, dy].iter().chain(color.iter()).any(|v| !v.is_finite()) {
        return Err(EditError::Invalid("the paragraph's position, size and colour must be finite".into()));
    }
    let rtl = appearance_text::paragraph_rtl(draft);
    let natural = if block.rows.len() == 1 {
        let available = if rtl { block.rect[2] + dx - (page.crop(doc)[0] + 36.0) } else { page.crop(doc)[2] - 36.0 - block.rect[0] - dx };
        available.max(block.rect[2] - block.rect[0])
    } else {
        block.rect[2] - block.rect[0]
    };
    let width = style.width.unwrap_or(natural);
    if !width.is_finite() || !(0.1..=14_400.0).contains(&width) {
        return Err(EditError::Invalid("the paragraph's width must be between 0.1 and 14400 points".into()));
    }
    if let Some(c) = appearance_text::first_undrawable(draft) {
        return Err(crate::undrawable(c));
    }
    let laid = appearance_text::wrap_styled(draft, size, width, look(first), &spans)
        .map_err(|_| EditError::Invalid("the Arabic paragraph cannot be shaped within its limits".into()))?;
    let align = style.align.unwrap_or(if rtl { crate::added::Align::Right } else { crate::added::Align::Left });
    let left = if rtl && style.width.is_none() && style.align.is_none() { block.rect[2] - width } else { block.rect[0] } + dx;
    let top = first.origin.baseline + size * laid.first().map_or(0.9, |line| line.ascent) + dy;
    let rect = [left, top - appearance_text::height(&laid, size), left + width, top];
    let alignment = match align {
        crate::added::Align::Center => 1,
        crate::added::Align::Right => 2,
        _ => 0,
    };
    let drawn = match row_positions(block, lines, draft, &laid, style) {
        // Each edited row stays on its source row's baseline and against the edge it was set
        // against, so a heading and its subtitle keep their spacing.
        Some(positions) => appearance_text::draw_positioned(&laid, size, &positions, Some(color)),
        None => match source_pitch(block, lines) {
            // A reflowed paragraph keeps its own line pitch where its rows have ink-safe room.
            Some(pitch) => {
                let mut positions = Vec::with_capacity(laid.len());
                let mut y = top - size * laid.first().map_or(0.9, |line| line.ascent);
                for (i, line) in laid.iter().enumerate() {
                    if let Some(previous) = i.checked_sub(1).and_then(|p| laid.get(p)) {
                        y -= pitch.max(size * (previous.descent + line.ascent));
                    }
                    let x = match alignment {
                        1 => rect[0] + (rect[2] - rect[0] - line.width) / 2.0,
                        2 => rect[2] - line.width,
                        _ => rect[0],
                    };
                    positions.push([x, y]);
                }
                appearance_text::draw_positioned(&laid, size, &positions, Some(color))
            }
            None => appearance_text::draw(&laid, size, rect, alignment, color),
        },
    };
    let (content, template) = drawn.map_err(|_| EditError::Invalid("the Arabic paragraph's glyphs exceed the drawing limits".into()))?;
    let inverse = Matrix(first.origin.ctm).invert().ok_or_else(|| EditError::Invalid("the paragraph has a singular transform".into()))?;
    let reveal = reveal_ops(block, lines, style, first.origin.ctm)?;
    let streams = content_streams(doc, &page.dict)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, bytes)| bytes.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let origin = &first.origin;
    // Validate the splice before materializing any new font objects.
    let at = first
        .ops
        .iter()
        .copied()
        .find(|index| ops.get(*index).is_some_and(|op| matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"")))
        .ok_or_else(|| EditError::Invalid("the paragraph has no text-show operation".into()))?;
    // Marked paragraph boundaries are authoritative. A later edit can wrap close to a
    // neighbouring paragraph; overlap alone must not turn that paragraph into a duplicate.
    let same_scope: Vec<_> = lines.iter().filter(|line| members.iter().any(|target| line.scope == target.scope)).cloned().collect();
    let mut drop = coincident_ops(&same_scope, &members);
    drop.extend(members.iter().flat_map(|line| line.ops.iter().copied()));
    let mut resources = page.dict.get(b"Resources").and_then(|o| doc.dict(o)).unwrap_or_default();
    let mut fonts = resources.get(b"Font").and_then(|o| doc.dict(o)).unwrap_or_default();
    let shown = interpret(doc, &ops, &fonts, &mut HashMap::new(), &mut Carry::new(), None, None)?;
    let first_shown = shown.iter().find(|s| s.op == at).ok_or_else(|| EditError::Invalid("the source text state cannot be restored".into()))?;
    // Closing a text object applies its accumulated text clip. Doing so early, even before a
    // different selected run, would change later text and artwork in the same object.
    if shown.iter().any(|s| s.bt == first_shown.bt && s.state.render_mode >= 4) {
        return Err(EditError::Invalid("text used as a clipping path cannot use the Arabic fallback".into()));
    }
    let restore = restore_text_matrix(first_shown)?;
    let mut patches = HashMap::new();
    for source in shown.iter().filter(|s| drop.contains(&s.op)) {
        if matches!(source.state.render_mode, 1 | 2) {
            return Err(EditError::Invalid("the Arabic fallback cannot preserve stroked source text; choose a complete font".into()));
        }
        if source.state.render_mode >= 4 {
            return Err(EditError::Invalid("text used as a clipping path cannot use the Arabic fallback".into()));
        }
        patches.insert(source.op, source_advance(source, &ops)?);
    }
    let mut prefix = patches.remove(&at).ok_or_else(|| EditError::Invalid("the source text advance cannot be restored".into()))?;
    let advance = prefix.pop().ok_or_else(|| EditError::Invalid("the source text advance cannot be restored".into()))?;
    let template = appearance_text::materialize_fonts(doc, &template);
    let mut renamed = HashMap::new();
    for (old, font) in template.iter() {
        let stem = format!("PCEdArabic{}", String::from_utf8_lossy(old));
        let mut name = stem.clone();
        let mut suffix = 0usize;
        while fonts.contains(name.as_bytes()) {
            suffix = suffix.saturating_add(1);
            name = format!("{stem}_{suffix}");
        }
        fonts.set(name.clone().into_bytes(), font.clone());
        renamed.insert(old.clone(), name.into_bytes());
    }
    let mut drawing = parse(&content).ops;
    for op in &mut drawing {
        if op.is("Tf")
            && let Some(Object::Name(old)) = op.operands.first_mut()
            && let Some(name) = renamed.get(old)
        {
            *old = name.clone();
        }
    }
    let mut insert = vec![Op::new("ET", vec![]), Op::new("q", vec![])];
    insert.extend(reveal);
    // Generated glyph matrices already include point size and absolute positions. Do not inherit
    // the source's spacing, horizontal scale, rise, clipping or invisible rendering mode.
    insert.extend([
        Op::new("Tc", vec![Object::Int(0)]),
        Op::new("Tw", vec![Object::Int(0)]),
        Op::new("Tz", vec![Object::Int(100)]),
        Op::new("Ts", vec![Object::Int(0)]),
        Op::new("Tr", vec![Object::Int(0)]),
    ]);
    insert.push(Op::new("cm", inverse.0.map(Object::Real).to_vec()));
    insert.extend(drawing);
    insert.push(Op::new("Q", vec![]));
    // Insert at the captured show operation, after any source cm/q/Q between BT and the text.
    // Closing and reopening the text object keeps that exact graphics state for the inverse CTM.
    insert.push(Op::new("BT", vec![]));
    insert.extend(origin.state.ops());
    insert.extend(restore);
    // A quote's line move and spacing changes happen before the insertion. Every removed show
    // retains its original numeric advance, so surviving Tj/TJ and T* siblings keep both matrices.
    prefix.extend(insert);
    prefix.push(advance);
    patches.insert(at, prefix);
    let data = splice(&pieces, &ops, |i| (patches.remove(&i).unwrap_or_default(), !drop.contains(&i)));
    let contents = rewritten_contents(doc, &streams, data);
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(page.obj, |dict| {
        dict.set(b"Contents".to_vec(), contents);
        dict.set(b"Resources".to_vec(), Object::Dict(resources));
    })?;
    Ok(LineEdit { substituted: pdfcraft_fonts::document_arabic_font().map(|font| format!("{} Type3", font.family)) })
}

/// The distance between a paragraph's evenly spaced source rows of one size, when it has two
/// or more (`None` for one row, mixed sizes or uneven rows).
fn source_pitch(block: &TextBlock, lines: &[TextLine]) -> Option<f64> {
    let firsts: Vec<&TextLine> = block.rows.iter().map(|row| row.first().and_then(|i| lines.get(*i))).collect::<Option<_>>()?;
    let (first, last) = (firsts.first()?, firsts.last()?);
    if firsts.len() < 2 || firsts.iter().any(|l| (l.size - first.size).abs() > 0.01) {
        return None;
    }
    let pitch = (first.origin.baseline - last.origin.baseline) / (firsts.len() - 1) as f64;
    let even = firsts.windows(2).all(|pair| ((pair[0].origin.baseline - pair[1].origin.baseline) - pitch).abs() <= first.size * 0.1);
    (even && pitch.is_finite() && pitch > 0.0 && pitch <= first.size * 4.0).then_some(pitch)
}

/// [`TextBlock::edited_spans`], except that a draft row replaced as a whole keeps its own source
/// row's look when the draft still has one row per source row: retyping a heading and its
/// subtitle must not set the subtitle in the heading's size. Inserted text inside a row still
/// inherits its neighbours, as everywhere else.
fn row_sources(block: &TextBlock, draft: &str, spans: Vec<(std::ops::Range<usize>, usize)>) -> Vec<(std::ops::Range<usize>, usize)> {
    if draft.split('\n').count() != block.rows.len() {
        return spans;
    }
    let mut start = 0usize;
    let mut out = Vec::with_capacity(spans.len());
    for (row, text) in block.rows.iter().zip(draft.split('\n')) {
        let range = start..start.saturating_add(text.len());
        start = range.end.saturating_add(1);
        let inside: Vec<_> = spans
            .iter()
            .filter_map(|(r, index)| {
                let clipped = r.start.max(range.start)..r.end.min(range.end);
                (clipped.start < clipped.end).then_some((clipped, *index))
            })
            .collect();
        match row.first() {
            Some(own) if !inside.iter().any(|(_, index)| row.contains(index)) => out.extend(inside.into_iter().map(|(r, _)| (r, *own))),
            _ => out.extend(inside),
        }
    }
    // The line feeds between rows draw nothing; keep them with the span before them.
    out.sort_by_key(|(r, _)| r.start);
    out
}

/// Baseline positions that keep every row where it was: when the draft still has one row per
/// source row, none of them wraps and no width, alignment or move is asked for. Every row keeps
/// its source baseline and the edge the source rows share: the right one (the paragraph reads
/// right to left, or only the right edges line up), the left one, or their centre.
fn row_positions(block: &TextBlock, lines: &[TextLine], draft: &str, laid: &[appearance_text::Line], style: &BlockStyle) -> Option<Vec<[f64; 2]>> {
    if style.width.is_some()
        || style.align.is_some()
        || style.offset.is_some()
        || laid.len() != block.rows.len()
        || draft.split('\n').count() != laid.len()
    {
        return None;
    }
    let rects: Vec<[f64; 4]> = block
        .rows
        .iter()
        .map(|row| {
            row.iter().filter_map(|i| lines.get(*i)).map(|l| l.rect).reduce(|a, r| [a[0].min(r[0]), a[1].min(r[1]), a[2].max(r[2]), a[3].max(r[3])])
        })
        .collect::<Option<_>>()?;
    let near = |a: f64, b: f64| (a - b).abs() <= 2.0;
    let rtl = appearance_text::paragraph_rtl(draft);
    let (right, left) = (rects.iter().all(|r| near(r[2], block.rect[2])), rects.iter().all(|r| near(r[0], block.rect[0])));
    let centre = (block.rect[0] + block.rect[2]) / 2.0;
    let centred = rects.iter().all(|r| near((r[0] + r[2]) / 2.0, centre));
    block
        .rows
        .iter()
        .zip(laid)
        .zip(&rects)
        .map(|((row, line), rect)| {
            let first = lines.get(*row.first()?)?;
            let x = if right && (rtl || !left) {
                block.rect[2] - line.width
            } else if left {
                block.rect[0]
            } else if centred {
                centre - line.width / 2.0
            } else if rtl {
                rect[2] - line.width
            } else {
                rect[0]
            };
            [x, first.origin.baseline].iter().all(|v| v.is_finite()).then_some([x, first.origin.baseline])
        })
        .collect()
}

fn advance_op(advance: f64, state: &Ts) -> Result<Op, EditError> {
    let denominator = state.size * state.scale;
    let adjustment = if denominator != 0.0 { -1000.0 * advance / denominator } else { 0.0 };
    if !adjustment.is_finite() || (denominator == 0.0 && advance != 0.0) {
        return Err(EditError::Invalid("the source text advance cannot be restored".into()));
    }
    Ok(Op::new("TJ", vec![Object::Array(vec![pdfcraft_content::num(adjustment)])]))
}

pub(super) fn source_advance(source: &Shown, ops: &[Op]) -> Result<Vec<Op>, EditError> {
    let mut replacement = Vec::new();
    if let Some(op) = ops.get(source.op) {
        if op.is("\"") {
            replacement.push(Op::new("Tw", vec![op.operands.first().cloned().unwrap_or(Object::Int(0))]));
            replacement.push(Op::new("Tc", vec![op.operands.get(1).cloned().unwrap_or(Object::Int(0))]));
        }
        if op.is("'") || op.is("\"") {
            replacement.push(Op::new("T*", vec![]));
        }
    }
    replacement.push(advance_op(source.advance, &source.state)?);
    Ok(replacement)
}

pub(super) fn restore_text_matrix(source: &Shown) -> Result<Vec<Op>, EditError> {
    let tm = source.tm.0;
    let tlm = source.tlm.0;
    if tm.iter().chain(tlm.iter()).any(|v| !v.is_finite()) || tm[..4] != tlm[..4] {
        return Err(EditError::Invalid("the source text matrix cannot be restored".into()));
    }
    let mut restore = vec![Op::new("Tm", tlm.map(Object::Real).to_vec())];
    if tm[4..] != tlm[4..] {
        // Text-show operators translate only along the line's x axis. Restore TLM with Tm,
        // then restore that advance with TJ: setting Tm to TM alone would lose T* / Td's origin.
        let advance = if tlm[0].abs() >= tlm[1].abs() { (tm[4] - tlm[4]) / tlm[0] } else { (tm[5] - tlm[5]) / tlm[1] };
        restore.push(advance_op(advance, &source.state)?);
    }
    Ok(restore)
}
