use super::common::{
    FLAGS, MAX_DEPTH, SUBTYPES, arr, check_form_xml_depth, csv, esc, hex, n, nums_of, parse_form_xml, parse_hex, quad_has_area, rect_strict, text_of,
    walk_xfdf,
};
use super::*;

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
pub(crate) const PDFCRAFT_NS: &str = "https://github.com/storytold/pdfcraft#xfdf";
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

/// The name of every annotation, for `inreplyto`: its /NM, or, for one in `only` without one
/// (other tools leave /NM out), a name new to the document for this export, so a reply copied
/// with it still finds it.
fn annotation_names(doc: &Document, pages: &[pdfcraft_model::Page], only: Option<&HashSet<(usize, usize)>>) -> HashMap<ObjRef, String> {
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
    names
}

/// The elements inside a comment's XFDF element: its text, default appearance, ink, vertices
/// and pop-up (on page `pi`).
fn xfdf_annot_children(doc: &Document, out: &mut String, d: &Dict, pi: usize) {
    if let Some(c) = text_of(doc, d, b"Contents") {
        let _ = write!(out, "<contents>{}</contents>", esc(&c));
    }
    if let Some(da) = text_of(doc, d, b"DA") {
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
}

/// `own`: write each comment's PdfCraft keys too (see [`own_keys`]), for a copy and paste.
fn xfdf_annots(doc: &Document, out: &mut String, only: Option<&HashSet<(usize, usize)>>, own: bool) {
    let pages = pdfcraft_model::pages(doc);
    // NM of every annotation, for inreplyto. A selected annotation without one (other tools
    // leave /NM out) gets a name for this export, so a reply copied with it still finds it.
    let names = annotation_names(doc, &pages, only);
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
            xfdf_annot_children(doc, out, &d, pi);
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
            _ => f.export_values(),
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

pub(crate) fn import_xfdf(doc: &mut Document, text: &str) -> Result<Report, DataError> {
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
