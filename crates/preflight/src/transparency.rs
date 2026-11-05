//! PDF/A-2 6.2.10: a graphics state's blend mode is one ISO 32000-1 defines, and a page that
//! contains transparency, in a file without a PDF/A output intent, names its blending colour
//! space: its `/Group` dictionary has a `/CS` entry.
//!
//! A page contains transparency when something its resources reach can paint transparently (ISO
//! 32000-1 11.6.4.4 to 11.6.6): a graphics state with a constant alpha (`/CA`, `/ca`) below 1,
//! a blend mode other than `Normal` or `Compatible`, or a soft mask; an image with a soft mask
//! (`/SMask`, or `/SMaskInData` above 0); or a transparency group XObject. The resources looked
//! through are the page's own, those of the form XObjects, tiling patterns and Type 3 fonts they
//! hold, and those of the appearance streams of the page's annotations. Whether the content uses
//! a resource is not asked: a resource a page holds is one it may paint with.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// How many dictionaries one page's search reads, across its resources, XObjects, patterns, Type
/// 3 fonts and appearance streams.
const MAX_VISITS: usize = 4096;
/// How deep resources nest (a form XObject in a form XObject, ...).
const MAX_DEPTH: usize = 32;

/// Whether the page `page` contains transparency.
pub(crate) fn page_has_transparency(doc: &Document, page: &Dict) -> bool {
    let mut walk = Walk { doc, seen: HashSet::new(), visits: 0 };
    if page.get(b"Resources").is_some_and(|r| walk.resources(r, 0)) {
        return true;
    }
    let Some(annots) = page.get(b"Annots").map(|a| doc.resolve(a)) else { return false };
    let Some(annots) = annots.as_array() else { return false };
    annots.iter().any(|a| {
        let a = doc.resolve(a);
        let Some(ap) = a.as_dict().and_then(|a| a.get(b"AP")).map(|ap| doc.resolve(ap)) else { return false };
        let Some(ap) = ap.as_dict() else { return false };
        // Each appearance (normal, rollover, down) is a stream, or a dictionary of streams by state.
        ap.iter().any(|(_, v)| match &*doc.resolve(v) {
            Object::Stream(_) => walk.xobject(v, 0),
            Object::Dict(states) => states.iter().any(|(_, s)| walk.xobject(s, 0)),
            _ => false,
        })
    })
}

/// Whether `page` names its blending colour space: a `/Group` dictionary with a `/CS` entry.
pub(crate) fn page_names_blending_space(doc: &Document, page: &Dict) -> bool {
    page.get(b"Group").map(|g| doc.resolve(g)).is_some_and(|g| g.as_dict().is_some_and(|g| g.get(b"CS").is_some()))
}

struct Walk<'a> {
    doc: &'a Document,
    /// The indirect objects already read: each is looked at once, and a cycle ends there.
    seen: HashSet<ObjRef>,
    visits: usize,
}

impl Walk<'_> {
    /// Whether `o` may be read: within the budget, and an indirect object not read before.
    fn enter(&mut self, o: &Object) -> bool {
        if self.visits >= MAX_VISITS {
            return false;
        }
        self.visits += 1;
        match o {
            Object::Ref(r) => self.seen.insert(*r),
            _ => true,
        }
    }

    fn resources(&mut self, r: &Object, depth: usize) -> bool {
        if depth > MAX_DEPTH || !self.enter(r) {
            return false;
        }
        let doc = self.doc;
        let Some(res) = doc.resolve(r).as_dict().cloned() else { return false };
        let entries = |key: &[u8]| -> Vec<Object> {
            res.get(key).map(|d| doc.resolve(d)).and_then(|d| d.as_dict().map(|d| d.iter().map(|(_, v)| v.clone()).collect())).unwrap_or_default()
        };
        entries(b"ExtGState").iter().any(|g| self.ext_gstate(g))
            || entries(b"XObject").iter().any(|x| self.xobject(x, depth))
            || entries(b"Pattern").iter().any(|p| self.pattern(p, depth))
            || entries(b"Font").iter().any(|f| self.type3(f, depth))
    }

    /// A graphics state that paints transparently (ISO 32000-1 Table 58).
    fn ext_gstate(&mut self, g: &Object) -> bool {
        if !self.enter(g) {
            return false;
        }
        let g = self.doc.resolve(g);
        let Some(g) = g.as_dict() else { return false };
        let doc = self.doc;
        let below_one = |key: &[u8]| g.get(key).and_then(|v| doc.resolve(v).as_f64()).is_some_and(|a| a < 1.0);
        let blend = g.get(b"BM").map(|b| doc.resolve(b)).is_some_and(|b| blends(doc, &b));
        let mask = g.get(b"SMask").map(|m| doc.resolve(m)).is_some_and(|m| !matches!(&*m, Object::Name(n) if n == b"None"));
        below_one(b"CA") || below_one(b"ca") || blend || mask
    }

    /// An image with a soft mask, or a form XObject that is a transparency group or holds
    /// transparency in its resources.
    fn xobject(&mut self, x: &Object, depth: usize) -> bool {
        if !self.enter(x) {
            return false;
        }
        let doc = self.doc;
        let x = doc.resolve(x);
        let Object::Stream(s) = &*x else { return false };
        let d = &s.dict;
        if d.name(b"Subtype") == Some(b"Image") {
            let soft_mask = d.get(b"SMask").is_some_and(|m| matches!(&*doc.resolve(m), Object::Stream(_)));
            let in_data = d.get(b"SMaskInData").and_then(|v| doc.resolve(v).as_int()).is_some_and(|v| v > 0);
            return soft_mask || in_data;
        }
        let group = d.get(b"Group").map(|g| doc.resolve(g)).is_some_and(|g| g.as_dict().is_some_and(|g| g.name(b"S") == Some(b"Transparency")));
        group || d.get(b"Resources").is_some_and(|r| self.resources(r, depth + 1))
    }

    /// A tiling pattern's resources, or a shading pattern's graphics state.
    fn pattern(&mut self, p: &Object, depth: usize) -> bool {
        if !self.enter(p) {
            return false;
        }
        let doc = self.doc;
        let p = doc.resolve(p);
        let Some(d) = (match &*p {
            Object::Stream(s) => Some(&s.dict),
            Object::Dict(d) => Some(d),
            _ => None,
        }) else {
            return false;
        };
        let state = d.get(b"ExtGState").cloned();
        let resources = d.get(b"Resources").cloned();
        state.is_some_and(|g| self.ext_gstate(&g)) || resources.is_some_and(|r| self.resources(&r, depth + 1))
    }

    /// A Type 3 font's resources, which its glyph procedures paint with.
    fn type3(&mut self, f: &Object, depth: usize) -> bool {
        if !self.enter(f) {
            return false;
        }
        let f = self.doc.resolve(f);
        let Some(d) = f.as_dict() else { return false };
        if d.name(b"Subtype") != Some(b"Type3") {
            return false;
        }
        let resources = d.get(b"Resources").cloned();
        resources.is_some_and(|r| self.resources(&r, depth + 1))
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

/// The blend modes ISO 32000-1 doesn't define that the object `o` names in a `/BM` entry: its own,
/// or one of a dictionary written inside it (a page's resources hold their graphics states
/// directly), within [`MAX_DEPTH`].
pub(crate) fn unknown_blend_modes(doc: &Document, o: &Object) -> Vec<String> {
    let mut out = Vec::new();
    blend_modes_within(doc, o, 0, &mut out);
    out
}

fn blend_modes_within(doc: &Document, o: &Object, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_DEPTH {
        return;
    }
    let d = match o {
        Object::Dict(d) => d,
        Object::Stream(s) => &s.dict,
        Object::Array(a) => {
            for v in a {
                blend_modes_within(doc, v, depth + 1, out);
            }
            return;
        }
        _ => return,
    };
    out.extend(unknown_in(doc, d));
    for (k, v) in d.iter() {
        if k != b"BM" {
            blend_modes_within(doc, v, depth + 1, out);
        }
    }
}

/// The names in a graphics state's `/BM` (a name, or an array of them) that are not blend modes
/// ISO 32000-1 defines.
fn unknown_in(doc: &Document, gs: &Dict) -> Vec<String> {
    let Some(bm) = gs.get(b"BM").map(|b| doc.resolve(b)) else { return Vec::new() };
    let names: Vec<Vec<u8>> = match &*bm {
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
