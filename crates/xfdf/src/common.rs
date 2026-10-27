use super::*;

// ── names ───────────────────────────────────────────────────────────────────────────────────

/// Form data files are attacker-supplied. Refuse a DTD and bound the node count, exactly as the
/// XFA template parser does (`crates/xfa/src/parse.rs`), and bound how deep a nested field tree
/// may go: the walk below recurses once per nesting level.
const MAX_NODES: u32 = 2_000_000;

pub(crate) const MAX_DEPTH: usize = 64;

/// The deepest element nesting accepted before the text reaches the XML parser.
///
/// roxmltree parses by recursive descent, so its stack use grows with the nesting depth: ten
/// thousand levels overflow the main thread's stack and abort the process. A stack overflow is
/// `abort()`, so `catch_unwind` cannot intercept it and the whole application dies - which is why
/// the bound has to be applied before parsing, not while walking the result. Around three thousand
/// levels still parse on the default stack, so this stays clear of the cliff while leaving far more
/// room than the walks below, which cut off at 64.
pub(crate) const MAX_XML_DEPTH: usize = 1_024;

/// Element nesting depth of `text`, skipping comments, CDATA, processing instructions, the DOCTYPE
/// and quoted attribute values. A tag that closes itself (`<field/>`) is one level while it is read
/// and does not stay open, so a long run of them cannot accumulate into the bound. It cannot
/// under-count a real element; a stray `<` in text is invalid XML and only makes it over-count,
/// which errs towards refusing the file.
pub(crate) fn xml_nesting_depth(text: &str) -> usize {
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

pub(crate) fn check_form_xml_depth(text: &str) -> Result<(), DataError> {
    let depth = xml_nesting_depth(text);
    if depth > MAX_XML_DEPTH {
        return Err(DataError::Malformed(format!("the file nests XML elements {depth} deep, past the {MAX_XML_DEPTH} the reader accepts")));
    }
    Ok(())
}

pub(crate) fn parse_form_xml(text: &str) -> Result<roxmltree::Document<'_>, roxmltree::Error> {
    roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES })
}

pub(crate) const SUBTYPES: [(&str, &str); 17] = [
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

pub(crate) const FLAGS: [(i64, &str); 9] = [
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

pub(crate) fn esc(s: &str) -> String {
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

pub(crate) fn n(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

pub(crate) fn hex(c: &[f64]) -> Option<String> {
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

pub(crate) fn parse_hex(s: &str) -> Option<[f64; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let p = |i: usize| h.get(i..i + 2).and_then(|s| u8::from_str_radix(s, 16).ok()).map(|v| f64::from(v) / 255.0);
    Some([p(0)?, p(2)?, p(4)?])
}

pub(crate) fn nums_of(doc: &Document, o: Option<&Object>) -> Vec<f64> {
    o.map(|o| doc.resolve(o)).and_then(|a| a.as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect())).unwrap_or_default()
}

/// A `/Rect` of exactly four finite numbers; any other entry makes the whole rectangle unusable.
pub(crate) fn rect_strict(doc: &Document, o: Option<&Object>) -> Option<[f64; 4]> {
    let num = |x: &Object| doc.resolve(x).as_f64().filter(|v| v.is_finite());
    match doc.resolve(o?).as_array()?.as_slice() {
        [x0, y0, x1, y1] => Some([num(x0)?, num(y0)?, num(x1)?, num(y1)?]),
        _ => None,
    }
}

/// Whether a quad's four corners enclose any area: some three of them are not collinear.
pub(crate) fn quad_has_area(q: &[f64; 8]) -> bool {
    let [ax, ay, bx, by, cx, cy, dx, dy] = *q;
    let cross = |(px, py): (f64, f64), (qx, qy): (f64, f64), (rx, ry): (f64, f64)| (qx - px) * (ry - py) - (qy - py) * (rx - px);
    let (a, b, c, d) = ((ax, ay), (bx, by), (cx, cy), (dx, dy));
    [cross(a, b, c), cross(a, b, d), cross(a, c, d), cross(b, c, d)].iter().any(|v| v.abs() > 0.0)
}

pub(crate) fn text_of(doc: &Document, d: &Dict, k: &[u8]) -> Option<String> {
    d.get(k).and_then(|o| match &*doc.resolve(o) {
        Object::String(s) => Some(s.to_text()),
        Object::Name(n) => Some(pdfcraft_forms::name_text(n)),
        _ => None,
    })
}

/// The numbers of a list; empty when any entry is non-finite (`nan`, `inf`), so the list is unusable as a whole.
pub(crate) fn csv(list: &str) -> Vec<f64> {
    let v: Vec<f64> = list.split([',', ';', ' ']).filter(|t| !t.is_empty()).filter_map(|t| t.trim().parse().ok()).collect();
    if v.iter().all(|x| x.is_finite()) { v } else { Vec::new() }
}

pub(crate) fn arr(v: &[f64]) -> Object {
    Object::Array(v.iter().map(|x| Object::Real(*x)).collect())
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
pub(crate) fn xml_name(name: &str) -> String {
    let mut s: String =
        name.chars().map(|c| if (c.is_alphanumeric() && xml_name_char(c)) || matches!(c, '_' | '-' | '.') { c } else { '_' }).collect();
    if !s.chars().next().is_some_and(|c| (c.is_alphabetic() && xml_name_start(c)) || c == '_') {
        s.insert(0, '_');
    }
    s
}

pub(crate) fn walk_xfdf(node: roxmltree::Node, prefix: &str, out: &mut Vec<(String, Vec<String>)>, depth: usize) -> Result<(), DataError> {
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
        // A leaf with no `<value>` is an empty field (the export writes one that way), so it
        // clears the field like FDF's `/V ()`; a field with kids only names their parent. A
        // leaf holding only `<value-richtext>` is not read yet, so it leaves the field alone.
        let names_kids_or_rich = f.children().any(|c| c.has_tag_name("field") || c.has_tag_name("value-richtext"));
        if !vals.is_empty() || !names_kids_or_rich {
            out.push((full.clone(), vals));
        }
        walk_xfdf(f, &full, out, depth + 1)?;
    }
    Ok(())
}
