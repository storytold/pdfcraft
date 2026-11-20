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
            let fields = a.get(b"Fields").map(|f| action_fields(doc, f, text)).unwrap_or_default();
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
            let fields = a.get(b"T").map(|t| action_fields(doc, t, text)).unwrap_or_default();
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

/// The field names targeted by a `ResetForm` (`/Fields`) or `Hide` (`/T`) action entry: a field
/// name string, an indirect field or widget dictionary reference, or an array of them, copied
/// against `text`, which each entry of an array costs as an entry.
pub(crate) fn action_fields(doc: &Document, o: &Object, text: &mut Allowance) -> Vec<String> {
    let mut fields = Vec::new();
    match &*held(doc, o) {
        Object::Array(items) => {
            for item in items {
                if !text.entry() {
                    break;
                }
                fields.extend(action_field(doc, item, text));
            }
        }
        _ => fields.extend(action_field(doc, o, text)),
    }
    fields
}

/// A field an action names: by name, or by reference to the field or one of its widgets.
fn action_field(doc: &Document, o: &Object, text: &mut Allowance) -> Option<String> {
    match o {
        Object::Ref(r) => qualified_name(doc, *r, text).or_else(|| text.text(doc, o)),
        other => text.text(doc, other),
    }
}

/// The fully qualified name of the field that `r` (a field or one of its widgets) belongs to,
/// from the partial names (`/T`) up its `/Parent` chain, copied against `text`, which each step
/// up the chain costs as an entry: a hide action can name one deep widget many times. `None` once
/// a part doesn't fit: the parts that do would name another field, as a reading leaves out a field
/// whose name doesn't fit.
pub(crate) fn qualified_name(doc: &Document, r: ObjRef, text: &mut Allowance) -> Option<String> {
    let mut parts = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cur = Some(r);
    let refusals = text.refusals();
    while let Some(c) = cur {
        if seen.len() > 64 || !seen.insert(c) {
            break;
        }
        if !text.entry() {
            return None;
        }
        // An object that is only a reference to another is followed, as `Document::resolve` does.
        let obj = doc.resolve(&Object::Ref(c));
        let d = obj.as_dict()?;
        if let Some(t) = d.get(b"T").and_then(|o| text.text(doc, o)) {
            parts.push(t);
        }
        if text.refusals() != refusals {
            return None;
        }
        cur = parent_of(doc, d);
    }
    parts.reverse();
    // The name is one more copy of its parts.
    let len = parts.iter().fold(parts.len(), |n, p| n.saturating_add(p.len()));
    (!parts.is_empty() && text.take(len)).then(|| parts.join("."))
}

/// The field `d` names as its `/Parent`, when that is a dictionary. A reference to an object the
/// file doesn't hold reads as `null`, and a `null` entry is the same as an absent one (ISO
/// 32000-2 §7.3.9): the field is then a root, as if it named no parent.
pub(crate) fn parent_of(doc: &Document, d: &Dict) -> Option<ObjRef> {
    d.reference(b"Parent").filter(|p| doc.get(*p).as_dict().is_some())
}

/// The fields of the form's tree whose `/Kids` list one of `kids` and whose own fully qualified
/// name is `name`: a field listing kids that don't name it as their `/Parent`, and so share its
/// name rather than extend it. Each node is visited once, at most 64 levels down.
pub(crate) fn listing_fields_named(doc: &Document, name: &str, kids: &[ObjRef]) -> Vec<ObjRef> {
    let mut out = Vec::new();
    let Some(af) = acroform(doc) else { return out };
    let top = af.get(b"Fields").map(|f| doc.resolve(f)).and_then(|f| f.as_array().cloned()).unwrap_or_default();
    let mut stack: Vec<(ObjRef, usize)> = top.iter().filter_map(Object::as_ref).map(|r| (r, 0)).collect();
    let mut seen = std::collections::HashSet::new();
    // The names are copied within one allowance for the whole walk, as a reading of the form
    // copies them.
    let mut text = Allowance::new(MAX_TEXT_BYTES);
    while let Some((r, depth)) = stack.pop() {
        if depth > 64 || !seen.insert(r) {
            continue;
        }
        let obj = doc.get(r);
        let Some(d) = obj.as_dict() else { continue };
        let list: Vec<ObjRef> = d
            .get(b"Kids")
            .map(|k| doc.resolve(k))
            .and_then(|k| k.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(Object::as_ref)
            .collect();
        if list.iter().any(|k| kids.contains(k)) && qualified_name(doc, r, &mut text).as_deref() == Some(name) {
            out.push(r);
        }
        let next = depth.saturating_add(1);
        stack.extend(list.into_iter().filter(|k| doc.get(*k).as_dict().is_some_and(|kd| kd.contains(b"T"))).map(|k| (k, next)));
    }
    out
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
    let mut out = af::Actions {
        // A date field the XFA layout generated: what its format doesn't read is kept as typed.
        xfa_date: matches!(d.get(b"PCXfaDate").map(|o| held(doc, o)).as_deref(), Some(Object::Bool(true))),
        ..af::Actions::default()
    };
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
/// or inherit it. Text fields with the Password flag have as much again of their own.
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

/// A choice field's `/I` (ISO 32000-2 Table 234) as option positions, when it is usable: an array
/// of integers "sorted in ascending order", each naming one of `count` options. `None` when it is
/// absent or anything in it is not (a non-integer, a position out of range, out of order or
/// repeated), so the field falls back to `/V`.
pub(crate) fn listed_indices(doc: &Document, d: &Dict, count: usize) -> Option<Vec<usize>> {
    let listed = doc.resolve(d.get(b"I")?);
    let entries = listed.as_array()?;
    // Strictly ascending positions below `count` number at most `count`.
    if entries.len() > count {
        return None;
    }
    let mut out: Vec<usize> = Vec::with_capacity(entries.len());
    for e in entries {
        let i = usize::try_from(doc.resolve(e).as_int()?).ok().filter(|&i| i < count)?;
        if out.last().is_some_and(|&last| last >= i) {
            return None;
        }
        out.push(i);
    }
    Some(out)
}

/// The options a choice field shows selected, as ascending positions in `options`, given its
/// values (`/V`) and its usable `/I` ([`listed_indices`]).
///
/// `/I` decides when it names the same export values as `/V`: it "shall be used when two or more
/// elements in the Opt array have different names but the same export value", and "if the items
/// identified by this entry differ from those in the V entry …, the V entry shall be used" (ISO
/// 32000-2 Table 234). Otherwise each value selects the first option with that export value, as
/// the writer numbers `/I` when it is given only an export value, so a pick keeps showing the same
/// option when `/I` is lost. A value no option exports selects nothing.
pub fn selected_options(options: &[(String, String)], values: &[String], listed: Option<&[usize]>) -> Vec<usize> {
    if let Some(listed) = listed {
        // The same items, each as many times: `/V [(1) (1)]` names two items, which an `/I [1]`
        // naming one doesn't, so `/V` decides then.
        let mut named: Vec<&str> = values.iter().map(String::as_str).collect();
        named.sort_unstable();
        let exports: Option<Vec<&str>> = listed.iter().map(|&i| options.get(i).map(|(e, _)| e.as_str())).collect();
        if let Some(mut exports) = exports {
            exports.sort_unstable();
            if exports == named {
                return listed.to_vec();
            }
        }
    }
    let first = first_by_export(options);
    let mut out: Vec<usize> = values.iter().filter_map(|v| first.get(v.as_str()).copied()).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Each export value in `options` with the position of the first option exporting it, so
/// matching values against options takes one pass over each rather than one over the options per
/// value (a file can hold many of both).
pub(crate) fn first_by_export(options: &[(String, String)]) -> std::collections::HashMap<&str, usize> {
    let mut first: std::collections::HashMap<&str, usize> = std::collections::HashMap::with_capacity(options.len());
    for (i, (e, _)) in options.iter().enumerate() {
        first.entry(e.as_str()).or_insert(i);
    }
    first
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
    /// Where `v` and `dv` are ([`Reading::values_at`]).
    v_at: Option<ObjRef>,
    dv_at: Option<ObjRef>,
    da: Option<Rc<str>>,
    /// The `/DA` that applies didn't fit the text allowance: a field that would draw with it is
    /// left out, and one that sets its own, or a signature, drawn without one, is read.
    da_refused: bool,
    /// The full name, from the field's own `/Parent` chain, didn't fit the text allowance: the
    /// field is left out, as for a name of its own that doesn't fit.
    name_refused: bool,
    q: Option<i64>,
    max_len: Option<usize>,
}

impl Inherited {
    /// The `/FT`, `/Ff`, `/V` and `/DV` of the field `obj` (at `r`) over what it inherits.
    fn take(&mut self, doc: &Document, r: ObjRef, obj: &Arc<Object>) {
        let Some(d) = obj.as_dict() else { return };
        if let Some(ft) = d.name(b"FT") {
            // Kept only as long as a type this reads (`Btn`, `Ch`, `Sig`, `Tx`): a longer name is
            // none of them, and isn't copied.
            self.ft = Some(Rc::from(if ft.len() <= 3 { ft } else { &[][..] }));
        }
        if let Some(ff) = field_flags(doc, d) {
            self.ff = Some(ff);
        }
        if d.contains(b"V") {
            (self.v, self.v_at) = (Some(obj.clone()), Some(r));
        }
        if d.contains(b"DV") {
            (self.dv, self.dv_at) = (Some(obj.clone()), Some(r));
        }
    }

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
    /// refuses to remove a widget under a mark, but one written in place with no `/Parent`: a
    /// field left out may list it, or another widget of its field.
    pub left_out: usize,
}

/// Read all terminal fields and the page-only adoption count in one enumeration.
///
/// The last reading on this thread is kept with the state of the document it read
/// ([`pdfcraft_cos::ReadState`], which every change to the document replaces), and a document
/// that still reads as that state gets a copy of it instead of a new walk. One form edit reads
/// the fields in several steps (filling, recalculating, the XFA data, the view), most of them
/// with nothing changed since the step before: on a form with a thousand fields each walk takes
/// tens of milliseconds.
pub fn fields_with_adopted(doc: &Document) -> Fields {
    let kept = LAST_READING
        .try_with(|last| match last.try_borrow().as_deref() {
            Ok(Some((state, fields))) if state.matches(doc) => Some(fields.clone()),
            _ => None,
        })
        .ok()
        .flatten();
    if let Some(fields) = kept {
        return fields;
    }
    let Reading { items, adopted_page_fields, left_out, .. } = enumerate(doc);
    let fields = Fields { items, adopted_page_fields, left_out };
    // Not kept (the thread is ending, or the slot is in use) costs only a later walk.
    let _ = LAST_READING.try_with(|last| {
        if let Ok(mut last) = last.try_borrow_mut() {
            *last = Some((doc.read_state(), fields.clone()));
        }
    });
    fields
}

/// One reading of the form: [`Fields`], with the dictionaries of the fields it left out.
pub(crate) struct Reading {
    pub(crate) items: Vec<Field>,
    pub(crate) adopted_page_fields: usize,
    pub(crate) left_out: usize,
    /// For each field read, the fields that carry the `/V` and the `/DV` it was read with: where a
    /// value its allowance didn't copy is, as the walk inherits it.
    pub(crate) values_at: std::collections::HashMap<ObjRef, (Option<ObjRef>, Option<ObjRef>)>,
}

thread_local! {
    /// The last [`fields_with_adopted`] on this thread, with the state of the document it read.
    /// One entry: one form's fields, and the objects that document had changed when it was read.
    static LAST_READING: std::cell::RefCell<Option<(pdfcraft_cos::ReadState, Fields)>> = const { std::cell::RefCell::new(None) };
    /// Whole-form walks ([`enumerate`]) on this thread.
    static FORM_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times this thread has walked a whole form: [`fields`] and the readings like it, but
/// not one answered from the last reading. Per thread, so tests running in parallel don't count
/// each other's; for tests that check how often an edit reads the form.
#[doc(hidden)]
pub fn form_reads_on_this_thread() -> usize {
    FORM_READS.try_with(std::cell::Cell::get).unwrap_or(0)
}

pub(crate) fn enumerate(doc: &Document) -> Reading {
    let _ = FORM_READS.try_with(|n| n.set(n.get().saturating_add(1)));
    let af = acroform(doc);
    let mut page_of = std::collections::HashMap::new();
    let mut annot_index = std::collections::HashMap::new();
    let mut by_parent: std::collections::HashMap<ObjRef, Vec<ObjRef>> = std::collections::HashMap::new();
    let pages = page_refs(doc);
    for (i, p) in pages.iter().enumerate() {
        if let Some(a) = doc.get(*p).as_dict().and_then(|d| d.get(b"Annots").cloned()) {
            for (k, e) in doc.resolve(&a).as_array().into_iter().flatten().enumerate() {
                if let Some(r) = e.as_ref()
                    && let std::collections::hash_map::Entry::Vacant(slot) = page_of.entry(r)
                {
                    slot.insert(i);
                    annot_index.insert(r, k);
                    if let Some(parent) = widget_parent(doc, r) {
                        by_parent.entry(parent).or_default().push(r);
                    }
                }
            }
        }
    }
    let listed_roots: Vec<ObjRef> = af
        .as_ref()
        .and_then(|af| af.get(b"Fields").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()))
        .unwrap_or_default()
        .iter()
        .filter_map(Object::as_ref)
        .collect();
    let mut seen = Seen {
        fields: std::collections::HashSet::new(),
        widgets: std::collections::HashSet::new(),
        withheld: std::collections::HashSet::new(),
        left_out: 0,
        values_at: std::collections::HashMap::new(),
        withheld_arrays: std::collections::HashMap::new(),
        roots: listed_roots.iter().copied().collect(),
        path: Vec::new(),
        by_parent,
        scripts: script_budget(),
        values: Allowance::new(MAX_VALUE_BYTES),
        passwords: Allowance::new(MAX_VALUE_BYTES),
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
    for r in listed_roots {
        // An entry under a field left out of the reading is that field's: read as a root, it would
        // be read under a name without that field's part.
        if seen.withheld.contains(&r) {
            continue;
        }
        // A `/Fields` entry that is a nameless kid of a field `/Fields` doesn't list is read
        // through that field, which names it (ISO 32000-2 §12.7.4.2). A kid a field already read
        // has claimed as its widget is not read again as a nameless field; one its parent's
        // `/Kids` omits is still read on its own, as before. One under a field left out of the
        // reading is that field's, and isn't read as a field of its own either.
        let nameless_kid = doc.get(r).as_dict().is_some_and(|d| !d.contains(b"T") && d.contains(b"Parent"));
        if nameless_kid {
            if let Some(root) = unlisted_root(doc, r, &seen.fields, &seen.withheld) {
                walk(doc, root, &base, &page_of, &mut seen, &mut out, 0);
            }
            if seen.widgets.contains(&r) || seen.withheld.contains(&r) {
                continue;
            }
        }
        walk(doc, r, &base, &page_of, &mut seen, &mut out, 0);
    }
    let listed = out.len();
    adopt_page_fields(doc, &pages, &base, &page_of, &mut seen, &mut out);
    let adopted = out.len() - listed;
    rank_tabs(doc, &pages, &annot_index, &mut out);
    Reading { items: out, adopted_page_fields: adopted, left_out: seen.left_out, values_at: seen.values_at }
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
        let parent = doc.get(top).as_dict().and_then(|d| parent_of(doc, d));
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
    /// [`Reading::values_at`].
    values_at: std::collections::HashMap<ObjRef, (Option<ObjRef>, Option<ObjRef>)>,
    /// The `/Kids` arrays given by reference that [`withhold_below`] has read, by each reference on
    /// the way to them, and whether each makes its node a branch; a chain that ends nowhere, by its
    /// first reference, as not.
    withheld_arrays: std::collections::HashMap<ObjRef, bool>,
    /// The fields `/Fields` lists: root fields, with no ancestors (ISO 32000-2 §12.7.3, Table 224).
    roots: std::collections::HashSet<ObjRef>,
    /// The fields being walked, outermost first, whose `/Kids` are being read.
    path: Vec<ObjRef>,
    /// The page widgets (no `/T`) that name each field as their `/Parent`, in page-annotation order.
    by_parent: std::collections::HashMap<ObjRef, Vec<ObjRef>>,
    scripts: Allowance,
    values: Allowance,
    /// The values of text fields with the Password flag, apart from the rest: other fields can't
    /// spend what a save needs to find and clear them, or the open document to keep them.
    passwords: Allowance,
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

/// Leaves out a field read up to its own text, with `claimed`, its widgets, which are withheld: a
/// later field that lists one takes it, but none is adopted as a field of its own.
fn leave_out(claimed: std::collections::HashSet<ObjRef>, seen: &mut Seen) {
    seen.withheld.extend(claimed);
    seen.left_out = seen.left_out.saturating_add(1);
}

/// Withholds the fields and widgets under `r`, a field left out of the reading for its name: none
/// is adopted from the page as a field of its own, or read from `/Fields` as a root, and a later
/// field in the tree that lists one still takes it as a widget. Each is visited once, its text
/// unread, so the walk ends however the kids loop. How many terminal fields it holds, `r` among
/// them: those with a name that aren't branches ([`withhold_kids`]).
///
/// It holds back everything `r`'s `/Kids` reach and the page widgets naming any of it, which is
/// all [`walk`] would read under `r` and may be more: a root `/Fields` lists that the walk passes
/// over as no child of the field listing it (`not_a_child`), a node past the walk's depth, or a
/// node under a widget. That direction is the safe one. A node held back that the walk would have
/// read elsewhere is only missing from the reading, which already counts a field left out, so a
/// save of a form with a password field refuses (`clear_password_values`); a node the walk would
/// read under `r` that wasn't held back would be read from `/Fields` or adopted from the page as a
/// root, under a name without `r`'s part.
fn withhold_below(doc: &Document, r: ObjRef, seen: &mut Seen) -> usize {
    let mut terminal = 0usize;
    seen.withheld.insert(r);
    let mut stack = vec![r];
    while let Some(n) = stack.pop() {
        let node = doc.get(n);
        let Some(d) = node.as_dict() else { continue };
        let branch = match d.get(b"Kids") {
            // A `/Kids` array given by reference is read once in a reading: the references on the
            // way to it, followed as far as `Document::resolve` follows them, are noted with
            // whether it makes a branch once the walk reaches it, and a node naming one of them
            // loads nothing. A chain that ends nowhere notes only its first.
            Some(Object::Ref(first)) => {
                let (mut r, mut hops, mut branch) = (*first, Vec::new(), None);
                for _ in 0..=32 {
                    if let Some(&known) = seen.withheld_arrays.get(&r) {
                        branch = Some(known);
                        break;
                    }
                    hops.push(r);
                    let o = doc.get(r);
                    #[cfg(test)]
                    WITHHELD_LOADS.with(|c| c.set(c.get() + 1));
                    match &*o {
                        Object::Ref(next) => r = *next,
                        list => {
                            branch = Some(withhold_kids(doc, list, seen, &mut stack));
                            break;
                        }
                    }
                }
                match branch {
                    Some(known) => seen.withheld_arrays.extend(hops.into_iter().map(|hop| (hop, known))),
                    None => seen.withheld_arrays.extend(hops.first().map(|&hop| (hop, false))),
                }
                branch.unwrap_or(false)
            }
            Some(list) => withhold_kids(doc, list, seen, &mut stack),
            None => false,
        };
        // A page widget naming `n` as its `/Parent` is its widget even when its `/Kids` leave it
        // out, as [`walk`] reads it.
        for &w in seen.by_parent.get(&n).into_iter().flatten() {
            if seen.withheld.insert(w) {
                stack.push(w);
            }
        }
        if !branch && d.contains(b"T") {
            terminal = terminal.saturating_add(1);
        }
    }
    terminal
}

/// Withholds the kids `list` holds, pushing those not withheld before onto `stack`; whether they
/// make their node a branch rather than a field: one of them has a name, and none is a widget
/// beside it, which [`walk`] reads as the node's own.
fn withhold_kids(doc: &Document, list: &Object, seen: &mut Seen, stack: &mut Vec<ObjRef>) -> bool {
    let kids: Vec<ObjRef> = list.as_array().into_iter().flatten().filter_map(Object::as_ref).collect();
    #[cfg(test)]
    WITHHELD_ENTRIES.with(|c| c.set(c.get() + kids.len()));
    let named = |k: &ObjRef| doc.get(*k).as_dict().is_some_and(|d| d.contains(b"T"));
    // `/Subtype` only when direct, as [`walk`] reads it to keep the widgets beside named kids.
    let widget = |k: &ObjRef| doc.get(*k).as_dict().is_some_and(|d| !d.contains(b"T") && d.name(b"Subtype") == Some(b"Widget"));
    let branch = kids.iter().any(named) && !kids.iter().any(widget);
    for k in kids {
        if seen.withheld.insert(k) {
            stack.push(k);
        }
    }
    branch
}

#[cfg(test)]
thread_local! {
    /// `/Kids` entries [`withhold_below`] read on this thread.
    pub(crate) static WITHHELD_ENTRIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Objects [`withhold_below`] loaded on the way to a `/Kids` array on this thread.
    pub(crate) static WITHHELD_LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The field a page widget `r` names as its `/Parent`, when `r` is a widget and not a field of its
/// own (no `/T`).
fn widget_parent(doc: &Document, r: ObjRef) -> Option<ObjRef> {
    let obj = doc.get(r);
    let d = obj.as_dict()?;
    if d.name(b"Subtype") != Some(b"Widget") || d.contains(b"T") {
        return None;
    }
    d.reference(b"Parent")
}

/// Whether `k`, listed in the `/Kids` of field `r`, is not a child of `r` after all: `r` itself or
/// a field `r` descends from (a loop), or a root field (one `/Fields` lists) that doesn't name `r`
/// as its `/Parent`. Root fields have no ancestors (ISO 32000-2 §12.7.3, Table 224), and a
/// child names its parent (§12.7.4.1, Table 226): only the `/Kids` entry says otherwise, so the
/// field is read as the root it is, and `r` keeps its other kids.
fn not_a_child(doc: &Document, seen: &Seen, r: ObjRef, k: ObjRef) -> bool {
    k == r || seen.path.contains(&k) || (seen.roots.contains(&k) && doc.get(k).as_dict().and_then(|d| d.reference(b"Parent")) != Some(r))
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
    inh.take(doc, r, &obj);
    // A name that doesn't fit leaves the field out, with the fields under it, whose full names
    // need it; a `/DA` that doesn't fit, only the fields that would draw with it.
    if inh.name_refused || seen.text.refusals() != refusals {
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
    let (field_kids, mut kids): (Vec<ObjRef>, Vec<ObjRef>) =
        kids.into_iter().partition(|k| doc.get(*k).as_dict().is_some_and(|kd| kd.contains(b"T")));
    let field_kids: Vec<ObjRef> = field_kids.into_iter().filter(|k| !not_a_child(doc, seen, r, *k)).collect();
    if !field_kids.is_empty() {
        seen.path.push(r);
        for k in field_kids {
            // A fully qualified name is built up the field's own `/Parent` chain (ISO 32000-2
            // §12.7.4.2). A kid that doesn't name this field as its `/Parent` is named from its
            // own chain, as other readers name it; it still inherits this field's attributes.
            // The `/Parent` as written: `qualified_name` follows an object that only refers on,
            // and names a missing one as no parent.
            let parent = doc.get(k).as_dict().and_then(|kd| kd.reference(b"Parent"));
            if parent == Some(r) {
                walk(doc, k, &inh, page_of, seen, out, depth + 1);
            } else {
                let mut own = inh.clone();
                let refusals = seen.text.refusals();
                own.name = Rc::from(parent.and_then(|p| qualified_name(doc, p, &mut seen.text)).unwrap_or_default());
                own.name_refused = seen.text.refusals() != refusals;
                walk(doc, k, &own, page_of, seen, out, depth + 1);
            }
        }
        seen.path.pop();
        // "A field's children in the hierarchy may also include widget annotations" (ISO 32000-2
        // §12.7.4): widget annotations listed beside named kids are still this field's widgets,
        // read as a field of this name next to the named kids.
        kids.retain(|k| doc.get(*k).as_dict().is_some_and(|kd| kd.name(b"Subtype") == Some(b"Widget")));
        if kids.is_empty() {
            return;
        }
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
    // A child names its parent (ISO 32000-2 §12.7.4.1, Table 226): a page widget naming this field
    // is one of its widgets even when its `/Kids` leave it out, as MuPDF and PDFium read it.
    // A field's widgets are its widget kids. The field dictionary is its own widget only when it
    // has none (no kid whose `/Subtype` is `/Widget`): field and widget are merged "when a field has only a single associated widget
    // annotation" (ISO 32000-2 §12.7.4), so a merged dictionary that also lists widget kids is a
    // field whose buttons are those kids, not one more (often zero-area) button.
    let has_widget_kids = kids.iter().any(|k| doc.get(*k).as_dict().is_some_and(|kd| kd.name(b"Subtype") == Some(b"Widget")));
    let widget_refs: Vec<ObjRef> = if d.contains(b"Rect") && !has_widget_kids {
        vec![r]
    } else {
        kids.into_iter().chain(seen.by_parent.get(&r).into_iter().flatten().copied()).collect()
    };
    // Each widget once, claimed only once the field is read whole, so a later field listing it
    // has it when this one is left out.
    let mut claimed = std::collections::HashSet::new();
    let widget_refs: Vec<ObjRef> = widget_refs.into_iter().filter(|w| !seen.widgets.contains(w) && claimed.insert(*w)).collect();
    // A field that would draw with a `/DA` that didn't fit is left out before it reads its
    // scripts; a signature, drawn without one, is read.
    if inh.da_refused && kind != FieldKind::Signature {
        leave_out(claimed, seen);
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
                mouse_up_script: if kind == FieldKind::Text {
                    let aa = wd.get(b"AA").map(|a| doc.resolve(a));
                    let action = aa.as_deref().and_then(Object::as_dict).and_then(|aa| aa.get(b"U")).or_else(|| wd.get(b"A"));
                    action
                        .filter(|a| doc.resolve(a).as_dict().and_then(|a| a.name(b"S")) == Some(b"JavaScript"))
                        .and_then(|a| script(doc, a, &mut seen.scripts))
                } else {
                    None
                },
                mouse_down: wd.get(b"AA").map(|a| doc.resolve(a)).as_deref().and_then(Object::as_dict).is_some_and(|aa| aa.contains(b"D")),
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
        leave_out(claimed, seen);
        return;
    }
    seen.widgets.extend(claimed);
    let values = if kind == FieldKind::Text && ff & flags::PASSWORD != 0 { &mut seen.passwords } else { &mut seen.values };
    let (value, default, selected, single_value_is_array) = field_values(doc, d, &inh, kind, &options, values);
    seen.values_at.insert(r, (inh.v_at, inh.dv_at));
    // Decided where the walk took the default, so Clear form and a script's reset agree.
    let default_unread = default.is_empty() && inh.dv_at.is_some_and(|at| crate::values::holds_own(doc, at, b"DV"));
    out.push(Field {
        name: inh.name.to_string(),
        obj: r,
        kind,
        value,
        default,
        default_unread,
        flags: ff,
        max_len: inh.max_len,
        options,
        selected,
        single_value_is_array,
        da: field_da.to_string(),
        quadding: inh.q.unwrap_or(0),
        tooltip,
        widgets,
        actions,
        button,
    });
}

/// A field's value and default, read from the fields that carry them against `values`; the options
/// of a choice field they select ([`selected_options`], with its `/I`); and whether a single-select
/// list box's value is an array.
fn field_values(
    doc: &Document,
    d: &Dict,
    inh: &Inherited,
    kind: FieldKind,
    options: &[(String, String)],
    values: &mut Allowance,
) -> (Vec<String>, Vec<String>, Vec<usize>, bool) {
    let v_entry = inh.v.as_deref().and_then(Object::as_dict).and_then(|f| f.get(b"V"));
    let mut value = values_of(doc, v_entry, values);
    // A list box without MultiSelect selects one option: "if clear, at most one item shall be
    // selected" (ISO 32000-2 Table 233). Given an array /V or /DV anyway, it holds the first
    // value only, and shows and resets to it; the file keeps its entries until the field is filled.
    let mut default = values_of(doc, inh.dv.as_deref().and_then(Object::as_dict).and_then(|f| f.get(b"DV")), values);
    let single = kind == FieldKind::List && inh.ff.unwrap_or(0) & flags::MULTI_SELECT == 0;
    // Read where it is: a value many fields inherit isn't copied for each.
    let single_value_is_array = single && v_entry.is_some_and(|v| held(doc, v).as_array().is_some());
    if single {
        value.truncate(1);
        default.truncate(1);
    }
    let mut selected = match kind {
        FieldKind::Combo | FieldKind::List => selected_options(options, &value, listed_indices(doc, d, options.len()).as_deref()),
        _ => Vec::new(),
    };
    if single {
        selected.truncate(1);
    }
    (value, default, selected, single_value_is_array)
}

pub(crate) fn field_flags(doc: &Document, dict: &Dict) -> Option<u32> {
    dict.get(b"Ff").and_then(|value| doc.resolve(value).as_int()).and_then(|value| u32::try_from(value).ok())
}
