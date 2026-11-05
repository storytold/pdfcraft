use std::rc::Rc;
use std::sync::Arc;

use super::tab_order::rank_tabs;
use super::{DecodeBudget, Dict, Document, Field, FieldKind, ObjRef, Object, PdfString, Widget, af, appearance, flags, name_text};

/// At most this many `/State` entries of a set-layer-visibility action are read.
const MAX_LAYER_STATE: usize = 1024;

/// A push button's mouse-up action (`/A`, or `/AA /U`, on the field or its widget), its script
/// copied against `scripts` and its other text against `text`; `pages` gives each page's index,
/// for a go-to action.
fn button_action(
    doc: &Document,
    d: &Dict,
    widgets: &[ObjRef],
    scripts: &mut Allowance,
    text: &mut Allowance,
    pages: &std::collections::HashMap<ObjRef, usize>,
) -> Option<af::ButtonAction> {
    // A button PdfCraft generated for an XFA image field (`/PCXfaImage true`, no PDF action):
    // clicking it picks a picture, as `buttonImportIcon()` does.
    if matches!(d.get(b"PCXfaImage").map(|o| doc.resolve(o)).as_deref(), Some(Object::Bool(true))) {
        return Some(af::ButtonAction::ImportIcon);
    }
    // The action of the first of these dictionaries that has one, read where it is: an `/AA`
    // many buttons share isn't copied for each.
    let mut from = |dict: &Dict| -> Option<Option<af::ButtonAction>> {
        if let Some(a) = dict.get(b"A") {
            return Some(button_action_of(doc, a, scripts, text, pages));
        }
        let aa = held(doc, dict.get(b"AA")?);
        let u = aa.as_dict()?.get(b"U")?;
        Some(button_action_of(doc, u, scripts, text, pages))
    };
    match from(d) {
        Some(found) => found,
        None => widgets
            .iter()
            .find_map(|w| {
                let wo = doc.get(*w);
                from(wo.as_dict()?)
            })
            .flatten(),
    }
}

fn button_action_of(
    doc: &Document,
    action: &Object,
    scripts: &mut Allowance,
    text: &mut Allowance,
    pages: &std::collections::HashMap<ObjRef, usize>,
) -> Option<af::ButtonAction> {
    let a = held(doc, action);
    let a = a.as_dict()?;
    Some(match a.name(b"S")? {
        b"ResetForm" => {
            let mut fields = Vec::new();
            let list = a.get(b"Fields").map(|f| held(doc, f));
            for o in list.as_deref().and_then(Object::as_array).into_iter().flatten() {
                if !text.entry() {
                    break;
                }
                let name = match o {
                    Object::Ref(r) => doc.get(*r).as_dict().and_then(|fd| fd.get(b"T")).and_then(|t| text.text(doc, t)),
                    other => text.text(doc, other),
                };
                fields.extend(name);
            }
            let exclude = a.get(b"Flags").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0) & 1 != 0;
            af::ButtonAction::Reset { fields, exclude }
        }
        b"Named" => {
            let n = a.name(b"N")?;
            af::ButtonAction::Named(text.copy(n.len(), || String::from_utf8_lossy(n).into_owned())?)
        }
        b"URI" => af::ButtonAction::Uri(text.text(doc, a.get(b"URI")?)?),
        b"SubmitForm" => af::ButtonAction::Submit(
            a.get(b"F")
                .and_then(|f| {
                    let f = held(doc, f);
                    match f.as_dict() {
                        Some(fd) => fd.get(b"F").and_then(|x| text.text(doc, x)),
                        None => text.text(doc, &f),
                    }
                })
                .unwrap_or_default(),
        ),
        b"GoTo" => {
            let dest = held(doc, a.get(b"D")?);
            let target = dest.as_array()?.first()?.as_ref()?;
            // Looked up, not walked to: a reading has many buttons, and a document many pages.
            af::ButtonAction::GoTo(*pages.get(&target)?)
        }
        b"Hide" => {
            // /T: a field name, an annotation (or field) reference, or an array of them.
            let mut fields = Vec::new();
            if let Some(t) = a.get(b"T") {
                match &*held(doc, t) {
                    Object::Array(items) => {
                        for o in items {
                            if !text.entry() {
                                break;
                            }
                            fields.extend(hide_target(doc, o, text));
                        }
                    }
                    _ => fields.extend(hide_target(doc, t, text)),
                }
            }
            let hide = !a.get(b"H").is_some_and(|h| matches!(&*doc.resolve(h), Object::Bool(false)));
            af::ButtonAction::ShowHide { fields, hide }
        }
        b"SetOCGState" => {
            // /State: ON, OFF or Toggle, each followed by the groups it applies to.
            let mut changes = Vec::new();
            if let Some(state) = a.get(b"State") {
                let mut op = None;
                for item in held(doc, state).as_array().into_iter().flatten().take(MAX_LAYER_STATE) {
                    match item {
                        Object::Name(n) => {
                            op = match n.as_slice() {
                                b"ON" => Some(af::LayerOp::On),
                                b"OFF" => Some(af::LayerOp::Off),
                                b"Toggle" => Some(af::LayerOp::Toggle),
                                _ => None,
                            }
                        }
                        Object::Ref(r) => changes.extend(op.map(|op| (op, (r.num, r.generation)))),
                        _ => {}
                    }
                }
            }
            let preserve_rb = !a.get(b"PreserveRB").is_some_and(|p| matches!(&*doc.resolve(p), Object::Bool(false)));
            af::ButtonAction::SetLayers { changes, preserve_rb }
        }
        b"JavaScript" => af::button_script(&script(doc, action, scripts)?),
        _ => return None,
    })
}

/// A field a hide action names: by name, or by reference to the field or one of its widgets.
fn hide_target(doc: &Document, o: &Object, text: &mut Allowance) -> Option<String> {
    match o {
        Object::Ref(r) => qualified_name(doc, *r, text).or_else(|| text.text(doc, o)),
        other => text.text(doc, other),
    }
}

/// The fully qualified name of the field that `r` (a field or one of its widgets) belongs to,
/// from the partial names (`/T`) up its `/Parent` chain, copied against `text`, which each step
/// up the chain costs as an entry: a hide action can name one deep widget many times.
pub(crate) fn qualified_name(doc: &Document, r: ObjRef, text: &mut Allowance) -> Option<String> {
    let mut parts = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cur = Some(r);
    while let Some(c) = cur {
        if seen.len() > 64 || !seen.insert(c) {
            break;
        }
        if !text.entry() {
            return None;
        }
        let obj = doc.get(c);
        let d = obj.as_dict()?;
        if let Some(t) = d.get(b"T").and_then(|o| text.text(doc, o)) {
            parts.push(t);
        }
        cur = d.get(b"Parent").and_then(|p| p.as_ref());
    }
    parts.reverse();
    // The name is one more copy of its parts.
    let len = parts.iter().fold(parts.len(), |n, p| n.saturating_add(p.len()));
    (!parts.is_empty() && text.take(len)).then(|| parts.join("."))
}

/// `o`, or the object it refers to, borrowed: [`Document::resolve`] copies a direct object, so a
/// string held directly in a dictionary many fields share would be copied once for each.
pub(crate) enum Held<'a> {
    Direct(&'a Object),
    Indirect(Arc<Object>),
}

impl std::ops::Deref for Held<'_> {
    type Target = Object;

    fn deref(&self) -> &Object {
        match self {
            Held::Direct(o) => o,
            Held::Indirect(o) => o,
        }
    }
}

/// The object `o` refers to, through references to references as far as [`Document::resolve`]
/// follows them (32); `None` when `o` is direct.
pub(crate) fn referred(doc: &Document, o: &Object) -> Option<ObjRef> {
    let mut r = o.as_ref()?;
    for _ in 0..32 {
        match Object::as_ref(&doc.get(r)) {
            Some(next) => r = next,
            None => break,
        }
    }
    Some(r)
}

/// [`Held`] `o`: a reference is followed as [`Document::resolve`] follows it, through references
/// to references.
pub(crate) fn held<'a>(doc: &Document, o: &'a Object) -> Held<'a> {
    match o {
        Object::Ref(_) => Held::Indirect(doc.resolve(o)),
        other => Held::Direct(other),
    }
}

/// What a copy costs besides its bytes, the `String` that holds it: a file naming one empty
/// string a million times pays for the million copies.
const COPY: usize = std::mem::size_of::<String>();

/// What one reading of the form may copy out of the document for one purpose, however often the
/// file names the same object or fields inherit it: the text of each string, and the decoded
/// bytes of each stream (every filter stage, as [`DecodeBudget`] counts them), each copy with
/// [`COPY`] bytes besides. Text that doesn't fit in what is left isn't kept; making text that
/// turns out longer than what is left can take up to three times that once, and spends it.
pub(crate) struct Allowance {
    left: usize,
    refusals: usize,
}

impl Allowance {
    pub(crate) const fn new(bytes: usize) -> Self {
        Self { left: bytes, refusals: 0 }
    }

    /// How many copies [`Allowance::take`], [`Allowance::entry`], [`Allowance::copy`] and
    /// [`Allowance::decode`] refused so far: a reader that must have all of something's text or
    /// none compares it before and after.
    pub(crate) fn refusals(&self) -> usize {
        self.refusals
    }

    /// Charges a copy of `bytes` bytes. `false`, charging nothing, when it doesn't fit.
    pub(crate) fn take(&mut self, bytes: usize) -> bool {
        match self.left.checked_sub(bytes.saturating_add(COPY)) {
            Some(left) => {
                self.left = left;
                true
            }
            None => {
                self.refusals = self.refusals.saturating_add(1);
                false
            }
        }
    }

    /// Charges reading one entry of an array, text or not, as a copy of nothing: an array the
    /// file names many times is paid for by its length, and once the allowance is spent the
    /// reader stops at the next entry.
    pub(crate) fn entry(&mut self) -> bool {
        self.take(0)
    }

    /// The text `text` makes of what the file stores in `stored` bytes, charged by its length.
    /// Text whose stored bytes alone don't fit isn't made, and text that comes out longer than
    /// what is left spends it, as a decode past it does.
    pub(crate) fn copy(&mut self, stored: usize, text: impl FnOnce() -> String) -> Option<String> {
        if stored.saturating_add(COPY) > self.left {
            self.refusals = self.refusals.saturating_add(1);
            return None;
        }
        let t = text();
        if self.take(t.len()) {
            Some(t)
        } else {
            self.left = 0;
            None
        }
    }

    /// The text of a string object (`o`, or what it refers to).
    pub(crate) fn string(&mut self, doc: &Document, o: &Object) -> Option<String> {
        match &*held(doc, o) {
            Object::String(s) => self.copy(s.bytes.len(), || s.to_text()),
            _ => None,
        }
    }

    /// The text of a string or name object, as [`crate::text_of`] reads it.
    pub(crate) fn text(&mut self, doc: &Document, o: &Object) -> Option<String> {
        match &*held(doc, o) {
            Object::String(s) => self.copy(s.bytes.len(), || s.to_text()),
            Object::Name(n) => self.copy(n.len(), || name_text(n)),
            _ => None,
        }
    }

    /// The text `text` makes of stream `s` decoded, charged as a [`DecodeBudget`] charges the
    /// decode and, past that, for what the text adds to the decoded bytes (a PDFDocEncoding byte
    /// can take three in UTF-8). `None` once what is left is spent, or when it won't decode; text
    /// that outgrows what is left spends it. Its `/DecodeParms` are read only when direct:
    /// `Document::with_direct_parms` would copy the whole stream for each field that names it.
    pub(crate) fn decode(&mut self, s: &pdfcraft_cos::Stream, text: impl FnOnce(Vec<u8>) -> Option<String>) -> Option<String> {
        if !self.take(0) {
            return None;
        }
        let mut budget = DecodeBudget::new(self.left, 0);
        let data = budget.decode(s).ok();
        self.left = budget.remaining();
        let Some(data) = data else {
            // A decode the allowance stopped counts as refused, one corrupt data stopped doesn't.
            if self.left == 0 {
                self.refusals = self.refusals.saturating_add(1);
            }
            return None;
        };
        let decoded = data.len();
        let t = text(data)?;
        match self.left.checked_sub(t.len().saturating_sub(decoded)) {
            Some(left) => {
                self.left = left;
                Some(t)
            }
            None => {
                self.left = 0;
                self.refusals = self.refusals.saturating_add(1);
                None
            }
        }
    }
}

/// The JavaScript, `/JS` strings and streams, one reading of the form (its fields, or its
/// document scripts with their names) may copy in all, however many actions name the same
/// script.
pub(crate) const MAX_SCRIPT_BYTES: usize = 64 << 20;

/// The allowance for the scripts of one reading of the form.
pub(crate) fn script_budget() -> Allowance {
    Allowance::new(MAX_SCRIPT_BYTES)
}

/// The JavaScript of an action dictionary (`/JS` string or stream), copied against `budget`:
/// `None` once it doesn't fit.
pub(crate) fn script(doc: &Document, action: &Object, budget: &mut Allowance) -> Option<String> {
    let a = held(doc, action);
    let js = a.as_dict()?.get(b"JS")?;
    match &*held(doc, js) {
        Object::String(s) => budget.copy(s.bytes.len(), || s.to_text()),
        Object::Stream(s) => budget.decode(s, |b| Some(String::from_utf8_lossy(&b).into_owned())),
        _ => None,
    }
}

/// A field's format/keystroke, validate and calculate scripts (field dictionary, or its only
/// widget's when the field has none).
fn actions_of(doc: &Document, d: &Dict, widgets: &[ObjRef], scripts: &mut Allowance) -> af::Actions {
    // Read where it is: an `/AA` many fields share isn't copied for each.
    let widget = match widgets {
        [w] => Some(doc.get(*w)),
        _ => None,
    };
    fn aa_of<'a>(doc: &Document, dict: &'a Dict) -> Option<Held<'a>> {
        dict.get(b"AA").map(|a| held(doc, a)).filter(|a| a.as_dict().is_some())
    }
    let aa = aa_of(doc, d).or_else(|| widget.as_deref().and_then(Object::as_dict).and_then(|wd| aa_of(doc, wd)));
    let mut out = af::Actions::default();
    let Some(aa) = aa.as_deref().and_then(Object::as_dict) else { return out };
    let mut js = |k: &[u8]| aa.get(k).and_then(|a| script(doc, a, scripts));
    if let Some(f) = js(b"F") {
        match af::parse_format(&f) {
            Some(fm) => out.format = fm,
            None if !f.trim().is_empty() => {
                out.unsupported.push("format");
                out.scripts.format = Some(f);
            }
            None => {}
        }
    }
    if let Some(k) = js(b"K")
        && out.format == af::Format::None
    {
        // A mask has only a keystroke script.
        match af::parse_format(&k) {
            Some(fm @ af::Format::Mask(_)) => out.format = fm,
            Some(_) => {}
            None if !k.trim().is_empty() => {
                out.unsupported.push("keystroke");
                out.scripts.keystroke = Some(k);
            }
            None => {}
        }
    }
    if let Some(v) = js(b"V") {
        match af::parse_validate(&v) {
            Some(vv) => out.validate = vv,
            None if !v.trim().is_empty() => {
                out.unsupported.push("validate");
                out.scripts.validate = Some(v);
            }
            None => {}
        }
    }
    if let Some(c) = js(b"C") {
        match af::parse_calculate(&c) {
            Some(cc) => out.calculate = cc,
            None if !c.trim().is_empty() => {
                out.unsupported.push("calculate");
                out.scripts.calculate = Some(c);
            }
            None => {}
        }
    }
    out
}

/// The field values (`/V`, `/DV`: strings and names by their text, text streams by what they
/// decode to) one reading of the form may copy in all, however many fields name the same value
/// or inherit it.
pub(crate) const MAX_VALUE_BYTES: usize = 64 << 20;

/// A field's value or default value (`/V`, `/DV`), copied against `budget`. A value that doesn't
/// fit reads as none, as one that won't decode does, and so does a list of values one of which
/// doesn't fit: a list read in part would be taken for the whole of it.
fn values_of(doc: &Document, o: Option<&Object>, budget: &mut Allowance) -> Vec<String> {
    let Some(o) = o else { return Vec::new() };
    match &*held(doc, o) {
        Object::Array(a) => {
            let refusals = budget.refusals();
            let mut out = Vec::new();
            for x in a {
                if !budget.entry() {
                    break;
                }
                out.extend(budget.text(doc, x));
            }
            if budget.refusals() == refusals { out } else { Vec::new() }
        }
        Object::Stream(s) => budget.decode(s, |b| Some(PdfString::literal(b).to_text())).into_iter().collect(),
        other => budget.text(doc, other).filter(|s| s != "Off" && !s.is_empty()).into_iter().collect(),
    }
}

fn nums(doc: &Document, o: Option<&Object>) -> Option<Vec<f64>> {
    let o = doc.resolve(o?);
    o.as_array()?.iter().map(|x| doc.resolve(x).as_f64()).collect()
}

#[cfg(test)]
thread_local! {
    /// Page-tree walks ([`page_refs`]) on this thread.
    static PAGE_WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times this thread has walked a page tree.
#[cfg(test)]
pub(crate) fn page_walks() -> usize {
    PAGE_WALKS.with(std::cell::Cell::get)
}

/// Leaf page objects in order (a small walker; `organize` and `annot` have their own).
pub(crate) fn page_refs(doc: &Document) -> Vec<ObjRef> {
    #[cfg(test)]
    PAGE_WALKS.with(|n| n.set(n.get() + 1));
    let Some(root) = doc.root() else { return Vec::new() };
    let Some(pages) = doc.get(root).as_dict().and_then(|d| d.reference(b"Pages")) else { return Vec::new() };
    PageWalk::new(doc, pages).collect()
}

/// Depth-first walk over the leaf pages under `top`. It keeps one cursor per open `/Kids` array,
/// never one entry per child, so a child listed many times or shared between nodes is skipped by
/// the seen check without being queued first (`pdfcraft_model::pages` walks the same way).
/// Cycles end at the seen check and at most a million nodes are visited.
pub(crate) struct PageWalk<'a> {
    doc: &'a Document,
    seen: std::collections::HashSet<ObjRef>,
    pub(crate) pending: Vec<(std::sync::Arc<Object>, usize)>,
}

impl<'a> PageWalk<'a> {
    pub(crate) fn new(doc: &'a Document, top: ObjRef) -> Self {
        let top = std::sync::Arc::new(Object::Array(vec![Object::Ref(top)]));
        Self { doc, seen: Default::default(), pending: vec![(top, 0)] }
    }
}

impl Iterator for PageWalk<'_> {
    type Item = ObjRef;

    fn next(&mut self) -> Option<ObjRef> {
        while let Some((kids, next)) = self.pending.last_mut() {
            let Some(kid) = kids.as_array().and_then(|a| a.get(*next)) else {
                self.pending.pop();
                continue;
            };
            let node = kid.as_ref();
            *next += 1;
            let Some(node) = node else { continue };
            if self.seen.len() >= 1_000_000 {
                self.pending.clear();
                return None;
            }
            if !self.seen.insert(node) {
                continue;
            }
            let obj = self.doc.get(node);
            let Some(d) = obj.as_dict() else { continue };
            match d.get(b"Kids").map(|k| self.doc.resolve(k)) {
                Some(kids) if d.name(b"Type") != Some(b"Page") => self.pending.push((kids, 0)),
                _ => return Some(node),
            }
        }
        None
    }
}

/// The AcroForm dictionary.
pub fn acroform(doc: &Document) -> Option<Dict> {
    let root = doc.root()?;
    let cat = doc.get(root);
    let af = cat.as_dict()?.get(b"AcroForm")?.clone();
    doc.resolve(&af).as_dict().cloned()
}

/// What a field inherits from the fields above it. Text is shared with the fields below, not
/// copied, and a value is read from the field that carries it, where a field needs it: a long
/// name, `/DA` or value would otherwise be copied once for each field under it.
#[derive(Clone, Default)]
struct Inherited {
    name: Rc<str>,
    ft: Option<Rc<[u8]>>,
    ff: Option<u32>,
    /// The fields that carry the `/V` and `/DV` that apply.
    v: Option<Arc<Object>>,
    dv: Option<Arc<Object>>,
    da: Option<Rc<str>>,
    /// The `/DA` that applies didn't fit the text allowance: a field that would draw with it is
    /// left out, and one that sets its own, or a signature, drawn without one, is read.
    da_refused: bool,
    q: Option<i64>,
    max_len: Option<usize>,
}

impl Inherited {
    /// `o`, a field's or the form's `/DA`, as the one that applies from here down, charged to
    /// `text`. One that doesn't fit is noted, and leaves out the fields that would draw with it;
    /// one that isn't text leaves the inherited one in place.
    fn read_da(&mut self, doc: &Document, o: &Object, text: &mut Allowance) {
        let refusals = text.refusals();
        match text.text(doc, o) {
            Some(da) => (self.da, self.da_refused) = (Some(Rc::from(da)), false),
            None if text.refusals() != refusals => (self.da, self.da_refused) = (None, true),
            None => {}
        }
    }
}

/// Every terminal field the reading reads: the `/Fields` tree in order, then the fields reachable
/// only through the page annotations (see [`adopt_page_fields`]). Fields whose text is past the
/// reading's allowances are left out, and [`Fields::left_out`] counts them.
pub fn fields(doc: &Document) -> Vec<Field> {
    fields_with_adopted(doc).items
}

/// How many of [`fields`] were reachable only through the page annotations: the form's `/Fields`
/// list names none or only some of them, so the leniency that adopts them is worth recording.
pub fn adopted_page_fields(doc: &Document) -> usize {
    fields_with_adopted(doc).adopted_page_fields
}

/// The terminal fields and page-only fields adopted while reading a form.
#[derive(Clone, Debug, PartialEq)]
pub struct Fields {
    /// Terminal fields in tree order, followed by adopted page-only fields.
    pub items: Vec<Field>,
    /// How many fields were reachable only through page annotations.
    pub adopted_page_fields: usize,
    /// How many terminal fields were left out because their text, or a field's above them, didn't
    /// fit the reading's allowances. More than none means `items` isn't the whole form: redaction
    /// refuses to remove a widget under a mark that no field read owns.
    pub left_out: usize,
}

/// Read all terminal fields and the page-only adoption count in one enumeration.
pub fn fields_with_adopted(doc: &Document) -> Fields {
    let (items, adopted_page_fields, left_out) = enumerate(doc);
    Fields { items, adopted_page_fields, left_out }
}

#[cfg(test)]
thread_local! {
    /// Whole-form readings ([`enumerate`]) on this thread.
    static FORM_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times this thread has read the whole form.
#[cfg(test)]
pub(crate) fn form_reads() -> usize {
    FORM_READS.with(std::cell::Cell::get)
}

pub(crate) fn enumerate(doc: &Document) -> (Vec<Field>, usize, usize) {
    #[cfg(test)]
    FORM_READS.with(|n| n.set(n.get() + 1));
    let af = acroform(doc);
    let mut page_of = std::collections::HashMap::new();
    let mut annot_index = std::collections::HashMap::new();
    let pages = page_refs(doc);
    for (i, p) in pages.iter().enumerate() {
        if let Some(a) = doc.get(*p).as_dict().and_then(|d| d.get(b"Annots").cloned()) {
            for (k, e) in doc.resolve(&a).as_array().into_iter().flatten().enumerate() {
                if let Some(r) = e.as_ref() {
                    page_of.entry(r).or_insert(i);
                    annot_index.entry(r).or_insert(k);
                }
            }
        }
    }
    let mut seen = Seen {
        fields: std::collections::HashSet::new(),
        widgets: std::collections::HashSet::new(),
        withheld: std::collections::HashSet::new(),
        left_out: 0,
        scripts: script_budget(),
        values: Allowance::new(MAX_VALUE_BYTES),
        text: Allowance::new(MAX_TEXT_BYTES),
        pages: pages.iter().enumerate().map(|(i, p)| (*p, i)).collect(),
    };
    let mut base = Inherited::default();
    if let Some(af) = &af {
        if let Some(o) = af.get(b"DA") {
            base.read_da(doc, o, &mut seen.text);
        }
        base.q = af.get(b"Q").and_then(|o| doc.resolve(o).as_int());
    }
    let mut out = Vec::new();
    if let Some(af) = &af {
        for f in af.get(b"Fields").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default() {
            if let Some(r) = f.as_ref() {
                walk(doc, r, &base, &page_of, &mut seen, &mut out, 0);
            }
        }
    }
    let listed = out.len();
    adopt_page_fields(doc, &pages, &base, &page_of, &mut seen, &mut out);
    let adopted = out.len() - listed;
    rank_tabs(doc, &pages, &annot_index, &mut out);
    (out, adopted, seen.left_out)
}

/// Widget annotations no `/Fields` entry reaches are adopted as fields, as Acrobat and the
/// browsers do: some writers list only some of the fields, or none at all. Each widget is adopted
/// through its topmost unlisted `/Parent`, so a field split across several widgets stays one
/// field. Tree-listed fields keep their order first; the rest follow in page-annotation order.
fn adopt_page_fields(
    doc: &Document,
    pages: &[ObjRef],
    base: &Inherited,
    page_of: &std::collections::HashMap<ObjRef, usize>,
    seen: &mut Seen,
    out: &mut Vec<Field>,
) {
    for p in pages {
        let Some(a) = doc.get(*p).as_dict().and_then(|d| d.get(b"Annots").cloned()) else { continue };
        let annots = doc.resolve(&a);
        let Some(annots) = annots.as_array() else { continue };
        for e in annots {
            let Some(r) = e.as_ref() else { continue };
            if seen.fields.contains(&r) || seen.widgets.contains(&r) || seen.withheld.contains(&r) {
                continue;
            }
            let obj = doc.get(r);
            let Some(d) = obj.as_dict() else { continue };
            if d.name(b"Subtype") != Some(b"Widget") {
                continue;
            }
            if !d.contains(b"FT") && !d.contains(b"T") && !d.contains(b"Parent") {
                continue;
            }
            if let Some(root) = unlisted_root(doc, r, &seen.fields, &seen.withheld) {
                walk(doc, root, base, page_of, seen, out, 0);
            }
        }
    }
}

/// The topmost field above `r` (itself without a `/Parent`), or `None` when `r` or an ancestor is
/// already reachable from the `/Fields` tree, is under a field left out of the reading
/// (`withheld`), or a `/Parent` loop hides the top.
fn unlisted_root(
    doc: &Document,
    r: ObjRef,
    seen: &std::collections::HashSet<ObjRef>,
    withheld: &std::collections::HashSet<ObjRef>,
) -> Option<ObjRef> {
    let known = |n: &ObjRef| seen.contains(n) || withheld.contains(n);
    let mut top = r;
    let mut visited = std::collections::HashSet::from([r]);
    loop {
        let parent = doc.get(top).as_dict().and_then(|d| d.reference(b"Parent"));
        let Some(p) = parent else {
            return if known(&top) { None } else { Some(top) };
        };
        if !visited.insert(p) || visited.len() > 64 {
            return None;
        }
        if known(&p) {
            return None;
        }
        top = p;
    }
}

/// The rest of the text one reading of the form may copy in all, however many fields name the
/// same string or inherit it: full names, `/DA` (a list box's widgets' too), options, tooltips,
/// captions, on states and the text of button actions. As much as a page's content may decode
/// (`DecodeBudget::page`): options are copied into each field that lists them, so an ordinary
/// large form, 300 combo boxes listing one 2,000-item catalogue, takes about 65 MiB of it.
pub(crate) const MAX_TEXT_BYTES: usize = 256 << 20;

/// What one reading of the form's fields has used: the field and widget dictionaries read, and
/// the allowances their scripts, values and other text share.
struct Seen {
    fields: std::collections::HashSet<ObjRef>,
    /// A widget belongs to the first field read whole that lists it, however often it is listed.
    widgets: std::collections::HashSet<ObjRef>,
    /// The widgets of fields left out of the reading: a later field that lists one takes it, but
    /// none is adopted as a field of its own.
    withheld: std::collections::HashSet<ObjRef>,
    /// Fields left out of the reading for text that didn't fit ([`Fields::left_out`]).
    left_out: usize,
    scripts: Allowance,
    values: Allowance,
    /// A field whose text doesn't fit is left out of the reading whole: a field read without
    /// its `/DA` or options would be drawn and checked as another field.
    text: Allowance,
    /// Each page's index, from the one walk of the page tree a reading makes.
    pages: std::collections::HashMap<ObjRef, usize>,
}

/// The on state of a check box or radio button widget: the first state of its normal appearance
/// (`/AP /N`) other than Off.
fn on_state_of(doc: &Document, wd: &Dict, text: &mut Allowance) -> Option<String> {
    let ap = held(doc, wd.get(b"AP")?);
    let n = held(doc, ap.as_dict()?.get(b"N")?);
    let (k, _) = n.as_dict()?.iter().find(|(k, _)| k.as_slice() != b"Off")?;
    text.copy(k.len(), || name_text(k))
}

/// Withholds the fields and widgets under `r`, a field left out of the reading for its name: none
/// is adopted from the page as a field of its own, and a later field in the tree that lists one
/// still takes it. Each is visited once, its text unread, so the walk ends however the kids loop.
/// How many terminal fields it holds, `r` among them: those with a name and no kid that has one.
fn withhold_below(doc: &Document, r: ObjRef, seen: &mut Seen) -> usize {
    let named = |n: &ObjRef| doc.get(*n).as_dict().is_some_and(|d| d.contains(b"T"));
    // A `/Kids` array several nodes share by reference is read once: its entries are withheld
    // already, and whether one is named is kept.
    let mut shared: std::collections::HashMap<ObjRef, bool> = std::collections::HashMap::new();
    let mut terminal = 0usize;
    let mut stack = vec![r];
    while let Some(n) = stack.pop() {
        let node = doc.get(n);
        let Some(d) = node.as_dict() else { continue };
        let kids_entry = d.get(b"Kids");
        let holder = kids_entry.and_then(|k| referred(doc, k));
        let has_named = match holder.and_then(|h| shared.get(&h)) {
            Some(&has_named) => has_named,
            None => {
                let kids: Vec<ObjRef> = kids_entry
                    .map(|k| doc.resolve(k))
                    .map(|k| k.as_array().into_iter().flatten().filter_map(Object::as_ref).collect())
                    .unwrap_or_default();
                #[cfg(test)]
                WITHHELD_ENTRIES.with(|c| c.set(c.get() + kids.len()));
                let has_named = kids.iter().any(named);
                if let Some(h) = holder {
                    shared.insert(h, has_named);
                }
                for k in kids {
                    if seen.withheld.insert(k) {
                        stack.push(k);
                    }
                }
                has_named
            }
        };
        if d.contains(b"T") && !has_named {
            terminal = terminal.saturating_add(1);
        }
    }
    terminal
}

#[cfg(test)]
thread_local! {
    /// `/Kids` entries [`withhold_below`] read on this thread.
    pub(crate) static WITHHELD_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Read the field at `r` and the fields under it into `out`, with what they inherit from
/// `parent`: kids with a name are fields of their own, kids without are this field's widgets.
fn walk(
    doc: &Document,
    r: ObjRef,
    parent: &Inherited,
    page_of: &std::collections::HashMap<ObjRef, usize>,
    seen: &mut Seen,
    out: &mut Vec<Field>,
    depth: usize,
) {
    if depth > 64 || !seen.fields.insert(r) {
        return;
    }
    let obj = doc.get(r);
    let Some(d) = obj.as_dict() else { return };
    let mut inh = parent.clone();
    let refusals = seen.text.refusals();
    if let Some(t) = d.get(b"T").and_then(|o| seen.text.text(doc, o)) {
        if inh.name.is_empty() {
            inh.name = Rc::from(t);
        } else if seen.text.take(inh.name.len().saturating_add(t.len()).saturating_add(1)) {
            // The full name: one more copy, of the parent's with this part added.
            inh.name = Rc::from(format!("{}.{t}", inh.name));
        }
    }
    if let Some(ft) = d.name(b"FT") {
        // Kept only as long as a type this reads (`Btn`, `Ch`, `Sig`, `Tx`): a longer name is
        // none of them, and isn't copied.
        inh.ft = Some(Rc::from(if ft.len() <= 3 { ft } else { &[][..] }));
    }
    if let Some(ff) = field_flags(doc, d) {
        inh.ff = Some(ff);
    }
    for (k, slot) in [(&b"V"[..], &mut inh.v), (b"DV", &mut inh.dv)] {
        if d.contains(k) {
            *slot = Some(obj.clone());
        }
    }
    // A name that doesn't fit leaves the field out, with the fields under it, whose full names
    // need it; a `/DA` that doesn't fit, only the fields that would draw with it.
    if seen.text.refusals() != refusals {
        let fields = withhold_below(doc, r, seen).max(1);
        seen.left_out = seen.left_out.saturating_add(fields);
        return;
    }
    if let Some(o) = d.get(b"DA") {
        inh.read_da(doc, o, &mut seen.text);
    }
    if let Some(q) = d.get(b"Q").and_then(|o| doc.resolve(o).as_int()) {
        inh.q = Some(q);
    }
    if let Some(m) = d.get(b"MaxLen").and_then(|o| doc.resolve(o).as_int()) {
        // Compatibility: some writers emit zero for an unlimited text field. Keep /MaxLen
        // untouched in the object graph, but expose only positive limits to both UI and API.
        // An explicit zero also clears an inherited limit rather than inheriting it again.
        inh.max_len = usize::try_from(m).ok().filter(|limit| *limit > 0);
    }
    let kids: Vec<ObjRef> =
        d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default().iter().filter_map(|k| k.as_ref()).collect();
    // Kids with /T are fields; kids without are this field's widgets.
    let field_kids: Vec<ObjRef> = kids.iter().copied().filter(|k| doc.get(*k).as_dict().is_some_and(|kd| kd.contains(b"T"))).collect();
    if !field_kids.is_empty() {
        for k in field_kids {
            walk(doc, k, &inh, page_of, seen, out, depth + 1);
        }
        return;
    }
    // A field of no kind this reads is skipped before it claims widgets or scripts, which a later
    // field listing them then reads.
    let ff = inh.ff.unwrap_or(0);
    let kind = match inh.ft.as_deref() {
        Some(b"Tx") => FieldKind::Text,
        Some(b"Btn") if ff & flags::PUSH_BUTTON != 0 => FieldKind::PushButton,
        Some(b"Btn") if ff & flags::RADIO != 0 => FieldKind::Radio,
        Some(b"Btn") => FieldKind::CheckBox,
        Some(b"Ch") if ff & flags::COMBO != 0 => FieldKind::Combo,
        Some(b"Ch") => FieldKind::List,
        Some(b"Sig") => FieldKind::Signature,
        _ => return,
    };
    let widget_refs: Vec<ObjRef> = if d.contains(b"Rect") { vec![r] } else { kids };
    // Each widget once, claimed only once the field is read whole, so a later field listing it
    // has it when this one is left out.
    let mut claimed = std::collections::HashSet::new();
    let widget_refs: Vec<ObjRef> = widget_refs.into_iter().filter(|w| !seen.widgets.contains(w) && claimed.insert(*w)).collect();
    // A field that would draw with a `/DA` that didn't fit is left out before it reads its
    // scripts; a signature, drawn without one, is read.
    if inh.da_refused && kind != FieldKind::Signature {
        seen.withheld.extend(claimed);
        seen.left_out = seen.left_out.saturating_add(1);
        return;
    }
    let refusals = seen.text.refusals();
    let actions = actions_of(doc, d, &widget_refs, &mut seen.scripts);
    let button = (inh.ft.as_deref() == Some(b"Btn") && inh.ff.unwrap_or(0) & flags::PUSH_BUTTON != 0)
        .then(|| button_action(doc, d, &widget_refs, &mut seen.scripts, &mut seen.text, &seen.pages))
        .flatten();
    let field_da = inh.da.as_deref().unwrap_or("/Helv 0 Tf 0 g");
    let text = &mut seen.text;
    let widgets = widget_refs
        .into_iter()
        .filter_map(|w| {
            let wo = doc.get(w);
            let wd = wo.as_dict()?;
            let rect = nums(doc, wd.get(b"Rect")).filter(|r| r.len() == 4).unwrap_or_else(|| vec![0.0; 4]);
            let annot_flags = wd.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
            let own_ff = if w == r { 0 } else { field_flags(doc, wd).unwrap_or(0) };
            Some(Widget {
                obj: w,
                page: page_of.get(&w).copied(),
                rect: [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])],
                on_state: matches!(kind, FieldKind::CheckBox | FieldKind::Radio).then(|| on_state_of(doc, wd, text)).flatten(),
                state: wd.get(b"AS").and_then(|o| match &*held(doc, o) {
                    Object::Name(n) => text.copy(n.len(), || name_text(n)),
                    _ => None,
                }),
                tab: usize::MAX,
                locked: annot_flags & 128 != 0,
                hidden: annot_flags & (2 | 32) != 0,
                read_only: (ff | own_ff) & flags::READ_ONLY != 0,
                rotation: appearance::mk_rotation(doc, wd),
                caption: (kind == FieldKind::PushButton)
                    .then(|| {
                        let mk = held(doc, wd.get(b"MK")?);
                        text.string(doc, mk.as_dict()?.get(b"CA")?)
                    })
                    .flatten(),
                list: (kind == FieldKind::List).then(|| {
                    let da = wd.get(b"DA").and_then(|o| text.string(doc, o));
                    appearance::list_rows_drawn_with(doc, da.as_deref().unwrap_or(field_da), Some(d), wd)
                }),
                round: kind == FieldKind::Radio || (kind == FieldKind::CheckBox && appearance::is_round(doc, wd)),
            })
        })
        .collect();
    let mut options = Vec::new();
    let opt = d.get(b"Opt").map(|o| held(doc, o));
    for o in opt.as_deref().and_then(Object::as_array).into_iter().flatten() {
        if !text.entry() {
            break;
        }
        match &*held(doc, o) {
            Object::Array(pair) => {
                if let [export, shown, ..] = pair.as_slice()
                    && let Some(export) = text.text(doc, export)
                    && let Some(shown) = text.text(doc, shown)
                {
                    options.push((export, shown));
                }
            }
            other => {
                if let Some(t) = text.text(doc, other)
                    && text.take(t.len())
                {
                    options.push((t.clone(), t));
                }
            }
        }
    }
    let tooltip = d.get(b"TU").and_then(|o| text.text(doc, o));
    // Then the field's own copies of its full name and `/DA`; a copy refused counts with the
    // rest.
    text.take(inh.name.len());
    text.take(field_da.len());
    if text.refusals() != refusals {
        seen.withheld.extend(claimed);
        seen.left_out = seen.left_out.saturating_add(1);
        return;
    }
    seen.widgets.extend(claimed);
    out.push(Field {
        name: inh.name.to_string(),
        obj: r,
        kind,
        value: values_of(doc, inh.v.as_deref().and_then(Object::as_dict).and_then(|f| f.get(b"V")), &mut seen.values),
        default: values_of(doc, inh.dv.as_deref().and_then(Object::as_dict).and_then(|f| f.get(b"DV")), &mut seen.values),
        flags: ff,
        max_len: inh.max_len,
        options,
        da: field_da.to_string(),
        quadding: inh.q.unwrap_or(0),
        tooltip,
        widgets,
        actions,
        button,
    });
}

pub(crate) fn field_flags(doc: &Document, dict: &Dict) -> Option<u32> {
    dict.get(b"Ff").and_then(|value| doc.resolve(value).as_int()).and_then(|value| u32::try_from(value).ok())
}
