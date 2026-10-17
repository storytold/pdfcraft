//! Status layers of named-layer signature appearances, drawn for the signature's real state.
//!
//! A named-layer appearance's `/N` draws a layer form, which draws `n0` (background), `n1`
//! (unknown or valid), `n2` (signer details), `n3` (invalid) and `n4` (status text). The file
//! stores one state, usually "not verified". When `/N` draws exactly one Form whose resources hold
//! both `n1` and `n3`, `n1`, `n3` and `n4` (if present) are redrawn for the signature's state and
//! every other layer is left as stored. Any other layout keeps its artwork, and the viewer marks
//! it with its own badge.
//!
//! Only the display copy changes, and nothing in it is edited in place: each recognised
//! appearance is copied once per state, so a stream shared between widgets or fields never shows
//! one signature's state on another. Saving writes the working file, so signed bytes stay as they
//! were.
//!
//! The icons are the Lucide ones the Signatures panel uses (`assets/icons`).

use std::collections::{HashMap, HashSet};

use pdfcraft_content::{Op, num, serialize_ops, string};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};
use pdfcraft_fonts::source_shaping::ShapedRun;
use pdfcraft_fonts::{INTER_REGULAR, embedding, first_non_win_ansi, helvetica_width, win_ansi};

use crate::pdf::{SignatureInfo, Status, sig_fields};

/// The words of the regenerated `n4` caption, one per state. The caller chooses them, so an
/// interface that speaks another language passes its own; the default is English.
///
/// The font follows from the words, chosen for all three at once so two signatures in
/// different states never read in two languages: the standard Helvetica when WinAnsi encodes
/// them, else Inter, embedded and shaped (Latin, Cyrillic, Greek), else a craft-fonts face for
/// Arabic, Japanese or Telugu, shaped and drawn as outlines, else all three in English.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusCaptions {
    pub valid: String,
    pub unknown: String,
    pub invalid: String,
}

impl Default for StatusCaptions {
    fn default() -> Self {
        Self { valid: english(Status::Valid).into(), unknown: english(Status::Unknown).into(), invalid: english(Status::Invalid).into() }
    }
}

/// The English caption for each state: the default, and what is drawn when the caller's words
/// cannot be.
const fn english(status: Status) -> &'static str {
    match status {
        Status::Valid => "Signature valid",
        Status::Unknown => "Validity unknown",
        Status::Invalid => "Signature invalid",
    }
}

/// How one call draws its captions: the words for each state and the font for all of them.
struct Face<'a> {
    /// Valid, unknown, invalid.
    words: [&'a str; 3],
    font: Font,
}

enum Font {
    /// The standard Helvetica, WinAnsi-encoded: nothing is embedded.
    Helvetica,
    /// Inter, embedded on first use. Each state's caption is shaped and its glyphs embedded once
    /// per call (`None` when that failed), and every copy drawn in that state shares them.
    Inter { shaped: HashMap<Status, Option<(Object, ShapedRun)>> },
    /// A craft-fonts face, shaped and drawn as filled glyph outlines: the Arabic and Telugu faces
    /// are variable fonts, which cannot be embedded.
    Outlined { bytes: &'static [u8], rtl: bool },
}

impl<'a> Face<'a> {
    fn new(captions: &'a StatusCaptions) -> Self {
        let words = [captions.valid.as_str(), captions.unknown.as_str(), captions.invalid.as_str()];
        if words.iter().all(|w| first_non_win_ansi(w).is_none()) {
            return Face { words, font: Font::Helvetica };
        }
        let script = script_of(&words);
        match face_for(script, &words) {
            Some((bytes, _)) => match script {
                None => Face { words, font: Font::Inter { shaped: HashMap::new() } },
                Some(script) => Face { words, font: Font::Outlined { bytes, rtl: script == "Arab" } },
            },
            None => Face::english(),
        }
    }

    fn english() -> Self {
        Face { words: [Status::Valid, Status::Unknown, Status::Invalid].map(english), font: Font::Helvetica }
    }

    fn words(&self, status: Status) -> &'a str {
        match status {
            Status::Valid => self.words[0],
            Status::Unknown => self.words[1],
            Status::Invalid => self.words[2],
        }
    }
}

/// The ISO 15924 script a craft-fonts face has to be meant for to draw `words`, or `None` for the
/// Latin, Cyrillic and Greek that Inter draws. Han without kana is `Hani`, which no face is meant
/// for: a Japanese face would draw Chinese in Japanese letterforms, so Chinese stays English.
fn script_of(words: &[&str]) -> Option<&'static str> {
    let any = |is: fn(char) -> bool| words.iter().flat_map(|w| w.chars()).any(is);
    if any(
        |c| matches!(c, '\u{0600}'..='\u{06FF}' | '\u{0750}'..='\u{077F}' | '\u{08A0}'..='\u{08FF}' | '\u{FB50}'..='\u{FDFF}' | '\u{FE70}'..='\u{FEFF}'),
    ) {
        Some("Arab")
    } else if any(|c| matches!(c, '\u{0C00}'..='\u{0C7F}')) {
        Some("Telu")
    } else if any(|c| matches!(c, '\u{3040}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}')) {
        Some("Jpan")
    } else if any(|c| matches!(c, '\u{3400}'..='\u{4DBF}' | '\u{4E00}'..='\u{9FFF}')) {
        Some("Hani")
    } else {
        None
    }
}

/// A face that draws all of `words`: Inter when `script` is `None`, else the first craft-fonts
/// face meant for `script`. Faces are picked by the script they are meant for, so one the asset
/// policy keeps out of documents (meant for `Hans`) is never picked.
fn face_for(script: Option<&str>, words: &[&str]) -> Option<(&'static [u8], &'static str)> {
    let draws = |bytes: &[u8]| words.iter().all(|w| embedding::covers(bytes, w));
    let face = match script {
        None => return draws(INTER_REGULAR).then_some((INTER_REGULAR, "Inter")),
        // Sans first, like the other captions.
        Some("Jpan") => pdfcraft_fonts::document_japanese_fonts_for_style(false, false).into_iter().find(|f| draws(f.bytes)),
        Some(script) => pdfcraft_fonts::CRAFT_FONTS.iter().find(|f| f.covers(script) && draws(f.bytes)),
    };
    face.map(|f| (f.bytes, f.family))
}

/// Each verdict's colour, shared by the regenerated icons and the viewer's badge and panel, so
/// the page and the interface agree.
pub const fn status_rgb(status: Status) -> [u8; 3] {
    match status {
        Status::Valid => [0x2D, 0x9D, 0x78],
        Status::Unknown => [0xE6, 0x86, 0x19],
        Status::Invalid => [0xD7, 0x37, 0x3F],
    }
}

// The icons as PDF path operators, in Lucide's own 24-unit, y-down space; `icon_ops` flips and
// scales them into a layer. Converted from lucide-static 1.49.0 (ISC; `assets/icons/*.svg`):
// each circle `cx=12 cy=12 r=10` is four cubics with k = 0.5522847498·r, and each arc of
// triangle-alert's `d` is split into ≤90° cubics per SVG 1.1 §F.6.5. Stroke width 2, round caps
// and joins, no fill, as in the SVGs.

/// `circle-check.svg`: `<circle cx="12" cy="12" r="10"/>`, `<path d="m16 9-5.5 5.5L8 12"/>`.
const CIRCLE_CHECK: &str =
    "22 12 m 22 17.523 17.523 22 12 22 c 6.477 22 2 17.523 2 12 c 2 6.477 6.477 2 12 2 c 17.523 2 22 6.477 22 12 c h S 16 9 m 10.5 14.5 l 8 12 l S";

/// `triangle-alert.svg`: `<path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3"/>`,
/// `<path d="M12 9v4"/>`, `<path d="M12 17h.01"/>`.
const TRIANGLE_ALERT: &str = "21.73 18 m 13.73 4 l 13.375 3.373 12.71 2.986 11.99 2.986 c 11.27 2.986 10.605 3.373 10.25 4 c 2.25 18 l 1.891 18.622 1.893 19.388 2.254 20.008 c 2.616 20.628 3.282 21.006 4 21 c 20 21 l 20.714 20.999 21.374 20.618 21.731 19.999 c 22.087 19.38 22.087 18.618 21.73 18 c h S 12 9 m 12 13 l S 12 17 m 12.01 17 l S";

/// `circle-x.svg`: `<circle cx="12" cy="12" r="10"/>`, `<path d="m15 9-6 6"/>`, `<path d="m9 9 6 6"/>`.
const CIRCLE_X: &str = "22 12 m 22 17.523 17.523 22 12 22 c 6.477 22 2 17.523 2 12 c 2 6.477 6.477 2 12 2 c 17.523 2 22 6.477 22 12 c h S 15 9 m 9 15 l S 9 9 m 15 15 l S";

/// Decoded bytes read from a `/N` stream at most: appearances are kilobytes, and anything
/// bigger is damage, which only costs the widget its regeneration.
const MAX_LAYER_DECODED: usize = 1 << 20;

/// The most distinct XObjects an `/N` may draw and still be recognised. The convention draws
/// one; the cap bounds the lookups a stream drawing thousands would cost.
const MAX_DRAWN_NAMES: usize = 16;

/// The caption's resource name. The regenerated `n4` has resources of its own holding this
/// font alone, so the name cannot collide with anything the document uses.
const CAPTION_FONT: &str = "Cap";

/// Draw each signed signature's state into its recognised widgets, in `doc` (the display copy
/// only). A signature whose every visible widget was regenerated comes back with `verdict_shown`
/// set. Best-effort and total: anything unexpected leaves that widget as it was. Returns whether
/// anything changed.
///
/// Visibility is each widget's own: `SignatureInfo::visible` describes the first widget only,
/// and a field's other widgets may show where the first does not.
pub fn show_status_layers(doc: &mut Document, signatures: &mut [SignatureInfo], captions: &StatusCaptions) -> bool {
    let fields = Fields::of(doc);
    let mut copies = Copies { face: Face::new(captions), n: HashMap::new(), forms: HashMap::new() };
    let mut changed = false;
    for sig in signatures.iter_mut().filter(|s| drawable(s)) {
        let Some(widgets) = fields.widgets(&sig.field) else { continue };
        let mut visible = 0usize;
        let mut shown = 0usize;
        for (r, widget) in widgets {
            if !shows(doc, widget) {
                continue;
            }
            visible += 1;
            let Some(r) = r.filter(|r| !fields.shared(*r)) else { continue };
            if draw(doc, r, widget, sig.status, &mut copies).is_some() {
                shown += 1;
                changed = true;
            }
        }
        sig.verdict_shown = visible > 0 && shown == visible;
    }
    changed
}

/// Whether any signed signature has a widget [`show_status_layers`] would regenerate, so a caller
/// builds a display copy only when it would change something.
pub fn has_status_layers(doc: &Document, signatures: &[SignatureInfo]) -> bool {
    let fields = Fields::of(doc);
    // Each appearance is read once, however many widgets share it.
    let mut checked = HashSet::new();
    signatures.iter().filter(|s| drawable(s)).any(|sig| {
        fields.widgets(&sig.field).is_some_and(|widgets| {
            widgets.iter().any(|(r, w)| {
                r.is_some_and(|r| !fields.shared(r))
                    && shows(doc, w)
                    && appearance(doc, w).is_some_and(|n| checked.insert(n.num) && recognise(doc, n).is_some())
            })
        })
    })
}

/// A signed field. A document timestamp has no field of its own, and its name could be a real
/// field's.
fn drawable(sig: &SignatureInfo) -> bool {
    sig.signed && !sig.doc_timestamp
}

/// What one call has drawn. A copy depends only on its original and the state drawn into it, so
/// widgets that share an appearance share its copy, and the display copy grows by at most one copy
/// per state of what the file holds. Keyed by object number, since references that differ only in
/// generation reach the same object.
struct Copies<'a> {
    face: Face<'a>,
    /// Original `/N` and state → its copy, or `None` when it cannot be drawn.
    n: HashMap<(u32, Status), Option<ObjRef>>,
    /// Original layer form and state → its copy.
    forms: HashMap<(u32, Status), ObjRef>,
}

/// Point one widget at its appearance's copy for `status`, drawing that copy first if this call
/// has not drawn it yet.
fn draw(doc: &mut Document, widget_ref: ObjRef, widget: &Dict, status: Status, copies: &mut Copies) -> Option<()> {
    let n = appearance(doc, widget)?;
    let copy = match copies.n.get(&(n.num, status)) {
        Some(copy) => *copy,
        None => {
            let copy = recognise(doc, n).and_then(|layout| regenerate(doc, &layout, status, copies));
            copies.n.insert((n.num, status), copy);
            copy
        }
    }?;
    // /D and /R (pressed and rollover) stay: only the static appearance shows the state.
    let mut ap = doc.resolve(widget.get(b"AP")?).as_dict()?.clone();
    ap.set(b"N".to_vec(), copy);
    let mut widget = widget.clone();
    widget.set(b"AP".to_vec(), ap);
    // Under the object's own generation, whatever generation the field's reference named.
    doc.set(ObjRef::new(widget_ref.num, doc.generation(widget_ref.num)), widget);
    Some(())
}

/// The widget's `/AP /N`. A stream is always an indirect object, so anything else (a dictionary
/// of named states, say) is not one static appearance.
fn appearance(doc: &Document, widget: &Dict) -> Option<ObjRef> {
    let ap = doc.resolve(widget.get(b"AP")?);
    ap.as_dict()?.get(b"N")?.as_ref()
}

/// A field's widgets: each one's reference (when it has one) and dictionary.
type Widgets = Vec<(Option<ObjRef>, Dict)>;

/// The signature fields' widgets by field name, and which widgets (by object number) more than
/// one field claims.
struct Fields {
    by_name: HashMap<String, Vec<Widgets>>,
    claims: HashMap<u32, usize>,
}

impl Fields {
    fn of(doc: &Document) -> Self {
        let mut by_name: HashMap<String, Vec<Widgets>> = HashMap::new();
        let mut claims: HashMap<u32, usize> = HashMap::new();
        for f in sig_fields(doc) {
            for r in f.widgets.iter().filter_map(|(r, _)| r.map(|r| r.num)).collect::<HashSet<_>>() {
                *claims.entry(r).or_default() += 1;
            }
            by_name.entry(f.name).or_default().push(f.widgets);
        }
        Self { by_name, claims }
    }

    /// The widgets of the one field called `name`. Two fields of one name leave it unclear which
    /// verdict is whose, so neither is drawn.
    fn widgets(&self, name: &str) -> Option<&[(Option<ObjRef>, Dict)]> {
        match self.by_name.get(name)?.as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }

    /// A widget two fields claim would show whichever verdict was drawn last.
    fn shared(&self, widget: ObjRef) -> bool {
        self.claims.get(&widget.num).is_some_and(|&n| n > 1)
    }
}

/// Whether a widget is on screen: not hidden or no-view (annotation flags 2 and 6), with a
/// `/Rect` of four finite numbers enclosing an area.
fn shows(doc: &Document, widget: &Dict) -> bool {
    let flags = widget.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
    if flags & (2 | 32) != 0 {
        return false;
    }
    four_numbers(doc, widget.get(b"Rect")).is_some()
}

/// Four finite numbers, ordered (`[x0 y0 x1 y1]` with x0 < x1 and y0 < y1), enclosing a
/// non-zero, bounded area.
fn four_numbers(doc: &Document, o: Option<&Object>) -> Option<[f64; 4]> {
    let a = doc.resolve(o?);
    let a = a.as_array()?;
    let [x0, y0, x1, y1] = a.as_slice() else { return None };
    let mut v = [0.0; 4];
    for (slot, x) in v.iter_mut().zip([x0, y0, x1, y1]) {
        *slot = doc.resolve(x).as_f64()?;
    }
    let b = [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])];
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    (b.iter().all(|x| x.is_finite()) && w > 0.0 && h > 0.0 && w <= 1e6 && h <= 1e6).then_some(b)
}

/// A status layer: the stream as stored and its normalised `/BBox`.
struct Layer {
    stream: Stream,
    bbox: [f64; 4],
}

/// An appearance that follows the named-layer convention, with every part that gets copied.
struct Layout {
    /// The widget's `/N`, and its XObject resources (resolved).
    n: Stream,
    n_xobjects: Dict,
    /// The layer form's name in `n_xobjects`, its reference, the layer form, and its XObject
    /// resources.
    name: Vec<u8>,
    form_ref: ObjRef,
    form: Stream,
    form_xobjects: Dict,
    n1: Layer,
    n3: Layer,
    n4: Option<Layer>,
}

/// The named-layer rule: a widget's `/N` (`n`) is a stream that draws, from its own XObject
/// resources, exactly one Form whose own XObject resources hold both `n1` and `n3`. Those two,
/// and `n4` if present, must be Forms with a usable `/BBox`.
fn recognise(doc: &Document, n: ObjRef) -> Option<Layout> {
    let n = doc.get(n);
    // A dictionary here is a set of named states (on/off), not the one static appearance.
    let Object::Stream(n) = &*n else { return None };
    let n_xobjects = xobjects(doc, &n.dict)?;
    let content = n.decoded_within(MAX_LAYER_DECODED).ok()?;
    let parsed = pdfcraft_content::parse(&content);
    let drawn: HashSet<&[u8]> = parsed.ops.iter().filter(|op| op.is("Do")).filter_map(|op| op.operands.last().and_then(Object::as_name)).collect();
    if drawn.len() > MAX_DRAWN_NAMES {
        return None;
    }
    let mut found = None;
    for name in drawn {
        let Some(form_ref) = n_xobjects.get(name).and_then(Object::as_ref) else { continue };
        let form = doc.get(form_ref);
        let Object::Stream(form) = &*form else { continue };
        if !is_form(doc, &form.dict) {
            continue;
        }
        let Some(layers) = xobjects(doc, &form.dict) else { continue };
        if layers.contains(b"n1") && layers.contains(b"n3") {
            // Which of two a viewer would take is not known, so neither is.
            if found.is_some() {
                return None;
            }
            found = Some((name.to_vec(), form_ref, form.clone(), layers));
        }
    }
    let (name, form_ref, form, form_xobjects) = found?;
    let n1 = layer(doc, form_xobjects.get(b"n1"))?;
    let n3 = layer(doc, form_xobjects.get(b"n3"))?;
    let n4 = match form_xobjects.get(b"n4") {
        Some(o) => Some(layer(doc, Some(o))?),
        None => None,
    };
    Some(Layout { n: n.clone(), n_xobjects, name, form_ref, form, form_xobjects, n1, n3, n4 })
}

fn is_form(doc: &Document, dict: &Dict) -> bool {
    dict.get(b"Subtype").is_some_and(|s| doc.resolve(s).as_name() == Some(b"Form"))
}

/// A stream dictionary's XObject resources, resolved.
fn xobjects(doc: &Document, dict: &Dict) -> Option<Dict> {
    let res = doc.resolve(dict.get(b"Resources")?);
    let xobjects = doc.resolve(res.as_dict()?.get(b"XObject")?);
    xobjects.as_dict().cloned()
}

/// A status layer: a Form with a usable `/BBox`.
fn layer(doc: &Document, o: Option<&Object>) -> Option<Layer> {
    let o = doc.resolve(o?);
    let Object::Stream(stream) = &*o else { return None };
    if !is_form(doc, &stream.dict) {
        return None;
    }
    let bbox = four_numbers(doc, stream.dict.get(b"BBox"))?;
    Some(Layer { stream: stream.clone(), bbox })
}

/// Draw `status` into a copy of a recognised appearance: new `n1`, `n3` and `n4`, a layer form
/// pointing at them (shared by every `/N` that draws the same layer form), and an `/N` pointing at
/// that. Returns the new `/N`; nothing is written for an appearance whose icons cannot be drawn.
fn regenerate(doc: &mut Document, layout: &Layout, status: Status, copies: &mut Copies) -> Option<ObjRef> {
    let form = match copies.forms.get(&(layout.form_ref.num, status)) {
        Some(form) => *form,
        None => {
            let form = regenerate_form(doc, layout, status, &mut copies.face)?;
            copies.forms.insert((layout.form_ref.num, status), form);
            form
        }
    };
    // /N is copied as stored, filters and all; only its XObject resources change.
    let mut n_xobjects = layout.n_xobjects.clone();
    n_xobjects.set(layout.name.clone(), form);
    let mut n = layout.n.clone();
    set_xobjects(doc, &mut n.dict, n_xobjects);
    Some(doc.add(n))
}

/// The layer form's copy for `status`, with its status layers redrawn.
fn regenerate_form(doc: &mut Document, layout: &Layout, status: Status, face: &mut Face) -> Option<ObjRef> {
    let icon = |path: &str, layer: &Layer| icon_ops(path, status_rgb(status), &layer.bbox);
    let (n1, n3) = match status {
        Status::Valid => (icon(CIRCLE_CHECK, &layout.n1)?, Vec::new()),
        Status::Unknown => (icon(TRIANGLE_ALERT, &layout.n1)?, Vec::new()),
        Status::Invalid => (Vec::new(), icon(CIRCLE_X, &layout.n3)?),
    };
    let mut layers = layout.form_xobjects.clone();
    let n1 = Stream::from_raw(layer_dict(doc, &layout.n1, Dict::new()), n1);
    let n3 = Stream::from_raw(layer_dict(doc, &layout.n3, Dict::new()), n3);
    layers.set(b"n1".to_vec(), doc.add(n1));
    layers.set(b"n3".to_vec(), doc.add(n3));
    if let Some(n4) = &layout.n4 {
        let (resources, ops) = caption(doc, face, status, &n4.bbox);
        let n4 = Stream::from_raw(layer_dict(doc, n4, resources), ops);
        layers.set(b"n4".to_vec(), doc.add(n4));
    }
    // Copied as stored, filters and all; only the XObject resources change.
    let mut form = layout.form.clone();
    set_xobjects(doc, &mut form.dict, layers);
    Some(doc.add(form))
}

/// A fresh Form dictionary for a regenerated layer: the original's `/BBox` and `/Matrix`, so it
/// sits where the original did, and `resources`.
fn layer_dict(doc: &Document, layer: &Layer, resources: Dict) -> Dict {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("XObject"));
    d.set(b"Subtype".to_vec(), Object::name("Form"));
    for key in [&b"BBox"[..], b"Matrix"] {
        if let Some(v) = layer.stream.dict.get(key) {
            d.set(key.to_vec(), (*doc.resolve(v)).clone());
        }
    }
    d.set(b"Resources".to_vec(), resources);
    d
}

/// Point `dict`'s XObject resources at `xobjects`, through a direct copy of its resources: they
/// may be shared, and the original must stay as it was.
fn set_xobjects(doc: &Document, dict: &mut Dict, xobjects: Dict) {
    let mut resources = dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()).unwrap_or_default();
    resources.set(b"XObject".to_vec(), xobjects);
    dict.set(b"Resources".to_vec(), resources);
}

/// A Lucide icon stroked in `rgb`, as large as fits in `bbox` and centred in it. The path is in
/// Lucide's 24-unit, y-down space, so the `cm` scales it and flips it about the top edge.
fn icon_ops(path: &str, rgb: [u8; 3], bbox: &[f64; 4]) -> Option<Vec<u8>> {
    let (w, h) = (bbox[2] - bbox[0], bbox[3] - bbox[1]);
    let side = w.min(h);
    let scale = side / 24.0;
    let (x, y) = (bbox[0] + (w - side) / 2.0, bbox[1] + (h + side) / 2.0);
    if !(scale > 0.0 && [scale, x, y].iter().all(|v| v.is_finite())) {
        return None;
    }
    let channel = |v: u8| num(f64::from(v) / 255.0);
    let [r, g, b] = rgb;
    let mut out = serialize_ops(&[
        Op::new("q", vec![]),
        Op::new("RG", vec![channel(r), channel(g), channel(b)]),
        Op::new("w", vec![num(2.0)]),
        Op::new("J", vec![num(1.0)]),
        Op::new("j", vec![num(1.0)]),
        Op::new("cm", vec![num(scale), num(0.0), num(0.0), num(-scale), num(x), num(y)]),
    ]);
    out.extend_from_slice(path.as_bytes());
    out.extend_from_slice(b"\nQ\n");
    Some(out)
}

/// The regenerated `n4`: its resources and content, one line enlarged to fill `bbox`, set from
/// its left (its right for right-to-left words). A caption too small to read leaves the layer
/// blank.
fn caption(doc: &mut Document, face: &mut Face, status: Status, bbox: &[f64; 4]) -> (Dict, Vec<u8>) {
    let words = face.words(status);
    // A face that draws every character (`face_for` checked) fails only on damage: that one
    // caption is then drawn in English rather than not at all.
    let shaped = match &mut face.font {
        Font::Helvetica => return helvetica_caption(words, bbox),
        Font::Outlined { bytes, rtl } => {
            return outlined_caption(bytes, words, *rtl, bbox).unwrap_or_else(|| helvetica_caption(english(status), bbox));
        }
        Font::Inter { shaped } => shaped.entry(status).or_insert_with(|| shape_caption(doc, INTER_REGULAR, "Inter", words)).clone(),
    };
    let Some((font, run)) = shaped else { return helvetica_caption(english(status), bbox) };
    let resources = caption_resources(font);
    let Some(size) = caption_size(bbox, run.advance) else { return (resources, Vec::new()) };
    let x = caption_x(bbox, run.advance * size, false);
    let y = caption_baseline(bbox, size);
    // One positioned glyph at a time, as the shaper placed them; the span keeps the words in
    // reading order for copy and search, whatever order the glyphs are painted in.
    let mut actual = Dict::new();
    actual.set(b"ActualText".to_vec(), Object::String(PdfString::text(words)));
    let mut ops = vec![Op::new("BDC", vec![Object::name("Span"), Object::Dict(actual)]), Op::new("g", vec![num(0.0)])];
    for (i, glyph) in run.glyphs.iter().enumerate() {
        // The shaper allows at most 65,534 glyphs, and `embed_mapping` numbers them from 1.
        let Ok(code) = u16::try_from(i + 1) else { break };
        let (gx, gy) = (x + glyph.position[0] * size, y + glyph.position[1] * size);
        ops.extend([
            Op::new("BT", vec![]),
            Op::new("Tf", vec![Object::name(CAPTION_FONT), num(size)]),
            Op::new("Tm", vec![num(1.0), num(0.0), num(0.0), num(1.0), num(gx), num(gy)]),
            Op::new("Tj", vec![Object::String(PdfString { bytes: code.to_be_bytes().to_vec(), hex: true })]),
            Op::new("ET", vec![]),
        ]);
    }
    ops.push(Op::new("EMC", vec![]));
    (resources, serialize_ops(&ops))
}

/// `words` in the standard Helvetica. The caller has made sure WinAnsi encodes them.
fn helvetica_caption(words: &str, bbox: &[f64; 4]) -> (Dict, Vec<u8>) {
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Helvetica"));
    font.set(b"Encoding".to_vec(), Object::name("WinAnsiEncoding"));
    let resources = caption_resources(Object::Dict(font));
    let Some(size) = caption_size(bbox, helvetica_width(words, 1.0)).filter(|_| !words.is_empty()) else { return (resources, Vec::new()) };
    let x = caption_x(bbox, 0.0, false);
    let ops = serialize_ops(&[
        Op::new("BT", vec![]),
        Op::new("Tf", vec![Object::name(CAPTION_FONT), num(size)]),
        Op::new("g", vec![num(0.0)]),
        Op::new("Tm", vec![num(1.0), num(0.0), num(0.0), num(1.0), num(x), num(caption_baseline(bbox, size))]),
        Op::new("Tj", vec![string(win_ansi(words))]),
        Op::new("ET", vec![]),
    ]);
    (resources, ops)
}

/// `words` shaped in a craft-fonts face and drawn as filled glyph outlines, in reading order. The
/// span keeps the words for copy and search where a reader takes them from it. `None` when the
/// words cannot be shaped or a glyph has no usable outline; a caption too small to read is blank.
fn outlined_caption(bytes: &[u8], words: &str, rtl: bool, bbox: &[f64; 4]) -> Option<(Dict, Vec<u8>)> {
    let run = pdfcraft_fonts::source_shaping::shape(bytes, words).ok().filter(|run| !run.glyphs.is_empty())?;
    let outlines = run.glyphs.iter().map(|g| pdfcraft_fonts::face_glyph(bytes, g.id.to_u32()).ok()).collect::<Option<Vec<_>>>()?;
    let Some(size) = caption_size(bbox, run.advance) else { return Some((Dict::new(), Vec::new())) };
    let x = caption_x(bbox, run.advance * size, rtl);
    let y = caption_baseline(bbox, size);
    let mut actual = Dict::new();
    actual.set(b"ActualText".to_vec(), Object::String(PdfString::text(words)));
    let mut ops = vec![Op::new("BDC", vec![Object::name("Span"), Object::Dict(actual)]), Op::new("q", vec![]), Op::new("g", vec![num(0.0)])];
    for (glyph, outline) in run.glyphs.iter().zip(&outlines) {
        let (gx, gy) = (x + glyph.position[0] * size, y + glyph.position[1] * size);
        for contour in &outline.contours {
            for (i, [px, py]) in contour.iter().enumerate() {
                ops.push(Op::new(if i == 0 { "m" } else { "l" }, vec![num(gx + px * size), num(gy + py * size)]));
            }
            ops.push(Op::new("h", vec![]));
        }
    }
    ops.extend([Op::new("f", vec![]), Op::new("Q", vec![]), Op::new("EMC", vec![])]);
    Some((Dict::new(), serialize_ops(&ops)))
}

/// Where a line `width` wide starts in `bbox`: at its left, or, for right-to-left words, so that
/// it ends at the right.
fn caption_x(bbox: &[f64; 4], width: f64, rtl: bool) -> f64 {
    let inset = 0.03 * (bbox[2] - bbox[0]);
    if rtl { bbox[2] - inset - width } else { bbox[0] + inset }
}

/// The caption's own resources: `font` alone, under [`CAPTION_FONT`].
fn caption_resources(font: Object) -> Dict {
    let mut fonts = Dict::new();
    fonts.set(CAPTION_FONT.as_bytes().to_vec(), font);
    let mut resources = Dict::new();
    resources.set(b"Font".to_vec(), fonts);
    resources
}

/// The size a line `width` wide at 1pt takes to fill `bbox`, or `None` when it would not be
/// legible.
fn caption_size(bbox: &[f64; 4], width: f64) -> Option<f64> {
    let (w, h) = (bbox[2] - bbox[0], bbox[3] - bbox[1]);
    Some((h * 0.62).min(w * 0.94 / width)).filter(|size| size.is_finite() && *size >= 1.0)
}

/// Where a line of `size` sits in `bbox`: centred on the height of its capitals.
fn caption_baseline(bbox: &[f64; 4], size: f64) -> f64 {
    bbox[1] + ((bbox[3] - bbox[1] - 0.72 * size) / 2.0).max(0.0)
}

/// Shape `words` in a static face and embed the glyphs that takes, as the edit crate does for
/// complex scripts: the font object and the shaped run. `None` when either step fails.
fn shape_caption(doc: &mut Document, bytes: &[u8], family: &str, words: &str) -> Option<(Object, ShapedRun)> {
    let run = pdfcraft_fonts::source_shaping::shape(bytes, words).ok().filter(|run| !run.glyphs.is_empty())?;
    let mappings: Vec<_> = run.glyphs.iter().map(|g| (g.text.clone(), g.id)).collect();
    let mut fonts = Dict::new();
    let name = embedding::embed_mapping(doc, &mut fonts, bytes, family, &mappings).ok()?;
    Some((fonts.get(name.as_bytes())?.clone(), run))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    // The geometry of a real layered appearance, scaled by 4: a 50×30pt widget; n0, n1 and n3
    // are 100×100-unit layers, n1 and n3 drawn through `cm [0.27 0 0 0.27 11.5 1.5]`; n2 lines of
    // signer text; n4 a strip [0 21 50 30]. Every word and drawing here is synthetic.
    const W: f64 = 200.0;
    const H: f64 = 120.0;
    const UNIT: [f64; 4] = [0.0, 0.0, 100.0, 100.0];
    const IDENTITY: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    const GLYPH_CM: [f64; 6] = [1.08, 0.0, 0.0, 1.08, 46.0, 6.0];
    const HELV: &str = "<< /Font << /F1 7 0 R >> >>";
    const QUESTION: &str =
        "q 1 0.84 0.2 rg 0.55 0.4 0 RG 3 w 30 72 m 30 88 42 98 56 98 c 72 98 84 88 84 74 c 66 50 l 58 28 l 44 28 l 44 48 l h B 44 6 14 14 re B Q";
    const SIGNER: &str = "BT /F1 13.6 Tf 1 0 0 1 8 62.4 Tm (Digitally signed by) Tj 1 0 0 1 8 48.8 Tm (Test Signer) Tj ET";
    const NOT_VERIFIED: &str = "BT /F1 18.8 Tf 1 0 0 1 8 93.2 Tm (Signature Not Verified) Tj ET";
    const BLANK: &str = "% DSBlank";
    const ARTWORK: &str = "q 0.1 0.1 0.45 RG 4 w 1 J 1 j 8 40 m 22 85 34 8 50 50 c 62 78 72 18 92 56 c S Q";

    #[derive(Clone)]
    struct NamedLayer {
        name: &'static str,
        res: String,
        ops: String,
        bbox: [f64; 4],
        cm: [f64; 6],
    }

    /// A named layer, placed as that appearance places it.
    fn layer(name: &'static str) -> NamedLayer {
        let (res, ops, bbox, cm) = match name {
            "n1" => ("<< >>", QUESTION, UNIT, GLYPH_CM),
            "n2" => (HELV, SIGNER, [0.0, 0.0, W, H], IDENTITY),
            "n3" => ("<< >>", BLANK, UNIT, GLYPH_CM),
            "n4" => (HELV, NOT_VERIFIED, [0.0, 84.0, W, H], IDENTITY),
            "n5" => ("<< >>", ARTWORK, UNIT, [1.08, 0.0, 0.0, 1.08, 120.0, 6.0]),
            _ => ("<< >>", BLANK, UNIT, IDENTITY),
        };
        NamedLayer { name, res: res.into(), ops: ops.into(), bbox, cm }
    }

    fn nums(v: &[f64]) -> String {
        v.iter().map(f64::to_string).collect::<Vec<_>>().join(" ")
    }

    fn form(bbox: [f64; 4], res: &str, ops: &str) -> String {
        format!("<< /Type /XObject /Subtype /Form /BBox [{}] /Resources {res} /Length {} >>\nstream\n{ops}\nendstream", nums(&bbox), ops.len())
    }

    /// A one-page file whose signature field (object 4, or 8 with widget 4 when split) shows
    /// the appearance in `objs` from object 10 (its `/N`) on.
    #[derive(Clone)]
    struct Fixture {
        layers: Vec<NamedLayer>,
        /// The layer form's name in `/N`'s resources.
        frm: &'static str,
        /// One more form between `/N` and the layer form.
        nested: bool,
        /// `/N`'s resources and the layer form's XObject dictionary as indirect objects.
        indirect: bool,
        /// A widget split from its field.
        split: bool,
        /// Widget `/F` and `/Rect`.
        flags: u32,
        rect: &'static str,
        /// Objects added or replaced last.
        extra: Vec<(u32, String)>,
    }

    impl Fixture {
        fn named(names: &[&'static str]) -> Self {
            Fixture {
                layers: names.iter().map(|n| layer(n)).collect(),
                frm: "FRM",
                nested: false,
                indirect: false,
                split: false,
                flags: 132,
                rect: "[200 600 400 720]",
                extra: Vec::new(),
            }
        }

        fn layered() -> Self {
            Self::named(&["n0", "n1", "n2", "n3", "n4"])
        }

        fn with(mut self, f: impl FnOnce(&mut Self)) -> Self {
            f(&mut self);
            self
        }

        fn objects(&self) -> BTreeMap<u32, String> {
            let full = [0.0, 0.0, W, H];
            let frm = self.frm;
            let draw = |name: &str| format!("q 1 0 0 1 0 0 cm /{name} Do Q");
            let mut objs = BTreeMap::new();
            objs.insert(1, "<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>".to_string());
            objs.insert(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string());
            objs.insert(
                3,
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R /Resources << /Font << /F1 7 0 R >> >> /Annots [4 0 R] >>"
                    .to_string(),
            );
            objs.insert(5, format!("<< /Fields [{}] /SigFlags 3 >>", if self.split { "8 0 R" } else { "4 0 R" }));
            let page = "BT /F1 14 Tf 72 720 Td (Contract text) Tj ET";
            objs.insert(6, format!("<< /Length {} >>\nstream\n{page}\nendstream", page.len()));
            objs.insert(7, "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string());
            let widget = format!("/Type /Annot /Subtype /Widget /F {} /Rect {} /P 3 0 R /AP << /N 10 0 R >>", self.flags, self.rect);
            if self.split {
                objs.insert(8, "<< /FT /Sig /T (Signature1) /V 9 0 R /Kids [4 0 R] >>".to_string());
                objs.insert(4, format!("<< {widget} /Parent 8 0 R >>"));
            } else {
                objs.insert(4, format!("<< /FT /Sig /T (Signature1) /V 9 0 R {widget} >>"));
            }
            objs.insert(9, "<< /Type /Sig /SubFilter /ETSI.CAdES.detached /ByteRange [0 0 0 0] /Contents <00> >>".to_string());
            if self.nested {
                let wrap = 12 + self.layers.len() as u32;
                objs.insert(10, form(full, &format!("<< /XObject << /X {wrap} 0 R >> >>"), &draw("X")));
                objs.insert(wrap, form(full, &format!("<< /XObject << /{frm} 11 0 R >> >>"), &draw(frm)));
            } else if self.indirect {
                objs.insert(10, form(full, "20 0 R", &draw(frm)));
                objs.insert(20, format!("<< /XObject << /{frm} 11 0 R >> >>"));
            } else {
                objs.insert(10, form(full, &format!("<< /XObject << /{frm} 11 0 R >> >>"), &draw(frm)));
            }
            let entries = self.layers.iter().enumerate().map(|(i, l)| format!("/{} {} 0 R", l.name, 12 + i)).collect::<Vec<_>>().join(" ");
            let frm_res = if self.indirect {
                objs.insert(21, format!("<< {entries} >>"));
                "<< /XObject 21 0 R >>".to_string()
            } else {
                format!("<< /XObject << {entries} >> >>")
            };
            let ops = self.layers.iter().map(|l| format!("q {} cm /{} Do Q", nums(&l.cm), l.name)).collect::<Vec<_>>().join(" ");
            objs.insert(11, form(full, &frm_res, &ops));
            for (i, l) in self.layers.iter().enumerate() {
                objs.insert(12 + i as u32, form(l.bbox, &l.res, &l.ops));
            }
            objs.extend(self.extra.iter().cloned());
            objs
        }

        fn open(&self) -> Document {
            open(pdf(&self.objects()))
        }
    }

    /// Objects by number, as a classic cross-reference file.
    fn pdf(objs: &BTreeMap<u32, String>) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = BTreeMap::new();
        for (num, o) in objs {
            offsets.insert(*num, out.len());
            out.extend_from_slice(format!("{num} 0 obj\n{o}\nendobj\n").as_bytes());
        }
        let size = objs.keys().max().map_or(1, |m| m + 1);
        let start = out.len();
        out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for num in 1..size {
            match offsets.get(&num) {
                Some(o) => out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes()),
                None => out.extend_from_slice(b"0000000000 65535 f \n"),
            }
        }
        out.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{start}\n%%EOF").as_bytes());
        out
    }

    fn open(bytes: Vec<u8>) -> Document {
        Document::open_with_password(Arc::new(bytes), None).unwrap()
    }

    fn sig(field: &str, status: Status) -> SignatureInfo {
        SignatureInfo {
            field: field.into(),
            signed: true,
            page: Some(0),
            rect: Some([200.0, 600.0, 400.0, 720.0]),
            visible: true,
            signer: None,
            certificate: None,
            chain: Vec::new(),
            date: None,
            signing_time: None,
            reason: None,
            location: None,
            contact: None,
            sub_filter: None,
            doc_timestamp: false,
            certify: None,
            revision: 1,
            signed_len: 0,
            digest: None,
            algorithm: None,
            timestamp: false,
            timestamp_time: None,
            status,
            modification: crate::pdf::Modification::None,
            verdict_shown: false,
            details: Vec::new(),
        }
    }

    /// Run the regeneration for one signature called `Signature1`.
    fn show(doc: &mut Document, status: Status) -> SignatureInfo {
        let mut sigs = vec![sig("Signature1", status)];
        show_status_layers(doc, &mut sigs, &StatusCaptions::default());
        sigs.remove(0)
    }

    fn widget_n(doc: &Document, widget: u32) -> ObjRef {
        let w = doc.get(ObjRef::new(widget, 0));
        let ap = doc.resolve(w.as_dict().unwrap().get(b"AP").unwrap());
        ap.as_dict().unwrap().get(b"N").unwrap().as_ref().unwrap()
    }

    fn xobjects_of(doc: &Document, r: ObjRef) -> Dict {
        let o = doc.get(r);
        let Object::Stream(s) = &*o else { panic!("{r} is not a stream") };
        let res = doc.resolve(s.dict.get(b"Resources").unwrap());
        doc.resolve(res.as_dict().unwrap().get(b"XObject").unwrap()).as_dict().unwrap().clone()
    }

    /// What layer `name` of the layer form called `frm` holds, as the widget now draws it.
    fn layer_ref(doc: &Document, widget: u32, frm: &[u8], name: &[u8]) -> Option<ObjRef> {
        let form = xobjects_of(doc, widget_n(doc, widget)).get(frm)?.as_ref()?;
        xobjects_of(doc, form).get(name)?.as_ref()
    }

    fn decoded(doc: &Document, r: ObjRef) -> Vec<u8> {
        match &*doc.get(r) {
            Object::Stream(s) => s.decoded().unwrap(),
            _ => panic!("{r} is not a stream"),
        }
    }

    fn drawn(doc: &Document, widget: u32, name: &[u8]) -> String {
        String::from_utf8_lossy(&decoded(doc, layer_ref(doc, widget, b"FRM", name).unwrap())).into_owned()
    }

    #[test]
    fn named_layer_rule_table() {
        // The layouts the rule was checked against, and whether their status layers are redrawn.
        // Tightening the rule to the name /FRM fails the FRM1 and Foo rows.
        let five = Fixture::layered();
        let image_n1 = Fixture {
            layers: vec![NamedLayer { res: "<< /XObject << /img1 40 0 R >> >>".into(), ops: "q 100 0 0 100 0 0 cm /img1 Do Q".into(), ..layer("n1") }],
            frm: "FRM1",
            extra: vec![(
                40,
                "<< /Type /XObject /Subtype /Image /Width 4 /Height 4 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 16 >>\nstream\n0123456789abcdef\nendstream".into(),
            )],
            ..Fixture::layered()
        };
        let flat = five.clone().with(|f| f.extra = vec![(10, form([0.0, 0.0, W, H], HELV, SIGNER))]);
        let cases: Vec<(&str, Fixture, bool)> = vec![
            ("n0 to n4", five.clone(), true),
            ("n0 to n4, invalid", five.clone(), true),
            ("indirect resources, split widget", five.clone().with(|f| (f.indirect, f.split) = (true, true)), true),
            ("n0 and n2 only", Fixture::named(&["n0", "n2"]), false),
            ("flat /N", flat, false),
            ("artwork in n1", five.clone().with(|f| f.layers[1].ops = ARTWORK.into()), true),
            ("no n4", Fixture::named(&["n0", "n1", "n2", "n3"]), true),
            ("n4 without n1 or n3", Fixture::named(&["n0", "n2", "n4"]), false),
            ("no n2", Fixture::named(&["n0", "n1", "n3", "n4"]), true),
            ("an extra n5", Fixture::named(&["n0", "n1", "n2", "n3", "n4", "n5"]), true),
            ("layer form named FRM1", five.clone().with(|f| f.frm = "FRM1"), true),
            ("layer form one level deeper", five.clone().with(|f| f.nested = true), false),
            ("n1 without n3", Fixture::named(&["n0", "n1", "n2", "n4"]), false),
            ("n3 without n1", Fixture::named(&["n0", "n2", "n3", "n4"]), false),
            ("layer form named Foo", five.clone().with(|f| f.frm = "Foo"), true),
            ("image in n1, no n3", image_n1, false),
        ];
        assert_eq!(cases.len(), 16);
        for (name, fixture, redrawn) in cases {
            let doc = fixture.open();
            let status = if name.ends_with("invalid") { Status::Invalid } else { Status::Valid };
            assert_eq!(has_status_layers(&doc, &[sig("Signature1", status)]), redrawn, "{name}");
        }
    }

    #[test]
    fn a_valid_signature_draws_the_check_and_the_valid_caption() {
        let mut doc = Fixture::layered().open();
        assert!(show(&mut doc, Status::Valid).verdict_shown);
        let n1 = drawn(&doc, 4, b"n1");
        assert!(n1.contains(CIRCLE_CHECK) && n1.contains("0.1765 0.6157 0.4706 RG"), "{n1}");
        assert!(!n1.contains("1 0.84 0.2 rg"), "the stored question mark is gone: {n1}");
        assert_eq!(drawn(&doc, 4, b"n3"), "");
        assert!(drawn(&doc, 4, b"n4").contains("(Signature valid) Tj"));
    }

    #[test]
    fn an_unknown_signature_draws_the_alert_and_the_unknown_caption() {
        let mut doc = Fixture::layered().open();
        assert!(show(&mut doc, Status::Unknown).verdict_shown);
        let n1 = drawn(&doc, 4, b"n1");
        assert!(n1.contains(TRIANGLE_ALERT) && n1.contains("0.902 0.5255 0.098 RG"), "{n1}");
        assert_eq!(drawn(&doc, 4, b"n3"), "");
        let n4 = drawn(&doc, 4, b"n4");
        assert!(n4.contains("(Validity unknown) Tj") && !n4.contains("Not Verified"), "{n4}");
    }

    #[test]
    fn an_invalid_signature_draws_the_cross_in_n3_and_the_invalid_caption() {
        let mut doc = Fixture::layered().open();
        assert!(show(&mut doc, Status::Invalid).verdict_shown);
        assert_eq!(drawn(&doc, 4, b"n1"), "");
        let n3 = drawn(&doc, 4, b"n3");
        assert!(n3.contains(CIRCLE_X) && n3.contains("0.8431 0.2157 0.2471 RG"), "{n3}");
        assert!(drawn(&doc, 4, b"n4").contains("(Signature invalid) Tj"));
    }

    #[test]
    fn the_icon_fills_the_layer_square_flipped_into_pdf_space() {
        // n1 is 100×100 units: 24 Lucide units scale by 100/24 and y flips about the top edge.
        let mut doc = Fixture::layered().open();
        show(&mut doc, Status::Valid);
        assert!(drawn(&doc, 4, b"n1").contains("4.1667 0 0 -4.1667 0 100 cm"), "{}", drawn(&doc, 4, b"n1"));
    }

    #[test]
    fn the_originals_are_untouched_and_the_widget_points_at_copies() {
        let mut doc = Fixture::layered().open();
        let before: Vec<(u32, Arc<Object>)> = doc.object_numbers().into_iter().map(|n| (n, doc.get(ObjRef::new(n, 0)))).collect();
        show(&mut doc, Status::Valid);
        for (n, o) in &before {
            if *n != 4 {
                assert_eq!(&doc.get(ObjRef::new(*n, 0)), o, "object {n} changed");
            }
        }
        assert!(before.iter().all(|(n, _)| *n != widget_n(&doc, 4).num), "the widget draws a new /N");
    }

    #[test]
    fn other_layers_keep_their_original_objects() {
        let mut doc = Fixture::named(&["n0", "n1", "n2", "n3", "n4", "n5"]).open();
        assert!(show(&mut doc, Status::Valid).verdict_shown);
        assert_ne!(layer_ref(&doc, 4, b"FRM", b"n1"), Some(ObjRef::new(13, 0)), "n1 is regenerated");
        for (name, num) in [(&b"n0"[..], 12), (b"n2", 14), (b"n5", 17)] {
            assert_eq!(layer_ref(&doc, 4, b"FRM", name), Some(ObjRef::new(num, 0)), "{}", String::from_utf8_lossy(name));
        }
    }

    #[test]
    fn without_n4_the_icon_still_draws() {
        let mut doc = Fixture::named(&["n0", "n1", "n2", "n3"]).open();
        assert!(show(&mut doc, Status::Valid).verdict_shown);
        assert!(drawn(&doc, 4, b"n1").contains(CIRCLE_CHECK));
        assert_eq!(layer_ref(&doc, 4, b"FRM", b"n4"), None);
    }

    #[test]
    fn copies_keep_the_encoding_of_what_they_copy() {
        // /N and the layer form are copied raw, filters and all, rather than re-encoded.
        let fixture = Fixture::layered();
        let mut doc = fixture.open();
        for n in [10, 11] {
            let r = ObjRef::new(n, 0);
            let Object::Stream(s) = &*doc.get(r) else { panic!() };
            doc.set(r, Stream::flate(s.dict.clone(), &s.decoded().unwrap()));
        }
        let doc = &mut doc;
        show(doc, Status::Valid);
        let new_n = widget_n(doc, 4);
        let Object::Stream(s) = &*doc.get(new_n) else { panic!() };
        assert_eq!(s.dict.name(b"Filter"), Some(&b"FlateDecode"[..]));
        assert_eq!(s.decoded().unwrap(), b"q 1 0 0 1 0 0 cm /FRM Do Q");
        assert!(drawn(doc, 4, b"n1").contains(CIRCLE_CHECK));
    }

    #[test]
    fn indirect_resources_and_a_split_widget_are_followed() {
        let mut doc = Fixture::layered().with(|f| (f.indirect, f.split) = (true, true)).open();
        assert!(show(&mut doc, Status::Valid).verdict_shown);
        assert!(drawn(&doc, 4, b"n1").contains(CIRCLE_CHECK));
        // The shared resource dictionaries were copied, not edited.
        assert_eq!(doc.get(ObjRef::new(21, 0)).as_dict().unwrap().get(b"n1"), Some(&Object::Ref(ObjRef::new(13, 0))));
    }

    #[test]
    fn two_fields_sharing_a_layer_form_each_show_their_own_state() {
        // Field B's widget (30) draws its own /N (31), which draws the same layer form as A's.
        let fixture = Fixture::layered().with(|f| {
            f.extra = vec![
                (5, "<< /Fields [4 0 R 30 0 R] /SigFlags 3 >>".into()),
                (30, "<< /FT /Sig /T (Signature2) /V 9 0 R /Type /Annot /Subtype /Widget /F 132 /Rect [200 400 400 520] /P 3 0 R /AP << /N 31 0 R >> >>".into()),
                (31, form([0.0, 0.0, W, H], "<< /XObject << /FRM 11 0 R >> >>", "/FRM Do")),
            ]
        });
        let mut doc = fixture.open();
        let mut sigs = vec![sig("Signature1", Status::Valid), sig("Signature2", Status::Unknown)];
        assert!(show_status_layers(&mut doc, &mut sigs, &StatusCaptions::default()));
        assert!(sigs.iter().all(|s| s.verdict_shown));
        assert!(drawn(&doc, 4, b"n1").contains(CIRCLE_CHECK));
        assert!(drawn(&doc, 30, b"n1").contains(TRIANGLE_ALERT));
        assert_eq!(decoded(&doc, ObjRef::new(13, 0)), QUESTION.as_bytes(), "the shared n1 is unchanged");
    }

    #[test]
    fn visible_siblings_of_an_invisible_first_widget_are_regenerated() {
        let mut doc = Fixture::layered()
            .with(|f| {
                f.split = true;
                f.rect = "[0 0 0 0]";
                f.extra = vec![
                    (8, "<< /FT /Sig /T (Signature1) /V 9 0 R /Kids [4 0 R 32 0 R] >>".into()),
                    (32, "<< /Subtype /Widget /F 4 /Rect [200 400 400 520] /AP << /N 10 0 R >> /Parent 8 0 R >>".into()),
                ];
            })
            .open();
        let mut signatures = vec![sig("Signature1", Status::Valid)];
        signatures[0].visible = false; // SignatureInfo describes the first widget only.
        assert!(has_status_layers(&doc, &signatures));
        assert!(show_status_layers(&mut doc, &mut signatures, &StatusCaptions::default()));
        assert!(signatures[0].verdict_shown);
        assert_eq!(widget_n(&doc, 4), ObjRef::new(10, 0));
        assert!(drawn(&doc, 32, b"n1").contains(CIRCLE_CHECK));
    }

    #[test]
    fn shared_appearances_are_copied_once_however_many_widgets_draw_them() {
        // 24 widgets sharing one /N whose layer form holds a megabyte: copied per widget, the
        // display copy would grow by 24 MiB; shared, by one copy per state.
        let mut doc = Fixture::layered().with(|f| f.split = true).open();
        let Object::Stream(mut large) = (*doc.get(ObjRef::new(11, 0))).clone() else { panic!() };
        let mut ops = large.decoded().unwrap();
        ops.resize(1 << 20, b' ');
        large = Stream::from_raw(large.dict, ops);
        doc.set(ObjRef::new(11, 0), large);
        let widget = (*doc.get(ObjRef::new(4, 0))).clone();
        let kids: Vec<ObjRef> = (0..24).map(|_| doc.add(widget.clone())).collect();
        doc.update_dict(ObjRef::new(8, 0), |f| f.set(b"Kids".to_vec(), kids.iter().map(|k| Object::Ref(*k)).collect::<Vec<_>>())).unwrap();
        let before = pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap().len();
        let mut signatures = vec![sig("Signature1", Status::Valid)];
        assert!(show_status_layers(&mut doc, &mut signatures, &StatusCaptions::default()));
        assert!(signatures[0].verdict_shown, "every widget shows the state");
        let copy = widget_n(&doc, kids[0].num);
        assert_ne!(copy, ObjRef::new(10, 0));
        assert!(kids.iter().all(|k| widget_n(&doc, k.num) == copy), "the widgets share one copy");
        let after = pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap().len();
        assert!(after - before < (1 << 20) + (64 << 10), "the display copy grew by {} bytes", after - before);
    }

    #[test]
    fn distinct_appearances_share_their_layer_forms_copy() {
        // 24 widgets, each with an /N of its own, all drawing one layer form that holds a megabyte.
        let mut doc = Fixture::layered().with(|f| f.split = true).open();
        let Object::Stream(mut large) = (*doc.get(ObjRef::new(11, 0))).clone() else { panic!() };
        let mut ops = large.decoded().unwrap();
        ops.resize(1 << 20, b' ');
        large = Stream::from_raw(large.dict, ops);
        doc.set(ObjRef::new(11, 0), large);
        let n = (*doc.get(ObjRef::new(10, 0))).clone();
        let Object::Dict(widget) = (*doc.get(ObjRef::new(4, 0))).clone() else { panic!() };
        let kids: Vec<ObjRef> = (0..24)
            .map(|_| {
                let mut w = widget.clone();
                let mut ap = Dict::new();
                ap.set(b"N".to_vec(), doc.add(n.clone()));
                w.set(b"AP".to_vec(), ap);
                doc.add(w)
            })
            .collect();
        doc.update_dict(ObjRef::new(8, 0), |f| f.set(b"Kids".to_vec(), kids.iter().map(|k| Object::Ref(*k)).collect::<Vec<_>>())).unwrap();
        let before = pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap().len();
        let mut signatures = vec![sig("Signature1", Status::Valid)];
        assert!(show_status_layers(&mut doc, &mut signatures, &StatusCaptions::default()));
        assert!(signatures[0].verdict_shown);
        let after = pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions::default()).unwrap().len();
        assert!(after - before < (1 << 20) + (256 << 10), "the display copy grew by {} bytes", after - before);
    }

    #[test]
    fn two_fields_sharing_one_appearance_each_show_their_own_state() {
        // The copy is per state as well as per original, or the second field would get the
        // first field's copy.
        let fixture = Fixture::layered().with(|f| {
            f.extra = vec![
                (5, "<< /Fields [4 0 R 30 0 R] /SigFlags 3 >>".into()),
                (30, "<< /FT /Sig /T (Signature2) /V 9 0 R /Type /Annot /Subtype /Widget /F 132 /Rect [200 400 400 520] /P 3 0 R /AP << /N 10 0 R >> >>".into()),
            ]
        });
        let mut doc = fixture.open();
        let mut sigs = vec![sig("Signature1", Status::Valid), sig("Signature2", Status::Invalid)];
        show_status_layers(&mut doc, &mut sigs, &StatusCaptions::default());
        assert!(drawn(&doc, 4, b"n1").contains(CIRCLE_CHECK));
        assert!(drawn(&doc, 30, b"n3").contains(CIRCLE_X));
    }

    #[test]
    fn references_that_differ_only_in_generation_share_a_copy() {
        // Objects are found by number alone, so `10 1 R` is object 10, and so is its copy.
        let fixture = Fixture::layered().with(|f| {
            f.split = true;
            f.extra = vec![
                (8, "<< /FT /Sig /T (Signature1) /V 9 0 R /Kids [4 0 R 32 0 R] >>".into()),
                (32, "<< /Type /Annot /Subtype /Widget /F 132 /Rect [200 400 400 520] /P 3 0 R /AP << /N 10 1 R >> /Parent 8 0 R >>".into()),
            ]
        });
        let mut doc = fixture.open();
        assert!(show(&mut doc, Status::Valid).verdict_shown);
        assert_eq!(widget_n(&doc, 4), widget_n(&doc, 32));
    }

    #[test]
    fn a_widget_two_fields_claim_under_different_generations_is_still_shared() {
        let fixture = Fixture::layered().with(|f| {
            f.split = true;
            f.extra =
                vec![(5, "<< /Fields [8 0 R 33 0 R] /SigFlags 3 >>".into()), (33, "<< /FT /Sig /T (Signature2) /V 9 0 R /Kids [4 1 R] >>".into())]
        });
        let mut doc = fixture.open();
        let mut sigs = vec![sig("Signature1", Status::Valid), sig("Signature2", Status::Invalid)];
        show_status_layers(&mut doc, &mut sigs, &StatusCaptions::default());
        assert_eq!(widget_n(&doc, 4), ObjRef::new(10, 0));
        assert!(sigs.iter().all(|s| !s.verdict_shown));
    }

    #[test]
    fn a_document_timestamp_never_draws_over_a_field_of_its_name() {
        // A timestamp's entry carries its own verdict under a name a field may also have.
        let mut doc = Fixture::layered().open();
        let mut timestamp = sig("Signature1", Status::Valid);
        (timestamp.doc_timestamp, timestamp.visible, timestamp.page, timestamp.rect) = (true, false, None, None);
        let mut sigs = vec![sig("Signature1", Status::Invalid), timestamp];
        assert!(show_status_layers(&mut doc, &mut sigs, &StatusCaptions::default()));
        assert!(drawn(&doc, 4, b"n3").contains(CIRCLE_X), "the field keeps its own state");
        assert!(sigs[0].verdict_shown && !sigs[1].verdict_shown);
    }

    #[test]
    fn an_appearance_drawing_many_xobjects_is_not_recognised() {
        let names: String = (0..MAX_DRAWN_NAMES).map(|i| format!("/X{i} 12 0 R ")).collect();
        let ops: String = (0..MAX_DRAWN_NAMES).map(|i| format!("/X{i} Do ")).collect();
        let n = form([0.0, 0.0, W, H], &format!("<< /XObject << /FRM 11 0 R {names}>> >>"), &format!("/FRM Do {ops}"));
        let mut doc = Fixture::layered().with(|f| f.extra = vec![(10, n)]).open();
        assert!(!show(&mut doc, Status::Valid).verdict_shown);
        assert_eq!(widget_n(&doc, 4), ObjRef::new(10, 0));
    }

    #[test]
    fn two_widgets_sharing_an_appearance_both_show_the_state() {
        let fixture = Fixture::layered().with(|f| {
            f.split = true;
            f.extra = vec![
                (8, "<< /FT /Sig /T (Signature1) /V 9 0 R /Kids [4 0 R 32 0 R] >>".into()),
                (32, "<< /Type /Annot /Subtype /Widget /F 132 /Rect [200 400 400 520] /P 3 0 R /AP << /N 10 0 R >> /Parent 8 0 R >>".into()),
            ]
        });
        let mut doc = fixture.open();
        assert!(show(&mut doc, Status::Valid).verdict_shown);
        // One field, one state: the two widgets draw the same copy.
        assert_eq!(widget_n(&doc, 4), widget_n(&doc, 32));
        assert_ne!(widget_n(&doc, 4), ObjRef::new(10, 0));
        for w in [4, 32] {
            assert!(drawn(&doc, w, b"n1").contains(CIRCLE_CHECK), "widget {w}");
        }
    }

    #[test]
    fn unrecognised_or_unusable_widgets_are_left_alone() {
        let five = Fixture::layered();
        // With a /BBox, so the Form check is what refuses it.
        let image = "<< /Type /XObject /Subtype /Image /BBox [0 0 100 100] /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n0\nendstream";
        let cases: Vec<(&str, Fixture)> = vec![
            ("hidden", five.clone().with(|f| f.flags = 132 | 2)),
            ("no view", five.clone().with(|f| f.flags = 132 | 32)),
            ("zero-width rect", five.clone().with(|f| f.rect = "[200 600 200 720]")),
            ("rect with a non-number", five.clone().with(|f| f.rect = "[200 600 (x) 720]")),
            ("three-number rect", five.clone().with(|f| f.rect = "[200 600 400]")),
            ("state dictionary /N", five.clone().with(|f| f.extra = vec![(4, "<< /FT /Sig /T (Signature1) /V 9 0 R /Type /Annot /Subtype /Widget /F 132 /Rect [200 600 400 720] /P 3 0 R /AP << /N << /On 10 0 R >> >> >>".into())])),
            ("n1 an image", five.clone().with(|f| f.extra = vec![(13, image.into())])),
            ("n3 an image", five.clone().with(|f| f.extra = vec![(15, image.into())])),
            ("n4 an image", five.clone().with(|f| f.extra = vec![(16, image.into())])),
            ("two qualifying forms", five.clone().with(|f| f.extra = vec![(10, form([0.0, 0.0, W, H], "<< /XObject << /FRM 11 0 R /FRM2 11 0 R >> >>", "/FRM Do /FRM2 Do"))])),
            ("n1 bbox of zeros", five.clone().with(|f| f.layers[1].bbox = [0.0; 4])),
            ("n1 bbox giant", five.clone().with(|f| f.layers[1].bbox = [0.0, 0.0, 1e9, 1e9])),
            ("n3 bbox missing", five.clone().with(|f| f.extra = vec![(15, "<< /Type /XObject /Subtype /Form /Resources << >> /Length 0 >>\nstream\n\nendstream".into())])),
        ];
        for (name, fixture) in cases {
            let mut doc = fixture.open();
            let before = doc.get(ObjRef::new(4, 0));
            let shown = show(&mut doc, Status::Valid);
            assert!(!shown.verdict_shown, "{name}");
            assert_eq!(doc.get(ObjRef::new(4, 0)), before, "{name}");
            assert!(!has_status_layers(&doc, &[sig("Signature1", Status::Valid)]), "{name}");
        }
    }

    #[test]
    fn a_drawn_form_holding_n1_alone_does_not_count_as_a_second_layer_form() {
        // Only a form holding both n1 and n3 qualifies, so /FRM is still the one layer form
        // when /N also draws one that holds n1 alone.
        let fixture = Fixture::layered().with(|f| {
            f.extra = vec![
                (10, form([0.0, 0.0, W, H], "<< /XObject << /FRM 11 0 R /Other 35 0 R >> >>", "/FRM Do /Other Do")),
                (35, form([0.0, 0.0, W, H], "<< /XObject << /n1 13 0 R >> >>", "/n1 Do")),
            ]
        });
        let mut doc = fixture.open();
        assert!(show(&mut doc, Status::Valid).verdict_shown);
        assert!(drawn(&doc, 4, b"n1").contains(CIRCLE_CHECK));
    }

    #[test]
    fn a_verdict_counts_as_shown_only_when_every_visible_widget_shows_it() {
        // Widget 32 draws its text flat, so it keeps its artwork and the field keeps its badge;
        // hidden, it no longer counts.
        let two = |flags: u32| {
            Fixture::layered().with(|f| {
                f.split = true;
                f.extra = vec![
                    (8, "<< /FT /Sig /T (Signature1) /V 9 0 R /Kids [4 0 R 32 0 R] >>".into()),
                    (
                        32,
                        format!("<< /Type /Annot /Subtype /Widget /F {flags} /Rect [200 400 400 520] /P 3 0 R /AP << /N 36 0 R >> /Parent 8 0 R >>"),
                    ),
                    (36, form([0.0, 0.0, W, H], HELV, SIGNER)),
                ]
            })
        };
        let mut doc = two(132).open();
        assert!(!show(&mut doc, Status::Valid).verdict_shown);
        assert!(drawn(&doc, 4, b"n1").contains(CIRCLE_CHECK), "the recognised widget is still drawn");
        let mut doc = two(132 | 2).open();
        assert!(show(&mut doc, Status::Valid).verdict_shown);
    }

    #[test]
    fn unsigned_signatures_are_left_alone() {
        let mut doc = Fixture::layered().open();
        let mut sigs = vec![sig("Signature1", Status::Valid)];
        sigs[0].signed = false;
        assert!(!has_status_layers(&doc, &sigs));
        assert!(!show_status_layers(&mut doc, &mut sigs, &StatusCaptions::default()));
        assert_eq!(widget_n(&doc, 4), ObjRef::new(10, 0));
    }

    #[test]
    fn a_widget_two_fields_claim_or_a_name_two_fields_share_is_left_alone() {
        let claimed = Fixture::layered().with(|f| {
            f.split = true;
            f.extra =
                vec![(5, "<< /Fields [8 0 R 33 0 R] /SigFlags 3 >>".into()), (33, "<< /FT /Sig /T (Signature2) /V 9 0 R /Kids [4 0 R] >>".into())]
        });
        let twins = Fixture::layered().with(|f| {
            f.extra = vec![
                (5, "<< /Fields [4 0 R 30 0 R] /SigFlags 3 >>".into()),
                (30, "<< /FT /Sig /T (Signature1) /V 9 0 R /Type /Annot /Subtype /Widget /F 132 /Rect [200 400 400 520] /P 3 0 R /AP << /N 10 0 R >> >>".into()),
            ]
        });
        for (name, fixture) in [("claimed", claimed), ("twins", twins)] {
            let mut doc = fixture.open();
            let mut sigs = vec![sig("Signature1", Status::Valid), sig("Signature2", Status::Invalid)];
            show_status_layers(&mut doc, &mut sigs, &StatusCaptions::default());
            assert_eq!(widget_n(&doc, 4), ObjRef::new(10, 0), "{name}");
            assert!(sigs.iter().all(|s| !s.verdict_shown), "{name}");
        }
    }

    #[test]
    fn cyclic_or_huge_field_trees_finish() {
        // Kids pointing back at their parent, and a chain of nested fields thousands deep.
        let cyclic = Fixture::layered().with(|f| {
            f.split = true;
            f.extra = vec![
                (8, "<< /FT /Sig /T (Signature1) /V 9 0 R /Kids [4 0 R 8 0 R 34 0 R] >>".into()),
                (34, "<< /T (loop) /Kids [8 0 R 34 0 R] >>".into()),
            ]
        });
        let mut doc = cyclic.open();
        assert!(show(&mut doc, Status::Valid).verdict_shown);
        let deep = Fixture::layered().with(|f| {
            let depth = 2_000u32;
            f.extra = (0..depth).map(|i| (100 + i, format!("<< /T (d) /Kids [{} 0 R] >>", 101 + i))).collect();
            f.extra.push((5, "<< /Fields [4 0 R 100 0 R] >>".into()));
        });
        let mut doc = deep.open();
        assert!(show(&mut doc, Status::Valid).verdict_shown);
    }

    fn captions(valid: &str, unknown: &str, invalid: &str) -> StatusCaptions {
        StatusCaptions { valid: valid.into(), unknown: unknown.into(), invalid: invalid.into() }
    }

    /// The caption layer as drawn for `status` with `captions`: the bytes all its `Tj`s show,
    /// the text its span gives for copy and search (if any), its font (none when it is drawn as
    /// outlines), its operators, and the document.
    fn caption_of(captions: &StatusCaptions, status: Status) -> (Vec<u8>, Option<String>, Option<Dict>, Vec<pdfcraft_content::Op>, Document) {
        let mut doc = Fixture::layered().open();
        let mut sigs = vec![sig("Signature1", status)];
        show_status_layers(&mut doc, &mut sigs, captions);
        let n4 = layer_ref(&doc, 4, b"FRM", b"n4").unwrap();
        let Object::Stream(s) = &*doc.get(n4) else { panic!() };
        let font = s
            .dict
            .get(b"Resources")
            .and_then(|r| r.as_dict())
            .and_then(|r| r.get(b"Font"))
            .and_then(|f| f.as_dict())
            .and_then(|f| f.get(b"Cap"))
            .and_then(|f| doc.dict(f));
        let ops = pdfcraft_content::parse(&decoded(&doc, n4)).ops;
        let shown = ops
            .iter()
            .filter(|op| op.is("Tj"))
            .filter_map(|op| op.operands.first().and_then(Object::as_string))
            .flat_map(|s| s.bytes.clone())
            .collect();
        let actual = ops
            .iter()
            .find(|op| op.is("BDC"))
            .and_then(|op| op.operands.get(1).and_then(Object::as_dict).and_then(|d| d.get(b"ActualText")).and_then(Object::as_string))
            .map(PdfString::to_text);
        (shown, actual, font, ops, doc)
    }

    #[test]
    fn captions_winansi_encodes_are_drawn_in_helvetica() {
        let german = captions("Signatur gültig", "Gültigkeit unbekannt", "Signatur ungültig");
        let (shown, _, font, _, _) = caption_of(&german, Status::Valid);
        assert_eq!(font.unwrap().name(b"BaseFont"), Some(&b"Helvetica"[..]));
        assert_eq!(shown, win_ansi("Signatur gültig"));
    }

    #[test]
    fn captions_beyond_winansi_are_shaped_in_embedded_inter() {
        let russian = captions("Подпись действительна", "Действительность неизвестна", "Подпись недействительна");
        let (shown, actual, font, _, doc) = caption_of(&russian, Status::Unknown);
        let font = font.unwrap();
        assert_eq!(font.name(b"Subtype"), Some(&b"Type0"[..]));
        assert_eq!(font.name(b"BaseFont"), Some(&b"Inter"[..]));
        assert_eq!(actual.as_deref(), Some("Действительность неизвестна"));
        assert_eq!(pdfcraft_fonts::pdf::Metrics::from_dict(&doc, &font).decode(&shown), "Действительность неизвестна");
    }

    #[test]
    fn captions_in_other_scripts_use_a_face_made_for_them_or_english() {
        // Arabic, Japanese and Telugu are shaped in a craft-fonts face meant for the script and
        // drawn as outlines when the build has one, and drawn in English when it does not.
        // Chinese is always English: no face is picked for it, since the Japanese ones would draw
        // Japanese letterforms and the Chinese one is kept out of documents.
        let cases = [
            ("Arab", ["التوقيع صالح", "الصلاحية غير معروفة", "التوقيع غير صالح"]),
            ("Jpan", ["署名は有効", "有効性は不明", "署名は無効"]),
            ("Telu", ["సంతకం చెల్లుబాటు అవుతుంది", "చెల్లుబాటు తెలియదు", "సంతకం చెల్లదు"]),
            ("Hani", ["签名有效", "有效性未知", "签名无效"]),
        ];
        for (script, [valid, unknown, invalid]) in cases {
            assert_eq!(script_of(&[valid, unknown, invalid]), Some(script));
            let (shown, actual, font, ops, _) = caption_of(&captions(valid, unknown, invalid), Status::Valid);
            if face_for(Some(script), &[valid, unknown, invalid]).is_some() {
                assert!(shown.is_empty() && font.is_none(), "{script}: outlines, not text");
                assert!(ops.iter().filter(|op| op.is("l")).count() > 20 && ops.iter().any(|op| op.is("f")), "{script}");
                assert_eq!(actual.as_deref(), Some(valid), "{script}");
            } else {
                assert_eq!(shown, b"Signature valid", "{script}");
                assert_eq!(font.unwrap().name(b"BaseFont"), Some(&b"Helvetica"[..]), "{script}");
            }
        }
        assert!(face_for(Some("Hani"), &["签名有效"]).is_none());
    }

    #[test]
    fn right_to_left_captions_end_at_the_right() {
        let bbox = [0.0, 84.0, 200.0, 120.0];
        assert!((caption_x(&bbox, 50.0, true) - 144.0).abs() < 1e-9);
        assert!((caption_x(&bbox, 50.0, false) - 6.0).abs() < 1e-9);
    }

    #[test]
    fn captions_no_font_draws_are_all_drawn_in_english() {
        // One caption no face can draw sends all three to English, so two signatures in
        // different states never read in two languages.
        let mixed = captions("签名有效", "Действительность неизвестна", "Подпись недействительна");
        for (status, english) in [(Status::Valid, "Signature valid"), (Status::Unknown, "Validity unknown")] {
            let (shown, _, font, _, _) = caption_of(&mixed, status);
            assert_eq!(shown, english.as_bytes());
            assert_eq!(font.unwrap().name(b"BaseFont"), Some(&b"Helvetica"[..]));
        }
    }

    #[test]
    fn the_font_program_is_embedded_once_per_call() {
        let fixture = Fixture::layered().with(|f| {
            f.extra = vec![
                (5, "<< /Fields [4 0 R 30 0 R] /SigFlags 3 >>".into()),
                (30, "<< /FT /Sig /T (Signature2) /V 9 0 R /Type /Annot /Subtype /Widget /F 132 /Rect [200 400 400 520] /P 3 0 R /AP << /N 31 0 R >> >>".into()),
                (31, form([0.0, 0.0, W, H], "<< /XObject << /FRM 11 0 R >> >>", "/FRM Do")),
            ]
        });
        let mut doc = fixture.open();
        let russian = captions("Подпись действительна", "Действительность неизвестна", "Подпись недействительна");
        let mut sigs = vec![sig("Signature1", Status::Valid), sig("Signature2", Status::Invalid)];
        show_status_layers(&mut doc, &mut sigs, &russian);
        let programs = doc
            .object_numbers()
            .into_iter()
            .filter(|n| matches!(&*doc.get(ObjRef::new(*n, 0)), Object::Stream(s) if s.dict.contains(b"PCFullFont")))
            .count();
        assert_eq!(programs, 1, "both states' captions share one embedded Inter");
    }

    #[test]
    fn a_caption_too_small_to_read_leaves_n4_blank() {
        let mut doc = Fixture::layered().with(|f| f.layers[4].bbox = [0.0, 0.0, 200.0, 1.0]).open();
        assert!(show(&mut doc, Status::Valid).verdict_shown);
        assert_eq!(drawn(&doc, 4, b"n4"), "");
        assert!(drawn(&doc, 4, b"n1").contains(CIRCLE_CHECK));
    }

    #[test]
    fn random_resource_graphs_never_panic() {
        // A seeded walk over appearance graphs built from a small pool of objects that point at
        // each other at random: cycles, self references, wrong types and missing entries.
        struct Rng(u64);
        impl Rng {
            fn below(&mut self, m: u64) -> u64 {
                self.0 ^= self.0 << 13;
                self.0 ^= self.0 >> 7;
                self.0 ^= self.0 << 17;
                self.0 % m
            }
            fn pick<'a>(&mut self, of: &[&'a str]) -> &'a str {
                of[self.below(of.len() as u64) as usize]
            }
        }
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let names = ["FRM", "n1", "n3", "n4", "X"];
        for _ in 0..300 {
            let mut extra = Vec::new();
            for num in 10..20u32 {
                let entries = (0..rng.below(4)).map(|_| format!("/{} {} 0 R", rng.pick(&names), 10 + rng.below(10))).collect::<Vec<_>>().join(" ");
                let ops = (0..rng.below(4)).map(|_| format!("/{} Do", rng.pick(&names))).collect::<Vec<_>>().join(" ");
                let bbox = rng.pick(&["[0 0 100 100]", "[0 0 0 0]", "[1 2]", "(x)", "[0 0 1e300 5]"]);
                let res = match rng.below(3) {
                    0 => format!("<< /XObject << {entries} >> >>"),
                    1 => format!("{} 0 R", 10 + rng.below(10)),
                    _ => format!("<< /XObject {} 0 R >>", 10 + rng.below(10)),
                };
                extra.push((
                    num,
                    match rng.below(4) {
                        0 => format!("<< {entries} >>"),
                        1 => "[1 2 3]".to_string(),
                        _ => format!(
                            "<< /Type /XObject /Subtype /Form /BBox {bbox} /Resources {res} /Length {} >>\nstream\n{ops}\nendstream",
                            ops.len()
                        ),
                    },
                ));
            }
            let mut doc = Fixture::layered().with(|f| f.extra = extra).open();
            let status = [Status::Valid, Status::Unknown, Status::Invalid][rng.below(3) as usize];
            let mut sigs = vec![sig("Signature1", status)];
            let _ = has_status_layers(&doc, &sigs);
            let _ = show_status_layers(&mut doc, &mut sigs, &StatusCaptions::default());
            assert!(pdfcraft_cos::write_incremental(&doc, &pdfcraft_cos::SaveOptions::default()).is_ok());
        }
    }
}
