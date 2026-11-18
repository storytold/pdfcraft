//! What PDF/A-4 (ISO 19005-4, with its 4e and 4f levels) asks differently from PDF/A-2 and 3.
//!
//! PDF/A-4 is built on PDF 2.0 (ISO 32000-2) and numbers its clauses differently. The rules here
//! come from ISO 32000-2 and from the rule descriptions the veraPDF validator prints (run as an
//! external program on the veraPDF test corpus and on small hand-made files; nothing of its
//! implementation was read). The clause numbers are the ones those reports and the corpus's
//! directory names give.
//!
//! The rules PDF/A-4 shares with parts 2 and 3 are checked by the same code in `lib.rs`; this
//! module renames their clauses ([`clause`]) and holds what is PDF/A-4's own.

use pdfcraft_cos::{Dict, Document, Object};

use crate::{Level, xmp};

/// The PDF/A-4 clause for a rule `lib.rs` cites by its ISO 19005-2 number.
///
/// Every clause a PDF/A-4 verification reports passes through here, so a rule shared with PDF/A-2
/// is cited as PDF/A-4 numbers it. The rules PDF/A-4 dropped (the title kept in step with the
/// document information, the XMP value types and extension schemas, the implementation limits and
/// PostScript XObjects) are not checked for it at all. A clause this module reports is already
/// PDF/A-4's, and none of them is one renamed here.
pub(crate) fn clause(part2: &'static str) -> &'static str {
    match part2 {
        "6.6.2.1" => "6.7.2.1",
        "6.6.4" => "6.7.3",
        "6.2.10" => "6.2.9",
        "6.5.1" => "6.6.1",
        "6.5.2" => "6.6.3",
        "6.2.11.4" => "6.2.10.4.1",
        "6.1.7.1" => "6.1.6.1",
        "6.1.7.2" => "6.1.6.2",
        "6.2.8" => "6.2.7.1",
        "6.8" => "6.9",
        same => same,
    }
}

/// The `pdfaid:rev` of ISO 19005-4:2020, its only edition.
const REVISION: &str = "2020";

/// The actions PDF/A-4 allows that PDF/A-2 forbids: JavaScript, and for PDF/A-4e the actions of
/// optional content and 3D views.
pub(crate) fn allows_action(level: Level, action: &[u8]) -> bool {
    match level {
        Level::A2b | Level::A3b => false,
        Level::A4 | Level::A4f => action == b"JavaScript",
        Level::A4e => matches!(action, b"JavaScript" | b"SetOCGState" | b"GoTo3DView"),
    }
}

/// The annotation types of ISO 32000-2 12.5.6.1, Table 171.
const ANNOTATION_TYPES: &[&[u8]] = &[
    b"Text",
    b"Link",
    b"FreeText",
    b"Line",
    b"Square",
    b"Circle",
    b"Polygon",
    b"PolyLine",
    b"Highlight",
    b"Underline",
    b"Squiggly",
    b"StrikeOut",
    b"Caret",
    b"Stamp",
    b"Ink",
    b"Popup",
    b"FileAttachment",
    b"Sound",
    b"Movie",
    b"Screen",
    b"Widget",
    b"PrinterMark",
    b"TrapNet",
    b"Watermark",
    b"3D",
    b"Redact",
    b"Projection",
    b"RichMedia",
];

/// Whether PDF/A-4 at `level` forbids annotations of type `subtype`: types ISO 32000-2 doesn't
/// define, and sound, movie, screen, 3D, rich media and file attachment annotations, except 3D and
/// rich media in PDF/A-4e and file attachments in PDF/A-4f.
pub(crate) fn forbids_annotation(level: Level, subtype: &[u8]) -> bool {
    match subtype {
        b"Sound" | b"Movie" | b"Screen" => true,
        b"3D" | b"RichMedia" => level != Level::A4e,
        b"FileAttachment" => level != Level::A4f,
        other => !ANNOTATION_TYPES.contains(&other),
    }
}

/// 6.3.3 and 6.4.1: an annotation's appearance dictionary holds only `/N`, which for a button
/// widget is a dictionary of appearance states and for every other annotation an appearance stream
/// (ISO 32000-2 12.5.5), and a widget has no `/A` action.
pub(crate) fn annotation(doc: &Document, dict: &Dict, subtype: Option<&[u8]>, name: &str, page: Option<usize>, issue: &mut Report<'_>) {
    let appearance = dict.get(b"AP").map(|ap| doc.resolve(ap));
    let appearance = appearance.as_deref().and_then(Object::as_dict);
    if appearance.is_some_and(|ap| ap.iter().any(|(k, _)| k.as_slice() != b"N")) {
        issue("6.3.3", "A {} annotation's appearance dictionary holds more than N", Some(name.to_owned()), page, false);
    }
    if let Some(normal) = appearance.and_then(|ap| ap.get(b"N")).map(|n| doc.resolve(n)) {
        let button = subtype == Some(b"Widget") && field_type(doc, dict) == Some(b"Btn".to_vec());
        if button && !matches!(&*normal, Object::Dict(_)) {
            issue("6.3.3", "A button's normal appearance isn't a dictionary of appearance states", None, page, false);
        } else if !button && !matches!(&*normal, Object::Stream(_)) {
            issue("6.3.3", "A {} annotation's normal appearance isn't an appearance stream", Some(name.to_owned()), page, false);
        }
    }
    // A null action, or a reference to no object, is no entry (ISO 32000-2 7.3.9, 7.3.10).
    if subtype == Some(b"Widget") && dict.get(b"A").is_some_and(|a| !matches!(&*doc.resolve(a), Object::Null)) {
        issue("6.4.1", "A widget annotation has an action (A)", None, page, false);
    }
}

/// How many `/Parent` links [`field_type`] follows.
const MAX_FIELD_DEPTH: usize = 32;

/// The field type (`/FT`) of the widget `dict`: its own, or inherited from its parents (ISO 32000-2
/// 12.7.4.1, Table 226). A null `/FT` is no entry (7.3.9), so it is inherited past.
fn field_type(doc: &Document, dict: &Dict) -> Option<Vec<u8>> {
    let mut current = dict.clone();
    for _ in 0..MAX_FIELD_DEPTH {
        if let Some(ft) = current.get(b"FT").map(|f| doc.resolve(f)).filter(|f| !matches!(**f, Object::Null)) {
            return ft.as_name().map(<[u8]>::to_vec);
        }
        let parent = doc.resolve(current.get(b"Parent")?);
        current = parent.as_dict()?.clone();
    }
    None
}

/// 6.1.6.1, 6.2.8.1 and 6.2.8.2: a 3D stream is U3D or PRC (ISO 32000-2 13.6.3, Table 311); and
/// a form XObject has no OPI dictionary and refers to no other file (`/Ref`, a reference XObject,
/// ISO 32000-2 8.10.4). Which filters a stream may use (6.1.6.2) is
/// [`check_filters`](crate::check_filters)'s, for every part.
pub(crate) fn stream(dict: &Dict, subtype: Option<&[u8]>, issue: &mut Report<'_>) {
    if dict.name(b"Type") == Some(b"3D") && !matches!(subtype, Some(b"U3D" | b"PRC")) {
        issue("6.1.6.1", "A 3D stream's Subtype isn't U3D or PRC", None, None, false);
    }
    if subtype != Some(b"Form") {
        return;
    }
    if dict.get(b"OPI").is_some() {
        issue("6.2.8.1", "A form XObject has OPI information", None, None, false);
    }
    if dict.get(b"Ref").is_some() {
        issue("6.2.8.2", "A form XObject refers to another file (Ref)", None, None, false);
    }
}

/// The triggers an additional-actions dictionary of the catalog, a page or an annotation other than
/// a widget may hold in PDF/A-4: those of annotations (ISO 32000-2 Table 197) that a user's pointer
/// or focus sets off.
const TRIGGERS: &[&[u8]] = &[b"E", b"X", b"D", b"U", b"Fo", b"Bl"];

/// The first trigger of the additional-actions entry `aa` PDF/A-4 doesn't allow, if any.
pub(crate) fn forbidden_trigger(doc: &Document, aa: &Object) -> Option<String> {
    let aa = doc.resolve(aa);
    let aa = aa.as_dict()?;
    aa.iter().map(|(k, _)| k).find(|k| !TRIGGERS.contains(&k.as_slice())).map(|k| String::from_utf8_lossy(k).into_owned())
}

/// The issue-reporting callback `lib.rs` hands every check.
pub(crate) type Report<'a> = dyn FnMut(&'static str, &'static str, Option<String>, Option<usize>, bool) + 'a;

/// 6.7.3: the identification schema names part 4, the 2020 revision and the level's conformance
/// letter (none for PDF/A-4 itself), and writes `part` and `rev` with the prefix `pdfaid`.
pub(crate) fn identification(x: &str, level: Level, issue: &mut Report<'_>) {
    let part = xmp::identifier(x, xmp::PDFA_ID, "part");
    let conformance = xmp::identifier(x, xmp::PDFA_ID, "conformance");
    let wanted = match level {
        Level::A4e => Some("E"),
        Level::A4f => Some("F"),
        _ => None,
    };
    // The letter is compared as written: veraPDF refuses a lower-case "f".
    if part.as_deref() != Some("4") || conformance.as_deref() != wanted {
        issue("6.7.3", "The metadata doesn't identify the file as {}", Some(level.label().to_owned()), None, false);
    }
    match xmp::identifier(x, xmp::PDFA_ID, "rev") {
        None => issue("6.7.3", "The metadata has no pdfaid:rev", None, None, false),
        // The start of it says what it is: a packet can hold megabytes there.
        Some(rev) if rev != REVISION => {
            issue("6.7.3", "The metadata gives pdfaid:rev as {}, not 2020", Some(rev.chars().take(32).collect()), None, false)
        }
        Some(_) => {}
    }
    for property in other_prefixes(x) {
        issue("6.7.3", "The identification property {} isn't written with the prefix pdfaid", Some(property.to_owned()), None, false);
    }
}

/// The identification properties among `part` and `rev` that the packet `x` writes with a prefix
/// other than `pdfaid`. A packet that can't be parsed reports none: it is read by the prefix anyway.
fn other_prefixes(x: &str) -> Vec<&'static str> {
    fn walk(el: &craft_xml::El, parent: &xmp::NsScope<'_>, found: &mut Vec<&'static str>, depth: usize) {
        // The parser already bounds the depth; this keeps the walk total on its own.
        if depth > 1024 {
            return;
        }
        let scope = parent.child(el);
        for property in ["part", "rev"] {
            let element = scope.is(el, xmp::PDFA_ID, property) && el.prefix() != "pdfaid";
            let attribute = el.attrs.iter().any(|(k, _)| {
                k.split_once(':').is_some_and(|(prefix, local)| local == property && prefix != "pdfaid" && scope.attr_ns(k) == Some(xmp::PDFA_ID))
            });
            if (element || attribute) && !found.contains(&property) {
                found.push(property);
            }
        }
        for child in el.elements() {
            walk(child, &scope, found, depth.saturating_add(1));
        }
    }
    let mut found = Vec::new();
    if let Some(root) = xmp::parse(x) {
        walk(&root, &xmp::NsScope::root(), &mut found, 0);
    }
    found
}

/// The rules for the document as a whole: the information dictionary, the catalog's version and
/// entries, the metadata stream, the output intents and the end of the file.
pub(crate) fn document(doc: &Document, catalog: &Dict, profiles: &mut Profiles, issue: &mut Report<'_>) {
    information(doc, catalog, issue);
    catalog_version(doc, catalog, issue);
    catalog_entries(doc, catalog, issue);
    metadata_stream(catalog, doc, issue);
    output_intents(doc, catalog, None, profiles, issue);
    end_of_file(doc, issue);
}

/// 6.1.3: a trailer names a document information dictionary only when the catalog has a
/// `/PieceInfo`, and that dictionary holds nothing but `/ModDate` (ISO 32000-2 14.3.3 deprecates
/// the rest; 14.5 needs the dictionary for page-piece dictionaries).
fn information(doc: &Document, catalog: &Dict, issue: &mut Report<'_>) {
    let Some(info) = doc.trailer().get(b"Info").map(|i| doc.resolve(i)).filter(|i| !matches!(**i, Object::Null)) else { return };
    if catalog.get(b"PieceInfo").is_none() {
        issue("6.1.3", "The trailer names a document information dictionary (Info), but the catalog has no PieceInfo", None, None, false);
    }
    if let Some(info) = info.as_dict() {
        let others = keys_other_than(info, &[b"ModDate"]);
        if !others.is_empty() {
            issue("6.1.3", "The document information dictionary holds {}: only ModDate is allowed", Some(others), None, false);
        }
    }
}

/// The keys of `dict` not in `allowed`, the first few of them, for a message: a dictionary of
/// thousands is not listed whole.
fn keys_other_than(dict: &Dict, allowed: &[&[u8]]) -> String {
    let others: Vec<String> =
        dict.iter().map(|(k, _)| k).filter(|k| !allowed.contains(&k.as_slice())).take(8).map(|k| String::from_utf8_lossy(k).into_owned()).collect();
    others.join(", ")
}

/// 6.1.12: a catalog `/Version` names PDF 2.n (ISO 32000-2 7.7.2, Table 29).
fn catalog_version(doc: &Document, catalog: &Dict, issue: &mut Report<'_>) {
    // A null version, direct or through a reference, is none (ISO 32000-2 7.3.9).
    let Some(version) = catalog.get(b"Version").map(|v| doc.resolve(v)).filter(|v| !matches!(&**v, Object::Null)) else { return };
    let ok = version.as_name().is_some_and(|v| matches!(v, [b'2', b'.', b'0'..=b'9']));
    if !ok {
        let shown = match version.as_name() {
            Some(v) => String::from_utf8_lossy(v.get(..v.len().min(32)).unwrap_or_default()).into_owned(),
            None => "something other than a name".to_owned(),
        };
        issue("6.1.12", "The catalog gives the version {}, not 2.0 to 2.9", Some(shown), None, false);
    }
}

/// 6.1.11, 6.4.2, 6.10, 6.11 and 6.12: what the catalog may not hold. A permissions dictionary
/// holds only `/UR3` and `/DocMDP` (ISO 32000-2 12.8.6, Table 263); no XFA form and no
/// `/NeedsRendering`; every optional content configuration has a `/Name` of its own; no alternate
/// presentations; no `/Requirements`.
fn catalog_entries(doc: &Document, catalog: &Dict, issue: &mut Report<'_>) {
    let dict_of = |key: &[u8]| catalog.get(key).map(|v| doc.resolve(v)).and_then(|v| v.as_dict().cloned());
    if let Some(perms) = dict_of(b"Perms") {
        let others = keys_other_than(&perms, &[b"UR3", b"DocMDP"]);
        if !others.is_empty() {
            issue("6.1.11", "The permissions dictionary holds {}: only UR3 and DocMDP are allowed", Some(others), None, false);
        }
    }
    if dict_of(b"AcroForm").is_some_and(|form| form.get(b"XFA").is_some()) {
        issue("6.4.2", "The form has an XFA form (XFA)", None, None, false);
    }
    if catalog.get(b"NeedsRendering").is_some() {
        issue("6.4.2", "The catalog asks for its XFA form to be rendered (NeedsRendering)", None, None, false);
    }
    if dict_of(b"Names").is_some_and(|names| names.get(b"AlternatePresentations").is_some()) {
        issue("6.11", "The document has alternate presentations (AlternatePresentations)", None, None, false);
    }
    if catalog.get(b"Requirements").is_some() {
        issue("6.12", "The catalog has requirements (Requirements)", None, None, false);
    }
    if let Some(properties) = dict_of(b"OCProperties") {
        optional_content_names(doc, &properties, issue);
    }
}

/// 6.11 and 6.2.3: a page has no presentation steps (`/PresSteps`), and its own output intents
/// follow the rules of the catalog's.
pub(crate) fn page(doc: &Document, page: &Dict, index: usize, profiles: &mut Profiles, issue: &mut Report<'_>) {
    if page.get(b"PresSteps").is_some() {
        issue("6.11", "A page has presentation steps (PresSteps)", None, Some(index), false);
    }
    output_intents(doc, page, Some(index), profiles, issue);
}

/// How many optional content configurations of `/Configs` [`optional_content_names`] reads.
const MAX_CONFIGURATIONS: usize = 4096;

/// 6.10: each optional content configuration (`/D` and those in `/Configs`, ISO 32000-2 8.11.4.2)
/// has a `/Name`, and no two have the same one. A name is a text string (Table 99), so two are the
/// same when they decode to the same text (7.9.2.2), however each is encoded. A `/Configs` longer than [`MAX_CONFIGURATIONS`] is
/// reported as not checked in full: what its other configurations hold isn't known, so the file
/// doesn't pass.
fn optional_content_names(doc: &Document, properties: &Dict, issue: &mut Report<'_>) {
    let mut configurations: Vec<Object> = properties.get(b"D").cloned().into_iter().collect();
    if let Some(list) = properties.get(b"Configs").map(|c| doc.resolve(c))
        && let Some(list) = list.as_array()
    {
        configurations.extend(list.iter().take(MAX_CONFIGURATIONS).cloned());
        if list.len() > MAX_CONFIGURATIONS {
            issue(
                "6.10",
                "Some optional content configurations weren't checked: there are more of them than one verification reads",
                None,
                None,
                false,
            );
        }
    }
    let mut names = std::collections::HashSet::new();
    let (mut unnamed, mut repeated) = (false, false);
    for configuration in &configurations {
        let configuration = doc.resolve(configuration);
        let Some(configuration) = configuration.as_dict() else { continue };
        match configuration.get(b"Name").map(|n| doc.resolve(n)).and_then(|n| n.as_string().map(pdfcraft_cos::PdfString::to_text)) {
            Some(name) => repeated |= !names.insert(name),
            None => unnamed = true,
        }
    }
    if unnamed {
        issue("6.10", "An optional content configuration has no Name", None, None, false);
    }
    if repeated {
        issue("6.10", "Two optional content configurations have the same Name", None, None, false);
    }
}

/// 6.7.2.1: the catalog's metadata stream says it is one (`/Type /Metadata /Subtype /XML`, ISO
/// 32000-2 14.3.2, Table 347), and its packet is UTF-8, well-formed, and has no `bytes` or
/// `encoding` attribute in its `xpacket` header.
fn metadata_stream(catalog: &Dict, doc: &Document, issue: &mut Report<'_>) {
    let Some(metadata) = catalog.get(b"Metadata").map(|m| doc.resolve(m)) else { return };
    let Object::Stream(stream) = &*metadata else { return };
    if stream.dict.name(b"Type") != Some(b"Metadata") || stream.dict.name(b"Subtype") != Some(b"XML") {
        issue("6.7.2.1", "The metadata stream isn't marked /Type /Metadata /Subtype /XML", None, None, false);
    }
    // What can't be decoded at all, or within the limit `lib.rs` reads metadata with, is reported
    // there as no metadata at all.
    let mut budget = pdfcraft_cos::DecodeBudget::new(xmp::MAX_XMP_BYTES, 0);
    let Ok((bytes, damaged)) = budget.decode_with_damage(doc, stream, xmp::MAX_XMP_BYTES) else { return };
    if damaged {
        issue("6.7.2.1", "The metadata stream's data is damaged: only part of it decodes", None, None, false);
    }
    packet(&bytes, None, issue);
}

/// Whether the decoding error `e` is a limit reached, not data that can't be decoded: the
/// filters' own wording of a decode past its limit ("… the limit of N bytes", whatever N), or of
/// an allowance spent.
fn over_limit(e: &pdfcraft_cos::CosError) -> bool {
    use pdfcraft_filters::FilterError;
    let pdfcraft_cos::CosError::Filter(text) = e else { return false };
    let limit = FilterError::LimitExceeded(0).to_string();
    let head = limit.strip_suffix("0 bytes").unwrap_or(&limit);
    (!head.is_empty() && text.starts_with(head)) || *text == FilterError::Spent.to_string()
}

/// 6.7.2.1 for the packet `bytes` of the catalog's metadata stream (`object` `None`) or of the
/// metadata stream `object` of a component: UTF-8, well-formed, and no `bytes` or `encoding`
/// attribute in its `xpacket` header.
fn packet(bytes: &[u8], object: Option<pdfcraft_cos::ObjRef>, issue: &mut Report<'_>) {
    let named = object.map(|o| o.to_string());
    let Ok(text) = std::str::from_utf8(bytes) else {
        match named {
            Some(o) => issue("6.7.2.1", "The XMP metadata in metadata stream {} isn't UTF-8", Some(o), None, false),
            None => issue("6.7.2.1", "The XMP metadata isn't UTF-8", None, None, false),
        }
        return;
    };
    if xmp::parse(text).is_none() {
        match &named {
            Some(o) => issue("6.7.2.1", "The XMP metadata in metadata stream {} isn't well-formed XML", Some(o.clone()), None, false),
            None => issue("6.7.2.1", "The XMP metadata isn't well-formed XML", None, None, false),
        }
    }
    if let Some(start) = packet_header(text)
        && let Some(header) = text.get(start..).and_then(|rest| rest.find("?>").and_then(|end| rest.get(..end)))
        && (declares_pseudo_attribute(header, "bytes") || declares_pseudo_attribute(header, "encoding"))
    {
        match named {
            Some(o) => issue("6.7.2.1", "The XMP packet header in metadata stream {} has a bytes or encoding attribute", Some(o), None, false),
            None => issue("6.7.2.1", "The XMP packet header has a bytes or encoding attribute", None, None, false),
        }
    }
}

/// Where the packet's `xpacket` header (ISO 16684-1 7.3.2) starts in `text`: the first processing
/// instruction whose target is `xpacket` (XML 1.0 2.6), outside comments (2.5). The text is read
/// once, front to back: a comment is skipped to its `-->` and another processing instruction to
/// its `?>`, whatever either holds, and one that never ends hides everything after it.
fn packet_header(text: &str) -> Option<usize> {
    let mut at: usize = 0;
    loop {
        let open = at.checked_add(text.get(at..)?.find('<')?)?;
        let rest = text.get(open..)?;
        if rest.starts_with("<!--") {
            let body = open.checked_add(4)?;
            at = body.checked_add(text.get(body..)?.find("-->")?)?.checked_add(3)?;
        } else if let Some(instruction) = rest.strip_prefix("<?") {
            // The target is the name up to white space or the instruction's end (2.6, `PITarget`).
            let target = instruction.split(|c: char| c.is_ascii_whitespace() || c == '?').next().unwrap_or_default();
            if target == "xpacket" {
                return Some(open);
            }
            let body = open.checked_add(2)?;
            at = body.checked_add(text.get(body..)?.find("?>")?)?.checked_add(2)?;
        } else {
            at = open.checked_add(1)?;
        }
    }
}

/// Whether the `xpacket` processing instruction `header` gives the pseudo-attribute `name`: the
/// name after white space, then `=` after any white space, as XML 1.0 2.3 (`S`, `Eq`) lets an
/// attribute be written: a header (ISO 16684-1 7.3.2) can space them however it likes.
fn declares_pseudo_attribute(header: &str, name: &str) -> bool {
    let white = |c: char| matches!(c, ' ' | '\t' | '\r' | '\n');
    header.match_indices(name).any(|(at, _)| {
        let before = header.get(..at).and_then(|h| h.chars().next_back()).is_some_and(white);
        let after = header.get(at.saturating_add(name.len())..).is_some_and(|rest| rest.trim_start_matches(white).starts_with('='));
        before && after
    })
}

/// The most of all component metadata streams [`component_metadata`] decodes, together.
const MAX_COMPONENT_METADATA: usize = 64 << 20;

/// 6.7.2.1 for the metadata streams of the document's components (ISO 32000-2 14.3.2: pages,
/// XObjects, fonts and the other dictionaries with a `/Metadata` entry): their packets are held to
/// the same encoding rules as the catalog's, and one whose data can't be decoded, or only in part,
/// is reported. Each stream is read once, the catalog's not again, and all of them within
/// [`MAX_COMPONENT_METADATA`]; one too large to decode within [`xmp::MAX_XMP_BYTES`] or what is
/// left of that is left alone, as the catalog's is: its size is no fault of its data.
pub(crate) fn component_metadata(doc: &Document, catalog: &Dict, objs: &[(pdfcraft_cos::ObjRef, std::sync::Arc<Object>)], issue: &mut Report<'_>) {
    let mut seen: std::collections::HashSet<pdfcraft_cos::ObjRef> = catalog.get(b"Metadata").and_then(Object::as_ref).into_iter().collect();
    let mut budget = pdfcraft_cos::DecodeBudget::new(MAX_COMPONENT_METADATA, 0);
    for (_, o) in objs {
        let dict = match &**o {
            Object::Dict(d) => d,
            Object::Stream(s) => &s.dict,
            _ => continue,
        };
        let Some(r) = dict.get(b"Metadata").and_then(Object::as_ref) else { continue };
        if !seen.insert(r) {
            continue;
        }
        let Object::Stream(stream) = &*doc.get(r) else { continue };
        if budget.remaining() == 0 {
            break;
        }
        match budget.decode_with_damage(doc, stream, xmp::MAX_XMP_BYTES) {
            Ok((bytes, damaged)) => {
                if damaged {
                    issue("6.7.2.1", "Metadata stream {}'s data is damaged: only part of it decodes", Some(r.to_string()), None, false);
                }
                packet(&bytes, Some(r), issue);
            }
            Err(e) if over_limit(&e) => {}
            Err(_) => issue("6.7.2.1", "Metadata stream {} can't be decoded", Some(r.to_string()), None, false),
        }
    }
}

/// The most of an ICC profile [`output_intents`] and [`cie_components`] decode to read its header;
/// real ICC profiles stay well under it.
const MAX_ICC_HEADER_READ: usize = 16 << 20;

/// How many output intents [`output_intents`] reads; past it, the others are reported as not
/// checked.
const MAX_INTENTS: usize = 4096;

/// What the start of an ICC profile stream holds, as [`Profiles`] reads it.
#[derive(Clone)]
enum ProfileHeader {
    /// The first 20 bytes of the profile (fewer when it is shorter): its header's device class and
    /// data colour space are bytes 12 to 19 (ICC.1 7.2.5 and 7.2.6).
    Read(Vec<u8>),
    /// Larger than [`MAX_ICC_HEADER_READ`] decoded: not read, and no fault of its data.
    TooLarge,
    /// Data that can't be decoded, or a profile cut short ([`icc_extent_fits`]).
    Broken,
}

#[cfg(test)]
thread_local! {
    /// How many ICC profile streams [`Profiles::header`] has decoded.
    pub(crate) static PROFILE_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The ICC profile headers read so far, by profile object: a profile that many output intents,
/// pages or transparency groups name is decoded once.
#[derive(Default)]
pub(crate) struct Profiles(std::collections::HashMap<pdfcraft_cos::ObjRef, ProfileHeader>);

impl Profiles {
    /// The header of the profile `profile`, a reference or a stream written in place; `None` when
    /// it is no stream.
    fn header(&mut self, doc: &Document, profile: &Object) -> Option<ProfileHeader> {
        let at = profile.as_ref();
        if let Some(known) = at.and_then(|r| self.0.get(&r)) {
            return Some(known.clone());
        }
        let resolved = doc.resolve(profile);
        let Object::Stream(s) = &*resolved else { return None };
        #[cfg(test)]
        PROFILE_DECODES.with(|n| n.set(n.get().saturating_add(1)));
        let header = match s.decoded_within(MAX_ICC_HEADER_READ) {
            Ok(data) if icc_extent_fits(&data) => ProfileHeader::Read(data.get(..20).unwrap_or(&data).to_vec()),
            Ok(_) => ProfileHeader::Broken,
            Err(e) if over_limit(&e) => ProfileHeader::TooLarge,
            Err(_) => ProfileHeader::Broken,
        };
        if let Some(r) = at {
            self.0.insert(r, header.clone());
        }
        Some(header)
    }
}

/// Whether the ICC profile `data` holds all it says it does: its 128-byte header and tag count,
/// at least the profile size its header gives (ICC.1 7.2.2), and every tag's data after the tag
/// table and within that size (7.3.1 to 7.3.5: a count, then an offset and size per tag; 7.1: the
/// tag data follows the table). A profile cut short isn't one.
fn icc_extent_fits(data: &[u8]) -> bool {
    let word = |at: usize| data.get(at..at.saturating_add(4)).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_be_bytes);
    let (Some(size), Some(count)) = (word(0), word(128)) else { return false };
    let (Ok(size), Ok(count)) = (usize::try_from(size), usize::try_from(count)) else { return false };
    if size > data.len() || size < 132 {
        return false;
    }
    // Each tag's entry is 12 bytes after the count; the table has to fit in the profile first.
    let Some(table_end) = count.checked_mul(12).and_then(|n| n.checked_add(132)).filter(|end| *end <= size) else {
        return false;
    };
    (0..count).all(|i| {
        let entry = 132usize.saturating_add(i.saturating_mul(12));
        match (word(entry.saturating_add(4)), word(entry.saturating_add(8))) {
            (Some(offset), Some(len)) => {
                usize::try_from(offset).is_ok_and(|start| start >= table_end)
                    && usize::try_from(u64::from(offset).saturating_add(u64::from(len))).is_ok_and(|end| end <= size)
            }
            _ => false,
        }
    })
}

/// 6.2.3: in the output intents of `holder`, the catalog or the page `page` (ISO 32000-2 14.11.5
/// lets both have them), none names an external profile (`/DestOutputProfileRef`, Table 401), all
/// that have a `/DestOutputProfile` share one profile object, and the PDF/A one's profile is an
/// output (`prtr`) or monitor (`mntr`) profile of gray, RGB or CMYK (the ICC header's device class
/// and data colour space, ICC.1 7.2.5 and 7.2.6). A profile stream that can't be decoded, or is cut
/// short, is none of those; one too large to read the header of within [`MAX_ICC_HEADER_READ`] is
/// left alone, and a profile that is no stream at all is taken as missing.
fn output_intents(doc: &Document, holder: &Dict, page: Option<usize>, headers: &mut Profiles, issue: &mut Report<'_>) {
    let Some(list) = holder.get(b"OutputIntents").map(|o| doc.resolve(o)) else { return };
    let Some(list) = list.as_array() else { return };
    let mut profiles = std::collections::HashSet::new();
    let (mut external, mut direct) = (false, 0usize);
    for intent in list.iter().take(MAX_INTENTS) {
        let intent = doc.resolve(intent);
        let Some(intent) = intent.as_dict() else { continue };
        external |= intent.get(b"DestOutputProfileRef").is_some();
        let header = match intent.get(b"DestOutputProfile") {
            Some(profile) if intent.name(b"S") == Some(b"GTS_PDFA1") => headers.header(doc, profile),
            _ => None,
        };
        let fits = |h: &[u8]| matches!(h.get(12..16), Some(b"prtr" | b"mntr")) && matches!(h.get(16..20), Some(b"GRAY" | b"RGB " | b"CMYK"));
        if matches!(&header, Some(ProfileHeader::Broken)) || matches!(&header, Some(ProfileHeader::Read(h)) if !fits(h.as_slice())) {
            issue("6.2.3", "The PDF/A output intent's profile isn't an output or monitor profile of gray, RGB or CMYK", None, page, false);
        }
        match intent.get(b"DestOutputProfile") {
            Some(Object::Ref(r)) => {
                profiles.insert(*r);
            }
            // A profile written directly is an object of its own, never the same as another.
            Some(Object::Null) | None => {}
            Some(_) => direct = direct.saturating_add(1),
        }
    }
    if external {
        issue("6.2.3", "An output intent names an external profile (DestOutputProfileRef)", None, page, false);
    }
    if profiles.len().saturating_add(direct) > 1 {
        issue("6.2.3", "The output intents have different destination profiles", None, page, false);
    }
    // What the intents past the limit hold isn't known, so the file doesn't pass.
    if list.len() > MAX_INTENTS {
        issue("6.2.3", "Some output intents weren't checked: there are more of them than one verification reads", None, page, false);
    }
}

/// 6.1.3: nothing follows the last end-of-file marker but an end-of-line (ISO 32000-2 7.5.5), and
/// there is one: a file whose last line isn't one, and that has none at the start of a line before
/// it, is reported too. A document its next save rewrites is judged by that save, which ends the
/// file itself.
fn end_of_file(doc: &Document, issue: &mut Report<'_>) {
    if doc.full_save_required() || doc.encryption_changed() {
        return;
    }
    let data = doc.bytes();
    // The marker is a line of its own (ISO 32000-2 7.5.5): `%%EOF` inside other data is data.
    let Some(end) = data
        .windows(5)
        .enumerate()
        .rev()
        .find(|(at, w)| *w == b"%%EOF" && (*at == 0 || matches!(data.get(at.saturating_sub(1)), Some(b'\r' | b'\n'))))
        .map(|(at, _)| at)
    else {
        issue("6.1.3", "The file has no end-of-file marker (%%EOF) on a line of its own", None, None, false);
        return;
    };
    let after = data.get(end.saturating_add(5)..).unwrap_or_default();
    let rest = match after {
        [b'\r', b'\n', rest @ ..] | [b'\r', rest @ ..] | [b'\n', rest @ ..] => rest,
        rest => rest,
    };
    if !rest.is_empty() {
        issue("6.1.3", "Data follows the last end-of-file marker (%%EOF)", None, None, false);
    }
}

/// 6.9: PDF/A-4f keeps its embedded files in the catalog's name dictionary (`/EmbeddedFiles`,
/// ISO 32000-2 7.7.4, Table 32).
pub(crate) fn embedded_files_named(doc: &Document, catalog: &Dict, issue: &mut Report<'_>) {
    let names = catalog.get(b"Names").map(|n| doc.resolve(n));
    if !names.as_deref().and_then(Object::as_dict).is_some_and(|n| n.get(b"EmbeddedFiles").is_some()) {
        issue("6.9", "A PDF/A-4f file needs embedded files listed in the catalog's name dictionary (EmbeddedFiles)", None, None, false);
    }
}

/// 6.9: an embedded file's file specification, at `level`: it has `/F` and `/UF` (ISO 32000-2
/// 7.11.3, Table 43) and an `/AFRelationship`, its stream names a MIME type (7.11.4, Table 44),
/// and in PDF/A-4 itself (not 4e or 4f) the file is a PDF/A file of part 1, 2 or 4: it declares
/// one, and when that is a level [`verify`](crate::verify) checks it is verified at it
/// ([`embedded_pdfa`]). An embedded file stream is judged once per verification, however many
/// file specifications or `/EF` entries name it: `checked` keeps the verdicts.
pub(crate) fn embedded_file(doc: &Document, level: Level, spec: &Dict, checked: &mut EmbeddedChecks, issue: &mut Report<'_>) {
    let entries = spec.get(b"EF").map(|ef| doc.resolve(ef)).and_then(|ef| ef.as_dict().cloned()).unwrap_or_default();
    // An entry that refers to the stream through objects that only refer on (ISO 32000-2 7.3.10)
    // is known by the stream it ends at, so its aliases share one verdict.
    let streams: Vec<_> = entries.iter().map(|(_, s)| (s.as_ref().map(|r| crate::terminal_ref(doc, r)), doc.resolve(s))).collect();
    if level == Level::A4 {
        let mut not_pdfa = false;
        for (at, stream) in &streams {
            let verdict = match &**stream {
                Object::Stream(s) => match at.and_then(|r| checked.verdicts.get(&r).copied()) {
                    Some(known) => known,
                    None => {
                        let found = embedded_pdfa(doc, s);
                        if let Some(r) = at {
                            checked.verdicts.insert(*r, found);
                        }
                        found
                    }
                },
                _ => Embedded::NotPdfa,
            };
            match verdict {
                Embedded::NotPdfa => not_pdfa = true,
                Embedded::Declares => {}
                Embedded::Fails(label) => issue("6.9", "An embedded file declares {} but doesn't conform to it", Some(label.to_owned()), None, false),
                Embedded::Unchecked => checked.unchecked = true,
            }
        }
        if not_pdfa {
            issue("6.9", "Embedded files must be PDF/A-1, PDF/A-2 or PDF/A-4 files themselves (use PDF/A-4f for others)", None, None, false);
        }
    }
    // A null value is no entry (ISO 32000-2 7.3.9).
    if !crate::present(doc, spec, b"AFRelationship") {
        issue("6.9", "An embedded file has no AFRelationship", None, None, false);
    }
    if !crate::present(doc, spec, b"F") || !crate::present(doc, spec, b"UF") {
        issue("6.9", "An embedded file's file specification lacks F or UF", None, None, false);
    }
    for (_, stream) in &streams {
        let Object::Stream(s) = &**stream else { continue };
        if !s.dict.name(b"Subtype").is_some_and(is_mime_type) {
            issue("6.9", "An embedded file has no MIME type (Subtype)", None, None, false);
        }
    }
}

/// The largest embedded file [`embedded_pdfa`] opens.
pub(crate) const MAX_EMBEDDED_PDF: usize = 64 << 20;

/// The decoded bytes of embedded files one top-level verification reads in all, the files embedded
/// in those included: four files of [`MAX_EMBEDDED_PDF`].
pub(crate) const MAX_EMBEDDED_DECODED: usize = 4 * MAX_EMBEDDED_PDF;

/// How many embedded files one top-level verification reads in all, the files embedded in those
/// included. Each one read may be verified, within allowances of its own, so this bounds how many
/// verifications one file can ask for.
pub(crate) const MAX_EMBEDDED_READ: usize = 32;

/// How many embedded files deep [`embedded_pdfa`] verifies: an embedded file's own embedded files
/// are judged by their declaration only.
const MAX_NESTED_VERIFY: usize = 1;

/// The wording for embedded files left unread for lack of allowance, or larger than
/// [`MAX_EMBEDDED_PDF`], as [`EmbeddedChecks::report`] gives it.
pub(crate) const UNCHECKED: &str = "Some embedded files weren't checked: there are more of them, or larger ones, than one verification reads";

/// What the embedded files of one top-level verification may still read, shared with the
/// verifications of those files.
struct EmbeddedAllowance {
    bytes: pdfcraft_cos::DecodeBudget,
    files: usize,
}

impl EmbeddedAllowance {
    fn new() -> Self {
        Self { bytes: pdfcraft_cos::DecodeBudget::new(MAX_EMBEDDED_DECODED, 0), files: MAX_EMBEDDED_READ }
    }
}

thread_local! {
    /// How many embedded-file verifications this thread is inside.
    static NESTED_VERIFY: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// What the current top-level verification's embedded files may still read.
    static EMBEDDED_ALLOWANCE: std::cell::RefCell<EmbeddedAllowance> = std::cell::RefCell::new(EmbeddedAllowance::new());
}

#[cfg(test)]
thread_local! {
    /// Embedded files [`embedded_pdfa`] read.
    pub(crate) static EMBEDDED_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Starts a verification: a top-level one (not that of an embedded file) gets the whole embedded
/// file allowance afresh.
pub(crate) fn begin_verification() {
    if NESTED_VERIFY.with(std::cell::Cell::get) == 0 {
        EMBEDDED_ALLOWANCE.with(|a| {
            if let Ok(mut a) = a.try_borrow_mut() {
                *a = EmbeddedAllowance::new();
            }
        });
    }
}

/// The embedded file verdicts of one verification, by stream object, and whether an embedded file
/// was left unread for lack of allowance.
#[derive(Default)]
pub(crate) struct EmbeddedChecks {
    verdicts: std::collections::HashMap<pdfcraft_cos::ObjRef, Embedded>,
    unchecked: bool,
}

impl EmbeddedChecks {
    /// 6.9: says, once, that embedded files went unchecked when one did. Their verdict isn't
    /// known, so the file doesn't pass.
    pub(crate) fn report(&self, issue: &mut Report<'_>) {
        if self.unchecked {
            issue(
                "6.9",
                "Some embedded files weren't checked: there are more of them, or larger ones, than one verification reads",
                None,
                None,
                false,
            );
        }
    }
}

/// What [`embedded_pdfa`] finds an embedded file to be.
#[derive(Clone, Copy)]
enum Embedded {
    /// Not a PDF, or one that declares no level of PDF/A part 1, 2 or 4.
    NotPdfa,
    /// A PDF that declares PDF/A-1, 2 or 4, and passes verification at that level when it is one
    /// [`verify`](crate::verify) checks (PDF/A-2b, 4, 4e or 4f) and the file isn't nested deeper
    /// than [`MAX_NESTED_VERIFY`].
    Declares,
    /// A PDF that declares this level and fails its verification.
    Fails(&'static str),
    /// Not read, or not verified through, because the file decodes to more than
    /// [`MAX_EMBEDDED_PDF`] or the top-level verification's embedded files reached
    /// [`MAX_EMBEDDED_READ`] or [`MAX_EMBEDDED_DECODED`].
    Unchecked,
}

/// Leaves one embedded-file verification when dropped.
struct NestedGuard;

impl Drop for NestedGuard {
    fn drop(&mut self) {
        NESTED_VERIFY.with(|n| n.set(n.get().saturating_sub(1)));
    }
}

/// The embedded file `s` of `doc` decoded, charged to the top-level verification's allowance:
/// `None` when the allowance can't pay for it or it decodes to more than [`MAX_EMBEDDED_PDF`],
/// `Some(None)` when its data can't be decoded, or only in part: what decodes before damage isn't
/// the file. Its `/Filter` and `/DecodeParms` are read through `doc`, given by reference or not.
fn read_embedded(doc: &Document, s: &pdfcraft_cos::Stream) -> Option<Option<Vec<u8>>> {
    EMBEDDED_ALLOWANCE.with(|a| {
        // The cell is borrowed only here, and nothing in here reenters it; were it busy, the file
        // would be left unchecked, not passed.
        let Ok(mut a) = a.try_borrow_mut() else { return None };
        if a.files == 0 {
            return None;
        }
        a.files = a.files.saturating_sub(1);
        #[cfg(test)]
        EMBEDDED_READ.with(|n| n.set(n.get().saturating_add(1)));
        // A limit reached, the file's own or the allowance's, leaves the file unread: its size is
        // no fault of its data, but its verdict isn't known.
        let s = with_direct_filters(doc, s);
        match a.bytes.decode_with_damage(doc, &s, MAX_EMBEDDED_PDF) {
            Ok((bytes, false)) => Some(Some(bytes)),
            Ok((_, true)) => Some(None),
            Err(e) if over_limit(&e) => None,
            Err(_) => Some(None),
        }
    })
}

/// `s` with its `/Filter`, a reference or an array holding references, made direct, so the stream
/// decodes with the filters it names (ISO 32000-2 7.3.8.2, Table 5). Only names are copied: a
/// filter that resolves to anything else, or more filters than a decoder takes, leaves `s` as it
/// is, for the decoder to reject, so a large object named many times is never copied.
pub(crate) fn with_direct_filters<'s>(doc: &Document, s: &'s pdfcraft_cos::Stream) -> std::borrow::Cow<'s, pdfcraft_cos::Stream> {
    let Some(filter) = s.dict.get(b"Filter") else { return std::borrow::Cow::Borrowed(s) };
    let name = |f: &Object| match &*doc.resolve(f) {
        Object::Name(n) => Some(Object::Name(n.clone())),
        _ => None,
    };
    let direct = match &*doc.resolve(filter) {
        // A chain longer than the decoders take (`/Crypt` counted besides) is rejected as it is.
        Object::Array(list) if list.len() > pdfcraft_filters::MAX_CHAIN.saturating_add(1) => return std::borrow::Cow::Borrowed(s),
        Object::Array(list) => match list.iter().map(name).collect::<Option<Vec<_>>>() {
            Some(names) => Object::Array(names),
            None => return std::borrow::Cow::Borrowed(s),
        },
        Object::Name(n) => Object::Name(n.clone()),
        _ => return std::borrow::Cow::Borrowed(s),
    };
    if &direct == filter {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut made = s.clone();
    made.dict.set(b"Filter".to_vec(), direct);
    std::borrow::Cow::Owned(made)
}

/// What the embedded file `s` is for PDF/A-4's 6.9: a PDF whose metadata declares PDF/A-1, 2 or 4,
/// verified at that level where PdfCraft checks it, one embedded file deep. PDF/A-1, 2a and 2u are
/// judged by their declaration alone.
fn embedded_pdfa(doc: &Document, s: &pdfcraft_cos::Stream) -> Embedded {
    let Some(bytes) = read_embedded(doc, s) else { return Embedded::Unchecked };
    let Some(bytes) = bytes else { return Embedded::NotPdfa };
    let Ok(inner) = Document::open(std::sync::Arc::new(bytes)) else { return Embedded::NotPdfa };
    let Some((part, conformance)) = crate::declared(&inner).pdfa.filter(|(part, _)| matches!(part, 1 | 2 | 4)) else {
        return Embedded::NotPdfa;
    };
    // A declaration PdfCraft doesn't verify counts only when it names a level of its part: PDF/A-1a
    // or 1b, 2a or 2u (ISO 19005-1 and -2 6.7.11 and 6.6.4). The letter is compared as the direct
    // checks compare it: in either case for parts 1 and 2, as written for part 4.
    let Some(level) = Level::from_declared(part, &conformance) else {
        let named = match part {
            1 => matches!(conformance.to_ascii_uppercase().as_str(), "A" | "B"),
            2 => matches!(conformance.to_ascii_uppercase().as_str(), "A" | "B" | "U"),
            _ => false,
        };
        return if named { Embedded::Declares } else { Embedded::NotPdfa };
    };
    if NESTED_VERIFY.with(std::cell::Cell::get) >= MAX_NESTED_VERIFY {
        return Embedded::Declares;
    }
    NESTED_VERIFY.with(|n| n.set(n.get().saturating_add(1)));
    let _guard = NestedGuard;
    let found = crate::verify(&inner, level);
    // A file whose only problem is embedded files of its own left unread isn't known to fail.
    if found.is_empty() {
        Embedded::Declares
    } else if found.iter().all(|i| i.pattern == UNCHECKED) {
        Embedded::Unchecked
    } else {
        Embedded::Fails(level.label())
    }
}

/// Whether `name` has the shape of a MIME type: a type and a subtype, neither empty, around one `/`.
fn is_mime_type(name: &[u8]) -> bool {
    let mut parts = name.split(|&b| b == b'/');
    matches!((parts.next(), parts.next(), parts.next()), (Some(t), Some(s), None) if !t.is_empty() && !s.is_empty())
}

/// Where device colour is drawn, for 6.2.4.3: the PDF/A output intent that applies there (`None`
/// when none does, `Some(None)` when one does whose components aren't known, else their number) and
/// the components of the transparency blending space there when it is a device-independent RGB (3)
/// or CMYK (4) one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Place {
    pub(crate) intent: Option<Option<i64>>,
    pub(crate) blend: Option<i64>,
}

/// The place a page's content is drawn in: the page's own PDF/A output intent or else the
/// document's `document` (ISO 32000-2 14.11.5), and the page's transparency group colour space
/// (`/Group /CS`, 11.4.7 and Table 145), the blending space of everything painted on the page.
pub(crate) fn page_place(doc: &Document, page: &Dict, document: Option<Option<i64>>, profiles: &mut Profiles) -> Place {
    let group = page.get(b"Group").map(|g| doc.resolve(g));
    let group = group.as_deref().and_then(Object::as_dict).filter(|g| g.name(b"S") == Some(b"Transparency"));
    let blend = group.and_then(|g| g.get(b"CS")).and_then(|cs| cie_components(doc, cs, profiles));
    Place { intent: plain_intent(intent_components(doc, page, profiles).or(document)), blend }
}

/// The PDF/A output intent of `holder`, the catalog or a page, as 6.2.4.3 reads it: `None` when
/// `holder` has none, else the components of its destination profile. They are taken from the ICC
/// header's data colour space (ICC.1 7.2.6), as [`output_intents`] classes the profile, so a
/// profile counts as what it is whatever its `/N` says; from `/N` only when the profile is too
/// large to read the header of; and are unknown (`Some(None)`) for a profile that is no stream,
/// can't be read, or is of another colour space.
pub(crate) fn intent_components(doc: &Document, holder: &Dict, profiles: &mut Profiles) -> Option<Option<i64>> {
    let list = holder.get(b"OutputIntents").map(|o| doc.resolve(o))?;
    list.as_array()?.iter().find_map(|intent| {
        let intent = doc.resolve(intent);
        let intent = intent.as_dict()?;
        if intent.name(b"S") != Some(b"GTS_PDFA1") {
            return None;
        }
        let profile = intent.get(b"DestOutputProfile")?;
        Some(match profiles.header(doc, profile) {
            Some(ProfileHeader::Read(header)) => match header.get(16..20) {
                Some(b"GRAY") => Some(1),
                Some(b"RGB ") => Some(3),
                Some(b"CMYK") => Some(4),
                _ => None,
            },
            Some(ProfileHeader::TooLarge) => match &*doc.resolve(profile) {
                Object::Stream(s) => s.dict.int(b"N"),
                _ => None,
            },
            Some(ProfileHeader::Broken) | None => None,
        })
    })
}

/// The output intent `intent` as 6.2.4.3 tells intents apart: none, an RGB (3) or a CMYK (4) one,
/// or one of another or an unknown number of components (`Some(None)`). A file can't make more
/// places than that out of the numbers its profiles give.
pub(crate) fn plain_intent(intent: Option<Option<i64>>) -> Option<Option<i64>> {
    intent.map(|n| n.filter(|n| matches!(n, 3 | 4)))
}

/// How far [`colour_places`] reads into one object's direct objects for the references it holds.
const MAX_GROUP_DEPTH: usize = 32;

/// An indirect object as [`colour_places`] takes and groups them.
type Indirect = (pdfcraft_cos::ObjRef, std::sync::Arc<Object>);

/// The objects of `objs` grouped by the places they are drawn in, each group with its places, for
/// 6.2.4.3: PDF/A-4 judges device colour against the output intent and the blending space where it
/// is used.
///
/// The content of the page `pages[i]`, and what it refers to, is drawn at `page_places[i]`. An
/// isolated transparency group XObject (`/I true`) with a group colour space (`/CS`) composites
/// what is painted inside it in that space (ISO 32000-2 11.4.7, Table 145 `/CS`, 11.6.6 and Annex
/// P), so what it refers to is drawn where it is, but in that blending space: a device-independent
/// RGB or CMYK one, or one that allows no device colour. A non-isolated group, or one without
/// `/CS`, blends in its parent's. A page leads to what its resolved dictionary refers to, resources
/// it inherits from the page tree included (ISO 32000-2 7.7.3.4). What a page leads to is drawn only
/// where pages draw it, even when the catalog, its form fields or its structure tree lead to it too;
/// what no page leads to (an object nothing refers to, and what only such objects lead to) is drawn
/// at `elsewhere`. A reference back up (`/Parent`, `/P`) leads nowhere: it draws nothing.
pub(crate) fn colour_places(
    doc: &Document,
    objs: &[Indirect],
    pages: &[pdfcraft_model::Page],
    page_places: &[Place],
    elsewhere: Place,
    profiles: &mut Profiles,
) -> Vec<(Vec<Place>, Vec<Indirect>)> {
    use std::collections::HashMap;
    // The blending space of each isolated group with a colour space of its own, by object number.
    let mut own: HashMap<u32, Option<i64>> = HashMap::new();
    for (r, o) in objs {
        let Object::Stream(s) = &**o else { continue };
        if s.dict.name(b"Subtype") != Some(b"Form") {
            continue;
        }
        let Some(group) = s.dict.get(b"Group").map(|g| doc.resolve(g)) else { continue };
        let Some(group) = group.as_dict().filter(|g| g.name(b"S") == Some(b"Transparency")) else { continue };
        if matches!(group.get(b"I").map(|i| doc.resolve(i)).as_deref(), Some(Object::Bool(true)))
            && let Some(cs) = group.get(b"CS")
        {
            own.insert(r.num, cie_components(doc, cs, profiles));
        }
    }
    // Every object drawn in the one place: nothing to tell apart.
    if page_places.iter().all(|p| *p == elsewhere) && own.values().all(|blend| *blend == elsewhere.blend) {
        return vec![(vec![elsewhere], objs.to_vec())];
    }
    let page_at: HashMap<u32, Place> = pages.iter().zip(page_places).map(|(p, place)| (p.obj.num, *place)).collect();
    let (refers, referred) = references(objs, pages);
    // The places go down the references: first from the pages, then, into what no page leads to,
    // from what nothing refers to, and last from what only a cycle of objects nothing else refers
    // to leads to. An object gains each place at most once, and there are few places, so the walk
    // is linear in the references.
    let mut places: Vec<Vec<Place>> = vec![Vec::new(); objs.len()];
    let mut work = Vec::new();
    for (i, (r, _)) in objs.iter().enumerate() {
        if let (Some(place), Some(list)) = (page_at.get(&r.num), places.get_mut(i)) {
            list.push(*place);
            work.push(i);
        }
    }
    // Whether a page leads to each object, known once the pages' walk is done.
    let mut on_page: Vec<bool> = Vec::new();
    for round in 0..3 {
        while let Some(i) = work.pop() {
            let from = places.get(i).cloned().unwrap_or_default();
            for &j in refers.get(i).map(Vec::as_slice).unwrap_or_default() {
                let Some((target, _)) = objs.get(j) else { continue };
                // A page is drawn in its own place only, and what a page leads to only where pages
                // draw it.
                if page_at.contains_key(&target.num) || on_page.get(j).copied().unwrap_or(false) {
                    continue;
                }
                let blend = own.get(&target.num).copied();
                let Some(list) = places.get_mut(j) else { continue };
                let mut grew = false;
                for place in &from {
                    let place = Place { blend: blend.unwrap_or(place.blend), ..*place };
                    if !list.contains(&place) {
                        list.push(place);
                        grew = true;
                    }
                }
                if grew {
                    work.push(j);
                }
            }
        }
        if round == 0 {
            on_page = places.iter().map(|list| !list.is_empty()).collect();
        }
        for (i, list) in places.iter_mut().enumerate() {
            if list.is_empty() && (round == 1 || !referred.get(i).copied().unwrap_or(true)) {
                list.push(elsewhere);
                work.push(i);
            }
        }
    }
    let mut groups: HashMap<Vec<Place>, Vec<Indirect>> = HashMap::new();
    for (object, mut list) in objs.iter().zip(places) {
        list.sort_unstable();
        groups.entry(list).or_default().push(object.clone());
    }
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    groups
}

/// The name maps a resource dictionary holds (ISO 32000-2 7.8.3, Table 34).
const RESOURCE_MAPS: [&[u8]; 7] = [b"ExtGState", b"ColorSpace", b"Pattern", b"Shading", b"XObject", b"Font", b"Properties"];

/// What each of `objs` refers to, as positions in `objs`, and whether anything refers to it. A
/// page refers to what its dictionary in `pages`, with the attributes it inherits, does. A
/// reference back up (`/Parent`, `/P`) isn't counted, but for a key of a resource name map.
fn references(objs: &[Indirect], pages: &[pdfcraft_model::Page]) -> (Vec<Vec<usize>>, Vec<bool>) {
    use std::collections::{HashMap, HashSet};
    let index: HashMap<u32, usize> = objs.iter().enumerate().map(|(i, (r, _))| (r.num, i)).collect();
    let page_dicts: HashMap<u32, &Dict> = pages.iter().map(|p| (p.obj.num, &p.dict)).collect();
    let object = |num: u32| index.get(&num).and_then(|&j| objs.get(j)).map(|(_, o)| &**o);
    // The name maps of resource dictionaries (ISO 32000-2 7.8.3, Table 34), and of an interactive
    // form's default resources (`/DR`), held as objects of their own. Their keys are the names
    // content uses, so a form or a pattern named `/P` or `/Parent` there is no reference back up.
    let mut maps: HashSet<u32> = HashSet::new();
    for (r, o) in objs {
        let holder = match (page_dicts.get(&r.num), &**o) {
            (Some(page), _) => Some(*page),
            (None, Object::Dict(d)) => Some(d),
            (None, Object::Stream(s)) => Some(&s.dict),
            _ => None,
        };
        for resources in [b"Resources".as_slice(), b"DR"].iter().filter_map(|key| holder?.get(key)) {
            // Through objects that only refer to another, to the resource dictionary itself.
            let mut resources = resources;
            for _ in 0..MAX_GROUP_DEPTH {
                match resources {
                    Object::Ref(t) => match object(t.num) {
                        Some(next) => resources = next,
                        None => break,
                    },
                    _ => break,
                }
            }
            let resources = resources.as_dict();
            for map in RESOURCE_MAPS.iter().filter_map(|key| resources?.get(key)?.as_ref()) {
                // Through objects that only refer to another, to the map itself, each counted.
                let mut at = map;
                for _ in 0..MAX_GROUP_DEPTH {
                    if !maps.insert(at.num) {
                        break;
                    }
                    match object(at.num) {
                        Some(Object::Ref(next)) => at = *next,
                        _ => break,
                    }
                }
            }
        }
    }
    // A dictionary's values. `/Parent` and `/P` refer back up (to a page tree node, a field, a page,
    // a structure element) everywhere but in a name map, and lead nowhere.
    fn entries(d: &Dict, map: bool) -> Vec<(&Object, usize)> {
        d.iter().filter(|(k, _)| map || !matches!(k.as_slice(), b"Parent" | b"P")).map(|(_, v)| (v, 1)).collect()
    }
    let mut refers: Vec<Vec<usize>> = vec![Vec::new(); objs.len()];
    let mut referred = vec![false; objs.len()];
    for (i, (r, o)) in objs.iter().enumerate() {
        let map = maps.contains(&r.num);
        let mut stack: Vec<(&Object, usize)> = match (page_dicts.get(&r.num), &**o) {
            (Some(page), _) => entries(page, false),
            (None, Object::Stream(s)) => entries(&s.dict, map),
            (None, Object::Dict(d)) => entries(d, map),
            // An array of its own, such as a page's `/Contents` (ISO 32000-2 7.7.3.3).
            (None, Object::Array(a)) => a.iter().map(|x| (x, 1)).collect(),
            // An object whose value is a reference to another stands for it.
            (None, Object::Ref(_)) => vec![(&**o, 1)],
            _ => continue,
        };
        let mut found = HashSet::new();
        while let Some((v, depth)) = stack.pop() {
            match v {
                Object::Ref(t) if t.num != r.num => {
                    if let Some(&j) = index.get(&t.num) {
                        found.insert(j);
                    }
                }
                Object::Array(a) if depth < MAX_GROUP_DEPTH => stack.extend(a.iter().map(|x| (x, depth + 1))),
                Object::Dict(d) if depth < MAX_GROUP_DEPTH => stack.extend(d.iter().map(|(_, x)| (x, depth + 1))),
                _ => {}
            }
        }
        for &j in &found {
            if let Some(flag) = referred.get_mut(j) {
                *flag = true;
            }
        }
        if let Some(list) = refers.get_mut(i) {
            list.extend(found);
        }
    }
    (refers, referred)
}

/// The components of `cs` when it is a device-independent RGB-based (3) or CMYK-based (4) colour
/// space a transparency group may blend in (ISO 32000-2 11.3.4): CalRGB, or an ICCBased space
/// whose profile header says `RGB ` or `CMYK` (ICC.1 7.2.6). Lab and the ICC spaces that hold
/// lightness and chromaticity apart may not be blending spaces, and CalGray covers no RGB or CMYK.
fn cie_components(doc: &Document, cs: &Object, profiles: &mut Profiles) -> Option<i64> {
    let cs = doc.resolve(cs);
    let a = cs.as_array()?;
    match doc.resolve(a.first()?).as_name()? {
        b"CalRGB" => Some(3),
        b"ICCBased" => {
            let profile = a.get(1)?;
            let n = match &*doc.resolve(profile) {
                Object::Stream(s) => s.dict.int(b"N")?,
                _ => return None,
            };
            let Some(ProfileHeader::Read(header)) = profiles.header(doc, profile) else { return None };
            match (n, header.get(16..20)?) {
                (3, b"RGB ") => Some(3),
                (4, b"CMYK") => Some(4),
                _ => None,
            }
        }
        _ => None,
    }
}
