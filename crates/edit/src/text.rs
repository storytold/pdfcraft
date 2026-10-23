//! Edit a PDF ▸ Edit text: the text lines already on a page, and replacing a line's text in place.
//!
//! A *line* is a run of compatible text-showing operators on the same baseline, read
//! from one content stream. Replacing it rewrites only that stream (as a new object, since content
//! streams may be shared): the first operator of the line shows the new text, the others are
//! dropped, and every positioning operator stays, so the lines after it keep their places
//! (`Td`, `T*` and friends move from the line matrix, which showing text doesn't change).
//!
//! The new text uses the line's own font when every character has a code and a glyph in it;
//! otherwise it is set in Helvetica (WinAnsi), and the result says the font was substituted.
//! Reflowing a paragraph to a new width is not done here (see `edit.text-reflow`).

use std::collections::HashMap;
use std::rc::Rc;

use pdfcraft_content::{Matrix, Op, Pieces, parse, serialize_ops};
use pdfcraft_cos::{DecodeBudget, Dict, Document, Object, PdfString, Stream};
use pdfcraft_fonts::pdf::Metrics;
use pdfcraft_fonts::{CraftFont, GlyphError, japanese_glyph_from};

use crate::EditError;

#[path = "text_regions.rs"]
mod regions;
#[path = "text_session.rs"]
mod session;
use regions::{group_blocks, is_centred, rewrite_styled_block};
pub use session::TextSession;

/// One line of existing text.
#[derive(Clone, Debug, PartialEq)]
pub struct TextLine {
    /// The line's text as shown.
    pub text: String,
    /// Its box in user space.
    pub rect: [f64; 4],
    /// The boxes of the words it draws, in user space: from each word's first glyph to the end of
    /// its last, without the spaces and `TJ` adjustments around it. `rect` also spans gaps that
    /// draw nothing, such as the adjustment an applied redaction leaves where text was removed.
    pub ink: Vec<[f64; 4]>,
    /// The font's resource name and `/BaseFont` (or Type 3 descriptor's `/FontName`), and its size in text space.
    pub font: String,
    pub base_font: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// The current fill colour as RGB.
    pub color: [f64; 3],
    /// Whether new text in this font can only be shown by substituting another font (no
    /// Unicode mapping for the line, so nothing could be reused).
    pub decodable: bool,
    /// Whether the font requires visual-order Indic text.
    pub visual_order: bool,
    /// The Japanese or Hebrew fallback face the font subsets, when the line is in one (see [`same_font`]).
    face: Option<String>,
    /// Its box along and across its baseline, `[along0, across0, along1, across1]` (see
    /// [`TextLine::frame_rect`]). Upright text has `rect`; turned text has its own box, which is
    /// narrower than `rect` turned into the same frame.
    frame: [f64; 4],
    /// The content stream piece the line starts in, and its operators (indices into the page's
    /// streams parsed joined).
    stream: usize,
    ops: Vec<usize>,
    /// Where the line starts: text matrix (text space), the state there, and its `BT`.
    origin: Origin,
    /// Explicit marked-content scope (when present); distinct scopes must not be guessed into one paragraph.
    scope: Option<usize>,
    /// The layers and graphics state parameters it is drawn in; see [`Paint`].
    paint: Paint,
    /// Right-to-left `/ActualText` already supplies typing order; glyph text still needs bidi
    /// recovery. Left-to-right ActualText (e.g. a Latin ligature) joins its line as usual.
    logical_order: bool,
    /// Some of the line's text is `/ActualText`, which is already in logical order: visual-order
    /// Indic recovery would scramble it.
    actual_text: bool,
    /// Whether any of the line's runs draws in a clipping mode (`4 Tr` to `7 Tr`). The join below
    /// does not require one rendering mode, so a line that starts as ink can continue as a mask;
    /// [`refuse_clipping_text`] reads this rather than the origin's mode alone.
    clips: bool,
    /// Why the line can't be edited: its font's writing mode (its CMap's `WMode`) couldn't be read,
    /// so which way its glyphs advance is unknown. The line is listed as horizontal text.
    unreadable_mode: Option<String>,
}

impl TextLine {
    /// Refuses editing a line whose font's writing mode couldn't be read (§9.7.4.3).
    fn ensure_editable(&self) -> Result<(), EditError> {
        self.unreadable_mode.as_ref().map_or(Ok(()), |why| Err(EditError::Invalid(why.clone())))
    }

    /// Origin of the source baseline in PDF user space, independent of font ascent/descent.
    pub fn baseline(&self) -> [f64; 2] {
        [self.origin.x, self.origin.baseline]
    }

    /// Leading used when a single source run wraps into additional rows.
    pub fn wrap_leading(&self) -> f64 {
        if self.origin.state.leading > 0.0 { self.origin.state.leading * self.origin.k } else { self.size * 1.2 }
    }
}

/// An embedded sfnt source face, read only for the inline editor. Bytes stay in memory and
/// are never installed, copied into assets, or used as a font for another document.
#[derive(Clone, Debug)]
pub struct TextEditorFont {
    pub resource: String,
    pub face: String,
    pub bytes: Vec<u8>,
}

pub fn text_editor_fonts(doc: &Document, page: usize) -> Result<Vec<TextEditorFont>, EditError> {
    let p = page_dict(doc, page)?;
    let resources = p.dict.get(b"Resources").and_then(|o| doc.dict(o)).unwrap_or_default();
    let fonts = resources.get(b"Font").and_then(|o| doc.dict(o)).unwrap_or_default();
    let mut budget = 32_usize << 20;
    // Every program decoded counts, usable or not, so aliases of one program can't repeat the work.
    let mut decode = DecodeBudget::new(2 * budget, 0);
    let mut result = Vec::new();
    for (name, font) in fonts.iter().take(128) {
        let Some(font) = doc.dict(font) else { continue };
        let Some(bytes) = pdfcraft_fonts::pdf::embedded_sfnt_within(doc, &font, &mut decode) else { continue };
        if bytes.len() > budget {
            continue;
        }
        budget -= bytes.len();
        result.push(TextEditorFont {
            resource: String::from_utf8_lossy(name).into_owned(),
            face: Metrics::from_dict(doc, &font).base_font.rsplit('+').next().unwrap_or_default().to_owned(),
            bytes,
        });
    }
    // The approved full face follows all source subsets in the editor's font stack. It can
    // display newly inserted glyphs before Apply without changing the source text's face.
    let source_faces: Vec<_> = result.iter().map(|p| p.face.clone()).collect();
    for face in source_faces {
        for full in pdfcraft_fonts::embedding::matching_faces(&face) {
            if full.bytes.len() > budget || result.iter().any(|p| p.resource == format!("PCApproved{}", full.name())) {
                continue;
            }
            budget -= full.bytes.len();
            result.push(TextEditorFont { resource: format!("PCApproved{}", full.name()), face: face.clone(), bytes: full.bytes.to_vec() });
        }
    }
    Ok(result)
}

/// Load an explicit full face into this page's resources. Nothing on the page is replaced;
/// the new resource can complete matching source subsets or be chosen in a text style.
pub fn register_text_font(doc: &mut Document, page: usize, bytes: &[u8]) -> Result<String, EditError> {
    pdfcraft_fonts::embedding::validate_document_input(bytes).map_err(EditError::Invalid)?;
    let (_, postscript) = pdfcraft_fonts::embedding::names(bytes).map_err(EditError::Invalid)?;
    let p = page_dict(doc, page)?;
    let mut resources = p.dict.get(b"Resources").and_then(|o| doc.dict(o)).unwrap_or_default();
    let mut fonts = resources.get(b"Font").and_then(|o| doc.dict(o)).unwrap_or_default();
    let name = pdfcraft_fonts::embedding::embed(doc, &mut fonts, bytes, &postscript).map_err(EditError::Invalid)?;
    resources.set(b"Font".to_vec(), Object::Dict(fonts));
    doc.update_dict(p.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(resources)))?;
    Ok(name)
}

/// Read-only draft bounds, measured with the same PDF rewrite used by Apply/Save.
#[derive(Clone, Debug, PartialEq)]
pub struct TextEditLayout {
    pub source_rect: [f64; 4],
    pub draft_rect: Option<[f64; 4]>,
    pub overflow: bool,
}

pub fn text_edit_layout(doc: &Document, page: usize, block: usize, text: &str, style: &BlockStyle) -> Result<TextEditLayout, EditError> {
    TextSession::open(doc, page, block)?.layout(text, style)
}

/// A draft of paragraph `source` (number `block` among the page's `lines`) laid out by the rewrite
/// Apply runs, on a copy of `doc`.
fn layout_draft(
    doc: &Document,
    page: usize,
    lines: &[TextLine],
    source: &TextBlock,
    block: usize,
    text: &str,
    style: &BlockStyle,
) -> Result<TextEditLayout, EditError> {
    if text == source.editor_text && *style == BlockStyle::default() {
        return Ok(TextEditLayout { source_rect: source.rect, draft_rect: Some(source.rect), overflow: false });
    }
    // Subtract the untouched runs by value, rather than by operator offsets: a rewrite may
    // split BT/ET or add font resources. A multiset also handles coincident repeated text.
    let key =
        |line: &TextLine| (line.text.clone(), line.font.clone(), line.rect.map(f64::to_bits), line.size.to_bits(), line.color.map(f64::to_bits));
    let mut untouched = std::collections::BTreeMap::new();
    for (index, line) in lines.iter().enumerate() {
        if !source.lines.contains(&index) {
            *untouched.entry(key(line)).or_insert(0_usize) += 1;
        }
    }
    let mut draft = doc.clone();
    rewrite_block_from(&mut draft, page, lines.to_vec(), block, Some(text), style, MAX_TYPE3_GLYPHS)?;
    let mut rect: Option<[f64; 4]> = None;
    for line in text_lines(&draft, page)? {
        if let Some(count) = untouched.get_mut(&key(&line))
            && *count > 0
        {
            *count -= 1;
            continue;
        }
        rect = Some(rect.map_or(line.rect, |r| [r[0].min(line.rect[0]), r[1].min(line.rect[1]), r[2].max(line.rect[2]), r[3].max(line.rect[3])]));
    }
    let overflow = rect
        .is_some_and(|r| r[0] < source.rect[0] - 0.5 || r[1] < source.rect[1] - 0.5 || r[2] > source.rect[2] + 0.5 || r[3] > source.rect[3] + 0.5);
    Ok(TextEditLayout { source_rect: source.rect, draft_rect: rect, overflow })
}

/// Text and graphics state captured for rewriting.
#[derive(Clone, Debug, PartialEq)]
struct TextState {
    font: Option<(String, f64)>,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
    rise: f64,
    fill: Vec<Op>,
    /// The text rendering mode (`Tr`); 3 is invisible text, as in an OCR layer over a scan.
    render_mode: i64,
}

impl TextState {
    fn of(ts: &Ts) -> Self {
        TextState {
            font: ts.font.as_ref().map(|(n, _)| (String::from_utf8_lossy(n).into_owned(), ts.size)),
            char_spacing: ts.char_spacing,
            word_spacing: ts.word_spacing,
            scale: ts.scale,
            leading: ts.leading,
            rise: ts.rise,
            fill: ts.fill.clone(),
            render_mode: ts.render_mode,
        }
    }

    /// Operators that set this state (inside a text object).
    fn ops(&self) -> Vec<Op> {
        let n = pdfcraft_content::num;
        let mut v = Vec::new();
        if let Some((f, size)) = &self.font {
            v.push(Op::new("Tf", vec![Object::name(f), n(*size)]));
        }
        v.push(Op::new("Tc", vec![n(self.char_spacing)]));
        v.push(Op::new("Tw", vec![n(self.word_spacing)]));
        v.push(Op::new("Tz", vec![n(self.scale * 100.0)]));
        v.push(Op::new("TL", vec![n(self.leading)]));
        v.push(Op::new("Ts", vec![n(self.rise)]));
        v.extend(self.fill.iter().cloned());
        v
    }
}

/// What a run is drawn in beyond its text state: the optional content groups of the `/OC`
/// marked-content sequences around it, and the effective graphics state parameters (`gs`) in
/// force. A rewrite writes a paragraph in its first run's place, so runs that differ here are
/// never joined: one in a hidden layer or at another opacity would lose it.
#[derive(Clone, Debug, Default, PartialEq)]
struct Paint {
    layers: Vec<Vec<u8>>,
    gs: Gs,
}

/// The transparency parameters of the graphics state: what the `gs` operators have set, resolved,
/// so a repeated `gs` or two names with the same settings leave it as it was.
#[derive(Clone, Debug, PartialEq)]
struct Gs {
    /// `ca`, the fill opacity.
    fill_alpha: f64,
    /// `CA`, the stroke opacity.
    stroke_alpha: f64,
    /// `BM`, the blend mode (`Compatible` is `Normal`).
    blend: Vec<u8>,
    /// `SMask`, when it is not `None`: the reference of an indirect soft mask, else the dictionary.
    soft_mask: Option<Object>,
}

impl Default for Gs {
    fn default() -> Self {
        Gs { fill_alpha: 1.0, stroke_alpha: 1.0, blend: b"Normal".to_vec(), soft_mask: None }
    }
}

impl Gs {
    /// Applies the parameters a graphics state parameter dictionary sets; the others stay.
    fn apply(&mut self, doc: &Document, params: &Dict) {
        let num = |key: &[u8]| params.get(key).and_then(|o| doc.resolve(o).as_f64()).filter(|v| v.is_finite());
        if let Some(v) = num(b"ca") {
            self.fill_alpha = v;
        }
        if let Some(v) = num(b"CA") {
            self.stroke_alpha = v;
        }
        if let Some(bm) = params.get(b"BM").map(|o| doc.resolve(o)) {
            let first = bm
                .as_name()
                .map(<[u8]>::to_vec)
                .or_else(|| bm.as_array().and_then(|a| a.iter().find_map(|o| doc.resolve(o).as_name().map(<[u8]>::to_vec))));
            if let Some(name) = first {
                self.blend = if name == b"Compatible" { b"Normal".to_vec() } else { name };
            }
        }
        if let Some(mask) = params.get(b"SMask") {
            self.soft_mask = match (mask.as_ref(), &*doc.resolve(mask)) {
                (_, Object::Name(_)) => None,
                (Some(r), _) => Some(Object::Ref(r)),
                (None, Object::Dict(d)) => Some(Object::Dict(d.clone())),
                (None, _) => None,
            };
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Origin {
    tm: [f64; 6],
    /// The text line matrix there (where the next line's Td/T* starts from).
    tlm: [f64; 6],
    /// Text-space units → user space (the scale of the text rendering matrix's y axis).
    k: f64,
    /// The CTM there (current space → page space), for moving the paragraph in page space.
    ctm: [f64; 6],
    baseline: f64,
    x: f64,
    state: TextState,
    bt_op: usize,
    bt_state: TextState,
}

/// An editable text region: adjacent runs on visual rows, followed by aligned rows.
/// A region may contain several source fonts, sizes, colours and weights.
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    /// The lines' text joined with spaces (lines that ended in a hyphen are joined directly).
    pub text: String,
    pub rect: [f64; 4],
    pub base_font: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    /// The text fill colour as RGB, used by the visual editor and preserved when no colour
    /// override is requested.
    pub color: [f64; 3],
    /// Counterclockwise baseline angle in page user space, in degrees.
    pub rotation: f64,
    /// Indexes into [`text_lines`].
    pub lines: Vec<usize>,
    /// Visual rows; each contains indexes into [`text_lines`], including mixed-style runs.
    pub rows: Vec<Vec<usize>>,
    /// Text for the inline editor, with a newline only between visual rows.
    pub editor_text: String,
    /// Byte ranges in `editor_text` and their source line indexes.
    pub spans: Vec<(std::ops::Range<usize>, usize)>,
    /// Every line is invisible text (rendering mode 3): an OCR layer over a scanned image, whose
    /// pixels show the words. A rewrite draws the new text visibly (see [`BlockStyle::cover`]).
    pub invisible: bool,
}

impl TextBlock {
    /// Preserve styles outside the changed ranges; inserted characters inherit the surrounding
    /// source run. The texts are aligned word by word, so words left untouched between two edits
    /// keep their own runs (#2198); past [`MAX_CHANGED_WORDS`] the draft counts as one change.
    /// Byte ranges are always UTF-8 boundaries. Linear time, including large edits.
    pub fn edited_spans(&self, draft: &str) -> Vec<(std::ops::Range<usize>, usize)> {
        if self.editor_text.len() > 4_000_000 || draft.len() > 4_000_000 {
            return vec![(0..draft.len(), self.lines.first().copied().unwrap_or(0))];
        }
        let old: Vec<_> = self.editor_text.chars().collect();
        let new: Vec<_> = draft.char_indices().collect();
        let chars: Vec<_> = new.iter().map(|(_, c)| *c).collect();
        let mut original = Vec::with_capacity(old.len());
        let mut span = 0;
        for (byte, _) in self.editor_text.char_indices() {
            while self.spans.get(span).is_some_and(|(r, _)| byte >= r.end) {
                span += 1;
            }
            original.push(self.spans.get(span).or_else(|| self.spans.last()).map_or(0, |(_, i)| *i));
        }
        let mut sources = Vec::with_capacity(new.len());
        let slices = |text: &[char]| words(text).into_iter().map(|w| text.get(w).unwrap_or_default().to_vec()).collect::<Vec<_>>();
        let (old_words, new_words) = (slices(&old), slices(&chars));
        match edit_script(&old_words, &new_words, MAX_CHANGED_WORDS) {
            Some(script) => {
                // `(o, n)`: where the current change starts in the old and new text; `(x, y)`: where it ends.
                let ((mut o, mut n), (mut x, mut y)) = ((0, 0), (0, 0));
                let (mut a, mut b) = (old_words.iter(), new_words.iter());
                for step in script {
                    match step {
                        Step::Keep => {
                            replaced(&original, o, old.get(o..x).unwrap_or_default(), chars.get(n..y).unwrap_or_default(), &mut sources);
                            let len = a.next().map_or(0, Vec::len);
                            b.next();
                            sources.extend(original.get(x..x.saturating_add(len)).unwrap_or_default());
                            (x, y) = (x.saturating_add(len), y.saturating_add(len));
                            (o, n) = (x, y);
                        }
                        Step::Delete => x = x.saturating_add(a.next().map_or(0, Vec::len)),
                        Step::Insert => y = y.saturating_add(b.next().map_or(0, Vec::len)),
                    }
                }
                replaced(&original, o, old.get(o..x).unwrap_or_default(), chars.get(n..y).unwrap_or_default(), &mut sources);
            }
            None => replaced(&original, 0, &old, &chars, &mut sources),
        }
        let fallback = original.first().copied().unwrap_or(0);
        let mut result: Vec<(std::ops::Range<usize>, usize)> = Vec::new();
        for (i, (byte, ch)) in new.iter().enumerate() {
            let source = sources.get(i).copied().unwrap_or(fallback);
            let end = byte.saturating_add(ch.len_utf8());
            if let Some((range, _)) = result.last_mut().filter(|(_, prev)| *prev == source) {
                range.end = end;
            } else {
                result.push((*byte..end, source));
            }
        }
        result
    }
}

/// Changed words [`TextBlock::edited_spans`] aligns before it treats the draft as one change.
/// Myers' diff costs O((N + M) · D), so bounding D keeps it linear.
const MAX_CHANGED_WORDS: usize = 64;

/// `text` split into words, each with the whitespace after it, as char ranges.
fn words(text: &[char]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, pair) in text.windows(2).enumerate() {
        if let [a, b] = pair
            && a.is_whitespace()
            && !b.is_whitespace()
        {
            out.push(start..i + 1);
            start = i + 1;
        }
    }
    if start < text.len() {
        out.push(start..text.len());
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Keep,
    Delete,
    Insert,
}

/// Myers' shortest edit script from `a` to `b`, or `None` when it takes more than `max` steps.
fn edit_script<T: PartialEq>(a: &[T], b: &[T], max: usize) -> Option<Vec<Step>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = max.min(a.len().saturating_add(b.len())) as isize;
    // Furthest x reached on each diagonal k = x - y, stored at k + max + 1.
    let at = |v: &[isize], k: isize| usize::try_from(k + max + 1).ok().and_then(|i| v.get(i).copied()).unwrap_or(0);
    let mut v = vec![0isize; (2 * max + 3) as usize];
    let mut trace = Vec::new();
    for d in 0..=max {
        trace.push(v.clone());
        for k in (-d..=d).step_by(2) {
            let down = k == -d || (k != d && at(&v, k - 1) < at(&v, k + 1));
            let mut x = if down { at(&v, k + 1) } else { at(&v, k - 1) + 1 };
            let mut y = x - k;
            while x < n && y < m && a.get(x as usize) == b.get(y as usize) {
                x += 1;
                y += 1;
            }
            if let Some(slot) = usize::try_from(k + max + 1).ok().and_then(|i| v.get_mut(i)) {
                *slot = x;
            }
            if x >= n && y >= m {
                return Some(backtrack(&trace, n, m, max));
            }
        }
    }
    None
}

/// Walk Myers' saved frontiers back from the end to the edit script.
fn backtrack(trace: &[Vec<isize>], n: isize, m: isize, max: isize) -> Vec<Step> {
    let at = |v: &[isize], k: isize| usize::try_from(k + max + 1).ok().and_then(|i| v.get(i).copied()).unwrap_or(0);
    let mut steps = Vec::new();
    let (mut x, mut y) = (n, m);
    for (d, v) in trace.iter().enumerate().rev() {
        let (d, k) = (d as isize, x - y);
        let prev_k = if k == -d || (k != d && at(v, k - 1) < at(v, k + 1)) { k + 1 } else { k - 1 };
        let prev_x = at(v, prev_k);
        let prev_y = prev_x - prev_k;
        while x > prev_x && y > prev_y {
            steps.push(Step::Keep);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            steps.push(if x == prev_x { Step::Insert } else { Step::Delete });
        }
        (x, y) = (prev_x, prev_y);
    }
    steps.reverse();
    steps
}

/// `old`, at char `at` of the source, became `new`: characters kept at either end keep their
/// runs, and the rest inherit the run just before the change (or the first one it replaces).
fn replaced(original: &[usize], at: usize, old: &[char], new: &[char], out: &mut Vec<usize>) {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old.iter().rev().zip(new.iter().rev()).take(old.len().min(new.len()).saturating_sub(prefix)).take_while(|(a, b)| a == b).count();
    let start = at.saturating_add(prefix);
    let inherited = start.checked_sub(1).and_then(|i| original.get(i)).or_else(|| original.get(start)).copied().unwrap_or(0);
    for i in 0..new.len() {
        let source = if i < prefix {
            original.get(at.saturating_add(i)).copied()
        } else if i >= new.len().saturating_sub(suffix) {
            original.get(at.saturating_add(old.len()).saturating_sub(new.len() - i)).copied()
        } else {
            Some(inherited)
        };
        out.push(source.unwrap_or(inherited));
    }
}

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

    fn rotation(&self) -> f64 {
        let m = Matrix(self.origin.tm).then(&Matrix(self.origin.ctm)).0;
        m[1].atan2(m[0]).to_degrees()
    }

    /// The unit direction its baseline runs in, in user space.
    fn direction(&self) -> (f64, f64) {
        direction(&Matrix(self.origin.tm).then(&Matrix(self.origin.ctm)))
    }

    /// Whether it reads left to right along the page's x axis (most text; a page drawn
    /// turned a quarter, as landscape pages often are, isn't).
    pub(crate) fn upright(&self) -> bool {
        let (x, y) = self.direction();
        (x - 1.0).abs() < 1e-3 && y.abs() < 1e-3
    }

    /// Where it starts as `(along, across)` its baseline: user space turned to the text's direction.
    fn frame_origin(&self) -> (f64, f64) {
        to_frame(self.direction(), (self.origin.x, self.origin.baseline))
    }

    /// Its box in the same frame, `[along0, across0, along1, across1]` (for upright text, `rect`).
    fn frame_rect(&self) -> [f64; 4] {
        self.frame
    }
}

/// The unit x axis of a text rendering matrix in user space (`(1, 0)` when it is degenerate).
fn direction(m: &Matrix) -> (f64, f64) {
    let (x, y) = (m.0[0], m.0[1]);
    let length = x.hypot(y);
    if length.is_finite() && length > 1e-9 { (x / length, y / length) } else { (1.0, 0.0) }
}

/// User-space point `p` as `(along, across)` a baseline running in unit direction `dir`. Upright
/// text (`dir` = `(1, 0)`) keeps its coordinates, so lines and rows of turned text are found the
/// same way (#1443).
fn to_frame(dir: (f64, f64), p: (f64, f64)) -> (f64, f64) {
    (p.0 * dir.0 + p.1 * dir.1, p.1 * dir.0 - p.0 * dir.1)
}

/// The box covering both `[x0, y0, x1, y1]` boxes.
fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

/// What replacing a line did.
#[derive(Clone, Debug, PartialEq)]
pub struct LineEdit {
    /// `Some(font)` when the line's font couldn't show the new text and Helvetica was used.
    pub substituted: Option<String>,
}

#[derive(Clone)]
struct Ts {
    ctm: Matrix,
    /// The operators that set the current fill colour (`g`, `rg`, `k`, or `cs` + `sc`/`scn`).
    fill: Vec<Op>,
    font: Option<(Vec<u8>, Rc<Metrics>)>,
    /// The Japanese or Hebrew fallback face the font subsets, when the font is one (see [`is_type3_fallback`]).
    face: Option<String>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
    rise: f64,
    render_mode: i64,
    /// The transparency parameters the `gs` operators have set so far (see [`Paint`]).
    gs: Gs,
    /// The font's vertical advance per unit of font size (`/DW2`'s w1, normally −1) when its
    /// CMap writes vertically (`WMode 1`, §9.7.4.3): each glyph then moves the pen down, not right.
    vertical: Option<f64>,
    /// Why the font's writing mode couldn't be read (a broken or cyclic CMap). Its text is then
    /// listed as horizontal, but the lines in it are refused for editing.
    unreadable: Option<Rc<str>>,
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

fn page_dict(doc: &Document, page: usize) -> Result<pdfcraft_model::Page, EditError> {
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

/// The text-showing operators of one stream with their text, font, box and baseline.
struct Shown {
    op: usize,
    extra_ops: Vec<usize>,
    /// Encoded glyphs, not bytes or Unicode characters (a ligature can represent several).
    glyph_count: usize,
    /// Advance in text space, including spacing and TJ adjustments.
    advance: f64,
    tracking: f64,
    /// The text matrix where it starts (text space), and the state there and at its `BT`.
    tm: Matrix,
    tlm: Matrix,
    state: Ts,
    bt_op: usize,
    bt_state: Ts,
    text: String,
    rect: [f64; 4],
    /// Its box along and across its baseline (see [`TextLine::frame`]).
    frame: [f64; 4],
    /// The boxes of its words (see [`TextLine::ink`]).
    ink: Vec<[f64; 4]>,
    baseline: f64,
    start_x: f64,
    end_x: f64,
    end_y: f64,
    bt: usize,
    font: Vec<u8>,
    base_font: String,
    face: Option<String>,
    size: f64,
    bold: bool,
    italic: bool,
    visual_order: bool,
    decodable: bool,
    scope: Option<usize>,
    paint: Paint,
    logical_order: bool,
    actual_text: bool,
}

impl Shown {
    /// The unit direction of its baseline, where it starts and ends along it, and the baseline's
    /// offset across it (for upright text: the user-space x, x and y). A vertical run goes along
    /// its column instead, the way the pen moves: for upright text down, with the column's x
    /// across.
    fn span(&self) -> ((f64, f64), f64, f64, f64) {
        let m = self.tm.then(&self.state.ctm);
        let dir = match self.state.vertical {
            Some(w1) => direction(&Matrix([m.0[2] * w1.signum(), m.0[3] * w1.signum(), 0.0, 0.0, 0.0, 0.0])),
            None => direction(&m),
        };
        let (start, across) = to_frame(dir, (self.start_x, self.baseline));
        (dir, start, to_frame(dir, (self.end_x, self.end_y)).0, across)
    }
}

/// Where the previous run of a line ended: its text object, baseline direction, offset across
/// the baseline, end along it and size.
#[derive(Clone, Copy)]
struct Pen {
    bt: usize,
    dir: (f64, f64),
    across: f64,
    end: f64,
    size: f64,
    vertical: bool,
}

impl Pen {
    fn after(s: &Shown) -> Self {
        let (dir, _, end, across) = s.span();
        Pen { bt: s.bt, dir, across, end, size: s.size.max(1.0), vertical: s.state.vertical.is_some() }
    }

    /// Whether `s` could continue the same line on geometry alone: on the same baseline, starting
    /// at most a word gap (`reach` sizes) after it. Also the gap along the baseline.
    fn continues(&self, s: &Shown, reach: f64) -> (bool, f64) {
        let (dir, start, _, across) = s.span();
        let gap = start - self.end;
        let same_dir = (dir.0 - self.dir.0).abs() < 1e-3 && (dir.1 - self.dir.1).abs() < 1e-3;
        // A column continues a column (along which the pen moves), a baseline a baseline.
        let same_dir = same_dir && self.vertical == s.state.vertical.is_some();
        (same_dir && (across - self.across).abs() < self.size * 0.3 && gap > -self.size && gap < self.size * reach, gap)
    }
}

/// Use the lower third of letter gaps, not word gaps. A pair of isolated labels is not
/// evidence of tracking; unadjusted neighbours inside a string contribute zero too.
/// The tracking a run of glyphs was set with: the lower third of its gaps, when that is at
/// most half an em (`limit`). A wider "tracking" is word or cell spacing between one-glyph
/// words (`a b c`, a column of digits or Y/N ticks), which must keep its spaces.
fn typical_tracking(gaps: &mut [f64], limit: f64) -> f64 {
    if gaps.len() < 3 {
        return 0.0;
    }
    let at = gaps.len() / 3;
    gaps.select_nth_unstable_by(at, f64::total_cmp);
    gaps.get(at).copied().filter(|n| n.is_finite() && *n <= limit).unwrap_or(0.0).max(0.0)
}

fn tj_tracking(pieces: &[Object], metrics: &Metrics, size: f64, scale: f64) -> f64 {
    let mut gaps = Vec::new();
    let mut previous = false;
    let mut gap = 0.0;
    for piece in pieces {
        if let Object::String(s) = piece {
            for (code, len) in metrics.codes(&s.bytes) {
                let blank = metrics.is_space(code, len) || metrics.text_of(code).is_some_and(|t| !t.is_empty() && t.chars().all(char::is_whitespace));
                if previous && !blank {
                    gaps.push(gap);
                }
                previous = !blank;
                gap = 0.0;
            }
        } else if let Some(n) = piece.as_f64() {
            gap += -n / 1000.0 * size * scale;
        }
    }
    typical_tracking(&mut gaps, size * scale.abs() * 0.5)
}

fn positioned_tracking(shown: &mut [Shown]) {
    let mut start = 0;
    while start < shown.len() {
        let mut end = start.saturating_add(1);
        while let (Some(prev), Some(next)) = (shown.get(end.saturating_sub(1)), shown.get(end)) {
            if prev.glyph_count != 1
                || next.glyph_count != 1
                || prev.font != next.font
                || prev.scope != next.scope
                || (prev.size - next.size).abs() > 0.01
                || prev.state.char_spacing != next.state.char_spacing
                || prev.state.scale != next.state.scale
                || !Pen::after(prev).continues(next, 1.5).0
            {
                break;
            }
            end = end.saturating_add(1);
        }
        if let Some(run) = shown.get_mut(start..end) {
            let mut gaps: Vec<f64> = run
                .windows(2)
                .filter_map(|pair| {
                    let [prev, next] = pair else { return None };
                    (!prev.text.trim().is_empty() && !next.text.trim().is_empty()).then(|| Pen::after(prev).continues(next, 0.0).1)
                })
                .collect();
            let limit = run.first().map_or(0.0, |s| s.size * s.state.scale.abs() * 0.5);
            let tracking = typical_tracking(&mut gaps, limit);
            for s in run.iter_mut().skip(1) {
                s.tracking = tracking;
            }
        }
        start = end;
    }
}

/// The graphics state carried from one of a page's content streams to the next: the streams
/// are one stream in pieces (§7.8.2), so a `cm` (AutoCAD scales the whole page in the first
/// stream), an unbalanced `q`, the font and the colour still apply in the streams after it.
struct Carry {
    ts: Ts,
    stack: Vec<Ts>,
    marked: Vec<Option<usize>>,
    /// For each open marked-content sequence, its optional content group when it is `/OC`.
    layers: Vec<Option<Vec<u8>>>,
    /// Bytes of glyph text the rest of the page may still produce (see [`MAX_PAGE_TEXT`]).
    text_left: usize,
}

/// The most glyph text one page's extraction produces, in bytes. A ToUnicode destination is
/// copied for every glyph showing its code, so a small page can otherwise expand into gigabytes;
/// real pages hold far less (a whole novel is about 1 MB).
const MAX_PAGE_TEXT: usize = 16 << 20;

impl Carry {
    fn new() -> Self {
        let ts = Ts {
            ctm: Matrix::IDENTITY,
            fill: Vec::new(),
            font: None,
            face: None,
            size: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            render_mode: 0,
            gs: Gs::default(),
            vertical: None,
            unreadable: None,
        };
        Carry { ts, stack: Vec::new(), marked: Vec::new(), layers: Vec::new(), text_left: MAX_PAGE_TEXT }
    }
}

/// A font resource's metrics, fallback face and vertical advance (see [`Ts`]), read once per page.
/// The advance is an error message when the font's writing mode can't be read.
type CachedFont = (Rc<Metrics>, Option<String>, Result<Option<f64>, String>);

/// A `Do` met while interpreting: its operator index, XObject name and the state there.
type Invocation = (usize, Vec<u8>, Ts);

fn interpret(
    doc: &Document,
    ops: &[Op],
    resources: &Dict,
    cache: &mut HashMap<Vec<u8>, CachedFont>,
    carry: &mut Carry,
    mut invoked: Option<&mut Vec<Invocation>>,
    // `Some(properties)`: read `/ActualText`, looking named property lists up in `properties`.
    logical_text: Option<&Dict>,
) -> Result<Vec<Shown>, EditError> {
    let fonts_res = resources.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    let states = resources.get(b"ExtGState").map(|g| doc.resolve(g)).and_then(|g| g.as_dict().cloned()).unwrap_or_default();
    let mut out: Vec<Shown> = Vec::new();
    let mut ts = carry.ts.clone();
    let mut at_bt = (0usize, ts.clone());
    let mut stack: Vec<Ts> = std::mem::take(&mut carry.stack);
    let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
    let mut bt = 0usize;
    let mut marked = std::mem::take(&mut carry.marked);
    let mut layers = std::mem::take(&mut carry.layers);
    let mut actual: Vec<Option<(usize, usize, String)>> = Vec::new();
    for (i, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            // No depth cap: every `EMC` pops, so a capped push would pair it with the enclosing
            // sequence (#3573). Both stacks stay bounded by the number of operators.
            b"BMC" | b"BDC" => {
                // Marked text paragraphs/headings are authoritative boundaries. Ignore
                // artifacts and inner spans, which inherit their enclosing region.
                let region = matches!(op.name(0), Some(b"P" | b"H" | b"H1" | b"H2" | b"H3" | b"H4" | b"H5" | b"H6" | b"LI" | b"TD" | b"TH"));
                marked.push(if region { Some(i) } else { marked.last().copied().flatten() });
                // A layer named in the resources is told apart by its name, an inline one by where it opens.
                let layer = op.is("BDC") && op.name(0) == Some(b"OC".as_slice());
                layers.push(layer.then(|| op.operands.get(1).and_then(Object::as_name).map_or_else(|| format!("@{i}").into_bytes(), <[u8]>::to_vec)));
                // The property list is inline or named in the resources' /Properties (§14.6.2).
                let list = logical_text.and_then(|properties| match op.operands.get(1) {
                    Some(Object::Name(n)) => properties.get(n).and_then(|o| doc.dict(o)),
                    other => other.and_then(Object::as_dict).cloned(),
                });
                let logical = list.as_ref().and_then(|d| d.get(b"ActualText")).and_then(|s| match s {
                    Object::String(s) if s.bytes.len() <= 4_000_000 => Some(s.to_text()),
                    _ => None,
                });
                actual.push(logical.map(|s| (out.len(), i, s)));
            }
            b"EMC" => {
                marked.pop();
                layers.pop();
                if let Some(Some((start, open, logical))) = actual.pop() {
                    let tail = out.split_off(start.min(out.len()));
                    let mut items = tail.into_iter();
                    if let Some(mut first) = items.next() {
                        for shown in items {
                            first.glyph_count = first.glyph_count.saturating_add(shown.glyph_count);
                            first.extra_ops.push(shown.op);
                            first.extra_ops.extend(shown.extra_ops);
                            first.rect = [
                                first.rect[0].min(shown.rect[0]),
                                first.rect[1].min(shown.rect[1]),
                                first.rect[2].max(shown.rect[2]),
                                first.rect[3].max(shown.rect[3]),
                            ];
                            first.frame = union(first.frame, shown.frame);
                            first.ink.extend(shown.ink);
                        }
                        first.extra_ops.extend([open, i]);
                        // Only right-to-left ActualText is already in typing order and must stay
                        // out of the line's bidi recovery. Word and Chrome wrap Latin ligatures
                        // (`fi`, `fl`) in ActualText spans mid-word; those keep joining their line.
                        first.logical_order = logical.chars().any(crate::bidi::is_rtl);
                        first.actual_text = true;
                        first.text = logical;
                        first.start_x = first.rect[0];
                        first.end_x = first.rect[2];
                        first.end_y = first.baseline;
                        first.advance = (first.end_x - first.start_x) / first.tm.then(&first.state.ctm).0[0].abs().max(1e-6);
                        out.push(first);
                    }
                }
            }
            b"q" => stack.push(ts.clone()),
            b"Q" => {
                if let Some(s) = stack.pop() {
                    ts = s;
                }
            }
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    ts.ctm = Matrix(m).then(&ts.ctm);
                }
            }
            b"BT" => {
                tm = Matrix::IDENTITY;
                tlm = Matrix::IDENTITY;
                bt += 1;
                at_bt = (i, ts.clone());
            }
            b"g" | b"rg" | b"k" | b"cs" => ts.fill = vec![op.clone()],
            b"sc" | b"scn" => {
                ts.fill.retain(|o| o.is("cs"));
                ts.fill.push(op.clone());
            }
            b"Tf" => {
                ts.size = op.num(1).unwrap_or(ts.size);
                if let Some(name) = op.name(0) {
                    let (m, face, vertical) = cache.entry(name.to_vec()).or_insert_with(|| {
                        let dict = fonts_res.get(name).and_then(|f| doc.resolve(f).as_dict().cloned());
                        let metrics = dict.as_ref().map_or_else(Metrics::fallback, |d| Metrics::from_dict(doc, d));
                        let vertical = dict.as_ref().map_or(Ok(None), |d| vertical_advance(doc, d).map_err(|e| e.to_string()));
                        let face = dict.filter(is_type3_fallback).map(|_| metrics.base_font.clone());
                        (Rc::new(metrics), face, vertical)
                    });
                    ts.font = Some((name.to_vec(), m.clone()));
                    ts.face = face.clone();
                    // Which way the glyphs advance is unknown (§9.7.4.3): list the font's text as
                    // horizontal and record why, so editing its lines is refused (not the page).
                    match vertical {
                        Ok(v) => (ts.vertical, ts.unreadable) = (*v, None),
                        Err(e) => (ts.vertical, ts.unreadable) = (None, Some(Rc::from(e.as_str()))),
                    }
                }
            }
            b"Tc" => ts.char_spacing = op.num(0).unwrap_or(0.0),
            b"Tw" => ts.word_spacing = op.num(0).unwrap_or(0.0),
            b"Tz" => ts.scale = op.num(0).unwrap_or(100.0) / 100.0,
            b"TL" => ts.leading = op.num(0).unwrap_or(0.0),
            b"Ts" => ts.rise = op.num(0).unwrap_or(0.0),
            b"Tr" => ts.render_mode = op.operands.first().and_then(Object::as_int).unwrap_or(-1),
            b"gs" => {
                let params = op.name(0).and_then(|name| states.get(name)).map(|p| doc.resolve(p));
                if let Some(params) = params.as_deref().and_then(Object::as_dict) {
                    ts.gs.apply(doc, params);
                }
            }
            b"Do" => {
                if let (Some(list), Some(name)) = (invoked.as_deref_mut(), op.name(0)) {
                    list.push((i, name.to_vec(), ts.clone()));
                }
            }
            b"Td" | b"TD" => {
                if let Some([x, y]) = op.nums::<2>() {
                    if op.is("TD") {
                        ts.leading = -y;
                    }
                    tlm = Matrix([1.0, 0.0, 0.0, 1.0, x, y]).then(&tlm);
                    tm = tlm;
                }
            }
            b"Tm" => {
                if let Some(m) = op.nums::<6>() {
                    tlm = Matrix(m);
                    tm = tlm;
                }
            }
            b"T*" => {
                tlm = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, -ts.leading]).then(&tlm);
                tm = tlm;
            }
            b"Tj" | b"TJ" | b"'" | b"\"" => {
                if matches!(op.op.as_slice(), b"'" | b"\"") {
                    if op.is("\"") {
                        ts.word_spacing = op.num(0).unwrap_or(ts.word_spacing);
                        ts.char_spacing = op.num(1).unwrap_or(ts.char_spacing);
                    }
                    tlm = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, -ts.leading]).then(&tlm);
                    tm = tlm;
                }
                let Some((name, m)) = ts.font.clone() else { continue };
                // Pieces: strings, and TJ number adjustments.
                let pieces: Vec<Object> = match op.op.as_slice() {
                    // A surplus array (`[(A)] [(B)] TJ`) is malformed, but the operator still
                    // takes the one immediately before it, as the renderer and every other
                    // reader here do (#3569) -- not the first, which a reader counting operands
                    // from the front of a surplus list would otherwise take.
                    b"TJ" => op.operands.last().and_then(Object::as_array).cloned().unwrap_or_default(),
                    _ => op.operands.last().cloned().into_iter().collect(),
                };
                // Tc is already included in the pen advance; TJ may encode tracking instead.
                let tracking = if op.is("TJ") { tj_tracking(&pieces, &m, ts.size, ts.scale) } else { 0.0 };
                let trm0 = tm.then(&ts.ctm);
                let start = trm0.apply(0.0, ts.rise);
                let mut text = String::new();
                let mut decodable = true;
                let mut x_text = 0.0;
                // Vertical text (§9.4.4): the pen moves along y instead, and each glyph is centred
                // on it horizontally; `half` is half the widest glyph.
                let vertical = ts.vertical;
                let (mut y_text, mut half) = (0.0, 0.0_f64);
                // Where each word's glyphs start and end along the run (text space), and whether
                // the next glyph continues the last word.
                let mut words: Vec<(f64, f64)> = Vec::new();
                let mut in_word = false;
                let mut glyph_count = 0_usize;
                for p in &pieces {
                    match p {
                        Object::String(s) => {
                            for (code, len) in m.codes(&s.bytes) {
                                glyph_count = glyph_count.saturating_add(1);
                                let shown = m.text_of(code);
                                let blank =
                                    m.is_space(code, len) || shown.as_deref().is_some_and(|t| !t.is_empty() && t.chars().all(char::is_whitespace));
                                match shown {
                                    Some(t) => {
                                        carry.text_left = carry
                                            .text_left
                                            .checked_sub(t.len())
                                            .ok_or_else(|| EditError::Invalid("the page's text is too large to read".into()))?;
                                        text.push_str(&t);
                                    }
                                    None => decodable = false,
                                }
                                let spacing = ts.char_spacing + if m.is_space(code, len) { ts.word_spacing } else { 0.0 };
                                if let Some(w1) = vertical {
                                    half = half.max((m.width(code) * ts.size * ts.scale).abs() / 2.0);
                                    if !blank {
                                        let ink_end = y_text + w1 * ts.size;
                                        let (lo, hi) = (y_text.min(ink_end), y_text.max(ink_end));
                                        match words.last_mut() {
                                            Some(word) if in_word => *word = (word.0.min(lo), word.1.max(hi)),
                                            _ => words.push((lo, hi)),
                                        }
                                    }
                                    in_word = !blank;
                                    y_text += w1 * ts.size + spacing;
                                    continue;
                                }
                                if !blank {
                                    let ink_end = x_text + m.width(code) * ts.size * ts.scale;
                                    // A negative font size or `Tz` runs the glyphs backwards:
                                    // the word is the union of its glyphs, whichever way they go.
                                    let (lo, hi) = (x_text.min(ink_end), x_text.max(ink_end));
                                    match words.last_mut() {
                                        Some(word) if in_word => *word = (word.0.min(lo), word.1.max(hi)),
                                        _ => words.push((lo, hi)),
                                    }
                                }
                                in_word = !blank;
                                let w = m.width(code) * ts.size + ts.char_spacing + if m.is_space(code, len) { ts.word_spacing } else { 0.0 };
                                x_text += w * ts.scale;
                            }
                        }
                        other => {
                            if let Some(n) = other.as_f64() {
                                if vertical.is_some() {
                                    // Down by n/1000 em, not scaled by `Tz`.
                                    let dy = -n / 1000.0 * ts.size;
                                    if -dy > ts.size * 0.2 && !text.ends_with(' ') {
                                        text.push(' ');
                                    }
                                    if dy.abs() > ts.size.abs() * 0.2 {
                                        in_word = false;
                                    }
                                    y_text += dy;
                                    continue;
                                }
                                let dx = -n / 1000.0 * ts.size * ts.scale;
                                // A large gap inside TJ reads as a space.
                                if dx - tracking > (ts.size * ts.scale).abs() * 0.2 && !text.ends_with(' ') {
                                    text.push(' ');
                                }
                                // Tracking and kerning keep a word together; a larger jump
                                // ends it (whatever the sign of the size or `Tz`).
                                if dx - tracking > (ts.size * ts.scale).abs() * 0.2 || dx < -(ts.size * ts.scale).abs() * 0.2 {
                                    in_word = false;
                                }
                                x_text += dx;
                            }
                        }
                    }
                }
                tm = Matrix([1.0, 0.0, 0.0, 1.0, x_text, y_text]).then(&tm);
                let end = tm.then(&ts.ctm).apply(0.0, ts.rise);
                // The box from `a` to `b` along the run: from descent to ascent across it, or for
                // vertical text, the widest glyph's width centred on the pen.
                let span = |a: f64, b: f64| match vertical {
                    Some(_) => {
                        [trm0.apply(-half, ts.rise + a), trm0.apply(half, ts.rise + a), trm0.apply(-half, ts.rise + b), trm0.apply(half, ts.rise + b)]
                    }
                    None => [
                        trm0.apply(a, ts.rise + m.descent * ts.size),
                        trm0.apply(b, ts.rise + m.descent * ts.size),
                        trm0.apply(a, ts.rise + m.ascent * ts.size),
                        trm0.apply(b, ts.rise + m.ascent * ts.size),
                    ],
                };
                let corners = span(0.0, if vertical.is_some() { y_text } else { x_text });
                let rect = corners
                    .iter()
                    .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)]);
                // The same corners along and across the baseline: the run's own box, not its
                // upright bounding box turned (which is wider by the box's height on a slant).
                let dir = direction(&trm0);
                let frame = corners
                    .iter()
                    .map(|&p| to_frame(dir, p))
                    .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)]);
                let ink = words
                    .iter()
                    .map(|&(a, b)| {
                        span(a, b)
                            .iter()
                            .fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |r, p| [r[0].min(p.0), r[1].min(p.1), r[2].max(p.0), r[3].max(p.1)])
                    })
                    .collect();
                let size_user = (trm0.0[2].powi(2) + trm0.0[3].powi(2)).sqrt() * ts.size;
                // A numeric-only TJ adjusts the current text matrix but paints no glyphs.
                // It must not create a phantom editable line or enlarge its neighbours' boxes.
                if !pieces.iter().any(|p| matches!(p, Object::String(s) if !s.bytes.is_empty())) {
                    continue;
                }
                out.push(Shown {
                    op: i,
                    extra_ops: Vec::new(),
                    glyph_count,
                    // Vertical text: the TJ adjustment that undoes the run is n = -1000 y / size; the
                    // preview divides by size × `Tz`, so `Tz` is put back here.
                    advance: if vertical.is_some() { y_text * ts.scale } else { x_text },
                    tracking: 0.0,
                    tm: Matrix([
                        tm.0[0],
                        tm.0[1],
                        tm.0[2],
                        tm.0[3],
                        tm.0[4] - x_text * tm.0[0] - y_text * tm.0[2],
                        tm.0[5] - x_text * tm.0[1] - y_text * tm.0[3],
                    ]),
                    state: ts.clone(),
                    tlm,
                    bt_op: at_bt.0,
                    bt_state: at_bt.1.clone(),
                    text,
                    rect,
                    frame,
                    ink,
                    baseline: start.1,
                    start_x: start.0,
                    end_x: end.0,
                    end_y: end.1,
                    bt,
                    font: name,
                    base_font: m.base_font.clone(),
                    face: ts.face.clone(),
                    size: if size_user > 0.0 { size_user } else { ts.size },
                    bold: m.bold,
                    italic: m.italic,
                    visual_order: m.visual_order,
                    decodable,
                    scope: marked.last().copied().flatten(),
                    paint: Paint { layers: layers.iter().flatten().cloned().collect(), gs: ts.gs.clone() },
                    logical_order: false,
                    actual_text: false,
                });
            }
            _ => {}
        }
    }
    carry.ts = ts;
    carry.stack = stack;
    carry.marked = marked;
    carry.layers = layers;
    positioned_tracking(&mut out);
    Ok(out)
}

/// The lines of text on a page (0-based), in content order.
pub fn text_lines(doc: &Document, page: usize) -> Result<Vec<TextLine>, EditError> {
    text_lines_in(doc, &page_dict(doc, page)?, true)
}

/// The lines `page`'s glyphs show, in content order, without `/ActualText` replacements: what the
/// page draws, not what its marked-content metadata says the glyphs mean. For checks that must
/// not trust that metadata (the redaction audit). Takes the page itself, so a caller going
/// through every page doesn't walk the page tree again for each.
pub fn glyph_text_lines(doc: &Document, page: &pdfcraft_model::Page) -> Result<Vec<TextLine>, EditError> {
    text_lines_in(doc, page, false)
}

/// [`text_lines`] of `p`; with `actual_text`, `/ActualText` replaces the glyphs it spans.
fn text_lines_in(doc: &Document, p: &pdfcraft_model::Page, actual_text: bool) -> Result<Vec<TextLine>, EditError> {
    let res = p.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned()).unwrap_or_default();
    let mut cache = HashMap::new();
    let mut lines: Vec<TextLine> = Vec::new();
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
                        (s.size - l.size).abs() < 0.01 && s.bold == l.bold && s.italic == l.italic && fill_color(&s.state.fill) == l.color
                    }))
                && near
                // An OCR layer is one text object over the whole scan: its words continue a line
                // only across a word space, not across a table cell or a tab stop.
                && (s.state.render_mode != 3 || gap < pen.size * 0.6)
        });
        if joins && let Some(l) = lines.last_mut() {
            let gap = last.map_or(0.0, |(_, pen)| pen.continues(&s, 0.0).1);
            if gap - s.tracking > l.size * s.state.scale.abs() * 0.2 && !l.text.ends_with(' ') && !s.text.starts_with(' ') {
                l.text.push(' ');
            }
            l.text.push_str(&s.text);
            l.rect = [l.rect[0].min(s.rect[0]), l.rect[1].min(s.rect[1]), l.rect[2].max(s.rect[2]), l.rect[3].max(s.rect[3])];
            l.frame = union(l.frame, s.frame);
            l.ink.extend(s.ink.iter().copied());
            l.ops.push(s.op);
            l.ops.extend(s.extra_ops.iter().copied());
            l.decodable &= s.decodable;
            l.visual_order |= s.visual_order;
            l.actual_text |= s.actual_text;
            l.clips |= s.state.render_mode >= 4;
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
                unreadable_mode: s.state.unreadable.as_deref().map(str::to_owned),
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
    for l in lines.iter_mut().filter(|l| !l.logical_order) {
        // A visual-order Indic font draws pre-base vowel signs before their consonant.
        if l.visual_order && !l.actual_text {
            l.text = crate::indic::to_logical(&l.text);
        }
        l.text = crate::bidi::to_logical(&l.text);
    }
    // Lines of only spaces aren't editable text.
    lines.retain(|l| !l.text.trim().is_empty());
    Ok(lines)
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
fn add_shown(lines: &mut Vec<TextLine>, last: &mut Option<Pen>, s: Shown, stream: usize) {
    let joins = last.is_some_and(|pen| {
        let bt = pen.bt;
        lines.last().is_some_and(|l| l.scope == s.scope && l.paint == s.paint && l.logical_order == s.logical_order)
            && (bt == s.bt || lines.last().is_some_and(|l| compatible_word(l, &s)))
            && (s.text.trim().is_empty() || lines.last().is_some_and(|l| same_font(l, &s.font, &s.base_font, s.face.as_deref())))
            && (s.text.trim().is_empty()
                || lines.last().is_some_and(|l| {
                    (s.size - l.size).abs() < 0.01 && s.bold == l.bold && s.italic == l.italic && fill_color(&s.state.fill) == l.color
                }))
            && pen.continues(&s, if bt == s.bt { 3.0 } else { 1.5 }).0
    });
    if joins && let Some(l) = lines.last_mut() {
        let gap = last.map_or(0.0, |pen| pen.continues(&s, 0.0).1);
        if gap - s.tracking > l.size * s.state.scale.abs() * 0.2 && !l.text.ends_with(' ') && !s.text.starts_with(' ') {
            l.text.push(' ');
        }
        l.text.push_str(&s.text);
        l.rect = [l.rect[0].min(s.rect[0]), l.rect[1].min(s.rect[1]), l.rect[2].max(s.rect[2]), l.rect[3].max(s.rect[3])];
        l.frame = union(l.frame, s.frame);
        l.ink.extend(s.ink.iter().copied());
        l.ops.push(s.op);
        l.ops.extend(s.extra_ops.iter().copied());
        l.decodable &= s.decodable;
        l.visual_order |= s.visual_order;
        l.actual_text |= s.actual_text;
        l.clips |= s.state.render_mode >= 4;
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
            unreadable_mode: s.state.unreadable.as_deref().map(str::to_owned),
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
    *last = Some(Pen::after(&s));
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
fn reading_lines(doc: &Document, page: usize) -> Result<(Vec<TextLine>, bool), EditError> {
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
            add_shown(&mut w.lines, &mut last, s, si);
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
        l.text = crate::indic::to_logical(&l.text);
    }
    lines.retain(|l| !l.text.trim().is_empty());
    Ok((lines, w.budget.truncated))
}

/// Text → the runs that show it: each run's font resource name and the bytes shown in that font.
type Encoder = Box<dyn Fn(&str) -> Option<Vec<(String, Vec<u8>)>>>;

fn is_win_ansi_char(c: char) -> bool {
    c == '\t' || pdfcraft_fonts::win_ansi_byte(c).is_some()
}

fn needs_type3(text: &str) -> bool {
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
fn is_type3_fallback(font: &Dict) -> bool {
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
fn drop_unused_fallback_fonts(doc: &mut Document, page: usize) {
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
fn same_font(line: &TextLine, font: &[u8], base_font: &str, face: Option<&str>) -> bool {
    line.font.as_bytes() == font || same_base_font(&line.base_font, base_font) || (line.face.is_some() && line.face.as_deref() == face)
}

/// Whether two `/BaseFont` names name the same font: a subset's six-letter tag and `+` prefix do
/// not change the font (ISO 32000-2 §9.6.4), so `ABCDEF+Arial` and `GHIJKL+Arial` are one font.
/// A name that is missing, or only a tag (`ABCDEF+`), names no font and matches nothing.
fn same_base_font(a: &str, b: &str) -> bool {
    let (a, b) = (strip_subset_tag(a), strip_subset_tag(b));
    !a.is_empty() && a == b
}

/// `ABCDEF+Arial` → `Arial`. A name without a subset tag is returned unchanged.
fn strip_subset_tag(name: &str) -> &str {
    let bytes = name.as_bytes();
    let is_tag = bytes.get(6) == Some(&b'+') && bytes.get(..6).is_some_and(|tag| tag.iter().all(u8::is_ascii_uppercase));
    if is_tag { name.get(7..).unwrap_or(name) } else { name }
}

fn source_family(base_font: &str) -> crate::added::Family {
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

const SUBSTITUTE_NAME: &str = "PCEdHelv";
const SUBSTITUTE: &[u8] = SUBSTITUTE_NAME.as_bytes();
const MAX_TYPE3_GLYPHS: usize = 240;

/// The generated fonts for one replacement: one per run of at most `max_glyphs` unique characters,
/// in order of first appearance.
#[derive(Clone)]
struct Type3Fallback {
    /// The family its glyphs come from ("Shippori Mincho").
    family: &'static str,
    /// The fonts' resource names in order; the first is `PCJp` (`PCLat` for the bundled Latin
    /// face, `PCHe` for Hebrew text).
    names: Vec<String>,
    /// Every unique character with its advance (em), in order of first appearance.
    widths: Vec<(char, f64)>,
    /// Where each character is shown: the index of its font in `names`, and its code there.
    codes: HashMap<char, (usize, u8)>,
}

/// One glyph of a generated font: its code there, its character, its glyph procedure and advance.
struct Type3Glyph {
    code: u8,
    ch: char,
    path: Vec<u8>,
    width: f64,
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
fn type3_glyph(face: &CraftFont, ch: char) -> Result<Option<(Vec<u8>, f64)>, EditError> {
    match japanese_glyph_from(face, ch) {
        Err(GlyphError::Missing) => Ok(None),
        _ => type3_path(face, ch).map(Some),
    }
}

/// No face this build has can draw `text`. Only Japanese text is about the Japanese face: other
/// characters are missing from the paragraph's font, and choosing a font that has them works too.
fn no_fallback_font(text: &str) -> EditError {
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
fn no_glyph_for(face: &CraftFont, ch: char) -> EditError {
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
fn no_japanese_font() -> EditError {
    EditError::Invalid(
        "this text needs PdfCraft's Japanese fallback font, which this build doesn't include \
         (official releases do; to build it in, set CRAFT_FONTS_DIR to a craft-fonts checkout)"
            .into(),
    )
}

/// The first of `faces` (best match first) that has a glyph for every one of `chars`, else the first.
fn fallback_face<'a>(faces: &[&'a CraftFont], chars: &[char], has_glyph: impl Fn(&CraftFont, char) -> bool) -> Option<&'a CraftFont> {
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
fn type3_faces(text: &str, family: crate::added::Family, bold: bool) -> Result<(Vec<&'static CraftFont>, bool), EditError> {
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

fn type3_font(
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
    Ok(Type3Fallback { family: face.family, names, widths, codes })
}

/// Adds one generated font whose codes 1 to `glyphs.len()` show `glyphs`, and returns its resource
/// name (`PCLat`, `PCJp`, then `PCJp1`, ...; `PCHe`, ... for Hebrew text). Nothing here can fail: the
/// outlines were read already.
fn add_type3_font(doc: &mut Document, fonts_res: &mut Dict, face: &CraftFont, hebrew: bool, glyphs: Vec<Type3Glyph>) -> String {
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
fn type3_encode(fallback: &Type3Fallback, text: &str) -> Option<Vec<(String, Vec<u8>)>> {
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
fn source_program(doc: &Document, fonts: &Dict, name: &str) -> Option<Vec<u8>> {
    let font = fonts.get(name.as_bytes()).and_then(|f| doc.resolve(f).as_dict().cloned())?;
    let base = Metrics::from_dict(doc, &font).base_font;
    if base.split_once('+').is_some_and(|(tag, _)| tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase())) {
        return None;
    }
    let bytes = pdfcraft_fonts::pdf::embedded_sfnt(doc, &font)?;
    pdfcraft_fonts::embedding::validate(&bytes).is_ok().then_some(bytes)
}

/// Whether `text` has characters `line`'s font can't encode that its own program can draw.
fn source_program_completes(doc: &Document, fonts: &Dict, line: &TextLine, text: &str) -> bool {
    let Some(metrics) = fonts.get(line.font.as_bytes()).and_then(|f| doc.resolve(f).as_dict().cloned()).map(|f| Metrics::from_dict(doc, &f)) else {
        return false;
    };
    let missing: String = text.chars().filter(|c| !c.is_whitespace() && !source_encodes(&metrics, &c.to_string())).collect();
    !missing.is_empty() && source_program(doc, fonts, &line.font).is_some_and(|bytes| pdfcraft_fonts::embedding::covers(&bytes, &missing))
}

/// A subset may contain all the letters but no space glyph: producers use another subset
/// or a positioning operator for spaces. Do the same instead of substituting the whole font.
fn source_encodes(metrics: &Metrics, text: &str) -> bool {
    text.split(' ').all(|word| metrics.encode(word).is_some())
}

#[cfg(test)]
thread_local! {
    /// How often [`source_space`] ran on this thread, for the test that pins it out of per-character loops.
    pub(crate) static SOURCE_SPACE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn source_space(doc: &Document, fonts: &Dict, metrics: &Metrics) -> f64 {
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

fn source_width(metrics: &Metrics, text: &str, size: f64, space: f64, char_spacing: f64, word_spacing: f64) -> f64 {
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

fn source_show(metrics: &Metrics, text: &str, size: f64, space: f64, char_spacing: f64, word_spacing: f64) -> Result<Vec<Op>, EditError> {
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
fn coincident_ops(lines: &[TextLine], targets: &[&TextLine]) -> std::collections::HashSet<usize> {
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

/// How a listing of [`text_blocks`] records a paragraph so that it can be found again.
///
/// The index of a paragraph is its position in a list regrouped from geometry on every read, so
/// an edit that merges or splits paragraphs makes the same index name another one (#2516). A key
/// names the paragraph by what it says and where it is; a paragraph that no longer exists as
/// such has no match, instead of a neighbour taking its place.
#[derive(Clone, Debug, PartialEq)]
pub struct BlockKey {
    text: String,
    rect: [f64; 4],
}

impl BlockKey {
    /// Two boxes are the same box within this many points.
    const TOLERANCE: f64 = 0.01;

    pub fn of(block: &TextBlock) -> Self {
        Self { text: block.text.clone(), rect: block.rect }
    }

    /// The paragraph's box when it was recorded (user space).
    pub fn rect(&self) -> [f64; 4] {
        self.rect
    }

    /// The paragraph in `blocks` with this text and box. `hint` is tried first, so two
    /// paragraphs drawn on top of one another are told apart by their order.
    pub fn find(&self, blocks: &[TextBlock], hint: usize) -> Option<usize> {
        let matches = |b: &TextBlock| b.text == self.text && b.rect.iter().zip(&self.rect).all(|(a, b)| (a - b).abs() <= Self::TOLERANCE);
        match blocks.get(hint) {
            Some(b) if matches(b) => Some(hint),
            _ => blocks.iter().position(matches),
        }
    }
}

/// The paragraph in `blocks` whose box overlaps `rect` the most, if any overlaps it at all. An
/// edit rewrites a paragraph in place, so this is the paragraph it became.
pub fn block_overlapping(blocks: &[TextBlock], rect: [f64; 4]) -> Option<usize> {
    // `f64::min` and `max` skip a NaN, which would make a box that isn't a number overlap anything.
    let area = |b: &TextBlock| {
        if !rect.iter().chain(&b.rect).all(|v| v.is_finite()) {
            return 0.0;
        }
        let w = b.rect[2].min(rect[2]) - b.rect[0].max(rect[0]);
        let h = b.rect[3].min(rect[3]) - b.rect[1].max(rect[1]);
        if w > 0.0 && h > 0.0 { w * h } else { 0.0 }
    };
    blocks.iter().enumerate().map(|(i, b)| (i, area(b))).filter(|(_, a)| *a > 0.0).max_by(|(_, a), (_, b)| a.total_cmp(b)).map(|(i, _)| i)
}

/// The paragraphs on a page (0-based).
pub fn text_blocks(doc: &Document, page: usize) -> Result<Vec<TextBlock>, EditError> {
    let lines = text_lines(doc, page)?;
    Ok(group_blocks(&lines))
}

/// Remove a paragraph's glyphs for a temporary editor background. Numeric TJ adjustments
/// preserve the text matrix, so unselected text positioned relative to it stays in place.
/// Call this on a clone: it is a preview operation, not a document edit.
pub fn clear_block(doc: &mut Document, page: usize, block: usize) -> Result<(), EditError> {
    let lines = text_lines(doc, page)?;
    let blocks = group_blocks(&lines);
    let target = blocks.get(block).ok_or_else(|| EditError::Invalid("the paragraph no longer exists".into()))?;
    let targets: Vec<&TextLine> = target.lines.iter().filter_map(|i| lines.get(*i)).collect();
    targets.iter().try_for_each(|line| line.ensure_editable())?;
    let mut drops = coincident_ops(&lines, &targets);
    drops.extend(target.lines.iter().filter_map(|i| lines.get(*i)).flat_map(|l| l.ops.iter().copied()));
    let p = page_dict(doc, page)?;
    let res = p.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    let streams = content_streams(doc, &p.dict)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    let shown = interpret(doc, &ops, &res, &mut HashMap::new(), &mut Carry::new(), None, None)?;
    let mut patches = HashMap::new();
    for s in shown.into_iter().filter(|s| drops.contains(&s.op)) {
        if s.state.render_mode >= 4 {
            return Err(EditError::Invalid("text used as a clipping path cannot be previewed for editing".into()));
        }
        let denominator = s.state.size * s.state.scale;
        let adjustment = if denominator != 0.0 { -1000.0 * s.advance / denominator } else { 0.0 };
        if !adjustment.is_finite() || (denominator == 0.0 && s.advance != 0.0) {
            return Err(EditError::Invalid("the paragraph's text advance cannot be previewed".into()));
        }
        let mut replacement = Vec::new();
        if let Some(op) = ops.get(s.op) {
            if op.is("\"") {
                replacement.push(Op::new("Tw", vec![op.operands.first().cloned().unwrap_or(Object::Int(0))]));
                replacement.push(Op::new("Tc", vec![op.operands.get(1).cloned().unwrap_or(Object::Int(0))]));
            }
            if op.is("'") || op.is("\"") {
                replacement.push(Op::new("T*", vec![]));
            }
        }
        replacement.push(Op::new("TJ", vec![Object::Array(vec![pdfcraft_content::num(adjustment)])]));
        patches.insert(s.op, replacement);
    }
    let data = splice(&pieces, &ops, |i| patches.remove(&i).map_or((Vec::new(), true), |v| (v, false)));
    let contents = rewritten_contents(doc, &streams, data);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}

/// The paragraphs on a page as a reader sees them, including text drawn by form XObjects
/// (Export to Word, HTML and RTF). Read-only: indexes into this list are not
/// [`text_blocks`] indexes, so they can't be passed to [`replace_block`] or [`rewrite_block`].
/// [`reading_blocks_checked`] also says whether text was left out.
pub fn reading_blocks(doc: &Document, page: usize) -> Result<Vec<TextBlock>, EditError> {
    reading_blocks_checked(doc, page).map(|r| r.blocks)
}

/// A page's paragraphs as a reader sees them ([`reading_blocks_checked`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReadingBlocks {
    /// The paragraphs, as [`reading_blocks`] returns them.
    pub blocks: Vec<TextBlock>,
    /// The page draws forms nested deeper than [`MAX_FORM_DEPTH`], more of them than
    /// [`MAX_FORM_VISITS`], or forms that run past what the page's content left of its decode
    /// budget. Reading leaves those forms out to stay bounded on hostile files, so their text
    /// is missing from `blocks`; [`crate::reading_images`] walks the same forms and misses
    /// their images too.
    pub truncated: bool,
}

/// [`reading_blocks`], saying whether the limits that bound reading left text out. A page
/// whose `/Contents` alone runs past its decode budget is an [`EditError::Invalid`], not a
/// truncated result.
pub fn reading_blocks_checked(doc: &Document, page: usize) -> Result<ReadingBlocks, EditError> {
    let (lines, truncated) = reading_lines(doc, page)?;
    Ok(ReadingBlocks { blocks: regions::group_reading_blocks(&lines), truncated })
}

// An ideographic space carries intentional Japanese spacing, including paragraph indentation.
// Keep other whitespace normalization unchanged.
fn paragraph_separator(c: char) -> bool {
    c.is_whitespace() && c != '\u{3000}'
}

/// Greedy word wrapping to `width` with `advance` giving a string's width. Each `\n` starts a
/// new line. A row may also end inside a word of Chinese or Japanese text ([`crate::linebreak`]);
/// those pieces join without a space.
fn wrap(text: &str, width: f64, advance: impl Fn(&str) -> f64) -> Vec<String> {
    let mut out = Vec::new();
    for row in text.split('\n') {
        let mut line = String::new();
        let start = out.len();
        for word in row.split(paragraph_separator).filter(|word| !word.is_empty()) {
            for (i, piece) in crate::linebreak::pieces(word).into_iter().enumerate() {
                let candidate = match (line.is_empty(), i) {
                    (true, _) => piece.to_string(),
                    (false, 0) => format!("{line} {piece}"),
                    (false, _) => format!("{line}{piece}"),
                };
                if !line.is_empty() && advance(&candidate) > width {
                    out.push(std::mem::take(&mut line));
                    line = piece.to_string();
                } else {
                    line = candidate;
                }
            }
        }
        if !line.is_empty() || out.len() == start {
            out.push(line);
        }
    }
    out
}

/// Replace paragraph `block` (an index into [`text_blocks`]) with `text`, rewrapped to the
/// paragraph's width with its line spacing. The paragraph keeps its first line's position, font
/// (or Helvetica when the font can't show the text), size and colour.
pub fn replace_block(doc: &mut Document, page: usize, block: usize, text: &str) -> Result<LineEdit, EditError> {
    rewrite_block(doc, page, block, Some(text), &BlockStyle::default())
}

/// Formatting for a rewritten paragraph (`None` keeps the paragraph's own).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlockStyle {
    /// Disjoint UTF-8 ranges in the replacement, each preserving unspecified source style.
    pub ranges: Vec<RangeStyle>,
    /// Explicit page font resource (from register_text_font or the page font inventory).
    pub resource: Option<String>,
    /// A standard font family, bold, italic.
    pub family: Option<(crate::added::Family, bool, bool)>,
    /// Override weight without choosing a different family (`None` keeps the source weight).
    pub bold: Option<bool>,
    /// Font size in points (user space).
    pub size: Option<f64>,
    /// Fill colour (RGB 0–1).
    pub color: Option<[f64; 3]>,
    pub align: Option<crate::added::Align>,
    /// Draw a line under each line of text.
    pub underline: Option<bool>,
    /// Line spacing as a multiple of the font size (1.2 is ordinary).
    pub line_spacing: Option<f64>,
    /// Character spacing (points) and horizontal scaling (percent).
    pub char_spacing: Option<f64>,
    pub scale: Option<f64>,
    /// Move the paragraph by `[dx, dy]` in user space (dragging its box).
    pub offset: Option<[f64; 2]>,
    /// Rewrap to this width in user space (dragging the box's edge).
    pub width: Option<f64>,
    /// For an invisible paragraph ([`TextBlock::invisible`]): fill its old area with this colour
    /// (RGB 0–1) before the new text, hiding the scanned word beneath it.
    pub cover: Option<[f64; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RangeStyle {
    pub range: std::ops::Range<usize>,
    pub family: Option<(crate::added::Family, bool, bool)>,
    pub resource: Option<String>,
    pub size: Option<f64>,
    pub color: Option<[f64; 3]>,
    pub underline: Option<bool>,
    pub char_spacing: Option<f64>,
    pub scale: Option<f64>,
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

/// [`rewrite_block_with_limit`] with the page's [`text_lines`], read from `doc` as it is now.
fn rewrite_block_from(
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
    let registered_face = fonts_res.iter().any(|(name, _)| name.starts_with(b"PCFull"));
    // Hebrew is written back in drawing order (and with the Hebrew fallback face) by the
    // single-style path below; the styled region path does not reorder right-to-left runs.
    let hebrew = text.unwrap_or(&b.text).chars().any(crate::bidi::is_hebrew);
    if hebrew && (!style.ranges.is_empty() || style.resource.is_some()) {
        return Err(EditError::Invalid(
            "formatting part of right-to-left (Hebrew) text, or choosing a document font for it, isn't supported yet".into(),
        ));
    }
    // Text turned on the page (a landscape page drawn a quarter turn round) is rewritten in its
    // own text space below; the styled region path lays rows out along the page's x axis.
    let upright = members.iter().all(|line| line.upright());
    if !upright && (!style.ranges.is_empty() || style.resource.is_some()) {
        return Err(EditError::Invalid("formatting part of turned text, or choosing a document font for it, isn't supported yet".into()));
    }
    let visual_indic = members.iter().any(|line| line.visual_order);
    if upright
        && !hebrew
        && !visual_indic
        && (!style.ranges.is_empty()
            || style.resource.is_some()
            || (text.is_some_and(|draft| {
                draft.contains('\n')
                    || pdfcraft_fonts::source_shaping::required(draft)
                    || !pdfcraft_fonts::embedding::matching_faces(&b.base_font).is_empty()
            }) || registered_face
                || text.is_some_and(|draft| source_program_completes(doc, &fonts_res, first, draft))
                || b.rows.iter().any(|row| row.len() > 1)
                || members.iter().any(|line| line.font != first.font || line.size != first.size || line.color != first.color))
                && style.family.is_none()
                && style.bold.is_none())
    {
        return rewrite_styled_block(doc, page, &b, &lines, text, style);
    }
    let streams = content_streams(doc, &p.dict)?;
    let pieces = Pieces::join(&streams.iter().map(|(_, d)| d.as_slice()).collect::<Vec<_>>());
    let ops = pieces.parse().ops;
    // Explicit line breaks stay (#1443); within a line, runs of white space become one space.
    let text = text
        .unwrap_or(&b.text)
        .split('\n')
        .map(|row| row.split(paragraph_separator).filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join("\n");
    // The characters to show, for choosing fonts: the breaks are moves, not glyphs.
    let flat = text.replace('\n', " ");
    let o = &first.origin;
    let last = members.last().copied().unwrap_or(first);
    let emit = &last.origin;
    let (font_name, old_size) = o.state.font.clone().ok_or_else(|| EditError::Invalid("the paragraph has no font".into()))?;
    let k = o.k.max(1e-6);
    // The size in text space: points ÷ the text-to-user scale.
    let size = style.size.map_or(old_size, |pt| (pt / k).max(0.1));
    // Spacing and scaling (text state), with the chosen values.
    let mut ts_state = o.state.clone();
    if let Some(c) = style.char_spacing {
        ts_state.char_spacing = c / k;
    }
    if let Some(sc) = style.scale {
        ts_state.scale = (sc / 100.0).clamp(0.1, 10.0);
    }
    let o_state = ts_state.clone();
    let metrics = fonts_res.get(font_name.as_bytes()).and_then(|f| doc.resolve(f).as_dict().cloned()).map(|d| Metrics::from_dict(doc, &d));
    let visual_indic = metrics.as_ref().is_some_and(|m| m.visual_order) || first.visual_order;
    let indic_text = if visual_indic { crate::indic::to_visual(&flat) } else { flat.clone() };
    let reuse = style.family.is_none() && style.bold.is_none() && metrics.as_ref().is_some_and(|m| source_encodes(m, &indic_text));
    let space = metrics.as_ref().map_or(0.33, |m| source_space(doc, &fonts_res, m));
    // The chosen style also determines the real Japanese fallback outlines and advances.
    let (family, bold, italic) = style.family.unwrap_or((source_family(&b.base_font), b.bold, b.italic));
    let bold = style.bold.unwrap_or(bold);
    // The face is chosen after `bold` settles, so a bold request measures with the bold metrics.
    let face = family.std14(bold, italic);
    let std_width = move |s: &str, size: f64| -> f64 { face.text_width(s, size) };
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
    // A paragraph keeps its width (or takes the one asked for); a single line grows to the right
    // (to the left when right-to-left), up to the page's margin.
    let width = match style.width {
        // Capped at the largest page PDF allows (14 400 pt), even for text set larger than that
        // (`clamp` panics when its minimum is the larger, or NaN).
        Some(w) if w.is_finite() => w.clamp((size * k).min(14_400.0), 14_400.0),
        Some(_) => return Err(EditError::Invalid("the paragraph's width must be a number".into())),
        // Turned text: the page's margins are along another axis. A single line grows (below).
        None if !upright => (frame[2] - frame[0]).max(size * k),
        None if anchored_right => (b.rect[2] - b.rect[0]).max(size * k).max(b.rect[2] + dx - (crop[0] + 36.0)),
        // A centred line is not stretched to the margin: its own box is the frame it is centred in.
        None if members.len() == 1 && !centred => (b.rect[2] - b.rect[0]).max(size * k).max(crop[2] - 36.0 - (b.rect[0] + dx)),
        None => (b.rect[2] - b.rect[0]).max(size * k),
    };
    if !width.is_finite() {
        return Err(EditError::Invalid("the paragraph's width must be a number".into()));
    }
    let align = if centred { Some(crate::added::Align::Center) } else { style.align.or(rtl.then_some(crate::added::Align::Right)) };
    // Where a right-aligned line ends, from the paragraph's left edge (user space).
    let right_edge = if anchored_right { (b.rect[2] - b.rect[0]).max(0.0) } else { width };
    // The move and the width are checked here, before the fallback fonts are written: a refused
    // paragraph leaves the document as it was. The move is in page space, so it is taken back through
    // the CTM's linear part into the space the paragraph is drawn in.
    let movement = if dx != 0.0 || dy != 0.0 {
        let c = emit.ctm;
        let back = Matrix([c[0], c[1], c[2], c[3], 0.0, 0.0])
            .invert()
            .ok_or_else(|| EditError::Invalid("the paragraph is drawn in a space it can't be moved in".into()))?;
        Some(back.apply(dx, dy))
    } else {
        None
    };
    let tm = if o.ctm == emit.ctm {
        o.tm
    } else {
        let emit_inv = Matrix(emit.ctm).invert().ok_or_else(|| EditError::Invalid("the paragraph has a singular transform".into()))?;
        Matrix(o.tm).then(&Matrix(o.ctm)).then(&emit_inv).0
    };
    let type3 = if !reuse && needs_type3(&flat) { Some(type3_font(doc, &mut fonts_res, &flat, family, bold, max_glyphs)?) } else { None };
    let advance = |s: &str| -> f64 {
        let t = match (&metrics, reuse) {
            (Some(m), true) => {
                let visual_s = if m.visual_order { crate::indic::to_visual(s) } else { s.to_owned() };
                source_width(m, &visual_s, size, space, o_state.char_spacing, o_state.word_spacing)
            }
            _ if let Some(fallback) = &type3 => {
                // Each character's count in `s`, from one pass over `s`.
                let mut counts: HashMap<char, usize> = HashMap::new();
                for c in s.chars() {
                    let count = counts.entry(c).or_insert(0);
                    *count = count.saturating_add(1);
                }
                fallback.widths.iter().map(|(ch, width)| counts.get(ch).copied().unwrap_or(0) as f64 * width * size).sum::<f64>()
                    + s.chars().count() as f64 * o_state.char_spacing
            }
            _ => std_width(s, size) + s.chars().count() as f64 * o_state.char_spacing,
        };
        t * o_state.scale * k
    };
    // A turned single line grows to fit, up to where its baseline reaches the page's margin.
    let width = if !upright && style.width.is_none() && members.len() == 1 {
        let crop = p.crop(doc);
        let (x0, y0) = (first.origin.x + dx, first.origin.baseline + dy);
        let (ux, uy) = first.direction();
        let tx = if ux > 1e-6 {
            (crop[2] - 36.0 - x0) / ux
        } else if ux < -1e-6 {
            (crop[0] + 36.0 - x0) / ux
        } else {
            f64::INFINITY
        };
        let ty = if uy > 1e-6 {
            (crop[3] - 36.0 - y0) / uy
        } else if uy < -1e-6 {
            (crop[1] + 36.0 - y0) / uy
        } else {
            f64::INFINITY
        };
        let cap = width.max(tx.min(ty)).min(14_400.0);
        text.split('\n').map(&advance).fold(width, f64::max).min(cap)
    } else {
        width
    };
    let right_edge = if anchored_right { right_edge } else { width };
    let wrapped = wrap(&text, width + 0.5, advance);
    let mut substituted = None;
    let new_font = !reuse;
    let (show_font, encode): (String, Encoder) = if let Some(m) = metrics.clone().filter(|_| reuse) {
        let name = font_name.clone();
        (font_name.clone(), Box::new(move |s: &str| m.encode(s).map(|bytes| vec![(name.clone(), bytes)])))
    } else if let Some(fallback) = type3.clone() {
        substituted = Some(format!("{} Type3", fallback.family));
        let first = fallback.names.first().cloned().ok_or_else(no_japanese_font)?;
        (first, Box::new(move |s: &str| type3_encode(&fallback, s)))
    } else {
        let win = pdfcraft_fonts::win_ansi(&flat);
        let back: String = win.iter().map(|c| char::from_u32(u32::from(*c)).unwrap_or('?')).collect();
        let base = family.base_font(bold, italic);
        if flat.chars().zip(back.chars()).any(|(a, c)| c == '?' && a != '?') {
            return Err(EditError::Invalid(format!("\"{flat}\" has characters neither {} nor {base} can show", b.base_font)));
        }
        let mut f = Dict::new();
        f.set(b"Type".to_vec(), Object::name("Font"));
        f.set(b"Subtype".to_vec(), Object::name("Type1"));
        f.set(b"BaseFont".to_vec(), Object::name(base));
        f.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
        let name = if style.family.is_none() { String::from_utf8_lossy(SUBSTITUTE).into_owned() } else { format!("PCEd{}", base.replace('-', "")) };
        fonts_res.set(name.clone().into_bytes(), Object::Dict(f));
        if style.family.is_none() {
            substituted = Some(base.to_string());
        }
        let shown = name.clone();
        (name, Box::new(move |s: &str| Some(vec![(shown.clone(), pdfcraft_fonts::win_ansi(s))])))
    };
    // Line spacing in text space: the paragraph's own (scaled with the size), or 1.2 × the size.
    let lead = match style.line_spacing {
        Some(m) => size * m.clamp(0.5, 5.0),
        // From the first two rows (a row may hold several runs).
        None if let Some((upper, lower)) = b
            .rows
            .first()
            .zip(b.rows.get(1))
            .and_then(|(upper, lower)| upper.first().zip(lower.first()))
            .and_then(|(upper, lower)| lines.get(*upper).zip(lines.get(*lower))) =>
        {
            (upper.frame_origin().1 - lower.frame_origin().1) / k * size / old_size
        }
        None if o_state.leading > 0.0 => o_state.leading,
        None => size * 1.2,
    };
    let n = pdfcraft_content::num;
    // In its own graphics state, so a new colour (or anything else) stops at the paragraph.
    let mut block_ops = vec![Op::new("q", vec![])];
    block_ops.extend(reveal_ops(&b, &lines, style, emit.ctm)?);
    // Moved: a translation inside that state.
    if let Some((ax, ay)) = movement {
        block_ops.push(Op::new("cm", vec![n(1.0), n(0.0), n(0.0), n(1.0), n(ax), n(ay)]));
    }
    block_ops.push(Op::new("BT", vec![]));
    let mut state = o_state.clone();
    state.word_spacing = 0.0;
    // The font the next run is shown in: the one the BT sets, until a run of another fallback subset.
    let mut shown_font = show_font.clone();
    state.font = Some((show_font, size));
    if let Some([r, g, bl]) = style.color {
        state.fill = vec![Op::new("rg", vec![n(r), n(g), n(bl)])];
    }
    block_ops.extend(state.ops());
    block_ops.push(Op::new("Tm", tm.iter().map(|v| n(*v)).collect()));
    // Alignment: each line's offset from the left edge, in text space.
    let offset = |line: &str| -> f64 {
        let free = (width - advance(line)).max(0.0) / k;
        match align {
            Some(crate::added::Align::Center) => free / 2.0,
            // A right-to-left line anchored at its right edge may start left of the old box.
            Some(crate::added::Align::Right) if anchored_right => (right_edge - advance(line)) / k,
            Some(crate::added::Align::Right) => free,
            // A justified right-to-left paragraph ends its last line at the right edge.
            Some(crate::added::Align::Justify) if rtl => free,
            _ => 0.0,
        }
    };
    // Justify: each line but the last fills the width. Word spacing reaches only single-byte code 32
    // (ISO 32000-1 §9.3.3), so where a space is shown with another code (a composite font, a
    // generated Type 3 font, a subset without a space) the extra space follows each space as a
    // TJ adjustment instead, and code 32, which may then be a letter, gets none.
    let justify = align == Some(crate::added::Align::Justify);
    let word_spacing_reaches_spaces = match metrics.as_ref().filter(|_| reuse) {
        Some(m) => m.encode(" ").is_some_and(|b| m.codes(&b).iter().all(|(c, len)| m.is_space(*c, *len))),
        None => type3.is_none(),
    };
    let mut x = 0.0;
    let mut tw_set = 0.0;
    let mut underlines: Vec<(f64, f64, f64)> = Vec::new(); // (x0, x1, y) in text space
    for (i, line) in wrapped.iter().enumerate() {
        let spaces = line.matches(' ').count();
        let tw =
            if justify && i + 1 < wrapped.len() && spaces > 0 { (width - advance(line)).max(0.0) / k / o_state.scale / spaces as f64 } else { 0.0 };
        // A stretched line fills the width from the left edge.
        let dx = if tw > 0.0 { 0.0 } else { offset(line) };
        if i > 0 || dx != 0.0 {
            block_ops.push(Op::new("Td", vec![n(dx - x), n(if i > 0 { -lead } else { 0.0 })]));
        }
        x = dx;
        let stretch = if word_spacing_reaches_spaces { 0.0 } else { tw };
        let tw = tw - stretch;
        if tw != tw_set {
            block_ops.push(Op::new("Tw", vec![n(tw)]));
            tw_set = tw;
        }
        // Each wrapped line in typing order; drawn left to right (right-to-left runs reversed).
        let visual = crate::bidi::to_visual(line);
        // Stretched without word spacing: shown up to and including each space, then moved on.
        let pieces = if stretch > 0.0 { visual.split_inclusive(' ').collect() } else { vec![visual.as_str()] };
        for piece in pieces {
            if let Some(metrics) = metrics.as_ref().filter(|_| reuse) {
                let indic_piece = if metrics.visual_order { crate::indic::to_visual(piece) } else { piece.to_owned() };
                block_ops.extend(source_show(metrics, &indic_piece, size, space, o_state.char_spacing, tw)?);
            } else {
                for (font, bytes) in encode(piece).ok_or_else(|| EditError::Invalid(format!("\"{line}\" can't be shown")))? {
                    if font != shown_font {
                        block_ops.push(Op::new("Tf", vec![Object::name(&font), n(size)]));
                        shown_font = font;
                    }
                    block_ops.push(Op::new("Tj", vec![Object::String(PdfString::literal(bytes))]));
                }
            }
            if stretch > 0.0 && piece.ends_with(' ') {
                block_ops.push(Op::new("TJ", vec![Object::Array(vec![n(-1000.0 * stretch / size)])]));
            }
        }
        let tw = tw + stretch;
        let w = (advance(line) + tw * spaces as f64 * o_state.scale * k) / k;
        underlines.push((dx, dx + w, -(i as f64) * lead - size * 0.12));
    }
    block_ops.push(Op::new("ET", vec![]));
    if style.underline == Some(true) {
        // In text space (the paragraph's text matrix), in its fill colour.
        let colour: Vec<Op> = state
            .fill
            .iter()
            .map(|f| match f.op.as_slice() {
                b"g" => Op::new("G", f.operands.clone()),
                b"rg" => Op::new("RG", f.operands.clone()),
                b"k" => Op::new("K", f.operands.clone()),
                b"cs" => Op::new("CS", f.operands.clone()),
                b"sc" => Op::new("SC", f.operands.clone()),
                _ => Op::new("SCN", f.operands.clone()),
            })
            .collect();
        block_ops.push(Op::new("q", vec![]));
        block_ops.push(Op::new("cm", tm.iter().map(|v| n(*v)).collect()));
        block_ops.extend(colour);
        block_ops.push(Op::new("w", vec![n(size * 0.06)]));
        for (x0, x1, y) in underlines {
            block_ops.push(Op::new("m", vec![n(x0), n(y)]));
            block_ops.push(Op::new("l", vec![n(x1), n(y)]));
        }
        block_ops.push(Op::new("S", vec![]));
        block_ops.push(Op::new("Q", vec![]));
    }
    block_ops.push(Op::new("Q", vec![]));
    // The paragraph's operators go, and so does another drawing on the same baseline (a fake-bold
    // second copy, an invisible text layer): the new text is all that may show.
    let mut drop = coincident_ops(&lines, &members);
    drop.extend(members.iter().flat_map(|l| l.ops.iter().copied()));
    // Text shown earlier in the same text object stays first: the paragraph then goes where its
    // last row was, after any backgrounds between the rows, or between two halves of the text object.
    let start = last.ops.first().copied().unwrap_or(emit.bt_op);
    let split = ops
        .get(emit.bt_op..start)
        .unwrap_or_default()
        .iter()
        .enumerate()
        .any(|(i, op)| matches!(op.op.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"") && !drop.contains(&(emit.bt_op + i)));
    // What the text after the paragraph expects: the state at its BT (or where it was split).
    let after = if split { &emit.state } else { &emit.bt_state };
    block_ops.extend(after.ops().into_iter().filter(|op| !op.is("Tf") || after.font.is_some()));
    if split {
        block_ops.insert(0, Op::new("ET", vec![]));
        block_ops.push(Op::new("BT", vec![]));
        block_ops.push(Op::new("Tm", emit.tlm.iter().map(|v| n(*v)).collect()));
    }
    let at = if split { start } else { emit.bt_op };
    let new_data = splice(&pieces, &ops, |i| {
        let insert = if i == at { std::mem::take(&mut block_ops) } else { Vec::new() };
        (insert, !drop.contains(&i))
    });
    let contents = rewritten_contents(doc, &streams, new_data);
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
    Ok(LineEdit { substituted })
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
fn vertical_advance(doc: &Document, font: &Dict) -> Result<Option<f64>, EditError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str, rect: [f64; 4]) -> TextBlock {
        TextBlock {
            text: text.into(),
            rect,
            base_font: String::new(),
            size: 12.0,
            bold: false,
            italic: false,
            color: [0.0; 3],
            rotation: 0.0,
            lines: Vec::new(),
            rows: Vec::new(),
            editor_text: text.into(),
            spans: Vec::new(),
            invisible: false,
        }
    }

    #[test]
    fn a_block_key_finds_its_paragraph_after_others_move_and_not_a_neighbour_in_its_place() {
        let (a, b, c) = (block("Alpha", [0.0, 80.0, 50.0, 100.0]), block("Bravo", [0.0, 50.0, 50.0, 60.0]), block("Charlie", [0.0, 0.0, 50.0, 20.0]));
        let key = BlockKey::of(&b);
        assert_eq!(key.find(&[a.clone(), b.clone(), c.clone()], 1), Some(1));
        // Alpha and Bravo became one paragraph: Bravo's number now belongs to Charlie.
        let merged = block("Alpha Bravo", [0.0, 50.0, 50.0, 100.0]);
        assert_eq!(key.find(&[merged, c.clone()], 1), None);
        // A paragraph that split in two shifts the others down.
        assert_eq!(key.find(&[a.clone(), block("x", [0.0; 4]), b.clone(), c], 1), Some(2));
        // The same words somewhere else, or other words in the same place, are not the paragraph.
        assert_eq!(key.find(&[block("Bravo", [0.0, 40.0, 50.0, 50.0])], 0), None);
        assert_eq!(key.find(&[block("Bravo!", b.rect)], 0), None);
        assert_eq!(key.find(&[], 0), None);
        // Two paragraphs drawn on top of one another are told apart by the hint.
        assert_eq!(key.find(&[b.clone(), b], 1), Some(1));
        // A box that is not a number matches nothing, and cannot panic.
        let nan = BlockKey::of(&block("Bravo", [f64::NAN; 4]));
        assert_eq!(nan.find(&[block("Bravo", [f64::NAN; 4])], 0), None);
    }

    #[test]
    fn the_block_that_overlaps_most_is_the_edited_paragraph() {
        let blocks = [block("a", [0.0, 80.0, 50.0, 100.0]), block("b", [0.0, 40.0, 50.0, 70.0])];
        assert_eq!(block_overlapping(&blocks, [0.0, 60.0, 50.0, 95.0]), Some(0));
        assert_eq!(block_overlapping(&blocks, [0.0, 35.0, 50.0, 75.0]), Some(1));
        assert_eq!(block_overlapping(&blocks, [200.0, 0.0, 250.0, 10.0]), None, "a box that overlaps nothing");
        assert_eq!(block_overlapping(&[], [0.0, 0.0, 1.0, 1.0]), None);
        assert_eq!(block_overlapping(&blocks, [f64::NAN; 4]), None);
    }

    #[test]
    fn a_subset_tag_is_the_only_difference_between_subsets_of_one_font() {
        assert!(same_base_font("ABCDEF+Arial", "GHIJKL+Arial"));
        assert!(same_base_font("Arial", "ABCDEF+Arial"));
        assert!(same_base_font("Arial", "Arial"));
        assert!(!same_base_font("ABCDEF+Arial", "GHIJKL+Helvetica"));
        // Five letters are part of the name, not a subset tag.
        assert!(!same_base_font("ABCDE+Helvetica-Bold", "Helvetica-Bold"));
        // A tag with no name after it, or no name at all, names no font.
        assert!(!same_base_font("ABCDEF+", "GHIJKL+"));
        assert!(!same_base_font("", ""));
        assert!(!same_base_font("Arial", ""));
    }

    #[test]
    fn paragraph_wrapping_keeps_ideographic_spaces() {
        let width = |s: &str| s.chars().count() as f64;
        assert_eq!(wrap("A　　B C", 4.0, width), ["A　　B", "C"]);
        assert_eq!(wrap("A\u{a0}B C", 3.0, width), ["A B", "C"]);
        assert_eq!(wrap("A\u{2028}B C", 3.0, width), ["A B", "C"]);
        assert_eq!(wrap(" \tA  B\r\nC ", 3.0, width), ["A B", "C"]);
    }

    #[test]
    fn japanese_text_wraps_between_characters_but_not_before_closing_marks() {
        let width = |s: &str| s.chars().count() as f64;
        assert_eq!(wrap("日本語の文章です。", 3.0, width), ["日本語", "の文章", "です。"]);
        // The comma can't begin a row, so the character before it moves down with it.
        assert_eq!(wrap("あいう、えお", 3.0, width), ["あい", "う、え", "お"]);
        // Pieces of one word join without a space; separate words keep theirs.
        assert_eq!(wrap("Edit PDFを編集", 6.0, width), ["Edit", "PDFを編集"]);
        assert_eq!(wrap("一行目　二行目", 4.0, width), ["一行目　", "二行目"]);
    }

    #[test]
    fn fallback_face_moves_on_when_the_best_face_lacks_a_character() {
        static JPAN: &[&str] = &["Jpan"];
        let mincho = CraftFont { family: "Mincho", style: "Regular", scripts: JPAN, bytes: b"" };
        let gothic = CraftFont { family: "Gothic", style: "Regular", scripts: JPAN, bytes: b"" };
        let faces = [&mincho, &gothic];
        // Only the Gothic face has the Cyrillic letter.
        let has = |face: &CraftFont, ch: char| ch.is_ascii() || face.family == "Gothic";
        assert_eq!(fallback_face(&faces, &['a', 'ф'], has).map(|f| f.family), Some("Gothic"));
        // Characters the best face has keep it.
        assert_eq!(fallback_face(&faces, &['a', 'b'], has).map(|f| f.family), Some("Mincho"));
        // No face has them all: the best face stays, and names the missing glyph.
        assert_eq!(fallback_face(&faces, &['a', 'ф'], |_, ch| ch.is_ascii()).map(|f| f.family), Some("Mincho"));
        assert!(fallback_face(&[], &['a'], has).is_none());
    }

    #[test]
    fn hebrew_and_japanese_fallback_shards_are_recognized_as_ours() {
        static HEBR: &[&str] = &["Hebr"];
        let face = CraftFont { family: "FakeHebrew", style: "Regular", scripts: HEBR, bytes: b"" };
        let glyph = |code, ch| Type3Glyph { code, ch, path: b"0 0 0 0 0 0 d1\nf\n".to_vec(), width: 0.5 };
        let mut doc = Document::new_empty();
        let mut fonts = Dict::new();
        // A replacement split across two Hebrew fonts, then one Japanese font.
        let he = add_type3_font(&mut doc, &mut fonts, &face, true, vec![glyph(1, 'א')]);
        let he1 = add_type3_font(&mut doc, &mut fonts, &face, true, vec![glyph(1, 'ב')]);
        let jp = add_type3_font(&mut doc, &mut fonts, &face, false, vec![glyph(1, 'あ')]);
        assert_eq!([he.as_str(), he1.as_str(), jp.as_str()], ["PCHe", "PCHe1", "PCJp"]);
        for name in [&he, &he1, &jp] {
            let font = fonts.get(name.as_bytes()).and_then(Object::as_dict).cloned().unwrap_or_default();
            assert!(is_type3_fallback(&font), "{name} isn't recognized as a generated fallback font");
        }
        // Someone else's Type 3 font is not ours.
        let mut other = Dict::new();
        other.set(b"Subtype".to_vec(), Object::name("Type3"));
        other.set(b"Name".to_vec(), Object::name("PCHebrewX"));
        assert!(!is_type3_fallback(&other));
    }
}
