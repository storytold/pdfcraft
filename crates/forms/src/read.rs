use super::tab_order::rank_tabs;
use super::*;

/// At most this many `/State` entries of a set-layer-visibility action are read.
const MAX_LAYER_STATE: usize = 1024;

/// A push button's mouse-up action (`/A`, or `/AA /U`, on the field or its widget).
fn button_action(doc: &Document, d: &Dict, widgets: &[ObjRef], scripts: &mut DecodeBudget) -> Option<af::ButtonAction> {
    // A button PdfCraft generated for an XFA image field (`/PCXfaImage true`, no PDF action):
    // clicking it picks a picture, as `buttonImportIcon()` does.
    if matches!(d.get(b"PCXfaImage").map(|o| doc.resolve(o)).as_deref(), Some(Object::Bool(true))) {
        return Some(af::ButtonAction::ImportIcon);
    }
    let pick = |dict: &Dict| -> Option<Object> {
        dict.get(b"A").cloned().or_else(|| dict.get(b"AA").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().and_then(|aa| aa.get(b"U").cloned())))
    };
    let action = pick(d).or_else(|| widgets.iter().find_map(|w| doc.get(*w).as_dict().and_then(pick)))?;
    let a = doc.resolve(&action);
    let a = a.as_dict()?;
    let s = |k: &[u8]| a.get(k).and_then(|v| text_of(&doc.resolve(v)));
    Some(match a.name(b"S")? {
        b"ResetForm" => {
            let fields = a
                .get(b"Fields")
                .map(|f| doc.resolve(f))
                .and_then(|f| f.as_array().cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|o| match o {
                    Object::Ref(r) => doc.get(*r).as_dict().and_then(|fd| fd.get(b"T").and_then(|t| text_of(&doc.resolve(t)))),
                    other => text_of(&doc.resolve(other)),
                })
                .collect();
            let exclude = a.get(b"Flags").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0) & 1 != 0;
            af::ButtonAction::Reset { fields, exclude }
        }
        b"Named" => af::ButtonAction::Named(String::from_utf8_lossy(a.name(b"N")?).into_owned()),
        b"URI" => af::ButtonAction::Uri(s(b"URI")?),
        b"SubmitForm" => af::ButtonAction::Submit(
            a.get(b"F")
                .map(|f| doc.resolve(f))
                .and_then(|f| f.as_dict().and_then(|fd| fd.get(b"F").and_then(|x| text_of(&doc.resolve(x)))).or_else(|| text_of(&f)))
                .unwrap_or_default(),
        ),
        b"GoTo" => {
            let dest = doc.resolve(a.get(b"D")?);
            let target = dest.as_array()?.first()?.as_ref()?;
            af::ButtonAction::GoTo(page_refs(doc).iter().position(|p| *p == target)?)
        }
        b"Hide" => {
            // /T: a field name, an annotation (or field) reference, or an array of them.
            let target = |o: &Object| match o {
                Object::Ref(r) => qualified_name(doc, *r).or_else(|| text_of(&doc.resolve(o))),
                other => text_of(other),
            };
            let fields = match a.get(b"T") {
                Some(t) => match &*doc.resolve(t) {
                    Object::Array(items) => items.iter().filter_map(target).collect(),
                    _ => target(t).into_iter().collect(),
                },
                None => Vec::new(),
            };
            let hide = !a.get(b"H").is_some_and(|h| matches!(&*doc.resolve(h), Object::Bool(false)));
            af::ButtonAction::ShowHide { fields, hide }
        }
        b"SetOCGState" => {
            // /State: ON, OFF or Toggle, each followed by the groups it applies to.
            let mut changes = Vec::new();
            if let Some(state) = a.get(b"State") {
                let mut op = None;
                for item in doc.resolve(state).as_array().into_iter().flatten().take(MAX_LAYER_STATE) {
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
        b"JavaScript" => af::button_script(&script(doc, &action, scripts)?),
        _ => return None,
    })
}

/// The fully qualified name of the field that `r` (a field or one of its widgets) belongs to,
/// from the partial names (`/T`) up its `/Parent` chain.
fn qualified_name(doc: &Document, r: ObjRef) -> Option<String> {
    let mut parts = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut cur = Some(r);
    while let Some(c) = cur {
        if seen.len() > 64 || !seen.insert(c) {
            break;
        }
        let obj = doc.get(c);
        let d = obj.as_dict()?;
        if let Some(t) = d.get(b"T").and_then(|o| text_of(&doc.resolve(o))) {
            parts.push(t);
        }
        cur = d.get(b"Parent").and_then(|p| p.as_ref());
    }
    parts.reverse();
    (!parts.is_empty()).then(|| parts.join("."))
}

/// JavaScript streams one reading of the form (its fields, or its document scripts) may decode
/// in all, however many actions name the same stream.
pub(crate) const MAX_SCRIPT_BYTES: usize = 64 << 20;

/// A budget for the script streams of one reading of the form.
pub(crate) fn script_budget() -> DecodeBudget {
    DecodeBudget::new(MAX_SCRIPT_BYTES, 0)
}

/// The JavaScript of an action dictionary (`/JS` string or stream). A stream is decoded against
/// `budget`, and is `None` once it is spent.
pub(crate) fn script(doc: &Document, action: &Object, budget: &mut DecodeBudget) -> Option<String> {
    let a = doc.resolve(action);
    let js = a.as_dict()?.get(b"JS")?.clone();
    match &*doc.resolve(&js) {
        Object::String(s) => Some(s.to_text()),
        Object::Stream(s) => budget.decode(s).ok().map(|b| String::from_utf8_lossy(&b).into_owned()),
        _ => None,
    }
}

/// A field's format/keystroke, validate and calculate scripts (field dictionary, or its only
/// widget's when the field has none).
fn actions_of(doc: &Document, d: &Dict, widgets: &[ObjRef], scripts: &mut DecodeBudget) -> af::Actions {
    let aa = d.get(b"AA").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned()).or_else(|| {
        (widgets.len() == 1)
            .then(|| doc.get(widgets[0]))
            .and_then(|w| w.as_dict().and_then(|wd| wd.get(b"AA").map(|a| doc.resolve(a))))
            .and_then(|a| a.as_dict().cloned())
    });
    let mut out = af::Actions::default();
    let Some(aa) = aa else { return out };
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

fn values_of(doc: &Document, o: Option<&Object>) -> Vec<String> {
    let Some(o) = o else { return Vec::new() };
    match &*doc.resolve(o) {
        Object::Array(a) => a.iter().filter_map(|x| text_of(&doc.resolve(x))).collect(),
        Object::Stream(s) => s.decoded().ok().map(|b| PdfString::literal(b).to_text()).into_iter().collect(),
        other => text_of(other).filter(|s| s != "Off" && !s.is_empty()).into_iter().collect(),
    }
}

fn nums(doc: &Document, o: Option<&Object>) -> Option<Vec<f64>> {
    let o = doc.resolve(o?);
    o.as_array()?.iter().map(|x| doc.resolve(x).as_f64()).collect()
}

/// Leaf page objects in order (a small walker; `organize` and `annot` have their own).
pub(crate) fn page_refs(doc: &Document) -> Vec<ObjRef> {
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

#[derive(Clone, Default)]
struct Inherited {
    name: String,
    ft: Option<Vec<u8>>,
    ff: Option<u32>,
    v: Option<Object>,
    dv: Option<Object>,
    da: Option<String>,
    q: Option<i64>,
    max_len: Option<usize>,
}

/// Every terminal field: the `/Fields` tree in order, then the fields reachable only through the
/// page annotations (see [`adopt_page_fields`]).
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
}

/// Read all terminal fields and the page-only adoption count in one enumeration.
pub fn fields_with_adopted(doc: &Document) -> Fields {
    let (items, adopted_page_fields) = enumerate(doc);
    Fields { items, adopted_page_fields }
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

pub(crate) fn enumerate(doc: &Document) -> (Vec<Field>, usize) {
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
    let base = match &af {
        Some(af) => Inherited {
            da: af.get(b"DA").and_then(|o| text_of(&doc.resolve(o))),
            q: af.get(b"Q").and_then(|o| doc.resolve(o).as_int()),
            ..Default::default()
        },
        None => Inherited::default(),
    };
    let mut out = Vec::new();
    let mut seen = Seen { fields: std::collections::HashSet::new(), widgets: std::collections::HashSet::new(), scripts: script_budget() };
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
    (out, adopted)
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
            if seen.fields.contains(&r) || seen.widgets.contains(&r) {
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
            if let Some(root) = unlisted_root(doc, r, &seen.fields) {
                walk(doc, root, base, page_of, seen, out, 0);
            }
        }
    }
}

/// The topmost field above `r` (itself without a `/Parent`), or `None` when `r` or an ancestor is
/// already reachable from the `/Fields` tree, or a `/Parent` loop hides the top.
fn unlisted_root(doc: &Document, r: ObjRef, seen: &std::collections::HashSet<ObjRef>) -> Option<ObjRef> {
    let mut top = r;
    let mut visited = std::collections::HashSet::from([r]);
    loop {
        let parent = doc.get(top).as_dict().and_then(|d| d.reference(b"Parent"));
        let Some(p) = parent else {
            return if seen.contains(&top) { None } else { Some(top) };
        };
        if !visited.insert(p) || visited.len() > 64 {
            return None;
        }
        if seen.contains(&p) {
            return None;
        }
        top = p;
    }
}

/// What one reading of the form's fields has used: the field and widget dictionaries read, and
/// the script budget their actions share.
struct Seen {
    fields: std::collections::HashSet<ObjRef>,
    /// A widget belongs to the first field that lists it, however often it is listed.
    widgets: std::collections::HashSet<ObjRef>,
    scripts: DecodeBudget,
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
    if let Some(t) = d.get(b"T").and_then(|o| text_of(&doc.resolve(o))) {
        inh.name = if inh.name.is_empty() { t } else { format!("{}.{t}", inh.name) };
    }
    if let Some(ft) = d.name(b"FT") {
        inh.ft = Some(ft.to_vec());
    }
    if let Some(ff) = field_flags(doc, d) {
        inh.ff = Some(ff);
    }
    for (k, slot) in [(&b"V"[..], &mut inh.v), (b"DV", &mut inh.dv)] {
        if let Some(v) = d.get(k) {
            *slot = Some(v.clone());
        }
    }
    if let Some(da) = d.get(b"DA").and_then(|o| text_of(&doc.resolve(o))) {
        inh.da = Some(da);
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
    let widget_refs: Vec<ObjRef> = widget_refs.into_iter().filter(|w| seen.widgets.insert(*w)).collect();
    let actions = actions_of(doc, d, &widget_refs, &mut seen.scripts);
    let button = (inh.ft.as_deref() == Some(b"Btn") && inh.ff.unwrap_or(0) & flags::PUSH_BUTTON != 0)
        .then(|| button_action(doc, d, &widget_refs, &mut seen.scripts))
        .flatten();
    let widgets = widget_refs
        .into_iter()
        .filter_map(|w| {
            let wo = doc.get(w);
            let wd = wo.as_dict()?;
            let rect = nums(doc, wd.get(b"Rect")).filter(|r| r.len() == 4).unwrap_or_else(|| vec![0.0; 4]);
            let annot_flags = wd.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
            let own_ff = if w == r { 0 } else { field_flags(doc, wd).unwrap_or(0) };
            let on_state = wd
                .get(b"AP")
                .map(|ap| doc.resolve(ap))
                .and_then(|ap| ap.as_dict().and_then(|a| a.get(b"N").cloned()))
                .and_then(|n| doc.resolve(&n).as_dict().cloned())
                .and_then(|n| n.iter().map(|(k, _)| name_text(k)).find(|k| k != "Off"));
            Some(Widget {
                obj: w,
                page: page_of.get(&w).copied(),
                rect: [rect[0].min(rect[2]), rect[1].min(rect[3]), rect[0].max(rect[2]), rect[1].max(rect[3])],
                on_state: on_state.filter(|_| matches!(kind, FieldKind::CheckBox | FieldKind::Radio)),
                state: wd.name(b"AS").map(name_text),
                tab: usize::MAX,
                locked: annot_flags & 128 != 0,
                hidden: annot_flags & (2 | 32) != 0,
                read_only: (ff | own_ff) & flags::READ_ONLY != 0,
                rotation: appearance::mk_rotation(doc, wd),
                caption: (kind == FieldKind::PushButton)
                    .then(|| {
                        let mk = doc.resolve(wd.get(b"MK")?);
                        let value = doc.resolve(mk.as_dict()?.get(b"CA")?);
                        value.as_string().map(|s| s.to_text())
                    })
                    .flatten(),
                list: (kind == FieldKind::List).then(|| appearance::list_rows_of(doc, inh.da.as_deref().unwrap_or("/Helv 0 Tf 0 g"), Some(d), wd)),
                round: kind == FieldKind::Radio || (kind == FieldKind::CheckBox && appearance::is_round(doc, wd)),
            })
        })
        .collect();
    let options = d
        .get(b"Opt")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|o| match &*doc.resolve(o) {
            Object::Array(pair) if pair.len() >= 2 => Some((text_of(&doc.resolve(&pair[0]))?, text_of(&doc.resolve(&pair[1]))?)),
            other => text_of(other).map(|t| (t.clone(), t)),
        })
        .collect();
    out.push(Field {
        name: inh.name.clone(),
        obj: r,
        kind,
        value: values_of(doc, inh.v.as_ref()),
        default: values_of(doc, inh.dv.as_ref()),
        flags: ff,
        max_len: inh.max_len,
        options,
        da: inh.da.clone().unwrap_or_else(|| "/Helv 0 Tf 0 g".into()),
        quadding: inh.q.unwrap_or(0),
        tooltip: d.get(b"TU").and_then(|o| text_of(&doc.resolve(o))),
        widgets,
        actions,
        button,
    });
}

pub(crate) fn field_flags(doc: &Document, dict: &Dict) -> Option<u32> {
    dict.get(b"Ff").and_then(|value| doc.resolve(value).as_int()).and_then(|value| u32::try_from(value).ok())
}
