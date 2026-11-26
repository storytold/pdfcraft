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

use std::collections::{BTreeSet, HashMap, HashSet};

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, Stream};

/// How deep a direct object is followed (arrays and dictionaries inside one another): as deep as
/// the parser nests them.
const MAX_DEPTH: u8 = 128;
/// How deep two definitions are compared, references followed.
const MAX_COMPARE_DEPTH: u8 = 32;
/// How many object pairs the comparisons of one document visit in all. Past it, a comparison
/// claims no difference.
const MAX_COMPARE_STEPS: u32 = 1 << 20;
/// The decoded bytes the stream comparisons of one document may spend in all.
const MAX_COMPARE_BYTES: usize = 64 << 20;
/// The stream entries that say how the data is stored, not what it is.
const ENCODING: [&[u8]; 3] = [b"Length", b"Filter", b"DecodeParms"];
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
    let compare = Compare { doc, equal: HashSet::new(), steps: MAX_COMPARE_STEPS, budget: DecodeBudget::new(MAX_COMPARE_BYTES, 0) };
    let mut scan = Scan { doc, compare, separations: HashMap::new(), findings: Findings::default() };
    for (_, o) in objs {
        scan.walk(o, 0);
    }
    scan.findings
}

struct Scan<'a> {
    doc: &'a Document,
    compare: Compare<'a>,
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
                match a.first().map(|v| self.doc.resolve(v)).as_deref().and_then(Object::as_name) {
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
                if !self.compare.same(alt, &this.0, 0) || !self.compare.same(fun, &this.1, 0) {
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
            // Each entry is a Separation space for the colorant it is keyed by (ISO 32000-1 8.6.6.5,
            // Table 71); a null entry, or one referring to no object, defines nothing (7.3.9).
            let defined = colorants.and_then(|c| c.get(n)).is_some_and(|e| self.separation_named(e, n));
            if spot && !defined {
                self.findings.undefined.insert(String::from_utf8_lossy(n).into_owned());
            }
        }
    }

    /// Whether `o` is `[/Separation name alternateSpace tintTransform]` for the colorant `name`,
    /// with a non-null alternate space and tint transform (ISO 32000-1 8.6.6.4 and 7.3.9).
    fn separation_named(&self, o: &Object, name: &[u8]) -> bool {
        let o = self.doc.resolve(o);
        let Some([kind, n, alt, tint, ..]) = o.as_array().map(Vec::as_slice) else { return false };
        self.doc.resolve(kind).as_name() == Some(b"Separation")
            && self.doc.resolve(n).as_name() == Some(name)
            && !matches!(&*self.doc.resolve(alt), Object::Null)
            && !matches!(&*self.doc.resolve(tint), Object::Null)
    }
}

/// Whether integer `i` and real `r` have the same numeric value (ISO 32000-1 7.3.3).
fn same_number(i: i64, r: f64) -> bool {
    r.is_finite() && r.fract() == 0.0 && (i as f64) == r && (r as i64) == i && (i as f64 as i128) == i128::from(i)
}

/// Compares definitions, remembering which reference pairs it found equal, so a structure shared
/// by many definitions is compared once, within [`MAX_COMPARE_STEPS`] and [`MAX_COMPARE_BYTES`].
struct Compare<'a> {
    doc: &'a Document,
    /// Reference pairs compared equal. A depth cut-off only ever answers "different", so an
    /// equal answer holds at any depth.
    equal: HashSet<(ObjRef, ObjRef)>,
    steps: u32,
    budget: DecodeBudget,
}

impl Compare<'_> {
    /// Whether `v` is present (not `null` or a reference to no object, ISO 32000-1 7.3.7 and 7.3.9).
    fn is_present(&self, v: &Object) -> bool {
        !matches!(&*self.doc.resolve(v), Object::Null)
    }

    /// Whether `a` and `b` are the same object, wherever their parts are stored: references are
    /// followed (to [`MAX_COMPARE_DEPTH`]), so two identical streams under different object
    /// numbers are the same, and streams compare by their decoded data. Once the steps or the
    /// decode allowance run out, nothing more is called different.
    fn same(&mut self, a: &Object, b: &Object, depth: u8) -> bool {
        let pair = match (a, b) {
            (Object::Ref(x), Object::Ref(y)) if x == y || self.equal.contains(&(*x, *y)) || self.equal.contains(&(*y, *x)) => return true,
            (Object::Ref(x), Object::Ref(y)) => Some((*x, *y)),
            _ => None,
        };
        let Some(steps) = self.steps.checked_sub(1) else { return true };
        self.steps = steps;
        if depth > MAX_COMPARE_DEPTH {
            return false;
        }
        let (a, b) = (self.doc.resolve(a), self.doc.resolve(b));
        let deeper = depth.saturating_add(1);
        let same = match (&*a, &*b) {
            (Object::Array(x), Object::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| self.same(p, q, deeper)),
            (Object::Dict(x), Object::Dict(y)) => self.same_dict(x, y, &[], deeper),
            (Object::Stream(x), Object::Stream(y)) => self.same_data(x, y, deeper) && self.same_dict(&x.dict, &y.dict, &ENCODING, deeper),
            (Object::Int(x), Object::Real(y)) | (Object::Real(y), Object::Int(x)) => same_number(*x, *y),
            (x, y) => x == y,
        };
        if same && let Some(pair) = pair {
            self.equal.insert(pair);
        }
        same
    }

    /// Whether two dictionaries hold the same non-null entries (ISO 32000-1 7.3.7), those keyed
    /// in `skip` aside.
    fn same_dict(&mut self, a: &Dict, b: &Dict, skip: &[&[u8]], depth: u8) -> bool {
        let kept = |c: &Self, d: &Dict| d.iter().filter(|(k, v)| !skip.contains(&k.as_slice()) && c.is_present(v)).count();
        if kept(self, a) != kept(self, b) {
            return false;
        }
        a.iter()
            .filter(|(k, v)| !skip.contains(&k.as_slice()) && self.is_present(v))
            .collect::<Vec<_>>()
            .into_iter()
            .all(|(k, v)| b.get(k).filter(|w| self.is_present(w)).is_some_and(|w| self.same(v, w, depth)))
    }

    /// Whether two streams hold the same data, however each is encoded. Data that can't be
    /// decoded within the allowance is not called different.
    fn same_data(&mut self, a: &Stream, b: &Stream, depth: u8) -> bool {
        let stored_alike = |c: &mut Self, k: &[u8]| match (a.dict.get(k).filter(|v| c.is_present(v)), b.dict.get(k).filter(|v| c.is_present(v))) {
            (None, None) => true,
            (Some(x), Some(y)) => c.same(x, y, depth),
            _ => false,
        };
        if a.raw == b.raw && stored_alike(self, b"Filter") && stored_alike(self, b"DecodeParms") {
            return true;
        }
        match (self.budget.decode(a), self.budget.decode(b)) {
            (Ok(x), Ok(y)) => x == y,
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use pdfcraft_cos::{Dict, Document, Object};

    use super::{Findings, check, same_number};
    use crate::{Level, objects, verify};

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

    fn spot_findings(spaces: &str, extra: &[&str]) -> Findings {
        let page = format!("<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << {spaces} >> >> >>");
        let mut objs = vec![
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            &page,
            "<< /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [1 0 0] /N 1 >>",
            "<< /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [0 1 0] /N 1 >>",
            "/Separation",
            "/DeviceN",
        ];
        objs.extend_from_slice(extra);
        let doc = build(&objs);
        let verified: Vec<String> = verify(&doc, Level::A2b).into_iter().filter(|i| i.clause == "6.2.4.4").map(|i| i.message).collect();
        let found = check(&doc, &objects(&doc));
        assert_eq!(verified.len(), found.differing.len() + found.undefined.len());
        found
    }

    #[test]
    fn resolves_indirect_separation_and_devicen_family_names() {
        // Object 6 is `/Separation` and object 7 is `/DeviceN`.
        let found = spot_findings("/A [6 0 R /Gold /DeviceRGB 4 0 R] /B [/Separation /Gold /DeviceRGB 5 0 R]", &[]);
        assert_eq!(found.differing, BTreeSet::from(["Gold".to_string()]));

        let found = spot_findings("/A [7 0 R [/Foil] /DeviceRGB 4 0 R << /Colorants << >> >>]", &[]);
        assert_eq!(found.undefined, BTreeSet::from(["Foil".to_string()]));

        // A `/Colorants` entry whose `/Separation` family name is indirect defines the colorant.
        let found = spot_findings("/A [7 0 R [/Foil] /DeviceRGB 4 0 R << /Colorants << /Foil [6 0 R /Foil /DeviceRGB 4 0 R] >> >>]", &[]);
        assert_eq!(found, Findings::default());
    }

    #[test]
    fn rejects_incomplete_or_null_operand_colorant_separations() {
        for bad in [
            "[/Separation /Spot]",
            "[/Separation /Spot /DeviceRGB]",
            "[/Separation /Spot null 4 0 R]",
            "[/Separation /Spot /DeviceRGB null]",
            "[/Separation /Spot 99 0 R 4 0 R]",
            "[/Separation /Spot /DeviceRGB 99 0 R]",
        ] {
            let spaces = format!("/A [/DeviceN [/Spot] /DeviceRGB 4 0 R << /Colorants << /Spot {bad} >> >>]");
            let found = spot_findings(&spaces, &[]);
            assert_eq!(found.undefined, BTreeSet::from(["Spot".to_string()]), "{bad}");
        }
        let valid = "/A [/DeviceN [/Spot] /DeviceRGB 4 0 R << /Colorants << /Spot [/Separation /Spot /DeviceRGB 4 0 R] >> >>]";
        assert_eq!(spot_findings(valid, &[]), Findings::default());
    }

    #[test]
    fn ignores_null_dictionary_entries_and_matches_int_real_numbers() {
        let extra = [
            "<< /FunctionType 2 /Domain [0.0 1.0] /C0 [1.0 1.0 1.0] /C1 [1.0 0.0 0.0] /N 1.0 /Range null /Encode 99 0 R >>",
            "<< /FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1] /Length 15 >>\nstream\n{ dup dup pop }\nendstream",
            "<< /FunctionType 4 /Domain [0.0 1.0] /Range [0.0 1.0 0.0 1.0 0.0 1.0] /Filter null /DecodeParms null /Length 15 >>\nstream\n{ dup dup pop }\nendstream",
            "<< /FunctionType 2 /Domain [0 1] /C0 [1 1 1] /C1 [1 0.5 0] /N 1 >>",
        ];
        // Object 8 has float equivalents of object 4's integers plus null/dangling optional keys.
        let found = spot_findings("/A [/Separation /Red /DeviceRGB 4 0 R] /B [/Separation /Red /DeviceRGB 8 0 R]", &extra);
        assert_eq!(found, Findings::default());

        // Object 9 and object 10 are identical Type 4 streams where object 10 has `/Filter null /DecodeParms null` and float domain/range.
        let found = spot_findings("/A [/Separation /Red /DeviceRGB 9 0 R] /B [/Separation /Red /DeviceRGB 10 0 R]", &extra);
        assert_eq!(found, Findings::default());

        // Genuinely different numeric values still report differing.
        let found = spot_findings("/A [/Separation /Red /DeviceRGB 8 0 R] /B [/Separation /Red /DeviceRGB 11 0 R]", &extra);
        assert_eq!(found.differing, BTreeSet::from(["Red".to_string()]));

        // Precision-losing large integers do not falsely match rounded floats.
        let mut doc = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R >>",
        ]);
        let mut fn_int = Dict::new();
        fn_int.set(b"N".to_vec(), Object::Int(i64::MAX));
        let mut fn_real = Dict::new();
        fn_real.set(b"N".to_vec(), Object::Real(i64::MAX as f64));
        let r_int = doc.add(Object::Dict(fn_int));
        let r_real = doc.add(Object::Dict(fn_real));
        let sep_a = Object::Array(vec![Object::name("Separation"), Object::name("Big"), Object::name("DeviceGray"), Object::Ref(r_int)]);
        let sep_b = Object::Array(vec![Object::name("Separation"), Object::name("Big"), Object::name("DeviceGray"), Object::Ref(r_real)]);
        doc.add(sep_a);
        doc.add(sep_b);
        let found = check(&doc, &objects(&doc));
        assert_eq!(found.differing, BTreeSet::from(["Big".to_string()]));

        assert!(same_number(0, -0.0));
        assert!(same_number(i64::MIN, i64::MIN as f64));
        assert!(!same_number(i64::MIN + 1, (i64::MIN + 1) as f64));
        assert!(!same_number(0, f64::NAN));
        assert!(!same_number(0, f64::INFINITY));
        assert!(!same_number(0, f64::NEG_INFINITY));
    }
}
