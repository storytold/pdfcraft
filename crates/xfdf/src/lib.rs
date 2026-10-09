//! Comment and form data exchange (execution plan M5.6, M6.7). Layer L4.
//!
//! - **XFDF** (ISO 19444-1, Acrobat's "Export all to data file" / "Import comments", and form
//!   data): comments with their geometry, colours, flags, authors, dates, replies, pop-ups,
//!   ink, quadrilaterals and line ends; field values, nested by name.
//! - **FDF** (ISO 32000-2 §12.7.8): fields and comments in PDF syntax.
//! - Form data as **XML**, **CSV** and **tab-delimited text** (Acrobat's Export data formats).
//!
//! Import merges: a comment whose name (`/NM`) already exists on its page replaces it; field
//! values go through the form's own checks (formats, validation) and recalculate.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString};
use pdfcraft_forms::{FieldKind, FieldValue};

#[cfg(test)]
mod tests;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DataError {
    #[error("this file isn't XFDF, FDF, XML or text form data")]
    UnknownFormat,
    #[error("the file can't be read: {0}")]
    Malformed(String),
    #[error("nothing in the file matches this document")]
    NothingImported,
    #[error(transparent)]
    Cos(#[from] pdfcraft_cos::CosError),
}

/// What export writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Xfdf,
    Fdf,
    Xml,
    Csv,
    Txt,
}

impl Format {
    pub fn from_extension(ext: &str) -> Option<Format> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "xfdf" => Format::Xfdf,
            "fdf" => Format::Fdf,
            "xml" => Format::Xml,
            "csv" => Format::Csv,
            "txt" | "tab" | "tsv" => Format::Txt,
            _ => return None,
        })
    }
}

/// What import did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub comments: usize,
    pub fields: usize,
    /// Field values the form refused (format, validation, unknown options).
    pub rejected: Vec<String>,
}

// ── names ───────────────────────────────────────────────────────────────────────────────────

const SUBTYPES: [(&str, &str); 17] = [
    ("Text", "text"),
    ("FreeText", "freetext"),
    ("Line", "line"),
    ("Square", "square"),
    ("Circle", "circle"),
    ("Polygon", "polygon"),
    ("PolyLine", "polyline"),
    ("Highlight", "highlight"),
    ("Underline", "underline"),
    ("Squiggly", "squiggly"),
    ("StrikeOut", "strikeout"),
    ("Stamp", "stamp"),
    ("Caret", "caret"),
    ("Ink", "ink"),
    ("FileAttachment", "fileattachment"),
    ("Sound", "sound"),
    ("Redact", "redact"),
];

const FLAGS: [(i64, &str); 9] = [
    (1, "invisible"),
    (2, "hidden"),
    (4, "print"),
    (8, "nozoom"),
    (16, "norotate"),
    (32, "noview"),
    (64, "readonly"),
    (128, "locked"),
    (256, "togglenoview"),
];

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\r' => o.push_str("&#13;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => {}
            c => o.push(c),
        }
    }
    o
}

fn n(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

fn hex(c: &[f64]) -> Option<String> {
    let rgb = match c.len() {
        1 => [c[0]; 3],
        3 => [c[0], c[1], c[2]],
        4 => [(1.0 - c[0]) * (1.0 - c[3]), (1.0 - c[1]) * (1.0 - c[3]), (1.0 - c[2]) * (1.0 - c[3])],
        _ => return None,
    };
    Some(format!(
        "#{:02X}{:02X}{:02X}",
        (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (rgb[2].clamp(0.0, 1.0) * 255.0).round() as u8
    ))
}

fn parse_hex(s: &str) -> Option<[f64; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let p = |i: usize| h.get(i..i + 2).and_then(|s| u8::from_str_radix(s, 16).ok()).map(|v| f64::from(v) / 255.0);
    Some([p(0)?, p(2)?, p(4)?])
}

fn nums_of(doc: &Document, o: Option<&Object>) -> Vec<f64> {
    o.map(|o| doc.resolve(o)).and_then(|a| a.as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect())).unwrap_or_default()
}

fn text_of(doc: &Document, d: &Dict, k: &[u8]) -> Option<String> {
    d.get(k).and_then(|o| match &*doc.resolve(o) {
        Object::String(s) => Some(s.to_text()),
        Object::Name(n) => Some(String::from_utf8_lossy(n).into_owned()),
        _ => None,
    })
}

fn csv(list: &str) -> Vec<f64> {
    list.split([',', ';', ' ']).filter(|t| !t.is_empty()).filter_map(|t| t.trim().parse().ok()).collect()
}

fn arr(v: &[f64]) -> Object {
    Object::Array(v.iter().map(|x| Object::Real(*x)).collect())
}

// ── export ──────────────────────────────────────────────────────────────────────────────────

/// Every comment as an XFDF element (one line per comment, replies included).
fn xfdf_annots(doc: &Document, out: &mut String) {
    let pages = pdfcraft_model::pages(doc);
    // NM of every annotation, for inreplyto.
    let mut names: HashMap<ObjRef, String> = HashMap::new();
    for p in &pages {
        for a in p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default() {
            if let Some(r) = a.as_ref()
                && let Some(nm) = doc.get(r).as_dict().and_then(|d| text_of(doc, d, b"NM"))
            {
                names.insert(r, nm);
            }
        }
    }
    for (pi, p) in pages.iter().enumerate() {
        for a in p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default() {
            let Some(d) = doc.resolve(&a).as_dict().cloned() else { continue };
            let Some(sub) = d.name(b"Subtype") else { continue };
            let Some((_, tag)) = SUBTYPES.iter().find(|(s, _)| s.as_bytes() == sub) else { continue };
            let mut attrs = format!(" page=\"{pi}\"");
            let rect = nums_of(doc, d.get(b"Rect"));
            if rect.len() == 4 {
                let _ = write!(
                    attrs,
                    " rect=\"{},{},{},{}\"",
                    n(rect[0].min(rect[2])),
                    n(rect[1].min(rect[3])),
                    n(rect[0].max(rect[2])),
                    n(rect[1].max(rect[3]))
                );
            }
            for (key, attr) in [
                (&b"NM"[..], "name"),
                (b"T", "title"),
                (b"Subj", "subject"),
                (b"M", "date"),
                (b"CreationDate", "creationdate"),
                (b"RT", "replyType"),
                (b"State", "state"),
                (b"StateModel", "statemodel"),
                (b"Name", "icon"),
            ] {
                if let Some(v) = text_of(doc, &d, key) {
                    let _ = write!(attrs, " {attr}=\"{}\"", esc(&v));
                }
            }
            let flags = d.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
            let fl: Vec<&str> = FLAGS.iter().filter(|(b, _)| flags & b != 0).map(|(_, s)| *s).collect();
            if !fl.is_empty() {
                let _ = write!(attrs, " flags=\"{}\"", fl.join(","));
            }
            if let Some(c) = hex(&nums_of(doc, d.get(b"C"))) {
                let _ = write!(attrs, " color=\"{c}\"");
            }
            if let Some(c) = hex(&nums_of(doc, d.get(b"IC"))) {
                let _ = write!(attrs, " interior-color=\"{c}\"");
            }
            if let Some(o) = d.get(b"CA").and_then(|o| doc.resolve(o).as_f64()) {
                let _ = write!(attrs, " opacity=\"{}\"", n(o));
            }
            if let Some(w) = d.get(b"BS").and_then(|b| doc.resolve(b).as_dict().and_then(|b| b.get(b"W").and_then(Object::as_f64))) {
                let _ = write!(attrs, " width=\"{}\"", n(w));
            }
            let quads = nums_of(doc, d.get(b"QuadPoints"));
            if !quads.is_empty() {
                let _ = write!(attrs, " coords=\"{}\"", quads.iter().map(|v| n(*v)).collect::<Vec<_>>().join(","));
            }
            let l = nums_of(doc, d.get(b"L"));
            if l.len() == 4 {
                let _ = write!(attrs, " start=\"{},{}\" end=\"{},{}\"", n(l[0]), n(l[1]), n(l[2]), n(l[3]));
            }
            if let Some(le) = d.get(b"LE").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned())
                && le.len() == 2
            {
                let name = |o: &Object| o.as_name().map(|x| String::from_utf8_lossy(x).into_owned()).unwrap_or_else(|| "None".into());
                let _ = write!(attrs, " head=\"{}\" tail=\"{}\"", name(&le[0]), name(&le[1]));
            }
            if let Some(irt) = d.get(b"IRT").and_then(Object::as_ref).and_then(|r| names.get(&r)) {
                let _ = write!(attrs, " inreplyto=\"{}\"", esc(irt));
            }
            let _ = write!(out, "<{tag}{attrs}>");
            if let Some(c) = text_of(doc, &d, b"Contents") {
                let _ = write!(out, "<contents>{}</contents>", esc(&c));
            }
            if let Some(da) = text_of(doc, &d, b"DA") {
                let _ = write!(out, "<defaultappearance>{}</defaultappearance>", esc(&da));
            }
            let ink = d.get(b"InkList").map(|o| doc.resolve(o)).and_then(|o| o.as_array().cloned()).unwrap_or_default();
            if !ink.is_empty() {
                out.push_str("<inklist>");
                for g in ink {
                    let v = nums_of(doc, Some(&g));
                    let pts: Vec<String> = v.as_chunks::<2>().0.iter().map(|p| format!("{},{}", n(p[0]), n(p[1]))).collect();
                    let _ = write!(out, "<gesture>{}</gesture>", pts.join(";"));
                }
                out.push_str("</inklist>");
            }
            if let Some(pd) = d.get(b"Popup").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()) {
                let r = nums_of(doc, pd.get(b"Rect"));
                if r.len() == 4 {
                    let open = matches!(pd.get(b"Open"), Some(Object::Bool(true)));
                    let _ = write!(
                        out,
                        "<popup page=\"{pi}\" rect=\"{},{},{},{}\" open=\"{}\"/>",
                        n(r[0]),
                        n(r[1]),
                        n(r[2]),
                        n(r[3]),
                        if open { "yes" } else { "no" }
                    );
                }
            }
            let _ = writeln!(out, "</{tag}>");
        }
    }
}

/// Field values as nested XFDF `<field>` elements.
fn xfdf_fields(doc: &Document, out: &mut String) {
    #[derive(Default)]
    struct Node {
        values: Vec<String>,
        kids: Vec<(String, Node)>,
    }
    let mut root = Node::default();
    for f in pdfcraft_forms::fields(doc) {
        if matches!(f.kind, FieldKind::PushButton | FieldKind::Signature) {
            continue;
        }
        let mut node = &mut root;
        for part in f.name.split('.') {
            let i = match node.kids.iter().position(|(k, _)| k == part) {
                Some(i) => i,
                None => {
                    node.kids.push((part.to_string(), Node::default()));
                    node.kids.len() - 1
                }
            };
            node = &mut node.kids[i].1;
        }
        node.values = match f.kind {
            FieldKind::CheckBox | FieldKind::Radio if f.value.is_empty() => vec!["Off".into()],
            _ => f.value.clone(),
        };
    }
    fn write_node(name: &str, node: &Node, out: &mut String) {
        let _ = write!(out, "<field name=\"{}\">", esc(name));
        for v in &node.values {
            let _ = write!(out, "<value>{}</value>", esc(v));
        }
        for (k, kid) in &node.kids {
            write_node(k, kid, out);
        }
        out.push_str("</field>\n");
    }
    for (k, kid) in &root.kids {
        write_node(k, kid, out);
    }
}

/// XFDF with the document's comments and/or field values.
pub fn export_xfdf(doc: &Document, comments: bool, fields: bool, file: &str) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xfdf xmlns=\"http://ns.adobe.com/xfdf/\" xml:space=\"preserve\">\n");
    if comments {
        out.push_str("<annots>\n");
        xfdf_annots(doc, &mut out);
        out.push_str("</annots>\n");
    }
    if fields {
        out.push_str("<fields>\n");
        xfdf_fields(doc, &mut out);
        out.push_str("</fields>\n");
    }
    let _ = writeln!(out, "<f href=\"{}\"/>", esc(file));
    out.push_str("</xfdf>\n");
    out
}

fn pdf_string(s: &str) -> Vec<u8> {
    let mut v = Vec::new();
    pdfcraft_cos::serialize(&Object::String(PdfString::text(s)), &mut v);
    v
}

/// FDF with field values and/or comments (appearances are not carried). Each comment is an
/// indirect object of the FDF, so a reply's `/IRT` can point at its parent's object there
/// (§12.7.8.3.1); nothing else refers back into the source document.
pub fn export_fdf(doc: &Document, comments: bool, fields: bool, file: &str) -> Vec<u8> {
    let mut body: Vec<u8> = b"<< /FDF << ".to_vec();
    // Comment dictionaries, written after the root as objects 2, 3, …
    let mut annots: Vec<Dict> = Vec::new();
    if fields {
        body.extend_from_slice(b"/Fields [");
        for f in pdfcraft_forms::fields(doc) {
            if matches!(f.kind, FieldKind::PushButton | FieldKind::Signature) {
                continue;
            }
            body.extend_from_slice(b"<< /T ");
            body.extend(pdf_string(&f.name));
            body.extend_from_slice(b" /V ");
            match f.kind {
                FieldKind::CheckBox | FieldKind::Radio => {
                    let v = f.value.first().cloned().unwrap_or_else(|| "Off".into());
                    pdfcraft_cos::serialize(&Object::Name(v.into_bytes()), &mut body);
                }
                FieldKind::List if f.value.len() > 1 => {
                    pdfcraft_cos::serialize(&Object::Array(f.value.iter().map(|v| Object::String(PdfString::text(v))).collect()), &mut body);
                }
                _ => body.extend(pdf_string(f.value.first().map(String::as_str).unwrap_or(""))),
            }
            body.extend_from_slice(b" >> ");
        }
        body.extend_from_slice(b"] ");
    }
    if comments {
        // Every exported comment with its source reference, numbered in export order.
        let mut picked: Vec<(Option<ObjRef>, Dict, usize)> = Vec::new();
        for (pi, p) in pdfcraft_model::pages(doc).iter().enumerate() {
            for a in p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default() {
                let Some(d) = doc.resolve(&a).as_dict().cloned() else { continue };
                if d.name(b"Subtype").is_some_and(|sub| SUBTYPES.iter().any(|(s, _)| s.as_bytes() == sub)) {
                    picked.push((a.as_ref(), d, pi));
                }
            }
        }
        let numbers: HashMap<ObjRef, u32> =
            picked.iter().enumerate().filter_map(|(i, (r, _, _))| Some(((*r)?, u32::try_from(i).ok()?.checked_add(2)?))).collect();
        for (_, mut d, pi) in picked {
            // A reply points at its parent's object in this FDF; a parent that isn't exported
            // leaves the reply unthreaded.
            let irt = d.get(b"IRT").and_then(Object::as_ref).and_then(|r| numbers.get(&r)).copied();
            // Only direct values travel: references into this file would dangle.
            for k in [&b"P"[..], b"AP", b"Popup", b"IRT", b"Parent", b"FS", b"Sound"] {
                d.remove(k);
            }
            let keys: Vec<Vec<u8>> = d.iter().filter(|(_, v)| matches!(v, Object::Ref(_))).map(|(k, _)| k.clone()).collect();
            for k in keys {
                let Some(o) = d.get(&k) else { continue };
                let v = (*doc.resolve(o)).clone();
                if matches!(v, Object::Stream(_)) {
                    d.remove(&k);
                } else {
                    d.set(k, v);
                }
            }
            if let Some(num) = irt {
                d.set(b"IRT".to_vec(), Object::Ref(ObjRef::new(num, 0)));
            }
            d.set(b"Page".to_vec(), Object::Int(pi as i64));
            annots.push(d);
        }
        body.extend_from_slice(b"/Annots [");
        for i in 0..annots.len() {
            let _ = write!(body, "{} 0 R ", i + 2);
        }
        body.extend_from_slice(b"] ");
    }
    body.extend_from_slice(b"/F ");
    body.extend(pdf_string(file));
    body.extend_from_slice(b" >> >>");
    let mut out = b"%FDF-1.2\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(annots.len() + 1);
    offsets.push(out.len());
    out.extend_from_slice(b"1 0 obj\n");
    out.extend(body);
    out.extend_from_slice(b"\nendobj\n");
    for (i, d) in annots.into_iter().enumerate() {
        offsets.push(out.len());
        let _ = writeln!(out, "{} 0 obj", i + 2);
        pdfcraft_cos::serialize(&Object::Dict(d), &mut out);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
    for o in offsets {
        let _ = writeln!(out, "{o:010} 00000 n ");
    }
    let _ = write!(out, "trailer\n<< /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n");
    out
}
/// The XML element name for a field name (letters, digits, `_`, `-`, `.`).
fn xml_name(name: &str) -> String {
    let mut s: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '_' | '-' | '.') { c } else { '_' }).collect();
    if !s.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') {
        s.insert(0, '_');
    }
    s
}

/// Form data as XML, CSV or tab-delimited text (Acrobat's Export data formats).
pub fn export_data(doc: &Document, format: Format) -> String {
    let fields: Vec<(String, String)> = pdfcraft_forms::fields(doc)
        .into_iter()
        .filter(|f| !matches!(f.kind, FieldKind::PushButton | FieldKind::Signature))
        .map(|f| {
            let v = match f.kind {
                FieldKind::CheckBox | FieldKind::Radio if f.value.is_empty() => "Off".to_string(),
                _ => f.value.join(", "),
            };
            (f.name, v)
        })
        .collect();
    match format {
        Format::Xml => {
            let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<fields xmlns:xfdf=\"http://ns.adobe.com/xfdf-transition/\">\n");
            for (k, v) in &fields {
                let tag = xml_name(k);
                let orig = if tag == *k { String::new() } else { format!(" xfdf:original=\"{}\"", esc(k)) };
                let _ = writeln!(out, "<{tag}{orig}>{}</{tag}>", esc(v));
            }
            out.push_str("</fields>\n");
            out
        }
        Format::Csv => {
            let q = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
            format!(
                "{}\n{}\n",
                fields.iter().map(|f| q(&f.0)).collect::<Vec<_>>().join(","),
                fields.iter().map(|f| q(&f.1)).collect::<Vec<_>>().join(",")
            )
        }
        _ => {
            let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
            format!(
                "{}\n{}\n",
                fields.iter().map(|f| clean(&f.0)).collect::<Vec<_>>().join("\t"),
                fields.iter().map(|f| clean(&f.1)).collect::<Vec<_>>().join("\t")
            )
        }
    }
}

// ── import ──────────────────────────────────────────────────────────────────────────────────

/// Set the values of fields named in `values` (full names), counting what took.
fn apply_values(doc: &mut Document, values: &[(String, Vec<String>)], report: &mut Report) {
    let all = pdfcraft_forms::fields(doc);
    for (name, vals) in values {
        let Some(f) = all.iter().find(|f| &f.name == name) else { continue };
        let first = vals.first().cloned().unwrap_or_default();
        let v = match f.kind {
            FieldKind::Text => FieldValue::Text(first),
            FieldKind::CheckBox => FieldValue::Check(!first.is_empty() && first != "Off"),
            FieldKind::Radio => FieldValue::Radio((!first.is_empty() && first != "Off").then_some(first)),
            // XML, CSV and text hold a multi-select list's values in one cell, joined by ", "
            // (as export_data writes them); split it unless the whole cell is one option.
            FieldKind::List if f.has(pdfcraft_forms::flags::MULTI_SELECT) && vals.len() == 1 && !f.options.iter().any(|(e, _)| *e == first) => {
                FieldValue::Choice(first.split(", ").filter(|v| !v.is_empty()).map(str::to_string).collect())
            }
            FieldKind::Combo | FieldKind::List => FieldValue::Choice(vals.iter().filter(|v| !v.is_empty()).cloned().collect()),
            FieldKind::PushButton | FieldKind::Signature => continue,
        };
        // Read-only fields still take imported data, as in Acrobat.
        let ro = f.read_only();
        if ro {
            let _ = doc.update_dict(f.obj, |d| d.set(b"Ff".to_vec(), Object::Int((f.flags & !pdfcraft_forms::flags::READ_ONLY) as i64)));
        }
        match pdfcraft_forms::set_value(doc, name, &v) {
            Ok(()) => report.fields += 1,
            Err(_) => report.rejected.push(name.clone()),
        }
        if ro {
            let _ = doc.update_dict(f.obj, |d| d.set(b"Ff".to_vec(), Object::Int(f.flags as i64)));
        }
    }
}

/// A new annotation dictionary from an XFDF element.
fn annot_from_xml(node: roxmltree::Node, subtype: &str, page_ref: ObjRef) -> Option<Dict> {
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Annot"));
    d.set(b"Subtype".to_vec(), Object::name(subtype));
    d.set(b"P".to_vec(), Object::Ref(page_ref));
    let rect = csv(node.attribute("rect")?);
    if rect.len() != 4 {
        return None;
    }
    d.set(b"Rect".to_vec(), arr(&rect));
    for (attr, key) in [
        ("name", &b"NM"[..]),
        ("title", b"T"),
        ("subject", b"Subj"),
        ("date", b"M"),
        ("creationdate", b"CreationDate"),
        ("state", b"State"),
        ("statemodel", b"StateModel"),
    ] {
        if let Some(v) = node.attribute(attr) {
            d.set(key.to_vec(), PdfString::text(v));
        }
    }
    if let Some(v) = node.attribute("replyType") {
        d.set(b"RT".to_vec(), Object::name(if v.eq_ignore_ascii_case("group") { "Group" } else { "R" }));
    }
    if let Some(v) = node.attribute("icon") {
        d.set(b"Name".to_vec(), Object::name(v));
    }
    let flags: i64 = node
        .attribute("flags")
        .unwrap_or("print")
        .split(',')
        .filter_map(|f| FLAGS.iter().find(|(_, s)| s.eq_ignore_ascii_case(f.trim())).map(|x| x.0))
        .sum();
    d.set(b"F".to_vec(), Object::Int(flags));
    if let Some(c) = node.attribute("color").and_then(parse_hex) {
        d.set(b"C".to_vec(), arr(&c));
    }
    if let Some(c) = node.attribute("interior-color").and_then(parse_hex) {
        d.set(b"IC".to_vec(), arr(&c));
    }
    if let Some(o) = node.attribute("opacity").and_then(|o| o.parse::<f64>().ok()) {
        d.set(b"CA".to_vec(), Object::Real(o.clamp(0.0, 1.0)));
    }
    if let Some(w) = node.attribute("width").and_then(|w| w.parse::<f64>().ok()) {
        let mut bs = Dict::new();
        bs.set(b"W".to_vec(), Object::Real(w.max(0.0)));
        bs.set(b"S".to_vec(), Object::name("S"));
        d.set(b"BS".to_vec(), Object::Dict(bs));
    }
    if let Some(q) = node.attribute("coords") {
        d.set(b"QuadPoints".to_vec(), arr(&csv(q)));
    }
    if let (Some(s), Some(e)) = (node.attribute("start"), node.attribute("end")) {
        let (s, e) = (csv(s), csv(e));
        if s.len() == 2 && e.len() == 2 {
            d.set(b"L".to_vec(), arr(&[s[0], s[1], e[0], e[1]]));
        }
    }
    if node.attribute("head").is_some() || node.attribute("tail").is_some() {
        d.set(
            b"LE".to_vec(),
            Object::Array(vec![Object::name(node.attribute("head").unwrap_or("None")), Object::name(node.attribute("tail").unwrap_or("None"))]),
        );
    }
    for child in node.children().filter(|c| c.is_element()) {
        match child.tag_name().name() {
            "contents" => {
                d.set(b"Contents".to_vec(), PdfString::text(&child.text().unwrap_or("").replace("\r\n", "\r").replace('\n', "\r")));
            }
            "defaultappearance" => {
                d.set(b"DA".to_vec(), PdfString::literal(child.text().unwrap_or("").as_bytes().to_vec()));
            }
            "inklist" => {
                let list: Vec<Object> = child
                    .children()
                    .filter(|g| g.has_tag_name("gesture"))
                    .map(|g| arr(&g.text().unwrap_or("").split(';').flat_map(|p| csv(p).into_iter()).collect::<Vec<_>>()))
                    .collect();
                d.set(b"InkList".to_vec(), Object::Array(list));
            }
            _ => {}
        }
    }
    Some(d)
}

/// Put an annotation dictionary on a page, replacing one with the same `/NM`. Returns its
/// reference.
fn place(doc: &mut Document, page_ref: ObjRef, d: Dict) -> Result<ObjRef, DataError> {
    let nm = d.get(b"NM").and_then(|n| n.as_string().map(|s| s.to_text()));
    let annots: Vec<Object> = doc
        .get(page_ref)
        .as_dict()
        .and_then(|p| p.get(b"Annots").cloned())
        .map(|a| doc.resolve(&a))
        .and_then(|a| a.as_array().cloned())
        .unwrap_or_default();
    let mut kept = Vec::new();
    for a in annots {
        let same = nm.is_some()
            && a.as_ref().and_then(|r| doc.get(r).as_dict().and_then(|x| x.get(b"NM").and_then(|n| n.as_string().map(|s| s.to_text())))) == nm;
        if !same {
            kept.push(a);
        }
    }
    let r = doc.add(Object::Dict(d));
    // Our own appearance where we can draw one; others keep none (viewers draw a default).
    let _ = pdfcraft_annot::set_appearance(doc, r);
    kept.push(Object::Ref(r));
    doc.update_dict(page_ref, |p| p.set(b"Annots".to_vec(), Object::Array(kept)))?;
    Ok(r)
}

fn import_xfdf(doc: &mut Document, text: &str) -> Result<Report, DataError> {
    let xml = roxmltree::Document::parse(text).map_err(|e| DataError::Malformed(e.to_string()))?;
    let root = xml.root_element();
    if root.tag_name().name() != "xfdf" {
        return Err(DataError::UnknownFormat);
    }
    let mut report = Report::default();
    let pages: Vec<ObjRef> = pdfcraft_model::pages(doc).iter().map(|p| p.obj).collect();
    // Comments: parents first, then replies (whose /IRT needs the parent's reference).
    let mut by_name: HashMap<String, ObjRef> = HashMap::new();
    let mut later: Vec<(ObjRef, String)> = Vec::new();
    if let Some(annots) = root.children().find(|c| c.has_tag_name("annots")) {
        for node in annots.children().filter(|c| c.is_element()) {
            let tag = node.tag_name().name().to_ascii_lowercase();
            let Some((subtype, _)) = SUBTYPES.iter().find(|(_, t)| *t == tag) else { continue };
            let Some(page) = node.attribute("page").and_then(|p| p.parse::<usize>().ok()).and_then(|p| pages.get(p).copied()) else { continue };
            let Some(d) = annot_from_xml(node, subtype, page) else { continue };
            let r = place(doc, page, d)?;
            if let Some(nm) = node.attribute("name") {
                by_name.insert(nm.to_string(), r);
            }
            if let Some(irt) = node.attribute("inreplyto") {
                later.push((r, irt.to_string()));
            }
            // A pop-up window.
            if let Some(pop) = node.children().find(|c| c.has_tag_name("popup"))
                && let Some(rect) = pop.attribute("rect").map(csv).filter(|r| r.len() == 4)
            {
                let mut p = Dict::new();
                p.set(b"Type".to_vec(), Object::name("Annot"));
                p.set(b"Subtype".to_vec(), Object::name("Popup"));
                p.set(b"Rect".to_vec(), arr(&rect));
                p.set(b"Parent".to_vec(), Object::Ref(r));
                p.set(b"Open".to_vec(), Object::Bool(pop.attribute("open") == Some("yes")));
                let pr = doc.add(Object::Dict(p));
                doc.update_dict(r, |d| d.set(b"Popup".to_vec(), Object::Ref(pr)))?;
                doc.update_dict(page, |pd| {
                    if let Some(Object::Array(a)) = pd.get_mut(b"Annots") {
                        a.push(Object::Ref(pr));
                    }
                })?;
            }
            report.comments += 1;
        }
    }
    for (r, irt) in later {
        if let Some(parent) = by_name.get(&irt).copied().or_else(|| find_by_name(doc, &irt)) {
            doc.update_dict(r, |d| d.set(b"IRT".to_vec(), Object::Ref(parent)))?;
        }
    }
    // Fields.
    if let Some(fields) = root.children().find(|c| c.has_tag_name("fields")) {
        let mut values = Vec::new();
        walk_xfdf(fields, "", &mut values);
        apply_values(doc, &values, &mut report);
    }
    Ok(report)
}

fn find_by_name(doc: &Document, nm: &str) -> Option<ObjRef> {
    for p in pdfcraft_model::pages(doc) {
        for a in p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default() {
            if let Some(r) = a.as_ref()
                && doc.get(r).as_dict().and_then(|d| text_of(doc, d, b"NM")).as_deref() == Some(nm)
            {
                return Some(r);
            }
        }
    }
    None
}

/// Copy an object out of another document, resolving references (FDF objects are small).
fn deep(src: &Document, o: &Object, depth: usize) -> Object {
    if depth > 16 {
        return Object::Null;
    }
    match o {
        Object::Ref(r) => deep(src, &src.get(*r), depth + 1),
        Object::Array(a) => Object::Array(a.iter().map(|x| deep(src, x, depth + 1)).collect()),
        Object::Dict(d) => Object::Dict(d.iter().map(|(k, v)| (k.clone(), deep(src, v, depth + 1))).collect()),
        Object::Stream(_) => Object::Null,
        other => other.clone(),
    }
}

fn walk_xfdf(node: roxmltree::Node, prefix: &str, out: &mut Vec<(String, Vec<String>)>) {
    for f in node.children().filter(|c| c.has_tag_name("field")) {
        let Some(name) = f.attribute("name") else { continue };
        let full = if prefix.is_empty() { name.to_string() } else { format!("{prefix}.{name}") };
        let vals: Vec<String> = f.children().filter(|c| c.has_tag_name("value")).map(|v| v.text().unwrap_or("").to_string()).collect();
        if !vals.is_empty() {
            out.push((full.clone(), vals));
        }
        walk_xfdf(f, &full, out);
    }
}

fn walk_fdf(fdf: &Document, list: &[Object], prefix: &str, out: &mut Vec<(String, Vec<String>)>, depth: usize) {
    if depth > 32 {
        return;
    }
    for f in list {
        let Some(d) = fdf.resolve(f).as_dict().cloned() else { continue };
        let Some(t) = d.get(b"T").and_then(|t| fdf.resolve(t).as_string().map(|s| s.to_text())) else { continue };
        let full = if prefix.is_empty() { t } else { format!("{prefix}.{t}") };
        if let Some(v) = d.get(b"V").map(|v| fdf.resolve(v)) {
            let vals: Vec<String> = match &*v {
                Object::String(s) => vec![s.to_text()],
                Object::Name(n) => vec![String::from_utf8_lossy(n).into_owned()],
                Object::Array(a) => a.iter().filter_map(|x| fdf.resolve(x).as_string().map(|s| s.to_text())).collect(),
                _ => Vec::new(),
            };
            out.push((full.clone(), vals));
        }
        if let Some(kids) = d.get(b"Kids").map(|k| fdf.resolve(k)).and_then(|k| k.as_array().cloned()) {
            walk_fdf(fdf, &kids, &full, out, depth + 1);
        }
    }
}

fn fdf_body(fdf: &Document) -> Result<Dict, DataError> {
    let root = fdf.root().ok_or_else(|| DataError::Malformed("no /Root".into()))?;
    fdf.get(root)
        .as_dict()
        .and_then(|d| d.get(b"FDF").cloned())
        .map(|f| fdf.resolve(&f))
        .and_then(|f| f.as_dict().cloned())
        .ok_or(DataError::UnknownFormat)
}

/// The field values in a form data file (FDF or XFDF) or a filled-in PDF form, in file order.
pub fn data_values(bytes: &[u8]) -> Result<Vec<(String, Vec<String>)>, DataError> {
    if bytes.starts_with(b"%FDF") {
        let fdf = Document::open(std::sync::Arc::new(bytes.to_vec())).map_err(|e| DataError::Malformed(e.to_string()))?;
        let body = fdf_body(&fdf)?;
        let mut values = Vec::new();
        if let Some(list) = body.get(b"Fields").map(|f| fdf.resolve(f)).and_then(|f| f.as_array().cloned()) {
            walk_fdf(&fdf, &list, "", &mut values, 0);
        }
        return Ok(values);
    }
    if bytes.starts_with(b"%PDF") || bytes.windows(5).take(1024).any(|w| w == b"%PDF-") {
        let doc = Document::open(std::sync::Arc::new(bytes.to_vec())).map_err(|e| DataError::Malformed(e.to_string()))?;
        return Ok(pdfcraft_forms::fields(&doc)
            .into_iter()
            .filter(|f| !matches!(f.kind, FieldKind::PushButton | FieldKind::Signature))
            .map(|f| {
                let v =
                    if matches!(f.kind, FieldKind::CheckBox | FieldKind::Radio) && f.value.is_empty() { vec!["Off".to_string()] } else { f.value };
                (f.name, v)
            })
            .collect());
    }
    let text = String::from_utf8_lossy(bytes);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    let xml = roxmltree::Document::parse(t).map_err(|_| DataError::UnknownFormat)?;
    let root = xml.root_element();
    if root.tag_name().name() != "xfdf" {
        return Err(DataError::UnknownFormat);
    }
    let mut values = Vec::new();
    if let Some(fields) = root.children().find(|c| c.has_tag_name("fields")) {
        walk_xfdf(fields, "", &mut values);
    }
    Ok(values)
}

/// Merge Data Files into Spreadsheet: one CSV row per file, one column per field name (in the
/// order they first appear).
pub fn merge_csv(files: &[Vec<(String, Vec<String>)>]) -> String {
    let mut columns: Vec<&str> = Vec::new();
    for f in files {
        for (k, _) in f {
            if !columns.contains(&k.as_str()) {
                columns.push(k);
            }
        }
    }
    let q = |s: &str| if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
    let mut out = columns.iter().map(|c| q(c)).collect::<Vec<_>>().join(",");
    out.push('\n');
    for f in files {
        let row: Vec<String> = columns.iter().map(|c| f.iter().find(|(k, _)| k == c).map_or(String::new(), |(_, v)| q(&v.join(", ")))).collect();
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

fn import_fdf(doc: &mut Document, bytes: &[u8]) -> Result<Report, DataError> {
    let fdf = Document::open(std::sync::Arc::new(bytes.to_vec())).map_err(|e| DataError::Malformed(e.to_string()))?;
    let body = fdf_body(&fdf)?;
    let mut report = Report::default();
    // Fields (with /Kids for hierarchical names).
    let mut values = Vec::new();
    if let Some(list) = body.get(b"Fields").map(|f| fdf.resolve(f)).and_then(|f| f.as_array().cloned()) {
        walk_fdf(&fdf, &list, "", &mut values, 0);
    }
    // Comments: parents first, then replies (whose /IRT needs the parent's new reference).
    let pages: Vec<ObjRef> = pdfcraft_model::pages(doc).iter().map(|p| p.obj).collect();
    let mut placed: HashMap<ObjRef, ObjRef> = HashMap::new();
    let mut later: Vec<(ObjRef, Option<ObjRef>, Option<String>)> = Vec::new();
    for a in body.get(b"Annots").map(|a| fdf.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default() {
        let Some(mut src) = fdf.resolve(&a).as_dict().cloned() else { continue };
        // The parent is another comment: link it below rather than copying it in.
        let irt = src.get(b"IRT").cloned();
        src.remove(b"IRT");
        let Object::Dict(mut d) = deep(&fdf, &Object::Dict(src), 0) else { continue };
        let Some(page) = d.get(b"Page").and_then(Object::as_int).and_then(|p| usize::try_from(p).ok()).and_then(|p| pages.get(p).copied()) else {
            continue;
        };
        if !d.name(b"Subtype").is_some_and(|s| SUBTYPES.iter().any(|(x, _)| x.as_bytes() == s)) {
            continue;
        }
        d.remove(b"Page");
        d.set(b"P".to_vec(), Object::Ref(page));
        let r = place(doc, page, d)?;
        if let Some(fr) = a.as_ref() {
            placed.insert(fr, r);
        }
        if let Some(irt) = irt {
            let parent_name = fdf.resolve(&irt).as_dict().and_then(|p| text_of(&fdf, p, b"NM"));
            later.push((r, irt.as_ref(), parent_name));
        }
        report.comments += 1;
    }
    for (r, parent_obj, parent_name) in later {
        // The parent imported from this file, else one already in the document with its name.
        let parent = parent_obj.and_then(|p| placed.get(&p).copied()).or_else(|| parent_name.and_then(|nm| find_by_name(doc, &nm)));
        if let Some(parent) = parent.filter(|p| *p != r) {
            doc.update_dict(r, |d| d.set(b"IRT".to_vec(), Object::Ref(parent)))?;
        }
    }
    apply_values(doc, &values, &mut report);
    Ok(report)
}

/// Form data from XML, CSV or tab-delimited text.
fn import_table(doc: &mut Document, text: &str) -> Result<Report, DataError> {
    let mut report = Report::default();
    let t = text.trim_start_matches('\u{feff}');
    let values: Vec<(String, Vec<String>)> = if t.trim_start().starts_with('<') {
        let xml = roxmltree::Document::parse(t).map_err(|e| DataError::Malformed(e.to_string()))?;
        xml.root_element()
            .children()
            .filter(|c| c.is_element())
            .map(|c| {
                let name =
                    c.attributes().find(|a| a.name() == "original").map(|a| a.value().to_string()).unwrap_or_else(|| c.tag_name().name().to_string());
                (name, vec![c.text().unwrap_or("").to_string()])
            })
            .collect()
    } else if t.lines().next().is_some_and(|l| l.contains('\t')) {
        let mut lines = t.lines();
        let (Some(head), Some(row)) = (lines.next(), lines.next()) else { return Err(DataError::UnknownFormat) };
        head.split('\t').map(str::to_string).zip(row.split('\t')).map(|(k, v)| (k, vec![v.to_string()])).collect()
    } else {
        // CSV with quotes: a quoted value may hold commas, doubled quotes and line breaks.
        let mut records: Vec<Vec<String>> = Vec::new();
        let mut record = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        let mut chars = t.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' if quoted && chars.peek() == Some(&'"') => {
                    cur.push('"');
                    chars.next();
                }
                '"' => quoted = !quoted,
                ',' if !quoted => record.push(std::mem::take(&mut cur)),
                '\r' if !quoted && chars.peek() == Some(&'\n') => {}
                '\n' if !quoted => {
                    record.push(std::mem::take(&mut cur));
                    records.push(std::mem::take(&mut record));
                }
                c => cur.push(c),
            }
        }
        record.push(cur);
        records.push(record);
        // Blank lines (such as the one a trailing line break leaves) are not records.
        let mut records = records.into_iter().filter(|r| !matches!(r.as_slice(), [only] if only.is_empty()));
        let (Some(head), Some(row)) = (records.next(), records.next()) else { return Err(DataError::UnknownFormat) };
        head.into_iter().zip(row).map(|(k, v)| (k, vec![v])).collect()
    };
    apply_values(doc, &values, &mut report);
    Ok(report)
}

/// Import comments and/or field values from XFDF, FDF, XML, CSV or tab-delimited text
/// (detected from the content).
pub fn import(doc: &mut Document, bytes: &[u8]) -> Result<Report, DataError> {
    let report = if bytes.starts_with(b"%FDF") {
        import_fdf(doc, bytes)?
    } else {
        let text = String::from_utf8_lossy(bytes);
        let t = text.trim_start_matches('\u{feff}').trim_start();
        if t.starts_with("<?xml") || t.starts_with("<xfdf") || t.starts_with('<') {
            match import_xfdf(doc, t) {
                Err(DataError::UnknownFormat) => import_table(doc, t)?,
                other => other?,
            }
        } else {
            import_table(doc, &text)?
        }
    };
    if report.comments == 0 && report.fields == 0 && report.rejected.is_empty() {
        return Err(DataError::NothingImported);
    }
    Ok(report)
}
