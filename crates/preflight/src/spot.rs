//! PDF/A-2 6.2.4.4, Separation and DeviceN colour spaces:
//!
//! - Separation spaces of the same colorant name agree on their alternate space and tint
//!   transform, also where a DeviceN space's `/Colorants` dictionary holds them. They are compared
//!   as objects, whether stored directly or by reference, at any depth (ISO 32000-1 7.3.10);
//! - a DeviceN space names each spot colorant it uses in the `/Colorants` dictionary of its
//!   attributes (ISO 32000-1 8.6.6.5, Table 71); a null or missing entry defines nothing. The
//!   process colorants (`Cyan`, `Magenta`, `Yellow`, `Black`, and those an NChannel space's
//!   `/Process` lists) and `None` are not spot colorants.
//!
//! Colour spaces are found wherever they are written: in resource dictionaries, images, shadings
//! and other colour spaces, direct or by reference.

use std::collections::{BTreeSet, HashMap};

use pdfcraft_cos::{Document, ObjRef, Object};

/// How deep a direct object is followed (arrays and dictionaries inside one another): as deep as
/// the parser nests them.
const MAX_DEPTH: u8 = 128;
/// How deep two definitions are compared, references followed.
const MAX_COMPARE_DEPTH: u8 = 32;
/// The colorants of DeviceCMYK, which a DeviceN space may name without defining them.
const PROCESS: [&[u8]; 4] = [b"Cyan", b"Magenta", b"Yellow", b"Black"];

/// What the colour spaces of a document break of 6.2.4.4.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Findings {
    /// Colorant names whose Separation spaces differ, in name order.
    pub(crate) differing: BTreeSet<String>,
    /// Spot colorants a DeviceN space uses without a `/Colorants` entry, in name order.
    pub(crate) undefined: BTreeSet<String>,
}

/// The 6.2.4.4 findings over the indirect objects `objs`.
pub(crate) fn check(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)]) -> Findings {
    let mut scan = Scan { doc, separations: HashMap::new(), findings: Findings::default() };
    for (_, o) in objs {
        scan.walk(o, 0);
    }
    scan.findings
}

struct Scan<'a> {
    doc: &'a Document,
    /// Each colorant name's first Separation space: its alternate space and tint transform.
    separations: HashMap<Vec<u8>, (Object, Object)>,
    findings: Findings,
}

impl Scan<'_> {
    fn walk(&mut self, o: &Object, depth: u8) {
        if depth > MAX_DEPTH {
            return;
        }
        match o {
            Object::Array(a) => {
                match a.first().and_then(Object::as_name) {
                    Some(b"Separation") => self.separation(a),
                    Some(b"DeviceN") => self.devicen(a),
                    _ => {}
                }
                for v in a {
                    self.walk(v, depth.saturating_add(1));
                }
            }
            Object::Dict(d) => {
                for (_, v) in d.iter() {
                    self.walk(v, depth.saturating_add(1));
                }
            }
            Object::Stream(s) => {
                for (_, v) in s.dict.iter() {
                    self.walk(v, depth.saturating_add(1));
                }
            }
            _ => {}
        }
    }

    /// `[/Separation name alternateSpace tintTransform]`: the same name must keep the same
    /// alternate space and tint transform.
    fn separation(&mut self, a: &[Object]) {
        let [_, name, alternate, tint, ..] = a else { return };
        let name = self.doc.resolve(name);
        let Some(name) = name.as_name() else { return };
        let this = (alternate.clone(), tint.clone());
        match self.separations.get(name) {
            Some((alt, fun)) => {
                if !same(self.doc, alt, &this.0, 0) || !same(self.doc, fun, &this.1, 0) {
                    self.findings.differing.insert(String::from_utf8_lossy(name).into_owned());
                }
            }
            None => {
                self.separations.insert(name.to_vec(), this);
            }
        }
    }

    /// `[/DeviceN names alternateSpace tintTransform attributes]`: each spot colorant in `names`
    /// needs an entry in the attributes' `/Colorants`.
    fn devicen(&mut self, a: &[Object]) {
        let Some(names) = a.get(1).map(|n| self.doc.resolve(n)) else { return };
        let Some(names) = names.as_array() else { return };
        let attributes = a.get(4).map(|d| self.doc.resolve(d));
        let attributes = attributes.as_deref().and_then(Object::as_dict);
        let colorants = attributes.and_then(|d| d.get(b"Colorants")).map(|c| self.doc.resolve(c));
        let colorants = colorants.as_deref().and_then(Object::as_dict);
        let nchannel = attributes.and_then(|d| d.get(b"Subtype")).map(|t| self.doc.resolve(t)).is_some_and(|t| t.as_name() == Some(b"NChannel"));
        let process = attributes
            .filter(|_| nchannel)
            .and_then(|d| d.get(b"Process"))
            .map(|p| self.doc.resolve(p))
            .and_then(|p| p.as_dict().and_then(|p| p.get(b"Components")).map(|c| self.doc.resolve(c)));
        let process: Vec<Vec<u8>> = process
            .as_deref()
            .and_then(Object::as_array)
            .map(|c| c.iter().filter_map(|n| self.doc.resolve(n).as_name().map(<[u8]>::to_vec)).collect())
            .unwrap_or_default();
        for n in names.iter().map(|n| self.doc.resolve(n)) {
            let Some(n) = n.as_name() else { continue };
            let spot = n != b"None" && !PROCESS.contains(&n) && !process.iter().any(|p| p == n);
            // A null entry, or one referring to no object, defines nothing (ISO 32000-1 7.3.9).
            let defined = colorants.and_then(|c| c.get(n)).is_some_and(|e| !matches!(*self.doc.resolve(e), Object::Null));
            if spot && !defined {
                self.findings.undefined.insert(String::from_utf8_lossy(n).into_owned());
            }
        }
    }
}

/// Whether `a` and `b` are the same object, wherever their parts are stored: references are
/// followed (to [`MAX_COMPARE_DEPTH`]), so two identical streams under different object numbers
/// are the same.
fn same(doc: &Document, a: &Object, b: &Object, depth: u8) -> bool {
    if let (Object::Ref(x), Object::Ref(y)) = (a, b)
        && x == y
    {
        return true;
    }
    if depth > MAX_COMPARE_DEPTH {
        return false;
    }
    let (a, b) = (doc.resolve(a), doc.resolve(b));
    let deeper = depth.saturating_add(1);
    match (&*a, &*b) {
        (Object::Array(x), Object::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(doc, p, q, deeper)),
        (Object::Dict(x), Object::Dict(y)) => same_dict(doc, x, y, deeper),
        (Object::Stream(x), Object::Stream(y)) => x.raw == y.raw && same_dict(doc, &x.dict, &y.dict, deeper),
        (x, y) => x == y,
    }
}

fn same_dict(doc: &Document, a: &pdfcraft_cos::Dict, b: &pdfcraft_cos::Dict, depth: u8) -> bool {
    // `/Length` may be written directly in one and by reference in the other: `same` resolves it.
    a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|w| same(doc, v, w, depth)))
}
