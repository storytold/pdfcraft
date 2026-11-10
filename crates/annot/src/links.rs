//! Links (ISO 32000-2 §12.5.6.5): create, edit and remove link annotations with Acrobat's Link
//! Properties (appearance and a go-to-page or open-a-web-page action).

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

use crate::{AnnotError, annots, page_ref, page_refs, set_annots};

/// Where a link goes.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkAction {
    /// A page of this document (0-based), shown at `zoom`.
    Page {
        page: usize,
        zoom: LinkZoom,
    },
    Uri(String),
    /// An explicit destination of this document, by name (`/Dest (name)`, or a
    /// go-to whose `/D` is a name). The name is the document's own key in
    /// `/Names /Dests`, so a link to one survives the pages moving.
    Named(String),
    /// A page of another file: a remote go-to (`/S /GoToR`). `file` is the path
    /// the file gives, kept as written and never resolved; `page` is a page of
    /// that file (0-based), so page operations of this document leave it alone.
    Remote {
        file: String,
        page: usize,
        zoom: LinkZoom,
    },
    /// Something PdfCraft doesn't edit yet (a JavaScript or Launch action…): kept as is.
    Other(String),
}

/// How a go-to-page link shows its page: Acrobat's Go to a page view zoom, written as an
/// explicit destination (ISO 32000-2 §12.3.2.2, Table 149). Positions are written `null`, so the
/// reader keeps its position in that direction, as a new bookmark does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LinkZoom {
    /// `/Fit`: the whole page in the window.
    FitPage,
    /// `/FitH null`: the page's width across the window.
    FitWidth,
    /// `/FitV null`: the page's height down the window.
    FitHeight,
    /// `/FitB`: the page's content (its bounding box) in the window.
    FitVisible,
    /// `/XYZ null null null`: the reader's zoom is kept.
    Inherit,
    /// `/XYZ null null z` with z = percent / 100; 100 is Actual Size. A new zoom is written
    /// within 1–6400%; a link rewritten with the zoom it has keeps its own factor.
    Percent(f64),
}

impl LinkZoom {
    /// The zoom an explicit destination array (`[page /XYZ …]`, `[page /Fit]`, …) asks for. The
    /// boxed variants read as their unboxed ones, and a `/FitR` rectangle or a missing or unknown
    /// type as Fit Page, the view links had before they carried a zoom. A link whose page and zoom
    /// are unchanged is not rewritten, so its position is kept; one given a new page keeps how its
    /// zoom is written (`write_action`) but not its position.
    fn of_dest(doc: &Document, dest: &[Object]) -> Self {
        let kind = dest.get(1).map(|k| doc.resolve(k)).and_then(|k| k.as_name().map(<[u8]>::to_vec));
        match kind.as_deref() {
            Some(b"FitB") => LinkZoom::FitVisible,
            Some(b"FitH" | b"FitBH") => LinkZoom::FitWidth,
            Some(b"FitV" | b"FitBV") => LinkZoom::FitHeight,
            Some(b"XYZ") => match dest.get(4).and_then(|z| doc.resolve(z).as_f64()) {
                // Finite even for a factor near f64::MAX. A link given a new page with this zoom
                // keeps the factor itself, however far outside 1–6400% (`write_action`).
                Some(z) if z.is_finite() && z > 0.0 => LinkZoom::Percent((z * 100.0).min(f64::MAX)),
                // A zoom of 0 or null keeps the reader's (Table 149).
                _ => LinkZoom::Inherit,
            },
            _ => LinkZoom::FitPage,
        }
    }

    /// The destination's elements after the page reference.
    fn operands(self) -> Result<Vec<Object>, AnnotError> {
        Ok(match self {
            LinkZoom::FitPage => vec![Object::name("Fit")],
            LinkZoom::FitWidth => vec![Object::name("FitH"), Object::Null],
            LinkZoom::FitHeight => vec![Object::name("FitV"), Object::Null],
            LinkZoom::FitVisible => vec![Object::name("FitB")],
            LinkZoom::Inherit => vec![Object::name("XYZ"), Object::Null, Object::Null, Object::Null],
            LinkZoom::Percent(p) => {
                if !(p.is_finite() && p > 0.0) {
                    return Err(AnnotError::Invalid("the link's zoom must be a positive percentage".into()));
                }
                vec![Object::name("XYZ"), Object::Null, Object::Null, Object::Real((p / 100.0).clamp(0.01, 64.0))]
            }
        })
    }
}

/// Acrobat's Link Properties ▸ Appearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkStyle {
    /// A visible rectangle (`/Border` width > 0) or invisible.
    pub visible: bool,
    pub color: [f64; 3],
    /// 1 thin, 2 medium, 3 thick.
    pub width: f64,
    pub dashed: bool,
    pub underline: bool,
    /// Highlight style when clicked: None, Invert, Outline, Inset (`/H` N, I, O, P).
    pub highlight: Highlight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Highlight {
    None,
    Invert,
    Outline,
    Inset,
}

impl Highlight {
    pub const ALL: [Highlight; 4] = [Highlight::None, Highlight::Invert, Highlight::Outline, Highlight::Inset];

    fn code(self) -> &'static str {
        match self {
            Highlight::None => "N",
            Highlight::Invert => "I",
            Highlight::Outline => "O",
            Highlight::Inset => "P",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Highlight::None => "None",
            Highlight::Invert => "Invert",
            Highlight::Outline => "Outline",
            Highlight::Inset => "Inset",
        }
    }
}

impl Default for LinkStyle {
    fn default() -> Self {
        // Acrobat's defaults for a new link: invisible rectangle, invert highlight.
        LinkStyle { visible: false, color: [0.0, 0.0, 1.0], width: 1.0, dashed: false, underline: false, highlight: Highlight::Invert }
    }
}

/// A link on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkItem {
    pub page: usize,
    /// Index in the page's `/Annots`.
    pub index: usize,
    pub obj: Option<ObjRef>,
    pub rect: [f64; 4],
    /// The text areas the link draws over, when the file gives them
    /// (`/QuadPoints`); `None` for a link that is only its rectangle.
    pub quads: Option<Vec<[f64; 8]>>,
    pub action: LinkAction,
    pub style: LinkStyle,
}

/// A link as a rewrite writes it back: everything the model carries, so a
/// caller can name the whole link (the room shares a page's links as one list)
/// and the writer can skip the ones that did not change. A link whose action
/// the model cannot express is an error from [`bodies`], never an omission: a
/// rewrite that silently dropped it would lose a destination no undo brings back.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkBody {
    pub rect: [f64; 4],
    pub quads: Option<Vec<[f64; 8]>>,
    pub action: LinkAction,
    pub style: LinkStyle,
}

/// Bytes of web-address text one listing keeps, counted after conversion to UTF-8; past it a link
/// lists as [`LinkAction::Other`].
pub(crate) const MAX_URI_BYTES: usize = 16 << 20;

fn nums(doc: &Document, o: Option<&Object>) -> Vec<f64> {
    o.map(|o| doc.resolve(o)).and_then(|a| a.as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect())).unwrap_or_default()
}

fn action_of(doc: &Document, d: &Dict, pages: &[ObjRef], uri_budget: &mut usize) -> LinkAction {
    let page_view = |dest: &Object| -> Option<LinkAction> {
        let a = doc.resolve(dest);
        let a = a.as_array()?;
        let page = match a.first()? {
            Object::Ref(r) => pages.iter().position(|p| p == r),
            Object::Int(i) => usize::try_from(*i).ok(),
            _ => None,
        }?;
        Some(LinkAction::Page { page, zoom: LinkZoom::of_dest(doc, a) })
    };
    if let Some(dest) = d.get(b"Dest") {
        return page_view(dest).or_else(|| named_of(doc, dest)).unwrap_or_else(|| LinkAction::Other("destination".into()));
    }
    let Some(a) = d.get(b"A").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()) else { return LinkAction::Other("no action".into()) };
    match a.name(b"S") {
        Some(b"URI") => {
            let uri = a.get(b"URI").map(|u| doc.resolve(u));
            let bytes = uri.as_ref().and_then(|u| u.as_string()).map_or(&[][..], |s| &s.bytes[..]);
            // The raw length first, so an address already over the budget is never converted; then the
            // converted text, which is up to three times longer where the bytes are not UTF-8.
            let text = (bytes.len() <= *uri_budget).then(|| String::from_utf8_lossy(bytes));
            match text.and_then(|t| Some((uri_budget.checked_sub(t.len())?, t))) {
                Some((left, text)) => {
                    *uri_budget = left;
                    LinkAction::Uri(text.into_owned())
                }
                None => LinkAction::Other("web address too long".into()),
            }
        }
        Some(b"GoTo") => {
            a.get(b"D").and_then(|d| page_view(d).or_else(|| named_of(doc, d))).unwrap_or_else(|| LinkAction::Other("destination".into()))
        }
        // A remote go-to names a page of another file. Its page number is a number in that
        // file, never a page of this one, so page operations must leave it alone; a name in
        // it names a destination of the other file, which this model cannot carry.
        Some(b"GoToR") => {
            let Some(file) = a.get(b"F").and_then(|f| file_spec(doc, f)) else { return LinkAction::Other("remote go-to".into()) };
            let dest = a.get(b"D").map(|d| doc.resolve(d));
            let remote = match dest.as_deref() {
                Some(Object::Int(i)) => usize::try_from(*i).ok().map(|page| LinkAction::Remote { file, page, zoom: LinkZoom::Inherit }),
                Some(Object::Array(_)) => {
                    let array = dest.as_deref().and_then(Object::as_array).map(Vec::as_slice).unwrap_or_default();
                    match &*doc.resolve(array.first().unwrap_or(&Object::Null)) {
                        Object::Int(i) => usize::try_from(*i).ok().map(|page| LinkAction::Remote { file, page, zoom: LinkZoom::of_dest(doc, array) }),
                        // A name or a page reference opens something of the other file this model
                        // cannot write; it is not a page number, so it is not guessed at.
                        _ => None,
                    }
                }
                _ => None,
            };
            remote.unwrap_or_else(|| LinkAction::Other("remote destination".into()))
        }
        Some(s) => LinkAction::Other(String::from_utf8_lossy(s).into_owned()),
        None => LinkAction::Other("no action".into()),
    }
}

/// A destination named as a name object or a string, the two forms `/Dest` and
/// a go-to's `/D` take for a named destination.
fn named_of(doc: &Document, value: &Object) -> Option<LinkAction> {
    let value = doc.resolve(value);
    match &*value {
        Object::Name(n) => Some(LinkAction::Named(String::from_utf8_lossy(n).into_owned())),
        Object::String(s) => Some(LinkAction::Named(s.to_text())),
        _ => None,
    }
}

/// The path a file specification names, as written: a string, or a dictionary's
/// `/UF`, `/F`, `/Unix`, `/DOS` or `/Mac` entry, in that order. A path is never
/// resolved or opened here (the resolver is the engine's, and a remote go-to is
/// only carried).
fn file_spec(doc: &Document, value: &Object) -> Option<String> {
    let value = doc.resolve(value);
    if let Some(s) = value.as_string() {
        return Some(s.to_text());
    }
    let d = value.as_dict()?;
    [b"UF".as_slice(), b"F", b"Unix", b"DOS", b"Mac"].iter().find_map(|key| {
        let v = doc.resolve(d.get(key)?);
        v.as_string().map(|s| s.to_text()).filter(|s| !s.is_empty())
    })
}

fn style_of(doc: &Document, d: &Dict) -> LinkStyle {
    let bs = d.get(b"BS").and_then(|b| doc.resolve(b).as_dict().cloned());
    let width = match &bs {
        Some(b) => b.get(b"W").and_then(Object::as_f64).unwrap_or(1.0),
        None => nums(doc, d.get(b"Border")).get(2).copied().unwrap_or(1.0),
    };
    let s = bs.as_ref().and_then(|b| b.name(b"S").map(<[u8]>::to_vec)).unwrap_or_default();
    let c = nums(doc, d.get(b"C"));
    LinkStyle {
        visible: width > 0.0,
        color: if c.len() == 3 { [c[0], c[1], c[2]] } else { [0.0, 0.0, 1.0] },
        width: if width > 0.0 { width } else { 1.0 },
        dashed: s == b"D",
        underline: s == b"U",
        highlight: match d.name(b"H") {
            Some(b"N") => Highlight::None,
            Some(b"O") => Highlight::Outline,
            Some(b"P") => Highlight::Inset,
            _ => Highlight::Invert,
        },
    }
}

/// Every link in the document.
pub fn list(doc: &Document) -> Vec<LinkItem> {
    let Ok(pages) = page_refs(doc) else { return Vec::new() };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut uri_budget = MAX_URI_BYTES;
    for (pi, p) in pages.iter().enumerate() {
        for (k, a) in annots(doc, *p).iter().enumerate() {
            // One annotation object listed under several entries or pages is one link.
            if let Some(r) = a.as_ref()
                && !seen.insert(r)
            {
                continue;
            }
            let Some(d) = doc.resolve(a).as_dict().cloned() else { continue };
            if d.name(b"Subtype") != Some(b"Link") {
                continue;
            }
            let r = nums(doc, d.get(b"Rect"));
            if r.len() != 4 {
                continue;
            }
            out.push(LinkItem {
                page: pi,
                index: k,
                obj: a.as_ref(),
                rect: [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])],
                quads: quads_of(doc, &d),
                action: action_of(doc, &d, &pages, &mut uri_budget),
                style: style_of(doc, &d),
            });
        }
    }
    out
}

/// The `/QuadPoints` of a link: its text areas, eight numbers each (§12.5.6.6). `None`
/// when the file gives none; a malformed array is `None` here and an error from
/// [`bodies`], because a rewrite must not silently drop areas it cannot write.
fn quads_of(doc: &Document, d: &Dict) -> Option<Vec<[f64; 8]>> {
    let raw = nums(doc, d.get(b"QuadPoints"));
    if raw.is_empty() || !raw.len().is_multiple_of(8) || !raw.iter().all(|v| v.is_finite()) {
        return None;
    }
    Some(raw.as_chunks::<8>().0.iter().map(|c| [c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]]).collect())
}

fn write_style(d: &mut Dict, s: &LinkStyle) {
    let mut bs = Dict::new();
    bs.set(b"W".to_vec(), Object::Real(if s.visible { s.width.clamp(0.5, 12.0) } else { 0.0 }));
    bs.set(
        b"S".to_vec(),
        Object::name(if s.dashed {
            "D"
        } else if s.underline {
            "U"
        } else {
            "S"
        }),
    );
    if s.dashed {
        bs.set(b"D".to_vec(), Object::Array(vec![Object::Int(3)]));
    }
    d.set(b"BS".to_vec(), Object::Dict(bs));
    d.remove(b"Border");
    d.set(b"C".to_vec(), Object::Array(s.color.iter().map(|v| Object::Real(v.clamp(0.0, 1.0))).collect()));
    d.set(b"H".to_vec(), Object::name(s.highlight.code()));
}

/// Bytes of `u` as a 7-bit ASCII URI string (§12.6.4.8).
fn uri_bytes(u: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(u.len());
    for &b in u.as_bytes() {
        if (0x21..=0x7e).contains(&b) {
            out.push(b);
        } else {
            out.extend_from_slice(format!("%{b:02X}").as_bytes());
        }
    }
    out
}

/// The explicit destination array link `d` goes to now: its `/Dest`, or a GoTo action's `/D`.
fn current_dest(doc: &Document, d: &Dict) -> Option<Vec<Object>> {
    let dest = match d.get(b"Dest") {
        Some(dest) => doc.resolve(dest),
        None => {
            let action = doc.resolve(d.get(b"A")?);
            let go_to = action.as_dict().filter(|a| a.name(b"S") == Some(b"GoTo"))?;
            doc.resolve(go_to.get(b"D")?)
        }
    };
    dest.as_array().cloned()
}

fn write_action(doc: &Document, d: &mut Dict, a: &LinkAction) -> Result<(), AnnotError> {
    match a {
        LinkAction::Page { page, zoom } => {
            let target = page_ref(doc, *page)?;
            let mut operands = zoom.operands()?;
            let current = current_dest(doc, d).filter(|c| LinkZoom::of_dest(doc, c) == *zoom);
            if let Some(c) = current {
                // The page and zoom the link has (`link_edit` naming its own page): the
                // destination is left as it is, position on the page and GoTo form included.
                let same_page = match c.first() {
                    Some(Object::Ref(r)) => *r == target,
                    Some(Object::Int(i)) => usize::try_from(*i).ok() == Some(*page),
                    _ => false,
                };
                if same_page {
                    return Ok(());
                }
                // A new page with the zoom the link has keeps how that zoom is written: a boxed
                // fit (`/FitBH`, `/FitBV`) and its own `/XYZ` factor, even one outside the 1–6400%
                // a new zoom is written within. Positions belong to the old page and are dropped.
                let (slot, own) = match zoom {
                    LinkZoom::Percent(_) => (operands.get_mut(3), c.get(4)),
                    LinkZoom::FitWidth | LinkZoom::FitHeight => (operands.get_mut(0), c.get(1)),
                    _ => (None, None),
                };
                if let (Some(slot), Some(own)) = (slot, own) {
                    *slot = doc.resolve(own).as_ref().clone();
                }
            }
            let mut dest = vec![Object::Ref(target)];
            dest.extend(operands);
            d.remove(b"A");
            d.set(b"Dest".to_vec(), Object::Array(dest));
        }
        LinkAction::Uri(u) => {
            let u = u.trim();
            if u.is_empty() {
                return Err(AnnotError::Invalid("type the web address first".into()));
            }
            d.remove(b"Dest");
            let mut act = Dict::new();
            act.set(b"S".to_vec(), Object::name("URI"));
            // URIs are 7-bit ASCII strings (§12.6.4.8).
            act.set(b"URI".to_vec(), PdfString::literal(uri_bytes(u)));
            d.set(b"A".to_vec(), Object::Dict(act));
        }
        // A named destination is an explicit destination of this document that the
        // `/Names /Dests` tree resolves; a string is the form every reader takes (§12.3.2.3).
        LinkAction::Named(name) => {
            let name = name.trim();
            if name.is_empty() {
                return Err(AnnotError::Invalid("type the destination's name first".into()));
            }
            d.remove(b"A");
            d.set(b"Dest".to_vec(), Object::String(PdfString::literal(name.as_bytes().to_vec())));
        }
        // A remote go-to opens a page of another file: `/D` is a page number of that
        // file, which is written as an integer, not as a reference or a number here.
        LinkAction::Remote { file, page, zoom } => {
            let file = file.trim();
            if file.is_empty() {
                return Err(AnnotError::Invalid("name the other file first".into()));
            }
            let page = i64::try_from(*page).map_err(|_| AnnotError::Invalid("the remote page number is too large".into()))?;
            let mut dest = vec![Object::Int(page)];
            dest.extend(zoom.operands()?);
            d.remove(b"Dest");
            let mut act = Dict::new();
            act.set(b"S".to_vec(), Object::name("GoToR"));
            // A file specification is a byte string; the path travels as written.
            act.set(b"F".to_vec(), PdfString::literal(file.as_bytes().to_vec()));
            act.set(b"D".to_vec(), Object::Array(dest));
            act.set(b"NewWindow".to_vec(), Object::Bool(false));
            d.set(b"A".to_vec(), Object::Dict(act));
        }
        LinkAction::Other(_) => {}
    }
    Ok(())
}

/// Check link geometry before writing PDF objects: a user or automation call can supply
/// non-finite coordinates or a finite pair whose difference overflows to infinity.
fn checked_rect(rect: [f64; 4]) -> Result<[f64; 4], AnnotError> {
    if !rect.iter().all(|v| v.is_finite()) {
        return Err(AnnotError::Invalid("the link area must have finite coordinates".into()));
    }
    let r = [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])];
    let (width, height) = (r[2] - r[0], r[3] - r[1]);
    if !width.is_finite() || !height.is_finite() || width < 2.0 || height < 2.0 {
        return Err(AnnotError::Invalid("the link area is too small or too large".into()));
    }
    Ok(r)
}

/// The text areas of a link, bounded and normalised: eight finite numbers a quad,
/// already inside the area the link draws over.
fn checked_quads(quads: &[[f64; 8]]) -> Result<(), AnnotError> {
    if quads.is_empty() {
        return Err(AnnotError::Invalid("a text link's areas cannot be empty".into()));
    }
    if !quads.iter().flatten().all(|v| v.is_finite()) {
        return Err(AnnotError::Invalid("the link's areas must have finite coordinates".into()));
    }
    Ok(())
}

/// The area a link draws over, from its quads: the smallest rectangle that holds them.
fn quads_rect(quads: &[[f64; 8]]) -> [f64; 4] {
    let mut rect = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for quad in quads {
        for pair in quad.as_chunks::<2>().0 {
            rect[0] = rect[0].min(pair[0]);
            rect[1] = rect[1].min(pair[1]);
            rect[2] = rect[2].max(pair[0]);
            rect[3] = rect[3].max(pair[1]);
        }
    }
    [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])]
}

/// Write a link's text areas, or remove them when it has none.
fn write_quads(d: &mut Dict, quads: Option<&[[f64; 8]]>) {
    match quads {
        Some(quads) => d.set(b"QuadPoints".to_vec(), Object::Array(quads.iter().flatten().map(|v| Object::Real(*v)).collect())),
        None => {
            d.remove(b"QuadPoints");
        }
    }
}

/// Add a link over `rect` (user space). Returns its index in the page's `/Annots`.
pub fn add(doc: &mut Document, page: usize, rect: [f64; 4], action: &LinkAction, style: &LinkStyle) -> Result<usize, AnnotError> {
    add_body(doc, page, &LinkBody { rect, quads: None, action: action.clone(), style: *style })
}

/// Add a link with an area (a rectangle and, for a text link, its quads).
pub fn add_body(doc: &mut Document, page: usize, body: &LinkBody) -> Result<usize, AnnotError> {
    let p = page_ref(doc, page)?;
    let obj = new_link(doc, p, body)?;
    let mut list = annots(doc, p);
    list.push(Object::Ref(obj));
    let index = list.len() - 1;
    set_annots(doc, p, list)?;
    Ok(index)
}

/// A link annotation object for page `p`, not yet on the page's `/Annots`.
fn new_link(doc: &mut Document, p: ObjRef, body: &LinkBody) -> Result<ObjRef, AnnotError> {
    let r = match body.quads.as_deref() {
        Some(quads) => {
            checked_quads(quads)?;
            checked_rect(quads_rect(quads))?
        }
        None => checked_rect(body.rect)?,
    };
    let action = &body.action;
    if matches!(action, LinkAction::Other(_)) {
        return Err(AnnotError::Invalid("choose a page, a name, another file or a web address".into()));
    }
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"Subtype".to_vec(), Object::name("Link"));
    d.set(b"Rect".to_vec(), Object::Array(r.iter().map(|v| Object::Real(*v)).collect()));
    write_quads(&mut d, body.quads.as_deref());
    d.set(b"P".to_vec(), Object::Ref(p));
    d.set(b"F".to_vec(), Object::Int(4));
    write_style(&mut d, &body.style);
    write_action(doc, &mut d, action)?;
    Ok(doc.add(Object::Dict(d)))
}

fn link_at(doc: &Document, page: usize, index: usize) -> Result<(ObjRef, ObjRef), AnnotError> {
    let p = page_ref(doc, page)?;
    let a = annots(doc, p).get(index).cloned().ok_or(AnnotError::NoSuchAnnotation { page, index })?;
    let r = a.as_ref().ok_or_else(|| AnnotError::Invalid("an inline link can't be edited".into()))?;
    if doc.get(r).as_dict().and_then(|d| d.name(b"Subtype").map(<[u8]>::to_vec)).as_deref() != Some(b"Link") {
        return Err(AnnotError::Invalid("that annotation isn't a link".into()));
    }
    Ok((p, r))
}

/// Link Properties: change a link's area, action and/or appearance. A rectangle
/// change drops the text areas the old area carried, since they would describe the
/// previous area; [`set_body`] writes an area whole.
pub fn set(
    doc: &mut Document,
    page: usize,
    index: usize,
    rect: Option<[f64; 4]>,
    action: Option<&LinkAction>,
    style: Option<&LinkStyle>,
) -> Result<(), AnnotError> {
    let (_, r) = link_at(doc, page, index)?;
    let mut d = doc.get(r).as_dict().cloned().unwrap_or_default();
    if let Some(rect) = rect {
        let rr = checked_rect(rect)?;
        d.set(b"Rect".to_vec(), Object::Array(rr.iter().map(|v| Object::Real(*v)).collect()));
        d.remove(b"QuadPoints");
    }
    if let Some(s) = style {
        write_style(&mut d, s);
    }
    if let Some(a) = action {
        write_action(doc, &mut d, a)?;
    }
    doc.set(r, Object::Dict(d));
    Ok(())
}

/// Write one link whole: its area (rectangle and quads), its action and its look.
/// Everything is checked before the annotation changes.
pub fn set_body(doc: &mut Document, page: usize, index: usize, body: &LinkBody) -> Result<(), AnnotError> {
    let (_, r) = link_at(doc, page, index)?;
    let rect = match body.quads.as_deref() {
        Some(quads) => {
            checked_quads(quads)?;
            checked_rect(quads_rect(quads))?
        }
        None => checked_rect(body.rect)?,
    };
    if matches!(body.action, LinkAction::Other(_)) {
        return Err(AnnotError::Invalid("choose a page, a name, another file or a web address".into()));
    }
    let mut d = doc.get(r).as_dict().cloned().unwrap_or_default();
    d.set(b"Rect".to_vec(), Object::Array(rect.iter().map(|v| Object::Real(*v)).collect()));
    write_quads(&mut d, body.quads.as_deref());
    write_style(&mut d, &body.style);
    write_action(doc, &mut d, &body.action)?;
    doc.set(r, Object::Dict(d));
    Ok(())
}

/// The links on `page` as [`LinkBody`]s a rewrite writes back, in `/Annots` order.
/// A link the model cannot carry — an action it has no form for, a rectangle or an
/// area that is not numbers — is an error: a rewrite that skipped it would lose a
/// destination, and no undo would bring it back.
pub fn bodies(doc: &Document, page: usize) -> Result<Vec<LinkBody>, AnnotError> {
    let p = page_ref(doc, page)?;
    let pages = page_refs(doc)?;
    let mut uri_budget = MAX_URI_BYTES;
    let mut out = Vec::new();
    for a in annots(doc, p) {
        let Some(d) = doc.resolve(&a).as_dict().cloned() else { continue };
        if d.name(b"Subtype") != Some(b"Link") {
            continue;
        }
        let r = nums(doc, d.get(b"Rect"));
        if r.len() != 4 || !r.iter().all(|v| v.is_finite()) {
            return Err(AnnotError::Invalid("a link on this page has no usable area".into()));
        }
        let fixed = [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])];
        let raw_quads = d.get(b"QuadPoints").is_some();
        let quads = quads_of(doc, &d);
        if raw_quads && quads.is_none() {
            return Err(AnnotError::Invalid("a link on this page has areas that are not four points each".into()));
        }
        let action = action_of(doc, &d, &pages, &mut uri_budget);
        if matches!(action, LinkAction::Other(_)) {
            return Err(AnnotError::Invalid("a link on this page goes somewhere PdfCraft cannot write back".into()));
        }
        out.push(LinkBody { rect: fixed, quads, action, style: style_of(doc, &d) });
    }
    Ok(out)
}

/// Make the page's links exactly `links`, in order. Links that did not change keep
/// their annotation object (and every key the model does not carry); a changed one
/// is written in place, a surplus one deleted from the last, and a new one appended.
/// The whole list is checked before the first write, and [`bodies`] refuses a page
/// whose links the model cannot carry, so nothing is dropped silently.
pub fn set_page_links(doc: &mut Document, page: usize, links: &[LinkBody]) -> Result<(), AnnotError> {
    page_ref(doc, page)?;
    // Check everything that can be checked without writing.
    for body in links {
        match body.quads.as_deref() {
            Some(quads) => {
                checked_quads(quads)?;
                checked_rect(quads_rect(quads))?;
            }
            None => {
                checked_rect(body.rect)?;
            }
        }
        match &body.action {
            LinkAction::Uri(u) if u.trim().is_empty() => return Err(AnnotError::Invalid("type the web address first".into())),
            LinkAction::Named(n) if n.trim().is_empty() => return Err(AnnotError::Invalid("name the destination first".into())),
            LinkAction::Remote { file, .. } if file.trim().is_empty() => return Err(AnnotError::Invalid("name the other file first".into())),
            LinkAction::Other(_) => return Err(AnnotError::Invalid("choose a page, a name, another file or a web address".into())),
            _ => {}
        }
        if let LinkAction::Page { page, .. } = &body.action {
            page_ref(doc, *page)?;
        }
    }
    let old = bodies(doc, page)?;
    let indices = link_indices(doc, page)?;
    if indices.len() != old.len() {
        return Err(AnnotError::Invalid("the page's links changed while they were being read".into()));
    }
    for (k, body) in links.iter().enumerate().take(old.len()) {
        if &old[k] != body {
            set_body(doc, page, indices[k], body)?;
        }
    }
    // Surplus links go from the last: deleting a higher `/Annots` index leaves the
    // indices of the links before it valid.
    for k in (links.len().min(old.len())..old.len()).rev() {
        delete(doc, page, indices[k])?;
    }
    for body in links.iter().skip(old.len()) {
        add_body(doc, page, body)?;
    }
    Ok(())
}

/// The `/Annots` positions of the links on `page`, in order.
fn link_indices(doc: &Document, page: usize) -> Result<Vec<usize>, AnnotError> {
    let p = page_ref(doc, page)?;
    Ok(annots(doc, p)
        .iter()
        .enumerate()
        .filter(|(_, a)| doc.resolve(a).as_dict().is_some_and(|d| d.name(b"Subtype") == Some(b"Link")))
        .map(|(k, _)| k)
        .collect())
}

/// Delete one link.
pub fn delete(doc: &mut Document, page: usize, index: usize) -> Result<(), AnnotError> {
    let (p, _) = link_at(doc, page, index)?;
    let mut list = annots(doc, p);
    list.remove(index);
    Ok(set_annots(doc, p, list)?)
}

/// Remove every link on `pages` (all when `None`). Returns how many went.
pub fn remove_all(doc: &mut Document, pages: Option<&[usize]>) -> Result<usize, AnnotError> {
    let refs = page_refs(doc)?;
    let mut n = 0;
    for (pi, p) in refs.iter().enumerate() {
        if pages.is_some_and(|ps| !ps.contains(&pi)) {
            continue;
        }
        let list = annots(doc, *p);
        let kept: Vec<Object> = list
            .iter()
            .filter(|a| doc.resolve(a).as_dict().and_then(|d| d.name(b"Subtype").map(<[u8]>::to_vec)).as_deref() != Some(b"Link"))
            .cloned()
            .collect();
        if kept.len() != list.len() {
            n += list.len() - kept.len();
            set_annots(doc, *p, kept)?;
        }
    }
    Ok(n)
}

/// Recognise common ASCII mailbox addresses without mistaking domains, partial words or
/// invalid labels for e-mail links. The returned end is in source-character indices.
fn email_at(chars: &[char], start: usize) -> Option<(usize, String)> {
    let local_char = |c: char| c.is_ascii_alphanumeric() || "._+%-".contains(c);
    let domain_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '.';
    // Don't create a link starting in the middle of an address.
    if start > 0 && chars.get(start - 1).is_some_and(|c| local_char(*c) || *c == '@') {
        return None;
    }
    let mut at = start;
    while chars.get(at).is_some_and(|c| local_char(*c)) {
        at += 1;
        if at - start > 64 {
            return None;
        }
    }
    let local = chars.get(start..at)?;
    if local.is_empty() || local.first() == Some(&'.') || local.last() == Some(&'.') || local.windows(2).any(|w| w == ['.', '.']) {
        return None;
    }
    if chars.get(at) != Some(&'@') {
        return None;
    }
    let mut end = at + 1;
    while chars.get(end).is_some_and(|c| domain_char(*c)) {
        end += 1;
        if end - at - 1 > 253 {
            return None;
        }
    }
    while end > at + 1 && chars.get(end - 1) == Some(&'.') {
        end -= 1;
    }
    if chars.get(end).is_some_and(|c| c.is_ascii_alphanumeric() || *c == '@' || *c == '_') {
        return None;
    }
    let domain = chars.get(at + 1..end)?;
    let mut labels = 0;
    let mut tld_length = 0;
    for label in domain.split(|c| *c == '.') {
        if label.is_empty() || label.len() > 63 || !label.first()?.is_ascii_alphanumeric() || !label.last()?.is_ascii_alphanumeric() {
            return None;
        }
        labels += 1;
        tld_length = label.len();
    }
    if labels < 2 || tld_length < 2 {
        return None;
    }
    let address: String = chars.get(start..end)?.iter().collect();
    Some((end, format!("mailto:{address}")))
}

/// The longest web address [`find_urls`] reads, in characters (browsers and servers commonly
/// stop near 2,000). Each address it might be is read this far at most, so text whose brackets
/// never close (`https://a/(https://a/(…`, without a space) costs this much per address in it,
/// not a pass over the rest of the page each.
const MAX_URL_CHARS: usize = 2048;

/// Find web addresses in text: `http(s)://…`, `www.…` and bare e-mail addresses become
/// `mailto:`. Returns character ranges and the URI for each.
///
/// No other scheme is ever found, and a `www.` or e-mail address that is part of one written with
/// another scheme (`ftp://www.example.com`, `smb://admin@example.org`) is not taken out of it:
/// it would open with a scheme the text doesn't say. Brackets inside a web address belong to it
/// when they pair up (`…/wiki/Mercury_(planet)`); an unpaired one ends it.
pub fn find_urls(chars: &[char]) -> Vec<(std::ops::Range<usize>, String)> {
    let url_char = |c: char| !c.is_whitespace() && !"<>\"'()[]{}".contains(c);
    // Characters found inside an address but not between words: a `www.` after one is part of
    // a longer address, and so is an e-mail address after a slash.
    let inside_address = |i: usize, set: &str| i > 0 && chars.get(i - 1).is_some_and(|c| set.contains(*c));
    // Case-insensitive prefix test that never steps past the end. Comparing against the original
    // characters keeps every offset in `chars`'s own index space: case-folding a copy can change
    // its length (U+0130 becomes "i" plus a combining dot), so a folded copy must never supply
    // offsets into the source (that mismatch panicked; #816).
    let starts_with =
        |start: usize, pat: &str| pat.chars().enumerate().all(|(k, p)| chars.get(start + k).is_some_and(|c| c.eq_ignore_ascii_case(&p)));
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let boundary = i == 0 || chars.get(i - 1).is_some_and(|c| !c.is_alphanumeric());
        let www = starts_with(i, "www.") && !inside_address(i, "/\\:@.-_~%+=&#");
        if boundary && (starts_with(i, "http://") || starts_with(i, "https://") || www) {
            let mut j = i;
            // Brackets pair up inside the address: `depth` are open, the outermost at `opened`.
            let (mut depth, mut opened) = (0usize, i);
            let mut too_long = false;
            loop {
                if j.saturating_sub(i) >= MAX_URL_CHARS {
                    too_long = true;
                    break;
                }
                match chars.get(j) {
                    Some('(') => {
                        if depth == 0 {
                            opened = j;
                        }
                        depth = depth.saturating_add(1);
                    }
                    Some(')') if depth > 0 => depth -= 1,
                    Some(c) if url_char(*c) => {}
                    _ => break,
                }
                j += 1;
            }
            // A bracket left open isn't the address's: it ends before it. A run of address
            // characters longer than any address is none, and the search goes on after it.
            if depth > 0 {
                j = opened;
            } else if too_long {
                i = j;
                continue;
            }
            // Trailing punctuation belongs to the sentence.
            while j > i && chars.get(j - 1).is_some_and(|c| ".,;:!?".contains(*c)) {
                j -= 1;
            }
            let s: String = chars.get(i..j).map(|c| c.iter().collect()).unwrap_or_default();
            if s.len() > 6 && s.contains('.') {
                let uri = if s.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("www.")) { format!("http://{s}") } else { s };
                out.push((i..j, uri));
                i = j;
                continue;
            }
        }
        if boundary
            && !inside_address(i, "/\\")
            && let Some((end, uri)) = email_at(chars, i)
        {
            out.push((i..end, uri));
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

/// Create links for found URLs: `(page, quads, uri)` with quads in user space (one link per
/// line rectangle). Returns how many links were added.
pub fn add_many(doc: &mut Document, links: &[(usize, Vec<[f64; 4]>, String)], style: &LinkStyle) -> Result<usize, AnnotError> {
    let pages = page_refs(doc)?;
    // Check the whole batch first, so a bad entry fails it before any link object is created.
    for (page, rects, uri) in links {
        pages.get(*page).ok_or(AnnotError::NoSuchPage(*page))?;
        if uri.trim().is_empty() {
            return Err(AnnotError::Invalid("type the web address first".into()));
        }
        for r in rects {
            checked_rect(*r)?;
        }
    }
    // Each page's `/Annots` is extended once, with its new links in the order given.
    let mut added: Vec<(ObjRef, Vec<Object>)> = Vec::new();
    let mut slot: std::collections::HashMap<ObjRef, usize> = std::collections::HashMap::new();
    let mut n = 0;
    for (page, rects, uri) in links {
        let p = pages.get(*page).copied().ok_or(AnnotError::NoSuchPage(*page))?;
        let action = LinkAction::Uri(uri.clone());
        for r in rects {
            let obj = new_link(doc, p, &LinkBody { rect: *r, quads: None, action: action.clone(), style: *style })?;
            let k = *slot.entry(p).or_insert_with(|| {
                added.push((p, Vec::new()));
                added.len() - 1
            });
            if let Some((_, list)) = added.get_mut(k) {
                list.push(Object::Ref(obj));
            }
            n += 1;
        }
    }
    for (p, new) in added {
        let mut list = annots(doc, p);
        list.extend(new);
        set_annots(doc, p, list)?;
    }
    Ok(n)
}
