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
    // A null version, direct or through a reference, is no version (ISO 32000-2 7.3.9).
    passes(&with_catalog("/Version null"), Level::A4);
    passes(&File { more: vec![o("null")], ..with_catalog("/Version 6 0 R") }, Level::A4);
}

#[test]
fn javascript_is_allowed_and_forbidden_actions_are_not() {
    passes(&with_catalog("/OpenAction << /S /JavaScript /JS (1;) >>"), Level::A4);
    passes(&with_catalog("/Names << /JavaScript << /Names [(a) << /S /JavaScript /JS (1;) >>] >> >>"), Level::A4);
    assert!(issues(&with_catalog("/OpenAction << /S /JavaScript /JS (1;) >>"), Level::A2b).iter().any(|i| i.message.contains("JavaScript")));
    reports(&with_catalog("/OpenAction << /S /Trans /Trans << >> >>"), Level::A4, "6.6.1", "A Trans action");
    reports(&with_catalog("/OpenAction << /S /GoTo /D [4 0 R /Fit] /Next << /S /Launch /F (x) >> >>"), Level::A4, "6.6.1", "Launch");
    // A document-level JavaScript action written in the name tree, at its root or in a kid, runs
    // its `/Next` chain too.
    let chained = "<< /S /JavaScript /JS (1;) /Next << /S /Launch /F (x) >> >>";
    reports(&with_catalog(&format!("/Names << /JavaScript << /Names [(a) {chained}] >> >>")), Level::A4, "6.6.1", "A Launch action");
    let kid = File {
        more: vec![o(&format!("<< /Limits [(a) (a)] /Names [(a) {chained}] >>")), o("[6 0 R 6 0 R]")],
        ..with_catalog("/Names << /JavaScript << /Kids 7 0 R >> >>")
    };
    reports(&kid, Level::A4, "6.6.1", "A Launch action");
    passes(&File { more: vec![o("<< /Names [(a) << /S /JavaScript /JS (1;) >>] >>"), o("[6 0 R]")], ..kid }, Level::A4);
    // A tree deeper than PdfCraft reads says so rather than passing what it didn't read.
    let deep = |levels: usize| File {
        more: (0..levels)
            .map(|i| o(&format!("<< /Kids [{} 0 R] >>", i + 7)))
            .chain([o(&format!("<< /Limits [(a) (a)] /Names [(a) {chained}] >>"))])
            .collect(),
        ..with_catalog("/Names << /JavaScript 6 0 R >>")
    };
    reports(&deep(32), Level::A4, "6.6.1", "A Launch action");
    reports(&deep(33), Level::A4, "6.6.1", "Some document-level JavaScript could not be read");
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
    // Any other annotation's normal appearance is an appearance stream.
    reports(&square("<< /N 0 >>"), Level::A4, "6.3.3", "Square annotation's normal appearance isn't an appearance stream");
    reports(&square("<< /N << /On 7 0 R >> >>"), Level::A4, "6.3.3", "normal appearance isn't an appearance stream");
    let widget = |entries: &str| File {
        page: "/Annots [6 0 R]".to_owned(),
        more: vec![o(&format!("<< /Type /Annot /Subtype /Widget /T (w) /Rect [0 0 10 10] /F 4 /P 4 0 R {entries} >>")), appearance.clone()],
        ..with_catalog("/AcroForm << /Fields [6 0 R] >>")
    };
    // A button's normal appearance is a dictionary of states.
    passes(&widget("/FT /Btn /AP << /N << /Off 7 0 R >> >>"), Level::A4);
    reports(&widget("/FT /Btn /AP << /N 7 0 R >>"), Level::A4, "6.3.3", "button's normal appearance");
    passes(&widget("/FT /Tx /AP << /N 7 0 R >>"), Level::A4);
    reports(&widget("/FT /Tx /AP << /N << /Off 7 0 R >> >>"), Level::A4, "6.3.3", "Widget annotation's normal appearance isn't an appearance stream");
    reports(&widget("/FT /Tx /AP << /N 7 0 R >> /A << /S /GoTo /D [4 0 R /Fit] >>"), Level::A4, "6.4.1", "widget annotation has an action");
    // A null action, or one referring to no object, is no entry (ISO 32000-2 7.3.9, 7.3.10).
    passes(&widget("/FT /Tx /AP << /N 7 0 R >> /A null"), Level::A4);
    passes(&widget("/FT /Tx /AP << /N 7 0 R >> /A 99 0 R"), Level::A4);
}

/// A null `/FT` is no entry (ISO 32000-2 7.3.9), so a widget whose `/FT` is null takes its parent's
/// field type: under a button its normal appearance is a dictionary of appearance states.
#[test]
fn a_null_field_type_is_inherited_from_the_parent() {
    let appearance = stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", b"");
    let button = File {
        page: "/Annots [6 0 R]".to_owned(),
        more: vec![
            o("<< /Type /Annot /Subtype /Widget /Parent 8 0 R /FT null /T (w) /Rect [0 0 10 10] /F 4 /P 4 0 R /AP << /N << /Off 7 0 R >> >> >>"),
            appearance,
            o("<< /FT /Btn /T (choice) /Kids [6 0 R] >>"),
        ],
        ..with_catalog("/AcroForm << /Fields [8 0 R] >>")
    };
    passes(&button, Level::A4);
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
    // A type 5 halftone's Gray, Red, Green and Blue components may have a transfer function (ISO
    // 32000-2 Table 128, pdf-association pdf-issues #314); Cyan, Magenta, Yellow and Black may not.
    let ht = |colorant: &str| {
        state(&format!(
            "/HT << /Type /Halftone /HalftoneType 5 /Default {screen} /{colorant} {screen} >>",
            screen = "<< /Type /Halftone /HalftoneType 1 /Frequency 90 /Angle 45 /SpotFunction /Round /TransferFunction /Identity >>"
        ))
    };
    for c in ["Red", "Green", "Blue", "Gray"] {
        passes(&ht(c), Level::A4);
    }
    reports(&ht("Cyan"), Level::A4, "6.2.5", "for Cyan, Magenta, Yellow or Black, has a transfer function");
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
    // A page's own intent takes the place of the document's for that page (ISO 32000-2 14.11.5).
    let intent =
        |n: u8| format!("/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (x) /DestOutputProfile {n} 0 R >>]");
    let both = |catalog: u8, page: u8| File {
        catalog: intent(catalog),
        content: "1 0 0 rg 0 0 10 10 re f",
        more: vec![srgb_profile(), profile_of(4, b"CMYK")],
        ..with_page(&intent(page))
    };
    reports(&both(6, 7), Level::A4, "6.2.4.3", "DeviceRGB is used, but the PDF/A output intent isn't an RGB profile");
    passes(&both(7, 6), Level::A4);
    passes(&both(6, 6), Level::A4);
}

/// 6.2.3 holds for a page's output intents as for the catalog's.
#[test]
fn a_pages_output_intents_follow_the_catalogs_rules() {
    let page = |intents: &str| File { more: vec![srgb_profile(), srgb_profile()], ..with_page(&format!("/OutputIntents [{intents}]")) };
    let intent = |n: u8| format!("<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (sRGB) /DestOutputProfile {n} 0 R >>");
    passes(&page(&intent(6)), Level::A4);
    let external = format!("{} << /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (x) /DestOutputProfileRef << >> >>", intent(6));
    reports(&page(&external), Level::A4, "6.2.3", "external profile");
    reports(&page(&format!("{} {}", intent(6), intent(7))), Level::A4, "6.2.3", "different destination profiles");
    let found = issues(&page(&external), Level::A4);
    assert!(found.iter().any(|i| i.message.contains("external profile") && i.page == Some(0)), "{found:#?}");
}

/// An ICC profile stream object of `n` components whose header names the data colour space
/// `space` (ICC.1 7.2.6): the sRGB profile with that one field changed.
fn profile_of(n: u8, space: &[u8; 4]) -> Vec<u8> {
    let mut data = icc::srgb();
    if let Some(field) = data.get_mut(16..20) {
        field.copy_from_slice(space);
    }
    stream(&format!("/N {n}"), &data)
}

/// PDF/A-4 6.2.4.3 (as veraPDF 1.30.4 prints it): DeviceRGB may be used where the current
/// transparency blending space is a device-independent RGB-based space, and DeviceCMYK where it is
/// a CMYK-based one. A page group's colour space (ISO 32000-2 11.4.7, Table 145 `/CS`) is that
/// space for the page (11.7.2). PDF/A-2 has no such allowance, nor does DeviceGray.
#[test]
fn a_page_groups_blending_space_covers_device_colour_of_as_many_components() {
    let drawn =
        |content: &'static str, cs: &str, more: Vec<Vec<u8>>| File { content, more, ..with_page(&format!("/Group << /S /Transparency {cs} >>")) };
    let rgb = "1 0 0 rg 0 0 10 10 re f";
    let cmyk = "0 0 0 1 k 0 0 10 10 re f";
    passes(&drawn(rgb, "/CS [/ICCBased 6 0 R]", vec![srgb_profile()]), Level::A4);
    passes(&drawn(rgb, "/CS [/CalRGB << /WhitePoint [0.9505 1 1.089] >>]", Vec::new()), Level::A4);
    passes(&drawn(cmyk, "/CS 7 0 R", vec![profile_of(4, b"CMYK"), o("[/ICCBased 6 0 R]")]), Level::A4);
    for (content, cs, more) in [
        // No blending space, a device one, Lab, and profiles that aren't RGB or CMYK.
        (rgb, "", Vec::new()),
        (rgb, "/CS /DeviceRGB", Vec::new()),
        (rgb, "/CS [/Lab << /WhitePoint [0.9505 1 1.089] >>]", Vec::new()),
        (rgb, "/CS [/ICCBased 6 0 R]", vec![profile_of(3, b"Lab ")]),
        (rgb, "/CS [/ICCBased 6 0 R]", vec![profile_of(4, b"CMYK")]),
        (cmyk, "/CS [/ICCBased 6 0 R]", vec![srgb_profile()]),
        ("0.5 g 0 0 10 10 re f", "/CS [/ICCBased 6 0 R]", vec![srgb_profile()]),
    ] {
        let f = drawn(content, cs, more);
        reports(&f, Level::A4, "6.2.4.3", "no PDF/A output intent");
    }
    // A group that isn't a transparency group sets no blending space.
    let other = File { content: rgb, more: vec![srgb_profile()], ..with_page("/Group << /S /Other /CS [/ICCBased 6 0 R] >>") };
    reports(&other, Level::A4, "6.2.4.3", "no PDF/A output intent");
    // Beside a CMYK output intent, an RGB blending space still covers DeviceRGB.
    let intent = "/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (CMYK) /DestOutputProfile 7 0 R >>]";
    let beside = File { catalog: intent.to_owned(), ..drawn(rgb, "/CS [/ICCBased 6 0 R]", vec![srgb_profile(), profile_of(4, b"CMYK")]) };
    passes(&beside, Level::A4);
    let without = File { catalog: intent.to_owned(), ..drawn(rgb, "", vec![srgb_profile(), profile_of(4, b"CMYK")]) };
    reports(&without, Level::A4, "6.2.4.3", "DeviceRGB is used, but the PDF/A output intent isn't an RGB profile");
    // Every page needs one: device colour is judged for the document as a whole.
    let two_pages = |second: &str| {
        let page = |group: &str| o(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R /Resources << >> {group} >>"));
        let objects = vec![
            o("<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R >>"),
            o("<< /Type /Pages /Kids [4 0 R 7 0 R] /Count 2 >>"),
            stream("/Type /Metadata /Subtype /XML", &packet("4", Some("2020"), None)),
            page("/Group << /S /Transparency /CS [/ICCBased 6 0 R] >>"),
            stream("", rgb.as_bytes()),
            srgb_profile(),
            page(second),
        ];
        verify(&Document::open(std::sync::Arc::new(raw("%PDF-2.0", &objects, "", b""))).unwrap(), Level::A4)
    };
    assert!(two_pages("/Group << /S /Transparency /CS [/ICCBased 6 0 R] >>").is_empty());
    let found = two_pages("");
    assert!(found.iter().any(|i| i.clause == "6.2.4.3" && i.message.contains("no PDF/A output intent")), "{found:#?}");
    // PDF/A-2 6.2.4.3 has no such allowance.
    let covered = drawn(rgb, "/CS [/ICCBased 6 0 R]", vec![srgb_profile()]);
    let found = verify(&open(bytes(&covered)), Level::A2b);
    assert!(found.iter().any(|i| i.message == "Device colour is used, but there is no PDF/A output intent"), "{found:#?}");
}

/// PDF/A-4 6.2.4.3 inside a form: an isolated transparency group XObject with a colour space of
/// its own blends what it paints in that space (ISO 32000-2 11.6.6, Table 145 `/CS`, Annex P), so
/// device colour of as many components drawn inside it is covered, whatever the output intent and
/// the page's group. The shape of veraPDF's 6-2-4-3-t04-pass-e and -i.
#[test]
fn a_form_groups_own_blending_space_covers_device_colour_drawn_inside_it() {
    let rgb = "1 0 0 rg 0 0 10 10 re f";
    let cmyk = "0 0 0 1 k 0 0 10 10 re f";
    let form = |dict: &str, content: &str| stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 10 10] {dict}"), content.as_bytes());
    // An output intent with profile 6 for `n` components, and a page group in that device space;
    // the page draws form 7 with `group`, `resources` and `inside`; group profile 8 has
    // `group_n` components; `more` from 9 on.
    let drawn = |n: u8, group_n: u8, group: &str, resources: &str, inside: &str, more: Vec<Vec<u8>>| {
        let profile = |n: u8| if n == 4 { profile_of(4, b"CMYK") } else { srgb_profile() };
        let (id, device) = if n == 4 { ("CMYK", "/DeviceCMYK") } else { ("sRGB", "/DeviceRGB") };
        let mut objects =
            vec![profile(n), form(&format!("/Group << /S /Transparency {group} >> /Resources << {resources} >>"), inside), profile(group_n)];
        objects.extend(more);
        File {
            catalog: format!("/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier ({id}) /DestOutputProfile 6 0 R >>]"),
            page: format!("/Group << /S /Transparency /CS {device} >>"),
            resources: "/XObject << /X0 7 0 R >>",
            content: "/X0 Do",
            more: objects,
            ..File::default()
        }
    };
    let not_rgb = "DeviceRGB is used, but the PDF/A output intent isn't an RGB profile";
    let not_cmyk = "DeviceCMYK is used, but the PDF/A output intent isn't a CMYK profile";
    let icc = "/I true /CS [/ICCBased 8 0 R]";
    // RGB inside an RGB group, beside a CMYK output intent (t04-pass-e), and the other way round
    // (t04-pass-i).
    passes(&drawn(4, 3, icc, "", rgb, Vec::new()), Level::A4);
    let cal = "/I true /CS [/CalRGB << /WhitePoint [0.9505 1 1.089] >>]";
    passes(&drawn(4, 3, cal, "/ColorSpace << /C /DeviceRGB >>", "/C cs 1 0 0 sc 0 0 10 10 re f", Vec::new()), Level::A4);
    passes(&drawn(3, 4, icc, "", cmyk, Vec::new()), Level::A4);
    // A page's RGB blending space doesn't reach into an isolated CMYK group: DeviceRGB drawn there
    // is blended in CMYK.
    let cal_page = "/Group << /S /Transparency /CS [/CalRGB << /WhitePoint [0.9505 1 1.089] >>] >>";
    reports(&File { page: cal_page.to_owned(), ..drawn(4, 4, icc, "", rgb, Vec::new()) }, Level::A4, "6.2.4.3", not_rgb);
    passes(&File { page: cal_page.to_owned(), content: "/X0 Do 1 0 0 rg 0 0 10 10 re f", ..drawn(4, 4, icc, "", cmyk, Vec::new()) }, Level::A4);
    // An isolated group whose own space is no device-independent one keeps the page's out too: a
    // DeviceCMYK group blends DeviceRGB in CMYK. A non-isolated one blends in the page's space.
    reports(&File { page: cal_page.to_owned(), ..drawn(4, 4, "/I true /CS /DeviceCMYK", "", rgb, Vec::new()) }, Level::A4, "6.2.4.3", not_rgb);
    passes(&File { page: cal_page.to_owned(), ..drawn(4, 4, "/CS /DeviceCMYK", "", rgb, Vec::new()) }, Level::A4);
    // A form inside the group, and an image only the group draws, are painted in its space too.
    let nested = form("/Resources << >>", rgb);
    passes(&drawn(4, 3, icc, "/XObject << /Y 9 0 R >>", "/Y Do", vec![nested.clone()]), Level::A4);
    let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceRGB", b"\xff\0\0");
    passes(&drawn(4, 3, icc, "/XObject << /Im 9 0 R >>", "/Im Do", vec![image]), Level::A4);
    for group in [
        // Not isolated, so the page's space; isolated without a space; a device space.
        "/CS [/ICCBased 8 0 R]",
        "/I true",
        "/I true /CS /DeviceRGB",
    ] {
        reports(&drawn(4, 3, group, "", rgb, Vec::new()), Level::A4, "6.2.4.3", not_rgb);
    }
    // An RGB space doesn't cover DeviceCMYK, nor a CMYK one DeviceRGB.
    reports(&drawn(3, 3, icc, "", cmyk, Vec::new()), Level::A4, "6.2.4.3", not_cmyk);
    reports(&drawn(4, 4, icc, "", rgb, Vec::new()), Level::A4, "6.2.4.3", not_rgb);
    // The group's space doesn't reach a form the page also draws outside it.
    let both = File {
        resources: "/XObject << /X0 7 0 R /Y 9 0 R >>",
        content: "/X0 Do /Y Do",
        ..drawn(4, 3, icc, "/XObject << /Y 9 0 R >>", "/Y Do", vec![nested.clone()])
    };
    reports(&both, Level::A4, "6.2.4.3", not_rgb);
    // Forms that draw each other are drawn inside the group all the same (veraPDF passes this too).
    let cycle = vec![form("/Resources << /XObject << /Z 10 0 R >> >>", rgb), form("/Resources << /XObject << /Y 9 0 R >> >>", "/Y Do")];
    passes(&drawn(4, 3, icc, "/XObject << /Y 9 0 R >>", "/Y Do", cycle), Level::A4);
    // PDF/A-2 6.2.4.3 has no such allowance.
    let found = verify(&open(bytes(&drawn(4, 3, icc, "", rgb, Vec::new()))), Level::A2b);
    assert!(found.iter().any(|i| i.message == not_rgb), "{found:#?}");
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
    // A profile cut short is no profile (ICC.1 7.2.2, 7.3): shorter than its header, shorter than
    // the size its header gives, or with a tag whose data runs past that size.
    let srgb = icc::srgb();
    let mut stretched = srgb.clone();
    if let Some(size) = stretched.get_mut(140..144) {
        size.copy_from_slice(&u32::try_from(srgb.len()).unwrap().to_be_bytes());
    }
    for cut in [srgb.get(..100).unwrap().to_vec(), srgb.get(..srgb.len() - 1).unwrap().to_vec(), stretched] {
        reports(
            &with(format!("/OutputIntents [{}]", intent(6)), vec![stream("/N 3", &cut)]),
            Level::A4,
            "6.2.3",
            "isn't an output or monitor profile",
        );
    }
    // A profile stream that can't be decoded is no such profile, even where nothing is drawn; one
    // that is no stream at all is taken as missing.
    let broken = stream("/N 3 /Filter /NoSuchDecode", b"xxxx");
    reports(&with(format!("/OutputIntents [{}]", intent(6)), vec![broken]), Level::A4, "6.2.3", "isn't an output or monitor profile");
    passes(&with(format!("/OutputIntents [{}]", intent(6)), vec![o("<< /N 3 >>")]), Level::A4);
}

/// PDF/A-4 6.2.4.3 where pages have output intents and blending spaces of their own: device colour
/// is judged against those of the page it is drawn on (ISO 32000-2 14.11.5 and 11.4.7), so pages
/// may differ.
/// 6.2.4.3: what a page draws is judged on that page however the file reaches it: through an
/// indirect `/Contents` array, resources the page inherits from the page tree (ISO 32000-2 7.7.3.4),
/// or an annotation the page lists that the form's `/Fields` lists too. The page tree node holding
/// the resources and the catalog's form don't draw anything themselves.
#[test]
fn device_colour_reaches_its_page_through_arrays_inherited_resources_and_fields() {
    let intent =
        |n: u8| format!("/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (x) /DestOutputProfile {n} 0 R >>]");
    // An RGB page (object 4, intent 8) and a CMYK one (object 6, intent 9); objects 10 on are `extra`.
    let two = |catalog: &str, tree: &str, first: &str, contents: &str, second: &str, extra: Vec<Vec<u8>>| {
        let mut objects = vec![
            o(&format!("<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R {catalog} >>")),
            o(&format!("<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 {tree} >>")),
            stream("/Type /Metadata /Subtype /XML", &packet("4", Some("2020"), None)),
            o(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents {contents} {first} {} >>", intent(8))),
            stream("", contents_rgb()),
            o(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 7 0 R {second} {} >>", intent(9))),
            stream("", b"1 0 0 0 k 0 0 10 10 re f /Im Do"),
            srgb_profile(),
            profile_of(4, b"CMYK"),
        ];
        objects.extend(extra);
        let found = verify(&Document::open(std::sync::Arc::new(raw("%PDF-2.0", &objects, "", b""))).unwrap(), Level::A4);
        found.into_iter().map(|i| i.message).collect::<Vec<_>>()
    };
    fn contents_rgb() -> &'static [u8] {
        b"1 0 0 rg 0 0 10 10 re f /Im Do"
    }
    let own = "/Resources << >>";
    let none: [&str; 0] = [];
    let image = || stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceRGB", b"\xff\0\0");
    // The first page's content in an indirect array.
    assert_eq!(two("", "", "/Resources << /XObject << /Im 11 0 R >> >>", "10 0 R", own, vec![o("[5 0 R]"), image()]), none);
    // An RGB image in resources the first page inherits and the second overrides.
    let inherited = "/Resources << /XObject << /Im 10 0 R >> >>";
    assert_eq!(two("", inherited, "", "5 0 R", own, vec![image()]), none);
    // An RGB widget appearance on the first page, listed by the form too.
    let widget = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (a) /F 4 /Rect [0 0 10 10] /P 4 0 R /AP << /N 11 0 R >> >>";
    let appearance = stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", b"0 1 0 rg 0 0 10 10 re f");
    assert_eq!(
        two(
            "/AcroForm << /Fields [10 0 R] >>",
            "",
            "/Annots [10 0 R] /Resources << /XObject << /Im 12 0 R >> >>",
            "5 0 R",
            own,
            vec![o(widget), appearance, image()]
        ),
        none
    );
    // The inherited image drawn on the CMYK page too is reported there.
    assert_eq!(two("", inherited, "", "5 0 R", "", vec![image()]), ["DeviceRGB is used, but the PDF/A output intent isn't an RGB profile"]);
}

#[test]
fn device_colour_is_judged_on_the_page_it_is_drawn_on() {
    let rgb = "1 0 0 rg 0 0 10 10 re f";
    let cmyk = "1 0 0 0 k 0 0 10 10 re f";
    let intent =
        |n: u8| format!("/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (x) /DestOutputProfile {n} 0 R >>]");
    let calrgb = "/Group << /S /Transparency /CS [/CalRGB << /WhitePoint [0.9505 1 1.089] >>] >>";
    let icc_cmyk = "/Group << /S /Transparency /CS [/ICCBased 9 0 R] >>";
    // Two pages, each with its own content: the sRGB profile is object 8, a CMYK one 9.
    let two = |first: &str, second: &str, contents: [&str; 2]| {
        let page = |entries: &str, content: u8| {
            o(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents {content} 0 R /Resources << >> {entries} >>"))
        };
        let objects = vec![
            o("<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R >>"),
            o("<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 >>"),
            stream("/Type /Metadata /Subtype /XML", &packet("4", Some("2020"), None)),
            page(first, 5),
            stream("", contents[0].as_bytes()),
            page(second, 7),
            stream("", contents[1].as_bytes()),
            srgb_profile(),
            profile_of(4, b"CMYK"),
        ];
        let found = verify(&Document::open(std::sync::Arc::new(raw("%PDF-2.0", &objects, "", b""))).unwrap(), Level::A4);
        let mut messages: Vec<String> = found.into_iter().map(|i| i.message).collect();
        messages.sort();
        messages
    };
    let none: [&str; 0] = [];
    // RGB on a page with an RGB intent, CMYK on one with a CMYK intent, and a page with neither
    // that draws nothing.
    assert_eq!(two(&intent(8), &intent(9), [rgb, cmyk]), none);
    assert_eq!(two(&intent(8), "", [rgb, ""]), none);
    assert_eq!(
        two(&intent(8), &intent(9), [cmyk, rgb]),
        [
            "DeviceCMYK is used, but the PDF/A output intent isn't a CMYK profile",
            "DeviceRGB is used, but the PDF/A output intent isn't an RGB profile"
        ]
    );
    assert_eq!(
        two(&intent(8), "", [rgb, cmyk]),
        ["Device colour is used, but there is no PDF/A output intent", "DeviceCMYK is used: it needs a CMYK output intent (not added automatically)"]
    );
    // Each page's blending space covers its own device colour.
    assert_eq!(two(calrgb, icc_cmyk, [rgb, cmyk]), none);
    assert_eq!(two(calrgb, "", [rgb, ""]), none);
    assert_eq!(
        two(calrgb, icc_cmyk, [cmyk, rgb]),
        ["Device colour is used, but there is no PDF/A output intent", "DeviceCMYK is used: it needs a CMYK output intent (not added automatically)"]
    );
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
    // The declaration isn't taken for conformance: a file that declares PDF/A-2b or PDF/A-4 is
    // verified at that level. PDF/A-1, 2a and 2u, which PdfCraft doesn't check, are taken as
    // declared.
    let pdf = |bytes: &[u8]| stream("/Type /EmbeddedFile /Subtype /application#2Fpdf", bytes);
    let red = "1 0 0 rg 0 0 10 10 re f";
    let failing2b = bytes(&File { header: "%PDF-1.7", packet: packet("2", None, Some("B")), content: red, ..File::default() });
    reports(&file(None, good, pdf(&failing2b), listed), Level::A4, "6.9", "declares PDF/A-2b but doesn't conform to it");
    let failing4 = bytes(&File { after: b"junk", ..File::default() });
    reports(&file(None, good, pdf(&failing4), listed), Level::A4, "6.9", "declares PDF/A-4 but doesn't conform to it");
    // A part and letter that name no level aren't a PDF/A declaration.
    for (part, conformance) in [("4", Some("B")), ("2", Some("Z")), ("1", Some("U"))] {
        let wrong = bytes(&File { header: "%PDF-1.7", packet: packet(part, None, conformance), ..File::default() });
        reports(&file(None, good, pdf(&wrong), listed), Level::A4, "6.9", "must be PDF/A-1, PDF/A-2 or PDF/A-4 files");
    }
    for (part, conformance) in [("1", Some("B")), ("2", Some("U"))] {
        let declared = bytes(&File { header: "%PDF-1.7", packet: packet(part, None, conformance), content: red, ..File::default() });
        passes(&file(None, good, pdf(&declared), listed), Level::A4);
    }
    // One embedded file deep: the files an embedded file embeds are judged by their declaration.
    let inner = bytes(&file(None, good, pdf(&failing4), listed));
    assert!(!verify(&open(inner.clone()), Level::A4).is_empty());
    passes(&file(None, good, pdf(&inner), listed), Level::A4);
}

/// 6.7.2.1 holds for every metadata stream, a page's as well as the catalog's.
#[test]
fn component_metadata_streams_are_well_formed_utf8_packets() {
    let good = packet("4", Some("2020"), None);
    let with = |data: &[u8]| File { more: vec![stream("/Type /Metadata /Subtype /XML", data)], ..with_page("/Metadata 6 0 R") };
    passes(&with(&good), Level::A4);
    reports(&with(&[good.as_slice(), b"<!-- \xFF -->"].concat()), Level::A4, "6.7.2.1", "metadata stream 6 0 R isn't UTF-8");
    reports(&with(b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">"), Level::A4, "6.7.2.1", "metadata stream 6 0 R isn't well-formed XML");
    let sized = String::from_utf8(good.clone()).unwrap().replacen("?>", " encoding=\"UTF-8\"?>", 1);
    reports(&with(sized.as_bytes()), Level::A4, "6.7.2.1", "in metadata stream 6 0 R has a bytes or encoding attribute");
    // Data that can't be decoded, or only in part, is reported; a packet past the size PdfCraft
    // reads isn't, as the catalog's isn't.
    let flated = |data: &[u8]| {
        use std::io::Write;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(data).unwrap();
        z.finish().unwrap()
    };
    let filtered = |filter: &str, data: &[u8]| File {
        more: vec![stream(&format!("/Type /Metadata /Subtype /XML /Filter {filter}"), data)],
        ..with_page("/Metadata 6 0 R")
    };
    passes(&filtered("/FlateDecode", &flated(&good)), Level::A4);
    let cut = flated(&good);
    let cut = cut.get(..cut.len() / 2).unwrap();
    reports(&filtered("/FlateDecode", cut), Level::A4, "6.7.2.1", "Metadata stream 6 0 R's data is damaged");
    reports(&filtered("/JBIG2Decode", b"x"), Level::A4, "6.7.2.1", "Metadata stream 6 0 R can't be decoded");
    // A packet past the 8 MiB PdfCraft parses isn't read, well-formed or not, and isn't reported:
    // the file is judged by what can be read.
    let huge = [good.as_slice(), vec![b' '; 9 << 20].as_slice()].concat();
    let found = issues(&filtered("/FlateDecode", &flated(&huge)), Level::A4);
    assert!(!found.iter().any(|i| i.message.contains("6 0 R")), "{found:#?}");
    let malformed = [b"<x:xmpmeta".as_slice(), vec![b' '; 9 << 20].as_slice()].concat();
    let found = issues(&filtered("/FlateDecode", &flated(&malformed)), Level::A4);
    assert!(!found.iter().any(|i| i.message.contains("6 0 R")), "{found:#?}");
    // The catalog's too.
    let mut objects = vec![
        o("<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R >>"),
        o("<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
        stream("/Type /Metadata /Subtype /XML /Filter /FlateDecode", cut),
        o("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R /Resources << >> >>"),
        stream("", b""),
    ];
    let found = verify(&open(raw("%PDF-2.0", &objects, "", b"")), Level::A4);
    assert!(found.iter().any(|i| i.message == "The metadata stream's data is damaged: only part of it decodes"), "{found:#?}");
    if let Some(m) = objects.get_mut(2) {
        *m = stream("/Type /Metadata /Subtype /XML /Filter /FlateDecode", &flated(&good));
    }
    assert!(verify(&open(raw("%PDF-2.0", &objects, "", b"")), Level::A4).is_empty());
}

/// PDF/A-4 6.6.3 allows the pointer and focus triggers, not every action they run (6.6.1): an
/// action written in an additional-actions dictionary, and the `/Next` actions written in it, are
/// checked as the others are.
#[test]
fn actions_in_additional_actions_are_checked() {
    let appearance = stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", b"");
    let square = |aa: &str| File {
        more: vec![o(&format!("<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /F 4 /AP << /N 7 0 R >> /AA {aa} >>")), appearance.clone()],
        ..with_page("/Annots [6 0 R]")
    };
    passes(&square("<< /E << /S /JavaScript /JS (1) >> >>"), Level::A4);
    reports(&square("<< /E << /S /Launch /F (x) >> >>"), Level::A4, "6.6.1", "Launch action isn't allowed");
    reports(&square("<< /E << /S /JavaScript /JS (1) /Next << /S /Launch /F (x) >> >> >>"), Level::A4, "6.6.1", "Launch action isn't allowed");
    reports(&square("<< /E << /S /JavaScript /JS (1) /Next [<< /S /Hide /T (x) >>] >> >>"), Level::A4, "6.6.1", "Hide action isn't allowed");
    // A widget's and a field's additional actions are not limited in their triggers, but in the
    // actions they run.
    let widget = File {
        page: "/Annots [6 0 R]".to_owned(),
        more: vec![
            o(
                "<< /Type /Annot /Subtype /Widget /FT /Tx /T (w) /Rect [0 0 10 10] /F 4 /P 4 0 R /AP << /N 7 0 R >> /AA << /K << /S /Launch /F (x) >> >> >>",
            ),
            appearance.clone(),
        ],
        ..with_catalog("/AcroForm << /Fields [6 0 R] >>")
    };
    reports(&widget, Level::A4, "6.6.1", "Launch action isn't allowed");
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
    // Configurations past the limit aren't read, and the file doesn't pass on what wasn't read.
    let named: String = (0..4096).map(|i| format!("<< /Name (C{i}) >>")).collect();
    passes(&layers(&format!("/D << /Name (D) >> /Configs [{named}]")), Level::A4);
    reports(&layers(&format!("/D << /Name (D) >> /Configs [{named} << >>]")), Level::A4, "6.10", "configurations weren't checked");
}

#[test]
fn the_metadata_stream_is_a_well_formed_utf8_packet() {
    // The identification is still read by its prefix, but the packet's elements aren't closed.
    let broken = o("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><pdfaid:part>4</pdfaid:part><pdfaid:rev>2020</pdfaid:rev>");
    reports(&File { packet: broken, ..File::default() }, Level::A4, "6.7.2.1", "isn't well-formed XML");
    let good = packet("4", Some("2020"), None);
    let sized = String::from_utf8(good.clone()).unwrap().replacen("?>", " bytes=\"10\"?>", 1);
    reports(&File { packet: sized.into_bytes(), ..File::default() }, Level::A4, "6.7.2.1", "bytes or encoding attribute");
    // However the attribute is spaced (XML 1.0 2.3), and only the attribute: `id` holds no `bytes`.
    let text = String::from_utf8(good.clone()).unwrap();
    for header in [" encoding = \"UTF-8\"?>", "\tbytes=\"10\"?>", "\nencoding\t=\"UTF-8\"?>"] {
        let spaced = text.replacen("?>", header, 1).into_bytes();
        reports(&File { packet: spaced, ..File::default() }, Level::A4, "6.7.2.1", "bytes or encoding attribute");
    }
    let named = text.replacen("W5M0MpCehiHzreSzNTczkc9d\"", "W5M0MpCehiHzreSzNTczkc9d\" xbytes=\"1\"", 1).into_bytes();
    passes(&File { packet: named, ..File::default() }, Level::A4);
    // The header is the processing instruction, not a comment's text that looks like one (XML 1.0
    // 2.5): a comment before the header neither adds the attribute nor hides it.
    let commented = |comment: &str, tail: &str| format!("<!-- {comment} -->{}", text.replacen("?>", tail, 1)).into_bytes();
    passes(&File { packet: commented("<?xpacket begin=\"\" bytes=\"10\"?>", "?>"), ..File::default() }, Level::A4);
    let hidden = commented("<?xpacket begin=\"\"?>", " bytes=\"10\"?>");
    reports(&File { packet: hidden, ..File::default() }, Level::A4, "6.7.2.1", "bytes or encoding attribute");
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
    // The marker is a line of its own: `%%EOF` at the end of appended data doesn't end the file.
    reports(&File { after: b"junk%%EOF\n", ..File::default() }, Level::A4, "6.1.3", "follows the last end-of-file marker");
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
    passes(&drawn("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /Subtype2 /PS"), Level::A4);
    passes(&drawn("/Type /XObject /Subtype /Form /BBox [0 0 1 1] /PS << >>"), Level::A4);
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
    // A null Name, direct or through a reference, is no Name: the Identity default (7.3.9).
    passes(&image("/Filter /Crypt /DecodeParms << /Name null >>"), Level::A4);
    for name in ["null", "/Identity"] {
        let mut by_reference = image("/Filter [/Crypt] /DecodeParms [<< /Name 7 0 R >>]");
        by_reference.more.push(o(name));
        passes(&by_reference, Level::A4);
    }
    let mut by_reference = image("/Filter [/Crypt] /DecodeParms [<< /Name 7 0 R >>]");
    by_reference.more.push(o("/StdCF"));
    reports(&by_reference, Level::A4, "6.1.6.2", "other than Identity");
    // No external file, nor its filters or their parameters on their own (ISO 32000-2 Table 5); a
    // null value is no entry (7.3.9).
    for entry in ["/F (data.bin)", "/FFilter /FlateDecode", "/FDecodeParms << >>"] {
        reports(&image(entry), Level::A4, "6.1.6.1", "has an external-file entry");
    }
    passes(&image("/F null /FFilter null /FDecodeParms null"), Level::A4);
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

/// Forms and patterns reached through an alias, or named `/P` in a resource name map of their own,
/// are drawn on the page that draws them, and are judged against that page's output intent.
#[test]
fn forms_reached_through_an_alias_or_named_p_are_judged_on_their_page() {
    let rgb = "1 0 0 rg 0 0 10 10 re f";
    let intent =
        |n: u8| format!("/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (x) /DestOutputProfile {n} 0 R >>]");
    // An RGB-intent page drawing `content` with `resources`, a CMYK-intent page drawing nothing, then
    // the sRGB profile (8), a CMYK one (9), and `more` from object 10.
    let two = |resources: &str, content: &str, more: Vec<Vec<u8>>| {
        let page = |entries: &str, resources: &str, content: u8| {
            o(&format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents {content} 0 R /Resources << {resources} >> {entries} >>"))
        };
        let mut objects = vec![
            o("<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R >>"),
            o("<< /Type /Pages /Kids [4 0 R 6 0 R] /Count 2 >>"),
            stream("/Type /Metadata /Subtype /XML", &packet("4", Some("2020"), None)),
            page(&intent(8), resources, 5),
            stream("", content.as_bytes()),
            page(&intent(9), "", 7),
            stream("", b""),
            srgb_profile(),
            profile_of(4, b"CMYK"),
        ];
        objects.extend(more);
        let found = verify(&Document::open(std::sync::Arc::new(raw("%PDF-2.0", &objects, "", b""))).unwrap(), Level::A4);
        found.into_iter().filter(|i| i.clause == "6.2.4.3").map(|i| i.message).collect::<Vec<String>>()
    };
    let form = |content: &str| stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", content.as_bytes());
    let none: [&str; 0] = [];
    // Object 10 is a reference to the form, 11.
    assert_eq!(two("/XObject << /X 10 0 R >>", "/X Do", vec![o("11 0 R"), form(rgb)]), none);
    // Object 10 is a resource dictionary of its own that names the form /P.
    assert_eq!(two("/XObject 10 0 R", "/P Do", vec![o("<< /P 11 0 R >>"), form(rgb)]), none);
    // A name map's /P that is an alias of the form (12 stands for 11).
    assert_eq!(two("/XObject 10 0 R", "/P Do", vec![o("<< /P 12 0 R >>"), form(rgb), o("11 0 R")]), none);
    // A name map that is itself reached through an alias (10 stands for 12, which names the form /P).
    assert_eq!(two("/XObject 10 0 R", "/P Do", vec![o("12 0 R"), form(rgb), o("<< /P 11 0 R >>")]), none);
    // A resource dictionary that is reached through an alias (form 10's /Resources is 12, which
    // stands for 13, whose /XObject map 14 names the form /P).
    let held = stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources 12 0 R", b"/P Do");
    let aliased = vec![held, form(rgb), o("13 0 R"), o("<< /XObject 14 0 R >>"), o("<< /P 11 0 R >>")];
    assert_eq!(two("/XObject << /X 10 0 R >>", "/X Do", aliased), none);
    // A name map's /P that is a dictionary: a shading pattern whose shading paints DeviceRGB.
    let shading = vec![
        o("<< /P 11 0 R >>"),
        o("<< /PatternType 2 /Shading 12 0 R >>"),
        o("<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 1 0] /Function << /FunctionType 2 /Domain [0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1 >> >>"),
    ];
    assert_eq!(two("/Pattern 10 0 R", "/Pattern cs /P scn 0 0 10 10 re f", shading), none);
    // The form still counts as drawn there: CMYK in it is judged against the RGB intent.
    assert_eq!(
        two("/XObject 10 0 R", "/P Do", vec![o("<< /P 11 0 R >>"), form("1 0 0 0 k 0 0 10 10 re f")]),
        ["DeviceCMYK is used, but the PDF/A output intent isn't a CMYK profile"]
    );
}

/// 6.10: configuration names are text strings (ISO 32000-2 8.11.4.2, Table 99), so two are the
/// same when they decode to the same text (7.9.2.2), however each is written.
#[test]
fn optional_content_names_are_compared_as_text() {
    let layers = |properties: &str| File {
        more: vec![o("<< /Type /OCG /Name (L) >>")],
        ..with_catalog(&format!("/OCProperties << /OCGs [6 0 R] {properties} >>"))
    };
    // (Layers) in PDFDocEncoding and in UTF-16BE with its byte order mark.
    reports(&layers("/D << /Name (Layers) >> /Configs [<< /Name <FEFF004C00610079006500720073> >>]"), Level::A4, "6.10", "same Name");
    passes(&layers("/D << /Name (Layers) >> /Configs [<< /Name <FEFF004C0061007900650072> >>]"), Level::A4);
}

/// 6.2.3: output intents past the number one verification reads aren't known to follow the rule,
/// so the file doesn't pass.
#[test]
fn output_intents_past_the_limit_are_reported_as_not_checked() {
    let plain = "<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (x) >>";
    let external = "<< /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (x) /DestOutputProfileRef << >> >>";
    let many: String = (0..4096).map(|_| plain).collect::<Vec<_>>().join(" ");
    passes(&with_catalog(&format!("/OutputIntents [{many}]")), Level::A4);
    reports(&with_catalog(&format!("/OutputIntents [{many} {external}]")), Level::A4, "6.2.3", "output intents weren't checked");
}

/// 6.7.2.1: the header is the processing instruction whose target is `xpacket` (XML 1.0 2.6), and
/// another instruction before it is skipped whole, whatever it holds.
#[test]
fn the_xpacket_header_is_found_by_its_target() {
    let text = String::from_utf8(packet("4", Some("2020"), None)).unwrap();
    let sized = text.replacen("?>", " bytes=\"10\"?>", 1);
    for before in ["<?xpacket-note ok?>", "<?tool <!-- ?>", "<?tool <?xpacket bytes=\"1\"?>"] {
        reports(&File { packet: format!("{before}{sized}").into_bytes(), ..File::default() }, Level::A4, "6.7.2.1", "bytes or encoding attribute");
    }
    passes(&File { packet: format!("<?tool <?xpacket bytes=\"1\"?>{text}").into_bytes(), ..File::default() }, Level::A4);
}

/// 6.7.2.1: many comments before the `xpacket` header are each read once. A search from every
/// comment on to the header would read some 10^11 bytes here, and take half a minute rather than a
/// tenth of a second; the bound leaves a hundredfold for slower machines.
#[test]
fn many_comments_before_the_xpacket_header_are_read_once() {
    let text = String::from_utf8(packet("4", Some("2020"), None)).unwrap();
    let sized = text.replacen("?>", " bytes=\"10\"?>", 1);
    let comments = "<!---->".repeat(300_000);
    let file = File { packet: format!("{comments}{sized}").into_bytes(), ..File::default() };
    let started = std::time::Instant::now();
    reports(&file, Level::A4, "6.7.2.1", "bytes or encoding attribute");
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "took {:?}", started.elapsed());
}

/// 6.2.3: an ICC profile's tag data follows its tag table (ICC.1 7.1, 7.3.4): a tag whose data
/// starts inside the header or the table is a profile cut short, not an output or monitor profile.
#[test]
fn an_output_intent_profile_tag_inside_the_header_is_no_profile() {
    let intent = "/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (sRGB) /DestOutputProfile 6 0 R >>]";
    let mut moved = icc::srgb();
    // The first tag's offset, after the 128-byte header and the 4-byte tag count.
    if let Some(offset) = moved.get_mut(136..140) {
        offset.copy_from_slice(&0u32.to_be_bytes());
    }
    reports(&File { more: vec![stream("/N 3", &moved)], ..with_catalog(intent) }, Level::A4, "6.2.3", "isn't an output or monitor profile");
    passes(&File { more: vec![srgb_profile()], ..with_catalog(intent) }, Level::A4);
}

/// 6.1.3 (ISO 32000-2 7.5.5): a file whose only `%%EOF` doesn't start a line has no end-of-file
/// marker.
#[test]
fn a_file_without_an_end_of_file_line_is_reported() {
    let good = bytes(&File::default());
    let at = good.windows(7).rposition(|w| w == b"\n%%EOF\n").unwrap();
    let mut spaced = good.get(..at).unwrap().to_vec();
    spaced.extend_from_slice(b"\n %%EOF\nmore");
    let found = verify(&open(spaced), Level::A4);
    assert!(found.iter().any(|i| i.clause == "6.1.3" && i.message.contains("no end-of-file marker")), "{found:#?}");
    passes(&File::default(), Level::A4);
}

/// 6.2.4.3: the PDF/A output intent's colour space is its profile's (the ICC header, ICC.1 7.2.6),
/// as 6.2.3 classes it, whatever the stream's `/N` says.
#[test]
fn the_output_intent_is_judged_by_its_profiles_header() {
    let intent = "/OutputIntents [<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier (x) /DestOutputProfile 6 0 R >>]";
    let drawn = |content: &'static str, profile: Vec<u8>| File { content, more: vec![profile], ..with_catalog(intent) };
    let cmyk = "0 0 0 1 k 0 0 10 10 re f";
    let rgb = "1 0 0 rg 0 0 10 10 re f";
    // An sRGB profile whose stream says /N 4 is still an RGB profile, on the page and in the catalog.
    reports(&drawn(cmyk, stream("/N 4", &icc::srgb())), Level::A4, "6.2.4.3", "isn't a CMYK profile");
    passes(&drawn(rgb, stream("/N 4", &icc::srgb())), Level::A4);
    let on_page = File { content: cmyk, more: vec![stream("/N 4", &icc::srgb())], ..with_page(intent) };
    reports(&on_page, Level::A4, "6.2.4.3", "isn't a CMYK profile");
    passes(&drawn(cmyk, profile_of(4, b"CMYK")), Level::A4);
}

/// A page's `/Contents` followed through more aliases than are followed (ISO 32000-2 7.3.10) is
/// content not read: the file doesn't pass on what wasn't read.
#[test]
fn page_content_past_the_alias_limit_is_reported_as_not_read() {
    let chained = |aliases: usize, array: bool| {
        let mut objects = vec![
            o("<< /Type /Catalog /Pages 2 0 R /Metadata 3 0 R >>"),
            o("<< /Type /Pages /Kids [4 0 R] /Count 1 >>"),
            stream("/Type /Metadata /Subtype /XML", &packet("4", Some("2020"), None)),
            o("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R /Resources << /Font << /F1 5 0 R >> >> >>"),
            o("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
        ];
        // Objects 6 on are each a reference to the next; the last is the content stream, which
        // paints DeviceRGB and shows text with font 5, which isn't embedded.
        objects.extend((0..aliases).map(|i| o(&format!("{} 0 R", i + 7))));
        if array {
            objects.push(o(&format!("[{} 0 R]", aliases + 7)));
        }
        objects.push(stream("", b"1 0 0 rg 0 0 10 10 re f BT /F1 12 Tf (x) Tj ET"));
        verify(&open(raw("%PDF-2.0", &objects, "", b"")), Level::A4)
    };
    let unembedded = |found: &[Issue]| found.iter().any(|i| i.message.contains("isn't embedded"));
    for array in [false, true] {
        let near = chained(4, array);
        assert!(near.iter().any(|i| i.clause == "6.2.4.3" && i.message.contains("no PDF/A output intent")), "{near:#?}");
        assert!(unembedded(&near), "{near:#?}");
        // Past the limit, whether a stream or an array of them lies beyond, the font counts as
        // drawing and the colour as not read.
        for aliases in [33, 40] {
            let far = chained(aliases, array);
            assert!(far.iter().any(|i| i.clause == "6.2.4.3" && i.message.contains("could not be read")), "{aliases} {array}: {far:#?}");
            assert!(unembedded(&far), "{aliases} {array}: {far:#?}");
        }
    }
}
