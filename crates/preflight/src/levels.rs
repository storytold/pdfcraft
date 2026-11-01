//! What PDF/A conformance levels u and a add to level b (ISO 19005-2; the clauses are those
//! veraPDF's public rule list gives, as the ISO text isn't available here):
//!
//! - u and a: every character the document shows maps to Unicode, and never to U+0000, U+FEFF or
//!   U+FFFE (6.2.11.7.2). The map is the one PdfCraft's text extraction uses: the font's
//!   `ToUnicode`, else its encoding and glyph names (ISO 32000-2 9.10.2).
//! - a: text in the Private Use Area has `ActualText` around it (6.2.11.7.3; marked content only,
//!   not `ActualText` on structure elements); the document is tagged: `MarkInfo` `/Marked true`
//!   (6.7.2.2) and a structure tree in `StructTreeRoot` (6.7.3.3) whose types are standard or
//!   mapped to standard ones, without cycles, and without standard types mapped away (6.7.3.4);
//!   language tags are well formed (6.7.4).

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use pdfcraft_fonts::pdf::Metrics;

use crate::walk;

/// What a level-u or level-a check found: clause, wording, the document's name for it, fixable.
pub(crate) type Found = (&'static str, &'static str, Option<String>, bool);

/// The standard structure types (ISO 32000-1 14.8.4).
const STANDARD: &[&[u8]] = &[
    b"Document",
    b"Part",
    b"Art",
    b"Sect",
    b"Div",
    b"BlockQuote",
    b"Caption",
    b"TOC",
    b"TOCI",
    b"Index",
    b"NonStruct",
    b"Private",
    b"P",
    b"H",
    b"H1",
    b"H2",
    b"H3",
    b"H4",
    b"H5",
    b"H6",
    b"L",
    b"LI",
    b"Lbl",
    b"LBody",
    b"Table",
    b"TR",
    b"TH",
    b"TD",
    b"THead",
    b"TBody",
    b"TFoot",
    b"Span",
    b"Quote",
    b"Note",
    b"Reference",
    b"BibEntry",
    b"Code",
    b"Link",
    b"Annot",
    b"Ruby",
    b"RB",
    b"RT",
    b"RP",
    b"Warichu",
    b"WT",
    b"WP",
    b"Figure",
    b"Formula",
    b"Form",
];

/// How many structure elements are read, and how deep.
const MAX_ELEMENTS: usize = 1_000_000;
const MAX_STRUCTURE_DEPTH: usize = 512;

/// Per font, what its text needs.
#[derive(Default)]
struct FontUse {
    name: String,
    unmapped: bool,
    invalid: bool,
    private_use: bool,
}

fn private_use(c: char) -> bool {
    matches!(u32::from(c), 0xE000..=0xF8FF | 0xF_0000..=0xF_FFFD | 0x10_0000..=0x10_FFFD)
}

/// Every character content shows maps to Unicode (levels u and a); with `actual_text`, text in
/// the Private Use Area also has `ActualText` (level a).
pub(crate) fn unicode(doc: &Document, actual_text: bool, out: &mut Vec<Found>) {
    let mut fonts: HashMap<Vec<u8>, (Metrics, FontUse)> = HashMap::new();
    let mut order: Vec<Vec<u8>> = Vec::new();
    for c in walk::contents(doc) {
        let data: Vec<Vec<u8>> = c
            .streams
            .iter()
            .filter_map(|r| match &*doc.get(*r) {
                Object::Stream(s) => s.decoded().ok(),
                _ => None,
            })
            .collect();
        let parsed = pdfcraft_content::Pieces::join(&data).parse();
        let font_list = walk::dict_of(doc, c.resources.get(b"Font"));
        let properties = walk::dict_of(doc, c.resources.get(b"Properties"));
        let mut current: Option<Vec<u8>> = None;
        // Marked content: whether each open sequence carries ActualText.
        let mut marked: Vec<bool> = Vec::new();
        let mut saved: Vec<Option<Vec<u8>>> = Vec::new();
        for op in &parsed.ops {
            let strings: Vec<&[u8]> = match op.op.as_slice() {
                b"q" => {
                    saved.push(current.clone());
                    continue;
                }
                b"Q" => {
                    if let Some(f) = saved.pop() {
                        current = f;
                    }
                    continue;
                }
                b"BMC" => {
                    marked.push(false);
                    continue;
                }
                b"BDC" => {
                    let props = match op.operands.get(1) {
                        Some(Object::Name(n)) => properties.get(n).map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned()),
                        Some(Object::Dict(d)) => Some(d.clone()),
                        _ => None,
                    };
                    marked.push(props.is_some_and(|p| p.get(b"ActualText").is_some()));
                    continue;
                }
                b"EMC" => {
                    marked.pop();
                    continue;
                }
                b"Tf" => {
                    current = op.name(0).and_then(|name| {
                        let entry = font_list.get(name)?;
                        // A font is known by its object, or by where it is written.
                        let key = match entry {
                            Object::Ref(r) => format!("{} {}", r.num, r.generation).into_bytes(),
                            _ => [b"inline ".as_slice(), name, format!(" {:?}", c.streams.first()).as_bytes()].concat(),
                        };
                        if !fonts.contains_key(&key) {
                            let dict = doc.resolve(entry).as_dict().cloned()?;
                            let name = dict
                                .name(b"BaseFont")
                                .map(|n| String::from_utf8_lossy(n).into_owned())
                                .unwrap_or_else(|| String::from_utf8_lossy(name).into_owned());
                            fonts.insert(key.clone(), (Metrics::from_dict(doc, &dict), FontUse { name, ..FontUse::default() }));
                            order.push(key.clone());
                        }
                        Some(key)
                    });
                    continue;
                }
                b"Tj" | b"'" => op.operands.first().and_then(Object::as_string).map(|s| vec![s.bytes.as_slice()]).unwrap_or_default(),
                b"\"" => op.operands.get(2).and_then(Object::as_string).map(|s| vec![s.bytes.as_slice()]).unwrap_or_default(),
                b"TJ" => match op.operands.first() {
                    Some(Object::Array(a)) => a.iter().filter_map(Object::as_string).map(|s| s.bytes.as_slice()).collect(),
                    _ => Vec::new(),
                },
                _ => continue,
            };
            let Some((metrics, used)) = current.as_ref().and_then(|k| fonts.get_mut(k)) else { continue };
            let in_actual_text = marked.iter().any(|m| *m);
            for s in strings {
                for (code, _) in metrics.codes(s) {
                    match metrics.text_of(code) {
                        None => used.unmapped = true,
                        Some(t) if t.is_empty() => used.unmapped = true,
                        Some(t) => {
                            if t.chars().any(|ch| matches!(ch, '\0' | '\u{FEFF}' | '\u{FFFE}')) {
                                used.invalid = true;
                            }
                            if actual_text && !in_actual_text && t.chars().any(private_use) {
                                used.private_use = true;
                            }
                        }
                    }
                }
            }
        }
    }
    for key in order {
        let Some((_, used)) = fonts.get(&key) else { continue };
        if used.unmapped {
            out.push(("6.2.11.7.2", "The font {} has characters with no Unicode value", Some(used.name.clone()), false));
        }
        if used.invalid {
            out.push(("6.2.11.7.2", "The font {} maps characters to U+0000, U+FEFF or U+FFFE", Some(used.name.clone()), false));
        }
        if used.private_use {
            out.push(("6.2.11.7.3", "Text in the font {} uses the Private Use Area without ActualText", Some(used.name.clone()), false));
        }
    }
}

/// Whether `tag` is a well-formed language tag (RFC 3066: letters, then subtags of letters and
/// digits, each 1–8 long). Empty is allowed: it says the language is unknown.
fn language_tag(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let primary = parts.next().unwrap_or_default();
    tag.is_empty()
        || (!primary.is_empty() && primary.len() <= 8 && primary.chars().all(|c| c.is_ascii_alphabetic()))
            && parts.all(|p| !p.is_empty() && p.len() <= 8 && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// The document is tagged, its structure types are standard or mapped to standard ones, and its
/// language tags are well formed (level a).
pub(crate) fn structure(doc: &Document, out: &mut Vec<Found>) {
    let cat = crate::catalog(doc);
    let tree = cat.get(b"StructTreeRoot").map(|t| doc.resolve(t)).and_then(|t| t.as_dict().cloned());
    let marked = cat.get(b"MarkInfo").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned()).and_then(|m| m.get(b"Marked").cloned());
    if !matches!(marked, Some(Object::Bool(true))) {
        // With a structure tree, saying so is all that is missing.
        out.push(("6.7.2.2", "The document isn't marked as tagged (MarkInfo /Marked true)", None, tree.is_some()));
    }
    let lang = |o: Option<&Object>| o.map(|l| doc.resolve(l)).and_then(|l| l.as_string().map(|s| s.to_text()));
    if let Some(l) = lang(cat.get(b"Lang"))
        && !language_tag(&l)
    {
        out.push(("6.7.4", "The language {} isn't a valid language tag", Some(l), false));
    }
    let Some(tree) = tree else {
        out.push(("6.7.3.3", "The document has no structure tree: it isn't tagged", None, false));
        return;
    };
    let roles = walk::dict_of(doc, tree.get(b"RoleMap"));
    // The structure types used, and the languages structure elements state.
    let mut types: Vec<Vec<u8>> = Vec::new();
    let mut languages: Vec<String> = Vec::new();
    let mut seen: HashSet<ObjRef> = HashSet::new();
    let mut stack: Vec<(Object, usize)> = vec![(tree.get(b"K").cloned().unwrap_or(Object::Null), 0)];
    let mut count = 0usize;
    while let Some((k, depth)) = stack.pop() {
        count += 1;
        if count > MAX_ELEMENTS || depth > MAX_STRUCTURE_DEPTH {
            break;
        }
        if let Object::Ref(r) = &k
            && !seen.insert(*r)
        {
            continue;
        }
        let o = doc.resolve(&k);
        match &*o {
            Object::Array(a) => stack.extend(a.iter().map(|x| (x.clone(), depth.saturating_add(1)))),
            Object::Dict(d) => {
                // Marked-content and object references are leaves.
                if matches!(d.name(b"Type"), Some(b"MCR" | b"OBJR")) {
                    continue;
                }
                if let Some(s) = d.name(b"S")
                    && !types.iter().any(|t| t == s)
                {
                    types.push(s.to_vec());
                }
                if let Some(l) = lang(d.get(b"Lang"))
                    && !languages.contains(&l)
                {
                    languages.push(l);
                }
                if let Some(kids) = d.get(b"K") {
                    stack.push((kids.clone(), depth.saturating_add(1)));
                }
            }
            _ => {}
        }
    }
    for l in languages {
        if !language_tag(&l) {
            out.push(("6.7.4", "The language {} isn't a valid language tag", Some(l), false));
        }
    }
    let name = |n: &[u8]| String::from_utf8_lossy(n).into_owned();
    for (from, to) in roles.iter() {
        if STANDARD.contains(&from.as_slice()) && !to.as_name().is_some_and(|t| STANDARD.contains(&t)) {
            out.push(("6.7.3.4", "The standard structure type {} is mapped to a type that isn't standard", Some(name(from)), false));
        }
    }
    for t in types {
        let mut at = t.clone();
        let mut path: Vec<Vec<u8>> = Vec::new();
        loop {
            if STANDARD.contains(&at.as_slice()) {
                break;
            }
            if path.contains(&at) {
                out.push(("6.7.3.4", "The role map maps {} in a circle", Some(name(&t)), false));
                break;
            }
            path.push(at.clone());
            match roles.get(&at).and_then(Object::as_name) {
                Some(next) => at = next.to_vec(),
                None => {
                    out.push(("6.7.3.4", "The structure type {} isn't standard and isn't mapped to a standard type", Some(name(&t)), false));
                    break;
                }
            }
        }
    }
}

/// Mark a document with a structure tree as tagged. Returns whether it changed.
pub(crate) fn mark_tagged(doc: &mut Document) -> Result<bool, pdfcraft_cos::CosError> {
    let cat = crate::catalog(doc);
    if cat.get(b"StructTreeRoot").is_none() {
        return Ok(false);
    }
    let Some(root) = doc.root() else { return Ok(false) };
    match cat.get(b"MarkInfo").cloned() {
        Some(Object::Ref(r)) if matches!(&*doc.get(r), Object::Dict(_)) => doc.update_dict(r, |m| m.set(b"Marked".to_vec(), Object::Bool(true)))?,
        Some(Object::Dict(mut m)) => {
            m.set(b"Marked".to_vec(), Object::Bool(true));
            doc.update_dict(root, |c| c.set(b"MarkInfo".to_vec(), Object::Dict(m)))?;
        }
        _ => {
            let mut m = Dict::new();
            m.set(b"Marked".to_vec(), Object::Bool(true));
            doc.update_dict(root, |c| c.set(b"MarkInfo".to_vec(), Object::Dict(m)))?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::language_tag;

    #[test]
    fn language_tag_syntax() {
        for ok in ["", "en", "en-US", "es-419", "x-klingon", "zh-Hant-TW"] {
            assert!(language_tag(ok), "{ok}");
        }
        for bad in ["en_US", "-en", "en-", "englishlanguage", "en-subtagtoolong", "1en", "en--US"] {
            assert!(!language_tag(bad), "{bad}");
        }
    }
}
