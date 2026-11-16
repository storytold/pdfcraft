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
    if subtype == Some(b"Widget") && dict.get(b"A").is_some() {
        issue("6.4.1", "A widget annotation has an action (A)", None, page, false);
    }
}

/// How many `/Parent` links [`field_type`] follows.
const MAX_FIELD_DEPTH: usize = 32;

/// The field type (`/FT`) of the widget `dict`: its own, or inherited from its parents (ISO 32000-2
/// 12.7.4.1, Table 226).
fn field_type(doc: &Document, dict: &Dict) -> Option<Vec<u8>> {
    let mut current = dict.clone();
    for _ in 0..MAX_FIELD_DEPTH {
        if let Some(ft) = current.get(b"FT").map(|f| doc.resolve(f)) {
            return ft.as_name().map(<[u8]>::to_vec);
        }
        let parent = doc.resolve(current.get(b"Parent")?);
        current = parent.as_dict()?.clone();
    }
    None
}

/// The standard filters of ISO 32000-2 7.4, Table 6.
const FILTERS: &[&[u8]] = &[
    b"ASCIIHexDecode",
    b"ASCII85Decode",
    b"LZWDecode",
    b"FlateDecode",
    b"RunLengthDecode",
    b"CCITTFaxDecode",
    b"JBIG2Decode",
    b"DCTDecode",
    b"JPXDecode",
    b"Crypt",
];

/// 6.1.6.1, 6.1.6.2, 6.2.8.1 and 6.2.8.2: a stream's `filters` are the standard ones (LZW, which
/// PDF/A-2 forbids too, is reported with its rules), a Crypt filter only the Identity one (ISO
/// 32000-2 7.4.10, Table 14); a 3D stream is U3D or PRC (ISO 32000-2 13.6.3, Table 311); and a
/// form XObject has no OPI dictionary and refers to no other file (`/Ref`, a reference XObject,
/// ISO 32000-2 8.10.4).
pub(crate) fn stream(doc: &Document, dict: &Dict, subtype: Option<&[u8]>, filters: &[Vec<u8>], issue: &mut Report<'_>) {
    for filter in filters.iter().filter(|f| !FILTERS.contains(&f.as_slice())) {
        let shown = String::from_utf8_lossy(filter.get(..filter.len().min(64)).unwrap_or_default()).into_owned();
        issue("6.1.6.2", "A stream uses the filter {}, which ISO 32000-2 doesn't define", Some(shown), None, false);
    }
    if let Some(at) = filters.iter().position(|f| f.as_slice() == b"Crypt") {
        let parameters = dict.get(b"DecodeParms").map(|p| doc.resolve(p));
        let parameters = match parameters.as_deref() {
            Some(Object::Array(list)) => list.get(at).map(|p| doc.resolve(p)),
            _ => parameters,
        };
        // Without decode parameters, or a Name in them, the crypt filter is Identity (Table 14).
        let identity =
            parameters.as_deref().and_then(Object::as_dict).is_none_or(|p| p.get(b"Name").is_none() || p.name(b"Name") == Some(b"Identity"));
        if !identity {
            issue("6.1.6.2", "A stream uses the Crypt filter with a crypt filter other than Identity", None, None, false);
        }
    }
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
pub(crate) fn document(doc: &Document, catalog: &Dict, issue: &mut Report<'_>) {
    information(doc, catalog, issue);
    catalog_version(doc, catalog, issue);
    catalog_entries(doc, catalog, issue);
    metadata_stream(catalog, doc, issue);
    output_intents(doc, catalog, None, issue);
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
    let Some(version) = catalog.get(b"Version").map(|v| doc.resolve(v)) else { return };
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
pub(crate) fn page(doc: &Document, page: &Dict, index: usize, issue: &mut Report<'_>) {
    if page.get(b"PresSteps").is_some() {
        issue("6.11", "A page has presentation steps (PresSteps)", None, Some(index), false);
    }
    output_intents(doc, page, Some(index), issue);
}

/// How many optional content configurations [`optional_content_names`] reads.
const MAX_CONFIGURATIONS: usize = 4096;

/// 6.10: each optional content configuration (`/D` and those in `/Configs`, ISO 32000-2 8.11.4.2)
/// has a `/Name`, and no two have the same one.
fn optional_content_names(doc: &Document, properties: &Dict, issue: &mut Report<'_>) {
    let mut configurations: Vec<Object> = properties.get(b"D").cloned().into_iter().collect();
    if let Some(list) = properties.get(b"Configs").map(|c| doc.resolve(c))
        && let Some(list) = list.as_array()
    {
        configurations.extend(list.iter().take(MAX_CONFIGURATIONS).cloned());
    }
    let mut names = std::collections::HashSet::new();
    let (mut unnamed, mut repeated) = (false, false);
    for configuration in &configurations {
        let configuration = doc.resolve(configuration);
        let Some(configuration) = configuration.as_dict() else { continue };
        match configuration.get(b"Name").map(|n| doc.resolve(n)).and_then(|n| n.as_string().map(|s| s.bytes.clone())) {
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
    // What can't be decoded within the limit `lib.rs` reads metadata with is reported there as no
    // metadata at all.
    let Ok(bytes) = stream.decoded_within(xmp::MAX_XMP_BYTES) else { return };
    packet(&bytes, None, issue);
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
    if let Some(start) = text.find("<?xpacket")
        && let Some(header) = text.get(start..).and_then(|rest| rest.find("?>").and_then(|end| rest.get(..end)))
        && (header.contains(" bytes=") || header.contains(" encoding="))
    {
        match named {
            Some(o) => issue("6.7.2.1", "The XMP packet header in metadata stream {} has a bytes or encoding attribute", Some(o), None, false),
            None => issue("6.7.2.1", "The XMP packet header has a bytes or encoding attribute", None, None, false),
        }
    }
}

/// The most of all component metadata streams [`component_metadata`] decodes, together.
const MAX_COMPONENT_METADATA: usize = 64 << 20;

/// 6.7.2.1 for the metadata streams of the document's components (ISO 32000-2 14.3.2: pages,
/// XObjects, fonts and the other dictionaries with a `/Metadata` entry): their packets are held to
/// the same encoding rules as the catalog's. Each stream is read once, the catalog's not again, and
/// all of them within [`MAX_COMPONENT_METADATA`]; one too large to decode is left alone, as the
/// catalog's is.
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
        let Ok(bytes) = budget.decode_within(stream, xmp::MAX_XMP_BYTES) else { continue };
        packet(&bytes, Some(r), issue);
    }
}

/// The most of an ICC profile [`output_intents`] and [`cie_components`] decode to read its header;
/// real ICC profiles stay well under it.
const MAX_ICC_HEADER_READ: usize = 16 << 20;

/// How many output intents [`output_intents`] reads.
const MAX_INTENTS: usize = 4096;

/// 6.2.3: in the output intents of `holder`, the catalog or the page `page` (ISO 32000-2 14.11.5
/// lets both have them), none names an external profile (`/DestOutputProfileRef`, Table 401), all
/// that have a `/DestOutputProfile` share one profile object, and the PDF/A one's profile is an
/// output (`prtr`) or monitor (`mntr`) profile of gray, RGB or CMYK (the ICC header's device class
/// and data colour space, ICC.1 7.2.5 and 7.2.6).
fn output_intents(doc: &Document, holder: &Dict, page: Option<usize>, issue: &mut Report<'_>) {
    let Some(list) = holder.get(b"OutputIntents").map(|o| doc.resolve(o)) else { return };
    let Some(list) = list.as_array() else { return };
    let mut profiles = std::collections::HashSet::new();
    let (mut external, mut direct) = (false, 0usize);
    for intent in list.iter().take(MAX_INTENTS) {
        let intent = doc.resolve(intent);
        let Some(intent) = intent.as_dict() else { continue };
        external |= intent.get(b"DestOutputProfileRef").is_some();
        if intent.name(b"S") == Some(b"GTS_PDFA1")
            && let Some(profile) = intent.get(b"DestOutputProfile").map(|p| doc.resolve(p))
            && let Object::Stream(profile) = &*profile
            && let Ok(header) = profile.decoded_within(MAX_ICC_HEADER_READ)
            && !(matches!(header.get(12..16), Some(b"prtr" | b"mntr")) && matches!(header.get(16..20), Some(b"GRAY" | b"RGB " | b"CMYK")))
        {
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
}

/// 6.1.3: nothing follows the last end-of-file marker but an end-of-line (ISO 32000-2 7.5.5). A
/// document its next save rewrites is judged by that save, which ends the file itself.
fn end_of_file(doc: &Document, issue: &mut Report<'_>) {
    if doc.full_save_required() || doc.encryption_changed() {
        return;
    }
    let data = doc.bytes();
    let Some(end) = data.windows(5).rposition(|w| w == b"%%EOF") else { return };
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
/// ([`embedded_pdfa`]).
pub(crate) fn embedded_file(doc: &Document, level: Level, spec: &Dict, issue: &mut Report<'_>) {
    let streams = spec.get(b"EF").map(|ef| doc.resolve(ef)).and_then(|ef| ef.as_dict().cloned()).unwrap_or_default();
    let streams: Vec<_> = streams.iter().map(|(_, s)| doc.resolve(s)).collect();
    if level == Level::A4 {
        let mut not_pdfa = false;
        for stream in &streams {
            match &**stream {
                Object::Stream(s) => match embedded_pdfa(s) {
                    Embedded::NotPdfa => not_pdfa = true,
                    Embedded::Declares => {}
                    Embedded::Fails(label) => {
                        issue("6.9", "An embedded file declares {} but doesn't conform to it", Some(label.to_owned()), None, false)
                    }
                },
                _ => not_pdfa = true,
            }
        }
        if not_pdfa {
            issue("6.9", "Embedded files must be PDF/A-1, PDF/A-2 or PDF/A-4 files themselves (use PDF/A-4f for others)", None, None, false);
        }
    }
    if spec.get(b"AFRelationship").is_none() {
        issue("6.9", "An embedded file has no AFRelationship", None, None, false);
    }
    if spec.get(b"F").is_none() || spec.get(b"UF").is_none() {
        issue("6.9", "An embedded file's file specification lacks F or UF", None, None, false);
    }
    for stream in &streams {
        let Object::Stream(s) = &**stream else { continue };
        if !s.dict.name(b"Subtype").is_some_and(is_mime_type) {
            issue("6.9", "An embedded file has no MIME type (Subtype)", None, None, false);
        }
    }
}

/// The largest embedded file [`embedded_pdfa`] opens.
const MAX_EMBEDDED_PDF: usize = 64 << 20;

/// How many embedded files deep [`embedded_pdfa`] verifies: an embedded file's own embedded files
/// are judged by their declaration only.
const MAX_NESTED_VERIFY: usize = 1;

thread_local! {
    /// How many embedded-file verifications this thread is inside.
    static NESTED_VERIFY: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// What [`embedded_pdfa`] finds an embedded file to be.
enum Embedded {
    /// Not a PDF, or one that declares no PDF/A part 1, 2 or 4.
    NotPdfa,
    /// A PDF that declares PDF/A-1, 2 or 4, and passes verification at that level when it is one
    /// [`verify`](crate::verify) checks (PDF/A-2b, 4, 4e or 4f) and the file isn't nested deeper
    /// than [`MAX_NESTED_VERIFY`].
    Declares,
    /// A PDF that declares this level and fails its verification.
    Fails(&'static str),
}

/// Leaves one embedded-file verification when dropped.
struct NestedGuard;

impl Drop for NestedGuard {
    fn drop(&mut self) {
        NESTED_VERIFY.with(|n| n.set(n.get().saturating_sub(1)));
    }
}

/// What the embedded file `s` is for PDF/A-4's 6.9: a PDF whose metadata declares PDF/A-1, 2 or 4,
/// verified at that level where PdfCraft checks it, one embedded file deep. PDF/A-1, 2a and 2u are
/// judged by their declaration alone.
fn embedded_pdfa(s: &pdfcraft_cos::Stream) -> Embedded {
    let Ok(bytes) = s.decoded_within(MAX_EMBEDDED_PDF) else { return Embedded::NotPdfa };
    let Ok(inner) = Document::open(std::sync::Arc::new(bytes.to_vec())) else { return Embedded::NotPdfa };
    let Some((part, conformance)) = crate::declared(&inner).pdfa.filter(|(part, _)| matches!(part, 1 | 2 | 4)) else {
        return Embedded::NotPdfa;
    };
    let Some(level) = Level::from_declared(part, &conformance) else { return Embedded::Declares };
    if NESTED_VERIFY.with(std::cell::Cell::get) >= MAX_NESTED_VERIFY {
        return Embedded::Declares;
    }
    NESTED_VERIFY.with(|n| n.set(n.get().saturating_add(1)));
    let _guard = NestedGuard;
    if crate::verify(&inner, level).is_empty() { Embedded::Declares } else { Embedded::Fails(level.label()) }
}

/// Whether `name` has the shape of a MIME type: a type and a subtype, neither empty, around one `/`.
fn is_mime_type(name: &[u8]) -> bool {
    let mut parts = name.split(|&b| b == b'/');
    matches!((parts.next(), parts.next(), parts.next()), (Some(t), Some(s), None) if !t.is_empty() && !s.is_empty())
}

/// 6.2.4.3: the device colour space that every page's transparency group makes device-independent
/// for PDF/A-4, as the number of its components: 3 for DeviceRGB, 4 for DeviceCMYK.
///
/// PDF/A-4 accepts DeviceRGB or DeviceCMYK where the current transparency blending space is a
/// device-independent RGB- or CMYK-based one. A page's group colour space (`/Group /CS`, ISO
/// 32000-2 11.4.7 and Table 145) is the blending space of everything painted on the page, and a
/// device space of as many components inside it is taken as that space (11.7.2). Device colour is
/// judged for the document as a whole here, so the allowance holds only when every page has a
/// transparency group with a CalRGB or an ICCBased RGB space (3), or with an ICCBased CMYK one (4),
/// the same number for all of them. A form XObject's own group is [`group_blending_components`].
pub(crate) fn page_blending_components(doc: &Document, pages: &[pdfcraft_model::Page]) -> Option<i64> {
    let mut found = None;
    for p in pages {
        let group = p.dict.get(b"Group").map(|g| doc.resolve(g));
        let group = group.as_deref().and_then(Object::as_dict).filter(|g| g.name(b"S") == Some(b"Transparency"))?;
        let n = cie_components(doc, group.get(b"CS")?)?;
        if found.is_some_and(|f| f != n) {
            return None;
        }
        found = Some(n);
    }
    found
}

/// How far [`group_blending_components`] follows references back towards a page, and how deep it
/// reads into one object's direct objects for the references it holds.
const MAX_GROUP_DEPTH: usize = 32;

/// 6.2.4.3: the objects whose device colour an isolated transparency group of their own makes
/// device-independent for PDF/A-4, each with the components of that group's space (3 or 4), by
/// object number.
///
/// An isolated transparency group XObject (`/I true`) with a group colour space (`/CS`) composites
/// everything painted inside it in that space, and a device space of as many components is taken
/// as it (ISO 32000-2 11.4.7, Table 145 `/CS`, 11.6.6 and Annex P). That holds for the form's own
/// content and resources, and for anything they alone refer to: an object belongs to a group's
/// space when every object referring to it does, and anything else referring to it (a page, an
/// annotation, a structure element) leaves it out. A non-isolated group, or one without `/CS`,
/// takes its parent's space, so it is judged by what refers to it in the same way; an isolated
/// group whose `/CS` isn't a device-independent RGB or CMYK space covers nothing inside it.
pub(crate) fn group_blending_components(
    doc: &Document,
    objs: &[(pdfcraft_cos::ObjRef, std::sync::Arc<Object>)],
) -> std::collections::HashMap<u32, i64> {
    use std::collections::{HashMap, HashSet};
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
            own.insert(r.num, cie_components(doc, cs));
        }
    }
    if !own.values().any(Option::is_some) {
        return HashMap::new();
    }
    // Who refers to each object.
    let mut referrers: HashMap<u32, Vec<u32>> = HashMap::new();
    for (r, o) in objs {
        let mut found = HashSet::new();
        let mut stack: Vec<(&Object, usize)> = match &**o {
            Object::Stream(s) => s.dict.iter().map(|(_, x)| (x, 1)).collect(),
            other => vec![(other, 0)],
        };
        while let Some((v, depth)) = stack.pop() {
            match v {
                Object::Ref(t) if t.num != r.num => {
                    found.insert(t.num);
                }
                Object::Array(a) if depth < MAX_GROUP_DEPTH => stack.extend(a.iter().map(|x| (x, depth + 1))),
                Object::Dict(d) if depth < MAX_GROUP_DEPTH => stack.extend(d.iter().map(|(_, x)| (x, depth + 1))),
                _ => {}
            }
        }
        for t in found {
            referrers.entry(t).or_default().push(r.num);
        }
    }
    let mut known: HashMap<u32, Option<i64>> = HashMap::new();
    let mut out = HashMap::new();
    for (r, _) in objs {
        let mut open = HashSet::new();
        if let Some(n) = covered_by_group(r.num, 0, &own, &referrers, &mut known, &mut open) {
            out.insert(r.num, n);
        }
    }
    out
}

/// The components of the group space object `num` is painted in, when it is always the same
/// device-independent RGB or CMYK one: see [`group_blending_components`]. `open` holds the objects
/// being worked out further up, which a cycle back to them leaves uncovered.
fn covered_by_group(
    num: u32,
    depth: usize,
    own: &std::collections::HashMap<u32, Option<i64>>,
    referrers: &std::collections::HashMap<u32, Vec<u32>>,
    known: &mut std::collections::HashMap<u32, Option<i64>>,
    open: &mut std::collections::HashSet<u32>,
) -> Option<i64> {
    if let Some(n) = own.get(&num) {
        return *n;
    }
    if let Some(n) = known.get(&num) {
        return *n;
    }
    if depth >= MAX_GROUP_DEPTH || !open.insert(num) {
        return None;
    }
    let mut found = None;
    for &p in referrers.get(&num).map(Vec::as_slice).unwrap_or_default() {
        match covered_by_group(p, depth + 1, own, referrers, known, open) {
            Some(n) if found.is_none_or(|f| f == n) => found = Some(n),
            _ => {
                found = None;
                break;
            }
        }
    }
    open.remove(&num);
    known.insert(num, found);
    found
}

/// The components of `cs` when it is a device-independent RGB-based (3) or CMYK-based (4) colour
/// space a transparency group may blend in (ISO 32000-2 11.3.4): CalRGB, or an ICCBased space
/// whose profile header says `RGB ` or `CMYK` (ICC.1 7.2.6). Lab and the ICC spaces that hold
/// lightness and chromaticity apart may not be blending spaces, and CalGray covers no RGB or CMYK.
fn cie_components(doc: &Document, cs: &Object) -> Option<i64> {
    let cs = doc.resolve(cs);
    let a = cs.as_array()?;
    match doc.resolve(a.first()?).as_name()? {
        b"CalRGB" => Some(3),
        b"ICCBased" => {
            let profile = doc.resolve(a.get(1)?);
            let Object::Stream(s) = &*profile else { return None };
            let header = s.decoded_within(MAX_ICC_HEADER_READ).ok()?;
            match (s.dict.int(b"N")?, header.get(16..20)?) {
                (3, b"RGB ") => Some(3),
                (4, b"CMYK") => Some(4),
                _ => None,
            }
        }
        _ => None,
    }
}
