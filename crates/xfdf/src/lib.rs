//! Comment and form data exchange (execution plan M5.6, M6.7). Layer L4.
//!
//! - **XFDF** (ISO 19444-1, Acrobat's "Export all to data file" / "Import comments", and form
//!   data): comments with their geometry, colours, flags, authors, dates, replies, pop-ups,
//!   ink, quadrilaterals, line ends and callouts; field values, nested by name.
//! - **FDF** (ISO 32000-2 §12.7.8): fields and comments in PDF syntax.
//! - Form data as **XML**, **CSV** and **tab-delimited text** (Acrobat's Export data formats).
//!
//! Import merges: a comment whose name (`/NM`) already exists on its page replaces it; field
//! values go through the form's own checks (formats, validation) and recalculate.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::{HashMap, HashSet};
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
    #[error("FDF export exceeds the {0} limit; no data was exported")]
    ExportLimit(&'static str),
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
    /// An XFA form's data as an XML Data Package (the engine writes it; for a form without
    /// XFA packets it is the XML format).
    Xdp,
}

impl Format {
    pub fn from_extension(ext: &str) -> Option<Format> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "xfdf" => Format::Xfdf,
            "fdf" => Format::Fdf,
            "xml" => Format::Xml,
            "xdp" => Format::Xdp,
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

/// Form data files are attacker-supplied. Refuse a DTD and bound the node count, exactly as the
/// XFA template parser does (`crates/xfa/src/parse.rs`), and bound how deep a nested field tree
/// may go: the walk below recurses once per nesting level.
const MAX_NODES: u32 = 2_000_000;
const MAX_DEPTH: usize = 64;

/// The deepest element nesting accepted before the text reaches the XML parser.
///
/// roxmltree parses by recursive descent, so its stack use grows with the nesting depth: ten
/// thousand levels overflow the main thread's stack and abort the process. A stack overflow is
/// `abort()`, so `catch_unwind` cannot intercept it and the whole application dies - which is why
/// the bound has to be applied before parsing, not while walking the result. Around three thousand
/// levels still parse on the default stack, so this stays clear of the cliff while leaving far more
/// room than the walks below, which cut off at 64.
const MAX_XML_DEPTH: usize = 1_024;

/// Element nesting depth of `text`, skipping comments, CDATA, processing instructions, the DOCTYPE
/// and quoted attribute values. A tag that closes itself (`<field/>`) is one level while it is read
/// and does not stay open, so a long run of them cannot accumulate into the bound. It cannot
/// under-count a real element; a stray `<` in text is invalid XML and only makes it over-count,
/// which errs towards refusing the file.
fn xml_nesting_depth(text: &str) -> usize {
    fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
        (from..hay.len()).find(|&p| hay[p..].starts_with(needle))
    }
    let b = text.as_bytes();
    let (mut i, mut depth, mut max) = (0usize, 0usize, 0usize);
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
        } else if b[i..].starts_with(b"<!--") {
            i = find(b, i + 4, b"-->").map_or(b.len(), |p| p + 3);
        } else if b[i..].starts_with(b"<![CDATA[") {
            i = find(b, i + 9, b"]]>").map_or(b.len(), |p| p + 3);
        } else if b[i..].starts_with(b"<?") {
            i = find(b, i + 2, b"?>").map_or(b.len(), |p| p + 2);
        } else if b[i..].starts_with(b"<!") {
            i = find(b, i + 2, b">").map_or(b.len(), |p| p + 1);
        } else if b[i..].starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = find(b, i + 2, b">").map_or(b.len(), |p| p + 1);
        } else {
            depth += 1;
            max = max.max(depth);
            // Skip the rest of the start tag, over any quoted attribute value, and remember its last
            // significant byte: a tag that ends in `/` closes itself, so it is one level deep while it
            // is read rather than a level left open.
            i += 1;
            let mut quote = 0u8;
            let mut last = 0u8;
            while i < b.len() {
                let c = b[i];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    i += 1;
                    break;
                } else if !c.is_ascii_whitespace() {
                    last = c;
                }
                i += 1;
            }
            if last == b'/' {
                depth -= 1;
            }
        }
    }
    max
}

fn check_form_xml_depth(text: &str) -> Result<(), DataError> {
    let depth = xml_nesting_depth(text);
    if depth > MAX_XML_DEPTH {
        return Err(DataError::Malformed(format!("the file nests XML elements {depth} deep, past the {MAX_XML_DEPTH} the reader accepts")));
    }
    Ok(())
}

fn parse_form_xml(text: &str) -> Result<roxmltree::Document<'_>, roxmltree::Error> {
    roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES })
}

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
            // XML 1.0 §3.3.3 normalizes literal attribute whitespace, but not references.
            '\t' => o.push_str("&#9;"),
            '\n' => o.push_str("&#10;"),
            '\r' => o.push_str("&#13;"),
            c if (c as u32) < 0x20 && c != '\n' && c != '\t' => {}
            // XML 1.0 noncharacters are not allowed even as raw text.
            '\u{FFFE}' | '\u{FFFF}' => {}
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
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let p = |i: usize| h.get(i..i + 2).and_then(|s| u8::from_str_radix(s, 16).ok()).map(|v| f64::from(v) / 255.0);
    Some([p(0)?, p(2)?, p(4)?])
}

fn nums_of(doc: &Document, o: Option<&Object>) -> Vec<f64> {
    o.map(|o| doc.resolve(o)).and_then(|a| a.as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect())).unwrap_or_default()
}

/// A `/Rect` of exactly four finite numbers; any other entry makes the whole rectangle unusable.
fn rect_strict(doc: &Document, o: Option<&Object>) -> Option<[f64; 4]> {
    let num = |x: &Object| doc.resolve(x).as_f64().filter(|v| v.is_finite());
    match doc.resolve(o?).as_array()?.as_slice() {
        [x0, y0, x1, y1] => Some([num(x0)?, num(y0)?, num(x1)?, num(y1)?]),
        _ => None,
    }
}

/// Whether a quad's four corners enclose any area: some three of them are not collinear.
fn quad_has_area(q: &[f64; 8]) -> bool {
    let [ax, ay, bx, by, cx, cy, dx, dy] = *q;
    let cross = |(px, py): (f64, f64), (qx, qy): (f64, f64), (rx, ry): (f64, f64)| (qx - px) * (ry - py) - (qy - py) * (rx - px);
    let (a, b, c, d) = ((ax, ay), (bx, by), (cx, cy), (dx, dy));
    [cross(a, b, c), cross(a, b, d), cross(a, c, d), cross(b, c, d)].iter().any(|v| v.abs() > 0.0)
}

fn text_of(doc: &Document, d: &Dict, k: &[u8]) -> Option<String> {
    d.get(k).and_then(|o| match &*doc.resolve(o) {
        Object::String(s) => Some(s.to_text()),
        Object::Name(n) => Some(pdfcraft_forms::name_text(n)),
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

/// The annotations of `which` (0-based page, position in the page's `/Annots`) as positions,
/// with every reply to them (and to those replies), so a thread travels whole. Positions, not
/// references: other tools write comments straight into the page's array, without an object of
/// their own, and those copy too.
fn annots_with_replies(doc: &Document, which: &[(usize, usize)]) -> HashSet<(usize, usize)> {
    let pages = pdfcraft_model::pages(doc);
    let annots = |p: &pdfcraft_model::Page| -> Vec<Object> {
        p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default()
    };
    /// An annotation's position, its reference (when it has one) and its parent's (`/IRT`).
    struct Entry {
        pos: (usize, usize),
        obj: Option<ObjRef>,
        parent: Option<ObjRef>,
    }
    let all: Vec<Entry> = pages
        .iter()
        .enumerate()
        .flat_map(|(pi, p)| {
            annots(p).into_iter().enumerate().map(move |(ai, a)| {
                let parent = doc.resolve(&a).as_dict().and_then(|d| d.get(b"IRT")).and_then(Object::as_ref);
                Entry { pos: (pi, ai), obj: a.as_ref(), parent }
            })
        })
        .collect();
    let mut set: HashSet<(usize, usize)> = which.iter().copied().filter(|w| all.iter().any(|e| e.pos == *w)).collect();
    // Replies point at their parent through /IRT: a pass per reply depth, bounded by the count.
    for _ in 0..all.len() {
        let before = set.len();
        let parents: HashSet<ObjRef> = all.iter().filter(|e| set.contains(&e.pos)).filter_map(|e| e.obj).collect();
        for e in &all {
            if e.parent.is_some_and(|parent| parents.contains(&parent)) {
                set.insert(e.pos);
            }
        }
        if set.len() == before {
            break;
        }
    }
    set
}

/// Every comment as an XFDF element (one line per comment, replies included), or only those at
/// the positions in `only`.
/// The namespace of PdfCraft's own attributes in XFDF (`pdfcraft:keys`): other readers skip them.
const PDFCRAFT_NS: &str = "https://github.com/storytold/pdfcraft#xfdf";
/// At most this much text in a `pdfcraft:keys` attribute is read.
const MAX_OWN_KEYS: usize = 64 * 1024;

/// A value that means the same in another document: no reference or stream into this one.
fn portable(o: &Object, depth: usize) -> bool {
    match o {
        Object::Ref(_) | Object::Stream(_) => false,
        Object::Array(a) => depth < 4 && a.iter().all(|o| portable(o, depth + 1)),
        Object::Dict(d) => depth < 4 && d.iter().all(|(_, o)| portable(o, depth + 1)),
        _ => true,
    }
}

/// The annotation's own PdfCraft keys (`/PC…`: a Fill & Sign item, a turned comment's box and
/// angle, …) as one PDF dictionary, for a copy that is what the original was. Values that point
/// into this document stay behind.
fn own_keys(d: &Dict) -> Option<String> {
    let mut keys = Dict::new();
    // Each value must also write as text: XML can't hold raw bytes.
    let writes_as_text = |v: &Object| {
        let mut out = Vec::new();
        pdfcraft_cos::serialize(v, &mut out);
        std::str::from_utf8(&out).is_ok()
    };
    for (k, v) in d.iter().filter(|(k, v)| k.starts_with(b"PC") && portable(v, 0) && writes_as_text(v)) {
        keys.set(k.clone(), v.clone());
    }
    if keys.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    pdfcraft_cos::serialize(&Object::Dict(keys), &mut out);
    String::from_utf8(out).ok()
}

/// The `pdfcraft:keys` of an XFDF comment back as dictionary entries: PdfCraft's keys only, with
/// values that mean the same here. Anything malformed is skipped.
fn read_own_keys(node: roxmltree::Node, d: &mut Dict) {
    let Some(text) = node.attribute((PDFCRAFT_NS, "keys")).filter(|t| t.len() <= MAX_OWN_KEYS) else { return };
    let Ok(Object::Dict(keys)) = pdfcraft_cos::Lexer::new(text.as_bytes(), 0).object() else { return };
    for (k, v) in keys.iter().filter(|(k, v)| k.starts_with(b"PC") && portable(v, 0)) {
        if !d.contains(k) {
            d.set(k.clone(), v.clone());
        }
    }
}

/// `own`: write each comment's PdfCraft keys too (see [`own_keys`]), for a copy and paste.
fn xfdf_annots(doc: &Document, out: &mut String, only: Option<&HashSet<(usize, usize)>>, own: bool) {
    let pages = pdfcraft_model::pages(doc);
    // NM of every annotation, for inreplyto. A selected annotation without one (other tools
    // leave /NM out) gets a name for this export, so a reply copied with it still finds it.
    let mut names: HashMap<ObjRef, String> = HashMap::new();
    let mut unnamed: Vec<(usize, usize, ObjRef)> = Vec::new();
    // Every name in use, those of annotations written straight into the page array too, so a
    // given name is new.
    let mut taken: HashSet<String> = HashSet::new();
    for (pi, p) in pages.iter().enumerate() {
        for (ai, a) in p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default().into_iter().enumerate() {
            let nm = doc.resolve(&a).as_dict().and_then(|d| text_of(doc, d, b"NM"));
            if let Some(nm) = &nm {
                taken.insert(nm.clone());
            }
            let Some(r) = a.as_ref() else { continue };
            match nm {
                Some(nm) => {
                    names.insert(r, nm);
                }
                None if only.is_some_and(|only| only.contains(&(pi, ai))) => unnamed.push((pi, ai, r)),
                None => {}
            }
        }
    }
    for (pi, ai, r) in unnamed {
        let mut name = format!("pdfcraft-{pi}-{ai}");
        while taken.contains(&name) {
            name.push('x');
        }
        taken.insert(name.clone());
        names.insert(r, name);
    }
    for (pi, p) in pages.iter().enumerate() {
        for (ai, a) in p.dict.get(b"Annots").map(|a| doc.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default().into_iter().enumerate() {
            if only.is_some_and(|only| !only.contains(&(pi, ai))) {
                continue;
            }
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
            // The name: /NM, or the one given above for this export.
            if let Some(nm) = text_of(doc, &d, b"NM").or_else(|| a.as_ref().and_then(|r| names.get(&r).cloned())) {
                let _ = write!(attrs, " name=\"{}\"", esc(&nm));
            }
            for (key, attr) in [
                (&b"T"[..], "title"),
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
            let _ = write!(attrs, " flags=\"{}\"", fl.join(","));
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
                let _ = write!(attrs, " head=\"{}\" tail=\"{}\"", esc(&name(&le[0])), esc(&name(&le[1])));
            }
            // A callout: its leader path, text-box inset and (single-name) line ending.
            let cl = nums_of(doc, d.get(b"CL"));
            if cl.len() == 4 || cl.len() == 6 {
                let _ = write!(attrs, " callout=\"{}\"", cl.iter().map(|v| n(*v)).collect::<Vec<_>>().join(","));
                let rd = nums_of(doc, d.get(b"RD"));
                if rd.len() == 4 {
                    let _ = write!(attrs, " fringe=\"{}\"", rd.iter().map(|v| n(*v)).collect::<Vec<_>>().join(","));
                }
                if let Some(le) = d.get(b"LE").map(|o| doc.resolve(o)).and_then(|o| o.as_name().map(|x| String::from_utf8_lossy(x).into_owned())) {
                    let _ = write!(attrs, " head=\"{}\"", esc(&le));
                }
            }
            // What the comment is meant as (/IT): typewriter text, a callout, a cloud, an arrow.
            if let Some(it) = d.get(b"IT").map(|o| doc.resolve(o)).and_then(|o| o.as_name().map(|x| String::from_utf8_lossy(x).into_owned())) {
                let _ = write!(attrs, " intent=\"{}\"", esc(&it));
            }
            // A cloudy border (/BE /S /C) and how far its arcs bulge.
            if let Some(be) =
                d.get(b"BE").map(|o| doc.resolve(o)).and_then(|o| o.as_dict().cloned()).filter(|b| b.name(b"S") == Some(b"C".as_slice()))
            {
                attrs.push_str(" style=\"cloudy\"");
                if let Some(i) = be.get(b"I").and_then(Object::as_f64) {
                    let _ = write!(attrs, " intensity=\"{}\"", n(i));
                }
            }
            if let Some(irt) = d.get(b"IRT").and_then(Object::as_ref).and_then(|r| names.get(&r)) {
                let _ = write!(attrs, " inreplyto=\"{}\"", esc(irt));
            }
            if own && let Some(keys) = own_keys(&d) {
                let _ = write!(attrs, " pdfcraft:keys=\"{}\"", esc(&keys));
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
            // Polygon and polyline points (XFDF `<vertices>`: `x,y;x,y;…`).
            let vertices = nums_of(doc, d.get(b"Vertices"));
            let pts: Vec<String> = vertices.as_chunks::<2>().0.iter().map(|p| format!("{},{}", n(p[0]), n(p[1]))).collect();
            if !pts.is_empty() {
                let _ = write!(out, "<vertices>{}</vertices>", pts.join(";"));
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
        // One `<field>` level per part of the name, but no more levels than the import reads
        // back. A `/T` can hold periods of its own, so a name can split into far more parts than
        // the field tree has levels: nesting every part wrote a file the import refuses and, for
        // a long enough name, recursed in `write_node` until the stack overflowed. The parts past
        // the last level stay together in the deepest element's name, and the import joins them
        // back with periods into the same full name.
        for part in f.name.splitn(MAX_DEPTH, '.') {
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
        xfdf_annots(doc, &mut out, None, false);
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

/// XFDF with these comments alone (0-based page and position in the page's `/Annots`), each
/// with its replies: what Copy puts on the clipboard, and Export selected comments writes.
pub fn export_xfdf_comments(doc: &Document, which: &[(usize, usize)], file: &str) -> String {
    let only = annots_with_replies(doc, which);
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<xfdf xmlns=\"http://ns.adobe.com/xfdf/\" xmlns:pdfcraft=\"{PDFCRAFT_NS}\" xml:space=\"preserve\">\n"
    );
    out.push_str("<annots>\n");
    xfdf_annots(doc, &mut out, Some(&only), true);
    out.push_str("</annots>\n");
    let _ = writeln!(out, "<f href=\"{}\"/>", esc(file));
    out.push_str("</xfdf>\n");
    out
}

/// Whether `text` is XFDF with comments in it (what a paste of copied comments carries).
pub fn is_xfdf_comments(text: &str) -> bool {
    let head: String = text.chars().take(400).collect();
    head.contains("<xfdf") && text.contains("<annots")
}

/// What a comment of an XFDF text looks like, for drawing a bundle of copied comments before
/// it is placed: its geometry in user space and its colour (RGB 0–1).
#[derive(Clone, Debug, PartialEq)]
pub enum Outline {
    /// A box: notes, squares, text boxes, stamps, attachments, carets (`round` for a circle).
    Rect { rect: [f64; 4], round: bool, color: Option<[f64; 3]> },
    /// A line or arrow, and a callout's leader.
    Line { points: Vec<[f64; 2]>, color: Option<[f64; 3]> },
    /// A polygon (closed) or connected lines.
    Poly { points: Vec<[f64; 2]>, closed: bool, color: Option<[f64; 3]> },
    /// Freehand strokes.
    Ink { strokes: Vec<Vec<[f64; 2]>>, color: Option<[f64; 3]> },
    /// Text markup: each quad is x1,y1 (upper left), x2,y2 (upper right), x3,y3 (lower left), x4,y4.
    Quads { quads: Vec<[f64; 8]>, color: Option<[f64; 3]> },
    /// A text box's text, from the top-left of its box (`rect`, which it wraps in), at its font
    /// `size` in points and in its text colour.
    Text { rect: [f64; 4], text: String, size: f64, color: Option<[f64; 3]> },
}

/// Every comment element of an XFDF text, in order (none when it is no XFDF).
fn with_comment_nodes(text: &str, mut f: impl FnMut(roxmltree::Node<'_, '_>)) {
    if check_form_xml_depth(text).is_err() {
        return;
    }
    let Ok(xml) = parse_form_xml(text) else { return };
    let root = xml.root_element();
    if root.tag_name().name() != "xfdf" {
        return;
    }
    let Some(annots) = root.children().find(|c| c.has_tag_name("annots")) else { return };
    for node in annots.children().filter(|c| c.is_element()) {
        if SUBTYPES.iter().any(|(_, t)| *t == node.tag_name().name().to_ascii_lowercase()) {
            f(node);
        }
    }
}

/// The comment elements of an XFDF text: tag, `inreplyto`, rect.
fn comment_nodes(text: &str) -> Vec<(String, Option<String>, Vec<f64>)> {
    let mut out = Vec::new();
    with_comment_nodes(text, |node| {
        out.push((
            node.tag_name().name().to_owned(),
            node.attribute("inreplyto").map(str::to_owned),
            node.attribute("rect").map(csv).unwrap_or_default(),
        ));
    });
    out
}

/// Pairs `x,y;x,y;…` (XFDF vertices and ink gestures).
fn pairs(text: &str) -> Vec<[f64; 2]> {
    text.split(';').filter_map(|p| if let [x, y] = csv(p).as_slice() { Some([*x, *y]) } else { None }).collect()
}

/// The outlines of the comments in an XFDF text (replies, which have no place of their own,
/// left out): what a paste shows hanging on the pointer.
pub fn comment_outlines(text: &str) -> Vec<Outline> {
    let mut out = Vec::new();
    with_comment_nodes(text, |node| {
        if node.attribute("inreplyto").is_some() {
            return;
        }
        let tag = node.tag_name().name().to_ascii_lowercase();
        let color = node.attribute("color").and_then(parse_hex);
        let rect = node
            .attribute("rect")
            .map(csv)
            .filter(|r| r.len() == 4 && r.iter().all(|v| v.is_finite()))
            .map(|r| [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])]);
        match tag.as_str() {
            "line" => {
                if let (Some(a), Some(b)) = (node.attribute("start").map(csv), node.attribute("end").map(csv))
                    && let ([ax, ay], [bx, by]) = (a.as_slice(), b.as_slice())
                {
                    out.push(Outline::Line { points: vec![[*ax, *ay], [*bx, *by]], color });
                }
            }
            "polygon" | "polyline" => {
                let points = node.children().find(|c| c.has_tag_name("vertices")).and_then(|v| v.text()).map(pairs).unwrap_or_default();
                if points.len() >= 2 {
                    out.push(Outline::Poly { points, closed: tag == "polygon", color });
                }
            }
            "ink" => {
                let strokes: Vec<Vec<[f64; 2]>> = node
                    .children()
                    .find(|c| c.has_tag_name("inklist"))
                    .map(|list| {
                        list.children().filter(|g| g.has_tag_name("gesture")).filter_map(|g| g.text()).map(pairs).filter(|s| !s.is_empty()).collect()
                    })
                    .unwrap_or_default();
                if !strokes.is_empty() {
                    out.push(Outline::Ink { strokes, color });
                }
            }
            "highlight" | "underline" | "strikeout" | "squiggly" | "redact" => {
                let coords = node.attribute("coords").map(csv).unwrap_or_default();
                let quads: Vec<[f64; 8]> = coords.as_chunks::<8>().0.to_vec();
                if quads.is_empty() {
                    if let Some(rect) = rect {
                        out.push(Outline::Rect { rect, round: false, color });
                    }
                } else {
                    out.push(Outline::Quads { quads, color });
                }
            }
            // A Fill & Sign mark: its tick, cross, dot or line, as its appearance draws it.
            "stamp"
                if let Some(rect) = rect
                    && let Some(mark) = node.attribute("icon").and_then(|i| pdfcraft_annot::FillMark::from_name(i.as_bytes())) =>
            {
                if mark == pdfcraft_annot::FillMark::Dot {
                    let r = (rect[2] - rect[0]).min(rect[3] - rect[1]) * 0.3;
                    let (cx, cy) = ((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0);
                    out.push(Outline::Rect { rect: [cx - r, cy - r, cx + r, cy + r], round: true, color });
                }
                for points in mark.strokes(rect) {
                    out.push(Outline::Line { points, color });
                }
            }
            _ => {
                if let Some(rect) = rect {
                    out.push(Outline::Rect { rect, round: tag == "circle", color });
                }
                // A callout's leader, from the box to what it points at.
                let callout = node.attribute("callout").map(csv).filter(|c| c.len() == 4 || c.len() == 6);
                if tag == "freetext"
                    && let Some(cl) = &callout
                {
                    out.push(Outline::Line { points: cl.as_chunks::<2>().0.to_vec(), color });
                }
                // A text box's text, in its own colour and size, inside the box (a callout's box
                // is its rect inset by the fringe).
                if tag == "freetext"
                    && let Some(rect) = rect
                    && let Some(text) = node.children().find(|c| c.has_tag_name("contents")).and_then(|c| c.text()).filter(|t| !t.trim().is_empty())
                {
                    let mut da = Dict::new();
                    if let Some(s) = node.children().find(|c| c.has_tag_name("defaultappearance")).and_then(|c| c.text()) {
                        da.set(b"DA".to_vec(), PdfString::literal(s.as_bytes().to_vec()));
                    }
                    let (text_color, size) = pdfcraft_annot::appearance::parse_da(&da);
                    let fringe = node.attribute("fringe").map(csv).filter(|f| callout.is_some() && f.len() == 4 && f.iter().all(|v| v.is_finite()));
                    let rect = match fringe.as_deref() {
                        Some([l, b, r, t]) => [rect[0] + l, rect[1] + b, rect[2] - r, rect[3] - t],
                        _ => rect,
                    };
                    out.push(Outline::Text { rect, text: text.replace("\r\n", "\n").replace('\r', "\n"), size, color: Some(text_color) });
                }
            }
        }
    });
    out
}

/// How many comments (replies not counted) an XFDF text holds.
pub fn comment_count(text: &str) -> usize {
    comment_nodes(text).iter().filter(|(_, irt, _)| irt.is_none()).count()
}

/// The box around every comment in an XFDF text, in the pages' user space: [x0, y0, x1, y1],
/// or `None` without a comment with a rect. What a paste shows hanging on the pointer and centres
/// on the point: replies, which have no place of their own (some tools still give them a rect, at
/// 0,0 or away from their parent), are left out, as [`comment_outlines`] leaves them out.
pub fn comments_bounds(text: &str) -> Option<[f64; 4]> {
    let mut bounds: Option<[f64; 4]> = None;
    for (_, in_reply_to, rect) in comment_nodes(text) {
        if in_reply_to.is_some() {
            continue;
        }
        if let [x0, y0, x1, y1] = rect.as_slice()
            && rect.iter().all(|v| v.is_finite())
        {
            let r = [x0.min(*x1), y0.min(*y1), x0.max(*x1), y0.max(*y1)];
            bounds = Some(match bounds {
                None => r,
                Some(b) => [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])],
            });
        }
    }
    bounds
}

/// Move an annotation's geometry by (dx, dy) in user space: its rect, quads, vertices, ink,
/// line ends and callout leader (what a copy placed elsewhere needs).
fn translate(d: &mut Dict, dx: f64, dy: f64) {
    let shift = |o: &Object| -> Option<Object> {
        let v: Vec<f64> = o.as_array()?.iter().map(|x| x.as_f64()).collect::<Option<Vec<f64>>>()?;
        Some(arr(&v.iter().enumerate().map(|(i, x)| if i % 2 == 0 { x + dx } else { x + dy }).collect::<Vec<f64>>()))
    };
    for key in [&b"Rect"[..], b"QuadPoints", b"Vertices", b"L", b"CL"] {
        if let Some(moved) = d.get(key).and_then(shift) {
            d.set(key.to_vec(), moved);
        }
    }
    if let Some(Object::Array(strokes)) = d.get(b"InkList").cloned() {
        let moved: Vec<Object> = strokes.iter().map(|s| shift(s).unwrap_or_else(|| s.clone())).collect();
        d.set(b"InkList".to_vec(), Object::Array(moved));
    }
}

fn pdf_string(s: &str) -> Vec<u8> {
    let mut v = Vec::new();
    pdfcraft_cos::serialize(&Object::String(PdfString::text(s)), &mut v);
    v
}

// Keep aliases as aliases instead of expanding a shared graph once per use. References are
// queued, not followed recursively, so cycles and long indirect chains cost linear work.
const MAX_FDF_OBJECTS: usize = 100_000;
const MAX_FDF_BYTES: usize = 64 * 1024 * 1024;

/// Direct values stay borrowed: `Document::resolve` would deep-clone them. Only
/// indirect values need an owner, and annotation bodies are never retained in discovery.
enum FdfValue<'a> {
    Borrowed(&'a Object),
    Indirect(std::sync::Arc<Object>),
}

impl<'a> FdfValue<'a> {
    fn resolve(doc: &Document, value: &'a Object) -> Self {
        match value {
            Object::Ref(_) => Self::Indirect(doc.resolve(value)),
            _ => Self::Borrowed(value),
        }
    }

    fn object(&self) -> &Object {
        match self {
            Self::Borrowed(value) => value,
            Self::Indirect(value) => value.as_ref(),
        }
    }
}

struct FdfCopy<'a> {
    doc: &'a Document,
    objects: Vec<Object>,
    numbers: HashMap<ObjRef, u32>,
    pending: Vec<(ObjRef, usize)>,
    nodes: usize,
    bytes: usize,
}

impl<'a> FdfCopy<'a> {
    fn new(doc: &'a Document) -> Self {
        Self { doc, objects: Vec::new(), numbers: HashMap::new(), pending: Vec::new(), nodes: MAX_FDF_OBJECTS, bytes: MAX_FDF_BYTES }
    }

    fn charge(&mut self, bytes: usize) -> Result<(), DataError> {
        self.bytes = self.bytes.checked_sub(bytes).ok_or(DataError::ExportLimit("64 MiB data"))?;
        Ok(())
    }

    fn slot(&mut self) -> Result<(usize, u32), DataError> {
        if self.objects.len() >= MAX_FDF_OBJECTS {
            return Err(DataError::ExportLimit("100,000 objects"));
        }
        // Object 1 is the FDF root; arena indices start at object 2. The arena is capped above.
        let index = self.objects.len();
        let number = index as u32 + 2;
        self.charge(100)?; // indirect-object and xref overhead
        self.objects.push(Object::Null);
        Ok((index, number))
    }

    fn dict(&mut self, dict: &Dict, depth: usize) -> Result<Dict, DataError> {
        if dict.len() > self.nodes {
            return Err(DataError::ExportLimit("100,000 values"));
        }
        let mut out = Dict::new();
        for (key, value) in dict.iter() {
            self.dict_entry(&mut out, key, value, depth + 1)?;
        }
        Ok(out)
    }

    fn dict_entry(&mut self, out: &mut Dict, key: &[u8], value: &Object, depth: usize) -> Result<(), DataError> {
        self.charge(key.len().saturating_mul(3).saturating_add(2))?;
        let value = self.copy(value, depth)?;
        out.set(key.to_vec(), value);
        Ok(())
    }

    /// Skip source-document links and appearances without cloning their values. The
    /// dictionary and every retained entry are charged before any owned copy is made.
    fn annotation_dict(&mut self, dict: &Dict) -> Result<Dict, DataError> {
        self.charge_value(0)?;
        let mut out = Dict::new();
        for (key, value) in dict.iter() {
            if matches!(key.as_slice(), b"P" | b"AP" | b"Popup" | b"IRT" | b"Parent" | b"FS" | b"Sound") {
                continue;
            }
            self.dict_entry(&mut out, key, value, 1)?;
        }
        Ok(out)
    }

    fn charge_value(&mut self, depth: usize) -> Result<(), DataError> {
        if depth > MAX_DEPTH {
            return Err(DataError::ExportLimit("64 direct nesting levels"));
        }
        self.nodes = self.nodes.checked_sub(1).ok_or(DataError::ExportLimit("100,000 values"))?;
        // Includes delimiters, spacing and even the longest finite f64 in plain decimal form.
        self.charge(330)?;
        Ok(())
    }

    fn is_source_page(&self, dict: &Dict) -> Result<bool, DataError> {
        let Some(value) = dict.get(b"Type") else { return Ok(false) };
        let mut value = FdfValue::Borrowed(value);
        let mut seen = HashSet::new();
        for hops in 0..=MAX_DEPTH {
            match value.object() {
                Object::Name(name) => return Ok(matches!(name.as_slice(), b"Page" | b"Pages")),
                Object::Ref(reference) => {
                    if self.doc.generation(reference.num) != reference.generation || !seen.insert(*reference) {
                        return Ok(false);
                    }
                    if hops == MAX_DEPTH {
                        return Err(DataError::ExportLimit("64 Type reference levels"));
                    }
                    value = FdfValue::Indirect(self.doc.get(*reference));
                }
                _ => return Ok(false),
            }
        }
        Err(DataError::ExportLimit("64 Type reference levels"))
    }

    fn copy(&mut self, value: &Object, depth: usize) -> Result<Object, DataError> {
        self.charge_value(depth)?;
        Ok(match value {
            Object::Ref(source) => {
                let number = if let Some(number) = self.numbers.get(source) {
                    *number
                } else {
                    let (index, number) = self.slot()?;
                    self.numbers.insert(*source, number);
                    self.pending.push((*source, index));
                    number
                };
                Object::Ref(ObjRef::new(number, 0))
            }
            Object::Array(values) => {
                if values.len() > self.nodes {
                    return Err(DataError::ExportLimit("100,000 values"));
                }
                let mut out = Vec::new();
                for value in values {
                    out.push(self.copy(value, depth + 1)?);
                }
                Object::Array(out)
            }
            // Source page links must not drag page contents or the page tree into an FDF.
            Object::Dict(dict) if self.is_source_page(dict)? => Object::Null,
            Object::Dict(dict) => Object::Dict(self.dict(dict, depth)?),
            Object::String(s) => {
                self.charge(s.bytes.len().saturating_mul(4))?;
                value.clone()
            }
            Object::Name(name) => {
                self.charge(name.len().saturating_mul(3))?;
                value.clone()
            }
            // A PDF stream can only be an indirect object, never a dictionary/array value.
            Object::Stream(_) => return Err(DataError::Malformed("FDF export encountered a direct stream".into())),
            other => other.clone(),
        })
    }

    fn finish(&mut self, doc: &Document) -> Result<(), DataError> {
        let mut cursor = 0;
        while let Some((source, index)) = self.pending.get(cursor).copied() {
            // ISO 32000-2 §7.3.10: an absent or stale reference denotes null. Keep reader
            // errors distinct so damaged objects cannot be silently lost during export.
            let value = if doc.generation(source.num) != source.generation {
                std::sync::Arc::new(Object::Null)
            } else {
                match doc.try_get(source.num) {
                    Ok(value) => value,
                    Err(pdfcraft_cos::CosError::MissingObject(_)) => std::sync::Arc::new(Object::Null),
                    Err(error) => return Err(error.into()),
                }
            };
            let copied = match &*value {
                Object::Stream(stream) => {
                    self.charge(stream.raw.len())?;
                    Object::Stream(pdfcraft_cos::Stream { dict: self.dict(&stream.dict, 0)?, raw: stream.raw.clone() })
                }
                other => self.copy(other, 0)?,
            };
            let slot = self.objects.get_mut(index).ok_or_else(|| DataError::Malformed("FDF object slot is missing".into()))?;
            *slot = copied;
            cursor += 1;
        }
        Ok(())
    }
}

/// FDF with field values and/or comments (appearances are not carried). Each comment is an
/// indirect object of the FDF, so a reply's `/IRT` can point at its parent's object there
/// (§12.7.8.3.1). Nested values are copied and renumbered, including shared or cyclic graphs.
/// Source `/Page` and `/Pages` dictionaries become null at every copy depth.
/// Refuses exports above the object, value, direct-depth or data budget instead of losing data.
pub fn export_fdf(doc: &Document, comments: bool, fields: bool, file: &str) -> Result<Vec<u8>, DataError> {
    let mut copy = FdfCopy::new(doc);
    copy.charge(file.len().saturating_mul(8))?;
    let mut body: Vec<u8> = b"<< /FDF << ".to_vec();
    // Comment dictionaries, written after the root as objects 2, 3, …
    if fields {
        body.extend_from_slice(b"/Fields [");
        for f in pdfcraft_forms::fields(doc) {
            if matches!(f.kind, FieldKind::PushButton | FieldKind::Signature) {
                continue;
            }
            // Field dictionary, /T string and /V scalar/array, plus every emitted list
            // entry. Other field kinds emit only their first value (or a default).
            let field_nodes = if matches!(f.kind, FieldKind::List) && f.value.len() > 1 { f.value.len().saturating_add(3) } else { 3 };
            copy.nodes = copy.nodes.checked_sub(field_nodes).ok_or(DataError::ExportLimit("100,000 values"))?;
            copy.charge(f.name.len().saturating_mul(8).saturating_add(100))?;
            for value in &f.value {
                copy.charge(value.len().saturating_mul(8).saturating_add(100))?;
            }
            body.extend_from_slice(b"<< /T ");
            body.extend(pdf_string(&f.name));
            body.extend_from_slice(b" /V ");
            match f.kind {
                FieldKind::CheckBox | FieldKind::Radio => {
                    let v = f.value.first().cloned().unwrap_or_else(|| "Off".into());
                    pdfcraft_cos::serialize(&Object::Name(pdfcraft_forms::name_bytes(&v)), &mut body);
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
        // Keep the page/array owners once. Handles retain positions, never annotation
        // bodies: the document cache can evict and reparse even a repeated source reference.
        let pages = pdfcraft_model::pages(doc);
        let mut arrays = Vec::new();
        let mut picked: Vec<(Option<ObjRef>, usize, usize)> = Vec::new();
        for (pi, page) in pages.iter().enumerate() {
            let owner = page.dict.get(b"Annots").map(|value| FdfValue::resolve(doc, value));
            let before = picked.len();
            if let Some(values) = owner.as_ref().and_then(|owner| owner.object().as_array()) {
                for (ai, a) in values.iter().enumerate() {
                    let value = FdfValue::resolve(doc, a);
                    let Some(d) = value.object().as_dict() else { continue };
                    if d.name(b"Subtype").is_some_and(|sub| SUBTYPES.iter().any(|(s, _)| s.as_bytes() == sub)) {
                        if picked.len() >= MAX_FDF_OBJECTS {
                            return Err(DataError::ExportLimit("100,000 comments"));
                        }
                        picked.push((a.as_ref(), pi, ai));
                    }
                }
            }
            // Empty/unselected arrays need no owner. Resolve subsequent pages only after
            // this page passes the cap, rather than loading them all before discovery.
            arrays.push(if picked.len() > before { owner } else { None });
        }
        let mut numbers = HashMap::new();
        for (source, _, _) in &picked {
            let (_, number) = copy.slot()?;
            if let Some(source) = source {
                numbers.insert(*source, number);
            }
        }
        copy.numbers = numbers.clone();
        let count = picked.len();
        for (index, (_, pi, ai)) in picked.into_iter().enumerate() {
            let annotation = arrays
                .get(pi)
                .and_then(Option::as_ref)
                .and_then(|owner| owner.object().as_array())
                .and_then(|values| values.get(ai))
                .ok_or_else(|| DataError::Malformed("FDF annotation slot is missing".into()))?;
            let value = FdfValue::resolve(doc, annotation);
            let d = value.object().as_dict().ok_or_else(|| DataError::Malformed("FDF comment is not a dictionary".into()))?;
            // A reply points at its parent's object in this FDF; a parent that isn't exported
            // leaves the reply unthreaded.
            let irt = d.get(b"IRT").and_then(Object::as_ref).and_then(|r| numbers.get(&r)).copied();
            let mut d = copy.annotation_dict(d)?;
            if let Some(num) = irt {
                d.set(b"IRT".to_vec(), Object::Ref(ObjRef::new(num, 0)));
            }
            d.set(b"Page".to_vec(), Object::Int(pi as i64));
            let slot = copy.objects.get_mut(index).ok_or_else(|| DataError::Malformed("FDF comment slot is missing".into()))?;
            *slot = Object::Dict(d);
        }
        body.extend_from_slice(b"/Annots [");
        for i in 0..count {
            let _ = write!(body, "{} 0 R ", i + 2);
        }
        body.extend_from_slice(b"] ");
    }
    body.extend_from_slice(b"/F ");
    body.extend(pdf_string(file));
    body.extend_from_slice(b" >> >>");
    copy.finish(doc)?;
    let mut out = b"%FDF-1.2\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(copy.objects.len() + 1);
    offsets.push(out.len());
    out.extend_from_slice(b"1 0 obj\n");
    out.extend(body);
    out.extend_from_slice(b"\nendobj\n");
    for (i, object) in copy.objects.into_iter().enumerate() {
        offsets.push(out.len());
        let _ = writeln!(out, "{} 0 obj", i + 2);
        pdfcraft_cos::serialize(&object, &mut out);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
    for o in offsets {
        let _ = writeln!(out, "{o:010} 00000 n ");
    }
    let _ = write!(out, "trailer\n<< /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n");
    Ok(out)
}
/// XML 1.0 fifth-edition NameStartChar, excluding `:` for an unprefixed NCName.
/// https://www.w3.org/TR/2008/REC-xml-20081126/#NT-NameStartChar
fn xml_name_start(c: char) -> bool {
    matches!(
        c,
        '_' | 'A'..='Z' | 'a'..='z'
            | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
            | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
            | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
            | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
            | '\u{10000}'..='\u{EFFFF}'
    )
}

fn xml_name_char(c: char) -> bool {
    xml_name_start(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// Keep the existing letters/digits/`_`/`-`/`.` normalization, constrained to valid NCNames.
fn xml_name(name: &str) -> String {
    let mut s: String =
        name.chars().map(|c| if (c.is_alphanumeric() && xml_name_char(c)) || matches!(c, '_' | '-' | '.') { c } else { '_' }).collect();
    if !s.chars().next().is_some_and(|c| (c.is_alphabetic() && xml_name_start(c)) || c == '_') {
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
        Format::Xml | Format::Xdp => {
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
            let q = |s: &str| if s.contains([',', '"', '\n', '\r', '\t']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
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
        .fold(0, |bits, flag| bits | flag);
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
    if let Some(it) = node.attribute("intent").filter(|it| !it.is_empty()) {
        d.set(b"IT".to_vec(), Object::name(it));
    }
    if node.attribute("style") == Some("cloudy") {
        let mut be = Dict::new();
        be.set(b"S".to_vec(), Object::name("C"));
        let intensity = node.attribute("intensity").and_then(|i| i.parse::<f64>().ok()).filter(|i| i.is_finite()).unwrap_or(1.0);
        be.set(b"I".to_vec(), Object::Real(intensity.clamp(0.0, 2.0)));
        d.set(b"BE".to_vec(), Object::Dict(be));
    }
    if let Some(cl) = node.attribute("callout").map(csv).filter(|c| c.len() == 4 || c.len() == 6) {
        d.set(b"CL".to_vec(), arr(&cl));
        if let Some(rd) = node.attribute("fringe").map(csv).filter(|r| r.len() == 4) {
            d.set(b"RD".to_vec(), arr(&rd));
        }
        d.set(b"IT".to_vec(), Object::name(node.attribute("intent").unwrap_or("FreeTextCallout")));
        // A callout's /LE is one name, not the head/tail pair of a line.
        match node.attribute("head") {
            Some(h) => d.set(b"LE".to_vec(), Object::name(h)),
            None => {
                d.remove(b"LE");
            }
        }
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
            "vertices" => {
                // Whole x,y pairs only; a stray trailing number is dropped.
                let mut v = csv(child.text().unwrap_or(""));
                v.truncate(v.len() - v.len() % 2);
                if !v.is_empty() {
                    d.set(b"Vertices".to_vec(), arr(&v));
                }
            }
            _ => {}
        }
    }
    read_own_keys(node, &mut d);
    Some(d)
}

/// Put an annotation dictionary on a page, replacing one with the same `/NM`. Returns its
/// reference.
/// `/F` bits an imported file may not set on a redaction mark: Invisible, Hidden, NoView — so a
/// mark it brings in is always drawn for the user to review before content is removed.
const REDACT_UNSEEN: i64 = 1 | 2 | 32;
/// `/F` Locked (ISO 32000-2 Table 167): the user protected this annotation from change.
const LOCKED: i64 = 128;

/// An existing annotation an import must never replace, move or unlock: one the user locked, a
/// redaction mark (replacing it would drop the region the user marked to remove), or a form
/// widget or field (replacing it would set the field's value and policy around the form setter).
fn import_protected(doc: &Document, x: &Dict) -> bool {
    let flags = x.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
    flags & LOCKED != 0 || matches!(x.name(b"Subtype"), Some(b"Redact" | b"Widget")) || x.contains(b"FT")
}

fn place(doc: &mut Document, page_ref: ObjRef, mut d: Dict) -> Result<ObjRef, DataError> {
    // A redaction mark the import carries is always made visible: clear any Invisible/Hidden/NoView
    // flag so it cannot remove content the user never sees.
    if d.name(b"Subtype") == Some(b"Redact".as_slice()) {
        let f = d.int(b"F").unwrap_or(0);
        if f & REDACT_UNSEEN != 0 {
            d.set(b"F".to_vec(), Object::Int(f & !REDACT_UNSEEN));
        }
        // Its review outline is drawn opaque, and from the area it removes: applying a mark
        // without usable /QuadPoints removes its /Rect, which the outline is then drawn from.
        d.remove(b"CA");
        d.remove(b"ca");
        let quads_drawable = d.get(b"QuadPoints").and_then(Object::as_array).is_some_and(|q| {
            let v: Vec<f64> = q.iter().filter_map(Object::as_f64).filter(|v| v.is_finite()).collect();
            let (quads, rest) = v.as_chunks::<8>();
            v.len() == q.len() && rest.is_empty() && !quads.is_empty() && quads.iter().all(quad_has_area)
        });
        if !quads_drawable {
            // Quads the outline can't draw, or any quad that encloses no area, are replaced by the
            // /Rect's, or dropped when there is no usable /Rect either; such a mark then removes
            // nothing.
            match rect_strict(doc, d.get(b"Rect")) {
                Some([x0, y0, x1, y1]) => {
                    let (l, r, b, t) = (x0.min(x1), x0.max(x1), y0.min(y1), y0.max(y1));
                    d.set(b"QuadPoints".to_vec(), arr(&[l, t, r, t, l, b, r, b]));
                }
                _ => {
                    d.remove(b"QuadPoints");
                }
            }
        }
    }
    let nm = d.get(b"NM").and_then(|n| n.as_string().map(|s| s.to_text()));
    let annots: Vec<Object> = doc
        .get(page_ref)
        .as_dict()
        .and_then(|p| p.get(b"Annots").cloned())
        .map(|a| doc.resolve(&a))
        .and_then(|a| a.as_array().cloned())
        .unwrap_or_default();
    let mut kept = Vec::new();
    let mut removed: Vec<ObjRef> = Vec::new();
    // Never let a same-name import replace a protected annotation (`import_protected`): keep it,
    // and add the imported dict beside it under no name of its own.
    let mut clash = false;
    for a in annots {
        let existing = a.as_ref().and_then(|r| doc.get(r).as_dict().cloned());
        let same = nm.is_some() && existing.as_ref().and_then(|x| x.get(b"NM").and_then(|n| doc.resolve(n).as_string().map(|s| s.to_text()))) == nm;
        if same && existing.as_ref().is_some_and(|x| import_protected(doc, x)) {
            clash = true;
            kept.push(a);
        } else if same {
            removed.extend(a.as_ref());
        } else {
            kept.push(a);
        }
    }
    if clash {
        d.remove(b"NM");
    }
    // The replaced note's pop-up goes with it; otherwise it stays on the page pointing at a note that is no longer there.
    kept.retain(|a| {
        let Some(x) = a.as_ref().map(|r| doc.get(r)).and_then(|o| o.as_dict().cloned()) else { return true };
        x.name(b"Subtype") != Some(b"Popup".as_slice()) || x.reference(b"Parent").is_none_or(|p| !removed.contains(&p))
    });
    // Keep the replaced comment's identity: replies and review statuses not included in a
    // partial import still point at it through /IRT. Allocating a new object would detach
    // those comments and leave their parent outside the page's annotation list.
    let r = match removed.first().copied() {
        Some(r) => {
            doc.set(r, Object::Dict(d));
            r
        }
        None => doc.add(Object::Dict(d)),
    };
    // Our own appearance where we can draw one; others keep none (viewers draw a default).
    let _ = pdfcraft_annot::set_appearance(doc, r);
    kept.push(Object::Ref(r));
    doc.update_dict(page_ref, |p| p.set(b"Annots".to_vec(), Object::Array(kept)))?;
    Ok(r)
}

/// Import XFDF comments as copies: every comment lands on `page` (0-based; its own page when
/// `None`) and, with `fresh` (a prefix unique to this import), under a new name, so nothing in
/// the document is replaced and pasting the same comments twice gives two copies. Replies keep
/// their thread among the pasted comments; a reply to a comment that was not copied stands
/// alone. Field values in the file are ignored.
pub fn import_comments(
    doc: &mut Document,
    text: &str,
    page: Option<usize>,
    fresh: Option<&str>,
    offset: Option<[f64; 2]>,
) -> Result<Report, DataError> {
    import_xfdf_with(doc, text, page, fresh, offset.filter(|o| o.iter().all(|v| v.is_finite())))
}

fn import_xfdf(doc: &mut Document, text: &str) -> Result<Report, DataError> {
    import_xfdf_with(doc, text, None, None, None)
}

fn import_xfdf_with(
    doc: &mut Document,
    text: &str,
    target_page: Option<usize>,
    fresh: Option<&str>,
    offset: Option<[f64; 2]>,
) -> Result<Report, DataError> {
    check_form_xml_depth(text)?;
    let xml = parse_form_xml(text).map_err(|e| DataError::Malformed(e.to_string()))?;
    let root = xml.root_element();
    if root.tag_name().name() != "xfdf" {
        return Err(DataError::UnknownFormat);
    }
    let mut report = Report::default();
    let pages: Vec<ObjRef> = pdfcraft_model::pages(doc).iter().map(|p| p.obj).collect();
    if let Some(p) = target_page
        && p >= pages.len()
    {
        return Err(DataError::Malformed(format!("page {} is not in the document", p + 1)));
    }
    // Comments: parents first, then replies (whose /IRT needs the parent's reference).
    let mut by_name: HashMap<String, ObjRef> = HashMap::new();
    let mut later: HashMap<ObjRef, String> = HashMap::new();
    // Fresh names: every copy gets one from its own position, and `name` → the first copy's,
    // so replies find their copied parent (a name two source comments share, as other tools
    // write, still gives two copies).
    let mut renamed: HashMap<String, String> = HashMap::new();
    if let (Some(prefix), Some(annots)) = (fresh, root.children().find(|c| c.has_tag_name("annots"))) {
        for (n, node) in annots.children().filter(|c| c.is_element()).enumerate() {
            if let Some(nm) = node.attribute("name") {
                renamed.entry(nm.to_string()).or_insert_with(|| format!("{prefix}-{}", n + 1));
            }
        }
    }
    if let Some(annots) = root.children().find(|c| c.has_tag_name("annots")) {
        for (n, node) in annots.children().filter(|c| c.is_element()).enumerate() {
            let tag = node.tag_name().name().to_ascii_lowercase();
            let Some((subtype, _)) = SUBTYPES.iter().find(|(_, t)| *t == tag) else { continue };
            let own_page = node.attribute("page").and_then(|p| p.parse::<usize>().ok());
            let Some(page) = target_page.or(own_page).and_then(|p| pages.get(p).copied()) else { continue };
            let Some(mut d) = annot_from_xml(node, subtype, page) else { continue };
            let assigned = fresh.map(|prefix| format!("{prefix}-{}", n + 1));
            if let Some(name) = &assigned {
                // A copy under a name of its own (one even where the original had none).
                d.set(b"NM".to_vec(), PdfString::text(name));
            }
            if let Some([dx, dy]) = offset {
                translate(&mut d, dx, dy);
            }
            let r = place(doc, page, d)?;
            // A later same-name record replaces this comment, including its reply relation.
            later.remove(&r);
            if let Some(name) = assigned.clone().or_else(|| node.attribute("name").map(str::to_owned)) {
                by_name.insert(name, r);
            }
            if let Some(irt) = node.attribute("inreplyto") {
                match (fresh, renamed.get(irt)) {
                    // The parent is among the copies: the reply follows it.
                    (Some(_), Some(parent)) => later.insert(r, parent.clone()),
                    // A copy's parent was not copied: the reply stands alone rather than
                    // attaching to a same-named comment the document happens to have.
                    (Some(_), None) => None,
                    (None, _) => later.insert(r, irt.to_string()),
                };
            }
            // A pop-up window.
            if let Some(pop) = node.children().find(|c| c.has_tag_name("popup"))
                && let Some(mut rect) = pop.attribute("rect").map(csv).filter(|r| r.len() == 4)
            {
                if let Some([dx, dy]) = offset {
                    for (i, v) in rect.iter_mut().enumerate() {
                        *v += if i % 2 == 0 { dx } else { dy };
                    }
                }
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
    // Comments placed as copies: the file's field values are not part of the paste.
    if fresh.is_some() || target_page.is_some() || offset.is_some() {
        return Ok(report);
    }
    // Fields.
    if let Some(fields) = root.children().find(|c| c.has_tag_name("fields")) {
        let mut values = Vec::new();
        walk_xfdf(fields, "", &mut values, 0)?;
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

/// What one FDF import may copy out of the file. A field tree or comment that repeats a reference
/// level after level would otherwise expand a small file into millions of values.
const MAX_FDF_IMPORT_VALUES: usize = 1_000_000;

struct ImportBudget {
    values: usize,
    bytes: usize,
}

impl ImportBudget {
    fn new() -> Self {
        Self { values: MAX_FDF_IMPORT_VALUES, bytes: MAX_FDF_BYTES }
    }

    fn spend(&mut self, values: usize, bytes: usize) -> Result<(), DataError> {
        match (self.values.checked_sub(values), self.bytes.checked_sub(bytes)) {
            (Some(v), Some(b)) => {
                (self.values, self.bytes) = (v, b);
                Ok(())
            }
            _ => Err(DataError::Malformed(format!(
                "the file expands to more than {MAX_FDF_IMPORT_VALUES} values or {} MiB of text",
                MAX_FDF_BYTES >> 20
            ))),
        }
    }
}

/// Copy an object out of another document, resolving references. A reference back to an object
/// being copied becomes null.
fn deep(src: &Document, o: &Object, depth: usize, path: &mut Vec<ObjRef>, budget: &mut ImportBudget) -> Result<Object, DataError> {
    budget.spend(1, 0)?;
    if depth > 16 {
        return Ok(Object::Null);
    }
    Ok(match o {
        Object::Ref(r) => {
            if path.contains(r) {
                return Ok(Object::Null);
            }
            path.push(*r);
            let copy = deep(src, &src.get(*r), depth + 1, path, budget);
            path.pop();
            copy?
        }
        Object::Array(a) => Object::Array(a.iter().map(|x| deep(src, x, depth + 1, path, budget)).collect::<Result<_, _>>()?),
        Object::Dict(d) => {
            Object::Dict(d.iter().map(|(k, v)| Ok((k.clone(), deep(src, v, depth + 1, path, budget)?))).collect::<Result<_, DataError>>()?)
        }
        Object::Stream(_) => Object::Null,
        Object::String(s) => {
            budget.spend(0, s.bytes.len())?;
            o.clone()
        }
        Object::Name(n) => {
            budget.spend(0, n.len())?;
            o.clone()
        }
        other => other.clone(),
    })
}

fn walk_xfdf(node: roxmltree::Node, prefix: &str, out: &mut Vec<(String, Vec<String>)>, depth: usize) -> Result<(), DataError> {
    if depth > MAX_DEPTH {
        return Err(DataError::Malformed(format!("XFDF fields nest more than {MAX_DEPTH} deep")));
    }
    for f in node.children().filter(|c| c.has_tag_name("field")) {
        let Some(name) = f.attribute("name") else { continue };
        let full = if prefix.is_empty() { name.to_string() } else { format!("{prefix}.{name}") };
        let vals: Vec<String> = f
            .children()
            .filter(|c| c.has_tag_name("value"))
            // Comments and processing instructions can split a value into several text nodes.
            .map(|v| v.children().filter(|n| n.is_text()).filter_map(|n| n.text()).collect())
            .collect();
        if !vals.is_empty() {
            out.push((full.clone(), vals));
        }
        walk_xfdf(f, &full, out, depth + 1)?;
    }
    Ok(())
}

/// Read an FDF field tree. A field listed again beneath itself is skipped.
fn walk_fdf(
    fdf: &Document,
    list: &[Object],
    prefix: &str,
    out: &mut Vec<(String, Vec<String>)>,
    depth: usize,
    path: &mut Vec<ObjRef>,
    budget: &mut ImportBudget,
) -> Result<(), DataError> {
    if depth > 32 {
        return Ok(());
    }
    for f in list {
        budget.spend(1, 0)?;
        let held;
        let field = match f {
            Object::Ref(r) if path.contains(r) => continue,
            Object::Ref(r) => {
                held = fdf.get(*r);
                &*held
            }
            direct => direct,
        };
        let Some(d) = field.as_dict() else { continue };
        let Some(t) = d.get(b"T").and_then(|t| fdf.resolve(t).as_string().map(|s| s.to_text())) else { continue };
        let full = if prefix.is_empty() { t } else { format!("{prefix}.{t}") };
        budget.spend(0, full.len())?;
        if let Some(v) = d.get(b"V").map(|v| fdf.resolve(v)) {
            let vals: Vec<String> = match &*v {
                Object::String(s) => vec![s.to_text()],
                Object::Name(n) => vec![pdfcraft_forms::name_text(n)],
                Object::Array(a) => {
                    budget.spend(a.len(), 0)?;
                    a.iter().filter_map(|x| fdf.resolve(x).as_string().map(|s| s.to_text())).collect()
                }
                _ => Vec::new(),
            };
            budget.spend(vals.len(), vals.iter().map(String::len).fold(0, usize::saturating_add))?;
            out.push((full.clone(), vals));
        }
        if let Some(kids) = d.get(b"Kids").map(|k| fdf.resolve(k))
            && let Some(kids) = kids.as_array()
        {
            let entered = f.as_ref().map(|r| path.push(r)).is_some();
            let walked = walk_fdf(fdf, kids, &full, out, depth + 1, path, budget);
            if entered {
                path.pop();
            }
            walked?;
        }
    }
    Ok(())
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
        let mut budget = ImportBudget::new();
        if let Some(list) = body.get(b"Fields").map(|f| fdf.resolve(f)).and_then(|f| f.as_array().cloned()) {
            walk_fdf(&fdf, &list, "", &mut values, 0, &mut Vec::new(), &mut budget)?;
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
    check_form_xml_depth(t)?;
    let xml = parse_form_xml(t).map_err(|_| DataError::UnknownFormat)?;
    let root = xml.root_element();
    if root.tag_name().name() != "xfdf" {
        return Err(DataError::UnknownFormat);
    }
    let mut values = Vec::new();
    if let Some(fields) = root.children().find(|c| c.has_tag_name("fields")) {
        walk_xfdf(fields, "", &mut values, 0)?;
    }
    Ok(values)
}

/// Merge Data Files into Spreadsheet: one CSV row per file, one column per field name (in the
/// order they first appear).
///
/// The names and values come from other people's files, and spreadsheet programs evaluate a cell
/// that starts with `=`, `+`, `-` or `@` (after any leading whitespace) as a formula. Such a cell
/// is written with a leading `'`, which keeps it text; a plain number such as `-12.5` is not.
pub fn merge_csv(files: &[Vec<(String, Vec<String>)>]) -> String {
    let mut columns: Vec<&str> = Vec::new();
    for f in files {
        for (k, _) in f {
            if !columns.contains(&k.as_str()) {
                columns.push(k);
            }
        }
    }
    let literal = |s: &str| {
        let t = s.trim_start();
        let formula = match t.chars().next() {
            Some('=' | '@') => true,
            Some('+' | '-') => !t.parse::<f64>().is_ok_and(f64::is_finite),
            _ => false,
        };
        if formula { format!("'{s}") } else { s.to_string() }
    };
    let q = |s: &str| {
        let s = literal(s);
        if s.contains([',', '"', '\n', '\r', '\t']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
    };
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
    let mut budget = ImportBudget::new();
    // Fields (with /Kids for hierarchical names).
    let mut values = Vec::new();
    if let Some(list) = body.get(b"Fields").map(|f| fdf.resolve(f)).and_then(|f| f.as_array().cloned()) {
        walk_fdf(&fdf, &list, "", &mut values, 0, &mut Vec::new(), &mut budget)?;
    }
    // Comments: parents first, then replies (whose /IRT needs the parent's new reference).
    let pages: Vec<ObjRef> = pdfcraft_model::pages(doc).iter().map(|p| p.obj).collect();
    let mut placed: HashMap<ObjRef, ObjRef> = HashMap::new();
    let mut later: HashMap<ObjRef, (Option<ObjRef>, Option<String>)> = HashMap::new();
    for a in body.get(b"Annots").map(|a| fdf.resolve(a)).and_then(|a| a.as_array().cloned()).unwrap_or_default() {
        let Some(mut src) = fdf.resolve(&a).as_dict().cloned() else { continue };
        // The parent is another comment: link it below rather than copying it in.
        let irt = src.get(b"IRT").cloned();
        src.remove(b"IRT");
        let mut path: Vec<ObjRef> = a.as_ref().into_iter().collect();
        let Object::Dict(mut d) = deep(&fdf, &Object::Dict(src), 0, &mut path, &mut budget)? else { continue };
        let Some(page) = d.get(b"Page").and_then(Object::as_int).and_then(|p| usize::try_from(p).ok()).and_then(|p| pages.get(p).copied()) else {
            continue;
        };
        if !d.name(b"Subtype").is_some_and(|s| SUBTYPES.iter().any(|(x, _)| x.as_bytes() == s)) {
            continue;
        }
        d.remove(b"Page");
        d.set(b"P".to_vec(), Object::Ref(page));
        let r = place(doc, page, d)?;
        later.remove(&r);
        if let Some(fr) = a.as_ref() {
            placed.insert(fr, r);
        }
        if let Some(irt) = irt {
            let parent_name = fdf.resolve(&irt).as_dict().and_then(|p| text_of(&fdf, p, b"NM"));
            later.insert(r, (irt.as_ref(), parent_name));
        }
        report.comments += 1;
    }
    for (r, (parent_obj, parent_name)) in later {
        // The parent imported from this file, else one already in the document with its name.
        let parent = parent_obj.and_then(|p| placed.get(&p).copied()).or_else(|| parent_name.and_then(|nm| find_by_name(doc, &nm)));
        if let Some(parent) = parent.filter(|p| *p != r) {
            doc.update_dict(r, |d| d.set(b"IRT".to_vec(), Object::Ref(parent)))?;
        }
    }
    apply_values(doc, &values, &mut report);
    Ok(report)
}

// Inspect the first logical record: tabs within quoted CSV cells are data. A quote
// in the middle of an unquoted TSV name stays literal, as in the TSV reader.
fn has_tab_delimiter(text: &str) -> bool {
    let mut quoted = false;
    let mut cell_start = true;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
            }
            '"' if quoted || cell_start => {
                quoted = !quoted;
                cell_start = false;
            }
            '\t' if !quoted => return true,
            '\r' | '\n' if !quoted => return false,
            ',' if !quoted => cell_start = true,
            _ => cell_start = false,
        }
    }
    // An unclosed quote can be a literal TSV field-name character.
    quoted && text.lines().next().is_some_and(|line| line.contains('\t'))
}

/// Form data from XML, CSV or tab-delimited text.
fn import_table(doc: &mut Document, text: &str) -> Result<Report, DataError> {
    let mut report = Report::default();
    let t = text.trim_start_matches('\u{feff}');
    let values: Vec<(String, Vec<String>)> = if t.trim_start().starts_with('<') {
        check_form_xml_depth(t)?;
        let xml = parse_form_xml(t).map_err(|e| DataError::Malformed(e.to_string()))?;
        xml.root_element()
            .children()
            .filter(|c| c.is_element())
            .map(|c| {
                let name =
                    c.attributes().find(|a| a.name() == "original").map(|a| a.value().to_string()).unwrap_or_else(|| c.tag_name().name().to_string());
                // Comments and processing instructions do not end a field's character data.
                (name, vec![c.children().filter(|node| node.is_text()).filter_map(|node| node.text()).collect()])
            })
            .collect()
    } else if has_tab_delimiter(t) {
        tab_delimited(t)?
    } else {
        // CSV with quotes: a quoted value may hold commas, doubled quotes and line breaks.
        let mut records: Vec<(Vec<String>, bool)> = Vec::new();
        let mut record = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        let mut has_quotes = false;
        let mut chars = t.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' if quoted && chars.peek() == Some(&'"') => {
                    cur.push('"');
                    chars.next();
                }
                '"' => {
                    quoted = !quoted;
                    has_quotes = true;
                }
                ',' if !quoted => record.push(std::mem::take(&mut cur)),
                '\r' if !quoted && chars.peek() == Some(&'\n') => {}
                '\n' if !quoted => {
                    record.push(std::mem::take(&mut cur));
                    records.push((std::mem::take(&mut record), std::mem::take(&mut has_quotes)));
                }
                c => cur.push(c),
            }
        }
        record.push(cur);
        records.push((record, has_quotes));
        // Blank lines (such as the one a trailing line break leaves) are not records, but a
        // quoted empty value is a record that clears a field.
        let mut records =
            records.into_iter().filter(|(r, has_quotes)| *has_quotes || !matches!(r.as_slice(), [only] if only.is_empty())).map(|(r, _)| r);
        let (Some(head), Some(row)) = (records.next(), records.next()) else { return Err(DataError::UnknownFormat) };
        // Text exported from a one-field form has no tab to show for it (#1321). CSV quotes a
        // value with a comma, so one name over several unquoted cells is that text, not CSV.
        if head.len() == 1 && row.len() > 1 { tab_delimited(t)? } else { head.into_iter().zip(row).map(|(k, v)| (k, vec![v])).collect() }
    };
    apply_values(doc, &values, &mut report);
    Ok(report)
}

/// Tab-delimited text: a row of names, a row of values.
fn tab_delimited(t: &str) -> Result<Vec<(String, Vec<String>)>, DataError> {
    let mut lines = t.lines();
    let (Some(head), Some(row)) = (lines.next(), lines.next()) else { return Err(DataError::UnknownFormat) };
    Ok(head.split('\t').map(str::to_string).zip(row.split('\t')).map(|(k, v)| (k, vec![v.to_string()])).collect())
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
