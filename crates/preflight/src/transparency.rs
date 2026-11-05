//! PDF/A-2 6.2.10: a graphics state's blend mode is one ISO 32000-1 defines, and a page that
//! contains transparency, in a file without a PDF/A output intent, names its blending colour
//! space: its `/Group` dictionary has a `/CS` entry, which as any colour space in such a file is
//! not a device space without a default (6.2.4.3).
//!
//! A page contains transparency when something its resources reach can paint transparently (ISO
//! 32000-1 11.6.4.4 to 11.6.6): a graphics state with a constant alpha (`/CA`, `/ca`) below 1,
//! a blend mode other than `Normal` or `Compatible`, or a soft mask; an image with a soft mask
//! (`/SMask`, or `/SMaskInData` above 0); or a transparency group XObject. The resources looked
//! through are the page's own, those of the form XObjects, tiling patterns and Type 3 fonts they
//! hold, and those of the appearance streams of the page's annotations. Whether the content uses
//! a resource is not asked: a resource a page holds is one it may paint with. Blend modes are read
//! from the same graphics states, so private data (`/PieceInfo`) is never taken for one.

use std::collections::HashMap;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// How many dictionaries one page's search reads, across its resources, XObjects, patterns, Type
/// 3 fonts and appearance streams.
const MAX_VISITS: usize = 4096;
/// How deep resources nest (a form XObject in a form XObject, ...).
const MAX_DEPTH: usize = 32;
/// The resource categories that hold what a page paints with, read in this order.
const CATEGORIES: [&[u8]; 4] = [b"ExtGState", b"XObject", b"Pattern", b"Font"];

/// What the graphics a page's resources reach paint with.
#[derive(Debug, Default)]
pub(crate) struct Painting {
    /// Something paints transparently.
    pub(crate) transparency: bool,
    /// The blend modes graphics states name that ISO 32000-1 doesn't define.
    pub(crate) unknown_blend_modes: Vec<String>,
}

/// What the page `page` paints with: its resources, and the appearance streams of its
/// annotations.
pub(crate) fn page_painting(doc: &Document, page: &Dict) -> Painting {
    let mut walk = Walk { doc, seen: HashMap::new(), visits: 0, found: Painting::default() };
    if let Some(r) = page.get(b"Resources") {
        walk.resources(r, 0);
    }
    let annots = page.get(b"Annots").map(|a| doc.resolve(a));
    for a in annots.as_deref().and_then(Object::as_array).into_iter().flatten() {
        let a = doc.resolve(a);
        let Some(ap) = a.as_dict().and_then(|a| a.get(b"AP")).map(|ap| doc.resolve(ap)) else { continue };
        let Some(ap) = ap.as_dict() else { continue };
        // Each appearance (normal, rollover, down) is a stream, or a dictionary of streams by state.
        for (_, v) in ap.iter() {
            match &*doc.resolve(v) {
                Object::Stream(_) => walk.xobject(v, 0),
                Object::Dict(states) => {
                    for (_, s) in states.iter() {
                        walk.xobject(s, 0);
                    }
                }
                _ => {}
            }
        }
    }
    walk.found
}

/// Whether `page` names a blending colour space PDF/A accepts without an output intent: its
/// `/Group` has a `/CS` that is a blending colour space (ISO 32000-1 and 32000-2 11.3.4:
/// `CalGray`, `CalRGB`, or an `ICCBased` space of 1, 3 or 4 components whose profile describes
/// gray, RGB or CMYK; not Lab, Indexed, Pattern, Separation or DeviceN), and not a device colour
/// space unless the page's resources give that device space a default (`DefaultGray`,
/// `DefaultRGB`, `DefaultCMYK`) that is itself such a space with as many components (ISO
/// 32000-1 8.6.5.6), as PDF/A-2 6.2.4.3 asks of any device colour.
pub(crate) fn page_names_blending_space(doc: &Document, page: &Dict) -> bool {
    let group = page.get(b"Group").map(|g| doc.resolve(g));
    let Some(cs) = group.as_deref().and_then(Object::as_dict).and_then(|g| g.get(b"CS")).map(|c| doc.resolve(c)) else { return false };
    let (default, components): (&[u8], i64) = match &*cs {
        Object::Name(n) if n == b"DeviceGray" => (b"DefaultGray", 1),
        Object::Name(n) if n == b"DeviceRGB" => (b"DefaultRGB", 3),
        Object::Name(n) if n == b"DeviceCMYK" => (b"DefaultCMYK", 4),
        Object::Array(a) => return independent_blending_space(doc, a).is_some(),
        _ => return false,
    };
    let resources = page.get(b"Resources").map(|r| doc.resolve(r));
    let spaces = resources.as_deref().and_then(Object::as_dict).and_then(|r| r.get(b"ColorSpace")).map(|c| doc.resolve(c));
    let space = spaces.as_deref().and_then(Object::as_dict).and_then(|s| s.get(default)).map(|d| doc.resolve(d));
    space.as_deref().and_then(Object::as_array).and_then(|a| independent_blending_space(doc, a)) == Some(components)
}

/// The number of components of the colour space array `a` when it is a device-independent
/// blending colour space: `CalGray` (1), `CalRGB` (3), or `ICCBased` with 1, 3 or 4 components
/// whose profile header, where it can be read (within the output intent's profile cap), says
/// gray, RGB or CMYK to match.
fn independent_blending_space(doc: &Document, a: &[Object]) -> Option<i64> {
    let family = a.first().map(|f| doc.resolve(f));
    match family.as_deref().and_then(Object::as_name)? {
        b"CalGray" => Some(1),
        b"CalRGB" => Some(3),
        b"ICCBased" => {
            let profile = a.get(1).map(|p| doc.resolve(p));
            let Some(Object::Stream(s)) = profile.as_deref() else { return None };
            let components = s.dict.get(b"N").and_then(|n| doc.resolve(n).as_int())?;
            // ICC.1 header: the data colour space is bytes 16–19.
            let header: Option<i64> = match s.decoded_within(crate::MAX_ICC_PROFILE).ok().and_then(|data| data.get(16..20).map(<[u8]>::to_vec)) {
                Some(space) => match space.as_slice() {
                    b"GRAY" => Some(1),
                    b"RGB " => Some(3),
                    b"CMYK" => Some(4),
                    _ => return None,
                },
                None => None,
            };
            (matches!(components, 1 | 3 | 4) && header.is_none_or(|h| h == components)).then_some(components)
        }
        _ => None,
    }
}

struct Walk<'a> {
    doc: &'a Document,
    /// The indirect objects already read, each with the shallowest depth it was read at: an
    /// object is read again only from a shallower place, so a cycle ends and a deep first visit
    /// doesn't hide what a shallower one would read.
    seen: HashMap<ObjRef, usize>,
    visits: usize,
    found: Painting,
}

impl Walk<'_> {
    /// Whether `o`, reached `depth` levels down, may be read: within the budget and the depth, and
    /// an indirect object not read before from as shallow a place.
    fn enter(&mut self, o: &Object, depth: usize) -> bool {
        if self.visits >= MAX_VISITS || depth > MAX_DEPTH {
            return false;
        }
        if let Object::Ref(r) = o {
            if self.seen.get(r).is_some_and(|&d| d <= depth) {
                return false;
            }
            self.seen.insert(*r, depth);
        }
        self.visits += 1;
        true
    }

    fn resources(&mut self, r: &Object, depth: usize) {
        if !self.enter(r, depth) {
            return;
        }
        let doc = self.doc;
        let Some(res) = doc.resolve(r).as_dict().cloned() else { return };
        for category in CATEGORIES {
            // Once the budget is spent nothing more is read, so nothing more is gathered either.
            if self.visits >= MAX_VISITS {
                return;
            }
            let entries: Vec<Object> = res
                .get(category)
                .map(|d| doc.resolve(d))
                .and_then(|d| d.as_dict().map(|d| d.iter().map(|(_, v)| v.clone()).collect()))
                .unwrap_or_default();
            for v in &entries {
                match category {
                    b"ExtGState" => self.ext_gstate(v, depth),
                    b"XObject" => self.xobject(v, depth),
                    b"Pattern" => self.pattern(v, depth),
                    _ => self.type3(v, depth),
                }
            }
        }
    }

    /// A graphics state (ISO 32000-1 Table 58): whether it paints transparently, and the blend
    /// modes it names that ISO 32000-1 doesn't define.
    fn ext_gstate(&mut self, g: &Object, depth: usize) {
        if !self.enter(g, depth) {
            return;
        }
        let doc = self.doc;
        let g = doc.resolve(g);
        let Some(g) = g.as_dict() else { return };
        let below_one = |key: &[u8]| g.get(key).and_then(|v| doc.resolve(v).as_f64()).is_some_and(|a| a < 1.0);
        let bm = g.get(b"BM").map(|b| doc.resolve(b));
        let blend = bm.as_deref().is_some_and(|b| blends(doc, b));
        // A soft mask is a dictionary; `/None`, null or anything else is none (ISO 32000-1 Table 58).
        let mask = g.get(b"SMask").map(|m| doc.resolve(m)).is_some_and(|m| matches!(&*m, Object::Dict(_)));
        self.found.transparency |= below_one(b"CA") || below_one(b"ca") || blend || mask;
        for name in bm.as_deref().map(|b| unknown_blend_modes(doc, b)).unwrap_or_default() {
            if !self.found.unknown_blend_modes.contains(&name) {
                self.found.unknown_blend_modes.push(name);
            }
        }
    }

    /// An image with a soft mask, or a form XObject that is a transparency group or holds
    /// transparency in its resources.
    fn xobject(&mut self, x: &Object, depth: usize) {
        if !self.enter(x, depth) {
            return;
        }
        let doc = self.doc;
        let x = doc.resolve(x);
        let Object::Stream(s) = &*x else { return };
        let d = &s.dict;
        if d.name(b"Subtype") == Some(b"Image") {
            let soft_mask = d.get(b"SMask").is_some_and(|m| matches!(&*doc.resolve(m), Object::Stream(_)));
            let in_data = d.get(b"SMaskInData").and_then(|v| doc.resolve(v).as_int()).is_some_and(|v| v > 0);
            self.found.transparency |= soft_mask || in_data;
            return;
        }
        let group = d.get(b"Group").map(|g| doc.resolve(g)).is_some_and(|g| g.as_dict().is_some_and(|g| g.name(b"S") == Some(b"Transparency")));
        self.found.transparency |= group;
        if let Some(r) = d.get(b"Resources") {
            self.resources(r, depth + 1);
        }
    }

    /// A tiling pattern's resources, or a shading pattern's graphics state.
    fn pattern(&mut self, p: &Object, depth: usize) {
        if !self.enter(p, depth) {
            return;
        }
        let p = self.doc.resolve(p);
        let Some(d) = (match &*p {
            Object::Stream(s) => Some(&s.dict),
            Object::Dict(d) => Some(d),
            _ => None,
        }) else {
            return;
        };
        if let Some(g) = d.get(b"ExtGState") {
            self.ext_gstate(g, depth + 1);
        }
        if let Some(r) = d.get(b"Resources") {
            self.resources(r, depth + 1);
        }
    }

    /// A Type 3 font's resources, which its glyph procedures paint with.
    fn type3(&mut self, f: &Object, depth: usize) {
        if !self.enter(f, depth) {
            return;
        }
        let f = self.doc.resolve(f);
        let Some(d) = f.as_dict() else { return };
        if d.name(b"Subtype") != Some(b"Type3") {
            return;
        }
        if let Some(r) = d.get(b"Resources") {
            self.resources(r, depth + 1);
        }
    }
}

/// The blend modes of ISO 32000-1 11.3.5 (Tables 136 and 137), and `Compatible` (Table 58).
const BLEND_MODES: &[&[u8]] = &[
    b"Normal",
    b"Compatible",
    b"Multiply",
    b"Screen",
    b"Overlay",
    b"Darken",
    b"Lighten",
    b"ColorDodge",
    b"ColorBurn",
    b"HardLight",
    b"SoftLight",
    b"Difference",
    b"Exclusion",
    b"Hue",
    b"Saturation",
    b"Color",
    b"Luminosity",
];

/// The names in a `/BM` value (a name, or an array of them) that are not blend modes ISO 32000-1
/// defines.
fn unknown_blend_modes(doc: &Document, bm: &Object) -> Vec<String> {
    let names: Vec<Vec<u8>> = match bm {
        Object::Name(n) => vec![n.clone()],
        Object::Array(a) => a.iter().filter_map(|m| doc.resolve(m).as_name().map(<[u8]>::to_vec)).collect(),
        _ => Vec::new(),
    };
    names.into_iter().filter(|n| !BLEND_MODES.contains(&n.as_slice())).map(|n| String::from_utf8_lossy(&n).into_owned()).collect()
}

/// Whether a `/BM` value blends: a name other than `Normal` or `Compatible`, or an array whose
/// first name is not one of those.
fn blends(doc: &Document, b: &Object) -> bool {
    let normal = |n: &[u8]| n == b"Normal" || n == b"Compatible";
    match b {
        Object::Name(n) => !normal(n),
        Object::Array(a) => a.iter().find_map(|m| doc.resolve(m).as_name().map(|n| !normal(n))).unwrap_or(false),
        _ => false,
    }
}
