//! PDF/A-2, -3 and -4 6.2.4.2: overprinting in overprint mode 1 with an ICCBased CMYK colour space.
//!
//! ISO 19005-2/-3/-4 6.2.4.2 forbid painting in an ICCBased CMYK colour space while overprinting is
//! on for that painting operation and the overprint mode is 1: a stroke with the stroking overprint
//! parameter true, or any other painting with the non-stroking one true. Overprint mode 1 treats a
//! zero CMYK component as "leave alone", which is only defined for device CMYK (ISO 32000-2 8.6.7).
//!
//! The graphics state is followed as ISO 32000-2 8.4 has it:
//!
//! - `gs` sets the overprint parameters from `/OP` and `/op` and the mode from `/OPM` (Table 57).
//!   An `/OP` sets both parameters unless the same dictionary also has `/op`;
//! - `CS` and `cs` set the stroking and non-stroking colour space, and `G`, `RG`, `K`, `g`, `rg`
//!   and `k` set a device space. Device CMYK is drawn in the resources' `/DefaultCMYK` when they
//!   have one (8.6.5.6), so it counts when that space is ICCBased CMYK;
//! - `q` and `Q` save and restore all of it, and `Tr` sets the text rendering mode;
//! - a form XObject drawn with `Do` is read in the state it is drawn in (8.10.1), with its own
//!   `/Resources`, or its caller's when it has none.
//!
//! A form is read again only when drawn in a new state, with other resources when it takes its
//! caller's, or less deeply nested than before (so the forms it draws are followed further).
//!
//! Painting is `S` and `s` (stroke), `f`, `F` and `f*` (fill), `B`, `B*`, `b` and `b*` (both), and
//! text shown by `Tj`, `TJ`, `'` and `"` in a mode that fills, strokes or both (Table 104).
//!
//! The content read is each page's `/Contents` with the page's resources, and the forms it draws;
//! then each content stream that wasn't reached that way (an annotation's appearance, a tiling
//! pattern, a form no page draws), on its own from the initial graphics state. Only documents with
//! a graphics state dictionary whose `/OPM` is 1 are read at all, since without one the mode is
//! never 1.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object};

/// Decoded bytes one verification reads for this check, all content streams together.
const MAX_DECODED: usize = 1 << 30;
/// How deeply forms drawn by forms are followed.
const MAX_FORM_DEPTH: u8 = 16;
/// Saved graphics states kept; a `Q` past them restores nothing.
const MAX_SAVES: usize = 256;
/// How deep a direct object is searched for an `/OPM` entry.
const MAX_DIRECT_DEPTH: u8 = 16;
/// Largest ICC profile decoded to read its header when the stream has no `/N`.
const MAX_ICC_PROFILE: usize = 16 << 20;

/// The part of the graphics state the rule depends on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
struct State {
    /// Overprint for stroking (`OP`) and for other painting (`op`).
    stroke_overprint: bool,
    fill_overprint: bool,
    /// The overprint mode is 1.
    mode_1: bool,
    /// The stroking and non-stroking colour spaces are ICCBased CMYK.
    stroke_cmyk: bool,
    fill_cmyk: bool,
    /// The text rendering mode, 0 to 7.
    text_mode: u8,
}

impl State {
    fn stroke_breaks(self) -> bool {
        self.mode_1 && self.stroke_overprint && self.stroke_cmyk
    }

    fn fill_breaks(self) -> bool {
        self.mode_1 && self.fill_overprint && self.fill_cmyk
    }
}

/// Whether any content paints in an ICCBased CMYK colour space with overprinting on for that
/// painting and overprint mode 1.
pub(crate) fn icc_cmyk_overprinted(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)]) -> bool {
    if !objs.iter().any(|(_, o)| has_mode_1(o, 0)) {
        return false;
    }
    let mut walk = Walk { doc, budget: DecodeBudget::new(MAX_DECODED, usize::MAX), read: HashMap::new(), reached: HashSet::new() };
    for page in pdfcraft_model::pages(doc) {
        let resources = page.dict.get(b"Resources").map(|r| doc.resolve(r)).and_then(|r| r.as_dict().cloned());
        let contents = match page.dict.get(b"Contents").map(|c| doc.resolve(c)).as_deref() {
            Some(Object::Array(a)) => a.clone(),
            Some(_) => page.dict.get(b"Contents").cloned().into_iter().collect(),
            None => Vec::new(),
        };
        // A page's content streams are one stream cut into pieces (ISO 32000-2 7.8.2).
        let mut data = Vec::new();
        for c in &contents {
            if let Some(r) = c.as_ref() {
                walk.reached.insert(r);
            }
            if let Object::Stream(s) = &*doc.resolve(c)
                && let Ok(piece) = walk.budget.decode(s)
            {
                data.extend_from_slice(&piece);
                data.push(b'\n');
            }
        }
        if walk.content(&data, resources.as_ref(), page.obj, State::default(), 0) {
            return true;
        }
    }
    for (r, o) in objs {
        let Object::Stream(s) = &**o else { continue };
        // Type 3 glyph procedures take their font's resources, so on their own they draw nothing
        // this rule reads; the rest of the content streams say so in their dictionaries.
        if walk.reached.contains(r) || !crate::is_content(&s.dict) {
            continue;
        }
        let resources = s.dict.get(b"Resources").map(|x| doc.resolve(x)).and_then(|x| x.as_dict().cloned());
        let Ok(data) = walk.budget.decode(s) else { continue };
        walk.reached.insert(*r);
        if walk.content(&data, resources.as_ref(), *r, State::default(), 0) {
            return true;
        }
    }
    false
}

/// Whether `o` holds, directly, a dictionary whose `/OPM` is 1, or may be: a reference is followed
/// only when the state is used.
/// A stream's dictionary is searched as any other, so a form's direct `/Resources` count.
fn has_mode_1(o: &Object, depth: u8) -> bool {
    if depth > MAX_DIRECT_DEPTH {
        return false;
    }
    let d = match o {
        Object::Dict(d) => d,
        Object::Stream(s) => &s.dict,
        Object::Array(a) => return a.iter().any(|v| has_mode_1(v, depth.saturating_add(1))),
        _ => return false,
    };
    d.get(b"OPM").is_some_and(|m| is_one(m) || matches!(m, Object::Ref(_))) || d.iter().any(|(_, v)| has_mode_1(v, depth.saturating_add(1)))
}

fn is_one(o: &Object) -> bool {
    match o {
        Object::Int(1) => true,
        Object::Real(r) => *r == 1.0,
        _ => false,
    }
}

/// A form drawn in a state: the form, the state, and, for a form without `/Resources` of its own,
/// the object whose resources it takes.
type Drawn = (ObjRef, State, Option<ObjRef>);

struct Walk<'a> {
    doc: &'a Document,
    budget: DecodeBudget,
    /// Forms already read, by what decides how they read, with the least depth each was read at:
    /// the same state and resources read the same way, and a form read less deeply follows its
    /// own forms further.
    read: HashMap<Drawn, u8>,
    /// Content streams read at least once.
    reached: HashSet<ObjRef>,
}

impl Walk<'_> {
    /// Reads one content stream from `state`; true at the first painting that breaks the rule.
    /// `owner` is the object `resources` come from: a page, or a form or other stream with its own.
    fn content(&mut self, data: &[u8], resources: Option<&Dict>, owner: ObjRef, mut state: State, depth: u8) -> bool {
        let mut saved = Vec::new();
        // `q`s past `MAX_SAVES`, whose `Q`s restore nothing.
        let mut unsaved = 0usize;
        for op in pdfcraft_content::ops(data) {
            match op.op.as_slice() {
                b"q" => {
                    if saved.len() < MAX_SAVES {
                        saved.push(state);
                    } else {
                        unsaved = unsaved.saturating_add(1);
                    }
                }
                b"Q" => {
                    if unsaved > 0 {
                        unsaved -= 1;
                    } else if let Some(s) = saved.pop() {
                        state = s;
                    }
                }
                b"gs" => {
                    if let Some(gs) = op.name(0).and_then(|n| self.resource(resources, b"ExtGState", n))
                        && let Some(gs) = gs.as_dict()
                    {
                        let op_fill = gs.get(b"op").and_then(|v| self.flag(v));
                        if let Some(stroke) = gs.get(b"OP").and_then(|v| self.flag(v)) {
                            state.stroke_overprint = stroke;
                            if op_fill.is_none() {
                                state.fill_overprint = stroke;
                            }
                        }
                        if let Some(fill) = op_fill {
                            state.fill_overprint = fill;
                        }
                        if let Some(mode) = gs.get(b"OPM").map(|v| self.doc.resolve(v)) {
                            state.mode_1 = is_one(&mode);
                        }
                    }
                }
                b"CS" => state.stroke_cmyk = op.name(0).is_some_and(|n| self.icc_cmyk(resources, n)),
                b"cs" => state.fill_cmyk = op.name(0).is_some_and(|n| self.icc_cmyk(resources, n)),
                b"G" | b"RG" => state.stroke_cmyk = false,
                b"g" | b"rg" => state.fill_cmyk = false,
                b"K" => state.stroke_cmyk = self.icc_cmyk(resources, b"DeviceCMYK"),
                b"k" => state.fill_cmyk = self.icc_cmyk(resources, b"DeviceCMYK"),
                b"Tr" => {
                    if let Some(m) = op.num(0).filter(|m| (0.0..=7.0).contains(m)) {
                        // In 0..=7, checked above, so the cast keeps the value.
                        state.text_mode = m as u8;
                    }
                }
                b"S" | b"s" if state.stroke_breaks() => return true,
                b"f" | b"F" | b"f*" if state.fill_breaks() => return true,
                b"B" | b"B*" | b"b" | b"b*" if state.stroke_breaks() || state.fill_breaks() => return true,
                b"Tj" | b"TJ" | b"'" | b"\"" => {
                    let fills = matches!(state.text_mode, 0 | 2 | 4 | 6);
                    let strokes = matches!(state.text_mode, 1 | 2 | 5 | 6);
                    if (fills && state.fill_breaks()) || (strokes && state.stroke_breaks()) {
                        return true;
                    }
                }
                b"Do" if depth < MAX_FORM_DEPTH => {
                    let Some(name) = op.name(0) else { continue };
                    let Some(Object::Ref(r)) = resources.and_then(|res| self.entry(res, b"XObject", name)) else { continue };
                    let form = self.doc.resolve(&Object::Ref(r));
                    let Object::Stream(s) = &*form else { continue };
                    if s.dict.name(b"Subtype") != Some(b"Form") {
                        continue;
                    }
                    let own = s.dict.get(b"Resources").map(|x| self.doc.resolve(x)).and_then(|x| x.as_dict().cloned());
                    // Without resources of its own the form names what its caller's resources hold,
                    // so another caller may make it read differently.
                    let key = (r, state, own.is_none().then_some(owner));
                    if self.read.get(&key).is_some_and(|&read_at| read_at <= depth) {
                        continue;
                    }
                    self.read.insert(key, depth);
                    self.reached.insert(r);
                    let Ok(inner) = self.budget.decode(s) else { continue };
                    let inner_owner = if own.is_some() { r } else { owner };
                    if self.content(&inner, own.as_ref().or(resources), inner_owner, state, depth.saturating_add(1)) {
                        return true;
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// A boolean value, followed through a reference.
    fn flag(&self, v: &Object) -> Option<bool> {
        match &*self.doc.resolve(v) {
            Object::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The entry `name` of the resource category `kind`, unresolved.
    fn entry(&self, resources: &Dict, kind: &[u8], name: &[u8]) -> Option<Object> {
        let category = self.doc.resolve(resources.get(kind)?);
        category.as_dict()?.get(name).cloned()
    }

    fn resource(&self, resources: Option<&Dict>, kind: &[u8], name: &[u8]) -> Option<std::sync::Arc<Object>> {
        let entry = self.entry(resources?, kind, name)?;
        Some(self.doc.resolve(&entry))
    }

    /// Whether the colour space resource `name` is ICCBased with four components: `/N` 4, or no
    /// `/N` and a profile whose header gives the data colour space `CMYK` (ICC.1 bytes 16–19).
    ///
    /// `DeviceCMYK` is drawn in the resources' `/DefaultCMYK` when they have one (ISO 32000-2
    /// 8.6.5.6), so it counts when that is ICCBased CMYK.
    fn icc_cmyk(&mut self, resources: Option<&Dict>, name: &[u8]) -> bool {
        let name = if name == b"DeviceCMYK" { b"DefaultCMYK".as_slice() } else { name };
        let Some(space) = self.resource(resources, b"ColorSpace", name) else { return false };
        let Some(a) = space.as_array() else { return false };
        if a.first().map(|k| self.doc.resolve(k)).as_deref().and_then(Object::as_name) != Some(b"ICCBased") {
            return false;
        }
        let Some(profile) = a.get(1).map(|p| self.doc.resolve(p)) else { return false };
        let Object::Stream(s) = &*profile else { return false };
        match s.dict.get(b"N").map(|n| self.doc.resolve(n)).and_then(|n| n.as_int()) {
            Some(n) => n == 4,
            None => s.decoded_within(MAX_ICC_PROFILE).ok().is_some_and(|d| d.get(16..20) == Some(b"CMYK".as_slice())),
        }
    }
}
