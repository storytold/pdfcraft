use super::blocks::{BlockStyle, TextBlock, paragraph_separator, wrap};
use super::fallback::{
    Encoder, MAX_TYPE3_GLYPHS, Type3Fallback, drop_unused_fallback_fonts, needs_type3, no_japanese_font, source_encodes, source_family,
    source_program_completes, source_show, source_space, source_width, type3_encode, type3_font,
};
use super::interp::{Carry, Origin, TextState, interpret};
use super::lines::{content_streams, page_dict, rewritten_contents, splice, text_lines};
use super::{
    Dict, Document, EditError, HashMap, Matrix, Metrics, Object, Op, PdfString, Pieces, TextLine, TextSession, alignment_frame, group_blocks,
    is_centred, parse, rewrite_styled_block,
};

/// What replacing a line did.
#[derive(Clone, Debug, PartialEq)]
pub struct LineEdit {
    /// `Some(font)` when the line's font couldn't show the new text and Helvetica was used.
    pub substituted: Option<String>,
}

/// Replace the text of line `line` (an index into [`text_lines`]) on `page` with `text`.
pub fn replace_line(doc: &mut Document, page: usize, line: usize, text: &str) -> Result<LineEdit, EditError> {
    let edit = replace_line_with_limit(doc, page, line, text, MAX_TYPE3_GLYPHS)?;
    drop_unused_fallback_fonts(doc, page);
    Ok(edit)
}

/// Refuses a rewrite that would remove text drawn as a clipping path (`4 Tr` to `7 Tr`). Such
/// glyphs are not ink but a mask, and whatever is painted through them takes their shape.
/// Removing the glyphs removes the mask, and the fill or image behind floods its own rectangle
/// instead -- the artwork is destroyed, in a file that still opens without a warning (#3429).
/// `clear_block` already refuses these for the same reason.
///
/// Every line the rewrite drops is checked, not only the one being replaced: a line's runs need
/// not share a rendering mode (see [`TextLine::clips`]), and [`coincident_ops`] drops a whole
/// overlapping line without regard to its mode -- so a mask could go with text that is ink.
fn refuse_clipping_text(lines: &[TextLine], dropped: &std::collections::HashSet<usize>) -> Result<(), EditError> {
    if lines.iter().any(|l| l.clips && l.ops.iter().any(|op| dropped.contains(op))) {
        return Err(EditError::Invalid("text used as a clipping path cannot be edited: its glyphs mask the artwork behind them".into()));
    }
    Ok(())
}

/// [`replace_line`] with the most unique characters one generated font may hold (tests lower it).
pub(crate) fn replace_line_with_limit(doc: &mut Document, page: usize, line: usize, text: &str, max_glyphs: usize) -> Result<LineEdit, EditError> {
    let text = text.replace(['\n', '\r'], " ");
    let lines = text_lines(doc, page)?;
    let target = lines.get(line).cloned().ok_or_else(|| EditError::Invalid(format!("page {} has no line {}", page + 1, line + 1)))?;
    target.ensure_editable()?;
    // Copies of the line drawn on the same baseline go with it, so the replacement is all that
    // shows. Which operators go is settled here, before any font is generated, so that a refusal
    // leaves the document untouched.
    let mut drops = coincident_ops(&lines, &[&target]);
    drops.extend(target.ops.iter().copied());
    refuse_clipping_text(&lines, &drops)?;
    let p = page_dict(doc, page)?;
    let mut res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut fonts_res = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let streams = content_streams(doc, &p.dict)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let first = target.ops.first().copied().filter(|i| *i < ops.len()).ok_or_else(|| EditError::Invalid("the page's content changed".into()))?;
    // The line's own font, when it can show every character.
    let font = fonts_res.get(target.font.as_bytes()).and_then(|f| doc.resolve(f).as_dict().cloned()).map(|d| Metrics::from_dict(doc, &d));
    let visual_indic = font.as_ref().is_some_and(|m| m.visual_order) || target.visual_order;
    let indic_text = if visual_indic { crate::indic::to_visual(&text) } else { text.clone() };
    // Typed in logical order; drawn left to right (right-to-left runs reversed).
    let shown = crate::bidi::to_visual(&indic_text);
    let reused = font.as_ref().filter(|m| source_encodes(m, &shown));
    let mut substituted = None;
    let mut replacement: Vec<Op> = Vec::new();
    // ' and " also move to the next line; keep that.
    match ops[first].op.as_slice() {
        b"'" => replacement.push(Op::new("T*", vec![])),
        b"\"" => {
            replacement.push(Op::new("Tw", vec![ops[first].operands.first().cloned().unwrap_or(Object::Int(0))]));
            replacement.push(Op::new("Tc", vec![ops[first].operands.get(1).cloned().unwrap_or(Object::Int(0))]));
            replacement.push(Op::new("T*", vec![]));
        }
        _ => {}
    }
    match reused {
        Some(metrics) => replacement.extend(source_show(
            metrics,
            &shown,
            target.origin.state.font.as_ref().map_or(target.size, |(_, size)| *size),
            source_space(doc, &fonts_res, metrics),
            target.origin.state.char_spacing,
            target.origin.state.word_spacing,
        )?),
        None => {
            let fallback_shown = crate::bidi::to_visual(&text);
            if needs_type3(&text) {
                let fallback = type3_font(doc, &mut fonts_res, &text, source_family(&target.base_font), target.bold, max_glyphs)?;
                let runs = type3_encode(&fallback, &fallback_shown)
                    .ok_or_else(|| EditError::Invalid(format!("\"{text}\" can't be shown by the {} fallback", fallback.family)))?;
                let size = font_size_before(&ops, first).unwrap_or(target.size);
                for (name, bytes) in runs {
                    replacement.push(Op::new("Tf", vec![Object::name(&name), pdfcraft_content::num(size)]));
                    replacement.push(Op::new("Tj", vec![Object::String(PdfString::literal(bytes))]));
                }
                replacement.push(Op::new("Tf", vec![Object::name(&target.font), pdfcraft_content::num(size)]));
                substituted = Some(format!("{} Type3", fallback.family));
            } else {
                let win = pdfcraft_fonts::win_ansi(&fallback_shown);
                // WinAnsi turns what it can't show into '?'; refuse rather than print the wrong thing.
                let back: String = win.iter().map(|b| char::from_u32(u32::from(*b)).unwrap_or('?')).collect();
                if fallback_shown.chars().zip(back.chars()).any(|(a, b)| b == '?' && a != '?') {
                    return Err(EditError::Invalid(format!("\"{text}\" has characters neither {} nor Helvetica can show", target.base_font)));
                }
                // The size in text space: the current Tf's size.
                let size = font_size_before(&ops, first).unwrap_or(target.size);
                let family = source_family(&target.base_font);
                let base = family.base_font(target.bold, target.italic);
                let substitute_name = format!("PCEd{}", base.replace('-', ""));
                replacement.push(Op::new("Tf", vec![Object::name(&substitute_name), pdfcraft_content::num(size)]));
                replacement.push(Op::new("Tj", vec![Object::String(PdfString::literal(win))]));
                replacement.push(Op::new("Tf", vec![Object::name(&target.font), pdfcraft_content::num(size)]));
                substituted = Some(base.to_string());
                let mut f = Dict::new();
                f.set(b"Type".to_vec(), Object::name("Font"));
                f.set(b"Subtype".to_vec(), Object::name("Type1"));
                f.set(b"BaseFont".to_vec(), Object::name(base));
                f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
                fonts_res.set(substitute_name.into_bytes(), Object::Dict(f));
            }
        }
    }
    // Rebuild: the line's first operator becomes the replacement, its others go.
    let new_data = splice(&pieces, &ops, |i| if i == first { (std::mem::take(&mut replacement), false) } else { (Vec::new(), !drops.contains(&i)) });
    let contents = rewritten_contents(doc, &streams, new_data);
    let page_ref = p.obj;
    if substituted.is_some() {
        res.set(b"Font".to_vec(), Object::Dict(fonts_res));
    }
    doc.update_dict(page_ref, |d| {
        d.set(b"Contents".to_vec(), contents);
        if substituted.is_some() {
            d.set(b"Resources".to_vec(), Object::Dict(res));
        }
    })?;
    Ok(LineEdit { substituted })
}

/// The size of the last `Tf` before operator `at`.
fn font_size_before(ops: &[Op], at: usize) -> Option<f64> {
    ops[..at].iter().rev().find(|o| o.is("Tf")).and_then(|o| o.num(1))
}

/// Operators of every line that is another drawing of one of `targets` (fake bold, an invisible
/// text layer). A surviving copy would show the old text under the replacement.
///
/// A copy overlaps a target by more than half the smaller box and sits on the same baseline
/// (within the tolerance that joins showing operators onto one line). A neighbouring line can
/// share box area when the leading is tighter than the font's ascent plus descent; its baseline
/// is farther away, so it stays.
pub(crate) fn coincident_ops(lines: &[TextLine], targets: &[&TextLine]) -> std::collections::HashSet<usize> {
    let area = |r: [f64; 4]| ((r[2] - r[0]) * (r[3] - r[1])).max(0.0);
    let mut drop = std::collections::HashSet::new();
    for line in lines {
        let copy = targets.iter().any(|target| {
            let same_text = line.text.trim() == target.text.trim() || (line.origin.state.render_mode == 3) != (target.origin.state.render_mode == 3);
            same_text && overlaps_half(line.rect, target.rect, area) && on_same_baseline(line, target)
        });
        if copy {
            drop.extend(line.ops.iter().copied());
        }
    }
    drop
}

fn overlaps_half(a: [f64; 4], b: [f64; 4], area: impl Fn([f64; 4]) -> f64) -> bool {
    let ix = (b[2].min(a[2]) - b[0].max(a[0])).max(0.0);
    let iy = (b[3].min(a[3]) - b[1].max(a[1])).max(0.0);
    let small = area(b).min(area(a));
    small > 0.0 && ix * iy / small > 0.5
}

/// Perpendicular distance between baselines, in user space. Horizontal text compares height;
/// turned text compares the distance across the writing direction.
fn on_same_baseline(line: &TextLine, target: &TextLine) -> bool {
    let (dir, target_dir) = (line.direction(), target.direction());
    let same_dir = (dir.0 - target_dir.0).abs() < 1e-3 && (dir.1 - target_dir.1).abs() < 1e-3;
    let same_size = line.size.min(target.size) >= line.size.max(target.size) * 0.8;
    let angle = target.rotation().to_radians();
    let (sin, cos) = if angle.is_finite() { angle.sin_cos() } else { (0.0, 1.0) };
    let dx = line.origin.x - target.origin.x;
    let dy = line.origin.baseline - target.origin.baseline;
    let offset = dx.mul_add(-sin, dy * cos).abs();
    let size = line.size.min(target.size).max(1.0);
    same_dir && same_size && offset.is_finite() && offset < size * 0.3
}

/// Rewrite paragraph `block` with new text (or its own) and formatting, rewrapped to its width.
pub fn rewrite_block(doc: &mut Document, page: usize, block: usize, text: Option<&str>, style: &BlockStyle) -> Result<LineEdit, EditError> {
    let edit = rewrite_block_with_limit(doc, page, block, text, style, MAX_TYPE3_GLYPHS)?;
    drop_unused_fallback_fonts(doc, page);
    Ok(edit)
}

/// [`rewrite_block`] with the most unique characters one generated font may hold (tests lower it).
pub(crate) fn rewrite_block_with_limit(
    doc: &mut Document,
    page: usize,
    block: usize,
    text: Option<&str>,
    style: &BlockStyle,
    max_glyphs: usize,
) -> Result<LineEdit, EditError> {
    TextSession::open(doc, page, block)?.commit_with_limit(doc, text, style, max_glyphs)
}

struct RewriteDispatch {
    upright: bool,
    use_styled_path: bool,
}

fn choose_rewrite_path(
    doc: &Document,
    fonts: &Dict,
    block: &TextBlock,
    members: &[&TextLine],
    first: &TextLine,
    text: Option<&str>,
    style: &BlockStyle,
) -> Result<RewriteDispatch, EditError> {
    let registered_face = fonts.iter().any(|(name, _)| name.starts_with(b"PCFull"));
    // Hebrew is written back in drawing order by the single-style path. Styled regions do not
    // reorder right-to-left runs.
    let hebrew = text.unwrap_or(&block.text).chars().any(crate::bidi::is_hebrew);
    if hebrew && (!style.ranges.is_empty() || style.resource.is_some()) {
        return Err(EditError::Invalid(
            "formatting part of right-to-left (Hebrew) text, or choosing a document font for it, isn't supported yet".into(),
        ));
    }
    // Turned text is rewritten in its own text space by the single-style path. Styled layout uses
    // page x-coordinates and cannot safely apply those operations to rotated runs.
    let upright = members.iter().all(|line| line.upright());
    if !upright && (!style.ranges.is_empty() || style.resource.is_some()) {
        return Err(EditError::Invalid("formatting part of turned text, or choosing a document font for it, isn't supported yet".into()));
    }
    let visual_indic = members.iter().any(|line| line.visual_order);
    let has_special_text = text.is_some_and(|draft| {
        draft.contains(['\n', '\r'])
            || pdfcraft_fonts::source_shaping::required(draft)
            || !pdfcraft_fonts::embedding::matching_faces(&block.base_font).is_empty()
    });
    let has_multiple_runs = block.rows.iter().any(|row| row.len() > 1)
        || members.iter().any(|line| line.font != first.font || line.size != first.size || line.color != first.color);
    let source_requires_styled_path =
        has_special_text || registered_face || text.is_some_and(|draft| source_program_completes(doc, fonts, first, draft)) || has_multiple_runs;
    let explicit_styled_text = !style.ranges.is_empty() || style.resource.is_some();
    Ok(RewriteDispatch {
        upright,
        use_styled_path: upright
            && !hebrew
            && !visual_indic
            && (explicit_styled_text || (source_requires_styled_path && style.family.is_none() && style.bold.is_none())),
    })
}

/// The paragraph's text for writing back: explicit line breaks stay (#1443), and so does each typed
/// U+0020 space: a run of them is not folded into one (#4199, #3797). `\r\n` and a lone `\r` are
/// breaks too; other white space is written as one space.
fn paragraph_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .map(|c| match c {
            '\r' => '\n',
            c if c != '\n' && paragraph_separator(c) => ' ',
            c => c,
        })
        .collect()
}

/// Where a paragraph sits on its page, for choosing the width it is laid out in.
struct ParaGeometry {
    rect: [f64; 4],
    frame: [f64; 4],
    crop: [f64; 4],
    upright: bool,
    /// The text-to-user scale.
    k: f64,
    /// The size in text space.
    size: f64,
    /// The requested horizontal move, in points.
    dx: f64,
    single_line: bool,
    anchored_right: bool,
    centred: bool,
}

impl ParaGeometry {
    /// A paragraph keeps its width (or takes the one asked for); a single line grows to the right
    /// (to the left when right-to-left), up to the page's margin.
    fn width(&self, requested: Option<f64>) -> Result<f64, EditError> {
        let [left, _, right, _] = self.rect;
        let own = (right - left).max(self.size * self.k);
        let width = match requested {
            // Capped at the largest page PDF allows (14 400 pt), even for text set larger than that
            // (`clamp` panics when its minimum is the larger, or NaN).
            Some(w) if w.is_finite() => w.clamp((self.size * self.k).min(14_400.0), 14_400.0),
            Some(_) => return Err(EditError::Invalid("the paragraph's width must be a number".into())),
            // Turned text: the page's margins are along another axis. A single line grows (below).
            None if !self.upright => (self.frame[2] - self.frame[0]).max(self.size * self.k),
            None if self.anchored_right => own.max(right + self.dx - (self.crop[0] + 36.0)),
            // A centred line is not stretched to the margin: its own box is the frame it is centred in.
            None if self.single_line && !self.centred => own.max(self.crop[2] - 36.0 - (left + self.dx)),
            None => own,
        };
        if width.is_finite() { Ok(width) } else { Err(EditError::Invalid("the paragraph's width must be a number".into())) }
    }
}

/// How far a baseline starting at `origin` and running along `direction` goes before it is 36 pt
/// from the page edge `low` or `high`.
fn margin_reach(origin: f64, direction: f64, low: f64, high: f64) -> f64 {
    if direction > 1e-6 {
        (high - 36.0 - origin) / direction
    } else if direction < -1e-6 {
        (low + 36.0 - origin) / direction
    } else {
        f64::INFINITY
    }
}

/// A turned single line grows to fit, up to where its baseline reaches the page's margin.
fn turned_line_width(first: &TextLine, crop: [f64; 4], offset: [f64; 2], width: f64, text: &str, advance: &Advance<'_>) -> f64 {
    let (x0, y0) = (first.origin.x + offset[0], first.origin.baseline + offset[1]);
    let (ux, uy) = first.direction();
    let tx = margin_reach(x0, ux, crop[0], crop[2]);
    let ty = margin_reach(y0, uy, crop[1], crop[3]);
    let cap = width.max(tx.min(ty)).min(14_400.0);
    text.split('\n').map(|line| advance.of(line)).fold(width, f64::max).min(cap)
}

/// The widths of the paragraph's text in the font it will be written in, in user space.
struct Advance<'a> {
    /// The source font, when its glyphs are reused.
    source: Option<&'a Metrics>,
    type3: Option<&'a Type3Fallback>,
    family: crate::added::Family,
    bold: bool,
    italic: bool,
    size: f64,
    space: f64,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    k: f64,
}

impl Advance<'_> {
    fn of(&self, s: &str) -> f64 {
        let extra = s.chars().count() as f64 * self.char_spacing;
        let t = match (self.source, self.type3) {
            (Some(m), _) => {
                let visual_s = if m.visual_order { crate::indic::to_visual(s) } else { s.to_owned() };
                source_width(m, &visual_s, self.size, self.space, self.char_spacing, self.word_spacing)
            }
            (None, Some(fallback)) => {
                // Each character's count in `s`, from one pass over `s`.
                let mut counts: HashMap<char, usize> = HashMap::new();
                for c in s.chars() {
                    let count = counts.entry(c).or_insert(0);
                    *count = count.saturating_add(1);
                }
                fallback.widths.iter().map(|(ch, width)| counts.get(ch).copied().unwrap_or(0) as f64 * width * self.size).sum::<f64>() + extra
            }
            (None, None) => self.family.std14(self.bold, self.italic).text_width(s, self.size) + extra,
        };
        t * self.scale * self.k
    }
}

/// The font a rewritten paragraph is shown in, and how its text is encoded.
struct FontChoice {
    show_font: String,
    encode: Encoder,
    /// The stand-in face to tell the caller about.
    substituted: Option<String>,
}

/// What [`choose_font`] needs to know about the paragraph.
struct FontInputs<'a> {
    /// The source font, when its glyphs are reused.
    source: Option<&'a Metrics>,
    type3: Option<&'a Type3Fallback>,
    font_name: &'a str,
    flat: &'a str,
    source_base_font: &'a str,
    family: crate::added::Family,
    bold: bool,
    italic: bool,
    family_requested: bool,
}

fn choose_font(inputs: &FontInputs<'_>, fonts_res: &mut Dict) -> Result<FontChoice, EditError> {
    if let Some(m) = inputs.source.cloned() {
        let name = inputs.font_name.to_owned();
        return Ok(FontChoice {
            show_font: name.clone(),
            encode: Box::new(move |s: &str| m.encode(s).map(|bytes| vec![(name.clone(), bytes)])),
            substituted: None,
        });
    }
    if let Some(fallback) = inputs.type3.cloned() {
        let substituted = Some(format!("{} Type3", fallback.family));
        let first = fallback.names.first().cloned().ok_or_else(no_japanese_font)?;
        return Ok(FontChoice { show_font: first, encode: Box::new(move |s: &str| type3_encode(&fallback, s)), substituted });
    }
    let flat = inputs.flat;
    let win = pdfcraft_fonts::win_ansi(flat);
    let back: String = win.iter().map(|c| char::from_u32(u32::from(*c)).unwrap_or('?')).collect();
    let base = inputs.family.base_font(inputs.bold, inputs.italic);
    if flat.chars().zip(back.chars()).any(|(a, c)| c == '?' && a != '?') {
        return Err(EditError::Invalid(format!("\"{flat}\" has characters neither {} nor {base} can show", inputs.source_base_font)));
    }
    let mut f = Dict::new();
    f.set(b"Type".to_vec(), Object::name("Font"));
    f.set(b"Subtype".to_vec(), Object::name("Type1"));
    f.set(b"BaseFont".to_vec(), Object::name(base));
    f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    // Named after its face, as the line and region rewrites name theirs: one name for every
    // stand-in would repoint the paragraphs set in an earlier one at this face (#4280).
    let name = format!("PCEd{}", base.replace('-', ""));
    fonts_res.set(name.clone().into_bytes(), Object::Dict(f));
    let substituted = (!inputs.family_requested).then(|| base.to_string());
    let shown = name.clone();
    Ok(FontChoice { show_font: name, encode: Box::new(move |s: &str| Some(vec![(shown.clone(), pdfcraft_fonts::win_ansi(s))])), substituted })
}

/// The paragraph's move in the space it is drawn in: the move is in page space, so it is taken
/// back through the CTM's linear part. Checked before the fallback fonts are written: a refused
/// paragraph leaves the document as it was.
fn drawn_movement(ctm: [f64; 6], dx: f64, dy: f64) -> Result<Option<(f64, f64)>, EditError> {
    if dx == 0.0 && dy == 0.0 {
        return Ok(None);
    }
    let back = Matrix([ctm[0], ctm[1], ctm[2], ctm[3], 0.0, 0.0])
        .invert()
        .ok_or_else(|| EditError::Invalid("the paragraph is drawn in a space it can't be moved in".into()))?;
    Ok(Some(back.apply(dx, dy)))
}

/// The paragraph's first text matrix, taken from where its text starts to where it is written.
fn emit_text_matrix(first: &Origin, emit: &Origin) -> Result<[f64; 6], EditError> {
    if first.ctm == emit.ctm {
        return Ok(first.tm);
    }
    let emit_inv = Matrix(emit.ctm).invert().ok_or_else(|| EditError::Invalid("the paragraph has a singular transform".into()))?;
    Ok(Matrix(first.tm).then(&Matrix(first.ctm)).then(&emit_inv).0)
}

/// Line spacing in text space: the paragraph's own (scaled with the size), or 1.2 × the size.
struct LeadScale {
    k: f64,
    size: f64,
    old_size: f64,
    leading: f64,
}

fn line_lead(block: &TextBlock, lines: &[TextLine], style: &BlockStyle, scale: &LeadScale) -> f64 {
    if let Some(m) = style.line_spacing {
        return scale.size * m.clamp(0.5, 5.0);
    }
    // From the first two rows (a row may hold several runs).
    let rows = block.rows.first().zip(block.rows.get(1)).and_then(|(upper, lower)| upper.first().zip(lower.first()));
    if let Some((upper, lower)) = rows.and_then(|(upper, lower)| lines.get(*upper).zip(lines.get(*lower))) {
        return (upper.frame_origin().1 - lower.frame_origin().1) / scale.k * scale.size / scale.old_size;
    }
    if scale.leading > 0.0 { scale.leading } else { scale.size * 1.2 }
}

/// The stroking twin of a fill colour operator.
fn stroke_twin(fill: &Op) -> Op {
    let name = match fill.op.as_slice() {
        b"g" => "G",
        b"rg" => "RG",
        b"k" => "K",
        b"cs" => "CS",
        b"sc" => "SC",
        _ => "SCN",
    };
    Op::new(name, fill.operands.clone())
}

/// Underlines in text space (the paragraph's text matrix), in its fill colour.
fn underline_ops(fill: &[Op], tm: &[f64; 6], size: f64, underlines: &[(f64, f64, f64)]) -> Vec<Op> {
    let n = pdfcraft_content::num;
    let mut ops = vec![Op::new("q", vec![]), Op::new("cm", tm.iter().map(|v| n(*v)).collect())];
    ops.extend(fill.iter().map(stroke_twin));
    ops.push(Op::new("w", vec![n(size * 0.06)]));
    for (x0, x1, y) in underlines {
        ops.push(Op::new("m", vec![n(*x0), n(*y)]));
        ops.push(Op::new("l", vec![n(*x1), n(*y)]));
    }
    ops.push(Op::new("S", vec![]));
    ops.push(Op::new("Q", vec![]));
    ops
}

/// Alignment: each line's offset from the left edge.
struct Alignment {
    width: f64,
    k: f64,
    anchored_right: bool,
    right_edge: f64,
    rtl: bool,
}

impl Alignment {
    /// `visible` is the width the line is aligned on, in user space, and `align` its row's
    /// alignment; the offset is in text space.
    fn offset(&self, visible: f64, align: Option<crate::added::Align>) -> f64 {
        let free = (self.width - visible).max(0.0) / self.k;
        match align {
            Some(crate::added::Align::Center) => free / 2.0,
            // A right-to-left line anchored at its right edge may start left of the old box.
            Some(crate::added::Align::Right) if self.anchored_right => (self.right_edge - visible) / self.k,
            Some(crate::added::Align::Right) => free,
            // A justified right-to-left paragraph ends its last line at the right edge.
            Some(crate::added::Align::Justify) if self.rtl => free,
            _ => 0.0,
        }
    }
}

/// Everything the wrapped lines of one paragraph are written with.
struct LineWriter<'a> {
    advance: &'a Advance<'a>,
    alignment: Alignment,
    encode: &'a Encoder,
    source: Option<&'a Metrics>,
    /// The paragraph's alignment, and the rows aligned on their own ([`BlockStyle::aligns`]).
    align: Option<crate::added::Align>,
    aligns: &'a [Option<crate::added::Align>],
    /// Whether `Tw` reaches every shown space.
    word_spacing_reaches_spaces: bool,
    width: f64,
    scale: f64,
    k: f64,
    size: f64,
    space: f64,
    char_spacing: f64,
    lead: f64,
}

/// What carries over from one line to the next.
struct LineCursor {
    x: f64,
    tw_set: f64,
    shown_font: String,
}

impl LineWriter<'_> {
    /// Row `row`'s alignment: its own, or the paragraph's.
    fn align_of(&self, row: usize) -> Option<crate::added::Align> {
        self.aligns.get(row).copied().flatten().or(self.align)
    }

    /// Justify: each line but the last fills the width. A stretched line shows only its words, as
    /// before #4199: word spacing would reach the spaces before its first word and after its
    /// last, so they are left out. Nothing to stretch over, or a paragraph drawn at `0 Tz`, isn't
    /// justified.
    fn stretch(&self, body: &str, last: bool, justify: bool) -> f64 {
        let interior = body.matches(' ').count();
        if justify && !last && interior > 0 && self.scale.abs() > 1e-6 {
            Some((self.width - self.advance.of(body)).max(0.0) / self.k / self.scale / interior as f64).filter(|tw| tw.is_finite()).unwrap_or(0.0)
        } else {
            0.0
        }
    }

    /// The text to show for a line and the width it is aligned on. Other lines show every space
    /// typed. A left-to-right one is aligned on its width up to its last word; a right-to-left
    /// one on its whole width, since the spaces typed after its last word are drawn on its left
    /// and the ones before its first on its right.
    fn shown<'t>(&self, line: &'t str, body: &'t str, tw: f64) -> (&'t str, f64) {
        if tw > 0.0 {
            (body, self.advance.of(body))
        } else if crate::bidi::rtl_base(line) {
            (line, self.advance.of(line))
        } else {
            (line, self.advance.of(line.trim_end_matches(' ')))
        }
    }

    /// Writes one wrapped line of its logical draft row, returning its underline (x0, x1, y) in text space. `wrap`: whether
    /// this line continues the previous one because a word did not fit, and, when it does, how
    /// many separator characters the wrap consumed (0 for a Chinese/Japanese word split).
    fn write_line(
        &self,
        index: usize,
        line: &str,
        wrap: (usize, bool, usize),
        last: bool,
        cursor: &mut LineCursor,
        ops: &mut Vec<Op>,
    ) -> Result<(f64, f64, f64), EditError> {
        let n = pdfcraft_content::num;
        let (row, soft, gap) = wrap;
        let line_align = self.align_of(row);
        let body = line.trim_matches(' ');
        let interior = body.matches(' ').count();
        let tw = self.stretch(body, last, line_align == Some(crate::added::Align::Justify));
        let (shown, visible) = self.shown(line, body, tw);
        let dx = if tw > 0.0 { 0.0 } else { self.alignment.offset(visible, line_align) };
        if index > 0 {
            // Inside this text object, so the marker starts and ends here. `/PCHard` is a newline
            // that was already in the text. `/PCSoft` rejoins when the box widens, with `/N` the
            // exact number of separator characters the wrap consumed (0 for a Chinese/Japanese
            // word split), so a rejoin restores what was actually there instead of guessing a
            // single space and silently dropping the rest of a typed run (#2529).
            if soft {
                let mut properties = Dict::new();
                properties.set(b"N".to_vec(), Object::Int(i64::try_from(gap).unwrap_or(i64::MAX)));
                ops.push(Op::new("BDC", vec![Object::name("PCSoft"), Object::Dict(properties)]));
            } else {
                ops.push(Op::new("BMC", vec![Object::name("PCHard")]));
            }
            ops.push(Op::new("EMC", vec![]));
        }
        if index > 0 || dx != 0.0 {
            ops.push(Op::new("Td", vec![n(dx - cursor.x), n(if index > 0 { -self.lead } else { 0.0 })]));
        }
        cursor.x = dx;
        // Word spacing reaches only single-byte code 32 (ISO 32000-1 §9.3.3), so where a space is
        // shown with another code (a composite font, a generated Type 3 font, a subset without a
        // space) the extra space follows each space as a TJ adjustment instead.
        let stretch = if self.word_spacing_reaches_spaces { 0.0 } else { tw };
        let tw = tw - stretch;
        if tw != cursor.tw_set {
            ops.push(Op::new("Tw", vec![n(tw)]));
            cursor.tw_set = tw;
        }
        // Each wrapped line in typing order; drawn left to right (right-to-left runs reversed).
        let visual = crate::bidi::to_visual(shown);
        // Stretched without word spacing: shown up to and including each space, then moved on.
        let pieces: Vec<&str> = if stretch > 0.0 { visual.split_inclusive(' ').collect() } else { vec![visual.as_str()] };
        for piece in pieces {
            self.show_piece(piece, line, tw, cursor, ops)?;
            if stretch > 0.0 && piece.ends_with(' ') {
                ops.push(Op::new("TJ", vec![Object::Array(vec![n(-1000.0 * stretch / self.size)])]));
            }
        }
        let tw = tw + stretch;
        // Underlined to the line's last word, over its stretched gaps.
        let w = (visible + tw * interior as f64 * self.scale * self.k) / self.k;
        Ok((dx, dx + w, -(index as f64) * self.lead - self.size * 0.12))
    }

    fn show_piece(&self, piece: &str, line: &str, tw: f64, cursor: &mut LineCursor, ops: &mut Vec<Op>) -> Result<(), EditError> {
        let n = pdfcraft_content::num;
        if let Some(metrics) = self.source {
            let indic_piece = if metrics.visual_order { crate::indic::to_visual(piece) } else { piece.to_owned() };
            ops.extend(source_show(metrics, &indic_piece, self.size, self.space, self.char_spacing, tw)?);
            return Ok(());
        }
        for (font, bytes) in (self.encode)(piece).ok_or_else(|| EditError::Invalid(format!("\"{line}\" can't be shown")))? {
            if font != cursor.shown_font {
                ops.push(Op::new("Tf", vec![Object::name(&font), n(self.size)]));
                cursor.shown_font = font;
            }
            ops.push(Op::new("Tj", vec![Object::String(PdfString::literal(bytes))]));
        }
        Ok(())
    }

    /// Writes every wrapped line, returning their underlines.
    fn write_all(&self, wrapped: &[(String, bool, usize)], cursor: &mut LineCursor, ops: &mut Vec<Op>) -> Result<Vec<(f64, f64, f64)>, EditError> {
        let mut row = 0usize;
        wrapped
            .iter()
            .enumerate()
            .map(|(i, (line, soft, gap))| {
                if i > 0 && !soft {
                    row = row.saturating_add(1);
                }
                self.write_line(i, line, (row, *soft, *gap), i + 1 == wrapped.len(), cursor, ops)
            })
            .collect()
    }
}

/// The operators that write a paragraph in its own graphics state, so a new colour (or anything
/// else) stops at the paragraph.
struct BlockPlan<'a> {
    block: &'a TextBlock,
    lines: &'a [TextLine],
    style: &'a BlockStyle,
    emit: &'a Origin,
    after: &'a TextState,
    movement: Option<(f64, f64)>,
    state: TextState,
    tm: [f64; 6],
    size: f64,
    underline: bool,
}

fn block_ops(plan: &BlockPlan<'_>, writer: &LineWriter<'_>, wrapped: &[(String, bool, usize)], show_font: String) -> Result<Vec<Op>, EditError> {
    let n = pdfcraft_content::num;
    let mut ops = vec![Op::new("q", vec![])];
    ops.extend(reveal_ops(plan.block, plan.lines, plan.style, plan.emit.ctm)?);
    // Moved: a translation inside that state.
    if let Some((ax, ay)) = plan.movement {
        ops.push(Op::new("cm", vec![n(1.0), n(0.0), n(0.0), n(1.0), n(ax), n(ay)]));
    }
    // Painted as the paragraph was, including what its text object set before the first show.
    ops.extend(plan.state.paint_ops(plan.after));
    ops.push(Op::new("BT", vec![]));
    let mut state = plan.state.clone();
    state.word_spacing = 0.0;
    // The font the next run is shown in: the one the BT sets, until a run of another fallback subset.
    let mut cursor = LineCursor { x: 0.0, tw_set: 0.0, shown_font: show_font.clone() };
    state.font = Some((show_font, plan.size));
    if let Some([r, g, bl]) = plan.style.color {
        state.fill = vec![Op::new("rg", vec![n(r), n(g), n(bl)])];
    }
    ops.extend(state.ops());
    ops.push(Op::new("Tm", plan.tm.iter().map(|v| n(*v)).collect()));
    let underlines = writer.write_all(wrapped, &mut cursor, &mut ops)?;
    ops.push(Op::new("ET", vec![]));
    if plan.underline {
        ops.extend(underline_ops(&state.fill, &plan.tm, plan.size, &underlines));
    }
    ops.push(Op::new("Q", vec![]));
    Ok(ops)
}

/// Whether text shown earlier in the same text object stays first: the paragraph then goes where
/// its last row was, after any backgrounds between the rows, or between two halves of the text
/// object. The paragraph's own operators (and another drawing on the same baseline: a fake-bold
/// second copy, an invisible text layer) go, so the new text is all that may show.
fn shows_before_paragraph(ops: &[Op], from: usize, to: usize, dropped: &std::collections::HashSet<usize>) -> bool {
    ops.get(from..to)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .any(|(i, op)| matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"") && !dropped.contains(&(from + i)))
}

/// A paragraph's single style, with the text-space values read from where it starts.
struct ParaStyle {
    state: TextState,
    size: f64,
    old_size: f64,
    k: f64,
}

/// Spacing and scaling (text state), with the chosen values.
fn paragraph_style(o: &Origin, style: &BlockStyle) -> Result<(String, ParaStyle), EditError> {
    let (font_name, old_size) = o.state.font.clone().ok_or_else(|| EditError::Invalid("the paragraph has no font".into()))?;
    let k = o.k.max(1e-6);
    // The size in text space: points ÷ the text-to-user scale, with the sign of the source's `Tf`
    // (a negative size mirrors the glyphs, which a flipped CTM turns upright again).
    let size = style.size.map_or(old_size, |pt| (pt / k).max(0.1).copysign(old_size));
    let mut state = o.state.clone();
    if let Some(c) = style.char_spacing {
        state.char_spacing = c / k;
    }
    if let Some(sc) = style.scale {
        state.scale = (sc / 100.0).clamp(0.1, 10.0);
    }
    Ok((font_name, ParaStyle { state, size, old_size, k }))
}

/// The font the paragraph's existing runs use, for reusing its glyphs.
struct SourceFont {
    metrics: Option<Metrics>,
    reuse: bool,
    space: f64,
}

fn source_font(doc: &Document, fonts_res: &Dict, font_name: &str, first: &TextLine, flat: &str, style: &BlockStyle) -> SourceFont {
    let metrics = fonts_res.get(font_name.as_bytes()).and_then(|f| doc.resolve(f).as_dict().cloned()).map(|d| Metrics::from_dict(doc, &d));
    let visual_indic = metrics.as_ref().is_some_and(|m| m.visual_order) || first.visual_order;
    let indic_text = if visual_indic { crate::indic::to_visual(flat) } else { flat.to_owned() };
    let reuse = style.family.is_none() && style.bold.is_none() && metrics.as_ref().is_some_and(|m| source_encodes(m, &indic_text));
    let space = metrics.as_ref().map_or(0.33, |m| source_space(doc, fonts_res, m));
    SourceFont { metrics, reuse, space }
}

/// [`rewrite_block_with_limit`] with the page's [`text_lines`], read from `doc` as it is now.
pub(crate) fn rewrite_block_from(
    doc: &mut Document,
    page: usize,
    lines: Vec<TextLine>,
    block: usize,
    text: Option<&str>,
    style: &BlockStyle,
    max_glyphs: usize,
) -> Result<LineEdit, EditError> {
    let blocks = group_blocks(&lines);
    let b = blocks.get(block).cloned().ok_or_else(|| EditError::Invalid(format!("page {} has no paragraph {}", page + 1, block + 1)))?;
    // Existing PDF paragraphs do not yet persist selected-row alignment intent. Refuse
    // that operation before writing anything; whole-paragraph `align` remains supported.
    if style.aligns.iter().any(Option::is_some) {
        return Err(EditError::Invalid(
            "selected-row alignment of existing paragraphs is not supported until it can be preserved across editing sessions".into(),
        ));
    }
    // An alignment for a one-row paragraph is laid out across the column it sits in, as a
    // paragraph of that width moved to its left edge.
    let framed;
    let style = match alignment_frame(&lines, &blocks, &b, style, text.unwrap_or(&b.editor_text)) {
        Some([left, right]) => {
            framed = BlockStyle { width: Some(right - left), offset: Some([left - b.rect[0], 0.0]), ..style.clone() };
            &framed
        }
        None => style,
    };
    let members: Vec<&TextLine> = b.lines.iter().map(|i| &lines[*i]).collect();
    // One clipping run is enough: rewriting the paragraph would drop its mask with the rest.
    let mut dropped = coincident_ops(&lines, &members);
    dropped.extend(members.iter().flat_map(|l| l.ops.iter().copied()));
    refuse_clipping_text(&lines, &dropped)?;
    let first = members.first().copied().ok_or_else(|| EditError::Invalid("the paragraph has no source runs".into()))?;
    members.iter().try_for_each(|line| line.ensure_editable())?;
    let p = page_dict(doc, page)?;
    let mut res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut fonts_res = res.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let dispatch = choose_rewrite_path(doc, &fonts_res, &b, &members, first, text, style)?;
    if dispatch.use_styled_path && style.align == Some(crate::added::Align::Justify) {
        return Err(EditError::Invalid("justification of mixed-style existing paragraphs is not supported".into()));
    }
    if dispatch.use_styled_path {
        return rewrite_styled_block(doc, page, &b, &lines, text, style, max_glyphs);
    }
    let upright = dispatch.upright;
    let streams = content_streams(doc, &p.dict)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let text = paragraph_text(text.unwrap_or(&b.text));
    // The characters to show, for choosing fonts: the breaks are moves, not glyphs.
    let flat = text.replace('\n', " ");
    let o = &first.origin;
    let last = members.last().copied().unwrap_or(first);
    let emit = &last.origin;
    let (font_name, ParaStyle { state: o_state, size, old_size, k }) = paragraph_style(o, style)?;
    let source = source_font(doc, &fonts_res, &font_name, first, &flat, style);
    let reuse = source.reuse;
    // The chosen style also determines the real Japanese fallback outlines and advances.
    let (family, bold, italic) = style.family.unwrap_or((source_family(&b.base_font), b.bold, b.italic));
    let bold = style.bold.unwrap_or(bold);
    let [dx, dy] = style.offset.unwrap_or([0.0, 0.0]);
    if !(dx.is_finite() && dy.is_finite()) {
        return Err(EditError::Invalid("the paragraph can't be moved that far".into()));
    }
    // Right-to-left text (more Hebrew or Arabic letters than Latin) is right-aligned unless an
    // alignment is asked for, and a single right-to-left line keeps its right edge as it grows.
    let rtl = crate::bidi::rtl_base(&flat);
    let anchored_right = rtl && upright && style.width.is_none() && members.len() == 1;
    // The paragraph's width along its lines: in user space unless it is turned.
    let frame = if upright {
        b.rect
    } else {
        members.iter().map(|l| l.frame_rect()).reduce(|a, f| [a[0].min(f[0]), a[1].min(f[1]), a[2].max(f[2]), a[3].max(f[3])]).unwrap_or(b.rect)
    };
    // A centred title is laid out in the box it already sits in, so it keeps its centre as it
    // grows or wraps, rather than being grown to the right up to the page's margin (#2581).
    // Turned text is left alone: its centre lives along its own baseline, not in page space.
    let crop = p.crop(doc);
    let centred = upright && !rtl && style.width.is_none() && style.align.is_none() && is_centred(&b, &lines, (crop[0] + crop[2]) / 2.0, first.size);
    let geometry = ParaGeometry { rect: b.rect, frame, crop, upright, k, size, dx, single_line: members.len() == 1, anchored_right, centred };
    let width = geometry.width(style.width)?;
    let align = if centred { Some(crate::added::Align::Center) } else { style.align.or(rtl.then_some(crate::added::Align::Right)) };
    // Where a right-aligned line ends, from the paragraph's left edge (user space).
    let right_edge = if anchored_right { (b.rect[2] - b.rect[0]).max(0.0) } else { width };
    let movement = drawn_movement(emit.ctm, dx, dy)?;
    let tm = emit_text_matrix(o, emit)?;
    let type3 = if !reuse && needs_type3(&flat) { Some(type3_font(doc, &mut fonts_res, &flat, family, bold, max_glyphs)?) } else { None };
    let source_metrics = source.metrics.as_ref().filter(|_| reuse);
    let advance = Advance {
        source: source_metrics,
        type3: type3.as_ref(),
        family,
        bold,
        italic,
        size,
        space: source.space,
        char_spacing: o_state.char_spacing,
        word_spacing: o_state.word_spacing,
        scale: o_state.scale,
        k,
    };
    let width = if !upright && style.width.is_none() && members.len() == 1 {
        turned_line_width(first, crop, [dx, dy], width, &text, &advance)
    } else {
        width
    };
    let right_edge = if anchored_right { right_edge } else { width };
    let wrapped = wrap(&text, width + 0.5, |s| advance.of(s));
    let font = choose_font(
        &FontInputs {
            source: source_metrics,
            type3: type3.as_ref(),
            font_name: &font_name,
            flat: &flat,
            source_base_font: &b.base_font,
            family,
            bold,
            italic,
            family_requested: style.family.is_some(),
        },
        &mut fonts_res,
    )?;
    let lead = line_lead(&b, &lines, style, &LeadScale { k, size, old_size, leading: o_state.leading });
    let start = last.ops.first().copied().unwrap_or(emit.bt_op);
    let split = shows_before_paragraph(&ops, emit.bt_op, start, &dropped);
    // What the text after the paragraph expects, and what is in force where the paragraph is
    // drawn: the state at its BT (or where it was split).
    let after = if split { &emit.state } else { &emit.bt_state };
    // Word spacing reaches only single-byte code 32 (ISO 32000-1 §9.3.3); see `write_line`.
    let word_spacing_reaches_spaces = match source_metrics {
        Some(m) => m.encode(" ").is_some_and(|b| m.codes(&b).iter().all(|(c, len)| m.is_space(*c, *len))),
        None => type3.is_none(),
    };
    let writer = LineWriter {
        advance: &advance,
        alignment: Alignment { width, k, anchored_right, right_edge, rtl },
        encode: &font.encode,
        source: source_metrics,
        align,
        aligns: &style.aligns,
        word_spacing_reaches_spaces,
        width,
        scale: o_state.scale,
        k,
        size,
        space: source.space,
        char_spacing: o_state.char_spacing,
        lead,
    };
    let plan =
        BlockPlan { block: &b, lines: &lines, style, emit, after, movement, state: o_state, tm, size, underline: style.underline == Some(true) };
    let mut block_ops = block_ops(&plan, &writer, &wrapped, font.show_font.clone())?;
    let n = pdfcraft_content::num;
    block_ops.extend(after.ops().into_iter().filter(|op| !op.is("Tf") || after.font.is_some()));
    if split {
        block_ops.insert(0, Op::new("ET", vec![]));
        block_ops.push(Op::new("BT", vec![]));
        block_ops.push(Op::new("Tm", emit.tlm.iter().map(|v| n(*v)).collect()));
    }
    let at = if split { start } else { emit.bt_op };
    let new_data = splice(&pieces, &ops, |i| {
        let insert = if i == at { std::mem::take(&mut block_ops) } else { Vec::new() };
        (insert, !dropped.contains(&i))
    });
    let contents = rewritten_contents(doc, &streams, new_data);
    let new_font = !reuse;
    if new_font {
        res.set(b"Font".to_vec(), Object::Dict(fonts_res));
    }
    let page_ref = p.obj;
    doc.update_dict(page_ref, |d| {
        d.set(b"Contents".to_vec(), contents);
        if new_font {
            d.set(b"Resources".to_vec(), Object::Dict(res));
        }
    })?;
    Ok(LineEdit { substituted: font.substituted })
}

/// The cover (when [`BlockStyle::cover`] asks for one) and visible rendering that start a
/// rewritten invisible paragraph, drawn in the space whose CTM is `ctm`. Nothing for visible text.
/// The paragraph's `q`/`Q` restores the rendering mode for the invisible text after it.
pub(crate) fn reveal_ops(block: &TextBlock, lines: &[TextLine], style: &BlockStyle, ctm: [f64; 6]) -> Result<Vec<Op>, EditError> {
    if !block.invisible {
        return Ok(Vec::new());
    }
    let n = pdfcraft_content::num;
    let mut ops = Vec::new();
    if let Some(colour) = style.cover {
        if colour.iter().any(|c| !c.is_finite() || !(0.0..=1.0).contains(c)) {
            return Err(EditError::Invalid("the cover colour must be RGB values from 0 to 1".into()));
        }
        let back = Matrix(ctm).invert().ok_or_else(|| EditError::Invalid("the paragraph is drawn in a space it can't be covered in".into()))?;
        ops.push(Op::new("q", vec![]));
        ops.push(Op::new("rg", colour.map(n).to_vec()));
        for rect in block.lines.iter().filter_map(|i| lines.get(*i)).map(|l| l.rect) {
            // Scanned ink reaches a little past the recognised box.
            let pad = ((rect[3] - rect[1]).abs() * 0.1).clamp(0.5, 2.0);
            let corners =
                [[rect[0] - pad, rect[1] - pad], [rect[2] + pad, rect[1] - pad], [rect[2] + pad, rect[3] + pad], [rect[0] - pad, rect[3] + pad]];
            for (i, [x, y]) in corners.into_iter().enumerate() {
                let (x, y) = back.apply(x, y);
                if !(x.is_finite() && y.is_finite()) {
                    return Err(EditError::Invalid("the paragraph's area can't be covered".into()));
                }
                ops.push(Op::new(if i == 0 { "m" } else { "l" }, vec![n(x), n(y)]));
            }
            ops.push(Op::new("h", vec![]));
        }
        ops.push(Op::new("f", vec![]));
        ops.push(Op::new("Q", vec![]));
    }
    ops.push(Op::new("Tr", vec![Object::Int(0)]));
    Ok(ops)
}

/// Existing paragraph indexes, excluding streams owned by an added-content item.
pub(crate) fn selectable_blocks(
    doc: &Document,
    page: usize,
    excluded: &std::collections::BTreeSet<pdfcraft_cos::ObjRef>,
) -> Result<Vec<(usize, TextBlock)>, EditError> {
    let p = page_dict(doc, page)?;
    let streams = content_streams(doc, &p.dict)?;
    let lines = text_lines(doc, page)?;
    Ok(group_blocks(&lines)
        .into_iter()
        .enumerate()
        .filter(|(_, b)| {
            !b.lines
                .iter()
                .filter_map(|i| lines.get(*i))
                .any(|l| streams.get(l.stream).and_then(|(o, _)| o.as_ref()).is_some_and(|r| excluded.contains(&r)))
        })
        .collect())
}

/// The vertical advance per unit of font size of a font that writes vertically: `/DW2`'s w1
/// (default −1000, §9.7.4.3), or `None` for horizontal text. Per-glyph `/W2` entries are not
/// read. An encoding whose writing mode can't be read is an error, as in [`vertical_encoding`];
/// listing text treats it as horizontal and records the error (see [`Ts::unreadable`]).
pub(super) fn vertical_advance(doc: &Document, font: &Dict) -> Result<Option<f64>, EditError> {
    if !vertical_encoding(doc, font)? {
        return Ok(None);
    }
    let descendant = font.get(b"DescendantFonts").and_then(|d| doc.resolve(d).as_array().and_then(|a| a.first().cloned()));
    let w1 = descendant
        .and_then(|d| doc.resolve(&d).as_dict().and_then(|d| d.get(b"DW2").cloned()))
        .and_then(|a| doc.resolve(&a).as_array().and_then(|a| a.get(1).cloned()))
        .and_then(|w| doc.resolve(&w).as_f64())
        .filter(|w| w.is_finite())
        .unwrap_or(-1000.0);
    Ok(Some(w1 / 1000.0))
}

/// Detect vertical CMaps before moving horizontal text. A broken/cyclic embedded CMap is
/// refused rather than guessing which direction TJ advances its text matrix.
fn vertical_encoding(doc: &Document, font: &Dict) -> Result<bool, EditError> {
    if font.name(b"Subtype") != Some(b"Type0") {
        return Ok(false);
    }
    let mut next = font.get(b"Encoding").cloned();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..8 {
        let Some(o) = next.take() else { return Ok(false) };
        if let Some(r) = o.as_ref()
            && !seen.insert(r)
        {
            return Err(EditError::Invalid("the text encoding is cyclic".into()));
        }
        match &*doc.resolve(&o) {
            Object::Name(n) => return Ok(n.ends_with(b"-V")),
            Object::Stream(stream) => {
                if let Some(mode) = stream.dict.get(b"WMode") {
                    match mode.as_int() {
                        Some(1) => return Ok(true),
                        Some(0) => {}
                        _ => return Err(EditError::Invalid("the text writing mode is invalid".into())),
                    }
                }
                let bytes = stream
                    .decoded_within(8 * 1024 * 1024)
                    .map_err(|_| EditError::Invalid("the text encoding cannot be decoded within its size limit".into()))?;
                let ops = parse(&bytes).ops;
                if let Some(mode) = ops.iter().rev().find(|op| op.is("def") && op.name(0) == Some(b"WMode")) {
                    return match mode.num(1) {
                        Some(0.0) => Ok(false),
                        Some(1.0) => Ok(true),
                        _ => Err(EditError::Invalid("the text writing mode is invalid".into())),
                    };
                }
                next = stream
                    .dict
                    .get(b"UseCMap")
                    .cloned()
                    .or_else(|| ops.iter().rev().find(|op| op.is("usecmap")).and_then(|op| op.operands.first().cloned()));
            }
            _ => return Err(EditError::Invalid("the text encoding is invalid".into())),
        }
    }
    Err(EditError::Invalid("the text encoding is nested too deeply".into()))
}

/// Translate paragraphs together without re-encoding glyphs, changing fonts or reflowing.
/// Numeric TJ offsets and text rise temporarily move each selected show operation, then
/// restore both advances and rise. The PDF reader advances the original glyph bytes itself:
/// no approximate font metrics are used to position following, unselected text (§9.4.4).
pub(crate) fn translate_blocks(doc: &mut Document, page: usize, indexes: &[usize], offset: [f64; 2]) -> Result<(), EditError> {
    if indexes.is_empty() {
        return Ok(());
    }
    let p = page_dict(doc, page)?;
    let streams = content_streams(doc, &p.dict)?;
    let lines = text_lines(doc, page)?;
    let blocks = group_blocks(&lines);
    let mut selected = std::collections::BTreeSet::new();
    for index in indexes {
        let b = blocks.get(*index).ok_or_else(|| EditError::Invalid("a selected paragraph no longer exists".into()))?;
        for line in &b.lines {
            let l = lines.get(*line).ok_or_else(|| EditError::Invalid("a selected text line no longer exists".into()))?;
            l.ensure_editable()?;
            selected.extend(l.ops.iter().copied());
        }
    }
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let fonts = res.get(b"Font").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let shown = interpret(doc, &ops, &res, &mut HashMap::new(), &mut Carry::new(), None, None)?;
    let mut patches = HashMap::new();
    // Keep full precision: a tiny rounded text-space change can be magnified by a CTM,
    // and restoring rise/spacing must not alter following unselected text.
    let n = Object::Real;
    for s in shown.into_iter().filter(|s| selected.contains(&s.op)) {
        if !(0..=3).contains(&s.state.render_mode) {
            return Err(EditError::Invalid("text used as a clipping path cannot be moved in a group".into()));
        }
        let font = fonts
            .get(&s.font)
            .map(|f| doc.resolve(f))
            .and_then(|f| f.as_dict().cloned())
            .ok_or_else(|| EditError::Invalid("a selected paragraph has no font".into()))?;
        if vertical_encoding(doc, &font)? {
            return Err(EditError::Invalid("vertical text cannot be moved in a group".into()));
        }
        let inverse = s.tm.then(&s.state.ctm).invert().ok_or_else(|| EditError::Invalid("a selected paragraph has no area".into()))?;
        let [a, b, c, d, _, _] = inverse.0;
        let (dx, dy) = (a * offset[0] + c * offset[1], b * offset[0] + d * offset[1]);
        let denominator = s.state.size * s.state.scale;
        let shift = if dx == 0.0 {
            0.0
        } else if denominator.abs() > 1e-12 {
            -1000.0 * dx / denominator
        } else {
            return Err(EditError::Invalid("a selected paragraph has no horizontal advance".into()));
        };
        let rise = s.state.rise + dy;
        if ![shift, rise, dx, dy].iter().all(|v| v.is_finite() && v.abs() <= 1e12) {
            return Err(EditError::Invalid("the paragraph move is too large".into()));
        }
        let original = ops.get(s.op).ok_or_else(|| EditError::Invalid("a selected text operation is missing".into()))?;
        let mut replacement = Vec::new();
        let show = if original.is("'") || original.is("\"") {
            if original.is("\"") {
                let spacing = original
                    .operands
                    .get(..2)
                    .filter(|a| a.iter().all(|o| o.as_f64().is_some()))
                    .ok_or_else(|| EditError::Invalid("a text operation has invalid spacing".into()))?;
                for (operator, operand) in ["Tw", "Tc"].into_iter().zip(spacing) {
                    replacement.push(Op::new(operator, vec![operand.clone()]));
                }
            }
            replacement.push(Op::new("T*", vec![]));
            Op::new("Tj", vec![original.operands.last().cloned().ok_or_else(|| EditError::Invalid("a text operation has no string".into()))?])
        } else {
            original.clone()
        };
        if dy != 0.0 {
            replacement.push(Op::new("Ts", vec![n(rise)]));
        }
        if shift != 0.0 {
            replacement.push(Op::new("TJ", vec![Object::Array(vec![n(shift)])]));
        }
        replacement.push(show);
        if shift != 0.0 {
            replacement.push(Op::new("TJ", vec![Object::Array(vec![n(-shift)])]));
        }
        if dy != 0.0 {
            replacement.push(Op::new("Ts", vec![n(s.state.rise)]));
        }
        patches.insert(s.op, replacement);
    }
    if patches.len() != selected.len() {
        return Err(EditError::Invalid("a selected text operation cannot be moved".into()));
    }
    let data = splice(&pieces, &ops, |i| patches.remove(&i).map_or((Vec::new(), true), |v| (v, false)));
    let contents = rewritten_contents(doc, &streams, data);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}
