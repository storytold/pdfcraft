//! PDF/A-2 and -3 6.1.8 (PDF/A-4 6.1.7): names that stand for text are UTF-8.
//!
//! ISO 19005-2/-3 6.1.8 and ISO 19005-4 6.1.7 ask that a name, its `#` escapes expanded (ISO
//! 32000-2 7.3.5), be a valid UTF-8 byte sequence. The names read here are those a reader turns
//! into text, where a name in another encoding shows wrong:
//!
//! - the `/BaseFont` of a font the pages use, other than a Type 0 font, whose name is taken from
//!   its descendant (ISO 32000-2 9.6.2, 9.7.4);
//! - the colorant names of Separation and DeviceN colour spaces (8.6.6.4, 8.6.6.5);
//! - structure types: the `/S` of structure elements and the keys and values of the role map
//!   (14.7.2, 14.7.3).
//!
//! Fonts and colour spaces are those the pages reach: their resources, those of the form
//! XObjects, tiling patterns and Type 3 fonts in them, of the soft masks' groups and the fonts of
//! their graphics state parameter dictionaries (Table 57), the colour spaces of their images and
//! shadings, and the appearance streams of their annotations. Structure elements are those the
//! structure tree reaches from `/StructTreeRoot`. Other names, such as resource names, annotation
//! states and the keys of private data, aren't read: they name parts of the file, not text a reader
//! shows.

use std::collections::HashMap;

use pdfcraft_cos::{Document, ObjRef, Object};

/// How many objects the search reads in all. Past it, the rest of the document isn't read.
const MAX_VISITS: usize = 1 << 16;
/// How deep resources, colour spaces and structure elements nest.
const MAX_DEPTH: usize = 64;

/// The first name of each kind that isn't UTF-8, written with its other bytes `#`-escaped.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Findings {
    pub(crate) font: Option<String>,
    pub(crate) colorant: Option<String>,
    pub(crate) structure_type: Option<String>,
}

pub(crate) fn check(doc: &Document) -> Findings {
    let mut walk = Walk { doc, seen: HashMap::new(), visits: 0, found: Findings::default() };
    for p in pdfcraft_model::pages(doc) {
        if let Some(r) = p.dict.get(b"Resources") {
            walk.visit(r, 0, Walk::resources);
        }
        let annots = p.dict.get(b"Annots").map(|a| doc.resolve(a));
        for a in annots.as_deref().and_then(Object::as_array).into_iter().flatten() {
            let a = doc.resolve(a);
            let Some(ap) = a.as_dict().and_then(|a| a.get(b"AP")).map(|ap| doc.resolve(ap)) else { continue };
            // Each appearance is a stream, or a dictionary of streams by state.
            for (_, v) in ap.as_dict().into_iter().flat_map(|d| d.iter()) {
                match &*doc.resolve(v) {
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
    }
    let catalog = doc.root().map(|r| doc.get(r));
    if let Some(tree) = catalog.as_deref().and_then(Object::as_dict).and_then(|c| c.get(b"StructTreeRoot")).map(|t| doc.resolve(t))
        && let Some(tree) = tree.as_dict()
    {
        if let Some(map) = tree.get(b"RoleMap").map(|m| doc.resolve(m))
            && let Some(map) = map.as_dict()
        {
            for (k, v) in map.iter() {
                walk.structure_type(k);
                if let Some(v) = doc.resolve(v).as_name() {
                    walk.structure_type(v);
                }
            }
        }
        if let Some(k) = tree.get(b"K") {
            walk.visit(k, 0, Walk::structure);
        }
    }
    walk.found
}

/// How one kind of object the walk reaches is read.
type Read<'a> = fn(&mut Walk<'a>, &Object, usize);

struct Walk<'a> {
    doc: &'a Document,
    /// The indirect objects read, by how each was read, each with the least depth it was read at:
    /// one reached again less deep is read again, since the depth limit may have cut off what lies
    /// under it before. One object may be read as two kinds, such as a dictionary that is both a
    /// pattern's `/Shading` and its `/ExtGState`, and each reading finds names of its own.
    seen: HashMap<(ObjRef, usize), usize>,
    visits: usize,
    found: Findings,
}

impl<'a> Walk<'a> {
    /// Read `o` with `read`, once for an indirect object and each way of reading it unless it is
    /// reached again less deep, within the budget and the depth. A cycle reaches an object again
    /// deeper, so it ends.
    fn visit(&mut self, o: &Object, depth: usize, read: Read<'a>) {
        if self.visits >= MAX_VISITS || depth > MAX_DEPTH {
            return;
        }
        if let Object::Ref(r) = o {
            // The reader's address tells the kinds apart. Two readers the compiler merged into one
            // read alike, and one given two addresses is only read twice, within the budget.
            let key = (*r, read as usize);
            if self.seen.get(&key).is_some_and(|&read_at| read_at <= depth) {
                return;
            }
            self.seen.insert(key, depth);
        }
        self.visits = self.visits.saturating_add(1);
        let held = self.doc.resolve(o);
        read(self, &held, depth);
    }

    fn resources(&mut self, r: &Object, depth: usize) {
        let doc = self.doc;
        let Some(res) = r.as_dict() else { return };
        let reads: [(&[u8], Read<'a>); 6] = [
            (b"Font", Self::font),
            (b"ExtGState", Self::graphics_state),
            (b"XObject", Self::xobject),
            (b"ColorSpace", Self::colour_space),
            (b"Pattern", Self::pattern),
            (b"Shading", Self::shading),
        ];
        for (key, read) in reads {
            let Some(entries) = res.get(key).map(|d| doc.resolve(d)) else { continue };
            for (_, v) in entries.as_dict().into_iter().flat_map(|d| d.iter()) {
                self.visit(v, depth.saturating_add(1), read);
            }
        }
    }

    fn font(&mut self, f: &Object, depth: usize) {
        let doc = self.doc;
        let Some(d) = f.as_dict() else { return };
        let subtype = d.get(b"Subtype").map(|s| doc.resolve(s));
        match subtype.as_deref().and_then(Object::as_name) {
            Some(b"Type0") => {
                let kids = d.get(b"DescendantFonts").map(|k| doc.resolve(k));
                for k in kids.as_deref().and_then(Object::as_array).into_iter().flatten() {
                    self.visit(k, depth.saturating_add(1), Self::font);
                }
            }
            Some(sub) => {
                if let Some(name) = d.get(b"BaseFont").map(|n| doc.resolve(n))
                    && let Some(name) = name.as_name()
                {
                    note(&mut self.found.font, name);
                }
                if sub == b"Type3"
                    && let Some(r) = d.get(b"Resources")
                {
                    self.visit(r, depth.saturating_add(1), Self::resources);
                }
            }
            None => {}
        }
    }

    /// A graphics state parameter dictionary's font, `[font size]`, and its soft mask's group,
    /// a form XObject (ISO 32000-2 Table 57, 11.6.5.2).
    fn graphics_state(&mut self, gs: &Object, depth: usize) {
        let doc = self.doc;
        let Some(d) = gs.as_dict() else { return };
        let font = d.get(b"Font").map(|f| doc.resolve(f));
        if let Some(f) = font.as_deref().and_then(Object::as_array).and_then(|a| a.first()) {
            self.visit(f, depth.saturating_add(1), Self::font);
        }
        let mask = d.get(b"SMask").map(|m| doc.resolve(m));
        if let Some(g) = mask.as_deref().and_then(Object::as_dict).and_then(|m| m.get(b"G")) {
            self.visit(g, depth.saturating_add(1), Self::xobject);
        }
    }

    /// An image's colour space, or a form XObject's resources.
    fn xobject(&mut self, x: &Object, depth: usize) {
        let doc = self.doc;
        let Object::Stream(s) = x else { return };
        let subtype = s.dict.get(b"Subtype").map(|t| doc.resolve(t));
        if subtype.as_deref().and_then(Object::as_name) == Some(b"Image") {
            if let Some(cs) = s.dict.get(b"ColorSpace") {
                self.visit(cs, depth.saturating_add(1), Self::colour_space);
            }
        } else if let Some(r) = s.dict.get(b"Resources") {
            self.visit(r, depth.saturating_add(1), Self::resources);
        }
    }

    /// A tiling pattern's resources, or a shading pattern's shading and graphics state (ISO
    /// 32000-2 Table 76).
    fn pattern(&mut self, p: &Object, depth: usize) {
        let d = match p {
            Object::Stream(s) => &s.dict,
            Object::Dict(d) => d,
            _ => return,
        };
        if let Some(r) = d.get(b"Resources") {
            self.visit(r, depth.saturating_add(1), Self::resources);
        }
        if let Some(sh) = d.get(b"Shading") {
            self.visit(sh, depth.saturating_add(1), Self::shading);
        }
        if let Some(gs) = d.get(b"ExtGState") {
            self.visit(gs, depth.saturating_add(1), Self::graphics_state);
        }
    }

    fn shading(&mut self, sh: &Object, depth: usize) {
        let d = match sh {
            Object::Stream(s) => &s.dict,
            Object::Dict(d) => d,
            _ => return,
        };
        if let Some(cs) = d.get(b"ColorSpace") {
            self.visit(cs, depth.saturating_add(1), Self::colour_space);
        }
    }

    /// The colorant names of a Separation or DeviceN space, and the spaces it is built on.
    fn colour_space(&mut self, cs: &Object, depth: usize) {
        let doc = self.doc;
        let Some(a) = cs.as_array() else { return };
        let family = a.first().map(|f| doc.resolve(f));
        let at = |i: usize| a.get(i);
        match family.as_deref().and_then(Object::as_name) {
            Some(b"Separation") => {
                if let Some(name) = at(1).map(|n| doc.resolve(n))
                    && let Some(name) = name.as_name()
                {
                    note(&mut self.found.colorant, name);
                }
                if let Some(alt) = at(2) {
                    self.visit(alt, depth.saturating_add(1), Self::colour_space);
                }
            }
            Some(b"DeviceN") => {
                let names = at(1).map(|n| doc.resolve(n));
                for n in names.as_deref().and_then(Object::as_array).into_iter().flatten() {
                    if let Some(name) = doc.resolve(n).as_name() {
                        note(&mut self.found.colorant, name);
                    }
                }
                if let Some(alt) = at(2) {
                    self.visit(alt, depth.saturating_add(1), Self::colour_space);
                }
                // The Separation spaces of the attributes' `/Colorants` (Table 71).
                let attrs = at(4).map(|d| doc.resolve(d));
                let colorants = attrs.as_deref().and_then(Object::as_dict).and_then(|d| d.get(b"Colorants")).map(|c| doc.resolve(c));
                for (_, sep) in colorants.as_deref().and_then(Object::as_dict).into_iter().flat_map(|d| d.iter()) {
                    self.visit(sep, depth.saturating_add(1), Self::colour_space);
                }
            }
            Some(b"Indexed" | b"Pattern") => {
                if let Some(base) = at(1) {
                    self.visit(base, depth.saturating_add(1), Self::colour_space);
                }
            }
            _ => {}
        }
    }

    /// A structure element's type, and its kids: a dictionary, a reference or an array of them.
    /// Marked-content and object references (`/Type /MCR`, `/OBJR`) have no type.
    fn structure(&mut self, k: &Object, depth: usize) {
        let doc = self.doc;
        match k {
            Object::Array(kids) => {
                for kid in kids {
                    self.visit(kid, depth.saturating_add(1), Self::structure);
                }
            }
            Object::Dict(e) => {
                if let Some(s) = e.get(b"S").map(|s| doc.resolve(s))
                    && let Some(s) = s.as_name()
                {
                    self.structure_type(s);
                }
                if let Some(kids) = e.get(b"K") {
                    self.visit(kids, depth.saturating_add(1), Self::structure);
                }
            }
            _ => {}
        }
    }

    fn structure_type(&mut self, name: &[u8]) {
        note(&mut self.found.structure_type, name);
    }
}

/// Keep `name` in `slot` when it is the first name there that isn't UTF-8.
fn note(slot: &mut Option<String>, name: &[u8]) {
    if slot.is_none() && std::str::from_utf8(name).is_err() {
        *slot = Some(escaped(name));
    }
}

/// `name` as written in a file: printable ASCII as it is, any other byte and `#` as `#XX`.
fn escaped(name: &[u8]) -> String {
    let mut out = String::with_capacity(name.len().min(256));
    for &b in name.iter().take(127) {
        if b.is_ascii_graphic() && b != b'#' {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("#{b:02X}"));
        }
    }
    out
}
