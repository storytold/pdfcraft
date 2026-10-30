use super::common::{
    FLAGS, MAX_DEPTH, SUBTYPES, arr, check_form_xml_depth, csv, esc, hex, n, nums_of, parse_form_xml, parse_hex, quad_has_area, rect_strict, text_of,
    walk_xfdf,
};
use super::*;

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
            // A callout: its leader path, text-box inset, intent and (single-name) line ending.
            let cl = nums_of(doc, d.get(b"CL"));
            if cl.len() == 4 || cl.len() == 6 {
                let _ = write!(attrs, " callout=\"{}\"", cl.iter().map(|v| n(*v)).collect::<Vec<_>>().join(","));
                let rd = nums_of(doc, d.get(b"RD"));
                if rd.len() == 4 {
                    let _ = write!(attrs, " fringe=\"{}\"", rd.iter().map(|v| n(*v)).collect::<Vec<_>>().join(","));
                }
                if let Some(it) = d.get(b"IT").map(|o| doc.resolve(o)).and_then(|o| o.as_name().map(|x| String::from_utf8_lossy(x).into_owned())) {
                    let _ = write!(attrs, " intent=\"{}\"", esc(&it));
                }
                if let Some(le) = d.get(b"LE").map(|o| doc.resolve(o)).and_then(|o| o.as_name().map(|x| String::from_utf8_lossy(x).into_owned())) {
                    let _ = write!(attrs, " head=\"{}\"", esc(&le));
                }
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

// ── import ──────────────────────────────────────────────────────────────────────────────────

/// Set the values of fields named in `values` (full names), counting what took.
pub(crate) fn apply_values(doc: &mut Document, values: &[(String, Vec<String>)], report: &mut Report) {
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
    Some(d)
}

/// Put an annotation dictionary on a page, replacing one with the same `/NM`. Returns its
/// reference.
/// `/F` bits an imported file may not set on a redaction mark: Invisible, Hidden, NoView — so a
/// mark it brings in is always drawn for the user to review before content is removed.
const REDACT_UNSEEN: i64 = 1 | 2 | 32;

/// `/F` Locked (ISO 32000-2 Table 167): the user protected this annotation from change.
pub(crate) const LOCKED: i64 = 128;

/// An existing annotation an import must never replace, move or unlock: one the user locked, a
/// redaction mark (replacing it would drop the region the user marked to remove), or a form
/// widget or field (replacing it would set the field's value and policy around the form setter).
fn import_protected(doc: &Document, x: &Dict) -> bool {
    let flags = x.get(b"F").and_then(|f| doc.resolve(f).as_int()).unwrap_or(0);
    flags & LOCKED != 0 || matches!(x.name(b"Subtype"), Some(b"Redact" | b"Widget")) || x.contains(b"FT")
}

pub(crate) fn place(doc: &mut Document, page_ref: ObjRef, mut d: Dict) -> Result<ObjRef, DataError> {
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

/// A pop-up window for the comment `parent`, on its page.
pub(crate) fn add_popup(doc: &mut Document, page: ObjRef, parent: ObjRef, rect: &[f64], open: bool) -> Result<(), DataError> {
    let mut p = Dict::new();
    p.set(b"Type".to_vec(), Object::name("Annot"));
    p.set(b"Subtype".to_vec(), Object::name("Popup"));
    p.set(b"Rect".to_vec(), arr(rect));
    p.set(b"Parent".to_vec(), Object::Ref(parent));
    p.set(b"Open".to_vec(), Object::Bool(open));
    let pr = doc.add(Object::Dict(p));
    doc.update_dict(parent, |d| d.set(b"Popup".to_vec(), Object::Ref(pr)))?;
    doc.update_dict(page, |pd| {
        if let Some(Object::Array(a)) = pd.get_mut(b"Annots") {
            a.push(Object::Ref(pr));
        }
    })?;
    Ok(())
}

pub(crate) fn import_xfdf(doc: &mut Document, text: &str) -> Result<Report, DataError> {
    check_form_xml_depth(text)?;
    let xml = parse_form_xml(text).map_err(|e| DataError::Malformed(e.to_string()))?;
    let root = xml.root_element();
    if root.tag_name().name() != "xfdf" {
        return Err(DataError::UnknownFormat);
    }
    let mut report = Report::default();
    let pages: Vec<ObjRef> = pdfcraft_model::pages(doc).iter().map(|p| p.obj).collect();
    // Comments: parents first, then replies (whose /IRT needs the parent's reference).
    let mut by_name: HashMap<String, ObjRef> = HashMap::new();
    let mut later: HashMap<ObjRef, String> = HashMap::new();
    if let Some(annots) = root.children().find(|c| c.has_tag_name("annots")) {
        for node in annots.children().filter(|c| c.is_element()) {
            let tag = node.tag_name().name().to_ascii_lowercase();
            let Some((subtype, _)) = SUBTYPES.iter().find(|(_, t)| *t == tag) else { continue };
            let Some(page) = node.attribute("page").and_then(|p| p.parse::<usize>().ok()).and_then(|p| pages.get(p).copied()) else { continue };
            let Some(d) = annot_from_xml(node, subtype, page) else { continue };
            let r = place(doc, page, d)?;
            // A later same-name record replaces this comment, including its reply relation.
            later.remove(&r);
            if let Some(nm) = node.attribute("name") {
                by_name.insert(nm.to_string(), r);
            }
            if let Some(irt) = node.attribute("inreplyto") {
                later.insert(r, irt.to_string());
            }
            // A pop-up window.
            if let Some(pop) = node.children().find(|c| c.has_tag_name("popup"))
                && let Some(rect) = pop.attribute("rect").map(csv).filter(|r| r.len() == 4)
            {
                add_popup(doc, page, r, &rect, pop.attribute("open") == Some("yes"))?;
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
        walk_xfdf(fields, "", &mut values, 0)?;
        apply_values(doc, &values, &mut report);
    }
    Ok(report)
}

pub(crate) fn find_by_name(doc: &Document, nm: &str) -> Option<ObjRef> {
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
