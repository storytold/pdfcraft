//! PDF/A-4 verification (ISO 19005-4: PDF/A-4, 4e and 4f), case by case.
//!
//! Every file is written here by hand, in the shape the rule describes. The rules come from ISO
//! 32000-2 and from the rule descriptions the veraPDF validator prints, run as an external program
//! (see `src/part4.rs`); the same shapes, run through veraPDF 1.30.4, got the answers pinned here.

use pdfcraft_cos::Document;
use pdfcraft_preflight::{Issue, Level, WORDINGS, convert, icc, verify};

/// The parts of a hand-written file: catalog 1, page tree 2, metadata 3, page 4, content 5, then
/// `more` from 6 on.
#[derive(Clone)]
struct File {
    header: &'static str,
    packet: Vec<u8>,
    catalog: String,
    page: String,
    resources: &'static str,
    content: &'static str,
    more: Vec<Vec<u8>>,
    trailer: &'static str,
    /// Bytes after the final `%%EOF` line.
    after: &'static [u8],
}

impl Default for File {
    fn default() -> Self {
        File {
            header: "%PDF-2.0",
            packet: packet("4", Some("2020"), None),
            catalog: String::new(),
            page: String::new(),
            resources: "",
            content: "",
            more: Vec::new(),
            trailer: "",
            after: b"",
        }
    }
}

/// A file like the default one with `catalog` entries.
fn with_catalog(catalog: &str) -> File {
    File { catalog: catalog.to_owned(), ..File::default() }
}

/// A file like the default one with `page` entries.
fn with_page(page: &str) -> File {
    File { page: page.to_owned(), ..File::default() }
}

/// An object written as text.
fn o(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

/// An XMP packet identifying a file as `part`, with `rev` and `conformance` when given.
fn packet(part: &str, rev: Option<&str>, conformance: Option<&str>) -> Vec<u8> {
    let rev = rev.map(|r| format!("<pdfaid:rev>{r}</pdfaid:rev>")).unwrap_or_default();
    let conformance = conformance.map(|c| format!("<pdfaid:conformance>{c}</pdfaid:conformance>")).unwrap_or_default();
    described(&format!("<pdfaid:part>{part}</pdfaid:part>{rev}{conformance}"), "")
}

/// An XMP packet whose description holds `properties`, with `extra` written in its start tag.
fn described(properties: &str, extra: &str) -> Vec<u8> {
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n\
<rdf:Description rdf:about=\"\" xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\" {extra}>{properties}</rdf:Description>\n\
</rdf:RDF></x:xmpmeta>\n<?xpacket end=\"w\"?>"
    )
    .into_bytes()
}

/// The PDF/A-4 identification with `properties` after it.
fn identified_with(properties: &str, namespaces: &str) -> Vec<u8> {
    described(&format!("<pdfaid:part>4</pdfaid:part><pdfaid:rev>2020</pdfaid:rev>{properties}"), namespaces)
}

/// A stream object with `dict` entries and `data`.
fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    out
}

fn bytes(f: &File) -> Vec<u8> {
    let mut objects = vec![
        o(&format!("<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R {} >>", f.catalog)),
        o("<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
        stream("/Type /Metadata /Subtype /XML", &f.packet),
        o(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R /Resources << {} >> {} >>", f.resources, f.page)),
        stream("", f.content.as_bytes()),
    ];
    objects.extend(f.more.iter().cloned());
    raw(f.header, &objects, f.trailer, f.after)
}

/// A file with `objects` numbered from 1, the header line `header` followed by a binary comment.
fn raw(header: &str, objects: &[Vec<u8>], trailer: &str, after: &[u8]) -> Vec<u8> {
    let mut out = format!("{header}\n").into_bytes();
    out.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(object);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    let id = "/ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]";
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R {id} {trailer} >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    out.extend_from_slice(after);
    out
}

fn open(bytes: Vec<u8>) -> Document {
    let doc = Document::open(std::sync::Arc::new(bytes)).unwrap();
    assert_eq!(pdfcraft_model::pages(&doc).len(), 1, "the page of this case does not resolve");
    doc
}

fn issues(f: &File, level: Level) -> Vec<Issue> {
    verify(&open(bytes(f)), level)
}

/// Every clause a PDF/A-4 verification may cite in these cases: the numbers veraPDF prints for
/// its PDF/A-4 rules and the veraPDF corpus's directories use.
const PART4_CLAUSES: &[&str] = &[
    "6.1.2",
    "6.1.3",
    "6.1.6.1",
    "6.1.6.2",
    "6.1.11",
    "6.1.12",
    "6.2.3",
    "6.2.4.3",
    "6.2.4.4",
    "6.2.5",
    "6.2.7.1",
    "6.2.8.1",
    "6.2.8.2",
    "6.2.9",
    "6.2.10.4.1",
    "6.3.1",
    "6.3.2",
    "6.3.3",
    "6.4.1",
    "6.4.2",
    "6.6.1",
    "6.6.3",
    "6.7.2.1",
    "6.7.3",
    "6.9",
    "6.10",
    "6.11",
    "6.12",
];

#[track_caller]
fn passes(f: &File, level: Level) {
    let found = issues(f, level);
    assert!(found.is_empty(), "{level:?}: {found:#?}");
}

/// Asserts `f` breaks `clause` at `level` with a message containing `words`, and that every issue
/// is PDF/A-4's own: a PDF/A-4 clause, worded from the list interfaces translate, and not fixable.
#[track_caller]
fn reports(f: &File, level: Level, clause: &str, words: &str) {
    let found = issues(f, level);
    assert!(found.iter().any(|i| i.clause == clause && i.message.contains(words)), "no {clause} {words:?} at {level:?} in {found:#?}");
    for i in &found {
        assert!(PART4_CLAUSES.contains(&i.clause), "{} is not a PDF/A-4 clause: {i:#?}", i.clause);
        assert!(!i.fixable, "Save as PDF/A doesn't make PDF/A-4 files: {i:#?}");
        assert!(WORDINGS.contains(&i.pattern), "not listed for translation: {:?}", i.pattern);
    }
}

#[test]
fn a_minimal_pdfa4_file_passes_and_its_levels_are_named() {
    passes(&File::default(), Level::A4);
    assert_eq!(Level::from_id("4"), Some(Level::A4));
    assert_eq!(Level::from_id("PDF/A-4e"), Some(Level::A4e));
    assert_eq!(Level::from_id("4f"), Some(Level::A4f));
    assert_eq!(Level::A4f.label(), "PDF/A-4f");
    assert_eq!(Level::A4e.part(), 4);
    assert_eq!(Level::from_declared(4, ""), Some(Level::A4));
    assert_eq!(Level::from_declared(4, "E"), Some(Level::A4e));
    assert_eq!(Level::from_declared(4, "F"), Some(Level::A4f));
    assert_eq!(Level::from_declared(4, "B"), None);
    assert_eq!(Level::from_declared(2, "B"), Some(Level::A2b));
    assert_eq!(Level::from_declared(3, "B"), Some(Level::A3b));
    assert_eq!(Level::from_declared(1, "B"), None);
}

#[test]
fn a_pdfa4_file_is_not_judged_by_pdfa2_rules() {
    // What a PDF/A-2b verification says of a PDF/A-4 file (PDFBOX-6088's blanko_PDFA4.pdf had
    // these two), PDF/A-4 doesn't.
    let f = File::default();
    let as_2b: Vec<String> = issues(&f, Level::A2b).into_iter().map(|i| i.message).collect();
    assert!(as_2b.iter().any(|m| m == "The metadata doesn't identify the file as PDF/A-2b"), "{as_2b:#?}");
    assert!(as_2b.iter().any(|m| m.contains("%PDF-1.0 to %PDF-1.7")), "{as_2b:#?}");
    passes(&f, Level::A4);
}

#[test]
fn a_pdfa2b_file_still_verifies_as_2b_and_not_as_4() {
    let f = File { header: "%PDF-1.7", packet: packet("2", None, Some("B")), ..File::default() };
    passes(&f, Level::A2b);
    reports(&f, Level::A4, "6.1.2", "%PDF-2.0 to %PDF-2.9");
    reports(&f, Level::A4, "6.7.3", "doesn't identify the file as PDF/A-4");
    reports(&f, Level::A4, "6.7.3", "no pdfaid:rev");
}

#[test]
fn the_header_is_pdf_2_n() {
    reports(&File { header: "%PDF-1.7", ..File::default() }, Level::A4, "6.1.2", "%PDF-2.0 to %PDF-2.9");
    passes(&File { header: "%PDF-2.1", ..File::default() }, Level::A4);
}

#[test]
fn identification_names_part_4_the_2020_revision_and_the_levels_letter() {
    let id = |part, rev, conformance| File { packet: packet(part, rev, conformance), ..File::default() };
    let mismatch = "doesn't identify the file as";
    reports(&id("4", Some("2020"), Some("B")), Level::A4, "6.7.3", mismatch);
    reports(&id("4", Some("2020"), Some(" ")), Level::A4, "6.7.3", mismatch);
    reports(&id("3", Some("2020"), None), Level::A4, "6.7.3", mismatch);
    reports(&id("4", Some("2020"), None), Level::A4f, "6.7.3", "as PDF/A-4f");
    reports(&id("4", Some("2021"), None), Level::A4, "6.7.3", "pdfaid:rev as 2021, not 2020");
    reports(&id("4", None, None), Level::A4, "6.7.3", "no pdfaid:rev");
    // The letter is compared as written.
    reports(&id("4", Some("2020"), Some("e")), Level::A4e, "6.7.3", "as PDF/A-4e");
    passes(&id("4", Some("2020"), Some("E")), Level::A4e);
    let other_prefix = described("<a:part>4</a:part><a:rev>2020</a:rev>", "xmlns:a=\"http://www.aiim.org/pdfa/ns/id/\"");
    reports(&File { packet: other_prefix, ..File::default() }, Level::A4, "6.7.3", "isn't written with the prefix pdfaid");
    let attributes = described("", "pdfaid:part=\"4\" pdfaid:rev=\"2020\"");
    passes(&File { packet: attributes, ..File::default() }, Level::A4);
}

#[test]
fn the_information_dictionary_holds_only_moddate_and_only_with_pieceinfo() {
    let piece = "/PieceInfo << /X << /LastModified (D:20240101000000Z) /Private 1 >> >> /LastModified (D:20240101000000Z)";
    let info = |dict: &str, catalog: &str| File { more: vec![o(dict)], trailer: "/Info 6 0 R", ..with_catalog(catalog) };
    reports(&info("<< /ModDate (D:20240101000000Z) >>", ""), Level::A4, "6.1.3", "but the catalog has no PieceInfo");
    passes(&info("<< /ModDate (D:20240101000000Z) >>", piece), Level::A4);
    reports(&info("<< /Title (x) /ModDate (D:20240101000000Z) >>", piece), Level::A4, "6.1.3", "holds Title: only ModDate is allowed");
    // PDF/A-2's title kept in step with the information dictionary is not a PDF/A-4 rule.
    let titled = identified_with(
        "<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">A</rdf:li></rdf:Alt></dc:title>",
        "xmlns:dc=\"http://purl.org/dc/elements/1.1/\"",
    );
    passes(&File { packet: titled, ..File::default() }, Level::A4);
}

#[test]
fn the_catalog_version_is_2_n() {
    reports(&with_catalog("/Version /1.7"), Level::A4, "6.1.12", "version 1.7, not 2.0 to 2.9");
    reports(&with_catalog("/Version (2.0)"), Level::A4, "6.1.12", "something other than a name");
    passes(&with_catalog("/Version /2.0"), Level::A4);
}

#[test]
fn javascript_is_allowed_and_forbidden_actions_are_not() {
    passes(&with_catalog("/OpenAction << /S /JavaScript /JS (1;) >>"), Level::A4);
    passes(&with_catalog("/Names << /JavaScript << /Names [(a) << /S /JavaScript /JS (1;) >>] >> >>"), Level::A4);
    assert!(issues(&with_catalog("/OpenAction << /S /JavaScript /JS (1;) >>"), Level::A2b).iter().any(|i| i.message.contains("JavaScript")));
    reports(&with_catalog("/OpenAction << /S /Trans /Trans << >> >>"), Level::A4, "6.6.1", "A Trans action");
    reports(&with_catalog("/OpenAction << /S /GoTo /D [4 0 R /Fit] /Next << /S /Launch /F (x) >> >>"), Level::A4, "6.6.1", "Launch");
    let optional = with_catalog("/OpenAction << /S /SetOCGState /State [] >>");
    reports(&optional, Level::A4, "6.6.1", "SetOCGState");
    reports(&File { packet: packet("4", Some("2020"), Some("F")), ..optional.clone() }, Level::A4f, "6.6.1", "SetOCGState");
    passes(&File { packet: packet("4", Some("2020"), Some("E")), ..optional }, Level::A4e);
}

#[test]
fn additional_actions_hold_only_pointer_and_focus_triggers() {
    let go = "<< /S /GoTo /D [4 0 R /Fit] >>";
    reports(&with_catalog(&format!("/AA << /WC {go} >>")), Level::A4, "6.6.3", "trigger WC");
    reports(&with_page(&format!("/AA << /O {go} >>")), Level::A4, "6.6.3", "trigger O");
    passes(&with_page(&format!("/AA << /E {go} /X {go} >>")), Level::A4);
    let link = o(&format!("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] /F 4 /AA << /C {go} >> >>"));
    reports(&File { more: vec![link], ..with_page("/Annots [6 0 R]") }, Level::A4, "6.6.3", "Link annotation's additional actions");
    // A widget and a form field may have any.
    let widget = o(&format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T (a) /Rect [0 0 0 0] /F 4 /P 4 0 R /AA << /K {go} >> >>"));
    let form = File { page: "/Annots [6 0 R]".to_owned(), more: vec![widget], ..with_catalog("/AcroForm << /Fields [6 0 R] >>") };
    passes(&form, Level::A4);
    let field = o("<< /FT /Tx /T (a) /AA << /K << /S /JavaScript /JS (1;) >> >> /Kids [] >>");
    passes(&File { more: vec![field], ..with_catalog("/AcroForm << /Fields [6 0 R] >>") }, Level::A4);
}

#[test]
fn annotation_types_follow_the_level() {
    let on_page = |subtype: &str, conformance: Option<&str>| File {
        packet: packet("4", Some("2020"), conformance),
        more: vec![o(&format!("<< /Type /Annot /Subtype /{subtype} /Rect [0 0 0 0] /F 4 >>"))],
        ..with_page("/Annots [6 0 R]")
    };
    reports(&on_page("FileAttachment", None), Level::A4, "6.3.1", "FileAttachment annotations aren't allowed");
    reports(&on_page("3D", None), Level::A4, "6.3.1", "3D annotations aren't allowed");
    reports(&on_page("RichMedia", Some("F")), Level::A4f, "6.3.1", "RichMedia annotations aren't allowed");
    reports(&on_page("FileAttachment", Some("E")), Level::A4e, "6.3.1", "FileAttachment annotations aren't allowed");
    passes(&on_page("3D", Some("E")), Level::A4e);
    passes(&on_page("RichMedia", Some("E")), Level::A4e);
    reports(&on_page("Sound", Some("E")), Level::A4e, "6.3.1", "Sound annotations aren't allowed");
    // Types ISO 32000-2 Table 171 doesn't list, the name compared as written.
    reports(&on_page("line", None), Level::A4, "6.3.1", "line annotations aren't allowed");
    // A file attachment is PDF/A-4f's (which also lists its embedded files).
    let attached = File { catalog: "/Names << /EmbeddedFiles << /Names [] >> >>".to_owned(), ..on_page("FileAttachment", Some("F")) };
    passes(&attached, Level::A4f);
    // A projection annotation needs no appearance stream, even one with an area.
    let projection = File { more: vec![o("<< /Type /Annot /Subtype /Projection /Rect [0 0 10 10] /F 4 >>")], ..with_page("/Annots [6 0 R]") };
    passes(&projection, Level::A4);
    assert!(issues(&on_page("Square", None), Level::A4).is_empty(), "a zero-area annotation needs no appearance");
}

#[test]
fn annotations_print_show_and_have_only_a_normal_appearance() {
    let link = |entries: &str| File {
        more: vec![o(&format!("<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] {entries} >>"))],
        ..with_page("/Annots [6 0 R]")
    };
    reports(&link("/F 260"), Level::A4, "6.3.2", "isn't set to print, or is hidden");
    passes(&link("/F 4"), Level::A4);
    let appearance = stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", b"");
    let square = |ap: &str| File {
        more: vec![o(&format!("<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /F 4 /AP {ap} >>")), appearance.clone()],
        ..with_page("/Annots [6 0 R]")
    };
    passes(&square("<< /N 7 0 R >>"), Level::A4);
    reports(&square("<< /N 7 0 R /D 7 0 R >>"), Level::A4, "6.3.3", "appearance dictionary holds more than N");
    let widget = |entries: &str| File {
        page: "/Annots [6 0 R]".to_owned(),
        more: vec![o(&format!("<< /Type /Annot /Subtype /Widget /T (w) /Rect [0 0 10 10] /F 4 /P 4 0 R {entries} >>")), appearance.clone()],
        ..with_catalog("/AcroForm << /Fields [6 0 R] >>")
    };
    // A button's normal appearance is a dictionary of states.
    passes(&widget("/FT /Btn /AP << /N << /Off 7 0 R >> >>"), Level::A4);
    reports(&widget("/FT /Btn /AP << /N 7 0 R >>"), Level::A4, "6.3.3", "button's normal appearance");
    reports(&widget("/FT /Tx /AP << /N 7 0 R >> /A << /S /GoTo /D [4 0 R /Fit] >>"), Level::A4, "6.4.1", "widget annotation has an action");
}

#[test]
fn graphics_states_have_no_halftone_origin() {
    let state = |entry: &str| File {
        resources: "/ExtGState << /G0 6 0 R >>",
        content: "/G0 gs",
        more: vec![o(&format!("<< /Type /ExtGState {entry} >>"))],
        ..File::default()
    };
    reports(&state("/HTO [0 0]"), Level::A4, "6.2.5", "halftone origin (HTO)");
    // HTP is a PDF 1.x key PDF/A-4 says nothing of; PDF/A-2 still forbids it.
    passes(&state("/HTP [0 0]"), Level::A4);
    assert!(issues(&state("/HTP [0 0]"), Level::A2b).iter().any(|i| i.message.contains("(HTP)")));
    reports(&state("/BM /Foo"), Level::A4, "6.2.9", "which ISO 32000-2 doesn't define");
}

/// An sRGB profile stream object.
fn srgb_profile() -> Vec<u8> {
    stream("/N 3", &icc::srgb())
}

const PAGE_INTENT: &str = "/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (sRGB) /DestOutputProfile 6 0 R >>]";

#[test]
fn a_page_output_intent_stands_in_for_the_documents() {
    let red = |page: &str| File { content: "1 0 0 rg 0 0 10 10 re f", more: vec![srgb_profile()], ..with_page(page) };
    reports(&red(""), Level::A4, "6.2.4.3", "no PDF/A output intent");
    passes(&red(PAGE_INTENT), Level::A4);
    let group = "/Group << /S /Transparency >>";
    let transparent = |page: &str| File {
        resources: "/ExtGState << /G0 7 0 R >>",
        content: "/G0 gs",
        more: vec![srgb_profile(), o("<< /Type /ExtGState /ca 0.5 >>")],
        ..with_page(page)
    };
    reports(&transparent(group), Level::A4, "6.2.9", "transparency but no device-independent blending colour space");
    passes(&transparent(&format!("{group} {PAGE_INTENT}")), Level::A4);
}

#[test]
fn output_intents_share_one_local_output_or_monitor_profile() {
    let intent = |n: u8| format!("<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (sRGB) /DestOutputProfile {n} 0 R >>");
    let with = |catalog: String, more: Vec<Vec<u8>>| File { more, ..with_catalog(&catalog) };
    passes(&with(format!("/OutputIntents [{} {}]", intent(6), intent(6)), vec![srgb_profile()]), Level::A4);
    let two = with(format!("/OutputIntents [{} {}]", intent(6), intent(7)), vec![srgb_profile(), srgb_profile()]);
    reports(&two, Level::A4, "6.2.3", "different destination profiles");
    let external = "/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (x) /DestOutputProfileRef << >> >>]";
    reports(&with(external.to_owned(), Vec::new()), Level::A4, "6.2.3", "external profile");
    let mut input = icc::srgb();
    if let Some(class) = input.get_mut(12..16) {
        class.copy_from_slice(b"scnr");
    }
    reports(&with(format!("/OutputIntents [{}]", intent(6)), vec![stream("/N 3", &input)]), Level::A4, "6.2.3", "isn't an output or monitor profile");
}

#[test]
fn embedded_files_follow_the_level() {
    let text = || stream("/Type /EmbeddedFile /Subtype /text#2Fplain", b"a");
    let listed = "/Names << /EmbeddedFiles << /Names [(a.txt) 6 0 R] >> >> /AF [6 0 R]";
    let good = "/F (a.txt) /UF (a.txt) /AFRelationship /Data";
    let file = |conformance: Option<&str>, spec: &str, embedded: Vec<u8>, catalog: &str| File {
        packet: packet("4", Some("2020"), conformance),
        more: vec![o(&format!("<< /Type /Filespec {spec} /EF << /F 7 0 R >> >>")), embedded],
        ..with_catalog(catalog)
    };
    passes(&file(Some("F"), good, text(), listed), Level::A4f);
    passes(&file(Some("E"), good, text(), listed), Level::A4e);
    reports(&file(None, good, text(), listed), Level::A4, "6.9", "must be PDF/A-1, PDF/A-2 or PDF/A-4 files");
    reports(&file(Some("F"), "/F (a.txt) /UF (a.txt)", text(), listed), Level::A4f, "6.9", "no AFRelationship");
    reports(&file(Some("F"), "/F (a.txt) /AFRelationship /Data", text(), listed), Level::A4f, "6.9", "lacks F or UF");
    reports(&file(Some("F"), good, stream("/Type /EmbeddedFile", b"a"), listed), Level::A4f, "6.9", "no MIME type");
    reports(&file(Some("F"), good, stream("/Type /EmbeddedFile /Subtype /text", b"a"), listed), Level::A4f, "6.9", "no MIME type");
    reports(&file(Some("F"), good, text(), "/AF [6 0 R]"), Level::A4f, "6.9", "listed in the catalog's name dictionary");
    reports(&File { packet: packet("4", Some("2020"), Some("F")), ..File::default() }, Level::A4f, "6.9", "EmbeddedFiles");
    // An embedded file that declares PDF/A-2 is taken for one in PDF/A-4 itself; one that declares
    // nothing is not.
    let pdfa2 = bytes(&File { header: "%PDF-1.7", packet: packet("2", None, Some("B")), ..File::default() });
    passes(&file(None, good, stream("/Type /EmbeddedFile /Subtype /application#2Fpdf", &pdfa2), listed), Level::A4);
    let plain = bytes(&File { header: "%PDF-1.7", packet: described("", ""), ..File::default() });
    let undeclared = file(None, good, stream("/Type /EmbeddedFile /Subtype /application#2Fpdf", &plain), listed);
    reports(&undeclared, Level::A4, "6.9", "must be PDF/A-1, PDF/A-2 or PDF/A-4 files");
}

#[test]
fn the_catalog_holds_none_of_what_pdfa4_leaves_out() {
    reports(&with_catalog("/Perms << /DocMDP << >> /Foo << >> >>"), Level::A4, "6.1.11", "holds Foo: only UR3 and DocMDP");
    passes(&with_catalog("/Perms << /DocMDP << >> >>"), Level::A4);
    reports(&with_catalog("/AcroForm << /Fields [] /XFA [] >>"), Level::A4, "6.4.2", "XFA form");
    reports(&with_catalog("/NeedsRendering true"), Level::A4, "6.4.2", "NeedsRendering");
    reports(&with_catalog("/Names << /AlternatePresentations << >> >>"), Level::A4, "6.11", "alternate presentations");
    reports(&with_catalog("/Requirements []"), Level::A4, "6.12", "requirements");
    reports(&with_page("/PresSteps << >>"), Level::A4, "6.11", "presentation steps");
    let layers = |properties: &str| File {
        more: vec![o("<< /Type /OCG /Name (L) >>")],
        ..with_catalog(&format!("/OCProperties << /OCGs [6 0 R] {properties} >>"))
    };
    passes(&layers("/D << /Name (Default) >> /Configs [<< /Name (Other) >>]"), Level::A4);
    reports(&layers("/D << >>"), Level::A4, "6.10", "has no Name");
    reports(&layers("/D << /Name (A) >> /Configs [<< /Name (A) >>]"), Level::A4, "6.10", "same Name");
}

#[test]
fn the_metadata_stream_is_a_well_formed_utf8_packet() {
    // The identification is still read by its prefix, but the packet's elements aren't closed.
    let broken = o("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><pdfaid:part>4</pdfaid:part><pdfaid:rev>2020</pdfaid:rev>");
    reports(&File { packet: broken, ..File::default() }, Level::A4, "6.7.2.1", "isn't well-formed XML");
    let good = packet("4", Some("2020"), None);
    let sized = String::from_utf8(good.clone()).unwrap().replacen("?>", " bytes=\"10\"?>", 1);
    reports(&File { packet: sized.into_bytes(), ..File::default() }, Level::A4, "6.7.2.1", "bytes or encoding attribute");
    reports(&File { packet: [good.as_slice(), b"<!-- \xFF -->"].concat(), ..File::default() }, Level::A4, "6.7.2.1", "isn't UTF-8");
    let mut objects = vec![
        o("<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R >>"),
        o("<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
        stream("/Type /Metadata", &good),
        o("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R /Resources << >> >>"),
        stream("", b""),
    ];
    let found = verify(&open(raw("%PDF-2.0", &objects, "", b"")), Level::A4);
    assert!(found.iter().any(|i| i.clause == "6.7.2.1" && i.message.contains("/Subtype /XML")), "{found:#?}");
    if let Some(metadata) = objects.get_mut(2) {
        *metadata = stream("/Type /Metadata /Subtype /XML", &good);
    }
    assert!(verify(&open(raw("%PDF-2.0", &objects, "", b"")), Level::A4).is_empty());
}

#[test]
fn nothing_follows_the_last_end_of_file_marker() {
    reports(&File { after: b"garbage!", ..File::default() }, Level::A4, "6.1.3", "follows the last end-of-file marker");
    // One end-of-line after the marker is its own; a second is data, as veraPDF counts it.
    reports(&File { after: b"\n", ..File::default() }, Level::A4, "6.1.3", "follows the last end-of-file marker");
    passes(&File::default(), Level::A4);
}

#[test]
fn form_xobjects_and_3d_streams_are_checked() {
    let drawn = |dict: &str| File { resources: "/XObject << /X0 6 0 R >>", content: "/X0 Do", more: vec![stream(dict, b"")], ..File::default() };
    reports(&drawn("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Ref << /F (x.pdf) /Page 0 >>"), Level::A4, "6.2.8.2", "refers to another file");
    reports(&drawn("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /OPI << >>"), Level::A4, "6.2.8.1", "OPI information");
    reports(&drawn("/Type /3D /Subtype /Foo"), Level::A4, "6.1.6.1", "isn't U3D or PRC");
    passes(&drawn("/Type /3D /Subtype /PRC"), Level::A4);
    // PDF 2.0 doesn't define PostScript XObjects, and PDF/A-4 says nothing of them.
    passes(&drawn("/Type /XObject /Subtype /PS"), Level::A4);
}

#[test]
fn streams_use_the_standard_filters_and_only_the_identity_crypt_filter() {
    let image = |filter: &str| File {
        more: vec![stream(&format!("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /CalGray /BitsPerComponent 8 {filter}"), b"")],
        ..File::default()
    };
    reports(&image("/Filter /Flatedecode"), Level::A4, "6.1.6.2", "the filter Flatedecode, which ISO 32000-2 doesn't define");
    reports(&image("/Filter /LZWDecode"), Level::A4, "6.1.6.2", "LZW");
    reports(&image("/Filter [/Crypt] /DecodeParms [<< /Name /StdCF >>]"), Level::A4, "6.1.6.2", "other than Identity");
    passes(&image("/Filter [/Crypt] /DecodeParms [<< /Name /Identity >>]"), Level::A4);
    passes(&image("/Filter /Crypt"), Level::A4);
}

#[test]
fn rules_pdfa4_dropped_are_not_checked_for_it() {
    // Implementation limits (PDF/A-2 6.1.13): ISO 32000-2 sets none.
    let long_name = format!("/{} 1", "N".repeat(200));
    passes(&with_page(&long_name), Level::A4);
    let as_2b = File { header: "%PDF-1.7", packet: packet("2", None, Some("B")), ..with_page(&long_name) };
    assert!(issues(&as_2b, Level::A2b).iter().any(|i| i.clause == "6.1.13"));
    // XMP value types and extension schemas (PDF/A-2 6.6.2.3).
    passes(&File { packet: identified_with("<foo:bar>1</foo:bar>", "xmlns:foo=\"http://example.com/foo/\""), ..File::default() }, Level::A4);
    let mistyped = identified_with("<xmp:CreateDate>not a date</xmp:CreateDate>", "xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"");
    passes(&File { packet: mistyped, ..File::default() }, Level::A4);
}

#[test]
fn save_as_pdfa_changes_nothing_for_pdfa4() {
    let mut doc = open(bytes(&File { header: "%PDF-1.7", ..File::default() }));
    let report = convert(&mut doc, Level::A4).unwrap();
    assert!(report.fixed.is_empty(), "{report:#?}");
    assert!(report.remaining.iter().any(|i| i.clause == "6.1.2") && report.remaining.iter().all(|i| !i.fixable), "{report:#?}");
    assert!(!doc.full_save_required(), "nothing was changed");
}

mod never_panics {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

        /// Whatever the identification holds, or however broken the packet is, verification at
        /// every level answers, and a declared level is read back only as one of the five.
        #[test]
        fn malformed_identification_is_reported_not_a_crash(
            part in "\\PC{0,12}",
            rev in proptest::option::of("\\PC{0,12}"),
            conformance in proptest::option::of("\\PC{0,4}"),
            noise in proptest::collection::vec(any::<u8>(), 0..64),
            cut in 0usize..400,
        ) {
            let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;");
            let mut xmp = packet(&escape(&part), rev.as_deref().map(escape).as_deref(), conformance.as_deref().map(escape).as_deref());
            let mut broken = xmp.clone();
            broken.truncate(cut.min(broken.len()));
            broken.extend_from_slice(&noise);
            for packet in [std::mem::take(&mut xmp), broken] {
                let doc = open(bytes(&File { packet, ..File::default() }));
                for level in [Level::A2b, Level::A3b, Level::A4, Level::A4e, Level::A4f] {
                    let _ = verify(&doc, level);
                }
                if let Some((part, conformance)) = pdfcraft_preflight::declared(&doc).pdfa
                    && let Some(level) = Level::from_declared(part, &conformance)
                {
                    prop_assert_eq!(level.part(), part);
                }
            }
        }
    }
}
