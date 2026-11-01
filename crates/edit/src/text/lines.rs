use super::fallback::{same_font, same_paint};
use super::interp::{Carry, Origin, Pen, Shown, TextState, Ts, direction, interpret, to_frame, union};
use super::{
    DecodeBudget, Dict, Document, EditError, HashMap, Matrix, Metrics, Object, Op, Pieces, Stream, TextBlock, TextLine, parse, serialize_ops,
};

impl TextLine {
    /// The baseline's height in user space.
    pub fn origin_baseline(&self) -> f64 {
        self.origin.baseline
    }

    /// Paint position of the line's first operator: `(content-stream index, operator index in
    /// the page's joined stream)`. A line joins runs painted at different times, so anything
    /// painted after this position may lie on top of part of it.
    /// (Lines from `text_lines` never come from form XObjects, so `stream` is always a
    /// page content-stream index and `ops` are indices into the joined operator list.)
    pub fn first_paint_pos(&self) -> (usize, usize) {
        (self.stream, self.ops.iter().min().copied().unwrap_or(0))
    }

    /// The runs of text the line joins, in content order: each run's text as its glyphs show it,
    /// the boxes of its words (a part of `ink`), and whether joining put a space between it and
    /// the run before (`text` has one there; runs closer together join with none, so one word
    /// can span runs). None when the line's text was reordered for reading (right-to-left or
    /// visual-order Indic text): then only the line's text as a whole says what it reads.
    pub fn runs(&self) -> impl Iterator<Item = (&str, &[[f64; 4]], bool)> {
        let mut start = 0;
        self.runs.iter().map(move |(text, end, spaced)| {
            let ink = self.ink.get(start..*end).unwrap_or_default();
            start = *end;
            (text.as_str(), ink, *spaced)
        })
    }

    pub(crate) fn rotation(&self) -> f64 {
        let m = Matrix(self.origin.tm).then(&Matrix(self.origin.ctm)).0;
        m[1].atan2(m[0]).to_degrees()
    }

    /// The unit direction its baseline runs in, in user space.
    pub(super) fn direction(&self) -> (f64, f64) {
        direction(&Matrix(self.origin.tm).then(&Matrix(self.origin.ctm)))
    }

    /// Whether it reads left to right along the page's x axis (most text; a page drawn
    /// turned a quarter, as landscape pages often are, isn't).
    pub(crate) fn upright(&self) -> bool {
        let (x, y) = self.direction();
        (x - 1.0).abs() < 1e-3 && y.abs() < 1e-3
    }

    /// Where it starts as `(along, across)` its baseline: user space turned to the text's direction.
    pub(crate) fn frame_origin(&self) -> (f64, f64) {
        to_frame(self.direction(), (self.origin.x, self.origin.baseline))
    }

    /// Its box in the same frame, `[along0, across0, along1, across1]` (for upright text, `rect`).
    pub(crate) fn frame_rect(&self) -> [f64; 4] {
        self.frame
    }
}

/// The page's content streams as listed, each with its decoded data (empty when it doesn't
/// decode), within one page's [`DecodeBudget`]. An error when the content runs past it: reading
/// or rewriting a page from part of its content would lose the rest.
pub(crate) fn content_streams(doc: &Document, page: &Dict) -> Result<Vec<(Object, Vec<u8>)>, EditError> {
    content_streams_within(doc, page, &mut DecodeBudget::page())
}

/// [`content_streams`], decoded against `budget`, which a walk of the page's forms then goes on
/// spending.
pub(crate) fn content_streams_within(doc: &Document, page: &Dict, budget: &mut DecodeBudget) -> Result<Vec<(Object, Vec<u8>)>, EditError> {
    let list: Vec<Object> = match page.get(b"Contents") {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    };
    let mut out = Vec::with_capacity(list.len());
    for o in list {
        let data = match &*doc.resolve(&o) {
            Object::Stream(s) => match budget.decode(s) {
                Ok(data) => data,
                Err(e) if budget.remaining() == 0 => return Err(EditError::Invalid(format!("the page content is too large to read: {e}"))),
                Err(_) => Vec::new(),
            },
            _ => Vec::new(),
        };
        out.push((o, data));
    }
    Ok(out)
}

/// Rewrite a page's content (its streams joined, see [`Pieces`]): `edit(i)` gives the operators
/// to insert before operator `i` of `ops` (parsed from the joined pieces) and whether to keep it.
/// Everything else is copied byte for byte. A page's streams are one stream in pieces, split
/// between any two tokens (ISO 32000-2 §7.8.2), so an operator's operands can end one piece and
/// its keyword start the next; removing or replacing it rewrites every piece it spans. Returns
/// each piece's new bytes.
pub(crate) fn splice(pieces: &Pieces, ops: &[Op], mut edit: impl FnMut(usize) -> (Vec<Op>, bool)) -> Vec<Vec<u8>> {
    let data = pieces.data();
    let mut edits = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        let (insert, keep) = edit(i);
        if insert.is_empty() && keep {
            continue;
        }
        let mut bytes = Vec::new();
        if !insert.is_empty() {
            if op.span.start.checked_sub(1).and_then(|p| data.get(p)).is_some_and(|b| !b.is_ascii_whitespace()) {
                bytes.push(b'\n');
            }
            bytes.extend_from_slice(&serialize_ops(&insert));
        }
        let end = if keep { op.span.start } else { op.span.end };
        edits.push((op.span.start..end, bytes));
    }
    pieces.splice(edits)
}

/// The page's `/Contents` after a [`splice`]: pieces whose bytes changed become new streams
/// (keeping their dictionaries), the others stay as they were.
pub(crate) fn rewritten_contents(doc: &mut Document, streams: &[(Object, Vec<u8>)], new: Vec<Vec<u8>>) -> Object {
    let mut contents: Vec<Object> = Vec::with_capacity(streams.len());
    for ((obj, old), data) in streams.iter().zip(new) {
        if *old == data {
            contents.push(obj.clone());
            continue;
        }
        let mut dict = match &*doc.resolve(obj) {
            Object::Stream(s) => s.dict.clone(),
            _ => Dict::new(),
        };
        dict.remove(b"Length");
        contents.push(Object::Ref(doc.add(Object::Stream(Stream::flate(dict, &data)))));
    }
    match <[Object; 1]>::try_from(contents) {
        Ok([one]) => one,
        Err(many) => Object::Array(many),
    }
}

pub(crate) fn page_dict(doc: &Document, page: usize) -> Result<pdfcraft_model::Page, EditError> {
    pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))
}

fn fill_color(fill: &[Op]) -> [f64; 3] {
    let Some(op) = fill.iter().rev().find(|op| matches!(op.op.as_slice(), b"g" | b"rg" | b"k" | b"sc" | b"scn")) else {
        return [0.0, 0.0, 0.0];
    };
    let space = match op.op.as_slice() {
        b"sc" | b"scn" => match op.operands.len() {
            1 => b"g".as_slice(),
            3 => b"rg".as_slice(),
            4 => b"k".as_slice(),
            _ => return [0.0; 3],
        },
        other => other,
    };
    match space {
        b"g" => {
            let v = op.operands.first().and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0);
            [v, v, v]
        }
        b"rg" => [
            op.operands.first().and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
            op.operands.get(1).and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
            op.operands.get(2).and_then(Object::as_f64).unwrap_or(0.0).clamp(0.0, 1.0),
        ],
        b"k" => {
            let c = op.operands.first().and_then(Object::as_f64).unwrap_or(0.0);
            let m = op.operands.get(1).and_then(Object::as_f64).unwrap_or(0.0);
            let y = op.operands.get(2).and_then(Object::as_f64).unwrap_or(0.0);
            let k = op.operands.get(3).and_then(Object::as_f64).unwrap_or(0.0);
            [(1.0 - c) * (1.0 - k), (1.0 - m) * (1.0 - k), (1.0 - y) * (1.0 - k)]
        }
        _ => [0.0, 0.0, 0.0],
    }
}

/// The lines of text on a page (0-based), in content order.
pub fn text_lines(doc: &Document, page: usize) -> Result<Vec<TextLine>, EditError> {
    text_lines_in(doc, &page_dict(doc, page)?, true, false).map(|(lines, _)| lines)
}

/// Private state runs refine each public line, never its public index or paragraph membership.
pub(super) fn text_lines_with_runs(doc: &Document, page: usize, keep_runs: bool) -> Result<(Vec<TextLine>, Vec<Vec<TextLine>>), EditError> {
    text_lines_in(doc, &page_dict(doc, page)?, true, keep_runs)
}

/// The page's drawn glyphs without ActualText replacements, for redaction auditing.
pub fn glyph_text_lines(doc: &Document, page: &pdfcraft_model::Page) -> Result<Vec<TextLine>, EditError> {
    text_lines_in(doc, page, false, false).map(|(lines, _)| lines)
}

fn text_lines_in(
    doc: &Document,
    p: &pdfcraft_model::Page,
    actual_text: bool,
    keep_runs: bool,
) -> Result<(Vec<TextLine>, Vec<Vec<TextLine>>), EditError> {
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut cache = HashMap::new();
    let mut lines: Vec<TextLine> = Vec::new();
    let mut runs: Vec<Vec<TextLine>> = Vec::new();
    let mut run_last = None;
    let mut carry = Carry::new();
    // The streams are one stream in pieces (§7.8.2), split between any two tokens: parse them
    // joined, so an operator whose operands end one piece still has them.
    let pieces = Pieces::join(&content_streams(doc, &p.dict)?.into_iter().map(|(_, d)| d).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let properties = res.get(b"Properties").and_then(|o| doc.dict(o)).unwrap_or_default();
    let shown = interpret(doc, &ops, &res, &mut cache, &mut carry, None, actual_text.then_some(&properties))?;
    let mut last: Option<(usize, Pen)> = None; // the stream piece and where its last run ended
    for mut s in shown {
        // A single RTL glyph's ActualText (e.g. lam-alef) is an inline ligature, not a
        // separate logical line. Put its letters into visual order before joining the
        // surrounding glyphs; the completed line is recovered to typing order below.
        // Multi-glyph and mixed-direction replacements keep their authoritative order.
        if s.logical_order && s.glyph_count == 1 && s.text.chars().all(crate::bidi::is_rtl) {
            s.text = crate::bidi::to_visual(&s.text);
            s.logical_order = false;
        }
        // A line belongs to one stream piece. Within it, PDF producers often put every word
        // in its own BT/ET: geometry and text state, not object boundaries, define a line.
        let si = ops.get(s.op).map_or(0, |op| pieces.pieces_of(op).0);
        let joins = last.is_some_and(|(piece, pen)| {
            let bt = pen.bt;
            let (near, gap) = pen.continues(&s, if bt == s.bt { 3.0 } else { 1.5 });
            piece == si
                && lines.last().is_some_and(|l| l.scope == s.scope && l.paint == s.paint && l.logical_order == s.logical_order)
                && (bt == s.bt || lines.last().is_some_and(|l| compatible_word(l, &s)))
                && (s.text.trim().is_empty() || lines.last().is_some_and(|l| same_font(l, &s.font, &s.base_font, s.face.as_deref())))
                && (s.text.trim().is_empty()
                    || lines.last().is_some_and(|l| {
                        (s.size - l.size).abs() < 0.01
                            && s.bold == l.bold
                            && s.italic == l.italic
                            && fill_color(&s.state.fill) == l.color
                            && same_paint(l, &s.state)
                    }))
                && near
                // An OCR layer is one text object over the whole scan: its words continue a line
                // only across a word space, not across a table cell or a tab stop.
                && (s.state.render_mode != 3 || gap < pen.size * 0.6)
        });
        if keep_runs {
            if !joins {
                runs.push(Vec::new());
                run_last = None;
            }
            if let Some(parts) = runs.last_mut() {
                add_shown(parts, &mut run_last, &s, si, true);
            }
        }
        if joins && let Some(l) = lines.last_mut() {
            let gap = last.map_or(0.0, |(_, pen)| pen.continues(&s, 0.0).1);
            let spaced = gap - s.tracking > l.size * s.state.scale.abs() * 0.2 && !l.text.ends_with(' ') && !s.text.starts_with(' ');
            if spaced {
                l.text.push(' ');
            }
            l.text.push_str(&s.text);
            l.rect = [l.rect[0].min(s.rect[0]), l.rect[1].min(s.rect[1]), l.rect[2].max(s.rect[2]), l.rect[3].max(s.rect[3])];
            l.frame = union(l.frame, s.frame);
            l.ink.extend(s.ink.iter().copied());
            l.runs.push((s.text.clone(), l.ink.len(), spaced));
            l.ops.push(s.op);
            l.ops.extend(s.extra_ops.iter().copied());
            l.decodable &= s.decodable;
            l.visual_order |= s.visual_order;
            l.actual_text |= s.actual_text;
            l.clips |= s.state.render_mode >= 4;
            l.positioned |= !sequential_show(last.map(|(_, pen)| pen), &s);
            l.mixed_appearance |= l.origin.state.render_mode != s.state.render_mode || l.origin.state.rise != s.state.rise;
            if l.unreadable_mode.is_none() {
                l.unreadable_mode = s.state.unreadable.as_deref().map(str::to_owned);
            }
            // Spaces may use a separate subset font; they do not change the visible style.
        } else {
            lines.push(TextLine {
                text: s.text.clone(),
                rect: s.rect,
                frame: s.frame,
                ink: s.ink.clone(),
                font: String::from_utf8_lossy(&s.font).into_owned(),
                base_font: s.base_font.clone(),
                size: s.size,
                bold: s.bold,
                italic: s.italic,
                color: fill_color(&s.state.fill),
                decodable: s.decodable,
                visual_order: s.visual_order,
                face: s.face.clone(),
                stream: si,
                scope: s.scope,
                paint: s.paint.clone(),
                logical_order: s.logical_order,
                actual_text: s.actual_text,
                clips: s.state.render_mode >= 4,
                positioned: false,
                mixed_appearance: false,
                unreadable_mode: s.state.unreadable.as_deref().map(str::to_owned),
                runs: vec![(s.text.clone(), s.ink.len(), false)],
                soft_break: s.soft_break,
                hard_break: s.hard_break,
                soft_gap: s.soft_gap,
                ops: std::iter::once(s.op).chain(s.extra_ops.iter().copied()).collect(),
                origin: Origin {
                    tm: s.tm.0,
                    tlm: s.tlm.0,
                    k: (s.tm.then(&s.state.ctm).0[2].powi(2) + s.tm.then(&s.state.ctm).0[3].powi(2)).sqrt(),
                    ctm: s.state.ctm.0,
                    baseline: s.baseline,
                    x: s.start_x,
                    state: TextState::of(&s.state),
                    bt_op: s.bt_op,
                    bt_state: TextState::of(&s.bt_state),
                },
            });
        }
        last = Some((si, Pen::after(&s)));
    }
    // Content streams draw right-to-left lines in visual order; the editor works in typing order.
    for l in lines.iter_mut().chain(runs.iter_mut().flatten()).filter(|l| !l.logical_order) {
        // A visual-order Indic font draws pre-base vowel signs before their consonant.
        let shown = l.text.clone();
        if l.visual_order && !l.actual_text {
            l.text = crate::indic::to_logical(&l.text);
        }
        l.text = crate::bidi::to_logical(&l.text);
        // The runs hold the text in drawing order, which no longer reads as the line does.
        if l.text != shown {
            l.runs.clear();
        }
    }
    // Lines of only spaces aren't editable text.
    if keep_runs {
        Ok(lines.into_iter().zip(runs).filter(|(line, _)| !line.text.trim().is_empty()).unzip())
    } else {
        lines.retain(|l| !l.text.trim().is_empty());
        Ok((lines, runs))
    }
}

/// Only allow arithmetic roundoff when checking whether shows use sequential pen positions.
fn sequential_show(last: Option<Pen>, shown: &Shown) -> bool {
    let Some(last) = last else { return true };
    let (_, start, _, across) = shown.span();
    let same = |a: f64, b: f64| a.is_finite() && b.is_finite() && (a - b).abs() <= f64::EPSILON * 8.0 * a.abs().max(b.abs()).max(1.0);
    same(last.end, start) && same(last.across, across)
}

/// One public line as an editor row, without selecting its neighbouring paragraph lines.
pub(super) fn single_line_block(line: &TextLine, index: usize) -> TextBlock {
    TextBlock {
        text: line.text.clone(),
        editor_text: line.text.clone(),
        reflow_text: line.text.clone(),
        rect: line.rect,
        base_font: line.base_font.clone(),
        size: line.size,
        bold: line.bold,
        italic: line.italic,
        color: line.color,
        rotation: line.rotation(),
        lines: vec![index],
        rows: vec![vec![index]],
        spans: vec![(0..line.text.len(), index)],
        invisible: line.origin.state.render_mode == 3,
    }
}

/// A separate paragraph text object cannot preserve an unselected show that relies on the
/// selected show's advance. Absolute/line positioning ends that dependency; a line splice
/// does not need this restriction because it retains the original text matrix.
pub(super) fn refuse_dependent_following_text(doc: &Document, page: usize, dropped: &std::collections::HashSet<usize>) -> Result<(), EditError> {
    let page = page_dict(doc, page)?;
    let pieces = Pieces::join(&content_streams(doc, &page.dict)?.into_iter().map(|(_, data)| data).collect::<Vec<_>>());
    let mut dependent = false;
    for (index, op) in pieces.parse().ops.iter().enumerate() {
        if matches!(op.op.as_slice(), b"BT" | b"ET" | b"Tm" | b"Td" | b"TD" | b"T*" | b"'" | b"\"") {
            dependent = false;
        }
        if dropped.contains(&index) {
            dependent = true;
        } else if dependent && matches!(op.op.as_slice(), b"Tj" | b"TJ") {
            return Err(EditError::Invalid(
                "mixed text spacing cannot be rewritten without moving following text; the document was not changed".into(),
            ));
        }
    }
    Ok(())
}

/// Sparse, retained TJ displacements in page points. Internal boundaries are emitted after
/// their left glyph, never as an indent on a newly wrapped row. Only source start sentinels
/// are emitted before a glyph. Numeric-only source operators are consumed exactly once.
#[derive(Clone, Default)]
pub(super) struct TextPositioning {
    pub(super) source: bool,
    pub(super) ends: std::collections::HashSet<usize>,
    pub(super) before: std::collections::BTreeMap<usize, Vec<f64>>,
    pub(super) after: std::collections::BTreeMap<usize, Vec<f64>>,
    pub(super) drops: std::collections::HashSet<usize>,
}

impl TextPositioning {
    pub(super) fn is_empty(&self) -> bool {
        self.before.is_empty() && self.after.is_empty()
    }

    pub(super) fn internal_after(&self, byte: usize, ch: char) -> f64 {
        let end = byte.saturating_add(ch.len_utf8());
        if self.ends.contains(&end) { 0.0 } else { self.after.get(&end).into_iter().flatten().sum() }
    }

    pub(super) fn advance(&self, byte: usize, ch: char) -> f64 {
        self.before.get(&byte).into_iter().flatten().chain(self.after.get(&byte.saturating_add(ch.len_utf8())).into_iter().flatten()).sum()
    }
}

pub(super) struct SpacingExpansion {
    pub(super) lines: Vec<TextLine>,
    pub(super) block: TextBlock,
    pub(super) positioning: TextPositioning,
}

/// Decode only selected sequential-horizontal TJ programs. Unicode must round-trip to the
/// original code bytes: a second code for the same character may have a different width.
/// State at each numeric operand matters, including numeric-only TJ between state changes.
fn text_positioning(
    doc: &Document,
    fonts: &Dict,
    ops: &[Op],
    lines: &[TextLine],
    runs: &[Vec<TextLine>],
    block: &TextBlock,
    draft: &str,
) -> Result<(TextPositioning, std::collections::HashSet<usize>), EditError> {
    let invalid = || EditError::Invalid("the source TJ positioning cannot be mapped safely; the document was not changed".into());
    let mut source = Vec::new();
    let mut positioned = std::collections::HashSet::new();
    let mut result = TextPositioning::default();
    let mut count = 0_usize;
    for (range, index) in &block.spans {
        let line = lines.get(*index).ok_or_else(invalid)?;
        let first = line.ops.iter().min().copied().ok_or_else(invalid)?;
        let last = line.ops.iter().max().copied().ok_or_else(invalid)?;
        let interval = ops.get(first..=last).ok_or_else(invalid)?;
        if !interval.iter().any(|op| {
            op.is("TJ")
                && op.operands.last().and_then(Object::as_array).is_some_and(|values| values.iter().any(|v| v.as_f64().is_some_and(|n| n != 0.0)))
        }) {
            continue;
        }
        let font = fonts.get(line.font.as_bytes()).and_then(|f| doc.dict(f)).ok_or_else(invalid)?;
        let metrics = Metrics::from_dict(doc, &font);
        let mut state = line.origin.state.clone();
        let size = state.font.as_ref().map(|(_, size)| *size).ok_or_else(invalid)?;
        let matrix = Matrix(line.origin.tm).then(&Matrix(line.origin.ctm));
        let across = matrix.0[0].hypot(matrix.0[1]);
        let states: HashMap<_, _> =
            runs.get(*index).ok_or_else(invalid)?.iter().flat_map(|part| part.ops.iter().map(move |op| (*op, &part.origin.state))).collect();
        let mut decoded = String::new();
        let mut advance = 0.0;
        let mut gaps: std::collections::BTreeMap<usize, Vec<f64>> = std::collections::BTreeMap::new();
        for (offset, op) in interval.iter().enumerate() {
            let at = first.checked_add(offset).ok_or_else(invalid)?;
            match op.op.as_slice() {
                b"Tc" => state.char_spacing = op.num(0).ok_or_else(invalid)?,
                b"Tw" => state.word_spacing = op.num(0).ok_or_else(invalid)?,
                b"Tz" => state.scale = op.num(0).ok_or_else(invalid)? / 100.0,
                b"Tf" => {
                    if op.name(0) != Some(line.font.as_bytes()) || op.num(1) != Some(size) {
                        return Err(invalid());
                    }
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    if at != first && matches!(op.op.as_slice(), b"'" | b"\"") {
                        return Err(invalid());
                    }
                    let pieces = if op.is("TJ") {
                        op.operands.last().and_then(Object::as_array).ok_or_else(invalid)?.as_slice()
                    } else {
                        std::slice::from_ref(op.operands.last().ok_or_else(invalid)?)
                    };
                    if let Some(original) = states.get(&at) {
                        if state.char_spacing != original.char_spacing || state.word_spacing != original.word_spacing || state.scale != original.scale
                        {
                            return Err(invalid());
                        }
                    } else if pieces.iter().any(|v| matches!(v, Object::String(s) if !s.bytes.is_empty())) {
                        return Err(invalid());
                    } else {
                        result.drops.insert(at);
                    }
                    for piece in pieces {
                        if let Object::String(string) = piece {
                            let mut byte = 0_usize;
                            for (code, len) in metrics.codes(&string.bytes) {
                                let end = byte.checked_add(len).ok_or_else(invalid)?;
                                let raw = string.bytes.get(byte..end).ok_or_else(invalid)?;
                                let text = metrics.text_of(code).ok_or_else(invalid)?;
                                if text.chars().count() != 1 || metrics.encode(&text).as_deref() != Some(raw) {
                                    return Err(invalid());
                                }
                                advance += (metrics.width(code) * size
                                    + state.char_spacing
                                    + if metrics.is_space(code, len) { state.word_spacing } else { 0.0 })
                                    * state.scale
                                    * across;
                                if !advance.is_finite() {
                                    return Err(invalid());
                                }
                                decoded.push_str(&text);
                                if decoded.len() > 4_000_000 {
                                    return Err(invalid());
                                }
                                byte = end;
                            }
                            if byte != string.bytes.len() {
                                return Err(invalid());
                            }
                        } else {
                            let number = piece.as_f64().ok_or_else(invalid)?;
                            let gap = -number / 1000.0 * size * state.scale * across;
                            if !number.is_finite() || !gap.is_finite() || !state.scale.is_finite() || state.scale <= 0.0 {
                                return Err(invalid());
                            }
                            advance += gap;
                            if !advance.is_finite() {
                                return Err(invalid());
                            }
                            if number != 0.0 {
                                count = count.checked_add(1).filter(|n| *n <= 100_000).ok_or_else(invalid)?;
                                gaps.entry(range.start.checked_add(decoded.len()).ok_or_else(invalid)?).or_default().push(gap);
                            }
                        }
                    }
                }
                // These do not move the text matrix. All explicit positioning, graphics
                // transforms/restores and unknown operators remain outside this bounded path.
                b"TL" | b"Tr" | b"Ts" | b"g" | b"rg" | b"k" | b"cs" | b"sc" | b"scn" | b"BMC" | b"BDC" | b"EMC" | b"MP" | b"DP" => {}
                _ => return Err(invalid()),
            }
        }
        if decoded != line.text || block.editor_text.get(range.clone()) != Some(decoded.as_str()) {
            return Err(invalid());
        }
        // Large positive offsets between letters are inferred tracking/word gaps, whose
        // extraction changes when the retained neighbour count changes. Do not normalize them.
        let minimum_em = states.values().map(|s| size * s.scale * across).fold(f64::INFINITY, f64::min);
        for (at, values) in &gaps {
            let local = at.checked_sub(range.start).ok_or_else(invalid)?;
            let left = decoded.get(..local).and_then(|s| s.chars().next_back());
            let right = decoded.get(local..).and_then(|s| s.chars().next());
            if left.is_some_and(|c| !c.is_whitespace())
                && right.is_some_and(|c| !c.is_whitespace())
                && values.iter().any(|gap| *gap > minimum_em * 0.2)
            {
                return Err(invalid());
            }
        }
        result.source = true;
        positioned.insert(*index);
        for (at, values) in gaps {
            source.push((range.clone(), at, values));
        }
    }
    if source.is_empty() {
        return Ok((result, positioned));
    }
    let mapping = block.edited_char_sources(draft);
    let new: Vec<_> = draft.char_indices().collect();
    let wanted: std::collections::HashSet<_> = source
        .iter()
        .flat_map(|(range, at, _)| {
            [(*at < range.end).then_some(*at), block.editor_text.get(range.start..*at).and_then(|s| s.chars().next_back()).map(|c| at - c.len_utf8())]
        })
        .flatten()
        .collect();
    let retained: HashMap<_, _> =
        mapping.iter().enumerate().filter_map(|(i, (_, old))| old.filter(|old| wanted.contains(old)).map(|old| (old, i))).collect();
    for (range, at, values) in source {
        let left = block.editor_text.get(range.start..at).and_then(|s| s.chars().next_back()).map(|c| at - c.len_utf8());
        let right = (at < range.end).then_some(at);
        let boundary = match (left, right) {
            (Some(left), Some(right)) => retained
                .get(&left)
                .zip(retained.get(&right))
                .filter(|(a, b)| a.checked_add(1) == Some(**b))
                .and_then(|(_, b)| new.get(*b).map(|(byte, _)| *byte)),
            (None, Some(right)) => retained
                .get(&right)
                .filter(|i| **i == 0 || i.checked_sub(1).and_then(|n| mapping.get(n)).and_then(|(_, old)| *old).is_some_and(|old| old < range.start))
                .and_then(|i| new.get(*i).map(|(byte, _)| *byte)),
            (Some(left), None) => retained
                .get(&left)
                .filter(|i| {
                    i.checked_add(1) == Some(mapping.len())
                        || i.checked_add(1).and_then(|n| mapping.get(n)).and_then(|(_, old)| *old).is_some_and(|old| old >= range.end)
                })
                .and_then(|i| new.get(*i).map(|(byte, ch)| byte.saturating_add(ch.len_utf8()))),
            _ => None,
        };
        if let Some(byte) = boundary {
            if right.is_none() {
                result.ends.insert(byte);
            }
            let target = if left.is_none() { &mut result.before } else { &mut result.after };
            target.entry(byte).or_default().extend(values);
        }
    }
    Ok((result, positioned))
}

/// Refine selected public rows, preserving their exact text, boundaries and source operators.
/// Unselected page lines remain available to the existing coincidence/clipping checks.
pub(super) fn expand_spacing_runs(
    doc: &Document,
    page: usize,
    lines: &[TextLine],
    runs: &[Vec<TextLine>],
    block: &TextBlock,
    draft: &str,
    paragraph: bool,
) -> Result<Option<SpacingExpansion>, EditError> {
    if !block.lines.iter().any(|i| runs.get(*i).is_some_and(|parts| parts.len() > 1)) {
        return Ok(None);
    }
    let invalid = || EditError::Invalid("mixed text spacing cannot be mapped safely to this text; the document was not changed".into());
    if draft.len() > 4_000_000
        || block.editor_text.len() > 4_000_000
        || draft.chars().any(crate::bidi::is_rtl)
        || pdfcraft_fonts::source_shaping::required(draft)
    {
        return Err(invalid());
    }
    // Exact TJ boundaries are retained separately from style inheritance. Explicit matrix
    // positioning remains unsupported, even when it has the same net advance.
    let page_dict = page_dict(doc, page)?;
    let pieces = Pieces::join(&content_streams(doc, &page_dict.dict)?.into_iter().map(|(_, data)| data).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let resources = page_dict.dict.get(b"Resources").and_then(|r| doc.dict(r)).unwrap_or_default();
    let fonts = resources.get(b"Font").and_then(|f| doc.dict(f)).unwrap_or_default();
    let (positioning, positioned) = text_positioning(doc, &fonts, &ops, lines, runs, block, draft)?;
    // Adjacent public font/color runs are not row-edge sentinels. Preserve their established
    // writer until source-glyph identity across those separate semantic lines is supported.
    if block.rows.iter().any(|row| row.len() > 1 && row.iter().any(|index| positioned.contains(index))) {
        return Err(invalid());
    }
    for index in &block.lines {
        let line = lines.get(*index).ok_or_else(invalid)?;
        let first = line.ops.iter().min().copied().ok_or_else(invalid)?;
        let last = line.ops.iter().max().copied().ok_or_else(invalid)?;
        let start = first.checked_add(1).ok_or_else(invalid)?;
        // Upstream groups sequential words across separate BT/ET objects. Their initial
        // matrices are safe only when every show exactly continues the preceding pen.
        let sequential_objects =
            paragraph && !line.positioned && runs.get(*index).is_some_and(|parts| parts.iter().any(|part| part.origin.bt_op != line.origin.bt_op));
        let mut shown = true;
        for op in ops.get(start..=last).unwrap_or_default() {
            match op.op.as_slice() {
                b"BT" if sequential_objects => shown = false,
                b"ET" if sequential_objects => {}
                b"Tm" | b"Td" | b"TD" if sequential_objects && !shown => {}
                b"Tj" | b"TJ" => shown = true,
                b"Tm" | b"Td" | b"TD" | b"T*" | b"'" | b"\"" | b"BT" | b"ET" => return Err(invalid()),
                _ => {}
            }
        }
    }
    for row in &block.rows {
        let first = row.first().and_then(|i| lines.get(*i)).ok_or_else(invalid)?;
        if row.iter().filter_map(|i| lines.get(*i)).any(|line| {
            line.origin.tm.get(..4) != first.origin.tm.get(..4)
                || line.origin.ctm != first.origin.ctm
                || line.origin.baseline != first.origin.baseline
        }) {
            return Err(invalid());
        }
    }
    let selected: std::collections::HashSet<_> = block.lines.iter().copied().collect();
    let mut refined = Vec::new();
    let mut indexes: Vec<Vec<usize>> = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        let parts = if selected.contains(&index) { runs.get(index).map(Vec::as_slice).ok_or_else(invalid)? } else { std::slice::from_ref(line) };
        if selected.contains(&index) {
            if parts.is_empty()
                || !line.upright()
                || line.visual_order
                || line.actual_text
                || (line.positioned && !positioned.contains(&index))
                || line.mixed_appearance
                || line.text.chars().any(crate::bidi::is_rtl)
            {
                return Err(invalid());
            }
            let text: String = parts.iter().map(|part| part.text.as_str()).collect();
            let source_ops: std::collections::HashSet<_> = line.ops.iter().copied().collect();
            let private_ops: std::collections::HashSet<_> = parts.iter().flat_map(|part| part.ops.iter().copied()).collect();
            if text != line.text
                || source_ops != private_ops
                || parts.iter().any(|part| {
                    let transform = Matrix(part.origin.tm).then(&Matrix(part.origin.ctm));
                    let across = transform.0[0].hypot(transform.0[1]);
                    !part.upright()
                        || !across.is_finite()
                        || across <= 0.0
                        || transform.0.get(1) != Some(&0.0)
                        || transform.0.get(2) != Some(&0.0)
                        || part.origin.state.scale <= 0.0
                        || part.origin.state.font.as_ref().is_none_or(|(_, size)| !size.is_finite() || *size <= 0.0)
                        || !part.rect.iter().all(|n| n.is_finite())
                        || part.rect.first().is_some_and(|left| *left < part.origin.x)
                        || part.rect.get(2).zip(part.rect.first()).is_none_or(|(right, left)| right <= left)
                        || part.visual_order
                        || part.origin.tm.get(..4) != line.origin.tm.get(..4)
                        || part.origin.ctm != line.origin.ctm
                        || part.origin.state.font.as_ref().map(|(_, size)| size) != line.origin.state.font.as_ref().map(|(_, size)| size)
                        || !part
                            .origin
                            .tm
                            .iter()
                            .chain(&part.origin.ctm)
                            .chain([&part.origin.state.char_spacing, &part.origin.state.word_spacing, &part.origin.state.scale])
                            .all(|n| n.is_finite())
                })
            {
                return Err(invalid());
            }
        }
        let mut mapped = Vec::with_capacity(parts.len());
        for part in parts {
            mapped.push(refined.len());
            refined.push(part.clone());
        }
        indexes.push(mapped);
    }
    let map_indices = |source: &[usize]| -> Result<Vec<usize>, EditError> {
        let mut result = Vec::new();
        for index in source {
            result.extend(indexes.get(*index).ok_or_else(invalid)?.iter().copied());
        }
        Ok(result)
    };
    let mut expanded = block.clone();
    expanded.lines = map_indices(&block.lines)?;
    expanded.rows = block.rows.iter().map(|row| map_indices(row)).collect::<Result<_, _>>()?;
    expanded.spans.clear();
    for (range, index) in &block.spans {
        let line = lines.get(*index).ok_or_else(invalid)?;
        if block.editor_text.get(range.clone()) != Some(line.text.as_str()) {
            return Err(invalid());
        }
        let mut start = range.start;
        for mapped in indexes.get(*index).ok_or_else(invalid)? {
            let part = refined.get(*mapped).ok_or_else(invalid)?;
            let end = start.checked_add(part.text.len()).ok_or_else(invalid)?;
            if end > range.end || block.editor_text.get(start..end) != Some(part.text.as_str()) {
                return Err(invalid());
            }
            expanded.spans.push((start..end, *mapped));
            start = end;
        }
        if start != range.end {
            return Err(invalid());
        }
    }
    Ok(Some(SpacingExpansion { lines: refined, block: expanded, positioning }))
}

/// Do not round authoring-state precision: tiny spacing can be magnified by a text/CTM scale.
pub(super) fn spacing_ops(state: &TextState) -> Vec<Op> {
    vec![
        Op::new("Tc", vec![Object::Real(state.char_spacing)]),
        Op::new("Tw", vec![Object::Real(state.word_spacing)]),
        Op::new("Tz", vec![Object::Real(state.scale * 100.0)]),
    ]
}

/// Private authoring runs retain exact Tc/Tz, including producer jitter. Public lines keep
/// their existing grouping; only the rewrite needs these state boundaries.
fn same_character_spacing_and_scale(line: &TextLine, shown: &Shown) -> bool {
    line.origin.state.char_spacing == shown.state.char_spacing && line.origin.state.scale == shown.state.scale
}

/// Joining independent text objects is safe only when they use the same text and graphics
/// transforms and spacing. In particular, a nearby rotated label isn't part of this line.
fn compatible_word(line: &TextLine, shown: &Shown) -> bool {
    let within = |a: &[f64], b: &[f64], tolerance: f64| a.iter().zip(b).all(|(a, b)| (a - b).abs() < tolerance);
    let same = |a: &[f64], b: &[f64]| within(a, b, 1e-6);
    let (a, b) = (&line.origin.state, TextState::of(&shown.state));
    // Spacing that differs by less than a thousandth of an em, the unit glyph widths are given in,
    // can't be seen. Some producers still write it (`0.0001 Tc` on one word of a line).
    let em = shown.state.size.abs() / 1000.0;
    line.scope == shown.scope
        && line.paint == shown.paint
        && same(&line.origin.tm[..4], &shown.tm.0[..4])
        && same(&line.origin.ctm, &shown.state.ctm.0)
        && within(&[a.char_spacing, a.word_spacing], &[b.char_spacing, b.word_spacing], em.max(1e-6))
        && same(&[a.scale, a.rise], &[b.scale, b.rise])
        && a.fill.len() == b.fill.len()
        && a.fill.iter().zip(&b.fill).all(|(a, b)| a.op == b.op && a.operands == b.operands)
}

/// The line `s` continues, or a new one: `last` is the previous run of the same stream
/// (`bt`, baseline, end x, size).
fn add_shown(lines: &mut Vec<TextLine>, last: &mut Option<Pen>, s: &Shown, stream: usize, exact_spacing: bool) {
    let joins = last.is_some_and(|pen| {
        let bt = pen.bt;
        lines.last().is_some_and(|l| l.scope == s.scope && l.paint == s.paint && l.logical_order == s.logical_order)
            && (bt == s.bt || lines.last().is_some_and(|l| compatible_word(l, s)))
            && (!exact_spacing || s.text.trim().is_empty() || lines.last().is_some_and(|l| same_character_spacing_and_scale(l, s)))
            && (s.text.trim().is_empty() || lines.last().is_some_and(|l| same_font(l, &s.font, &s.base_font, s.face.as_deref())))
            && (s.text.trim().is_empty()
                || lines.last().is_some_and(|l| {
                    (s.size - l.size).abs() < 0.01
                        && s.bold == l.bold
                        && s.italic == l.italic
                        && fill_color(&s.state.fill) == l.color
                        && same_paint(l, &s.state)
                }))
            && pen.continues(s, if bt == s.bt { 3.0 } else { 1.5 }).0
    });
    if joins && let Some(l) = lines.last_mut() {
        let gap = last.map_or(0.0, |pen| pen.continues(s, 0.0).1);
        let spaced = gap - s.tracking > l.size * s.state.scale.abs() * 0.2 && !l.text.ends_with(' ') && !s.text.starts_with(' ');
        if spaced {
            l.text.push(' ');
        }
        l.text.push_str(&s.text);
        l.rect = [l.rect[0].min(s.rect[0]), l.rect[1].min(s.rect[1]), l.rect[2].max(s.rect[2]), l.rect[3].max(s.rect[3])];
        l.frame = union(l.frame, s.frame);
        l.ink.extend(s.ink.iter().copied());
        l.runs.push((s.text.clone(), l.ink.len(), spaced));
        l.ops.push(s.op);
        l.ops.extend(s.extra_ops.iter().copied());
        l.decodable &= s.decodable;
        l.visual_order |= s.visual_order;
        l.actual_text |= s.actual_text;
        l.clips |= s.state.render_mode >= 4;
        l.positioned |= !sequential_show(*last, s);
        l.mixed_appearance |= l.origin.state.render_mode != s.state.render_mode || l.origin.state.rise != s.state.rise;
        if l.unreadable_mode.is_none() {
            l.unreadable_mode = s.state.unreadable.as_deref().map(str::to_owned);
        }
    } else {
        lines.push(TextLine {
            text: s.text.clone(),
            rect: s.rect,
            frame: s.frame,
            ink: s.ink.clone(),
            font: String::from_utf8_lossy(&s.font).into_owned(),
            base_font: s.base_font.clone(),
            size: s.size,
            bold: s.bold,
            italic: s.italic,
            color: fill_color(&s.state.fill),
            decodable: s.decodable,
            visual_order: s.visual_order,
            face: s.face.clone(),
            stream,
            scope: s.scope,
            paint: s.paint.clone(),
            logical_order: s.logical_order,
            actual_text: s.actual_text,
            clips: s.state.render_mode >= 4,
            positioned: false,
            mixed_appearance: false,
            unreadable_mode: s.state.unreadable.as_deref().map(str::to_owned),
            runs: vec![(s.text.clone(), s.ink.len(), false)],
            soft_break: s.soft_break,
            hard_break: s.hard_break,
            soft_gap: s.soft_gap,
            ops: std::iter::once(s.op).chain(s.extra_ops.iter().copied()).collect(),
            origin: Origin {
                tm: s.tm.0,
                tlm: s.tlm.0,
                k: (s.tm.then(&s.state.ctm).0[2].powi(2) + s.tm.then(&s.state.ctm).0[3].powi(2)).sqrt(),
                ctm: s.state.ctm.0,
                baseline: s.baseline,
                x: s.start_x,
                state: TextState::of(&s.state),
                bt_op: s.bt_op,
                bt_state: TextState::of(&s.bt_state),
            },
        });
    }
    *last = Some(Pen::after(s));
}

/// How deep form XObjects nest before reading stops (they may also refer to themselves). The
/// renderer stops at the same depth, so export reads the text the page shows (#3120).
pub const MAX_FORM_DEPTH: usize = 50;

/// The most form XObjects one page's walk enters, at any depth. The depth cap alone still lets
/// a form that draws another form many times fan out exponentially (100 `Do`s on each of 50
/// levels), which would hang export on a hostile PDF. The renderer paints as many forms nested
/// in forms; this budget also counts the forms the page draws itself, so a page that draws one
/// a million times stays bounded too. A label or table cell drawn as a form a few thousand
/// times is read in full (#3120).
pub const MAX_FORM_VISITS: usize = 50_000;

/// One page's walk through the form XObjects it draws: how many it entered (the page's
/// [`MAX_FORM_VISITS`]), the bytes the page's content and forms may still decode to, and whether
/// a limit left a form unread.
pub(crate) struct FormBudget {
    visits: usize,
    decode: DecodeBudget,
    /// A form was left unread because it nests deeper than [`MAX_FORM_DEPTH`], comes past
    /// [`MAX_FORM_VISITS`] or past the decode budget: what it draws is missing from the walk.
    pub(crate) truncated: bool,
}

impl FormBudget {
    /// The walk of a page's forms, spending what `decode` has left after the page's content.
    pub(crate) fn new(decode: DecodeBudget) -> Self {
        Self { visits: 0, decode, truncated: false }
    }
}

/// A form XObject a page draws: its object, content, resources and its matrix composed with the
/// CTM at the `Do`.
pub(crate) struct FormCall {
    pub(crate) obj: Option<pdfcraft_cos::ObjRef>,
    pub(crate) data: Vec<u8>,
    pub(crate) resources: Dict,
    pub(crate) ctm: Matrix,
}

/// The form XObject `name` in `resources`, drawn with `ctm`. `None` for images, missing names
/// and forms already being read (`path`), and for forms nested too deep or past the page's
/// budget, which `budget` records as truncated; each form entered counts one visit.
pub(crate) fn form_call(
    doc: &Document,
    resources: &Dict,
    name: &[u8],
    ctm: Matrix,
    path: &[pdfcraft_cos::ObjRef],
    budget: &mut FormBudget,
) -> Option<FormCall> {
    let limited = path.len() >= MAX_FORM_DEPTH || budget.visits >= MAX_FORM_VISITS || budget.decode.remaining() == 0;
    // Once a limit has cut the page short, refusing the rest costs nothing more.
    if limited && budget.truncated {
        return None;
    }
    let xobjects = resources.get(b"XObject").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned())?;
    let o = xobjects.get(name)?;
    let obj = o.as_ref();
    if obj.is_some_and(|r| path.contains(&r)) {
        return None;
    }
    let Object::Stream(s) = &*doc.resolve(o) else { return None };
    if s.dict.name(b"Subtype") != Some(b"Form") {
        return None;
    }
    // Only a form left unread loses content; an image at the limit is still read by its caller.
    if limited {
        budget.truncated = true;
        return None;
    }
    let m = s.dict.get(b"Matrix").map(|m| doc.resolve(m)).and_then(|m| {
        let a = m.as_array()?;
        let v: Vec<f64> = a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
        <[f64; 6]>::try_from(v).ok().filter(|v| v.iter().all(|x| x.is_finite()))
    });
    // A form without its own resources uses the ones it was drawn with (PDF 1.1 files).
    let own = s.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
    let data = match budget.decode.decode(s) {
        Ok(data) => data,
        Err(_) if budget.decode.remaining() == 0 => {
            budget.truncated = true;
            return None;
        }
        Err(_) => Vec::new(),
    };
    budget.visits = budget.visits.saturating_add(1);
    Some(FormCall { obj, data, resources: own.unwrap_or_else(|| resources.clone()), ctm: m.map_or(ctm, |m| Matrix(m).then(&ctm)) })
}

/// The lines a page shows as it reads, including text drawn by form XObjects (which
/// [`text_lines`] leaves out because it can only rewrite the page's own streams). Lines from a
/// form carry a stream number past the page's streams, one per form drawn. The flag is `true`
/// when forms past [`MAX_FORM_DEPTH`] or [`MAX_FORM_VISITS`] were left unread.
pub(super) fn reading_lines(doc: &Document, page: usize) -> Result<(Vec<TextLine>, bool), EditError> {
    struct Walk<'a> {
        doc: &'a Document,
        lines: Vec<TextLine>,
        next_stream: usize,
        path: Vec<pdfcraft_cos::ObjRef>,
        budget: FormBudget,
    }
    /// `stream` gives the stream number of the operator at an index of `ops`.
    fn walk(w: &mut Walk, ops: &[Op], resources: &Dict, carry: &mut Carry, stream: &dyn Fn(usize) -> usize) -> Result<(), EditError> {
        let mut cache = HashMap::new();
        let mut invoked = Vec::new();
        let properties = resources.get(b"Properties").and_then(|o| w.doc.dict(o)).unwrap_or_default();
        let shown = interpret(w.doc, ops, resources, &mut cache, carry, Some(&mut invoked), Some(&properties))?;
        let mut last = None;
        let mut last_stream = None;
        let mut forms = invoked.into_iter().peekable();
        for s in shown {
            while let Some((_, name, ts)) = forms.next_if(|f| f.0 < s.op) {
                enter(w, resources, &name, ts, &mut carry.text_left)?;
                // Text after a form starts a new line.
                last = None;
            }
            let si = stream(s.op);
            // Text from another stream starts a new line, as when each stream was read alone.
            if last_stream.replace(si).is_some_and(|prev| prev != si) {
                last = None;
            }
            add_shown(&mut w.lines, &mut last, &s, si, false);
        }
        for (_, name, ts) in forms {
            enter(w, resources, &name, ts, &mut carry.text_left)?;
        }
        Ok(())
    }
    /// `text_left`: the page's text budget, shared with the forms the page draws.
    fn enter(w: &mut Walk, resources: &Dict, name: &[u8], ts: Ts, text_left: &mut usize) -> Result<(), EditError> {
        let Some(form) = form_call(w.doc, resources, name, ts.ctm, &w.path, &mut w.budget) else { return Ok(()) };
        let ops = parse(&form.data).ops;
        // The form starts with the graphics state at its `Do`, its own matrix applied.
        let mut carry = Carry { ts: Ts { ctm: form.ctm, ..ts }, stack: Vec::new(), marked: Vec::new(), layers: Vec::new(), text_left: *text_left };
        let stream = w.next_stream;
        w.next_stream = w.next_stream.saturating_add(1);
        if let Some(r) = form.obj {
            w.path.push(r);
        }
        walk(w, &ops, &form.resources, &mut carry, &|_| stream)?;
        *text_left = carry.text_left;
        if form.obj.is_some() {
            w.path.pop();
        }
        Ok(())
    }
    let p = page_dict(doc, page)?;
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut decode = DecodeBudget::page();
    let streams = content_streams_within(doc, &p.dict, &mut decode)?;
    let mut w = Walk { doc, lines: Vec::new(), next_stream: streams.len(), path: Vec::new(), budget: FormBudget::new(decode) };
    let mut carry = Carry::new();
    // Read the page's streams as the one stream they are, in one pass, as rendering and editing
    // do: a comment, an operator's operands or a marked-content sequence (its /ActualText too)
    // can run on from one stream into the next. Text still counts as the stream its operator
    // starts in, which keeps lines from different streams apart.
    let pieces = Pieces::join(&streams.into_iter().map(|(_, data)| data).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let stream_of = |i: usize| ops.get(i).map_or(0, |op| pieces.pieces_of(op).0);
    walk(&mut w, &ops, &res, &mut carry, &stream_of)?;
    let mut lines = w.lines;
    // A visual-order Indic font draws pre-base vowel signs before their consonant.
    for l in lines.iter_mut().filter(|l| l.visual_order && !l.actual_text) {
        let shown = l.text.clone();
        l.text = crate::indic::to_logical(&l.text);
        if l.text != shown {
            l.runs.clear();
        }
    }
    lines.retain(|l| !l.text.trim().is_empty());
    Ok((lines, w.budget.truncated))
}
