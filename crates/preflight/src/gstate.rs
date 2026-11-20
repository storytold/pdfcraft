//! PDF/A-2 6.2.5, the extended graphics state: device-dependent settings a conforming file may not
//! rely on.
//!
//! - no transfer function: no `/TR` entry, and `/TR2` only as `/Default` (ISO 32000-1 10.4);
//! - no `/HTP` (halftone phase) entry, a PDF 1.2 key ISO 32000-1 no longer defines;
//! - halftones of type 1 or 5 only, without a `/HalftoneName` (ISO 32000-1 10.5.5); the halftones
//!   a type 5 halftone holds for its colorants are checked the same way;
//! - a halftone's `/TransferFunction` only where one is needed: a type 5 halftone's component for
//!   a non-primary colorant has one, and no other halftone does. The primary colour components
//!   are those of the standard device spaces: Gray; Red, Green and Blue; Cyan, Magenta, Yellow
//!   and Black (ISO 32000-1 10.5.5.6; ISO 32000-2 10.6.5.6). The `Default` component is left as
//!   it is (ISO 32000-1 Table 130);
//! - a rendering intent (`/RI`) that is one of the four of ISO 32000-1 8.6.5.8, Table 70.
//!
//! Graphics state dictionaries are found as the `/ExtGState` entries of resource dictionaries
//! (`/Resources` of pages, forms, Type 3 fonts, patterns and annotation appearances, and an
//! interactive form's `/DR`), and as the `/ExtGState` of a shading pattern (ISO 32000-1 8.7.4.3,
//! Table 76). They are looked for in what the document reaches from its trailer, following
//! references, but not into private application data (`/PieceInfo`), so an object only that data
//! refers to isn't read. A map shared by reference is read once.

use std::collections::{BTreeSet, HashSet, VecDeque};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// How deep a direct object is followed: as deep as the parser nests them.
const MAX_DEPTH: u8 = 128;
/// How many indirect reference hops a dictionary lookup follows, matching `Document::resolve`.
const MAX_REF_HOPS: u8 = 32;
/// The rendering intents of ISO 32000-1 Table 70.
const INTENTS: [&[u8]; 4] = [b"AbsoluteColorimetric", b"RelativeColorimetric", b"Saturation", b"Perceptual"];
/// The entries of a type 5 halftone that aren't colorant halftones.
const HALFTONE_KEYS: [&[u8]; 4] = [b"Type", b"HalftoneType", b"HalftoneName", b"TransferFunction"];
/// The primary colour components of DeviceGray, DeviceRGB and DeviceCMYK, whose halftones take
/// the graphics state's transfer function.
const PRIMARY: [&[u8]; 8] = [b"Gray", b"Red", b"Green", b"Blue", b"Cyan", b"Magenta", b"Yellow", b"Black"];
/// The entries that hold resource dictionaries.
const RESOURCES: [&[u8]; 2] = [b"Resources", b"DR"];

/// What the graphics states of a document break of 6.2.5.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Findings {
    /// A graphics state has a `/TR` entry.
    pub(crate) transfer: bool,
    /// A graphics state has a `/TR2` other than `/Default`.
    pub(crate) transfer2: bool,
    /// A graphics state has an `/HTP`.
    pub(crate) phase: bool,
    /// Halftone types other than 1 and 5, in order.
    pub(crate) halftone_types: BTreeSet<i64>,
    /// A halftone's `/HalftoneType` is missing or isn't an integer.
    pub(crate) untyped: bool,
    /// A halftone has a `/HalftoneName`.
    pub(crate) halftone_name: bool,
    /// A halftone that may not have a `/TransferFunction` has one.
    pub(crate) halftone_transfer: bool,
    /// Non-primary colorants whose halftone has no `/TransferFunction`, in name order.
    pub(crate) missing_transfer: BTreeSet<String>,
    /// Rendering intents outside Table 70, in name order.
    pub(crate) intents: BTreeSet<String>,
    /// A rendering intent isn't a name.
    pub(crate) intent_not_name: bool,
}

/// The 6.2.5 findings over what `doc` reaches from its trailer.
pub(crate) fn check(doc: &Document) -> Findings {
    scan(doc).findings
}

/// How many objects [`check`] reads.
#[cfg(test)]
pub(crate) fn reached(doc: &Document) -> usize {
    scan(doc).reached.len()
}

fn scan(doc: &Document) -> Scan<'_> {
    let mut scan = Scan {
        doc,
        read_resources: HashSet::new(),
        read_maps: HashSet::new(),
        read_states: HashSet::new(),
        reached: HashSet::new(),
        queue: VecDeque::new(),
        findings: Findings::default(),
    };
    scan.walk(&Object::Dict(doc.trailer().clone()), 0);
    // References are followed from a queue, not by recursion: a chain of them can be as long as
    // the file.
    while let Some(r) = scan.queue.pop_front() {
        let o = doc.get(r);
        scan.walk(&o, 0);
    }
    scan
}

struct Scan<'a> {
    doc: &'a Document,
    /// Resource dictionaries, `/ExtGState` maps and graphics states read so far, by role and
    /// object number.
    read_resources: HashSet<u32>,
    read_maps: HashSet<u32>,
    read_states: HashSet<u32>,
    /// Object numbers reached so far, queued or walked. All sets go by number, as
    /// [`Document::get`] does: references differing only in generation reach the same object.
    reached: HashSet<u32>,
    queue: VecDeque<ObjRef>,
    findings: Findings,
}

/// Resolves `o` through up to [`MAX_REF_HOPS`] indirect references for a dictionary read at most
/// once in its role. Reference hops are marked in `read` only once a non-reference target is
/// reached, so an aborted cycle or over-deep chain never hides a shorter path to a shared tail.
fn resolve_once(doc: &Document, read: &mut HashSet<u32>, o: &Object) -> Option<std::sync::Arc<Object>> {
    let Object::Ref(mut r) = *o else {
        return Some(std::sync::Arc::new(o.clone()));
    };
    if read.contains(&r.num) {
        return None;
    }
    let mut hops = Vec::with_capacity(4);
    hops.push(r.num);
    let mut held = doc.get(r);
    for _ in 0..MAX_REF_HOPS {
        let Object::Ref(next) = *held else {
            read.extend(hops);
            return Some(held);
        };
        if read.contains(&next.num) || hops.contains(&next.num) {
            return None;
        }
        hops.push(next.num);
        r = next;
        held = doc.get(r);
    }
    if matches!(*held, Object::Ref(_)) {
        None
    } else {
        read.extend(hops);
        Some(held)
    }
}

impl Scan<'_> {
    fn walk(&mut self, o: &Object, depth: u8) {
        if depth > MAX_DEPTH {
            return;
        }
        let d = match o {
            Object::Ref(r) => {
                if self.reached.insert(r.num) {
                    self.queue.push_back(*r);
                }
                return;
            }
            Object::Array(a) => {
                for v in a {
                    self.walk(v, depth.saturating_add(1));
                }
                return;
            }
            Object::Dict(d) => d,
            Object::Stream(s) => &s.dict,
            _ => return,
        };
        self.holder(d);
        for (k, v) in d.iter() {
            if k.as_slice() != b"PieceInfo" {
                self.walk(v, depth.saturating_add(1));
            }
        }
    }

    /// A dictionary that holds graphics states: through its resources, or as a shading pattern
    /// (`/PatternType 2`, ISO 32000-1 Table 76).
    fn holder(&mut self, d: &Dict) {
        for key in RESOURCES {
            if let Some(resources) = d.get(key) {
                self.resources(resources);
            }
        }
        if self.present(d, b"PatternType").is_some_and(|t| matches!(*t, Object::Int(2)))
            && let Some(gs) = d.get(b"ExtGState")
            && let Some(gs) = resolve_once(self.doc, &mut self.read_states, gs)
            && let Some(gs) = gs.as_dict()
        {
            self.gstate(gs);
        }
    }

    fn resources(&mut self, resources: &Object) {
        let Some(resources) = resolve_once(self.doc, &mut self.read_resources, resources) else { return };
        let Some(states) = resources.as_dict().and_then(|r| r.get(b"ExtGState")) else { return };
        let Some(states) = resolve_once(self.doc, &mut self.read_maps, states) else { return };
        let Some(states) = states.as_dict() else { return };
        for (_, gs) in states.iter() {
            if let Some(gs) = resolve_once(self.doc, &mut self.read_states, gs)
                && let Some(gs) = gs.as_dict()
            {
                self.gstate(gs);
            }
        }
    }

    /// `key` of `d`, resolved, unless it is missing, null (ISO 32000-1 7.3.9), or an unresolved
    /// reference chain.
    fn present(&self, d: &Dict, key: &[u8]) -> Option<std::sync::Arc<Object>> {
        d.get(key).map(|v| self.doc.resolve(v)).filter(|v| !matches!(**v, Object::Null | Object::Ref(_)))
    }

    fn gstate(&mut self, gs: &Dict) {
        if self.present(gs, b"TR").is_some() {
            self.findings.transfer = true;
        }
        if self.present(gs, b"TR2").is_some_and(|f| f.as_name() != Some(b"Default")) {
            self.findings.transfer2 = true;
        }
        if self.present(gs, b"HTP").is_some() {
            self.findings.phase = true;
        }
        if let Some(ht) = self.present(gs, b"HT") {
            match ht.as_dict() {
                Some(ht) => self.halftone(ht, None),
                None if ht.as_name() != Some(b"Default") => self.findings.untyped = true,
                None => {}
            }
        }
        if let Some(ri) = self.present(gs, b"RI") {
            match ri.as_name() {
                Some(ri) if !INTENTS.contains(&ri) => {
                    self.findings.intents.insert(String::from_utf8_lossy(ri).into_owned());
                }
                Some(_) => {}
                None => self.findings.intent_not_name = true,
            }
        }
    }

    /// A halftone dictionary or stream: the graphics state's own (`colorant` is `None`), or the
    /// one a type 5 halftone holds for `colorant`.
    fn halftone(&mut self, ht: &Dict, colorant: Option<&[u8]>) {
        // An integer (ISO 32000-1 Table 128), not a real of the same value.
        let kind = self.present(ht, b"HalftoneType").and_then(|t| if let Object::Int(t) = *t { Some(t) } else { None });
        match kind {
            None => self.findings.untyped = true,
            Some(1 | 5) => {}
            Some(kind) => {
                self.findings.halftone_types.insert(kind);
            }
        }
        if self.present(ht, b"HalftoneName").is_some() {
            self.findings.halftone_name = true;
        }
        let transfer = self.present(ht, b"TransferFunction").is_some();
        match colorant {
            Some(b"Default") => {}
            Some(c) if !PRIMARY.contains(&c) => {
                if !transfer {
                    self.findings.missing_transfer.insert(String::from_utf8_lossy(c).into_owned());
                }
            }
            _ => self.findings.halftone_transfer |= transfer,
        }
        if colorant.is_none() && kind == Some(5) {
            for (k, _) in ht.iter() {
                if HALFTONE_KEYS.contains(&k.as_slice()) {
                    continue;
                }
                let Some(entry) = self.present(ht, k) else { continue };
                match entry.as_dict() {
                    Some(component) => self.halftone(component, Some(k)),
                    None => {
                        self.findings.untyped = true;
                        if k != b"Default" && !PRIMARY.contains(&k.as_slice()) {
                            self.findings.missing_transfer.insert(String::from_utf8_lossy(k).into_owned());
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use pdfcraft_cos::Document;

    use super::{Findings, check};
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
    fn type5_halftone_transfer_function_is_not_treated_as_colorant() {
        let doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources << /ExtGState << /G << /HT << /HalftoneType 5 /Default 5 0 R /Cyan 5 0 R /TransferFunction 4 0 R >> >> >> >> >>",
            "<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>",
            "<< /HalftoneType 1 /Frequency 60 /Angle 45 /SpotFunction /Round >>",
        ]);
        let found = check(&doc);
        assert_eq!(found, Findings { halftone_transfer: true, ..Findings::default() });
        let messages: Vec<String> = verify(&doc, Level::A2b).into_iter().filter(|i| i.clause == "6.2.5").map(|i| i.message).collect();
        assert_eq!(messages, ["A halftone has a transfer function, which only a non-primary colorant's halftone may have"]);
    }

    #[test]
    fn type5_halftone_non_dict_colorant_reports_untyped_and_missing_transfer() {
        let doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources << /ExtGState << /G << /HT << /HalftoneType 5 /Default 4 0 R /Spot /Default /NullColorant null >> >> >> >> >>",
            "<< /HalftoneType 1 /Frequency 60 /Angle 45 /SpotFunction /Round >>",
        ]);
        let found = check(&doc);
        assert_eq!(found, Findings { untyped: true, missing_transfer: BTreeSet::from(["Spot".to_string()]), ..Findings::default() });
    }

    #[test]
    fn non_default_non_dict_halftone_reports_untyped() {
        for ht in ["/Round", "42", "[1 2]", "(Screen)"] {
            let page = format!("<< /Type /Page /Parent 2 0 R /Resources << /ExtGState << /G << /HT {ht} >> >> >> >>");
            let doc = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>", &page]);
            assert_eq!(check(&doc), Findings { untyped: true, ..Findings::default() }, "{ht}");
            let messages: Vec<String> = verify(&doc, Level::A2b).into_iter().filter(|i| i.clause == "6.2.5").map(|i| i.message).collect();
            assert_eq!(messages, ["A halftone's HalftoneType is missing or isn't an integer"], "{ht}");
        }
        for allowed in ["/Default", "null"] {
            let page = format!("<< /Type /Page /Parent 2 0 R /Resources << /ExtGState << /G << /HT {allowed} >> >> >> >>");
            let doc = build(&["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>", &page]);
            assert_eq!(check(&doc), Findings::default(), "{allowed}");
        }
    }

    #[test]
    fn chained_indirect_references_and_cycles_resolve_across_gstate() {
        // Multi-hop chains to conformant values (/Default, null, /Perceptual, HalftoneType 1) must not trigger false positives.
        let conformant = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources 4 0 R >>",
            "5 0 R",
            "<< /ExtGState 6 0 R /Pattern << /P << /PatternType 18 0 R /Shading << /ShadingType 2 >> /ExtGState 8 0 R >> >> >>",
            "7 0 R",
            "<< /G 8 0 R /Cycle 22 0 R >>",
            "9 0 R",
            "<< /TR 10 0 R /TR2 12 0 R /HT 14 0 R /RI 16 0 R /HTP 22 0 R >>",
            "11 0 R",
            "null",
            "13 0 R",
            "/Default",
            "15 0 R",
            "<< /HalftoneType 20 0 R /HalftoneName 10 0 R /TransferFunction 10 0 R >>",
            "17 0 R",
            "/Perceptual",
            "19 0 R",
            "2",
            "21 0 R",
            "1",
            "23 0 R",
            "22 0 R",
        ]);
        assert_eq!(check(&conformant), Findings::default());

        // Multi-hop chains to forbidden settings and shading pattern ExtGState are still caught,
        // while non-shading /PatternType values (1, null, real 2.0) do not treat /ExtGState as a single state.
        let violations = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources 4 0 R >>",
            "5 0 R",
            "<< /ExtGState 6 0 R /Pattern << /S << /PatternType 14 0 R /Shading << /ShadingType 2 >> /ExtGState 10 0 R >> /T << /PatternType 1 /ExtGState << /TR2 /Identity >> >> /N << /PatternType null /ExtGState << /HTP [0 1] >> >> /R << /PatternType 2.0 /ExtGState << /RI /Custom >> >> >> >>",
            "7 0 R",
            "<< /G 8 0 R >>",
            "9 0 R",
            "<< /HT 12 0 R >>",
            "11 0 R",
            "<< /TR /Identity >>",
            "13 0 R",
            "/Round",
            "15 0 R",
            "2",
        ]);
        assert_eq!(check(&violations), Findings { transfer: true, untyped: true, ..Findings::default() });
    }

    #[test]
    fn aborted_deep_ref_chain_and_shared_role_dict_do_not_hide_findings() {
        // Objects 4..=38 form a 35-hop chain to object 39 (`<< /TR /Identity >>`), exceeding MAX_REF_HOPS
        // from `/Long 4 0 R`, while `/Short 37 0 R` reaches object 39 in two hops.
        let mut objs = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Resources << /ExtGState << /Long 4 0 R /Short 37 0 R >> >> >>".to_owned(),
        ];
        for next in 5..=39 {
            objs.push(format!("{next} 0 R"));
        }
        objs.push("<< /TR /Identity >>".to_owned());
        let refs: Vec<&str> = objs.iter().map(String::as_str).collect();
        assert_eq!(check(&build(&refs)), Findings { transfer: true, ..Findings::default() });

        // One dictionary reused as /Resources, /ExtGState map, and graphics state is still checked in each role.
        let reused = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources 4 0 R >>",
            "<< /ExtGState 4 0 R /G 4 0 R /HTP [0 1] >>",
        ]);
        assert_eq!(check(&reused), Findings { phase: true, ..Findings::default() });
    }
}
