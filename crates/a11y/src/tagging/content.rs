//! A page's content as the pieces tagging works with (ISO 32000-2 §8.2, §9.4, §14.6).
//!
//! The content is read into *atoms*, the smallest things a tag can own: one text-showing
//! operator, one path object that paints, one image or form XObject, one inline image, one
//! shading. Marked-content sequences group them. A *unit* is what the Reading Order tool and the
//! Tags panel select: a tagged or artifact sequence as a whole, or an atom outside both.
//!
//! Rewrites never re-serialize the page: they splice new bytes in at operator boundaries
//! ([`pdfcraft_content::Pieces::splice`]) and copy everything else byte for byte, so the page
//! draws exactly as before.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use pdfcraft_content::{Matrix, Op, Pieces};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};
use pdfcraft_fonts::pdf::Metrics;

/// The most decoded content a page may have for tagging to rewrite it.
const MAX_PAGE_BYTES: usize = 64 << 20;
/// The most operators and atoms a page is read with.
const MAX_OPS: usize = 2_000_000;
const MAX_ATOMS: usize = 200_000;
/// How deep `q` saves the state (a hostile stream can nest millions).
const MAX_SAVED: usize = 256;
/// How much text a unit keeps for previews.
const MAX_TEXT: usize = 300;

/// What a unit draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnitKind {
    Text,
    Image,
    /// Paths, shadings and form XObjects.
    Graphic,
}

/// What a marked-content sequence is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mark {
    /// Structure content with this MCID.
    Tagged(i64),
    Artifact,
    /// Anything else (optional content, a span with actual text but no MCID…).
    Other,
}

#[derive(Clone, Debug)]
pub(crate) struct Sequence {
    /// The `BMC`/`BDC` operator and its `EMC`.
    pub open: usize,
    pub close: Option<usize>,
    pub mark: Mark,
    pub parent: Option<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct Atom {
    pub first: usize,
    pub last: usize,
    pub kind: UnitKind,
    pub bbox: Option<[f64; 4]>,
    pub text: String,
    /// The innermost marked-content sequence it is in.
    pub sequence: Option<usize>,
    /// The text object (its `BT`) it is in.
    pub text_object: Option<usize>,
    /// A form XObject that carries structure of its own (`/StructParent(s)`).
    pub structured: bool,
}

/// Where a unit comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Sequence(usize),
    Atom(usize),
}

#[derive(Clone, Debug)]
pub(crate) struct Unit {
    pub source: Source,
    /// The first operator (where it starts in content order).
    pub first: usize,
    pub kind: UnitKind,
    pub bbox: Option<[f64; 4]>,
    pub text: String,
    /// Atoms it covers.
    pub atoms: Vec<usize>,
    /// A tagged sequence inside it (it can't be retagged or made an artifact as a whole).
    pub nested: bool,
}

/// A page's content, read for tagging.
#[derive(Clone, Debug)]
pub(crate) struct PageContent {
    pub page: ObjRef,
    /// The `/Contents` entries (indirect or direct streams).
    entries: Vec<Object>,
    /// Each entry's decoded bytes.
    decoded: Vec<Vec<u8>>,
    pieces: Pieces,
    pub ops: Vec<Op>,
    pub sequences: Vec<Sequence>,
    pub atoms: Vec<Atom>,
    /// Per operator: marked-content depth and `q` depth before it, and its text object.
    mc_depth: Vec<u32>,
    q_depth: Vec<u32>,
    text_object: Vec<Option<usize>>,
    /// Per text object (`BT` index): its `ET`, and whether marked content is inside it.
    text_end: HashMap<usize, usize>,
    text_marked: HashSet<usize>,
    /// The greatest MCID the page's content uses.
    pub max_mcid: Option<i64>,
    /// How many sequences use each MCID.
    pub mcid_uses: HashMap<i64, usize>,
    /// Why the content can't be rewritten safely.
    pub read_only: Option<String>,
}

fn dict_of(doc: &Document, d: &Dict, key: &[u8]) -> Dict {
    d.get(key).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).unwrap_or_default()
}

/// The page's content entries: one stream, or the elements of a contents array.
fn entries_of(doc: &Document, page: &Dict) -> Result<Vec<Object>, String> {
    let Some(c) = page.get(b"Contents") else { return Ok(Vec::new()) };
    match &*doc.resolve(c) {
        Object::Array(items) => Ok(items.clone()),
        Object::Stream(_) => Ok(vec![c.clone()]),
        Object::Null => Ok(Vec::new()),
        _ => Err("its contents are not a stream or an array of streams".into()),
    }
}

/// Decode a stream strictly within `max` bytes.
fn decode_strict(stream: &Stream, max: usize) -> Result<Vec<u8>, String> {
    let chain = stream.filters();
    if chain.is_empty() {
        if stream.raw.len() > max {
            return Err("its content is too large".into());
        }
        return Ok(stream.raw.to_vec());
    }
    pdfcraft_filters::decode(&chain, &stream.raw, max).map_err(|e| format!("its content can't be decoded exactly ({e})"))
}

#[derive(Clone)]
struct State {
    ctm: Matrix,
    font: Option<Vec<u8>>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    scale: f64,
    leading: f64,
    rise: f64,
}

impl Default for State {
    fn default() -> Self {
        State { ctm: Matrix::IDENTITY, font: None, size: 0.0, char_spacing: 0.0, word_spacing: 0.0, scale: 1.0, leading: 0.0, rise: 0.0 }
    }
}

fn union(a: Option<[f64; 4]>, b: [f64; 4]) -> Option<[f64; 4]> {
    if !b.iter().all(|v| v.is_finite()) {
        return a;
    }
    Some(match a {
        Some(a) => [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])],
        None => b,
    })
}

fn push_text(text: &mut String, piece: &str) {
    if text.len() >= MAX_TEXT || piece.is_empty() {
        return;
    }
    if !text.is_empty() && !text.ends_with(' ') && !piece.starts_with(' ') {
        text.push(' ');
    }
    for c in piece.chars() {
        if text.len() >= MAX_TEXT {
            break;
        }
        text.push(c);
    }
}

impl PageContent {
    /// Read a page's content. `crop` is where a shading paints (its clip is not tracked).
    pub fn read(doc: &Document, page: &pdfcraft_model::Page) -> PageContent {
        let mut read_only = None;
        let entries = entries_of(doc, &page.dict).unwrap_or_else(|e| {
            read_only = Some(e);
            Vec::new()
        });
        let mut decoded = Vec::with_capacity(entries.len());
        let mut total = 0usize;
        for entry in &entries {
            let resolved = doc.resolve(entry);
            let Object::Stream(stream) = &*resolved else {
                read_only.get_or_insert_with(|| "its contents include something other than a stream".into());
                decoded.push(Vec::new());
                continue;
            };
            let bytes = match decode_strict(stream, MAX_PAGE_BYTES.saturating_sub(total)) {
                Ok(b) => b,
                Err(e) => {
                    read_only.get_or_insert(e);
                    stream.decoded_within(MAX_PAGE_BYTES.saturating_sub(total)).unwrap_or_default()
                }
            };
            total = total.saturating_add(bytes.len()).saturating_add(1);
            decoded.push(bytes);
        }
        let pieces = Pieces::join(&decoded);
        let parsed = pieces.parse();
        if parsed.skipped > 0 {
            read_only.get_or_insert_with(|| "its content has bytes that are not operators".into());
        }
        let mut ops = parsed.ops;
        if ops.len() > MAX_OPS {
            read_only.get_or_insert_with(|| "its content has too many operators".into());
            ops.truncate(MAX_OPS);
        }
        let mut content = PageContent {
            page: page.obj,
            entries,
            decoded,
            pieces,
            ops: Vec::new(),
            sequences: Vec::new(),
            atoms: Vec::new(),
            mc_depth: Vec::new(),
            q_depth: Vec::new(),
            text_object: Vec::new(),
            text_end: HashMap::new(),
            text_marked: HashSet::new(),
            max_mcid: None,
            mcid_uses: HashMap::new(),
            read_only,
        };
        content.interpret(doc, page, ops);
        content
    }

    fn interpret(&mut self, doc: &Document, page: &pdfcraft_model::Page, ops: Vec<Op>) {
        let resources = dict_of(doc, &page.dict, b"Resources");
        let fonts = dict_of(doc, &resources, b"Font");
        let xobjects = dict_of(doc, &resources, b"XObject");
        let properties = dict_of(doc, &resources, b"Properties");
        let crop = page.crop(doc);
        let mut metrics: HashMap<Vec<u8>, Metrics> = HashMap::new();
        let mut state = State::default();
        let mut saved: Vec<State> = Vec::new();
        let mut unsaved = 0usize;
        let (mut tm, mut tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
        let mut q_depth = 0u32;
        let mut open: Vec<usize> = Vec::new();
        let mut text_object: Option<usize> = None;
        let mut path_start: Option<usize> = None;
        let mut path_box: Option<[f64; 4]> = None;
        let n = ops.len();
        self.mc_depth.reserve(n.saturating_add(1));
        self.q_depth.reserve(n.saturating_add(1));
        self.text_object.reserve(n.saturating_add(1));
        for (i, op) in ops.iter().enumerate() {
            self.mc_depth.push(u32::try_from(open.len()).unwrap_or(u32::MAX));
            self.q_depth.push(q_depth);
            self.text_object.push(text_object);
            let name = op.op.as_slice();
            let marked = matches!(name, b"BMC" | b"BDC" | b"EMC");
            if marked {
                if path_start.is_some() {
                    self.read_only.get_or_insert_with(|| "marked content starts or ends inside a path".into());
                }
                if let Some(bt) = text_object {
                    self.text_marked.insert(bt);
                }
            }
            match name {
                b"q" => {
                    q_depth = q_depth.saturating_add(1);
                    if saved.len() < MAX_SAVED {
                        saved.push(state.clone());
                    } else {
                        unsaved = unsaved.saturating_add(1);
                    }
                }
                b"Q" => {
                    q_depth = q_depth.saturating_sub(1);
                    if unsaved > 0 {
                        unsaved -= 1;
                    } else if let Some(s) = saved.pop() {
                        state = s;
                    }
                }
                b"cm" => {
                    if let Some(m) = op.nums::<6>() {
                        state.ctm = Matrix(m).then(&state.ctm);
                    }
                }
                b"BT" => {
                    if text_object.is_some() {
                        self.read_only.get_or_insert_with(|| "a text object starts inside another".into());
                    }
                    text_object = Some(i);
                    (tm, tlm) = (Matrix::IDENTITY, Matrix::IDENTITY);
                }
                b"ET" => {
                    if let Some(bt) = text_object.take() {
                        self.text_end.insert(bt, i);
                    }
                }
                b"Tf" => {
                    state.size = op.num(1).unwrap_or(state.size);
                    state.font = op.name(0).map(<[u8]>::to_vec);
                    if let Some(f) = op.name(0)
                        && !metrics.contains_key(f)
                    {
                        let m = fonts.get(f).map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned()).map(|d| Metrics::from_dict(doc, &d));
                        metrics.insert(f.to_vec(), m.unwrap_or_else(Metrics::fallback));
                    }
                }
                b"Tc" => state.char_spacing = op.num(0).unwrap_or(0.0),
                b"Tw" => state.word_spacing = op.num(0).unwrap_or(0.0),
                b"Tz" => state.scale = op.num(0).unwrap_or(100.0) / 100.0,
                b"TL" => state.leading = op.num(0).unwrap_or(0.0),
                b"Ts" => state.rise = op.num(0).unwrap_or(0.0),
                b"Td" | b"TD" => {
                    if let Some([x, y]) = op.nums::<2>() {
                        if name == b"TD" {
                            state.leading = -y;
                        }
                        tlm = Matrix::translate(x, y).then(&tlm);
                        tm = tlm;
                    }
                }
                b"Tm" => {
                    if let Some(m) = op.nums::<6>() {
                        (tm, tlm) = (Matrix(m), Matrix(m));
                    }
                }
                b"T*" => {
                    tlm = Matrix::translate(0.0, -state.leading).then(&tlm);
                    tm = tlm;
                }
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    if name != b"Tj" && name != b"TJ" {
                        if name == b"\"" {
                            state.word_spacing = op.num(0).unwrap_or(state.word_spacing);
                            state.char_spacing = op.num(1).unwrap_or(state.char_spacing);
                        }
                        tlm = Matrix::translate(0.0, -state.leading).then(&tlm);
                        tm = tlm;
                    }
                    let fallback = Metrics::fallback();
                    let m = state.font.as_ref().and_then(|f| metrics.get(f)).unwrap_or(&fallback);
                    let (bbox, text) = show(op, m, &state, &mut tm);
                    self.atom(i, i, UnitKind::Text, bbox, text, &open, text_object, false);
                }
                b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" => {
                    path_start.get_or_insert(i);
                    let points: Vec<(f64, f64)> = match name {
                        b"re" => op.nums::<4>().map(|[x, y, w, h]| vec![(x, y), (x + w, y + h)]).unwrap_or_default(),
                        b"h" => Vec::new(),
                        _ => {
                            let nums: Vec<f64> = op.operands.iter().filter_map(Object::as_f64).collect();
                            nums.as_chunks::<2>().0.iter().map(|[x, y]| (*x, *y)).collect()
                        }
                    };
                    for (x, y) in points {
                        let (x, y) = state.ctm.apply(x, y);
                        path_box = union(path_box, [x, y, x, y]);
                    }
                }
                b"W" | b"W*" => {}
                b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                    let first = path_start.take().unwrap_or(i);
                    self.atom(first, i, UnitKind::Graphic, path_box.take(), String::new(), &open, text_object, false);
                }
                b"n" => {
                    path_start = None;
                    path_box = None;
                }
                b"Do" => {
                    let target = op.name(0).and_then(|n| xobjects.get(n)).and_then(Object::as_ref);
                    let (kind, bbox, structured) = match target.map(|r| doc.get(r)) {
                        Some(x) => match &*x {
                            Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Form") => {
                                let bb: Vec<f64> = s
                                    .dict
                                    .get(b"BBox")
                                    .and_then(|b| doc.resolve(b).as_array().map(|a| a.iter().filter_map(Object::as_f64).collect()))
                                    .unwrap_or_default();
                                let fm = s
                                    .dict
                                    .get(b"Matrix")
                                    .and_then(|m| doc.resolve(m).as_array().and_then(|a| Matrix::from_operands(a)))
                                    .unwrap_or(Matrix::IDENTITY);
                                let bbox = match bb[..] {
                                    [a, b, c, d] => Some(fm.then(&state.ctm).bbox([a.min(c), b.min(d), a.max(c), b.max(d)])),
                                    _ => None,
                                };
                                (UnitKind::Graphic, bbox, s.dict.contains(b"StructParents") || s.dict.contains(b"StructParent"))
                            }
                            _ => (UnitKind::Image, Some(state.ctm.bbox([0.0, 0.0, 1.0, 1.0])), false),
                        },
                        None => (UnitKind::Graphic, None, false),
                    };
                    self.atom(i, i, kind, bbox, String::new(), &open, text_object, structured);
                }
                b"BI" => self.atom(i, i, UnitKind::Image, Some(state.ctm.bbox([0.0, 0.0, 1.0, 1.0])), String::new(), &open, text_object, false),
                b"sh" => self.atom(i, i, UnitKind::Graphic, Some(crop), String::new(), &open, text_object, false),
                b"BMC" | b"BDC" => {
                    let tag = op.name(0);
                    let mcid = if name == b"BDC" {
                        match op.operands.get(1) {
                            Some(Object::Dict(d)) => d.get(b"MCID").and_then(Object::as_int),
                            Some(Object::Name(n)) => properties
                                .get(n)
                                .map(|x| doc.resolve(x))
                                .and_then(|x| x.as_dict().and_then(|d| d.get(b"MCID").and_then(Object::as_int))),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let mark = match (tag, mcid) {
                        (Some(b"Artifact"), _) => Mark::Artifact,
                        (_, Some(m)) => Mark::Tagged(m),
                        _ => Mark::Other,
                    };
                    if let Mark::Tagged(m) = mark {
                        self.max_mcid = Some(self.max_mcid.map_or(m, |x| x.max(m)));
                        *self.mcid_uses.entry(m).or_default() += 1;
                    }
                    self.sequences.push(Sequence { open: i, close: None, mark, parent: open.last().copied() });
                    open.push(self.sequences.len().saturating_sub(1));
                }
                b"EMC" => match open.pop() {
                    Some(s) => {
                        if let Some(seq) = self.sequences.get_mut(s) {
                            seq.close = Some(i);
                        }
                    }
                    None => {
                        self.read_only.get_or_insert_with(|| "it closes marked content it never opened".into());
                    }
                },
                _ => {}
            }
        }
        self.mc_depth.push(u32::try_from(open.len()).unwrap_or(u32::MAX));
        self.q_depth.push(q_depth);
        self.text_object.push(text_object);
        if !open.is_empty() {
            self.read_only.get_or_insert_with(|| "marked content is left open".into());
        }
        if text_object.is_some() {
            self.read_only.get_or_insert_with(|| "a text object is left open".into());
        }
        if self.atoms.len() >= MAX_ATOMS {
            self.read_only.get_or_insert_with(|| "it draws too many objects".into());
        }
        self.ops = ops;
    }

    #[allow(clippy::too_many_arguments)]
    fn atom(
        &mut self,
        first: usize,
        last: usize,
        kind: UnitKind,
        bbox: Option<[f64; 4]>,
        text: String,
        open: &[usize],
        text_object: Option<usize>,
        structured: bool,
    ) {
        if self.atoms.len() >= MAX_ATOMS {
            return;
        }
        let bbox = bbox.filter(|b| b.iter().all(|v| v.is_finite()));
        self.atoms.push(Atom { first, last, kind, bbox, text, sequence: open.last().copied(), text_object, structured });
    }

    fn structural(&self, s: usize) -> bool {
        self.sequences.get(s).is_some_and(|q| matches!(q.mark, Mark::Tagged(_) | Mark::Artifact))
    }

    /// For each sequence, the outermost tagged or artifact sequence it is in (itself included).
    fn outermost(&self) -> Vec<Option<usize>> {
        let mut out: Vec<Option<usize>> = Vec::with_capacity(self.sequences.len());
        for (s, seq) in self.sequences.iter().enumerate() {
            // A parent always comes first, so its answer is known.
            let above = seq.parent.and_then(|p| out.get(p).copied().flatten());
            out.push(above.or(self.structural(s).then_some(s)));
        }
        out
    }

    /// The units, in content order.
    pub fn units(&self) -> Vec<Unit> {
        let outer = self.outermost();
        let mut units: Vec<Unit> = Vec::new();
        let mut by_sequence: HashMap<usize, usize> = HashMap::new();
        for (s, seq) in self.sequences.iter().enumerate() {
            if outer.get(s).copied().flatten() == Some(s) {
                by_sequence.insert(s, units.len());
                units.push(Unit {
                    source: Source::Sequence(s),
                    first: seq.open,
                    kind: UnitKind::Graphic,
                    bbox: None,
                    text: String::new(),
                    atoms: Vec::new(),
                    nested: false,
                });
            }
        }
        for (s, seq) in self.sequences.iter().enumerate() {
            if let Some(top) = outer.get(s).copied().flatten()
                && top != s
                && matches!(seq.mark, Mark::Tagged(_))
                && let Some(u) = by_sequence.get(&top).and_then(|&u| units.get_mut(u))
            {
                u.nested = true;
            }
        }
        let mut kinds: HashMap<usize, HashSet<UnitKind>> = HashMap::new();
        for (a, atom) in self.atoms.iter().enumerate() {
            match atom.sequence.and_then(|s| outer.get(s).copied().flatten()) {
                Some(top) => {
                    if let Some(&u) = by_sequence.get(&top)
                        && let Some(unit) = units.get_mut(u)
                    {
                        unit.atoms.push(a);
                        if let Some(b) = atom.bbox {
                            unit.bbox = union(unit.bbox, b);
                        }
                        push_text(&mut unit.text, &atom.text);
                        kinds.entry(u).or_default().insert(atom.kind);
                    }
                }
                None => units.push(Unit {
                    source: Source::Atom(a),
                    first: atom.first,
                    kind: atom.kind,
                    bbox: atom.bbox,
                    text: atom.text.clone(),
                    atoms: vec![a],
                    nested: false,
                }),
            }
        }
        for (u, set) in kinds {
            if let Some(unit) = units.get_mut(u) {
                unit.kind = if set.contains(&UnitKind::Text) {
                    UnitKind::Text
                } else if set.contains(&UnitKind::Image) {
                    UnitKind::Image
                } else {
                    UnitKind::Graphic
                };
            }
        }
        units.sort_by_key(|u| u.first);
        units
    }

    /// The operator ranges that wrap `atoms` (untagged atoms, in content order) in as few
    /// marked-content sequences as the content's nesting allows. A text object whose every
    /// atom is chosen is wrapped whole, outside its `BT`/`ET`.
    pub fn runs(&self, atoms: &[usize]) -> Result<Vec<(usize, usize)>, String> {
        let chosen: HashSet<usize> = atoms.iter().copied().collect();
        let mut by_text: HashMap<usize, Vec<usize>> = HashMap::new();
        for (a, atom) in self.atoms.iter().enumerate() {
            if let Some(bt) = atom.text_object {
                by_text.entry(bt).or_default().push(a);
            }
        }
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let mut whole: HashSet<usize> = HashSet::new();
        for &a in atoms {
            let Some(atom) = self.atoms.get(a) else { return Err("an object is not on the page".into()) };
            match atom.text_object {
                Some(bt)
                    if !self.text_marked.contains(&bt)
                        && self.text_end.get(&bt).is_some_and(|&et| self.balanced(bt, et))
                        && by_text.get(&bt).is_some_and(|all| all.iter().all(|x| chosen.contains(x))) =>
                {
                    if whole.insert(bt)
                        && let Some(&et) = self.text_end.get(&bt)
                    {
                        spans.push((bt, et));
                    }
                }
                _ => spans.push((atom.first, atom.last)),
            }
        }
        spans.sort_unstable();
        spans.dedup();
        let atom_at: HashMap<usize, usize> =
            self.atoms.iter().enumerate().flat_map(|(a, atom)| (atom.first..=atom.last).map(move |op| (op, a))).collect();
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for (first, last) in spans {
            if !self.balanced(first, last) {
                return Err("an object's graphics state is not saved and restored around it".into());
            }
            if let Some(run) = runs.last_mut()
                && self.joinable(run.0, run.1, first, last, &chosen, &atom_at)
            {
                run.1 = last;
                continue;
            }
            runs.push((first, last));
        }
        Ok(runs)
    }

    fn depth_before(&self, op: usize) -> u32 {
        self.q_depth.get(op).copied().unwrap_or(0)
    }

    /// Whether wrapping ops `first..=last` keeps `q`/`Q` nesting intact.
    fn balanced(&self, first: usize, last: usize) -> bool {
        let start = self.depth_before(first);
        let end = last.saturating_add(1);
        (first..=end).all(|i| self.depth_before(i) >= start) && self.depth_before(end) == start
    }

    /// Whether run `s..=e` can grow to `last`, taking in the span `first..=last` after it. A run
    /// is balanced and holds whole text objects already, so only what it would gain is checked:
    /// each join costs the operators it adds, not the whole run again.
    fn joinable(&self, s: usize, e: usize, first: usize, last: usize, chosen: &HashSet<usize>, atom_at: &HashMap<usize, usize>) -> bool {
        if self.text_object.get(s) != self.text_object.get(first) {
            return false;
        }
        for i in e.saturating_add(1)..first {
            let Some(op) = self.ops.get(i) else { return false };
            if matches!(op.op.as_slice(), b"BMC" | b"BDC" | b"EMC") {
                return false;
            }
            if atom_at.get(&i).is_some_and(|a| !chosen.contains(a)) {
                return false;
            }
        }
        // A text object in what the run gains must be inside it whole.
        if self.text_object.get(s).copied().flatten().is_none()
            && (e.saturating_add(1)..=last)
                .any(|i| self.text_object.get(i).copied().flatten().is_some_and(|bt| bt < s || self.text_end.get(&bt).is_none_or(|&et| et > last)))
        {
            return false;
        }
        // `s..=e` is balanced, so `s..=last` is when what it gains is.
        self.balanced(e.saturating_add(1), last)
    }

    /// Bytes inserted before an operator.
    pub fn before(&self, op: usize, bytes: Vec<u8>) -> Option<(Range<usize>, Vec<u8>)> {
        let at = self.ops.get(op)?.span.start;
        Some((at..at, bytes))
    }

    /// Bytes inserted after an operator.
    pub fn after(&self, op: usize, bytes: Vec<u8>) -> Option<(Range<usize>, Vec<u8>)> {
        let at = self.ops.get(op)?.span.end;
        Some((at..at, bytes))
    }

    /// An operator replaced by bytes.
    pub fn replace(&self, op: usize, bytes: Vec<u8>) -> Option<(Range<usize>, Vec<u8>)> {
        Some((self.ops.get(op)?.span.clone(), bytes))
    }

    /// Write the spliced content back. `uses` counts how many times each content stream object
    /// is listed by any page: a stream another page (or this one twice) also draws is copied
    /// rather than changed.
    pub fn commit(&self, doc: &mut Document, mut edits: Vec<(Range<usize>, Vec<u8>)>, uses: &HashMap<ObjRef, usize>) -> Result<(), String> {
        if edits.is_empty() {
            return Ok(());
        }
        if let Some(why) = &self.read_only {
            return Err(why.clone());
        }
        edits.sort_by_key(|(r, _)| (r.start, r.end));
        for pair in edits.windows(2) {
            if let [(a, _), (b, _)] = pair
                && b.start < a.end
            {
                return Err("two changes to the page content overlap".into());
            }
        }
        let rebuilt = self.pieces.splice(edits);
        let mut entries = self.entries.clone();
        let mut relisted = false;
        for (i, bytes) in rebuilt.into_iter().enumerate() {
            if self.decoded.get(i).is_some_and(|old| *old == bytes) {
                continue;
            }
            let Some(entry) = entries.get_mut(i) else { return Err("the page content changed while it was edited".into()) };
            let original = doc.resolve(entry);
            let Object::Stream(stream) = &*original else { return Err("the page content changed while it was edited".into()) };
            let mut dict = stream.dict.clone();
            for key in [&b"Length"[..], b"Filter", b"DecodeParms", b"DL"] {
                dict.remove(key);
            }
            let stream = Object::Stream(Stream::flate(dict, &bytes));
            match entry.as_ref() {
                Some(r) if uses.get(&r).copied().unwrap_or(0) <= 1 => doc.set(r, stream),
                _ => {
                    *entry = Object::Ref(doc.add(stream));
                    relisted = true;
                }
            }
        }
        if relisted {
            let contents = match entries.as_slice() {
                [one] => one.clone(),
                _ => Object::Array(entries),
            };
            doc.update_dict(self.page, |d| d.set(b"Contents".to_vec(), contents)).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// The painting operators in order, with marked content left out: what a rewrite must keep.
    #[cfg(test)]
    pub fn painting(&self) -> Vec<Vec<u8>> {
        self.ops
            .iter()
            .filter(|op| !matches!(op.op.as_slice(), b"BMC" | b"BDC" | b"EMC"))
            .map(|op| pdfcraft_content::serialize_ops(std::slice::from_ref(op)))
            .collect()
    }
}

/// Where a text-showing operator draws, and what it says.
fn show(op: &Op, m: &Metrics, state: &State, tm: &mut Matrix) -> (Option<[f64; 4]>, String) {
    let pieces: Vec<&Object> = match op.op.as_slice() {
        b"TJ" => op.operands.first().and_then(Object::as_array).map(|a| a.iter().collect()).unwrap_or_default(),
        _ => op.operands.last().into_iter().collect(),
    };
    let mut bbox = None;
    let mut text = String::new();
    let font = Matrix([state.size * state.scale, 0.0, 0.0, state.size, 0.0, state.rise]);
    for piece in pieces {
        let s = match piece {
            Object::String(s) => s,
            other => {
                let dx = -other.as_f64().unwrap_or(0.0) / 1000.0 * state.size * state.scale;
                *tm = Matrix::translate(dx, 0.0).then(tm);
                continue;
            }
        };
        for (code, len) in m.codes(&s.bytes) {
            let w = m.width(code);
            let trm = font.then(tm).then(&state.ctm);
            bbox = union(bbox, trm.bbox([0.0, m.descent, w, m.ascent]));
            if let Some(t) = m.text_of(code)
                && text.len() < MAX_TEXT
            {
                text.push_str(&t);
            }
            let advance = w * state.size + state.char_spacing + if m.is_space(code, len) { state.word_spacing } else { 0.0 };
            *tm = Matrix::translate(advance * state.scale, 0.0).then(tm);
        }
    }
    (bbox, text)
}

/// How many times each content stream object is listed by the document's pages.
pub(crate) fn stream_uses(doc: &Document, pages: &[pdfcraft_model::Page]) -> HashMap<ObjRef, usize> {
    let mut uses: HashMap<ObjRef, usize> = HashMap::new();
    for page in pages {
        for entry in entries_of(doc, &page.dict).unwrap_or_default() {
            if let Some(r) = entry.as_ref() {
                *uses.entry(r).or_default() += 1;
            }
        }
    }
    uses
}

/// The bytes of a `BDC` opening tagged content.
pub(crate) fn open_tagged(tag: &str, mcid: i64) -> Vec<u8> {
    let mut properties = Dict::new();
    properties.set(b"MCID".to_vec(), Object::Int(mcid));
    let mut out = b"\n".to_vec();
    out.extend(pdfcraft_content::serialize_ops(&[Op::new("BDC", vec![Object::name(tag), Object::Dict(properties)])]));
    out
}

/// The bytes of a `BMC` opening an artifact.
pub(crate) fn open_artifact() -> Vec<u8> {
    b"\n/Artifact BMC\n".to_vec()
}

pub(crate) fn close() -> Vec<u8> {
    b"\nEMC\n".to_vec()
}
