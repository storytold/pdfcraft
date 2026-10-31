//! Turn laid-out pages into PDF objects inside an open document: page dictionaries with content
//! streams (standard-14 fonts, WinAnsi), image XObjects and AcroForm widgets. Widget appearance
//! streams are left empty here; the forms layer regenerates them from the field dictionaries.

use std::collections::{BTreeMap, HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};
use pdfcraft_fonts::{literal, win_ansi};

use crate::XfaError;
use crate::layout::{Action, BorderShape, Form, Item, Page, Rounded, Widget, WidgetKind};
use crate::model::Rect;
use crate::text::{Face, Family, Span};

mod common;
mod strip;

pub use common::jpeg_size;
pub(crate) use common::rounded;
use common::{all_faces, content_hash, empty_form, hex32, jpeg_info, js_action, n, parse_hex32, rgb, rgb_array, standard_font, state_name};
pub(crate) use strip::USAGE_RIGHTS_REMOVED;
use strip::drop_usage_rights;
pub use strip::{Stripped, XfaShape, existing_layout, strip, xfa_shape};

/// Private key on generated fields: the template's SOM path, so data can be put back.
pub const SOM_KEY: &[u8] = b"PCSom";
/// Private key on the AcroForm: what laying the form out produced, so a reopened file is not
/// laid out again.
pub const LAYOUT_KEY: &[u8] = b"PCXfaLayout";
/// Private key on generated push buttons whose template has a click script: the button has no
/// PDF action (the script is XFA's, not Acrobat JavaScript); PdfCraft runs the script by the
/// field's SOM path.
pub const CLICK_KEY: &[u8] = b"PCXfaClick";
/// Private key on the fields generated to name a signature field by its SOM path, one for each
/// step of the path above it (`form1[0]`, then `page1[0]`): a new layout replaces them with the
/// fields they hold.
pub const NODE_KEY: &[u8] = b"PCXfaNode";

pub struct Written {
    pub pages: usize,
    pub fields: usize,
    pub warnings: Vec<String>,
    /// Usage rights were taken off the catalog (laying the form out voided them).
    pub usage_rights: bool,
}

/// On a push button generated for an XFA image field: clicking it picks a picture (the forms
/// layer reads this key).
pub const IMAGE_FIELD_KEY: &[u8] = b"PCXfaImage";

/// On a check box or radio button generated for a round XFA check button (`shape="round"`): its
/// background and border are drawn as a circle, not the widget's rectangle (the forms layer
/// reads this key).
pub const ROUND_KEY: &[u8] = b"PCXfaRound";

/// On an image XObject written for a picture: the SHA-256 (hex) of the picture's bytes. A
/// re-layout (after every script event) finds the XObjects on the pages it replaces by this,
/// and reuses them instead of decoding and embedding the same picture again. Nothing is kept
/// in the catalog, and a match is by the picture's own hash, never by a map read from the file.
const IMAGE_SHA_KEY: &[u8] = b"PCXfaSHA256";
/// Most pages searched for earlier pictures.
const MAX_PAGES_SEARCHED: usize = 10_000;
/// Most pictures remembered.
const MAX_REMEMBERED_IMAGES: usize = 1000;

struct Emitter<'a> {
    doc: &'a mut Document,
    /// Image XObjects by content hash: those from earlier passes, then this one's.
    images: HashMap<[u8; 32], ObjRef>,
    fields: Vec<ObjRef>,
    radio_groups: HashMap<String, (ObjRef, Vec<ObjRef>)>,
    /// The on state selected in each radio group, from the data.
    radio_selected: HashMap<String, String>,
    field_count: usize,
    warnings: Vec<String>,
    /// The signatures the file holds, by the SOM path of the field signed.
    signed: HashMap<String, Signed>,
    /// The full names of the fields the file came with, which a generated name must not repeat.
    taken: HashSet<String>,
    /// The fields above the signature fields named by their SOM paths, by the path they name:
    /// each one's object and the fields it holds.
    nodes: HashMap<String, (ObjRef, Vec<ObjRef>)>,
}

/// A signature field of the file that has been signed.
struct Signed {
    /// Its `V`: the signature dictionary (a reference, as the file has it).
    value: Object,
    /// Its widget's `AP`: what the signer's application drew in the box.
    appearance: Option<Object>,
}

/// Most fields looked at for signatures, counted as they are queued.
const MAX_FIELDS_SEARCHED: usize = 20_000;
/// Deepest field nesting followed.
const MAX_FIELD_DEPTH: usize = 32;
/// Most `/Fields` and `/Kids` entries read while looking, repeats included.
const MAX_KIDS_READ: usize = 1 << 20;

/// The fields the document came with: the full name of each, and the signed signature fields by
/// SOM path. Signing applications name a dynamic form's fields by their SOM path
/// (`form1[0].page1[0].sign[0]`), which is how a laid-out signature box finds its signature. A
/// field an earlier layout pass generated gives no name, but a signature on it is kept, by its
/// `/PCSom`, so a new layout keeps the box signed.
fn existing_fields(doc: &Document, catalog: &Dict) -> (HashMap<String, Signed>, HashSet<String>) {
    let mut out = HashMap::new();
    let mut names = HashSet::new();
    let Some(acro) = catalog.get(b"AcroForm").map(|a| doc.resolve(a)) else { return (out, names) };
    let Some(fields) = acro.as_dict().and_then(|a| a.get(b"Fields")).map(|f| doc.resolve(f)) else { return (out, names) };
    let mut walk = FieldQueue::default();
    walk.queue(fields.as_array().map(Vec::as_slice).unwrap_or_default(), "", false, 0);
    while let Some((field, prefix, parent_is_sig, depth)) = walk.stack.pop() {
        let field = doc.get(field);
        let Some(d) = field.as_dict() else { continue };
        if d.contains(NODE_KEY) {
            // Generated to name a signature field: only what it holds is looked at.
            if depth < MAX_FIELD_DEPTH {
                let kids = d.get(b"Kids").map(|k| doc.resolve(k));
                walk.queue(kids.as_deref().and_then(Object::as_array).map(Vec::as_slice).unwrap_or_default(), "", false, depth + 1);
            }
            continue;
        }
        if d.contains(SOM_KEY) {
            let som = d.get(SOM_KEY).map(|s| doc.resolve(s)).and_then(|s| s.as_string().map(|s| s.to_text()));
            if d.name(b"FT") == Some(b"Sig")
                && let Some(som) = som
                && let Some(value) = d.get(b"V").filter(|v| doc.resolve(v).as_dict().is_some())
            {
                out.entry(som).or_insert(Signed { value: value.clone(), appearance: d.get(b"AP").cloned() });
            }
            continue;
        }
        let name = match d.get(b"T").map(|t| doc.resolve(t)).and_then(|t| t.as_string().map(|t| t.to_text())) {
            Some(t) if prefix.is_empty() => t,
            Some(t) => format!("{prefix}.{t}"),
            None => prefix,
        };
        if !name.is_empty() {
            names.insert(name.clone());
        }
        let is_sig = match d.name(b"FT") {
            Some(ft) => ft == b"Sig",
            None => parent_is_sig,
        };
        let kids = d.get(b"Kids").map(|k| doc.resolve(k));
        let kids = kids.as_deref().and_then(Object::as_array).map(Vec::as_slice).unwrap_or_default();
        if is_sig && let Some(value) = d.get(b"V").filter(|v| doc.resolve(v).as_dict().is_some()) {
            // The appearance is on the field when it is its own widget, or on its one widget.
            let appearance = d.get(b"AP").cloned().or_else(|| walk.find_map(kids, |k| doc.resolve(k).as_dict().and_then(|k| k.get(b"AP").cloned())));
            out.entry(name.clone()).or_insert(Signed { value: value.clone(), appearance });
        }
        if depth < MAX_FIELD_DEPTH {
            walk.queue(kids, &name, is_sig, depth + 1);
        }
    }
    (out, names)
}

/// The fields [`existing_fields`] still has to look at: each with its parent's full name,
/// whether it inherits a signature type, and its depth.
#[derive(Default)]
struct FieldQueue {
    stack: Vec<(ObjRef, String, bool, usize)>,
    /// Object numbers queued: [`Document::get`] finds a field by number alone, so `5 0 R` and
    /// `5 1 R` are the same field.
    seen: HashSet<u32>,
    read: usize,
}

impl FieldQueue {
    /// The first of `kids` for which `f` gives a value. The entries looked at count against
    /// [`MAX_KIDS_READ`] like those queued: signed fields sharing one long `/Kids` would
    /// otherwise each search all of it. Past the limit nothing more is looked at.
    fn find_map<T>(&mut self, kids: &[Object], mut f: impl FnMut(&Object) -> Option<T>) -> Option<T> {
        for k in kids {
            self.read += 1;
            if self.read > MAX_KIDS_READ {
                return None;
            }
            if let Some(v) = f(k) {
                return Some(v);
            }
        }
        None
    }

    /// Queue the fields `kids` lists, each the first time it is listed. Fields and kids are
    /// indirect references (ISO 32000-2 §12.7.3.1); anything else is skipped. A field listed
    /// again, by this list or another, under any generation, takes no room, and at most
    /// [`MAX_FIELDS_SEARCHED`] are ever queued: a field listing one kid many times, or fields
    /// sharing one long `/Kids`, would otherwise queue it all on every visit. Reading the repeats
    /// costs too, so at most [`MAX_KIDS_READ`] entries are read in all.
    fn queue(&mut self, kids: &[Object], prefix: &str, is_sig: bool, depth: usize) {
        for k in kids {
            self.read += 1;
            if self.seen.len() >= MAX_FIELDS_SEARCHED || self.read > MAX_KIDS_READ {
                return;
            }
            let Some(r) = k.as_ref() else { continue };
            if self.seen.insert(r.num) {
                self.stack.push((r, prefix.to_string(), is_sig, depth));
            }
        }
    }
}

impl Emitter<'_> {
    fn da(face: &Face) -> Vec<u8> {
        format!("/{} {} Tf {} rg", face.resource_name(), n(face.size), rgb(face.color)).into_bytes()
    }

    fn populate_widget_kind(&mut self, w: &Widget, d: &mut Dict, mk: &mut Dict) -> (i64, bool, Option<String>) {
        let r = w.rect;
        let mut ff: i64 = 0;
        if w.read_only {
            ff |= 1;
        }
        let mut top_level = true;
        // The SOM path of the generated field above this one, for a signature field named by its
        // SOM path.
        let mut under: Option<String> = None;
        match &w.kind {
            WidgetKind::Text | WidgetKind::Date(_) | WidgetKind::Password => {
                d.set(b"FT".to_vec(), Object::name("Tx"));
                if w.multiline {
                    ff |= 1 << 12;
                }
                if w.kind == WidgetKind::Password {
                    ff |= 1 << 13;
                }
                // DoNotScroll (ISO 32000-2 Table 228): the field takes no more than it shows.
                if w.no_scroll {
                    ff |= 1 << 23;
                }
                if let Some(m) = w.max_chars {
                    d.set(b"MaxLen".to_vec(), Object::Int(m.min(100_000) as i64));
                }
                let q = match w.h_align {
                    crate::model::HAlign::Center => 1,
                    crate::model::HAlign::Right => 2,
                    _ => 0,
                };
                if q != 0 {
                    d.set(b"Q".to_vec(), Object::Int(q));
                }
                if let Some(v) = &w.value {
                    d.set(b"V".to_vec(), Object::String(PdfString::text(v)));
                }
                if let Some(v) = &w.default {
                    d.set(b"DV".to_vec(), Object::String(PdfString::text(v)));
                }
                if let WidgetKind::Date(pattern) = &w.kind {
                    let mut aa = Dict::new();
                    aa.set(b"F".to_vec(), js_action(&format!("AFDate_FormatEx(\"{pattern}\");")));
                    aa.set(b"K".to_vec(), js_action(&format!("AFDate_KeystrokeEx(\"{pattern}\");")));
                    d.set(b"AA".to_vec(), Object::Dict(aa));
                }
            }
            WidgetKind::Choice { options, list_box, multi, editable } => {
                d.set(b"FT".to_vec(), Object::name("Ch"));
                if !list_box {
                    ff |= 1 << 17;
                    if *editable {
                        ff |= 1 << 18;
                    }
                }
                if *multi {
                    ff |= 1 << 21;
                }
                let opt: Vec<Object> = options
                    .iter()
                    .map(|(saved, shown)| {
                        if saved == shown {
                            Object::String(PdfString::text(shown))
                        } else {
                            Object::Array(vec![Object::String(PdfString::text(saved)), Object::String(PdfString::text(shown))])
                        }
                    })
                    .collect();
                d.set(b"Opt".to_vec(), Object::Array(opt));
                let q = match w.h_align {
                    crate::model::HAlign::Center => 1,
                    crate::model::HAlign::Right => 2,
                    _ => 0,
                };
                if q != 0 {
                    d.set(b"Q".to_vec(), Object::Int(q));
                }
                let values = |v: &str| -> Object {
                    let picked: Vec<&str> = if *multi { v.lines().filter(|l| !l.is_empty()).collect() } else { vec![v] };
                    match picked.as_slice() {
                        [one] => Object::String(PdfString::text(one)),
                        many => Object::Array(many.iter().map(|s| Object::String(PdfString::text(s))).collect()),
                    }
                };
                if let Some(v) = &w.value {
                    d.set(b"V".to_vec(), values(v));
                    // The selected indices (ISO 32000-2 Table 231): ascending, each once. Where
                    // two options save the same value the first is meant; the data can't say.
                    let picked: Vec<&str> = if *multi { v.lines().filter(|l| !l.is_empty()).collect() } else { vec![v.as_str()] };
                    let mut idx: Vec<usize> = picked.iter().filter_map(|p| options.iter().position(|(saved, _)| saved == p)).collect();
                    idx.sort_unstable();
                    idx.dedup();
                    if !idx.is_empty() {
                        d.set(b"I".to_vec(), Object::Array(idx.into_iter().map(|i| Object::Int(i as i64)).collect()));
                    }
                }
                if let Some(v) = &w.default {
                    // Already a saved value (the layout maps defaults as it maps data).
                    d.set(b"DV".to_vec(), values(v));
                }
            }
            WidgetKind::Signature => {
                d.set(b"FT".to_vec(), Object::name("Sig"));
                // Signed already: the box shows the signature the file holds and is done with.
                if let Some(signed) = self.signed.get(&w.som) {
                    d.set(b"V".to_vec(), signed.value.clone());
                    if let Some(ap) = &signed.appearance {
                        d.set(b"AP".to_vec(), ap.clone());
                    }
                    ff |= 1;
                }
                if let Some((above, last)) = self.som_steps(&w.som)
                    && let Some(parent) = self.node(&above)
                {
                    // Named by its SOM path, as fields under one for each step above it: other
                    // viewers lay a dynamic form out again and find a signature by that name.
                    d.set(b"T".to_vec(), Object::String(PdfString::text(&last)));
                    d.set(b"Parent".to_vec(), Object::Ref(parent));
                    top_level = false;
                    under = Some(above.join("."));
                }
            }
            WidgetKind::Image => {
                // A push button with nothing drawn: the forms layer treats one so marked as an
                // image field (clicking picks a picture), with no action other viewers run.
                d.set(b"FT".to_vec(), Object::name("Btn"));
                ff |= 1 << 16;
                d.set(IMAGE_FIELD_KEY.to_vec(), Object::Bool(true));
            }
            WidgetKind::CheckBox { on, .. } => {
                d.set(b"FT".to_vec(), Object::name("Btn"));
                let on = state_name(on);
                let checked = w.value.is_some();
                d.set(b"V".to_vec(), Object::name(if checked { &on } else { "Off" }));
                d.set(b"AS".to_vec(), Object::name(if checked { &on } else { "Off" }));
                if w.default.as_deref().is_some_and(|dv| crate::data::is_on(dv, &on)) {
                    d.set(b"DV".to_vec(), Object::name(&on));
                }
                let mut nd = Dict::new();
                nd.set(on.into_bytes(), empty_form(self.doc, r.w, r.h));
                nd.set(b"Off".to_vec(), empty_form(self.doc, r.w, r.h));
                let mut ap = Dict::new();
                ap.set(b"N".to_vec(), Object::Dict(nd));
                d.set(b"AP".to_vec(), Object::Dict(ap));
                d.set(b"PCItems".to_vec(), Object::Array(w.items.iter().map(|i| Object::String(PdfString::text(i))).collect()));
            }
            WidgetKind::Radio { group, on, .. } => {
                top_level = false;
                let parent = match self.radio_groups.get(group) {
                    Some((p, _)) => *p,
                    None => {
                        let mut g = Dict::new();
                        g.set(b"FT".to_vec(), Object::name("Btn"));
                        g.set(b"Ff".to_vec(), Object::Int((1 << 15) | (1 << 14)));
                        g.set(b"T".to_vec(), Object::String(PdfString::text(group)));
                        g.set(b"V".to_vec(), Object::name("Off"));
                        g.set(b"DA".to_vec(), Object::String(PdfString::literal(Self::da(&w.face))));
                        g.set(b"Kids".to_vec(), Object::Array(Vec::new()));
                        // The group's SOM path: the button's without its last step.
                        let group_som = w.som.rsplit_once('.').map(|(g, _)| g.to_string()).unwrap_or_else(|| w.som.clone());
                        g.set(SOM_KEY.to_vec(), Object::String(PdfString::text(&group_som)));
                        let p = self.doc.add(Object::Dict(g));
                        self.radio_groups.insert(group.clone(), (p, Vec::new()));
                        self.fields.push(p);
                        self.field_count += 1;
                        p
                    }
                };
                d.set(b"Parent".to_vec(), Object::Ref(parent));
                // Appearance states are PDF names, so a space becomes an underscore. The datasets
                // still hold the template's text (`/PCOn`); writing the name back would drop the selection.
                d.set(b"PCOn".to_vec(), Object::String(PdfString::text(on)));
                let state = state_name(on);
                let chosen = w.value.is_some();
                d.set(b"AS".to_vec(), Object::name(if chosen { &state } else { "Off" }));
                if chosen {
                    self.radio_selected.insert(group.clone(), state.clone());
                }
                let mut nd = Dict::new();
                nd.set(state.into_bytes(), empty_form(self.doc, r.w, r.h));
                nd.set(b"Off".to_vec(), empty_form(self.doc, r.w, r.h));
                let mut ap = Dict::new();
                ap.set(b"N".to_vec(), Object::Dict(nd));
                d.set(b"AP".to_vec(), Object::Dict(ap));
                d.remove(b"DA");
            }
            WidgetKind::Button { caption } => {
                d.set(b"FT".to_vec(), Object::name("Btn"));
                ff |= 1 << 16;
                mk.set(b"CA".to_vec(), Object::String(PdfString::text(caption)));
                write_button_action(w.action.as_ref(), d);
            }
        }
        (ff, top_level, under)
    }

    /// Write laid-out widget `w` on `page` as an AcroForm widget annotation (and its field), with
    /// its border, background and kind, and return the annotation's object.
    fn widget(&mut self, w: &Widget, page: &Page, page_ref: ObjRef) -> Result<Option<ObjRef>, XfaError> {
        let r = w.rect;
        let (x0, y0, x1, y1) = (r.x, page.height - r.bottom(), r.right(), page.height - r.y);
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name("Widget"));
        d.set(b"F".to_vec(), Object::Int(4));
        d.set(b"P".to_vec(), Object::Ref(page_ref));
        d.set(b"Rect".to_vec(), Object::Array([x0, y0, x1, y1].iter().map(|v| Object::Real(*v)).collect()));
        d.set(b"DA".to_vec(), Object::String(PdfString::literal(Self::da(&w.face))));
        if let Some(t) = &w.tooltip {
            d.set(b"TU".to_vec(), Object::String(PdfString::text(t)));
        }
        d.set(SOM_KEY.to_vec(), Object::String(PdfString::text(&w.som)));
        let mut mk = Dict::new();
        if let Some(c) = w.border.color
            && w.border.shape != BorderShape::None
        {
            mk.set(b"BC".to_vec(), rgb_array(c));
        }
        if let Some(c) = w.border.fill {
            mk.set(b"BG".to_vec(), rgb_array(c));
        }
        let mut bs = Dict::new();
        bs.set(b"W".to_vec(), Object::Real(if w.border.shape == BorderShape::None { 0.0 } else { w.border.width.clamp(0.1, 12.0) }));
        bs.set(b"S".to_vec(), Object::name(if w.border.shape == BorderShape::Underline { "U" } else { "S" }));
        d.set(b"BS".to_vec(), Object::Dict(bs));
        if matches!(w.kind, WidgetKind::CheckBox { round: true, .. } | WidgetKind::Radio { round: true, .. }) {
            d.set(ROUND_KEY.to_vec(), Object::Bool(true));
        }
        let (ff, top_level, under) = self.populate_widget_kind(w, &mut d, &mut mk);
        if !mk.is_empty() {
            d.set(b"MK".to_vec(), Object::Dict(mk));
        }
        if ff != 0 {
            d.set(b"Ff".to_vec(), Object::Int(ff));
        }
        if top_level {
            d.set(b"T".to_vec(), Object::String(PdfString::text(&w.name)));
        }
        let r = self.doc.add(Object::Dict(d));
        if top_level {
            self.fields.push(r);
            self.field_count += 1;
        } else if let Some(path) = under {
            if let Some((_, kids)) = self.nodes.get_mut(&path) {
                kids.push(r);
            }
            self.taken.insert(w.som.clone());
            self.field_count += 1;
        } else if let WidgetKind::Radio { group, .. } = &w.kind
            && let Some((_, kids)) = self.radio_groups.get_mut(group)
        {
            kids.push(r);
        }
        Ok(Some(r))
    }

    /// The steps of SOM path `som` above the last, and the last, when a field can be named by the
    /// path: two to [`MAX_FIELD_DEPTH`] steps, none empty, and no field of the file or of this
    /// pass already named by the path or by a part of it. `None` keeps the field's own name.
    fn som_steps(&self, som: &str) -> Option<(Vec<String>, String)> {
        let steps: Vec<&str> = som.split('.').collect();
        if steps.len() < 2 || steps.len() > MAX_FIELD_DEPTH || steps.iter().any(|s| s.is_empty()) {
            return None;
        }
        for i in 1..=steps.len() {
            let path = steps.get(..i)?.join(".");
            // A part already generated by this pass is shared; one the file has is not touched.
            let generated = i < steps.len() && self.nodes.contains_key(&path);
            if !generated && self.taken.contains(&path) {
                return None;
            }
        }
        let (last, above) = steps.split_last()?;
        Some((above.iter().map(|s| s.to_string()).collect(), last.to_string()))
    }

    /// The generated field named by the SOM path of `steps`, made (with those above it) when this
    /// pass has none yet; the topmost goes into the AcroForm's fields. `None` for no steps.
    fn node(&mut self, steps: &[String]) -> Option<ObjRef> {
        let mut parent: Option<(ObjRef, String)> = None;
        for step in steps {
            let path = match &parent {
                Some((_, above)) => format!("{above}.{step}"),
                None => step.clone(),
            };
            let r = match self.nodes.get(&path) {
                Some((r, _)) => *r,
                None => {
                    let mut d = Dict::new();
                    d.set(b"T".to_vec(), Object::String(PdfString::text(step)));
                    d.set(b"Kids".to_vec(), Object::Array(Vec::new()));
                    d.set(NODE_KEY.to_vec(), Object::Bool(true));
                    if let Some((p, _)) = &parent {
                        d.set(b"Parent".to_vec(), Object::Ref(*p));
                    }
                    let r = self.doc.add(Object::Dict(d));
                    match parent.as_ref().and_then(|(_, above)| self.nodes.get_mut(above)) {
                        Some((_, kids)) => kids.push(r),
                        None => self.fields.push(r),
                    }
                    self.nodes.insert(path.clone(), (r, Vec::new()));
                    self.taken.insert(path.clone());
                    r
                }
            };
            parent = Some((r, path));
        }
        parent.map(|(r, _)| r)
    }

    /// An image XObject for a picture: JPEGs as they are (`DCTDecode`), PNGs, GIFs, BMPs and TIFFs
    /// decoded to samples (`FlateDecode`, with a soft mask for transparency). `None`, with a warning,
    /// for anything else.
    fn image_xobject(&mut self, data: &[u8], content_type: &str) -> Option<ObjRef> {
        let hash = content_hash(data);
        if let Some(&r) = self.images.get(&hash)
            && let Object::Stream(s) = &*self.doc.get(r)
            && s.dict.name(b"Subtype") == Some(b"Image")
        {
            return Some(r);
        }
        let r = self.new_image_xobject(data, content_type, &hash)?;
        if self.images.len() < MAX_REMEMBERED_IMAGES {
            self.images.insert(hash, r);
        }
        Some(r)
    }

    /// Write a new image XObject for a picture, tagged with its `hash`: a JPEG as it is, a PNG
    /// or GIF decoded to samples with a soft mask for any transparency. `None`, with a warning,
    /// when the picture can't be read or is another kind.
    fn new_image_xobject(&mut self, data: &[u8], content_type: &str, hash: &[u8; 32]) -> Option<ObjRef> {
        let what = if content_type.is_empty() { "picture" } else { content_type };
        let mut d = Dict::new();
        d.set(IMAGE_SHA_KEY.to_vec(), Object::String(PdfString::literal(hex32(hash))));
        d.set(b"Type".to_vec(), Object::name("XObject"));
        d.set(b"Subtype".to_vec(), Object::name("Image"));
        d.set(b"BitsPerComponent".to_vec(), Object::Int(8));
        match crate::image::kind(data) {
            Some(crate::image::Kind::Jpeg) => {
                let Some((iw, ih, comps, adobe)) = jpeg_info(data) else {
                    self.warnings.push(format!("a {what} image is not a readable JPEG and is left out"));
                    return None;
                };
                d.set(b"Width".to_vec(), Object::Int(i64::from(iw)));
                d.set(b"Height".to_vec(), Object::Int(i64::from(ih)));
                d.set(
                    b"ColorSpace".to_vec(),
                    Object::name(match comps {
                        1 => "DeviceGray",
                        4 => "DeviceCMYK",
                        _ => "DeviceRGB",
                    }),
                );
                if comps == 4 && adobe {
                    // Adobe APP14 JPEGs store inverted CMYK.
                    d.set(b"Decode".to_vec(), Object::Array([1, 0, 1, 0, 1, 0, 1, 0].iter().map(|v| Object::Int(*v)).collect()));
                }
                d.set(b"Filter".to_vec(), Object::name("DCTDecode"));
                Some(self.doc.add(Object::Stream(Stream::from_raw(d, data.to_vec()))))
            }
            Some(crate::image::Kind::Png | crate::image::Kind::Gif | crate::image::Kind::Bmp | crate::image::Kind::Tiff) => {
                let img = match crate::image::decode(data) {
                    Ok(img) => img,
                    Err(why) => {
                        self.warnings.push(why);
                        return None;
                    }
                };
                d.set(b"Width".to_vec(), Object::Int(i64::from(img.width)));
                d.set(b"Height".to_vec(), Object::Int(i64::from(img.height)));
                d.set(b"ColorSpace".to_vec(), Object::name(img.color_space));
                if let Some(alpha) = &img.alpha {
                    let mut m = Dict::new();
                    m.set(b"Type".to_vec(), Object::name("XObject"));
                    m.set(b"Subtype".to_vec(), Object::name("Image"));
                    m.set(b"Width".to_vec(), Object::Int(i64::from(img.width)));
                    m.set(b"Height".to_vec(), Object::Int(i64::from(img.height)));
                    m.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
                    m.set(b"BitsPerComponent".to_vec(), Object::Int(8));
                    let mr = self.doc.add(Object::Stream(Stream::flate(m, alpha)));
                    d.set(b"SMask".to_vec(), Object::Ref(mr));
                }
                Some(self.doc.add(Object::Stream(Stream::flate(d, &img.samples))))
            }
            None => {
                self.warnings.push(format!("a {what} image is not a JPEG, PNG, GIF, BMP or TIFF and is left out"));
                None
            }
        }
    }

    /// Write one laid-out page under `pages_ref`: its fills, lines, text and pictures in a
    /// content stream, the fonts and images it uses as resources, and its widgets and the links
    /// in its text as annotations. Returns the page object.
    fn page(&mut self, page: &Page, pages_ref: ObjRef) -> Result<ObjRef, XfaError> {
        let page_ref = self.doc.add(Object::Dict(Dict::new()));
        let mut content: Vec<u8> = Vec::new();
        let mut fonts: BTreeMap<&'static str, &'static str> = BTreeMap::new();
        let mut xobjects = Dict::new();
        let mut annots = Vec::new();
        let h = page.height;
        // The link the text before ended in, which a next piece of it on the line widens.
        let mut last_link: Option<LinkPiece> = None;
        for (i, item) in page.items.iter().enumerate() {
            // A link's pieces join only across text and the underline drawn just before it: a fill,
            // picture, field or any other line (a rule) between them ends the link there.
            let keeps_link = match item {
                Item::Text(_) => true,
                Item::Line { from, to, .. } => matches!(page.items.get(i + 1), Some(Item::Text(s))
                    if (from.1 - to.1).abs() < 0.01
                        && (from.0 - s.x).abs() < 0.01
                        && (from.1 - (s.baseline + s.face.size * crate::layout::UNDERLINE_DROP)).abs() < 0.01),
                _ => false,
            };
            if !keeps_link {
                last_link = None;
            }
            match item {
                Item::Fill { rect, color } => {
                    content.extend(format!("{} rg {} {} {} {} re f\n", rgb(*color), n(rect.x), n(h - rect.bottom()), n(rect.w), n(rect.h)).bytes());
                }
                Item::Rounded(r) => content.extend(rounded(r, h).bytes()),
                Item::Line { from, to, width, color, dashed } => {
                    content.extend(
                        format!(
                            "q {} RG {} w {}{} {} m {} {} l S Q\n",
                            rgb(*color),
                            n(*width),
                            if *dashed { "[2 2] 0 d " } else { "" },
                            n(from.0),
                            n(h - from.1),
                            n(to.0),
                            n(h - to.1)
                        )
                        .bytes(),
                    );
                }
                Item::Text(s) => {
                    // The form's own text, set in the standard fonts: what they can't draw is
                    // shown as `?` and reported once (the layout goes on), never silently.
                    if let Some(c) = pdfcraft_fonts::first_undrawable(&s.text) {
                        const PREFIX: &str = "some of the form's text has characters the standard fonts can't draw";
                        if !self.warnings.iter().any(|w| w.starts_with(PREFIX)) {
                            self.warnings.push(format!(
                                "{PREFIX} (the first is \"{}\" (U+{:04X})); they are shown as ? until PdfCraft can embed fonts for them",
                                c.escape_debug(),
                                u32::from(c)
                            ));
                        }
                    }
                    let name = s.face.resource_name();
                    fonts.insert(name, s.face.family.base_font(s.face.bold, s.face.italic));
                    // Horizontal scaling is text state, which outlives the text object: a
                    // condensed span gets a graphics state of its own, so the next isn't.
                    let scaled = (s.face.h_scale - 1.0).abs() > 1e-6;
                    let (open, tz, close) =
                        if scaled { ("q BT", format!(" {} Tz", n(s.face.h_scale * 100.0)), " ET Q") } else { ("BT", String::new(), " ET") };
                    content.extend(
                        format!("{open} /{name} {} Tf {} rg{tz} 1 0 0 1 {} {} Tm ", n(s.face.size), rgb(s.face.color), n(s.x), n(h - s.baseline))
                            .bytes(),
                    );
                    content.extend(literal(&win_ansi(&s.text)));
                    content.extend_from_slice(format!(" Tj{close}\n").as_bytes());
                    if let Some(r) = self.text_link(s, h, page_ref, &mut last_link)? {
                        annots.push(Object::Ref(r));
                    }
                }
                Item::Image { rect, data, content_type } => {
                    let Some(r) = self.image_xobject(data, content_type) else { continue };
                    let name = format!("Im{i}");
                    xobjects.set(name.clone().into_bytes(), Object::Ref(r));
                    content.extend(format!("q {} 0 0 {} {} {} cm /{name} Do Q\n", n(rect.w), n(rect.h), n(rect.x), n(h - rect.bottom())).bytes());
                }
                Item::Widget(w) => {
                    if let Some(r) = self.widget(w, page, page_ref)? {
                        annots.push(Object::Ref(r));
                    }
                }
            }
        }
        let mut font_dict = Dict::new();
        for (name, base) in fonts {
            font_dict.set(name.as_bytes().to_vec(), standard_font(base));
        }
        let mut res = Dict::new();
        res.set(b"Font".to_vec(), Object::Dict(font_dict));
        if !xobjects.is_empty() {
            res.set(b"XObject".to_vec(), Object::Dict(xobjects));
        }
        let mut cd = Dict::new();
        cd.set(b"Type".to_vec(), Object::name("XObject"));
        let contents = self.doc.add(Object::Stream(Stream::flate(Dict::new(), &content)));
        let _ = cd;
        let mut pd = Dict::new();
        pd.set(b"Type".to_vec(), Object::name("Page"));
        pd.set(b"Parent".to_vec(), Object::Ref(pages_ref));
        pd.set(b"MediaBox".to_vec(), Object::Array([0.0, 0.0, page.width, page.height].iter().map(|v| Object::Real(*v)).collect()));
        pd.set(b"Resources".to_vec(), Object::Dict(res));
        pd.set(b"Contents".to_vec(), Object::Ref(contents));
        if !annots.is_empty() {
            pd.set(b"Annots".to_vec(), Object::Array(annots));
        }
        self.doc.set(page_ref, Object::Dict(pd));
        Ok(page_ref)
    }

    /// A link annotation over span `s` (on a page `page_height` tall), when its text links
    /// somewhere: over the text it draws, without the spaces around it, and opening its address.
    /// A span that goes on the link `last` ended in, on the same line, widens that annotation
    /// instead (a link whose text changes style partway is one link). An address PdfCraft
    /// wouldn't open from a document leaves the text as plain text, with one warning for the form.
    fn text_link(&mut self, s: &Span, page_height: f64, page_ref: ObjRef, last: &mut Option<LinkPiece>) -> Result<Option<ObjRef>, XfaError> {
        let piece = self.link_piece(s, page_height);
        let Some((target, rect)) = piece else {
            *last = None;
            return Ok(None);
        };
        if let Some(prev) = last.as_mut()
            && prev.uri == target
            && (prev.baseline - s.baseline).abs() < 0.01
            && (-0.01..=s.face.size).contains(&(rect[0] - prev.rect[2]))
        {
            prev.rect = [prev.rect[0].min(rect[0]), prev.rect[1].min(rect[1]), prev.rect[2].max(rect[2]), prev.rect[3].max(rect[3])];
            let r = prev.rect;
            self.doc.update_dict(prev.annot, |d| d.set(b"Rect".to_vec(), Object::Array(r.iter().map(|v| Object::Real(*v)).collect())))?;
            return Ok(None);
        }
        let mut action = Dict::new();
        action.set(b"S".to_vec(), Object::name("URI"));
        action.set(b"URI".to_vec(), Object::String(PdfString::literal(target.as_bytes().to_vec())));
        let mut d = Dict::new();
        d.set(b"Type".to_vec(), Object::name("Annot"));
        d.set(b"Subtype".to_vec(), Object::name("Link"));
        d.set(b"Rect".to_vec(), Object::Array(rect.iter().map(|v| Object::Real(*v)).collect()));
        d.set(b"P".to_vec(), Object::Ref(page_ref));
        d.set(b"F".to_vec(), Object::Int(4));
        d.set(b"Border".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(0)]));
        d.set(b"A".to_vec(), Object::Dict(action));
        let annot = self.doc.add(Object::Dict(d));
        *last = Some(LinkPiece { annot, uri: target, baseline: s.baseline, rect });
        Ok(Some(annot))
    }

    /// Where span `s` links to and the rectangle (PDF user space, on a page `page_height` tall)
    /// over the text it draws, without the spaces around it. `None` for text that links nowhere,
    /// draws nothing, or links to an address PdfCraft wouldn't open from a document, which is
    /// warned about once for the form.
    fn link_piece(&mut self, s: &Span, page_height: f64) -> Option<(String, [f64; 4])> {
        let href = s.link.as_deref()?;
        let shown = s.text.trim();
        if shown.is_empty() {
            return None;
        }
        let Some(target) = link_target(href) else {
            if !self.warnings.iter().any(|w| w.starts_with(LINK_LEFT_OUT)) {
                self.warnings.push(format!("{LINK_LEFT_OUT}: PdfCraft makes links only of web (http, https) and email (mailto) addresses"));
            }
            return None;
        };
        let lead = s.text.len() - s.text.trim_start().len();
        let x = s.x + s.face.width(s.text.get(..lead).unwrap_or_default());
        let w = s.face.width(shown);
        // The text's ascent and descent, as the layout measures a span's extent.
        let rect = pdf_rect(Rect::new(x, s.baseline - s.face.size * 0.8, w, s.face.size), page_height);
        (rect.iter().all(|v| v.is_finite()) && w > 0.0).then(|| (target.to_string(), rect))
    }
}

fn write_button_action(action: Option<&Action>, d: &mut Dict) {
    match action {
        Some(Action::Script(_)) => {
            // An XFA click script is no Acrobat JavaScript: no PDF action, which other
            // viewers would run; PdfCraft finds the script by the field's SOM path.
            d.set(CLICK_KEY.to_vec(), Object::Bool(true));
        }
        Some(action) => {
            let mut ad = Dict::new();
            match action {
                Action::Reset => {
                    ad.set(b"S".to_vec(), Object::name("ResetForm"));
                }
                Action::Print => {
                    ad.set(b"S".to_vec(), Object::name("Named"));
                    ad.set(b"N".to_vec(), Object::name("Print"));
                }
                Action::SaveAs => {
                    ad.set(b"S".to_vec(), Object::name("Named"));
                    ad.set(b"N".to_vec(), Object::name("SaveAs"));
                }
                Action::Url(url) => {
                    ad.set(b"S".to_vec(), Object::name("URI"));
                    ad.set(b"URI".to_vec(), Object::String(PdfString::literal(url.as_bytes().to_vec())));
                }
                Action::Script(_) => {}
            }
            d.set(b"A".to_vec(), Object::Dict(ad));
        }
        None => {}
    }
}

/// The link annotation a page's text last ended in: the next piece of the same link on the same
/// line widens it.
struct LinkPiece {
    annot: ObjRef,
    uri: String,
    baseline: f64,
    rect: [f64; 4],
}

/// How the layout warning about a link left out starts.
const LINK_LEFT_OUT: &str = "a link in the form's text was left as plain text";

/// The longest link address kept, in characters: as long as PdfCraft opens from a document.
const MAX_LINK: usize = 2048;

/// `href` as the address of a link PdfCraft makes from a form's text: a web (http, https) or
/// email (mailto) address, trimmed, of at most [`MAX_LINK`] characters and without control
/// characters. `None` for anything else (scripts, files, data, relative paths). Opening it still
/// goes through the app's own checks and prompt.
fn link_target(href: &str) -> Option<&str> {
    let href = href.trim();
    if href.is_empty() || href.chars().count() > MAX_LINK || href.chars().any(char::is_control) {
        return None;
    }
    let (scheme, rest) = href.split_once(':')?;
    let web = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
    let ok = if web { rest.strip_prefix("//").is_some_and(has_host) } else { scheme.eq_ignore_ascii_case("mailto") && !rest.is_empty() };
    ok.then_some(href)
}

/// Whether `after` (a web address past its `//`) names a host: what comes before the path (which a
/// backslash starts too, as web addresses read it), query or fragment, past any user name and
/// before any port, isn't empty.
fn has_host(after: &str) -> bool {
    let authority = after.split(['/', '\\', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit('@').next().unwrap_or_default();
    // An IPv6 host is bracketed and holds colons of its own.
    let host = if host.starts_with('[') { host } else { host.split(':').next().unwrap_or_default() };
    !host.is_empty()
}

/// Replace the document's pages with the laid-out form and add its fields to the AcroForm. The
/// XFA packets and everything else in the catalog stay.
/// The image XObjects earlier passes wrote for pictures, by the SHA-256 each records: those on
/// the pages under `pages` (the flat page list [`write_form`] writes).
fn earlier_images(doc: &Document, pages: ObjRef) -> HashMap<[u8; 32], ObjRef> {
    let mut out = HashMap::new();
    let kids = doc.get(pages).as_dict().and_then(|d| d.get(b"Kids").map(|k| doc.resolve(k))).and_then(|k| k.as_array().cloned()).unwrap_or_default();
    for page in kids.iter().filter_map(Object::as_ref).take(MAX_PAGES_SEARCHED) {
        let page = doc.get(page);
        let Some(resources) = page.as_dict().and_then(|p| p.get(b"Resources")).map(|r| doc.resolve(r)) else { continue };
        let Some(xobjects) = resources.as_dict().and_then(|r| r.get(b"XObject")).map(|x| doc.resolve(x)) else { continue };
        for r in xobjects.as_dict().into_iter().flat_map(|x| x.iter()).filter_map(|(_, v)| v.as_ref()) {
            if out.len() >= MAX_REMEMBERED_IMAGES {
                return out;
            }
            if let Object::Stream(s) = &*doc.get(r)
                && s.dict.name(b"Subtype") == Some(b"Image")
                && let Some(hash) = s.dict.get(IMAGE_SHA_KEY).and_then(Object::as_string).and_then(|h| parse_hex32(&h.bytes))
            {
                out.insert(hash, r);
            }
        }
    }
    out
}

/// Write laid-out `form` into `doc`: its pages replace the document's, its widgets join the
/// AcroForm's fields (a box the file holds a signature for keeps it), and the AcroForm is marked
/// laid out. What was written, with any warnings, comes back.
pub fn write_form(doc: &mut Document, form: &Form) -> Result<Written, XfaError> {
    let catalog_ref = doc.root().ok_or_else(|| XfaError::Malformed("the document has no catalog".into()))?;
    let mut catalog = doc.get(catalog_ref).as_dict().cloned().ok_or_else(|| XfaError::Malformed("the catalog is not a dictionary".into()))?;
    let mut catalog_changed = false;
    let pages_ref = match catalog.get(b"Pages").and_then(Object::as_ref) {
        Some(r) if doc.get(r).as_dict().is_some() => r,
        _ => {
            let mut p = Dict::new();
            p.set(b"Type".to_vec(), Object::name("Pages"));
            let r = doc.add(Object::Dict(p));
            catalog.set(b"Pages".to_vec(), Object::Ref(r));
            catalog_changed = true;
            r
        }
    };
    // Pictures embedded by earlier passes, from the pages this pass replaces.
    let images = earlier_images(doc, pages_ref);
    let (signed, taken) = existing_fields(doc, &catalog);
    let mut em = Emitter {
        doc,
        images,
        fields: Vec::new(),
        radio_groups: HashMap::new(),
        radio_selected: HashMap::new(),
        field_count: 0,
        warnings: Vec::new(),
        signed,
        taken,
        nodes: HashMap::new(),
    };
    // Every name a field of this pass takes at the top, wherever and in whatever order it is
    // written: a signature field named by its SOM path must not repeat one.
    for w in form.pages.iter().flat_map(|p| &p.items).filter_map(|i| if let Item::Widget(w) = i { Some(w) } else { None }) {
        em.taken.insert(match &w.kind {
            WidgetKind::Radio { group, .. } => group.clone(),
            _ => w.name.clone(),
        });
    }
    let mut kids = Vec::new();
    for page in &form.pages {
        kids.push(Object::Ref(em.page(page, pages_ref)?));
    }
    let groups: Vec<(String, (ObjRef, Vec<ObjRef>))> = em.radio_groups.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for (name, (parent, widgets)) in groups {
        let selected = em.radio_selected.get(&name).cloned();
        em.doc.update_dict(parent, |d| {
            d.set(b"Kids".to_vec(), Object::Array(widgets.iter().map(|r| Object::Ref(*r)).collect()));
            if let Some(s) = &selected {
                d.set(b"V".to_vec(), Object::name(s));
            }
        })?;
    }
    let nodes: Vec<(ObjRef, Vec<ObjRef>)> = em.nodes.values().cloned().collect();
    for (node, kids) in nodes {
        em.doc.update_dict(node, |d| d.set(b"Kids".to_vec(), Object::Array(kids.iter().map(|r| Object::Ref(*r)).collect())))?;
    }
    let Emitter { doc, fields, field_count, warnings, .. } = em;
    let count = kids.len();
    doc.update_dict(pages_ref, |d| {
        d.set(b"Type".to_vec(), Object::name("Pages"));
        d.set(b"Kids".to_vec(), Object::Array(kids));
        d.set(b"Count".to_vec(), Object::Int(count as i64));
        d.remove(b"Parent");
    })?;
    // AcroForm: keep what is there (the XFA packets above all), add our fields and fonts.
    let acro_ref = catalog.get(b"AcroForm").and_then(Object::as_ref);
    let mut acro = match catalog.get(b"AcroForm") {
        Some(o) => doc.resolve(o).as_dict().cloned().unwrap_or_default(),
        None => Dict::new(),
    };
    let mut all: Vec<Object> = acro.get(b"Fields").map(|f| doc.resolve(f)).and_then(|f| f.as_array().cloned()).unwrap_or_default();
    all.extend(fields.iter().map(|r| Object::Ref(*r)));
    acro.set(b"Fields".to_vec(), Object::Array(all));
    let mut dr = acro.get(b"DR").map(|d| doc.resolve(d)).and_then(|d| d.as_dict().cloned()).unwrap_or_default();
    let mut dr_fonts = dr.get(b"Font").map(|f| doc.resolve(f)).and_then(|f| f.as_dict().cloned()).unwrap_or_default();
    for (name, base) in all_faces() {
        if !dr_fonts.contains(name.as_bytes()) {
            dr_fonts.set(name.as_bytes().to_vec(), standard_font(base));
        }
    }
    if !dr_fonts.contains(b"ZaDb") {
        let mut z = Dict::new();
        z.set(b"Type".to_vec(), Object::name("Font"));
        z.set(b"Subtype".to_vec(), Object::name("Type1"));
        z.set(b"BaseFont".to_vec(), Object::name("ZapfDingbats"));
        dr_fonts.set(b"ZaDb".to_vec(), Object::Dict(z));
    }
    // Each font its own object: Acrobat shows nothing in a text field it redraws after typing
    // when the field's /DR font is a dictionary written inline (#3441).
    let inline: Vec<(Vec<u8>, Dict)> = dr_fonts.iter().filter_map(|(k, v)| Some((k.clone(), v.as_dict()?.clone()))).collect();
    for (name, font) in inline {
        let r = doc.add(Object::Dict(font));
        dr_fonts.set(name, Object::Ref(r));
    }
    dr.set(b"Font".to_vec(), Object::Dict(dr_fonts));
    acro.set(b"DR".to_vec(), Object::Dict(dr));
    if !acro.contains(b"DA") {
        acro.set(b"DA".to_vec(), Object::String(PdfString::literal(b"/Helv 0 Tf 0 g".to_vec())));
    }
    let mut mark = Dict::new();
    mark.set(b"Pages".to_vec(), Object::Int(count as i64));
    mark.set(b"Fields".to_vec(), Object::Int(field_count as i64));
    mark.set(b"Warnings".to_vec(), Object::Array(warnings.iter().map(|w| Object::String(PdfString::text(w))).collect()));
    acro.set(LAYOUT_KEY.to_vec(), Object::Dict(mark));
    match acro_ref {
        Some(r) => doc.set(r, Object::Dict(acro)),
        None => {
            catalog.set(b"AcroForm".to_vec(), Object::Dict(acro));
            catalog_changed = true;
        }
    }
    // The pages and fields are new: usage rights signed over the form as it was are void now.
    let usage_rights = drop_usage_rights(doc, &mut catalog);
    if catalog_changed || usage_rights {
        doc.set(catalog_ref, Object::Dict(catalog));
    }
    Ok(Written { pages: count, fields: field_count, warnings, usage_rights })
}

/// The rectangle of a widget in PDF user space (for tests).
pub fn pdf_rect(r: Rect, page_height: f64) -> [f64; 4] {
    [r.x, page_height - r.bottom(), r.right(), page_height - r.y]
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};

    use super::{MAX_FIELDS_SEARCHED, Signed, existing_fields, parse_hex32};

    /// The fields `existing_fields` finds under an AcroForm listing `fields`.
    fn names_found(doc: &Document, fields: Vec<Object>) -> Vec<String> {
        let (signed, names) = existing(doc, fields);
        assert!(signed.is_empty());
        let mut names: Vec<String> = names.into_iter().collect();
        names.sort();
        names
    }

    fn field(name: &str, kids: Object) -> Object {
        let mut d = Dict::new();
        d.set(b"T".to_vec(), Object::String(PdfString::literal(name)));
        d.set(b"Kids".to_vec(), kids);
        Object::Dict(d)
    }

    #[test]
    fn fields_listed_many_times_queue_a_bounded_number_of_kids() {
        // A field lists itself 5,000 times: queuing every kid on each of 20,000 visits would
        // hold a hundred million entries. It is read once.
        let mut doc = Document::new_empty();
        let r = doc.add(Object::Null);
        doc.set(r, field("a", Object::Array(vec![Object::Ref(r); 5_000])));
        assert_eq!(names_found(&doc, vec![Object::Ref(r)]), ["a"]);

        // 5,000 fields share one /Kids array listing each of them 200 times. Read once each,
        // the fields down one branch still queue the whole array at every level: 32 million
        // entries, unless the queue itself is bounded.
        let mut doc = Document::new_empty();
        let kids = doc.add(Object::Null);
        let fields: Vec<Object> = (0..5_000).map(|i| Object::Ref(doc.add(field(&format!("f{i}"), Object::Ref(kids))))).collect();
        doc.set(kids, Object::Array(fields.iter().cycle().take(1_000_000).cloned().collect()));
        let names = names_found(&doc, fields[..1].to_vec());
        assert!(names.contains(&"f0".to_string()) && names.len() <= MAX_FIELDS_SEARCHED, "{} names", names.len());
    }

    #[test]
    fn a_field_listed_many_times_leaves_room_for_the_fields_under_it() {
        // The AcroForm lists one field 19,999 times. The copies take no room, so its kids are
        // still queued, and the signed one among them is found.
        let mut doc = Document::new_empty();
        let parent = signed_parent(&mut doc);
        let (signed, names) = existing(&doc, vec![Object::Ref(parent); MAX_FIELDS_SEARCHED - 1]);
        assert!(signed.contains_key("c.sig"), "{:?}", signed.keys().collect::<Vec<_>>());
        assert!(names.contains("c.plain"));
    }

    #[test]
    fn a_field_listed_under_many_generations_is_queued_once() {
        // `Document::get` finds a field by object number alone, so `5 0 R` and `5 1 R` are one
        // field. Listed 19,999 times, each under another generation, it is still queued once,
        // and the signed field under it is found.
        let mut doc = Document::new_empty();
        let parent = signed_parent(&mut doc);
        let fields = (0..MAX_FIELDS_SEARCHED - 1).map(|g| Object::Ref(ObjRef::new(parent.num, u16::try_from(g).unwrap()))).collect();
        let (signed, names) = existing(&doc, fields);
        assert!(signed.contains_key("c.sig"), "{:?}", signed.keys().collect::<Vec<_>>());
        assert!(names.contains("c.plain"));
    }

    #[test]
    fn signed_fields_sharing_one_long_kids_array_search_it_within_the_read_limit() {
        // 1,000 signed fields share one `/Kids` array of 100,000 nulls and a widget with an
        // appearance at its end. Each searched the whole array for that appearance: with a
        // million nulls, a billion lookups (39 s). The search counts against `MAX_KIDS_READ`, so
        // the fields read once it is spent keep their value but get no appearance.
        let mut doc = Document::new_empty();
        let mut widget = Dict::new();
        widget.set(b"AP".to_vec(), Object::Dict(Dict::new()));
        let mut shared = vec![Object::Null; 100_000];
        shared.push(Object::Ref(doc.add(Object::Dict(widget))));
        let kids = doc.add(Object::Array(shared));
        let fields = (0..1_000)
            .map(|i| {
                let mut d = Dict::new();
                d.set(b"T".to_vec(), Object::String(PdfString::literal(format!("s{i}").as_str())));
                d.set(b"FT".to_vec(), Object::Name(b"Sig".to_vec()));
                d.set(b"V".to_vec(), Object::Dict(Dict::new()));
                d.set(b"Kids".to_vec(), Object::Ref(kids));
                Object::Ref(doc.add(Object::Dict(d)))
            })
            .collect();
        let (signed, _) = existing(&doc, fields);
        assert_eq!(signed.len(), 1_000);
        let with_appearance = signed.values().filter(|s| s.appearance.is_some()).count();
        assert!((1..=10).contains(&with_appearance), "{with_appearance} appearances found");
    }

    /// Field `c` with two kids: `plain`, and `sig`, a signed signature field.
    fn signed_parent(doc: &mut Document) -> ObjRef {
        let plain = doc.add(field("plain", Object::Array(Vec::new())));
        let mut sig = Dict::new();
        sig.set(b"T".to_vec(), Object::String(PdfString::literal("sig")));
        sig.set(b"FT".to_vec(), Object::Name(b"Sig".to_vec()));
        sig.set(b"V".to_vec(), Object::Dict(Dict::new()));
        let sig = doc.add(Object::Dict(sig));
        doc.add(field("c", Object::Array(vec![Object::Ref(plain), Object::Ref(sig)])))
    }

    /// What `existing_fields` finds under an AcroForm listing `fields`.
    fn existing(doc: &Document, fields: Vec<Object>) -> (HashMap<String, Signed>, HashSet<String>) {
        let mut acro = Dict::new();
        acro.set(b"Fields".to_vec(), Object::Array(fields));
        let mut catalog = Dict::new();
        catalog.set(b"AcroForm".to_vec(), Object::Dict(acro));
        existing_fields(doc, &catalog)
    }

    #[test]
    fn parse_hex32_rejects_non_hex_digits() {
        for prefix in ["+f", "-f", " f"] {
            let mut s = prefix.as_bytes().to_vec();
            s.resize(64, b'0');
            assert!(parse_hex32(&s).is_none(), "{prefix:?}");
        }
        let mut ok = b"Ff".to_vec();
        ok.resize(64, b'0');
        assert_eq!(parse_hex32(&ok).map(|h| h[0]), Some(0xff));
    }
}
