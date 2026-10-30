use super::fallback::is_type3_fallback;
use super::rewrite::vertical_advance;
use super::{Dict, Document, EditError, HashMap, Matrix, Metrics, Object, Op, Rc};

/// Text and graphics state captured for rewriting.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TextState {
    pub(crate) font: Option<(String, f64)>,
    pub(crate) char_spacing: f64,
    pub(crate) word_spacing: f64,
    pub(crate) scale: f64,
    pub(super) leading: f64,
    pub(crate) rise: f64,
    pub(crate) fill: Vec<Op>,
    /// The text rendering mode (`Tr`); 3 is invisible text, as in an OCR layer over a scan.
    pub(crate) render_mode: i64,
    /// The stroke colour operators, the line width and the ExtGStates selected (see [`Ts`]):
    /// graphics state the text was painted with that [`TextState::ops`] doesn't restate.
    pub(super) stroke: Vec<Op>,
    pub(super) line_width: Option<f64>,
    ext_gstate: Vec<Vec<u8>>,
    /// The dash pattern and line cap in force (see [`Ts`]): compared, not restated.
    pub(super) dash: Dash,
    pub(super) line_cap: i64,
}

impl TextState {
    pub(super) fn of(ts: &Ts) -> Self {
        TextState {
            font: ts.font.as_ref().map(|(n, _)| (String::from_utf8_lossy(n).into_owned(), ts.size)),
            char_spacing: ts.char_spacing,
            word_spacing: ts.word_spacing,
            scale: ts.scale,
            leading: ts.leading,
            rise: ts.rise,
            fill: ts.fill.clone(),
            render_mode: ts.render_mode,
            stroke: ts.stroke.clone(),
            line_width: ts.line_width,
            ext_gstate: ts.ext_gstate.clone(),
            dash: ts.dash.clone(),
            line_cap: ts.line_cap,
        }
    }

    /// Operators that change how text is painted from `current` (the state where they run) to
    /// this one: the ExtGStates, stroke colour, line width and a visible rendering mode. A
    /// rewritten paragraph is drawn before its text object, so whatever that object set before
    /// its first show (`/GS1 gs`, `1 Tr 1 0 0 RG 2 w` inside `BT`, #3508/#3509) must be stated
    /// again. Empty when nothing differs.
    ///
    /// Only fill, stroke and fill-then-stroke (`Tr` 0–2) are restated: invisible text is revealed
    /// by [`reveal_ops`] and clipping text is refused, so neither is ever drawn this way.
    ///
    /// A `gs` has no inverse, so a selection `current` has and this state doesn't stays in force:
    /// run these inside a `q`/`Q` that starts from `current`, one per state drawn.
    pub(crate) fn paint_ops(&self, current: &TextState) -> Vec<Op> {
        let n = pdfcraft_content::num;
        let mut v = Vec::new();
        // The selections both share stay in force; the ones after them are selected again in order.
        let shared = self.ext_gstate.iter().zip(&current.ext_gstate).take_while(|(a, b)| a == b).count();
        for name in self.ext_gstate.iter().skip(shared) {
            v.push(Op::new("gs", vec![Object::Name(name.clone())]));
        }
        // A reselected ExtGState may set `/LW`, so the width (already the effective one) follows it.
        let reselected = !v.is_empty();
        if self.stroke != current.stroke {
            if self.stroke.is_empty() {
                // The initial stroke colour: black in DeviceGray.
                v.push(Op::new("G", vec![n(0.0)]));
            } else {
                v.extend(self.stroke.iter().cloned());
            }
        }
        let width = self.line_width.unwrap_or(1.0);
        if reselected || width != current.line_width.unwrap_or(1.0) {
            v.push(Op::new("w", vec![n(width)]));
        }
        if (0..=2).contains(&self.render_mode) && self.render_mode != current.render_mode {
            v.push(Op::new("Tr", vec![Object::Int(self.render_mode)]));
        }
        v
    }

    /// Operators that set this state (inside a text object).
    pub(crate) fn ops(&self) -> Vec<Op> {
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
pub(super) struct Paint {
    layers: Vec<Vec<u8>>,
    gs: Gs,
}

/// The transparency parameters of the graphics state: what the `gs` operators have set, resolved,
/// so a repeated `gs` or two names with the same settings leave it as it was.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Gs {
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
pub(super) struct Origin {
    pub(crate) tm: [f64; 6],
    /// The text line matrix there (where the next line's Td/T* starts from).
    pub(crate) tlm: [f64; 6],
    /// Text-space units → user space (the scale of the text rendering matrix's y axis).
    pub(crate) k: f64,
    /// The CTM there (current space → page space), for moving the paragraph in page space.
    pub(crate) ctm: [f64; 6],
    pub(crate) baseline: f64,
    pub(crate) x: f64,
    pub(crate) state: TextState,
    pub(crate) bt_op: usize,
    pub(crate) bt_state: TextState,
}

/// The unit x axis of a text rendering matrix in user space (`(1, 0)` when it is degenerate).
pub(super) fn direction(m: &Matrix) -> (f64, f64) {
    let (x, y) = (m.0[0], m.0[1]);
    let length = x.hypot(y);
    if length.is_finite() && length > 1e-9 { (x / length, y / length) } else { (1.0, 0.0) }
}

/// User-space point `p` as `(along, across)` a baseline running in unit direction `dir`. Upright
/// text (`dir` = `(1, 0)`) keeps its coordinates, so lines and rows of turned text are found the
/// same way (#1443).
pub(super) fn to_frame(dir: (f64, f64), p: (f64, f64)) -> (f64, f64) {
    (p.0 * dir.0 + p.1 * dir.1, p.1 * dir.0 - p.0 * dir.1)
}

/// The box covering both `[x0, y0, x1, y1]` boxes.
pub(crate) fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

#[derive(Clone)]
pub(super) struct Ts {
    pub(super) ctm: Matrix,
    /// The operators that set the current fill colour (`g`, `rg`, `k`, or `cs` + `sc`/`scn`).
    pub(super) fill: Vec<Op>,
    pub(super) font: Option<(Vec<u8>, Rc<Metrics>)>,
    /// The Japanese or Hebrew fallback face the font subsets, when the font is one (see [`is_type3_fallback`]).
    pub(super) face: Option<String>,
    pub(super) size: f64,
    pub(super) char_spacing: f64,
    pub(super) word_spacing: f64,
    pub(super) scale: f64,
    pub(super) leading: f64,
    pub(super) rise: f64,
    pub(super) render_mode: i64,
    /// The transparency parameters the `gs` operators have set so far (see [`Paint`]).
    pub(super) gs: Gs,
    /// The font's vertical advance per unit of font size (`/DW2`'s w1, normally −1) when its
    /// CMap writes vertically (`WMode 1`, §9.7.4.3): each glyph then moves the pen down, not right.
    pub(super) vertical: Option<f64>,
    /// Why the font's writing mode couldn't be read (a broken or cyclic CMap). Its text is then
    /// listed as horizontal, but the lines in it are refused for editing.
    pub(super) unreadable: Option<Rc<str>>,
    /// The operators that set the current stroke colour (`G`, `RG`, `K`, or `CS` + `SC`/`SCN`).
    pub(super) stroke: Vec<Op>,
    /// The line width a `w` or an ExtGState's `/LW` set (`None`: the initial 1.0).
    pub(super) line_width: Option<f64>,
    /// The ExtGState resources selected with `gs`, oldest first (see [`select_gs`]).
    pub(super) ext_gstate: Vec<Vec<u8>>,
    /// The dash pattern a `d` or an ExtGState's `/D` set (initially solid: `[] 0`).
    pub(super) dash: Dash,
    /// The line cap a `J` or an ExtGState's `/LC` set (initially 0, butt).
    pub(super) line_cap: i64,
}

/// A dash pattern (ISO 32000-2 §8.4.3.6): the dash array and the phase.
type Dash = (Vec<f64>, f64);

/// The most dash array entries a pattern keeps; a longer one is ignored, as is a malformed one.
const MAX_DASH: usize = 32;

/// The dash pattern `array phase`, as `d` takes it and an ExtGState's `/D` holds it.
fn dash_pattern(doc: &Document, array: Option<&Object>, phase: Option<&Object>) -> Option<Dash> {
    let array = doc.resolve(array?);
    let items = array.as_array().filter(|a| a.len() <= MAX_DASH)?;
    let finite = |o: &Object| doc.resolve(o).as_f64().filter(|v| v.is_finite());
    Some((items.iter().map(finite).collect::<Option<Vec<_>>>()?, finite(phase?)?))
}

/// A line cap style, as `J` takes it and an ExtGState's `/LC` holds it: 0, 1 or 2.
fn line_cap(doc: &Document, cap: Option<&Object>) -> Option<i64> {
    doc.resolve(cap?).as_int().filter(|c| (0..=2).contains(c))
}

/// The most `gs` selections a state remembers. Repeating a selection moves it to the end, so
/// this bounds the distinct ExtGStates in force, not the operators a hostile stream repeats.
const MAX_GS: usize = 32;

/// Record `gs /name` in `list`. A `gs` sets every parameter its dictionary names to an absolute
/// value, so a later selection of the same dictionary overrides the earlier one completely: the
/// earlier entry goes, which keeps replaying the list equivalent to replaying the stream.
fn select_gs(list: &mut Vec<Vec<u8>>, name: &[u8]) {
    list.retain(|n| n != name);
    if list.len() >= MAX_GS {
        list.remove(0);
    }
    list.push(name.to_vec());
}

/// The text-showing operators of one stream with their text, font, box and baseline.
pub(super) struct Shown {
    pub(super) op: usize,
    pub(super) extra_ops: Vec<usize>,
    /// Encoded glyphs, not bytes or Unicode characters (a ligature can represent several).
    pub(super) glyph_count: usize,
    /// Advance in text space, including spacing and TJ adjustments.
    pub(super) advance: f64,
    pub(super) tracking: f64,
    /// The text matrix where it starts (text space), and the state there and at its `BT`.
    pub(super) tm: Matrix,
    pub(super) tlm: Matrix,
    pub(super) state: Ts,
    pub(super) bt_op: usize,
    pub(super) bt_state: Ts,
    pub(super) text: String,
    pub(super) rect: [f64; 4],
    /// Its box along and across its baseline (see [`TextLine::frame`]).
    pub(super) frame: [f64; 4],
    /// The boxes of its words (see [`TextLine::ink`]).
    pub(super) ink: Vec<[f64; 4]>,
    pub(super) baseline: f64,
    pub(super) start_x: f64,
    end_x: f64,
    end_y: f64,
    pub(super) bt: usize,
    pub(super) font: Vec<u8>,
    pub(super) base_font: String,
    pub(super) face: Option<String>,
    pub(super) size: f64,
    pub(super) bold: bool,
    pub(super) italic: bool,
    pub(super) visual_order: bool,
    pub(super) decodable: bool,
    pub(super) scope: Option<usize>,
    pub(super) paint: Paint,
    pub(super) logical_order: bool,
    pub(super) actual_text: bool,
    /// Set when a `/PCSoft` or `/PCHard` marker was seen since the previous text line.
    pub(super) soft_break: bool,
    pub(super) hard_break: bool,
    /// The `/N` a `/PCSoft` marker carried: how many separator characters the wrap consumed (0
    /// for a Chinese/Japanese word split). Meaningless unless `soft_break` is set; 1 for a
    /// `/PCSoft` with no `/N` (a file saved before this was tracked, or by another writer),
    /// matching the single space a rejoin always used before #2529.
    pub(super) soft_gap: usize,
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
pub(super) struct Pen {
    pub(super) bt: usize,
    dir: (f64, f64),
    across: f64,
    end: f64,
    pub(super) size: f64,
    vertical: bool,
}

impl Pen {
    pub(super) fn after(s: &Shown) -> Self {
        let (dir, _, end, across) = s.span();
        Pen { bt: s.bt, dir, across, end, size: s.size.max(1.0), vertical: s.state.vertical.is_some() }
    }

    /// Whether `s` could continue the same line on geometry alone: on the same baseline, starting
    /// at most a word gap (`reach` sizes) after it. Also the gap along the baseline.
    pub(super) fn continues(&self, s: &Shown, reach: f64) -> (bool, f64) {
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
pub(super) struct Carry {
    pub(super) ts: Ts,
    /// Each saved state with the text and text line matrices: inside a text object `q`/`Q`
    /// save and restore them too (ISO 32000-2 §9.4.1, as corrected by the PDF Association's
    /// errata, issue 368), as the renderer does.
    pub(super) stack: Vec<(Ts, Matrix, Matrix)>,
    pub(super) marked: Vec<Option<usize>>,
    /// For each open marked-content sequence, its optional content group when it is `/OC`.
    pub(super) layers: Vec<Option<Vec<u8>>>,
    /// Bytes of glyph text the rest of the page may still produce (see [`MAX_PAGE_TEXT`]).
    pub(super) text_left: usize,
}

/// The most glyph text one page's extraction produces, in bytes. A ToUnicode destination is
/// copied for every glyph showing its code, so a small page can otherwise expand into gigabytes;
/// real pages hold far less (a whole novel is about 1 MB).
const MAX_PAGE_TEXT: usize = 16 << 20;

impl Carry {
    pub(super) fn new() -> Self {
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
            stroke: Vec::new(),
            line_width: None,
            ext_gstate: Vec::new(),
            dash: (Vec::new(), 0.0),
            line_cap: 0,
        };
        Carry { ts, stack: Vec::new(), marked: Vec::new(), layers: Vec::new(), text_left: MAX_PAGE_TEXT }
    }
}

/// A font resource's metrics, fallback face and vertical advance (see [`Ts`]), read once per page.
/// The advance is an error message when the font's writing mode can't be read.
type CachedFont = (Rc<Metrics>, Option<String>, Result<Option<f64>, String>);

/// A `Do` met while interpreting: its operator index, XObject name and the state there.
type Invocation = (usize, Vec<u8>, Ts);

#[derive(Default)]
struct MarkedContent {
    regions: Vec<Option<usize>>,
    layers: Vec<Option<Vec<u8>>>,
    actual_text: Vec<Option<(usize, usize, String)>>,
}

impl MarkedContent {
    fn begin(&mut self, doc: &Document, op: &Op, index: usize, logical_text: Option<&Dict>, shown_count: usize) {
        // Marked text paragraphs/headings are authoritative boundaries. Ignore artifacts and
        // inner spans, which inherit their enclosing region.
        let region = matches!(op.name(0), Some(b"P" | b"H" | b"H1" | b"H2" | b"H3" | b"H4" | b"H5" | b"H6" | b"LI" | b"TD" | b"TH"));
        self.regions.push(if region { Some(index) } else { self.regions.last().copied().flatten() });
        // A layer named in the resources is told apart by its name, an inline one by where it opens.
        let layer = op.is("BDC") && op.name(0) == Some(b"OC".as_slice());
        self.layers
            .push(layer.then(|| op.operands.get(1).and_then(Object::as_name).map_or_else(|| format!("@{index}").into_bytes(), <[u8]>::to_vec)));
        // The property list is inline or named in the resources' /Properties (§14.6.2).
        let list = logical_text.and_then(|properties| match op.operands.get(1) {
            Some(Object::Name(name)) => properties.get(name).and_then(|object| doc.dict(object)),
            other => other.and_then(Object::as_dict).cloned(),
        });
        let logical = list.as_ref().and_then(|dict| dict.get(b"ActualText")).and_then(|value| match value {
            Object::String(value) if value.bytes.len() <= 4_000_000 => Some(value.to_text()),
            _ => None,
        });
        self.actual_text.push(logical.map(|text| (shown_count, index, text)));
    }

    fn end(&mut self, index: usize, shown: &mut Vec<Shown>) {
        self.regions.pop();
        self.layers.pop();
        if let Some(Some((start, open, logical))) = self.actual_text.pop() {
            let tail = shown.split_off(start.min(shown.len()));
            let mut items = tail.into_iter();
            if let Some(mut first) = items.next() {
                for item in items {
                    first.glyph_count = first.glyph_count.saturating_add(item.glyph_count);
                    first.extra_ops.push(item.op);
                    first.extra_ops.extend(item.extra_ops);
                    first.rect = [
                        first.rect[0].min(item.rect[0]),
                        first.rect[1].min(item.rect[1]),
                        first.rect[2].max(item.rect[2]),
                        first.rect[3].max(item.rect[3]),
                    ];
                    first.frame = union(first.frame, item.frame);
                    first.ink.extend(item.ink);
                }
                first.extra_ops.extend([open, index]);
                // Only right-to-left ActualText is already in typing order and must stay out of
                // bidi recovery. Word and Chrome also wrap Latin ligatures mid-word.
                first.logical_order = logical.chars().any(crate::bidi::is_rtl);
                first.actual_text = true;
                first.text = logical;
                first.start_x = first.rect[0];
                first.end_x = first.rect[2];
                first.end_y = first.baseline;
                first.advance = (first.end_x - first.start_x) / first.tm.then(&first.state.ctm).0[0].abs().max(1e-6);
                shown.push(first);
            }
        }
    }
}

struct TextRunState<'a> {
    ts: &'a mut Ts,
    tm: &'a mut Matrix,
    tlm: &'a mut Matrix,
    at_bt: &'a (usize, Ts),
    bt: usize,
    marked: &'a MarkedContent,
    carry: &'a mut Carry,
    out: &'a mut Vec<Shown>,
    soft_break: bool,
    hard_break: bool,
    soft_gap: usize,
}

fn show_text(op: &Op, i: usize, state: &mut TextRunState<'_>) -> Result<(), EditError> {
    let ts = &mut *state.ts;
    let tm = &mut *state.tm;
    let tlm = &mut *state.tlm;
    let at_bt = state.at_bt;
    let bt = state.bt;
    let marked = state.marked;
    let carry = &mut *state.carry;
    let out = &mut *state.out;
    if matches!(op.op.as_slice(), b"'" | b"\"") {
        if op.is("\"") {
            ts.word_spacing = op.num(0).unwrap_or(ts.word_spacing);
            ts.char_spacing = op.num(1).unwrap_or(ts.char_spacing);
        }
        *tlm = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, -ts.leading]).then(tlm);
        *tm = *tlm;
    }
    let Some((name, m)) = ts.font.clone() else { return Ok(()) };
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
                    let blank = m.is_space(code, len) || shown.as_deref().is_some_and(|t| !t.is_empty() && t.chars().all(char::is_whitespace));
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
    *tm = Matrix([1.0, 0.0, 0.0, 1.0, x_text, y_text]).then(tm);
    let end = tm.then(&ts.ctm).apply(0.0, ts.rise);
    // The box from `a` to `b` along the run: from descent to ascent across it, or for
    // vertical text, the widest glyph's width centred on the pen.
    let span = |a: f64, b: f64| match vertical {
        Some(_) => [trm0.apply(-half, ts.rise + a), trm0.apply(half, ts.rise + a), trm0.apply(-half, ts.rise + b), trm0.apply(half, ts.rise + b)],
        None => [
            trm0.apply(a, ts.rise + m.descent * ts.size),
            trm0.apply(b, ts.rise + m.descent * ts.size),
            trm0.apply(a, ts.rise + m.ascent * ts.size),
            trm0.apply(b, ts.rise + m.ascent * ts.size),
        ],
    };
    let corners = span(0.0, if vertical.is_some() { y_text } else { x_text });
    let rect = corners.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)]);
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
            span(a, b).iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |r, p| [r[0].min(p.0), r[1].min(p.1), r[2].max(p.0), r[3].max(p.1)])
        })
        .collect();
    // A negative `Tf` size mirrors the glyphs; some producers pair it with a flipped CTM so the
    // text reads upright. The size the line is set in is the magnitude either way.
    let size_user = (trm0.0[2].powi(2) + trm0.0[3].powi(2)).sqrt() * ts.size.abs();
    // A numeric-only TJ adjusts the current text matrix but paints no glyphs.
    // It must not create a phantom editable line or enlarge its neighbours' boxes.
    if !pieces.iter().any(|p| matches!(p, Object::String(s) if !s.bytes.is_empty())) {
        return Ok(());
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
        tlm: *tlm,
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
        size: if size_user > 0.0 { size_user } else { ts.size.abs() },
        bold: m.bold,
        italic: m.italic,
        visual_order: m.visual_order,
        decodable,
        scope: marked.regions.last().copied().flatten(),
        paint: Paint { layers: marked.layers.iter().flatten().cloned().collect(), gs: ts.gs.clone() },
        logical_order: false,
        actual_text: false,
        soft_break: state.soft_break,
        hard_break: state.hard_break,
        soft_gap: state.soft_gap,
    });
    Ok(())
}

pub(super) fn interpret(
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
    let mut stack: Vec<(Ts, Matrix, Matrix)> = std::mem::take(&mut carry.stack);
    let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
    let mut bt = 0usize;
    let mut marked =
        MarkedContent { regions: std::mem::take(&mut carry.marked), layers: std::mem::take(&mut carry.layers), ..MarkedContent::default() };
    // A rewrite names a wrap (`/PCSoft`) or a break it kept (`/PCHard`) in an empty marker
    // before the next line. Neither name is a paragraph region, so it does not change scope.
    let mut soft_break = false;
    let mut hard_break = false;
    // `/PCSoft`'s `/N` property: how many separator characters the wrap consumed. A `/PCSoft`
    // with no properties dict (a file saved before this was tracked, or by another writer) reads
    // as 1, the single space a rejoin always used before #2529.
    let mut soft_gap = 1usize;
    for (i, op) in ops.iter().enumerate() {
        match op.op.as_slice() {
            // No depth cap: every `EMC` pops, so a capped push would pair it with the enclosing
            // sequence (#3573). Both stacks stay bounded by the number of operators.
            b"BMC" | b"BDC" => {
                if op.name(0) == Some(b"PCSoft") {
                    soft_break = true;
                    hard_break = false;
                    soft_gap = op
                        .operands
                        .get(1)
                        .and_then(Object::as_dict)
                        .and_then(|d| d.get(b"N"))
                        .and_then(Object::as_int)
                        .and_then(|n| usize::try_from(n).ok())
                        .unwrap_or(1);
                    // The marker is untrusted and later expands into spaces during reflow.
                    // Charge it to the same page-wide budget as glyph text, so many individually
                    // small markers cannot multiply the allocation past the limit either.
                    carry.text_left =
                        carry.text_left.checked_sub(soft_gap).ok_or_else(|| EditError::Invalid("the page's text is too large to read".into()))?;
                } else if op.name(0) == Some(b"PCHard") {
                    hard_break = true;
                    soft_break = false;
                }
                marked.begin(doc, op, i, logical_text, out.len());
            }
            b"EMC" => {
                marked.end(i, &mut out);
            }
            b"q" => stack.push((ts.clone(), tm, tlm)),
            b"Q" => {
                if let Some(saved) = stack.pop() {
                    (ts, tm, tlm) = saved;
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
            b"G" | b"RG" | b"K" | b"CS" => ts.stroke = vec![op.clone()],
            b"SC" | b"SCN" => {
                ts.stroke.retain(|o| o.is("CS"));
                ts.stroke.push(op.clone());
            }
            b"w" => ts.line_width = op.num(0).filter(|w| w.is_finite()).or(ts.line_width),
            b"d" => ts.dash = dash_pattern(doc, op.operands.first(), op.operands.get(1)).unwrap_or_else(|| ts.dash.clone()),
            b"J" => ts.line_cap = line_cap(doc, op.operands.first()).unwrap_or(ts.line_cap),
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
                if let Some(name) = op.name(0) {
                    select_gs(&mut ts.ext_gstate, name);
                }
                let params = op.name(0).and_then(|name| states.get(name)).map(|p| doc.resolve(p));
                if let Some(params) = params.as_deref().and_then(Object::as_dict) {
                    ts.gs.apply(doc, params);
                    // `/LW` sets the line width as `w` does (ISO 32000-2 §8.4.5).
                    let lw = params.get(b"LW").and_then(|w| doc.resolve(w).as_f64()).filter(|w| w.is_finite());
                    ts.line_width = lw.or(ts.line_width);
                    let dash = params.get(b"D").map(|d| doc.resolve(d));
                    let dash = dash.as_deref().and_then(Object::as_array);
                    if let Some(d) = dash.and_then(|d| dash_pattern(doc, d.first(), d.get(1))) {
                        ts.dash = d;
                    }
                    ts.line_cap = line_cap(doc, params.get(b"LC")).unwrap_or(ts.line_cap);
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
                let before = out.len();
                let mut state = TextRunState {
                    ts: &mut ts,
                    tm: &mut tm,
                    tlm: &mut tlm,
                    at_bt: &at_bt,
                    bt,
                    marked: &marked,
                    carry,
                    out: &mut out,
                    soft_break,
                    hard_break,
                    soft_gap,
                };
                show_text(op, i, &mut state)?;
                // A numeric-only TJ adjustment shows no glyphs and pushes nothing: the break
                // belongs to whichever text-showing operator comes next.
                if out.len() > before {
                    soft_break = false;
                    hard_break = false;
                    soft_gap = 1;
                }
            }
            _ => {}
        }
    }
    carry.ts = ts;
    carry.stack = stack;
    carry.marked = marked.regions;
    carry.layers = marked.layers;
    positioned_tracking(&mut out);
    Ok(out)
}
