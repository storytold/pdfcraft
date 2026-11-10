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
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// How many dictionaries one page's search reads, across its resources, XObjects, patterns, Type
/// 3 fonts and appearance streams. An object [`Shared`] holds counts as one read.
const MAX_VISITS: usize = 4096;
/// How deep resources nest (a form XObject in a form XObject, ...).
const MAX_DEPTH: usize = 32;
/// The resource categories that hold what a page paints with, read in this order.
const CATEGORIES: [&[u8]; 4] = [b"ExtGState", b"XObject", b"Pattern", b"Font"];
/// How many unknown blend mode names one search gathers: enough to name them in the report,
/// without a `/BM` array of a million names comparing each with all the others.
const MAX_BLEND_NAMES: usize = 32;
/// How many objects [`Shared`] keeps what was found under.
const MAX_SHARED: usize = 1 << 16;

/// What the graphics a page's resources reach paint with.
#[derive(Clone, Debug, Default)]
pub(crate) struct Painting {
    /// Something paints transparently.
    pub(crate) transparency: bool,
    /// The blend modes graphics states name that ISO 32000-1 doesn't define.
    pub(crate) unknown_blend_modes: Vec<String>,
}

impl Painting {
    fn add(&mut self, other: &Painting) {
        self.transparency |= other.transparency;
        for name in &other.unknown_blend_modes {
            self.add_blend_mode(name);
        }
    }

    /// Whether no more unknown blend mode names are gathered.
    fn blend_modes_full(&self) -> bool {
        self.unknown_blend_modes.len() >= MAX_BLEND_NAMES
    }

    fn add_blend_mode(&mut self, name: &str) {
        if !self.blend_modes_full() && !self.unknown_blend_modes.iter().any(|n| n == name) {
            self.unknown_blend_modes.push(name.to_owned());
        }
    }
}

/// What was found under the indirect objects (resources, form XObjects, patterns, Type 3 fonts,
/// graphics states) that pages share, so the next page that names one takes it from here instead
/// of walking it again. Only an object read to the end goes in: none whose reading stopped below
/// it at the budget, at the depth limit, or at an object already read on that page (a cycle).
#[derive(Default)]
pub(crate) struct Shared(HashMap<ObjRef, Painting>);

/// An object read in place, or the object a reference names as the document holds it: either way
/// nothing is copied.
enum Held<'o> {
    Direct(&'o Object),
    Resolved(Arc<Object>),
}

impl std::ops::Deref for Held<'_> {
    type Target = Object;

    fn deref(&self) -> &Object {
        match self {
            Held::Direct(o) => o,
            Held::Resolved(o) => o,
        }
    }
}

/// `o`, or the object it refers to, without copying either ([`Document::resolve`] copies a direct
/// object).
fn held<'o>(doc: &Document, o: &'o Object) -> Held<'o> {
    match o {
        Object::Ref(_) => Held::Resolved(doc.resolve(o)),
        o => Held::Direct(o),
    }
}

/// What the page `page` paints with: its resources, and the appearance streams of its
/// annotations. `shared` carries what earlier pages found under the objects they share.
pub(crate) fn page_painting(doc: &Document, page: &Dict, shared: &mut Shared) -> Painting {
    walk_page(doc, page, shared).found
}

/// How many reads the search of `page` took, each object taken from `shared` counting as one.
#[cfg(test)]
pub(crate) fn page_reads(doc: &Document, page: &Dict, shared: &mut Shared) -> usize {
    walk_page(doc, page, shared).visits
}

fn walk_page<'a>(doc: &'a Document, page: &Dict, shared: &'a mut Shared) -> Walk<'a> {
    let mut walk = Walk { doc, shared, seen: HashMap::new(), visits: 0, cuts: 0, found: Painting::default() };
    if let Some(r) = page.get(b"Resources") {
        walk.visit(r, 0, Walk::resources);
    }
    let annots = page.get(b"Annots").map(|a| held(doc, a));
    for a in annots.as_deref().and_then(Object::as_array).into_iter().flatten() {
        let a = held(doc, a);
        let Some(ap) = a.as_dict().and_then(|a| a.get(b"AP")).map(|ap| held(doc, ap)) else { continue };
        let Some(ap) = ap.as_dict() else { continue };
        // Each appearance (normal, rollover, down) is a stream, or a dictionary of streams by state.
        for (_, v) in ap.iter() {
            match &*held(doc, v) {
                Object::Stream(_) => walk.visit(v, 0, Walk::xobject),
                Object::Dict(states) => {
                    for (_, s) in states.iter() {
                        walk.visit(s, 0, Walk::xobject);
                    }
                }
                _ => {}
            }
        }
    }
    walk
}

/// The default colour space resource name and component count for a device colour space name.
fn device_default_space(name: &[u8]) -> Option<(&'static [u8], i64)> {
    match name {
        b"DeviceGray" => Some((b"DefaultGray", 1)),
        b"DeviceRGB" => Some((b"DefaultRGB", 3)),
        b"DeviceCMYK" => Some((b"DefaultCMYK", 4)),
        _ => None,
    }
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
        Object::Name(n) => {
            let Some(space) = device_default_space(n) else { return false };
            space
        }
        Object::Array(a) => match a.as_slice() {
            [only] => {
                let Some(space) = doc.resolve(only).as_name().and_then(device_default_space) else {
                    return independent_blending_space(doc, a).is_some();
                };
                space
            }
            _ => return independent_blending_space(doc, a).is_some(),
        },
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
    // A CalGray or CalRGB space needs its dictionary, with the required `/WhitePoint` [X Y Z]: X
    // and Z positive, Y 1 (ISO 32000-1 Tables 63 and 64).
    let calibrated = || {
        let params = a.get(1).map(|p| doc.resolve(p));
        let white = params.as_deref().and_then(Object::as_dict).and_then(|d| d.get(b"WhitePoint")).map(|w| doc.resolve(w));
        let values: Option<Vec<f64>> = white.as_deref().and_then(Object::as_array).and_then(|w| w.iter().map(|v| doc.resolve(v).as_f64()).collect());
        matches!(values.as_deref(), Some(&[x, y, z]) if x > 0.0 && y == 1.0 && z > 0.0)
    };
    match family.as_deref().and_then(Object::as_name)? {
        b"CalGray" => calibrated().then_some(1),
        b"CalRGB" => calibrated().then_some(3),
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
    shared: &'a mut Shared,
    /// The indirect objects already read on this page, each with the shallowest depth it was read
    /// at: an object is read again only from a shallower place, so a cycle ends and a deep first
    /// visit doesn't hide what a shallower one would read.
    seen: HashMap<ObjRef, usize>,
    visits: usize,
    /// How many times reading stopped short (the budget, the depth limit, an object already read
    /// on this page). An object with a stop below it doesn't go in [`Shared`]: what was found
    /// under it may be incomplete.
    cuts: usize,
    found: Painting,
}

impl Walk<'_> {
    /// Read `o`, reached `depth` levels down, with `read`: from [`Shared`] when an earlier page
    /// read it to the end, else when it is within the budget and the depth, and not an indirect
    /// object read before on this page from as shallow a place.
    fn visit(&mut self, o: &Object, depth: usize, read: fn(&mut Self, &Object, usize)) {
        if self.visits >= MAX_VISITS || depth > MAX_DEPTH {
            self.cuts = self.cuts.saturating_add(1);
            return;
        }
        let Object::Ref(r) = o else {
            self.visits += 1;
            read(self, o, depth);
            return;
        };
        if let Some(found) = self.shared.0.get(r) {
            self.visits += 1;
            self.found.add(found);
            return;
        }
        if self.seen.get(r).is_some_and(|&d| d <= depth) {
            self.cuts = self.cuts.saturating_add(1);
            return;
        }
        self.seen.insert(*r, depth);
        self.visits += 1;
        // Gather what this object holds on its own, to keep for the next page, then add it to what
        // the page holds.
        let outer = std::mem::take(&mut self.found);
        let cuts = self.cuts;
        read(self, o, depth);
        let below = std::mem::replace(&mut self.found, outer);
        self.found.add(&below);
        if self.cuts == cuts && self.shared.0.len() < MAX_SHARED {
            self.shared.0.insert(*r, below);
        }
    }

    fn resources(&mut self, r: &Object, depth: usize) {
        let doc = self.doc;
        let res = held(doc, r);
        let Some(res) = res.as_dict() else { return };
        for category in CATEGORIES {
            let entries = res.get(category).map(|d| held(doc, d));
            let Some(entries) = entries.as_deref().and_then(Object::as_dict) else { continue };
            for (_, v) in entries.iter() {
                // Once the budget is spent nothing more is read, so nothing more is gathered either.
                if self.visits >= MAX_VISITS {
                    self.cuts = self.cuts.saturating_add(1);
                    return;
                }
                match category {
                    b"ExtGState" => self.visit(v, depth, Self::ext_gstate),
                    b"XObject" => self.visit(v, depth, Self::xobject),
                    b"Pattern" => self.visit(v, depth, Self::pattern),
                    _ => self.visit(v, depth, Self::type3),
                }
            }
        }
    }

    /// A graphics state (ISO 32000-1 Table 58): whether it paints transparently, and the blend
    /// modes it names that ISO 32000-1 doesn't define.
    fn ext_gstate(&mut self, g: &Object, depth: usize) {
        let doc = self.doc;
        let g = held(doc, g);
        let Some(g) = g.as_dict() else { return };
        let below_one = |key: &[u8]| g.get(key).and_then(|v| held(doc, v).as_f64()).is_some_and(|a| a < 1.0);
        let bm = g.get(b"BM").map(|b| held(doc, b));
        let blend = bm.as_deref().is_some_and(|b| blends(doc, b));
        // A soft mask is a dictionary; `/None`, null or anything else is none (ISO 32000-1 Table 58).
        let smask = g.get(b"SMask").map(|m| held(doc, m));
        let mask = smask.as_deref().is_some_and(|m| matches!(m, Object::Dict(_)));
        self.found.transparency |= below_one(b"CA") || below_one(b"ca") || blend || mask;
        if let Some(bm) = bm.as_deref() {
            unknown_blend_modes(doc, bm, &mut self.found);
        }
        if let Some(Object::Dict(m)) = smask.as_deref()
            && let Some(group) = m.get(b"G")
        {
            self.visit(group, depth + 1, Self::xobject);
        }
    }

    /// An image with a soft mask, or a form XObject that is a transparency group or holds
    /// transparency in its resources.
    fn xobject(&mut self, x: &Object, depth: usize) {
        let doc = self.doc;
        let x = held(doc, x);
        let Object::Stream(s) = &*x else { return };
        let d = &s.dict;
        if d.get(b"Subtype").is_some_and(|s| held(doc, s).as_name() == Some(b"Image")) {
            let soft_mask = d.get(b"SMask").is_some_and(|m| matches!(&*held(doc, m), Object::Stream(_)));
            let in_data = d.get(b"SMaskInData").and_then(|v| held(doc, v).as_int()).is_some_and(|v| v > 0);
            self.found.transparency |= soft_mask || in_data;
            return;
        }
        let group = d
            .get(b"Group")
            .map(|g| held(doc, g))
            .is_some_and(|g| g.as_dict().is_some_and(|g| g.get(b"S").is_some_and(|s| held(doc, s).as_name() == Some(b"Transparency"))));
        self.found.transparency |= group;
        if let Some(r) = d.get(b"Resources") {
            self.visit(r, depth + 1, Self::resources);
        }
    }

    /// A tiling pattern's resources, or a shading pattern's graphics state.
    fn pattern(&mut self, p: &Object, depth: usize) {
        let p = held(self.doc, p);
        let Some(d) = (match &*p {
            Object::Stream(s) => Some(&s.dict),
            Object::Dict(d) => Some(d),
            _ => None,
        }) else {
            return;
        };
        if let Some(g) = d.get(b"ExtGState") {
            self.visit(g, depth + 1, Self::ext_gstate);
        }
        if let Some(r) = d.get(b"Resources") {
            self.visit(r, depth + 1, Self::resources);
        }
    }

    /// A Type 3 font's resources, which its glyph procedures paint with.
    fn type3(&mut self, f: &Object, depth: usize) {
        let doc = self.doc;
        let f = held(doc, f);
        let Some(d) = f.as_dict() else { return };
        if !d.get(b"Subtype").is_some_and(|s| held(doc, s).as_name() == Some(b"Type3")) {
            return;
        }
        if let Some(r) = d.get(b"Resources") {
            self.visit(r, depth + 1, Self::resources);
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

/// Add to `found` the names in a `/BM` value (a name, or an array of them) that are not blend
/// modes ISO 32000-1 defines, up to [`MAX_BLEND_NAMES`] of them.
fn unknown_blend_modes(doc: &Document, bm: &Object, found: &mut Painting) {
    let one = |found: &mut Painting, n: &[u8]| {
        if !BLEND_MODES.contains(&n) {
            found.add_blend_mode(&String::from_utf8_lossy(n));
        }
    };
    match bm {
        Object::Name(n) => one(found, n),
        Object::Array(a) => {
            for m in a {
                if found.blend_modes_full() {
                    return;
                }
                if let Some(n) = held(doc, m).as_name() {
                    one(found, n);
                }
            }
        }
        _ => {}
    }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_cos::Document;

    use super::{Shared, page_names_blending_space, page_painting};
    use crate::{Level, verify};

    fn build(objs: &[&str]) -> Document {
        let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        let id = "/ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]";
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {id} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        Document::open(Arc::new(out)).unwrap()
    }

    #[test]
    fn walks_soft_mask_transparency_group_for_unknown_blend_modes() {
        let doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Group << /S /Transparency /CS [/CalRGB << /WhitePoint [0.9505 1 1.089] >>] >> /Resources << /ExtGState << /G0 << /SMask << /Type /Mask /S /Luminosity /G 4 0 R >> >> >> >> >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /ExtGState << /G1 << /BM /MaskCustom >> >> >> /Length 0 >>\nstream\n\nendstream",
        ]);
        let page = &pdfcraft_model::pages(&doc)[0];
        let painting = page_painting(&doc, &page.dict, &mut Shared::default());
        assert!(painting.transparency);
        assert_eq!(painting.unknown_blend_modes, ["MaskCustom"]);
        let messages: Vec<String> = verify(&doc, Level::A2b).into_iter().filter(|i| i.clause == "6.2.10").map(|i| i.message).collect();
        assert_eq!(messages, ["A graphics state uses the blend mode MaskCustom, which ISO 32000-1 doesn't define"]);
    }

    #[test]
    fn resolves_indirect_subtypes_and_transparency_group_kinds() {
        // Indirect `/Image` subtype on an image with `/SMaskInData 1`.
        let doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Im 4 0 R >> >> >>",
            "<< /Type /XObject /Subtype 5 0 R /Width 1 /Height 1 /SMaskInData 1 /Length 0 >>\nstream\n\nendstream",
            "/Image",
        ]);
        let page = &pdfcraft_model::pages(&doc)[0];
        assert!(page_painting(&doc, &page.dict, &mut Shared::default()).transparency);

        // Indirect `/Transparency` group kind `/S` on a form XObject.
        let doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Fm 4 0 R >> >> >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Group << /S 5 0 R >> /Length 0 >>\nstream\n\nendstream",
            "/Transparency",
        ]);
        let page = &pdfcraft_model::pages(&doc)[0];
        assert!(page_painting(&doc, &page.dict, &mut Shared::default()).transparency);

        // Indirect `/Type3` subtype on a font with translucent resources and an unknown blend mode.
        let doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F0 4 0 R >> >> >>",
            "<< /Type /Font /Subtype 5 0 R /Resources << /ExtGState << /G << /ca 0.5 /BM /T3Mode >> >> >> >>",
            "/Type3",
        ]);
        let page = &pdfcraft_model::pages(&doc)[0];
        let painting = page_painting(&doc, &page.dict, &mut Shared::default());
        assert!(painting.transparency);
        assert_eq!(painting.unknown_blend_modes, ["T3Mode"]);
    }

    #[test]
    fn accepts_single_element_device_colour_array_with_default_space() {
        let cal_rgb = "[/CalRGB << /WhitePoint [0.9505 1 1.089] >>]";
        let doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 6 0 R] /Count 4 /MediaBox [0 0 200 200] >>",
            &format!(
                "<< /Type /Page /Parent 2 0 R /Group << /S /Transparency /CS [/DeviceRGB] >> /Resources << /ColorSpace << /DefaultRGB {cal_rgb} >> >> >>"
            ),
            &format!(
                "<< /Type /Page /Parent 2 0 R /Group << /S /Transparency /CS [7 0 R] >> /Resources << /ColorSpace << /DefaultRGB {cal_rgb} >> >> >>"
            ),
            "<< /Type /Page /Parent 2 0 R /Group << /S /Transparency /CS [/DeviceRGB] >> /Resources << >> >>",
            &format!(
                "<< /Type /Page /Parent 2 0 R /Group << /S /Transparency /CS [/DeviceRGB /Extra] >> /Resources << /ColorSpace << /DefaultRGB {cal_rgb} >> >> >>"
            ),
            "/DeviceRGB",
        ]);
        let pages = pdfcraft_model::pages(&doc);
        assert!(page_names_blending_space(&doc, &pages[0].dict));
        assert!(page_names_blending_space(&doc, &pages[1].dict));
        assert!(!page_names_blending_space(&doc, &pages[2].dict));
        assert!(!page_names_blending_space(&doc, &pages[3].dict));
    }
}
