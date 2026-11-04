//! PDF/A-2 6.2.4.4, Separation and DeviceN colour spaces:
//!
//! - Separation spaces of the same colorant name agree on their alternate space and tint
//!   transform, also where a DeviceN space's `/Colorants` dictionary holds them;
//! - a DeviceN space names each spot colorant it uses in the `/Colorants` dictionary of its
//!   attributes (ISO 32000-1 8.6.6.5, Table 71). The process colorants (`Cyan`, `Magenta`,
//!   `Yellow`, `Black`, and those an NChannel space's `/Process` lists) and `None` are not spot
//!   colorants.
//!
//! Colour spaces are found wherever they are written: in resource dictionaries, images, shadings
//! and other colour spaces, direct or by reference.

use std::collections::{BTreeSet, HashMap};

use pdfcraft_cos::{Document, ObjRef, Object};

/// How deep a direct object is followed (arrays and dictionaries inside one another).
const MAX_DEPTH: u8 = 32;
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
    /// alternate space and tint transform. `All` and `None` name no colorant of their own.
    fn separation(&mut self, a: &[Object]) {
        let [_, name, alternate, tint, ..] = a else { return };
        let Some(name) = name.as_name() else { return };
        if name == b"All" || name == b"None" {
            return;
        }
        let this = ((*self.doc.resolve(alternate)).clone(), (*self.doc.resolve(tint)).clone());
        match self.separations.get(name) {
            Some(first) if *first != this => {
                self.findings.differing.insert(String::from_utf8_lossy(name).into_owned());
            }
            Some(_) => {}
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
        let process = attributes
            .and_then(|d| d.get(b"Process"))
            .map(|p| self.doc.resolve(p))
            .and_then(|p| p.as_dict().and_then(|p| p.get(b"Components")).map(|c| self.doc.resolve(c)));
        let process: Vec<Vec<u8>> = process
            .as_deref()
            .and_then(Object::as_array)
            .map(|c| c.iter().filter_map(|n| n.as_name().map(<[u8]>::to_vec)).collect())
            .unwrap_or_default();
        for n in names.iter().filter_map(Object::as_name) {
            let spot = n != b"None" && !PROCESS.contains(&n) && !process.iter().any(|p| p == n);
            if spot && !colorants.is_some_and(|c| c.get(n).is_some()) {
                self.findings.undefined.insert(String::from_utf8_lossy(n).into_owned());
            }
        }
    }
}
