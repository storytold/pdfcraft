use super::read::{acroform, fields};
use super::values::merged_appearance;
use super::{Dict, Document, FieldKind, ObjRef, Object, Widget, appearance, name_bytes};

/// Whether the form asks viewers to draw its fields' appearances themselves (`/NeedAppearances`).
pub fn need_appearances(doc: &Document) -> bool {
    acroform(doc).is_some_and(|af| matches!(af.get(b"NeedAppearances").map(|o| doc.resolve(o)).as_deref(), Some(Object::Bool(true))))
}

/// A copy of `doc` in which form fields are drawn the way the file leaves to the viewer, for
/// displaying the document (`None` when no widget needs it). Text and choice fields are drawn from
/// their values: every one when the form sets `/NeedAppearances` (ISO 32000-2 Table 224: the
/// writer left the appearances to the viewer), and otherwise each widget without a normal
/// appearance (`/AP /N`), as typing the value would draw it (#3470). Check boxes and radio buttons
/// with no normal appearance for their state (`/AS`) are drawn from their `/MK` background, border
/// and caption, and with `/NeedAppearances` every one that has an `/MK` is, over its own
/// appearance; push buttons with none are drawn from their `/MK` and `/BS` (Table 192). `doc` is
/// not changed: saving writes the file as it was.
pub fn with_field_appearances(doc: &Document) -> Option<Document> {
    with_field_appearances_cached(doc, &mut FieldAppearanceCache::default())
}

/// [`with_field_appearances`], reusing the appearances `cache` drew for widgets whose inputs are
/// the same objects as then (see [`FieldAppearanceCache`]).
pub fn with_field_appearances_cached(doc: &Document, cache: &mut FieldAppearanceCache) -> Option<Document> {
    let all = need_appearances(doc);
    let mut copy: Option<Document> = None;
    for f in fields(doc) {
        for w in &f.widgets {
            let widget = doc.get(w.obj);
            let normal = widget
                .as_dict()
                .and_then(|d| d.get(b"AP"))
                .and_then(|ap| doc.resolve(ap).as_dict().and_then(|ap| ap.get(b"N").map(|n| doc.resolve(n))));
            // A check box or radio button shows the appearance of its state (12.7.5.2.3): `/AS`,
            // else the value when it names this widget's on state (any check box value when the
            // widget names none), else Off. An Off state may go without one (it shows nothing);
            // an on state needs one.
            let state = match f.kind {
                FieldKind::CheckBox | FieldKind::Radio => {
                    let mine = |v: &&String| w.on_state.as_ref().map_or(f.kind == FieldKind::CheckBox, |on| on == *v);
                    let value = f.value.first().filter(|v| !v.is_empty() && v.as_str() != "Off").filter(mine);
                    Some(w.state.clone().or_else(|| value.cloned()).unwrap_or_else(|| "Off".into()))
                }
                _ => None,
            };
            // With /NeedAppearances a check box or radio button that describes its look in /MK is
            // drawn from it (Table 192), its own appearance or not; one without /MK keeps its own.
            let described = || widget.as_dict().and_then(|d| d.get(b"MK")).is_some_and(|mk| doc.resolve(mk).as_dict().is_some());
            let missing = match f.kind {
                FieldKind::Text | FieldKind::Combo | FieldKind::List => normal.is_none() || all,
                FieldKind::PushButton => normal.is_none(),
                FieldKind::CheckBox | FieldKind::Radio => match (normal.as_deref(), state.as_deref()) {
                    _ if all && described() => true,
                    (None, _) => true,
                    (Some(Object::Dict(states)), Some(s)) => s != "Off" && !states.contains(&name_bytes(s)),
                    _ => false,
                },
                FieldKind::Signature => false,
            };
            if !missing {
                continue;
            }
            let inputs = appearance_inputs(doc, w);
            let stream = match cache.get(w.obj, &inputs) {
                Some(stream) => stream,
                None => {
                    let stream = match f.kind {
                        FieldKind::CheckBox | FieldKind::Radio => appearance::button_state_shown(doc, &f, w, state.as_deref() != Some("Off")),
                        FieldKind::PushButton => appearance::push_button_shown(doc, &f, w),
                        _ => appearance::field_appearance_drawn(doc, &f, w, &f.value, true).0,
                    };
                    cache.insert(w.obj, inputs, &stream);
                    stream
                }
            };
            let target = copy.get_or_insert_with(|| doc.clone());
            let ap = target.add(Object::Stream(stream));
            let mut fresh = Dict::new();
            match &state {
                // The drawn look under its state's name, and that state as the widget's own.
                Some(s) => {
                    let mut states = Dict::new();
                    states.set(name_bytes(s), Object::Ref(ap));
                    fresh.set(b"N".to_vec(), Object::Dict(states));
                }
                None => fresh.set(b"N".to_vec(), Object::Ref(ap)),
            }
            let apd = merged_appearance(target, w.obj, &fresh);
            // Fails only when the widget isn't a dictionary; that widget then keeps its own look.
            target
                .update_dict(w.obj, |d| {
                    d.set(b"AP".to_vec(), Object::Dict(apd));
                    if let Some(s) = &state {
                        d.set(b"AS".to_vec(), Object::Name(name_bytes(s)));
                    }
                })
                .ok();
        }
    }
    copy
}

/// The objects a widget's drawn appearance is built from, as the document holds them: the widget
/// and the fields above it (value, `/DA`, `/Q`, flags, options, `/MK`, formatting actions), the
/// form dictionary (`/DA`) and the fonts of its `/DR`, each with the objects its entries refer to
/// (an indirect `/MK` or `/V`). Not the page: a field's rotation is its own `/MK /R`.
fn appearance_inputs(doc: &Document, w: &Widget) -> Vec<std::sync::Arc<Object>> {
    // The objects a dictionary's entries refer to. The tree links and the drawn appearance itself
    // are not inputs, and `/Kids` and `/Fields` can be long.
    let referred = |d: &Dict, inputs: &mut Vec<std::sync::Arc<Object>>| {
        for (k, v) in d.iter() {
            if !matches!(&k[..], b"Kids" | b"Fields" | b"P" | b"Parent" | b"AP")
                && let Some(r) = v.as_ref()
            {
                inputs.push(doc.get(r));
            }
        }
    };
    let mut inputs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut node = Some(w.obj);
    // The /Parent chain; the seen-set and the cap end a cycle or an absurd depth.
    while let Some(r) = node.filter(|r| seen.insert(*r) && seen.len() <= 64) {
        let obj = doc.get(r);
        if let Some(d) = obj.as_dict() {
            node = d.reference(b"Parent");
            referred(d, &mut inputs);
        } else {
            node = None;
        }
        inputs.push(obj);
    }
    let Some(root) = doc.root() else { return inputs };
    let catalog = doc.get(root);
    let form = match catalog.as_dict().and_then(|c| c.reference(b"AcroForm")) {
        Some(r) => doc.get(r),
        None => catalog,
    };
    if let Some(f) = form.as_dict() {
        referred(f, &mut inputs);
        // The fonts of /DR. An indirect /DR or /Font is listed through the entries that refer to
        // it; a direct one is part of the object that holds it. `held` only finds them: a copy
        // of a direct object is a new one each time, never an input.
        let held = |o: &Object| o.as_ref().map_or_else(|| std::sync::Arc::new(o.clone()), |r| doc.get(r));
        if let Some(dr) = f.get(b"DR").map(held)
            && let Some(dr) = dr.as_dict()
        {
            referred(dr, &mut inputs);
            if let Some(font) = dr.get(b"Font").map(held)
                && let Some(fonts) = font.as_dict()
            {
                referred(fonts, &mut inputs);
            }
        }
    }
    inputs.push(form);
    inputs
}

/// Field appearances drawn for the display copy ([`with_field_appearances_cached`]), so a refresh
/// redraws only the widgets whose inputs changed: a form that leaves every appearance to the
/// viewer otherwise redrew all of its fields on each keystroke in any one of them (#3470). An
/// entry holds the input objects it was drawn from and is used only while each is still the same
/// object (`Arc::ptr_eq`); the document replaces an object it edits, so a changed input is a
/// miss, never a stale hit.
#[derive(Clone, Debug, Default)]
pub struct FieldAppearanceCache {
    entries: std::collections::HashMap<ObjRef, (Vec<std::sync::Arc<Object>>, pdfcraft_cos::Stream)>,
    bytes: usize,
    hits: u64,
    builds: u64,
}

impl FieldAppearanceCache {
    /// Cached appearance data restarts past this instead of growing without bound.
    const MAX_BYTES: usize = 64 << 20;

    /// Forget every entry.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn get(&mut self, w: ObjRef, inputs: &[std::sync::Arc<Object>]) -> Option<pdfcraft_cos::Stream> {
        let (from, stream) = self.entries.get(&w)?;
        if from.len() != inputs.len() || !from.iter().zip(inputs).all(|(a, b)| std::sync::Arc::ptr_eq(a, b)) {
            return None;
        }
        self.hits += 1;
        Some(stream.clone())
    }

    fn insert(&mut self, w: ObjRef, inputs: Vec<std::sync::Arc<Object>>, stream: &pdfcraft_cos::Stream) {
        self.builds += 1;
        if self.bytes.saturating_add(stream.raw.len()) > Self::MAX_BYTES {
            self.clear();
        }
        self.bytes = self.bytes.saturating_add(stream.raw.len());
        if let Some((_, old)) = self.entries.insert(w, (inputs, stream.clone())) {
            self.bytes = self.bytes.saturating_sub(old.raw.len());
        }
    }

    /// Cache hits so far (reused appearances); for tests.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Appearances drawn so far; for tests.
    pub fn builds(&self) -> u64 {
        self.builds
    }
}

/// Whether the document's XMP metadata declares PDF/A conformance (`pdfaid:part`).
pub fn declares_pdfa(doc: &Document) -> bool {
    let Some(root) = doc.root() else { return false };
    let Some(m) = doc.get(root).as_dict().and_then(|c| c.get(b"Metadata").cloned()) else { return false };
    let m = doc.resolve(&m);
    match &*m {
        Object::Stream(s) => s.decoded().is_ok_and(|b| b.windows(11).any(|w| w == b"pdfaid:part")),
        _ => false,
    }
}

/// Every field with a widget whose appearance PdfCraft drew with `?` for a character its font
/// can't hold (`pdfcraft_fonts::UNDRAWN_MARK`): the field's name and that character, once per
/// field.
pub fn undrawn_fields(doc: &Document) -> Vec<(String, char)> {
    fields(doc)
        .into_iter()
        .filter_map(|f| {
            let c = f.widgets.iter().find_map(|w| doc.get(w.obj).as_dict().and_then(pdfcraft_fonts::undrawn_mark))?;
            Some((f.name, c))
        })
        .collect()
}
