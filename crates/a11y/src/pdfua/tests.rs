//! PDF/UA verification on synthetic files: a file that meets every checked condition, and the same
//! file broken one way at a time.

use std::collections::BTreeSet;

use pdfcraft_cos::{Dict, Object};

use super::checks::{NO_LANGUAGE, security};
use super::testfont::{TrueType, cid_cff, hex_stream, type1};
use super::xmp::{dynamic_render_required, xmp_has_language, xmp_value};
use super::*;
use crate::tests::{pdf, stream};

/// The page's marked content: a heading, a figure, a decorative rule and a two-cell table.
const PAGE_OPS: &str = "/H1 << /MCID 0 >> BDC BT /F1 12 Tf 20 180 Td (Title) Tj ET EMC \
     /Figure << /MCID 1 >> BDC q 10 0 0 10 20 120 cm /Im Do Q EMC \
     /Artifact BMC 0 0 200 2 re f EMC \
     /TH << /MCID 2 >> BDC BT /F1 12 Tf 20 100 Td (A) Tj ET EMC \
     /TD << /MCID 3 >> BDC BT /F1 12 Tf 20 80 Td (1) Tj ET EMC";

const XMP_UA1: &str = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\
     <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:pdfuaid=\"http://www.aiim.org/pdfua/ns/id/\" pdfuaid:part=\"1\">\
     <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Quarterly report</rdf:li></rdf:Alt></dc:title>\
     </rdf:Description></rdf:RDF></x:xmpmeta>";

/// A one-page file that meets every condition PdfCraft checks: object n is `ua()[n - 1]`.
fn ua() -> Vec<String> {
    vec![
        // 1 catalog, 2 pages, 3 page, 4 content, 5 font
        "<< /Type /Catalog /Pages 2 0 R /Lang (en-US) /MarkInfo << /Marked true >> /StructTreeRoot 6 0 R \
         /ViewerPreferences << /DisplayDocTitle true >> /Metadata 9 0 R >>"
            .into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Tabs /S /Annots [10 0 R 11 0 R] /StructParents 0 \
         /Resources << /Font << /F1 5 0 R >> /XObject << /Im 12 0 R >> >> >>"
            .into(),
        stream(PAGE_OPS),
        format!(
            "<< /Type /Font /Subtype /TrueType /BaseFont /Demo /FirstChar 32 /LastChar 126 /Widths [{}] /Encoding /WinAnsiEncoding /FontDescriptor 13 0 R >>",
            ["500"; 95].join(" ")
        ),
        // 6 structure tree root, 7 Document element, 8 info, 9 metadata
        "<< /Type /StructTreeRoot /K 7 0 R >>".into(),
        "<< /S /Document /Pg 3 0 R /K [\
         << /S /H1 /Pg 3 0 R /K 0 >> \
         << /S /Figure /Pg 3 0 R /Alt (A red square) /K 1 >> \
         << /S /Table /Pg 3 0 R /K [ << /S /TR /K [ << /S /TH /A << /O /Table /Scope /Column >> /K 2 >> ] >> \
            << /S /TR /K [ << /S /TD /K 3 >> ] >> ] >> \
         << /S /Link /Pg 3 0 R /K << /Type /OBJR /Obj 10 0 R >> >> \
         << /S /Form /Pg 3 0 R /K << /Type /OBJR /Obj 11 0 R >> >> ] >>"
            .into(),
        "<< /Title (Quarterly report) >>".into(),
        format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{XMP_UA1}\nendstream", XMP_UA1.len()),
        // 10 link, 11 field, 12 image, 13 font descriptor, 14 font program
        "<< /Type /Annot /Subtype /Link /Rect [20 20 80 40] /Contents (Our website) /StructParent 1 >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /TU (Your name) /Rect [20 60 120 80] /StructParent 2 >>".into(),
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >>\nstream\n\u{0}\nendstream"
            .into(),
        "<< /Type /FontDescriptor /FontName /Demo /Flags 32 /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 \
         /CapHeight 700 /StemV 80 /FontFile2 14 0 R >>"
            .into(),
        // Printable ASCII, each glyph 500 units wide like its /Widths entry.
        hex_stream("", &TrueType::latin(' '..='~', 500).bytes()),
    ]
}

const CATALOG: usize = 0;
const PAGE: usize = 2;
const CONTENT: usize = 3;
const TREE_ROOT: usize = 5;
const DOCUMENT: usize = 6;
const METADATA: usize = 8;
const LINK: usize = 9;
const FIELD: usize = 10;
const PROGRAM: usize = 13;

fn verified(objs: &[String]) -> UaReport {
    verify(&pdf(objs, "/Info 8 0 R"), Part::Ua1)
}

fn failed(r: &UaReport) -> BTreeSet<&'static str> {
    r.with(Outcome::Failed).map(|x| x.condition.id).collect()
}

fn outcome(r: &UaReport, id: &str) -> Outcome {
    r.result(id).map_or(Outcome::NotChecked, |x| x.outcome)
}

fn messages(r: &UaReport, id: &str) -> Vec<String> {
    r.result(id).map(|x| x.findings.iter().map(|f| f.message.clone()).collect()).unwrap_or_default()
}

/// `objs` with the structure element `Document` holding `kids` (the content stays as it is).
fn with_kids(kids: &str) -> Vec<String> {
    let mut objs = ua();
    objs[DOCUMENT] = format!("<< /S /Document /Pg 3 0 R /K [{kids}] >>");
    objs
}

fn ids(list: &[&'static str]) -> BTreeSet<&'static str> {
    list.iter().copied().collect()
}

#[test]
fn a_conforming_file_fails_no_checked_condition_and_leaves_the_rest_to_a_person() {
    let r = verified(&ua());
    assert!(failed(&r).is_empty(), "{:#?}", r.with(Outcome::Failed).collect::<Vec<_>>());
    assert!(r.machine_checks_pass());
    assert_eq!(r.declared, Some(1));
    // Every Matterhorn condition, plus Tagged PDF; the PDF/UA-2 ones are not part of PDF/UA-1.
    assert_eq!(r.results.len(), 135);
    assert_eq!((r.count(Outcome::Passed), r.count(Outcome::Manual), r.count(Outcome::NotChecked)), (84, 47, 4));
    assert_eq!(r.result("13-005").map(|x| x.outcome), Some(Outcome::Manual), "only a person can judge it");
    assert_eq!(r.result("28-014").map(|x| x.outcome), Some(Outcome::NotChecked), "a machine check PdfCraft lacks is not passed");
    assert_eq!(r.result("31-016").map(|x| x.outcome), Some(Outcome::Passed), "the font program was looked at");
}

#[test]
fn metadata_identification_and_title() {
    let mut objs = ua();
    objs[CATALOG] = objs[CATALOG].replace("/Metadata 9 0 R", "");
    assert_eq!(failed(&verified(&objs)), ids(&["06-001", "06-002", "06-003"]));

    let packet = |description: &str| {
        let xmp = format!("<x:xmpmeta><rdf:RDF><rdf:Description {description}</rdf:Description></rdf:RDF></x:xmpmeta>");
        format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{xmp}\nendstream", xmp.len())
    };
    let mut objs = ua();
    objs[METADATA] = packet("rdf:about=\"\"><dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">Report</rdf:li></rdf:Alt></dc:title>");
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["06-002"]));
    assert_eq!(messages(&r, "06-002"), ["The metadata doesn't identify the file as PDF/UA (pdfuaid:part)"]);

    objs[METADATA] = packet("rdf:about=\"\" pdfuaid:part=\"2\"><dc:title><rdf:Alt><rdf:li>Report</rdf:li></rdf:Alt></dc:title>");
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["06-002"]));
    assert_eq!(messages(&r, "06-002"), ["The metadata identifies the file as PDF/UA-2, not PDF/UA-1"]);

    objs[METADATA] = packet("rdf:about=\"\"><pdfuaid:part>1</pdfuaid:part><dc:title><rdf:Alt><rdf:li> </rdf:li></rdf:Alt></dc:title>");
    assert_eq!(failed(&verified(&objs)), ids(&["06-003"]), "an element-form part counts, a blank title doesn't");
}

#[test]
fn the_window_must_show_the_title() {
    let mut objs = ua();
    objs[CATALOG] = objs[CATALOG].replace("/ViewerPreferences << /DisplayDocTitle true >>", "");
    assert_eq!(failed(&verified(&objs)), ids(&["07-001"]));
    objs[CATALOG] = ua()[CATALOG].replace("/DisplayDocTitle true", "/DisplayDocTitle false");
    assert_eq!(failed(&verified(&objs)), ids(&["07-002"]));
}

#[test]
fn tagged_pdf_and_suspects() {
    let mut objs = ua();
    objs[CATALOG] = objs[CATALOG].replace("/MarkInfo << /Marked true >>", "/MarkInfo << /Marked false /Suspects true >>");
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["tagged-pdf", "08-003"]));
    assert_eq!(messages(&r, "tagged-pdf"), ["The document isn't marked as tagged (MarkInfo Marked)"]);
    // Without a structure tree nothing is tagged: the content, the link and the field neither.
    objs[CATALOG] = ua()[CATALOG].replace("/StructTreeRoot 6 0 R", "");
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["tagged-pdf", "01-005", "28-010", "28-011"]));
    assert_eq!(messages(&r, "01-005"), ["The document isn't tagged, so none of its content is"]);
    // Unmarked content too: still one finding, the checker's.
    objs[CONTENT] = stream("BT /F1 12 Tf (stray) Tj ET");
    assert_eq!(messages(&verified(&objs), "01-005"), ["The document isn't tagged, so none of its content is"]);
}

#[test]
fn artifacts_and_tagged_content_must_not_nest() {
    let mut objs = ua();
    objs[CONTENT] = stream(
        "/P << /MCID 0 >> BDC BT /F1 12 Tf (a) Tj ET /Artifact BMC 0 0 5 5 re f EMC EMC \
         /Artifact << /Type /Pagination >> BDC /P << /MCID 1 >> BDC BT /F1 12 Tf (b) Tj ET EMC EMC \
         0 0 9 9 re f",
    );
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["01-003", "01-004", "01-005"]));
    assert_eq!(messages(&r, "01-003"), ["Page 1: 1 artifact inside tagged content"]);
    assert_eq!(messages(&r, "01-004"), ["Page 1: 1 tagged content item inside an artifact"]);
    assert_eq!(messages(&r, "01-005"), ["Page 1: 1 untagged content item"]);
}

#[test]
fn a_form_xobject_with_marked_content_is_drawn_once() {
    let mut objs = ua();
    objs[PAGE] = objs[PAGE].replace("/XObject << /Im 12 0 R >>", "/XObject << /Im 12 0 R /Fm 15 0 R >>");
    objs[CONTENT] = stream("/Fm Do /Fm Do");
    objs.push(stream("/P << /MCID 0 >> BDC 0 0 5 5 re f EMC").replace("<< /Length", "<< /Type /XObject /Subtype /Form /BBox [0 0 9 9] /Length"));
    let r = verified(&objs);
    assert!(failed(&r).contains("30-002"), "{:?}", failed(&r));
    assert_eq!(messages(&r, "30-002"), ["A form XObject with marked content (MCIDs) is drawn 2 times"]);
    assert_eq!(r.result("30-002").and_then(|x| x.findings[0].object).map(|o| o.num), Some(15));
    // Drawn once, it is fine; so is one without MCIDs drawn twice.
    objs[CONTENT] = stream("/Fm Do");
    assert!(!failed(&verified(&objs)).contains("30-002"));
    objs[CONTENT] = stream("/Fm Do /Fm Do");
    objs[14] = stream("/Artifact BMC 0 0 5 5 re f EMC").replace("<< /Length", "<< /Type /XObject /Subtype /Form /BBox [0 0 9 9] /Length");
    assert!(!failed(&verified(&objs)).contains("30-002"));
}

#[test]
fn a_reference_xobject_is_not_allowed() {
    let mut objs = ua();
    objs.push("<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Ref << /F (other.pdf) /Page 0 >> /Length 0 >>\nstream\n\nendstream".into());
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["30-001"]));
    assert_eq!(r.result("30-001").and_then(|x| x.findings[0].object).map(|o| o.num), Some(15));
}

#[test]
fn the_role_map_reaches_pdf_1_7_standard_types() {
    let mut objs = with_kids(
        "<< /S /Heading1 /Pg 3 0 R /K 0 >> << /S /Figure /Alt (x) /K 1 >> << /S /Custom /K 2 >> << /S /Title /K 3 >> \
         << /S /Loop >> << /S /Link /K << /Type /OBJR /Obj 10 0 R >> >> << /S /Form /K << /Type /OBJR /Obj 11 0 R >> >>",
    );
    objs[TREE_ROOT] = "<< /Type /StructTreeRoot /K 7 0 R /RoleMap << /Heading1 /H1 /Span /P /Loop /Again /Again /Loop >> >>".into();
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["02-001", "02-003", "02-004"]));
    // Title is standard in PDF 2.0 but not in PDF 1.7, so a PDF/UA-1 file must map it.
    assert_eq!(
        messages(&r, "02-001"),
        [
            "Structure type Custom doesn't map to a standard type",
            "Structure type Loop doesn't map to a standard type",
            "Structure type Title doesn't map to a standard type"
        ]
    );
    let mut circles = messages(&r, "02-003");
    circles.sort();
    assert_eq!(circles, ["Role map entry Again leads round in a circle", "Role map entry Loop leads round in a circle"]);
    assert_eq!(messages(&r, "02-004"), ["Standard structure type Span is remapped to P"]);
}

#[test]
fn only_the_entries_of_a_role_map_circle_are_named_circular() {
    // A and Z lead into the circle Loop → Again → Loop without being part of it: 02-003 names
    // Loop and Again, whichever order the map lists them in, and A and Z still fail 02-001.
    let kids = "<< /S /H1 /Pg 3 0 R /K 0 >> << /S /Figure /Alt (x) /K 1 >> << /S /A /K 2 >> << /S /P /K 3 >> \
                << /S /Z >> << /S /Link /K << /Type /OBJR /Obj 10 0 R >> >> << /S /Form /K << /Type /OBJR /Obj 11 0 R >> >>";
    for role_map in ["/A /Loop /Loop /Again /Again /Loop /Z /Again", "/Loop /Again /Again /Loop /Z /Again /A /Loop"] {
        let mut objs = with_kids(kids);
        objs[TREE_ROOT] = format!("<< /Type /StructTreeRoot /K 7 0 R /RoleMap << {role_map} >> >>");
        let r = verified(&objs);
        assert_eq!(failed(&r), ids(&["02-001", "02-003"]), "{role_map}");
        let mut circles = messages(&r, "02-003");
        circles.sort();
        assert_eq!(circles, ["Role map entry Again leads round in a circle", "Role map entry Loop leads round in a circle"], "{role_map}");
        assert_eq!(
            messages(&r, "02-001"),
            ["Structure type A doesn't map to a standard type", "Structure type Z doesn't map to a standard type"],
            "{role_map}"
        );
    }
}

#[test]
fn long_role_map_chains_are_followed_to_their_end_once() {
    let n = 20_000;
    let chain = |last: &str| (0..n).map(|i| format!("/T{i} /T{}", i + 1)).chain([format!("/T{n} /{last}")]).collect::<Vec<_>>().join(" ");
    let mut objs = with_kids(
        "<< /S /H1 /K 0 >> << /S /Figure /Alt (x) /K 1 >> << /S /T0 /K 2 >> << /S /P /K 3 >> \
         << /S /Link /K << /Type /OBJR /Obj 10 0 R >> >> << /S /Form /K << /Type /OBJR /Obj 11 0 R >> >>",
    );
    objs[TREE_ROOT] = format!("<< /Type /StructTreeRoot /K 7 0 R /RoleMap << {} >> >>", chain("P"));
    assert!(failed(&verified(&objs)).is_empty(), "T0 reaches P after {n} steps");
    // Closed into a circle, every entry leads round it, and T0 reaches no standard type.
    objs[TREE_ROOT] = format!("<< /Type /StructTreeRoot /K 7 0 R /RoleMap << {} >> >>", chain("T0"));
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["02-001", "02-003"]));
    assert_eq!(messages(&r, "02-001"), ["Structure type T0 doesn't map to a standard type"]);
    assert_eq!(messages(&r, "02-003").last().map(String::as_str), Some(format!("… and {} more", n + 1 - MAX_FINDINGS).as_str()));
}

#[test]
fn structure_elements_nest_as_iso_32000_says() {
    let objs = with_kids(
        "<< /S /H1 /K 0 >> << /S /Figure /Alt (x) /K 1 >> << /S /P /K 2 >> << /S /P /K 3 >> \
         << /S /Link /K << /Type /OBJR /Obj 10 0 R >> >> << /S /Form /K << /Type /OBJR /Obj 11 0 R >> >> \
         << /S /TBody >> << /S /TR >> << /S /TD >> << /S /LI >> << /S /LBody >> << /S /TOCI >> << /S /TOC /K [ << /S /TOCI >> << /S /P >> ] >> \
         << /S /RB >> << /S /Ruby /K [ << /S /RB >> << /S /RT >> ] >> << /S /WT >> << /S /Warichu /K << /S /WP >> >>",
    );
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["09-004", "09-005", "09-006", "09-007", "09-008"]));
    // The checker's findings come first, then PDF/UA's own.
    assert_eq!(
        messages(&r, "09-004"),
        [
            "TR element on page 1: not in a Table, THead, TBody or TFoot",
            "TD element on page 1: not in a TR",
            "TBody element on page 1: not in a Table"
        ]
    );
    assert_eq!(messages(&r, "09-005"), ["LI element on page 1: not in an L (list)", "LBody element on page 1: not in an LI (list item)"]);
    assert_eq!(messages(&r, "09-006"), ["TOCI element on page 1: not in a TOC", "P element on page 1: not allowed directly in a TOC"]);
    assert_eq!(messages(&r, "09-007"), ["RB element on page 1: not in a Ruby"]);
    assert_eq!(messages(&r, "09-008"), ["WT element on page 1: not in a Warichu"]);
}

#[test]
fn headings_start_at_h1_skip_nothing_and_are_of_one_kind() {
    let kids = "<< /S /Figure /Alt (x) /K 1 >> << /S /P /K 2 >> << /S /P /K 3 >> \
                << /S /Link /K << /Type /OBJR /Obj 10 0 R >> >> << /S /Form /K << /Type /OBJR /Obj 11 0 R >> >>";
    let r = verified(&with_kids(&format!("<< /S /H2 /K 0 >> << /S /H4 >> {kids}")));
    assert_eq!(failed(&r), ids(&["14-002", "14-003"]));
    assert_eq!(messages(&r, "14-002"), ["H2 element on page 1: the first heading is an H2; headings should not skip levels"]);
    assert_eq!(messages(&r, "14-003"), ["H4 element on page 1: H4 follows an H2; headings should not skip levels"]);
    let r = verified(&with_kids(&format!("<< /S /H1 /K 0 >> << /S /Sect /K [ << /S /H >> << /S /P >> << /S /H >> ] >> {kids}")));
    assert_eq!(failed(&r), ids(&["14-006", "14-007"]));
    assert_eq!(messages(&r, "14-006"), ["Sect element on page 1: more than one H heading in it"]);
}

#[test]
fn header_cells_need_a_scope_unless_the_table_uses_headers() {
    let table = |th: &str, td: &str| {
        with_kids(&format!(
            "<< /S /H1 /K 0 >> << /S /Figure /Alt (x) /K 1 >> << /S /Table /K [ << /S /TR /K [ << /S /TH {th} /K 2 >> ] >> \
             << /S /TR /K [ << /S /TD {td} /K 3 >> ] >> ] >> \
             << /S /Link /K << /Type /OBJR /Obj 10 0 R >> >> << /S /Form /K << /Type /OBJR /Obj 11 0 R >> >>"
        ))
    };
    let r = verified(&table("", ""));
    assert_eq!(failed(&r), ids(&["15-003"]));
    assert_eq!(messages(&r, "15-003"), ["TH element on page 1: a header cell with no Scope, in a table that doesn't use Headers"]);
    assert!(failed(&verified(&table("/A << /O /Table /ID (h1) >>", "/A << /O /Table /Headers [(h1)] >>"))).is_empty());
}

#[test]
fn figures_and_formulas_need_alternate_text_and_notes_an_id() {
    let r = verified(&with_kids(
        "<< /S /H1 /K 0 >> << /S /Figure /K 1 >> << /S /Formula /K 2 >> << /S /Note /K 3 >> << /S /Note /ID (n1) >> << /S /Note /ID (n1) >> \
         << /S /Link /K << /Type /OBJR /Obj 10 0 R >> >> << /S /Form /K << /Type /OBJR /Obj 11 0 R >> >>",
    ));
    assert_eq!(failed(&r), ids(&["13-004", "17-002", "19-003", "19-004"]));
    assert_eq!(messages(&r, "19-004"), ["Note element on page 1: its ID is another Note's too"]);
}

#[test]
fn every_text_needs_a_language() {
    // No language anywhere: one finding per condition, not one per tag.
    let mut objs = ua();
    objs[CATALOG] = objs[CATALOG].replace("/Lang (en-US) ", "/Outlines << /First 15 0 R /Last 15 0 R >> ");
    objs[PAGE] = objs[PAGE].replace("/Annots [10 0 R 11 0 R]", "/Annots [10 0 R 11 0 R 16 0 R]");
    objs[DOCUMENT] = objs[DOCUMENT].replace("/Obj 11 0 R >> >> ]", "/Obj 11 0 R >> >> << /S /Annot /Pg 3 0 R /K << /Type /OBJR /Obj 16 0 R >> >> ]");
    objs.push("<< /Title (Intro) >>".into());
    objs.push("<< /Type /Annot /Subtype /Text /Rect [0 0 9 9] /Contents (A note) >>".into());
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["11-001", "11-002", "11-003", "11-004", "11-005", "11-006"]));
    assert_eq!(messages(&r, "11-001"), [NO_LANGUAGE]);
    assert_eq!(messages(&r, "11-002"), [NO_LANGUAGE]);
    assert_eq!(
        messages(&r, "11-004"),
        ["Link annotation on page 1: its description has no language (Lang)", "Text annotation on page 1: its description has no language (Lang)"]
    );
    assert_eq!(messages(&r, "11-005"), ["Field \"name\" on page 1: its description has no language (Lang)"]);

    // A language on the Document element covers everything in it; the bookmarks and the
    // metadata still have none.
    objs[DOCUMENT] = objs[DOCUMENT].replacen("<< /S /Document", "<< /S /Document /Lang (en)", 1);
    assert_eq!(failed(&verified(&objs)), ids(&["11-003", "11-006"]));

    // With a language somewhere, the tags that have none are named one by one.
    objs[DOCUMENT] = ua()[DOCUMENT].replacen("<< /S /H1 /Pg 3 0 R /K 0 >>", "<< /S /H1 /Pg 3 0 R /Lang (en) /K 0 >>", 1);
    let r = verified(&objs);
    assert!(messages(&r, "11-001").contains(&"TH element on page 1: no language (Lang) on it, above it or on the document".to_string()));
    assert_eq!(messages(&r, "11-002"), ["Figure element on page 1: its alternate text, actual text or expansion has no language (Lang)"]);
    // Exactly the tags whose page content shows text: the Figure shows an image only, the Link
    // and Form tags hold only an annotation (OBJR), left to 11-004 and 11-005, and H1 has its own
    // language.
    let named: Vec<String> = messages(&r, "11-001").iter().map(|m| m.split(' ').next().unwrap_or_default().to_owned()).collect();
    assert_eq!(named, ["TH", "TD"]);
}

/// The tags that 11-001 names, by type, for `objs`.
fn without_language(objs: &[String]) -> Vec<String> {
    messages(&verified(objs), "11-001").iter().map(|m| m.split(' ').next().unwrap_or_default().to_owned()).collect()
}

#[test]
fn text_takes_its_language_from_marked_content_too() {
    // H1 sets a language, so the other tags are named one by one.
    let mut objs = ua();
    objs[CATALOG] = objs[CATALOG].replace("/Lang (en-US) ", "");
    objs[DOCUMENT] = objs[DOCUMENT].replacen("<< /S /H1 /Pg 3 0 R /K 0 >>", "<< /S /H1 /Pg 3 0 R /Lang (en) /K 0 >>", 1);
    let th = "/TH << /MCID 2 >> BDC BT /F1 12 Tf 20 100 Td (A) Tj ET EMC";
    let td = "/TD << /MCID 3 >> BDC BT /F1 12 Tf 20 80 Td (1) Tj ET EMC";
    let with_content = |th_ops: &str, td_ops: &str| {
        let mut objs = objs.clone();
        objs[CONTENT] = stream(&PAGE_OPS.replace(th, th_ops).replace(td, td_ops));
        objs
    };
    assert_eq!(without_language(&with_content(th, td)), ["TH", "TD"]);
    // A Span with a language around the text inside the TH's marked content (ISO 32000-1
    // §14.9.2.1), a language on the TD's own property list, and a Span around the TD's sequence.
    let th_spanned = "/TH << /MCID 2 >> BDC /Span << /Lang (en-US) >> BDC BT /F1 12 Tf 20 100 Td (A) Tj ET EMC EMC";
    assert_eq!(without_language(&with_content(th_spanned, td)), ["TD"]);
    assert_eq!(
        without_language(&with_content(th_spanned, "/TD << /MCID 3 /Lang (de) >> BDC BT /F1 12 Tf 20 80 Td (1) Tj ET EMC")),
        Vec::<String>::new()
    );
    let td_around = "/Span << /Lang (de) >> BDC /TD << /MCID 3 >> BDC BT /F1 12 Tf 20 80 Td (1) Tj ET EMC EMC";
    assert_eq!(without_language(&with_content(th_spanned, td_around)), Vec::<String>::new());
    // Covering part of the text is not enough.
    let half = "/TD << /MCID 3 >> BDC /Span << /Lang (de) >> BDC BT /F1 12 Tf 20 80 Td (1) Tj ET EMC BT 40 80 Td (2) Tj ET EMC";
    assert_eq!(without_language(&with_content(th_spanned, half)), ["TD"]);
    // A property list's language that isn't a language tag covers nothing.
    assert_eq!(without_language(&with_content(&th_spanned.replace("(en-US)", "(en_US)"), td_around)), ["TH"]);

    // No language on any tag, but on every text's marked content: nothing is missing.
    let mut bare = with_content(th_spanned, td_around);
    bare[DOCUMENT] = ua()[DOCUMENT].clone();
    bare[CONTENT] =
        stream(&PAGE_OPS.replace(th, th_spanned).replace(td, td_around).replace("/H1 << /MCID 0 >> BDC", "/H1 << /MCID 0 /Lang (en) >> BDC"));
    let r = verified(&bare);
    assert_eq!(outcome(&r, "11-001"), Outcome::Passed);
    // The figure's alternate text still has none.
    assert_eq!(messages(&r, "11-002"), [NO_LANGUAGE]);
}

#[test]
fn a_language_must_be_a_language_tag() {
    // The tag forms of the veraPDF corpus's 7.2-t29 files: catalog languages a verifier must
    // refuse, and ones it must accept.
    for bad in ["portugues-pt", "1-pt", "-pt", "nl-1234abcde", "пт-PT", "en--US", "en-", "en US", "en_US"] {
        assert!(!crate::content::is_language_tag(bad), "{bad}");
    }
    for good in ["portugue-pt", "p", "nd", "PT", "en-US", "zh-Hant-TW", "de-CH-1996", "x-klingon", "i-default", "sl-rozaj-biske"] {
        assert!(crate::content::is_language_tag(good), "{good}");
    }

    let mut objs = ua();
    objs[CATALOG] = objs[CATALOG].replace("/Lang (en-US)", "/Lang (1-pt)");
    let r = verified(&objs);
    assert_eq!(messages(&r, "11-001"), ["The document's language (Lang) 1-pt isn't a language tag"]);
    // A tag's language that isn't a language tag is named as such.
    objs[DOCUMENT] = objs[DOCUMENT].replacen("<< /S /H1 /Pg 3 0 R /K 0 >>", "<< /S /H1 /Pg 3 0 R /Lang (en) /K 0 >>", 1).replacen(
        "<< /S /TD /K 3 >>",
        "<< /S /TD /Lang (portugues-pt) /K 3 >>",
        1,
    );
    let r = verified(&objs);
    assert!(
        messages(&r, "11-001").contains(&"TD element on page 1: its language (Lang) isn't a language tag".to_string()),
        "{:?}",
        messages(&r, "11-001")
    );
}

#[test]
fn annotations_with_no_area_on_the_page_are_not_presented() {
    // On a 200 × 200 page: a note with a point for a rectangle (the veraPDF corpus's
    // [800 800 800 800]), a free text note beside the page, and a field of no width; none is
    // shown, so none needs a tag or a description. A note that is partly on the page is.
    let mut objs = ua();
    objs[PAGE] = objs[PAGE].replace("/Annots [10 0 R 11 0 R]", "/Annots [10 0 R 11 0 R 15 0 R 16 0 R 17 0 R 18 0 R]");
    objs.extend([
        "<< /Type /Annot /Subtype /Text /Rect [800 800 800 800] /F 4 >>".to_string(),
        "<< /Type /Annot /Subtype /FreeText /Rect [300 0 400 50] /F 4 /DA (/Helv 12 Tf 0 g) >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (hidden) /Rect [0 0 0 50] /F 4 >>".into(),
        "<< /Type /Annot /Subtype /Text /Rect [190 190 260 260] /F 4 >>".into(),
    ]);
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["28-002", "28-004"]));
    assert_eq!(messages(&r, "28-002"), ["Text annotation on page 1 isn't tagged"]);
    assert_eq!(r.result("28-002").and_then(|x| x.findings[0].object).map(|o| o.num), Some(18));
}

#[test]
fn a_file_attachment_names_its_file_in_f_and_uf() {
    let attached = |fs: &str| {
        let mut objs = ua();
        objs[PAGE] = objs[PAGE].replace("/Annots [10 0 R 11 0 R]", "/Annots [10 0 R 11 0 R 15 0 R]");
        objs[DOCUMENT] =
            objs[DOCUMENT].replace("/Obj 11 0 R >> >> ]", "/Obj 11 0 R >> >> << /S /Annot /Pg 3 0 R /K << /Type /OBJR /Obj 15 0 R >> >> ]");
        objs.push(format!(
            "<< /Type /Annot /Subtype /FileAttachment /Rect [0 0 9 9] /Contents (Data) /StructParent 3 /FS << /Type /Filespec {fs} >> >>"
        ));
        failed(&verified(&objs))
    };
    assert!(attached("/F (a.csv) /UF (a.csv)").is_empty());
    // An empty string names no file (the veraPDF corpus's 7.18.7-t01-fail-c and -d).
    assert_eq!(attached("/F () /UF (a.csv)"), ids(&["28-016"]));
    assert_eq!(attached("/F (a.csv) /UF ()"), ids(&["28-016"]));
}

#[test]
fn tables_and_lists_hold_only_the_kids_iso_32000_allows() {
    let base = "<< /S /H1 /K 0 >> << /S /Figure /Alt (x) /K 1 >> << /S /P /K 2 >> << /S /P /K 3 >> \
                << /S /Link /K << /Type /OBJR /Obj 10 0 R >> >> << /S /Form /K << /Type /OBJR /Obj 11 0 R >> >>";
    let td = "<< /S /TD >>";
    let tables = [
        // A Span in a row.
        format!("<< /S /Table /K [ << /S /TR /K [ {td} << /S /Span >> ] >> ] >>"),
        // A Span in a row group, and a second THead.
        format!(
            "<< /S /Table /K [ << /S /THead /K [ << /S /TR /K {td} >> << /S /Span >> ] >> << /S /THead >> << /S /TBody /K << /S /TR /K {td} >> >> ] >>"
        ),
        // Rows directly in the table, and in a TBody.
        format!("<< /S /Table /K [ << /S /TR /K {td} >> << /S /TBody /K << /S /TR /K {td} >> >> ] >>"),
        // A Caption between rows, and a P in the table.
        format!("<< /S /Table /K [ << /S /TR /K {td} >> << /S /Caption >> << /S /TR /K {td} >> << /S /P >> ] >>"),
        // Rows of two cells and one, with a Caption last (allowed).
        format!("<< /S /Table /K [ << /S /TR /K [ {td} {td} ] >> << /S /TR /K {td} >> << /S /Caption >> ] >>"),
    ];
    let lists = [
        // A Span in a list item, a P in the list, and a Caption last.
        "<< /S /L /K [ << /S /LI /K [ << /S /Lbl >> << /S /LBody >> << /S /Span >> ] >> << /S /P >> << /S /Caption >> ] >>",
        // A Caption first and a nested list (allowed).
        "<< /S /L /K [ << /S /Caption >> << /S /LI >> << /S /L /K << /S /LI >> >> ] >>",
        // Labels outside list items, in a contents entry and in a section (allowed: ISO 32000-2
        // Table 368 makes Lbl a general type).
        "<< /S /TOC /K << /S /TOCI /K << /S /P /K << /S /Reference /K << /S /Link /K << /S /Lbl >> >> >> >> >> >> << /S /Sect /K << /S /Lbl >> >>",
    ];
    let r = verified(&with_kids(&format!("{base} {} {}", tables.join(" "), lists.join(" "))));
    assert_eq!(failed(&r), ids(&["09-004", "09-005"]));
    let set = |id| messages(&r, id).into_iter().collect::<BTreeSet<String>>();
    let want = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<BTreeSet<String>>();
    assert_eq!(
        set("09-004"),
        want(&[
            "Span element on page 1: not allowed directly inside TR",
            "Span element on page 1: not allowed directly inside THead",
            "THead element on page 1: a second THead in its table",
            "Table element on page 1: rows both directly in it and in THead, TBody or TFoot",
            "Caption element on page 1: not the first or last child of its table",
            "P element on page 1: not allowed directly inside Table",
            "Table element on page 1: rows have between 1 and 2 columns",
        ])
    );
    assert_eq!(
        set("09-005"),
        want(&[
            "Span element on page 1: not allowed directly inside LI",
            "P element on page 1: not allowed directly inside L",
            "Caption element on page 1: not the first child of its list",
        ])
    );
}

#[test]
fn annotations_are_tagged_described_and_tab_in_structure_order() {
    let mut objs = ua();
    objs[PAGE] =
        objs[PAGE].replace("/Tabs /S ", "").replace("/Annots [10 0 R 11 0 R]", "/Annots [10 0 R 11 0 R 15 0 R 16 0 R 17 0 R 18 0 R 19 0 R 20 0 R]");
    objs[LINK] = "<< /Type /Annot /Subtype /Link /Rect [20 20 80 40] >>".into();
    objs[FIELD] = "<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /Rect [20 60 120 80] >>".into();
    objs[DOCUMENT] = objs[DOCUMENT]
        .replace("<< /S /Link /Pg 3 0 R", "<< /S /P /Pg 3 0 R")
        .replace("<< /S /Form /Pg 3 0 R /K << /Type /OBJR /Obj 11 0 R >> >>", "<< /S /P /K << /Type /OBJR /Obj 18 0 R >> >>");
    objs.extend([
        // 15 a note, 16 a trap network, 17 a printer's mark in the tree, 18 a tagged popup,
        // 19 a hidden note, 20 a file attachment whose file specification lacks UF
        "<< /Type /Annot /Subtype /Text /Rect [0 0 9 9] >>".to_string(),
        "<< /Type /Annot /Subtype /TrapNet /Rect [0 0 9 9] >>".into(),
        "<< /Type /Annot /Subtype /PrinterMark /Rect [0 0 9 9] /StructParent 5 >>".into(),
        "<< /Type /Annot /Subtype /Popup /Rect [0 0 9 9] >>".into(),
        "<< /Type /Annot /Subtype /Text /Rect [0 0 9 9] /F 2 >>".into(),
        "<< /Type /Annot /Subtype /FileAttachment /Rect [0 0 9 9] /Contents (Data) /FS << /Type /Filespec /F (a.csv) >> >>".into(),
    ]);
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["28-002", "28-004", "28-005", "28-007", "28-008", "28-010", "28-011", "28-012", "28-016", "28-017"]));
    assert_eq!(messages(&r, "28-011"), ["Link annotation on page 1 is in a P tag, not Link"]);
    assert_eq!(messages(&r, "28-010"), ["Form field on page 1 isn't tagged"]);
    assert_eq!(messages(&r, "28-002"), ["Text annotation on page 1 isn't tagged", "FileAttachment annotation on page 1 isn't tagged"]);
    assert_eq!(messages(&r, "28-004"), ["Text annotation on page 1 has no description (Contents), and its tag no alternate text"]);
    assert_eq!(messages(&r, "28-005"), ["Field \"name\" on page 1 has no description (TU), and its tag no alternate text"]);
    assert_eq!(r.result("28-012").map(|x| x.findings[0].bbox), Some(Some([20.0, 20.0, 80.0, 40.0])));
    assert_eq!(r.result("28-012").and_then(|x| x.findings[0].object).map(|o| o.num), Some(10));

    objs[PAGE] = ua()[PAGE].replace("/Tabs /S", "/Tabs /R");
    let r = verified(&objs);
    assert!(failed(&r).contains("28-009") && !failed(&r).contains("28-008"));
    assert_eq!(messages(&r, "28-009"), ["Page 1 has annotations and tabs in R order, not structure order (S)"]);

    // A field tagged, but not as a Form, and a described note tagged, but not as an Annot.
    let mut objs = ua();
    objs[PAGE] = objs[PAGE].replace("/Annots [10 0 R 11 0 R]", "/Annots [10 0 R 11 0 R 15 0 R]");
    objs[DOCUMENT] = objs[DOCUMENT]
        .replace("<< /S /Form /Pg 3 0 R", "<< /S /P /Pg 3 0 R")
        .replace("/Obj 11 0 R >> >> ]", "/Obj 11 0 R >> >> << /S /P /Pg 3 0 R /K << /Type /OBJR /Obj 15 0 R >> >> ]");
    objs.push("<< /Type /Annot /Subtype /Text /Rect [0 0 9 9] /Contents (A note) >>".into());
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["28-002", "28-010"]));
    assert_eq!(messages(&r, "28-010"), ["Widget annotation on page 1 is in a P tag, not Form"]);
    assert_eq!(messages(&r, "28-002"), ["Text annotation on page 1 is in a P tag, not Annot"]);
}

#[test]
fn fonts_that_draw_text_are_embedded_and_map_to_unicode() {
    let mut objs = ua();
    objs[PAGE] = objs[PAGE].replace("/F1 5 0 R", "/F1 5 0 R /F2 15 0 R /F3 16 0 R");
    objs[CONTENT] = stream(&format!("{PAGE_OPS} /Artifact BMC BT /F2 9 Tf (x) Tj ET BT /F3 9 Tf 3 Tr <0001> Tj ET EMC"));
    objs.extend([
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_string(),
        "<< /Type /Font /Subtype /Type0 /BaseFont /Hidden /Encoding /Identity-H /DescendantFonts [17 0 R] >>".into(),
        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Hidden /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> >>".into(),
    ]);
    let r = verified(&objs);
    // Helvetica draws text and isn't embedded. The Type 0 font only draws invisible text (3 Tr), so
    // it needn't be embedded, but its codes still have to map to Unicode; it also names no
    // CIDToGIDMap (31-005) and has no ToUnicode map though Identity needs one (31-027).
    assert_eq!(failed(&r), ids(&["10-001", "31-005", "31-009", "31-027"]));
    assert_eq!(messages(&r, "31-009"), ["Font Helvetica on page 1 isn't embedded"]);
    assert!(messages(&r, "10-001")[0].contains("Hidden"));
}

#[test]
fn optional_content_embedded_files_and_xfa() {
    let mut objs = ua();
    objs[CATALOG] = objs[CATALOG].replace(
        "/Metadata 9 0 R",
        "/Metadata 9 0 R /OCProperties << /OCGs [] /D << /AS [] >> /Configs [ << /Name (Print) >> << >> ] >> \
         /Names << /EmbeddedFiles 15 0 R >> /AcroForm << /Fields [] /XFA [(config) 17 0 R] >>",
    );
    objs.extend([
        "<< /Kids [16 0 R 15 0 R] >>".to_string(),
        "<< /Names [(data.csv) << /Type /Filespec /F (data.csv) /EF << >> >> (ok.csv) << /Type /Filespec /F (ok.csv) /UF (ok.csv) >>] >>".into(),
        stream("<config><present><pdf><dynamicRender>\n required </dynamicRender></pdf></present></config>"),
    ]);
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["20-001", "20-002", "20-003", "21-001", "25-001"]));
    assert_eq!(messages(&r, "20-001"), ["Optional content configuration 2 has no Name"]);
    assert_eq!(messages(&r, "21-001"), ["Embedded file \"data.csv\": its file specification lacks F or UF"]);
    // An AS entry in one of the other configurations is found too.
    let mut objs = ua();
    objs[CATALOG] = objs[CATALOG]
        .replace("/Metadata 9 0 R", "/Metadata 9 0 R /OCProperties << /OCGs [] /D << /Name (View) >> /Configs [ << /Name (Print) /AS [] >> ] >>");
    assert_eq!(failed(&verified(&objs)), ids(&["20-003"]));
}

#[test]
fn encryption_must_permit_assistive_technology() {
    let doc = pdf(&ua(), "/Info 8 0 R");
    let check = |p: Option<i64>| {
        let mut encrypt = Dict::new();
        encrypt.set("Filter", Object::name("Standard"));
        if let Some(p) = p {
            encrypt.set("P", Object::Int(p));
        }
        let mut c = Collected::default();
        security(&doc, &encrypt, &mut c);
        ["26-001", "26-002"].into_iter().filter(|id| c.failed(id)).collect::<Vec<_>>()
    };
    assert_eq!(check(None), ["26-001"]);
    assert_eq!(check(Some(-3904)), ["26-002"], "bit 10 clear");
    assert!(check(Some(-3904 | 1 << 9)).is_empty());
    assert!(check(Some(-4)).is_empty(), "everything allowed");
}

#[test]
fn pdf_ua_2_checks_its_identification_and_version_and_what_it_keeps() {
    // A PDF/UA-1 file is not a PDF/UA-2 one.
    let r = verify(&pdf(&ua(), "/Info 8 0 R"), Part::Ua2);
    assert_eq!(r.part, Part::Ua2);
    assert_eq!(failed(&r), ids(&["ua2-identification", "ua2-pdf-version"]));
    assert_eq!(
        messages(&r, "ua2-identification"),
        ["The metadata identifies the file as PDF/UA-1, not PDF/UA-2", "The metadata has no PDF/UA revision year (pdfuaid:rev)"]
    );
    assert_eq!(messages(&r, "ua2-pdf-version"), ["The file is PDF 1.7, not PDF 2.0"]);
    assert!(r.result("06-002").is_none(), "PDF/UA-2 has its own identification");
    assert_eq!(outcome(&r, "09-004"), Outcome::NotChecked, "PDF 2.0 changed the nesting rules");
    assert_eq!(outcome(&r, "13-004"), Outcome::Passed);

    let mut objs = ua();
    let xmp = XMP_UA1.replace("pdfuaid:part=\"1\"", "pdfuaid:part=\"2\" pdfuaid:rev=\"2024\"");
    objs[METADATA] = format!("<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n{xmp}\nendstream", xmp.len());
    objs[CATALOG] = objs[CATALOG].replace("/Type /Catalog", "/Type /Catalog /Version /2.0");
    let doc = pdf(&objs, "/Info 8 0 R");
    let r = verify(&doc, Part::Ua2);
    assert!(failed(&r).is_empty(), "{:?}", failed(&r));
    assert_eq!(r.declared, Some(2));
    assert_eq!(Part::for_declared(r.declared), Part::Ua2);
    // A failure PDF/UA-2 keeps is still found.
    objs[DOCUMENT] = objs[DOCUMENT].replace(" /Alt (A red square)", "");
    assert_eq!(failed(&verify(&pdf(&objs, "/Info 8 0 R"), Part::Ua2)), ids(&["13-004"]));
}

#[test]
fn long_lists_of_findings_are_cut_and_counted() {
    let cells = vec!["<< /S /TH >>"; 450].join(" ");
    let objs = with_kids(&format!(
        "<< /S /H1 /K 0 >> << /S /Figure /Alt (x) /K 1 >> << /S /P /K 2 >> << /S /P /K 3 >> \
         << /S /Link /K << /Type /OBJR /Obj 10 0 R >> >> << /S /Form /K << /Type /OBJR /Obj 11 0 R >> >> \
         << /S /Table /K [ << /S /TR /K [{cells}] >> ] >>"
    ));
    let r = verified(&objs);
    let found = &r.result("15-003").map(|x| x.findings.clone()).unwrap_or_default();
    assert_eq!(found.len(), MAX_FINDINGS + 1);
    assert_eq!(found[MAX_FINDINGS].message, "… and 250 more");
}

#[test]
fn malformed_structures_end_without_hanging() {
    // A name tree whose kid is itself, a role map that maps a type to itself, and an XFA entry
    // that is no stream.
    let mut objs = ua();
    objs[CATALOG] = objs[CATALOG].replace("/Metadata 9 0 R", "/Metadata 9 0 R /Names << /EmbeddedFiles 15 0 R >> /AcroForm << /XFA 42 >>");
    objs[TREE_ROOT] = "<< /Type /StructTreeRoot /K 7 0 R /RoleMap << /Self /Self >> >>".into();
    objs.push("<< /Kids [15 0 R 15 0 R] /Names [(a)] >>".into());
    let r = verified(&objs);
    assert_eq!(failed(&r), ids(&["02-003"]));
}

#[test]
fn xmp_properties_are_read_in_either_form() {
    let x = "<rdf:Description pdfuaid:part=\"1\" xpdfuaid:rev='9' dc:x='a &amp; b'><dc:title xml:lang=\"en\"><rdf:Alt><rdf:li xml:lang=\"x-default\">Árbol &lt;1&gt;</rdf:li></rdf:Alt></dc:title><pdfuaid:rev>2024</pdfuaid:rev><dc:titles>no</dc:titles>";
    assert_eq!(xmp_value(x, "pdfuaid:part").as_deref(), Some("1"));
    assert_eq!(xmp_value(x, "dc:x").as_deref(), Some("a & b"));
    assert_eq!(xmp_value(x, "pdfuaid:rev").as_deref(), Some("2024"), "the attribute of a longer name doesn't count");
    assert_eq!(xmp_value(x, "dc:title").as_deref(), Some("Árbol <1>"));
    assert_eq!(xmp_value(x, "dc:description"), None);
    // Unterminated values and elements are not read past the end.
    for broken in ["pdfuaid:part=\"1", "<dc:title>", "<dc:title><rdf:li>é", " pdfuaid:part='", "<dc:title"] {
        let _ = xmp_value(broken, "pdfuaid:part");
        let _ = xmp_value(broken, "dc:title");
        assert!(!xmp_has_language(broken));
    }
    assert!(xmp_has_language(x));
    assert!(!xmp_has_language("<rdf:li xml:lang=\"x-default\">a</rdf:li><x xml:lang=>"));
}

#[test]
fn dynamic_render_is_read_from_its_element_only() {
    assert!(dynamic_render_required("<dynamicRender>required</dynamicRender>"));
    assert!(dynamic_render_required("<dynamicRender xmlns=\"x\"> required\n</dynamicRender>"));
    assert!(!dynamic_render_required("<dynamicRender>forbidden</dynamicRender>"));
    assert!(!dynamic_render_required("<dynamicRenderX>required</dynamicRenderX>"));
    assert!(!dynamic_render_required("<dynamicRender"));
    assert!(!dynamic_render_required("<dynamicRender/>required"), "a closed element has no value");
}

#[test]
fn the_condition_table_is_the_matterhorn_protocol() {
    let matterhorn: Vec<&Condition> = CONDITIONS.iter().filter(|c| c.matterhorn).collect();
    assert_eq!(matterhorn.len(), 134, "136 conditions less 23-001 and 27-001, which ask for no test");
    assert_eq!(matterhorn.iter().filter(|c| c.how == How::Machine).count(), 87);
    assert_eq!(matterhorn.iter().filter(|c| c.how == How::Human).count(), 47);
    let unique: BTreeSet<&str> = CONDITIONS.iter().map(|c| c.id).collect();
    assert_eq!(unique.len(), CONDITIONS.len(), "ids are unique");
    for c in CONDITIONS {
        assert!(c.how == How::Machine || !c.checked, "{} is a person's judgement", c.id);
        assert!(c.section == "UA2" || c.section.starts_with("UA1:"), "{}", c.id);
        if c.matterhorn {
            let digits: Vec<bool> = c.id.chars().map(|ch| ch.is_ascii_digit()).collect();
            assert_eq!(digits, [true, true, false, true, true, true], "{}", c.id);
        }
    }
    assert_eq!(Condition::by_id("28-008").map(Condition::clause).as_deref(), Some("ISO 14289-1 7.18.3"));
    assert_eq!(Condition::by_id("06-002").map(Condition::clause).as_deref(), Some("ISO 14289-1 5"));
    assert_eq!(Condition::by_id("ua2-pdf-version").map(Condition::clause).as_deref(), Some("ISO 14289-2"));
}

/// Every condition marked as checked is one the code reports, and every id the code reports is a
/// condition marked as checked: the table and the checks can't drift apart.
#[test]
fn the_checks_and_the_table_agree() {
    let code = |s: &'static str| s.get(..s.find("#[cfg(test)]").unwrap_or(s.len())).unwrap_or(s);
    let source = format!("{}{}", code(include_str!("checks.rs")), code(include_str!("fonts.rs")));
    let source = source.as_str();
    // An id is a quoted `NN-NNN`, or one of our own three.
    let mut reported: BTreeSet<&str> = source
        .as_bytes()
        .windows(8)
        .enumerate()
        .filter(|(_, w)| w[0] == b'"' && w[7] == b'"' && w[3] == b'-' && [1, 2, 4, 5, 6].iter().all(|i| w[*i].is_ascii_digit()))
        .filter_map(|(at, _)| source.get(at + 1..at + 7))
        .collect();
    for own in ["tagged-pdf", "ua2-identification", "ua2-pdf-version"] {
        if source.contains(&format!("\"{own}\"")) {
            reported.insert(own);
        }
    }
    let checked: BTreeSet<&str> = CONDITIONS.iter().filter(|c| c.checked).map(|c| c.id).collect();
    assert_eq!(reported, checked);
}

// ---- Fonts (Matterhorn checkpoint 31) and formulas (17-003) ----

/// `ua()` with one more font, `/F9` (object 15), drawn as an artifact in text render mode `mode`
/// showing `shown` (a PDF string), and `extra` objects from 16 on.
fn with_font(font: &str, shown: &str, mode: u8, extra: &[String]) -> Vec<String> {
    let mut objs = ua();
    objs[PAGE] = objs[PAGE].replace("/F1 5 0 R", "/F1 5 0 R /F9 15 0 R");
    objs[CONTENT] = stream(&format!("{PAGE_OPS} /Artifact BMC BT /F9 9 Tf {mode} Tr {shown} Tj ET EMC"));
    objs.push(font.to_owned());
    objs.extend_from_slice(extra);
    objs
}

/// The failed conditions about fonts and Unicode.
fn font_failures(r: &UaReport) -> BTreeSet<&'static str> {
    failed(r).into_iter().filter(|id| id.starts_with("31-") || id.starts_with("10-") || id.starts_with("17-")).collect()
}

/// The font conditions PdfCraft checks: a font it can read leaves none of them not checked.
const FONT_CONDITIONS: [&str; 30] = [
    "31-001", "31-002", "31-003", "31-004", "31-005", "31-006", "31-007", "31-008", "31-009", "31-011", "31-012", "31-013", "31-014", "31-015",
    "31-016", "31-017", "31-018", "31-019", "31-020", "31-021", "31-022", "31-023", "31-024", "31-025", "31-026", "31-027", "31-028", "31-029",
    "31-030", "17-003",
];

const DESCRIPTOR: &str =
    "/Type /FontDescriptor /FontName /Sample /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80";

/// A simple TrueType font (objects 15 to 17) over `program`, showing `AB`.
fn truetype_font(font: &str, flags: u32, program: &TrueType) -> Vec<String> {
    with_font(font, "(AB)", 0, &[format!("<< {DESCRIPTOR} /Flags {flags} /FontFile2 17 0 R >>"), hex_stream("", &program.bytes())])
}

const TRUETYPE: &str = "<< /Type /Font /Subtype /TrueType /BaseFont /Sample /FirstChar 65 /LastChar 66 /Widths [600 600] /Encoding /WinAnsiEncoding /FontDescriptor 16 0 R >>";

fn ab() -> TrueType {
    TrueType::latin(['A', 'B'], 600)
}

/// A Type 1 font (objects 15 to 17) whose program has A (600) and B (700), showing `shown`.
fn type1_font(font: &str, descriptor: &str, shown: &str) -> Vec<String> {
    let program = type1(&[("A", 600), ("B", 700)], "StandardEncoding");
    with_font(font, shown, 0, &[format!("<< {DESCRIPTOR} /Flags 32 {descriptor} /FontFile 17 0 R >>"), hex_stream("", &program)])
}

const TYPE1: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Sample /FirstChar 65 /LastChar 66 /Widths [600 700] /FontDescriptor 16 0 R >>";

/// A ToUnicode map with `entries` for two-byte codes.
fn to_unicode(entries: &str) -> String {
    stream(&format!(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap 1 begincodespacerange <0000> <FFFF> endcodespacerange \
         {entries} endcmap CMapName currentdict /CMap defineresource pop end end"
    ))
}

/// A Type 0 font over a TrueType CIDFont (objects 15 to 19): the CIDFont is `cidfont`, the
/// program `program` (A and B are CIDs and glyphs 1 and 2), and the ToUnicode map gives them
/// their letters.
fn cid_truetype_font(font: &str, cidfont: &str, shown: &str, mode: u8, program: &TrueType) -> Vec<String> {
    with_font(
        font,
        shown,
        mode,
        &[
            cidfont.to_owned(),
            format!("<< {DESCRIPTOR} /Flags 32 /FontFile2 18 0 R >>"),
            hex_stream("", &program.bytes()),
            to_unicode("2 beginbfchar <0001> <0041> <0002> <0042> endbfchar"),
        ],
    )
}

const TYPE0: &str = "<< /Type /Font /Subtype /Type0 /BaseFont /Sample /Encoding /Identity-H /DescendantFonts [16 0 R] /ToUnicode 19 0 R >>";
const CIDFONT2: &str = "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Sample /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
     /CIDToGIDMap /Identity /FontDescriptor 17 0 R /W [1 [600 600]] >>";

/// A Type 0 font over a CID-keyed CFF CIDFont with CIDs 5 (700) and 9 (300), objects 15 to 19.
fn cid_cff_font(cidfont: &str, descriptor: &str, shown: &str) -> Vec<String> {
    with_font(
        TYPE0,
        shown,
        0,
        &[
            cidfont.to_owned(),
            format!("<< {DESCRIPTOR} /Flags 4 {descriptor} /FontFile3 18 0 R >>"),
            hex_stream("/Subtype /CIDFontType0C", &cid_cff(&[(5, 700), (9, 300)])),
            to_unicode("2 beginbfchar <0005> <0041> <0009> <0042> endbfchar"),
        ],
    )
}

const CIDFONT0: &str = "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /Sample /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
     /FontDescriptor 17 0 R /W [5 [700] 9 [300]] >>";

#[test]
fn fonts_of_each_kind_that_conform_pass_every_font_condition() {
    let type3 = with_font(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 16 0 R >> \
         /Encoding << /Type /Encoding /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [400] /Resources << >> >>",
        "(a)",
        0,
        &[stream("400 0 d0 0 0 100 100 re f")],
    );
    let fonts = [
        ("TrueType", truetype_font(TRUETYPE, 32, &ab())),
        ("Type 1", type1_font(TYPE1, "/CharSet (/A/B)", "(AB)")),
        ("Type 0, TrueType", cid_truetype_font(TYPE0, CIDFONT2, "<00010002>", 0, &ab())),
        ("Type 0, CFF", cid_cff_font(CIDFONT0, "", "<00050009>")),
        ("Type 3", type3),
    ];
    for (kind, objs) in fonts {
        let r = verified(&objs);
        assert!(failed(&r).is_empty(), "{kind}: {:#?}", r.with(Outcome::Failed).collect::<Vec<_>>());
        for id in FONT_CONDITIONS {
            assert_eq!(outcome(&r, id), Outcome::Passed, "{kind}: {id}");
        }
    }
}

#[test]
fn type0_cmaps_and_character_collections_agree() {
    let cidfont = |info: &str| CIDFONT2.replace("/Registry (Adobe) /Ordering (Identity) /Supplement 0", info);
    let font = |encoding: &str| TYPE0.replace("/Encoding /Identity-H", encoding);
    // A predefined CMap of another collection. Its Supplement isn't read, so 31-003 stays open.
    let objs = cid_truetype_font(&font("/Encoding /UniGB-UCS2-H"), &cidfont("/Registry (Foo) /Ordering (Japan1) /Supplement 9"), "<0001>", 3, &ab());
    let r = verified(&objs);
    assert!(font_failures(&r).is_superset(&ids(&["31-001", "31-002"])), "{:?}", font_failures(&r));
    assert_eq!(messages(&r, "31-002"), ["Font Sample on page 1: its CMap and CIDFont name different orderings (CIDSystemInfo Ordering)"]);
    assert_eq!(outcome(&r, "31-003"), Outcome::NotChecked);
    let objs = cid_truetype_font(&font("/Encoding /UniGB-UCS2-H"), &cidfont("/Registry (Adobe) /Ordering (GB1) /Supplement 2"), "<0001>", 3, &ab());
    assert!(font_failures(&verified(&objs)).is_disjoint(&ids(&["31-001", "31-002", "31-006"])));
    // Neither predefined nor embedded, or missing.
    let r = verified(&cid_truetype_font(&font("/Encoding /Bogus-H"), CIDFONT2, "<0001>", 3, &ab()));
    assert_eq!(messages(&r, "31-006"), ["Font Sample on page 1: its CMap Bogus-H is neither predefined nor embedded"]);
    let r = verified(&cid_truetype_font(&font(""), CIDFONT2, "<0001>", 3, &ab()));
    assert_eq!(messages(&r, "31-006"), ["Font Sample on page 1 has no CMap (Encoding)"]);
    // An embedded CMap: its own collection and Supplement, writing mode, and the CMaps it uses.
    let cmap = |entries: &str, program: &str| {
        stream(&format!(
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap {program} 1 begincodespacerange <0000> <FFFF> endcodespacerange \
             1 begincidrange <0000> <FFFF> 0 endcidrange endcmap end end"
        ))
        .replace(
            "<< /Length",
            &format!("<< /Type /CMap /CMapName /Test /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 4 >> {entries} /Length"),
        )
    };
    let embedded = |entries: &str, program: &str, supplement: u8| {
        let info = format!("/Registry (Adobe) /Ordering (Japan1) /Supplement {supplement}");
        let mut objs = cid_truetype_font(&font("/Encoding 20 0 R"), &cidfont(&info), "<0001>", 3, &ab());
        objs.push(cmap(entries, program));
        verified(&objs)
    };
    let r = embedded("/WMode 1", "/Custom usecmap", 6);
    assert!(font_failures(&r).is_superset(&ids(&["31-003", "31-007", "31-008"])), "{:?}", font_failures(&r));
    assert_eq!(messages(&r, "31-003"), ["Font Sample on page 1: its CIDFont's Supplement is higher than its CMap's"]);
    // The CIDFont's Supplement may be below its CMap's (4), not above it: the veraPDF corpus's
    // 7.21.3.1-t01-pass-d has a CIDFont at 1 under a CMap at 2, and -fail-c one at 3 under 2.
    assert!(!font_failures(&embedded("/WMode 1", "/WMode 1 def /UniJIS-UTF16-H usecmap", 2)).contains("31-003"));
    assert_eq!(messages(&r, "31-008"), ["Font Sample on page 1: its CMap uses CMap Custom, which isn't predefined"]);
    let fine = embedded("/WMode 1", "/WMode 1 def /UniJIS-UTF16-H usecmap", 4);
    assert!(font_failures(&fine).is_disjoint(&ids(&["31-001", "31-002", "31-003", "31-007", "31-008"])), "{:?}", font_failures(&fine));
    assert!(font_failures(&embedded("/UseCMap /Other-H", "", 4)).contains("31-008"));
    assert!(!font_failures(&embedded("/UseCMap /90ms-RKSJ-H", "", 4)).contains("31-008"));
    assert_eq!(
        messages(&embedded("/UseCMap 19 0 R", "", 4), "31-008"),
        ["Font Sample on page 1: its CMap uses an embedded CMap, not a predefined one"]
    );
}

#[test]
fn truetype_cid_fonts_name_their_cid_to_gid_map() {
    let r = verified(&cid_truetype_font(TYPE0, &CIDFONT2.replace("/CIDToGIDMap /Identity", ""), "<00010002>", 0, &ab()));
    assert_eq!(font_failures(&r), ids(&["31-005"]));
    let r = verified(&cid_truetype_font(TYPE0, &CIDFONT2.replace("/CIDToGIDMap /Identity", "/CIDToGIDMap 7"), "<00010002>", 0, &ab()));
    assert_eq!(font_failures(&r), ids(&["31-004"]));
    // A map stream sends CIDs 1 and 2 to their glyphs; one that sends CID 2 to glyph 0 draws .notdef.
    let mapped = |map: &[u8]| {
        let mut objs = cid_truetype_font(TYPE0, &CIDFONT2.replace("/CIDToGIDMap /Identity", "/CIDToGIDMap 20 0 R"), "<00010002>", 0, &ab());
        objs.push(hex_stream("", map));
        verified(&objs)
    };
    assert!(font_failures(&mapped(&[0, 0, 0, 1, 0, 2])).is_empty());
    let r = mapped(&[0, 0, 0, 1, 0, 0]);
    assert_eq!(font_failures(&r), ids(&["31-030"]));
    assert_eq!(messages(&r, "31-030"), ["Font Sample on page 1 draws the .notdef glyph (code <0002>)"]);
}

#[test]
fn cid_sets_list_exactly_the_cids_the_program_has() {
    // Glyphs: .notdef, A and B with outlines, and an empty one a subsetter left (CID 3).
    let program = TrueType { glyphs: vec![(500, true), (600, true), (600, true), (600, false)], cmaps: vec![((3, 1), vec![(65, 1), (66, 2)])] };
    let with_set = |bits: u8| {
        let mut objs = cid_truetype_font(TYPE0, CIDFONT2, "<00010002>", 0, &program);
        objs[16] = objs[16].replace("/FontFile2 18 0 R", "/FontFile2 18 0 R /CIDSet 20 0 R");
        objs.push(hex_stream("", &[bits]));
        verified(&objs)
    };
    let r = with_set(0b1100_0000);
    assert_eq!(font_failures(&r), ids(&["31-014"]));
    assert_eq!(messages(&r, "31-014"), ["Font Sample on page 1: its CIDSet leaves out CID 2, which the font program has"]);
    let r = with_set(0b1110_0100);
    assert_eq!(font_failures(&r), ids(&["31-015"]));
    assert_eq!(messages(&r, "31-015"), ["Font Sample on page 1: its CIDSet lists CID 5, which the font program lacks"]);
    assert!(font_failures(&with_set(0b1110_0000)).is_empty(), "the empty glyph needn't be listed");
    assert!(font_failures(&with_set(0b1111_0000)).is_empty(), "but may be");
    // A CID-keyed CFF program has CIDs 5 and 9.
    let cff = |bits: [u8; 2]| {
        let mut objs = cid_cff_font(CIDFONT0, "/CIDSet 20 0 R", "<00050009>");
        objs.push(hex_stream("", &bits));
        font_failures(&verified(&objs))
    };
    assert_eq!(cff([0b1000_0100, 0b0100_0000]), ids(&[]));
    assert_eq!(cff([0b1000_0100, 0]), ids(&["31-014"]));
    assert_eq!(cff([0b1000_0100, 0b0100_1000]), ids(&["31-015"]));
}

#[test]
fn type1_charsets_list_exactly_the_glyphs_the_program_has() {
    let r = verified(&type1_font(TYPE1, "/CharSet (/A)", "(AB)"));
    assert_eq!(font_failures(&r), ids(&["31-012"]));
    assert_eq!(messages(&r, "31-012"), ["Font Sample on page 1: its CharSet leaves out glyph B, which the font program has"]);
    let r = verified(&type1_font(TYPE1, "/CharSet (/A/B /C)", "(AB)"));
    assert_eq!(font_failures(&r), ids(&["31-013"]));
    assert_eq!(messages(&r, "31-013"), ["Font Sample on page 1: its CharSet lists glyph C, which the font program lacks"]);
    assert!(font_failures(&verified(&type1_font(TYPE1, "/CharSet (/.notdef/A/B)", "(AB)"))).is_empty());
}

#[test]
fn embedded_fonts_have_every_glyph_the_text_draws() {
    let font = TYPE1.replace("/LastChar 66 /Widths [600 700]", "/LastChar 67 /Widths [600 700 600] /Encoding << /Differences [67 /C] >>");
    let r = verified(&type1_font(&font, "", "(ABC)"));
    assert_eq!(font_failures(&r), ids(&["31-011"]));
    assert_eq!(messages(&r, "31-011"), ["Font Sample on page 1 draws a glyph its font program lacks (code <43>)"]);
    // Invisible text (3 Tr) is not rendered, so its glyphs needn't be there.
    let mut objs = type1_font(&font, "", "(ABC)");
    objs[CONTENT] = objs[CONTENT].replace("0 Tr", "3 Tr");
    assert!(font_failures(&verified(&objs)).is_empty());
    // A CID the CFF program lacks, and a TrueType cmap pointing past the last glyph.
    assert_eq!(font_failures(&verified(&cid_cff_font(CIDFONT0, "", "<00050006>"))), ids(&["31-011"]));
    let past = TrueType { glyphs: vec![(600, true), (600, true), (600, true)], cmaps: vec![((3, 1), vec![(65, 1), (66, 7)])] };
    assert_eq!(font_failures(&verified(&truetype_font(TRUETYPE, 32, &past))), ids(&["31-011"]));
}

#[test]
fn glyph_widths_agree_with_the_program() {
    let r = verified(&truetype_font(&TRUETYPE.replace("[600 600]", "[600 650]"), 32, &ab()));
    assert_eq!(font_failures(&r), ids(&["31-016"]));
    assert_eq!(messages(&r, "31-016"), ["Font Sample on page 1: code <42> is 650 wide in the font dictionary but 600 in the font program"]);
    assert!(font_failures(&verified(&truetype_font(&TRUETYPE.replace("[600 600]", "[600.5 599.5]"), 32, &ab()))).is_empty(), "within 1/1000");
    assert_eq!(font_failures(&verified(&type1_font(&TYPE1.replace("[600 700]", "[600 600]"), "", "(AB)"))), ids(&["31-016"]));
    assert_eq!(font_failures(&verified(&cid_cff_font(&CIDFONT0.replace("9 [300]", "9 [333]"), "", "<00050009>"))), ids(&["31-016"]));
    // A CID left out of /W takes /DW, 1000 by default, which the program doesn't say.
    let short = CIDFONT2.replace("/W [1 [600 600]]", "/W [1 [600]]");
    assert_eq!(font_failures(&verified(&cid_truetype_font(TYPE0, &short, "<00010002>", 0, &ab()))), ids(&["31-016"]));
    // A Type 3 glyph's width is the one its description's d0 or d1 gives.
    let type3 = |widths: &str| {
        verified(&with_font(
            &format!(
                "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 16 0 R >> \
                 /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [{widths}] >>"
            ),
            "(a)",
            0,
            &[stream("400 0 0 0 100 100 d1 0 0 100 100 re f")],
        ))
    };
    assert_eq!(font_failures(&type3("500")), ids(&["31-016"]));
    assert!(font_failures(&type3("400")).is_empty());
}

#[test]
fn truetype_encodings_and_cmaps_meet_clause_7_21_6() {
    let tt = |encoding: &str| TRUETYPE.replace("/Encoding /WinAnsiEncoding", encoding);
    let failures = |encoding: &str, flags: u32, program: &TrueType| font_failures(&verified(&truetype_font(&tt(encoding), flags, program)));
    assert_eq!(failures("", 32, &ab()), ids(&["31-019"]));
    assert_eq!(failures("/Encoding /StandardEncoding", 32, &ab()), ids(&["31-021"]));
    assert_eq!(failures("/Encoding << /BaseEncoding /MacExpertEncoding >>", 32, &ab()), ids(&["31-021"]));
    assert_eq!(failures("/Encoding << /Differences [66 /A] >>", 32, &ab()), ids(&["31-020"]));
    assert!(failures("/Encoding << /BaseEncoding /MacRomanEncoding /Differences [66 /A] >>", 32, &ab()).is_empty(), "A is a Latin glyph");
    let r = verified(&truetype_font(&tt("/Encoding << /BaseEncoding /WinAnsiEncoding /Differences [66 /g12.alt] >>"), 32, &ab()));
    assert_eq!(messages(&r, "31-022"), ["Font Sample on page 1: its Differences name glyph g12.alt, which isn't in the Adobe Glyph List"]);
    // A name of letters and digits may be in the list, which PdfCraft doesn't hold.
    let r = verified(&truetype_font(&tt("/Encoding << /BaseEncoding /WinAnsiEncoding /Differences [67 /afii10017] >>"), 32, &ab()));
    assert_eq!(outcome(&r, "31-022"), Outcome::NotChecked);
    // Differences need a (3,1) cmap; a Mac Roman (1,0) one still finds the glyphs.
    let mac = TrueType { glyphs: ab().glyphs, cmaps: vec![((1, 0), vec![(65, 1), (66, 2)])] };
    assert_eq!(failures("/Encoding << /BaseEncoding /WinAnsiEncoding /Differences [66 /B] >>", 32, &mac), ids(&["31-023"]));
    assert!(failures("/Encoding /WinAnsiEncoding", 32, &mac).is_empty());
    // A non-symbolic font needs a non-symbolic cmap, and one that finds every glyph it draws.
    let symbol_cmap = TrueType { glyphs: ab().glyphs, cmaps: vec![((3, 0), vec![(0xF041, 1), (0xF042, 2)])] };
    assert_eq!(failures("/Encoding /WinAnsiEncoding", 32, &symbol_cmap), ids(&["31-017"]));
    let r = verified(&truetype_font(TRUETYPE, 32, &TrueType::latin(['A'], 600)));
    assert!(font_failures(&r).contains("31-018"), "{:?}", font_failures(&r));
    assert_eq!(messages(&r, "31-018"), ["Font Sample on page 1: no Unicode or Mac Roman cmap of its program finds glyph B (code <42>)"]);
    // Symbolic fonts: no Encoding, and a cmap, a (3,0) one when there are several.
    let symbolic = |encoding: &str, program: &TrueType| {
        let mut objs = truetype_font(&tt(encoding).replace("/FontDescriptor 16 0 R", "/FontDescriptor 16 0 R /ToUnicode 18 0 R"), 4, program);
        objs.push(stream("1 begincodespacerange <00> <FF> endcodespacerange 2 beginbfchar <41> <0041> <42> <0042> endbfchar"));
        font_failures(&verified(&objs))
    };
    assert!(symbolic("", &symbol_cmap).is_empty(), "{:?}", symbolic("", &symbol_cmap));
    assert_eq!(symbolic("/Encoding /WinAnsiEncoding", &symbol_cmap), ids(&["31-024"]));
    let none = TrueType { glyphs: ab().glyphs, cmaps: Vec::new() };
    assert_eq!(symbolic("", &none), ids(&["31-025"]));
    let two = TrueType { glyphs: ab().glyphs, cmaps: vec![((3, 1), vec![(65, 1), (66, 2)]), ((1, 0), vec![(65, 1), (66, 2)])] };
    assert_eq!(symbolic("", &two), ids(&["31-026"]));
    assert!(symbolic("", &ab()).is_empty(), "one cmap is enough");
}

#[test]
fn fonts_map_to_unicode_or_need_no_map() {
    // Invisible text in a font that isn't embedded: only the Unicode conditions apply to it.
    let simple = |encoding: &str| verified(&with_font(&format!("<< /Type /Font /Subtype /Type1 /BaseFont /Custom {encoding} >>"), "(AB)", 3, &[]));
    let r = simple("/Encoding << /Differences [65 /g12.alt] >>");
    assert!(font_failures(&r).contains("31-027"), "{:?}", font_failures(&r));
    assert_eq!(messages(&r, "31-027"), ["Font Custom on page 1 has no ToUnicode map, and no exception lets it go without one"]);
    assert_eq!(outcome(&simple("/Encoding << /Differences [65 /alpha] >>"), "31-027"), Outcome::NotChecked, "alpha may be a Symbol glyph");
    assert_eq!(outcome(&simple("/Encoding << /Differences [65 /Aacute] >>"), "31-027"), Outcome::Passed);
    assert_eq!(outcome(&simple("/Encoding /MacExpertEncoding"), "31-027"), Outcome::Passed);
    // A Type 0 font over Identity needs a map; over Adobe-Japan1 it doesn't.
    let type0 = |ordering: &str| {
        let objs =
            cid_truetype_font(&TYPE0.replace(" /ToUnicode 19 0 R", ""), &CIDFONT2.replace("/Ordering (Identity)", ordering), "<0001>", 3, &ab());
        font_failures(&verified(&objs))
    };
    assert!(type0("/Ordering (Identity)").contains("31-027"));
    assert!(!type0("/Ordering (Japan1)").contains("31-027"));
    // The values a ToUnicode map gives: never U+0000, U+FEFF or U+FFFE.
    let mapped = |entries: &str| {
        let mut objs = cid_truetype_font(TYPE0, CIDFONT2, "<00010002>", 0, &ab());
        objs[18] = to_unicode(entries);
        verified(&objs)
    };
    let r = mapped("1 beginbfchar <0001> <0000> endbfchar");
    assert_eq!(messages(&r, "31-028"), ["Font Sample on page 1: its ToUnicode map maps code <0001> to U+0000"]);
    let r = mapped("1 beginbfchar <0002> <FEFF0042> endbfchar");
    assert_eq!(messages(&r, "31-029"), ["Font Sample on page 1: its ToUnicode map maps code <0002> to U+FEFF"]);
    let r = mapped("1 beginbfrange <0030> <0039> <FFF7> endbfrange");
    assert_eq!(messages(&r, "31-029"), ["Font Sample on page 1: its ToUnicode map maps code <0037> to U+FFFE"]);
    assert!(font_failures(&mapped("1 beginbfrange <0001> <0002> <0041> endbfrange")).is_empty());
}

#[test]
fn text_never_draws_notdef() {
    // WinAnsiEncoding and StandardEncoding both leave 0x81 undefined: even invisible, it's .notdef.
    let mut objs = ua();
    objs[CONTENT] = stream(&format!("{PAGE_OPS} /Artifact BMC BT /F1 9 Tf 3 Tr <81> Tj ET EMC"));
    let r = verified(&objs);
    assert_eq!(font_failures(&r), ids(&["31-030"]));
    assert_eq!(messages(&r, "31-030"), ["Font Demo on page 1 draws the .notdef glyph (code <81>)"]);
    // CID 0 is .notdef in every CIDFont.
    let r = verified(&cid_truetype_font(TYPE0, CIDFONT2, "<00000001>", 0, &ab()));
    assert_eq!(messages(&r, "31-030"), ["Font Sample on page 1 draws the .notdef glyph (code <0000>)"]);
}

#[test]
fn a_formula_maps_its_characters_to_unicode() {
    let formula = |font: &str, shown: &str, extra: &[String]| {
        let mut objs = with_font(font, "()", 3, extra);
        objs[CONTENT] = stream(&format!("{PAGE_OPS} /Formula << /MCID 4 >> BDC BT /F9 9 Tf {shown} Tj ET EMC"));
        objs[DOCUMENT] = objs[DOCUMENT].replace("] >>", " << /S /Formula /Pg 3 0 R /Alt (x squared) /K 4 >> ] >>");
        verified(&objs)
    };
    // An Identity Type 0 font with no ToUnicode map: its codes map to nothing.
    let no_map = TYPE0.replace(" /ToUnicode 19 0 R", "");
    let extra = vec![CIDFONT2.to_owned(), format!("<< {DESCRIPTOR} /Flags 32 /FontFile2 18 0 R >>"), hex_stream("", &ab().bytes()), to_unicode("")];
    let r = formula(&no_map, "<0001>", &extra);
    assert!(font_failures(&r).contains("17-003"), "{:?}", font_failures(&r));
    assert_eq!(messages(&r, "17-003"), ["Font Sample on page 1: code <0001> in a formula can't be mapped to Unicode"]);
    // With a ToUnicode map the same formula is fine, and so is one in a WinAnsi font.
    let mut mapped = extra.clone();
    mapped[3] = to_unicode("1 beginbfchar <0001> <0078> endbfchar");
    assert_eq!(outcome(&formula(TYPE0, "<0001>", &mapped), "17-003"), Outcome::Passed);
    let mut objs = ua();
    objs[CONTENT] = stream(&format!("{PAGE_OPS} /Formula << /MCID 4 >> BDC BT /F1 9 Tf (x) Tj ET EMC"));
    objs[DOCUMENT] = objs[DOCUMENT].replace("] >>", " << /S /Formula /Pg 3 0 R /Alt (x) /K 4 >> ] >>");
    assert_eq!(outcome(&verified(&objs), "17-003"), Outcome::Passed);
    // The same content outside a formula is not a formula's, whatever else it fails.
    let r = verified(&with_font(&no_map, "<0001>", 0, &extra));
    assert_eq!(outcome(&r, "17-003"), Outcome::Passed);
    // A glyph name PdfCraft can't take to Unicode leaves the formula undecided.
    let r = formula("<< /Type /Font /Subtype /Type1 /BaseFont /Custom /Encoding << /Differences [65 /alpha] >> >>", "(A)", &[]);
    assert_eq!(outcome(&r, "17-003"), Outcome::NotChecked);
}

#[test]
fn what_pdfcraft_cant_read_is_left_not_checked() {
    // A font program that doesn't parse.
    let mut objs = ua();
    objs[PROGRAM] = stream("not a real font program");
    let r = verified(&objs);
    assert!(font_failures(&r).is_empty(), "{:?}", font_failures(&r));
    for id in ["31-011", "31-016", "31-017", "31-018", "31-030"] {
        assert_eq!(outcome(&r, id), Outcome::NotChecked, "{id}");
    }
    // A predefined CMap other than Identity: PdfCraft doesn't read which CID a code selects.
    let font = TYPE0.replace("/Identity-H", "/UniJIS-UCS2-H");
    let objs = cid_truetype_font(&font, &CIDFONT2.replace("/Ordering (Identity)", "/Ordering (Japan1)"), "<0001>", 0, &ab());
    let r = verified(&objs);
    for id in ["31-011", "31-016", "31-030"] {
        assert_eq!(outcome(&r, id), Outcome::NotChecked, "{id}");
    }
    // What a program that doesn't parse would have to be held against: a CharSet, a CIDSet,
    // a symbolic font's cmaps, the (3,1) cmap Differences need.
    let garbage = || stream("not a font program");
    let not_checked = |objs: &[String], ids: &[&str]| {
        let r = verified(objs);
        for id in ids {
            assert_eq!(outcome(&r, id), Outcome::NotChecked, "{id}");
        }
    };
    not_checked(
        &with_font(TYPE1, "(AB)", 0, &[format!("<< {DESCRIPTOR} /Flags 32 /CharSet (/A/B) /FontFile 17 0 R >>"), garbage()]),
        &["31-012", "31-013"],
    );
    let mut objs = cid_truetype_font(TYPE0, CIDFONT2, "<00010002>", 0, &ab());
    objs[16] = objs[16].replace("/FontFile2 18 0 R", "/FontFile2 18 0 R /CIDSet 20 0 R");
    objs[17] = garbage();
    objs.push(hex_stream("", &[0b1110_0000]));
    not_checked(&objs, &["31-014", "31-015"]);
    let symbolic = TRUETYPE.replace(" /Encoding /WinAnsiEncoding", "");
    not_checked(&with_font(&symbolic, "(AB)", 0, &[format!("<< {DESCRIPTOR} /Flags 4 /FontFile2 17 0 R >>"), garbage()]), &["31-025", "31-026"]);
    let differences = TRUETYPE.replace("/Encoding /WinAnsiEncoding", "/Encoding << /BaseEncoding /WinAnsiEncoding /Differences [66 /B] >>");
    not_checked(&with_font(&differences, "(AB)", 0, &[format!("<< {DESCRIPTOR} /Flags 32 /FontFile2 17 0 R >>"), garbage()]), &["31-023"]);
    // An embedded CMap that doesn't say its character collection.
    let mut objs = cid_truetype_font(
        &TYPE0.replace("/Identity-H", "20 0 R"),
        &CIDFONT2.replace("/Ordering (Identity)", "/Ordering (Japan1)"),
        "<0001>",
        3,
        &ab(),
    );
    objs.push(stream("1 begincodespacerange <0000> <FFFF> endcodespacerange 1 begincidrange <0000> <FFFF> 0 endcidrange"));
    not_checked(&objs, &["31-001", "31-002", "31-003"]);
}

/// Each font condition, its checks switched off in turn, is noticed by some test here: no test of
/// them passes whatever the checks do.
#[test]
fn every_font_condition_is_seen_by_a_test() {
    let tests: [(&str, fn()); 14] = [
        ("fonts_that_draw_text_are_embedded_and_map_to_unicode", fonts_that_draw_text_are_embedded_and_map_to_unicode),
        ("type0_cmaps_and_character_collections_agree", type0_cmaps_and_character_collections_agree),
        ("truetype_cid_fonts_name_their_cid_to_gid_map", truetype_cid_fonts_name_their_cid_to_gid_map),
        ("cid_sets_list_exactly_the_cids_the_program_has", cid_sets_list_exactly_the_cids_the_program_has),
        ("type1_charsets_list_exactly_the_glyphs_the_program_has", type1_charsets_list_exactly_the_glyphs_the_program_has),
        ("embedded_fonts_have_every_glyph_the_text_draws", embedded_fonts_have_every_glyph_the_text_draws),
        ("glyph_widths_agree_with_the_program", glyph_widths_agree_with_the_program),
        ("truetype_encodings_and_cmaps_meet_clause_7_21_6", truetype_encodings_and_cmaps_meet_clause_7_21_6),
        ("fonts_map_to_unicode_or_need_no_map", fonts_map_to_unicode_or_need_no_map),
        ("text_never_draws_notdef", text_never_draws_notdef),
        ("a_formula_maps_its_characters_to_unicode", a_formula_maps_its_characters_to_unicode),
        ("what_pdfcraft_cant_read_is_left_not_checked", what_pdfcraft_cant_read_is_left_not_checked),
        ("codes_past_the_limits_leave_their_conditions_not_checked", codes_past_the_limits_leave_their_conditions_not_checked),
        (
            "a_conforming_file_fails_no_checked_condition_and_leaves_the_rest_to_a_person",
            a_conforming_file_fails_no_checked_condition_and_leaves_the_rest_to_a_person,
        ),
    ];
    for id in FONT_CONDITIONS {
        SWITCHED_OFF.with(|off| off.set(Some(id)));
        let noticed = tests.iter().filter(|(_, test)| std::panic::catch_unwind(*test).is_err()).count();
        SWITCHED_OFF.with(|off| off.set(None));
        assert!(noticed > 0, "no test notices {id} switched off");
    }
    // With nothing switched off, every one of them passes.
    for (name, test) in tests {
        assert!(std::panic::catch_unwind(test).is_ok(), "{name}");
    }
}

/// Past the limits on the codes kept, what the codes decide is not checked rather than passed.
#[test]
fn codes_past_the_limits_leave_their_conditions_not_checked() {
    let doc = pdf(&ua(), "/Info 8 0 R");
    let pages = pdfcraft_model::pages(&doc);
    let run = |limits: (usize, usize)| {
        let mut codes = fonts::Codes::with_limits(&doc, std::collections::HashSet::new(), limits);
        let scan = crate::content::Scan::run_with(&doc, &pages, &[0], &mut codes);
        let mut c = Collected::default();
        fonts::check(&doc, &scan.fonts, &codes, &mut c);
        c
    };
    // The page shows seven distinct codes (T, i, t, l, e, A and 1) in one font.
    let all = run((7, 7));
    let cut = run((3, 100));
    let cut_in_all = run((100, 6));
    for id in ["31-011", "31-016", "31-018", "31-027", "31-030", "17-003"] {
        assert!(!all.is_undecided(id) && !all.failed(id), "{id}");
        assert!(cut.is_undecided(id) && cut_in_all.is_undecided(id), "{id}");
    }
}

/// Font dictionaries and programs at their worst: nothing panics.
#[test]
fn hostile_fonts_end_without_panicking() {
    let fonts = [
        "<< /Type /Font /Subtype /Type0 /Encoding 16 0 R /DescendantFonts [15 0 R] >>",
        "<< /Type /Font /Subtype /Type0 /Encoding /Identity-H /DescendantFonts 7 >>",
        "<< /Type /Font /Subtype /TrueType /FirstChar -5 /Widths [1e308 -1e308 0] /Encoding << /Differences [9223372036854775807 /a -3 /b 255 /c /d] >> \
         /FontDescriptor 17 0 R >>",
        "<< /Type /Font /Subtype /Type1 /Encoding << /BaseEncoding /Nonsense /Differences 3 >> /FontDescriptor 17 0 R /ToUnicode 16 0 R >>",
        "<< /Type /Font /Subtype /Type3 /FontMatrix [0 0 0 0 0 0] /CharProcs << /a 16 0 R >> /Encoding << /Differences [0 /a] >> /Widths [1] >>",
        "<< /Type /Font /Subtype /Bogus >>",
        "<< /Type /Font /Subtype /Type0 /Encoding /Identity-H /DescendantFonts [<< /Subtype /CIDFontType2 /CIDToGIDMap 16 0 R \
         /W [0 [] 4294967295 4294967296 1 5] /FontDescriptor 17 0 R >>] >>",
        "<< /Type /Font /Subtype /Type0 /Encoding /Identity-H /DescendantFonts [<< /Subtype /CIDFontType0 /FontDescriptor 17 0 R >>] >>",
    ];
    let extras = [
        stream("1 begincodespacerange <00> endcodespacerange begincidrange <0000> <FFFF> 4294967296 endcidrange /WMode def usecmap /x usecmap"),
        "<< /Flags 1e300 /FontFile2 18 0 R /FontFile 18 0 R /FontFile3 18 0 R /CIDSet 16 0 R /CharSet (/) >>".to_owned(),
        hex_stream("", &[0, 1, 0, 0, 0, 255, 0, 0, 0, 0, 0, 0, b'c', b'm', b'a', b'p', 0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255]),
    ];
    for font in fonts {
        for shown in ["<00>", "<0000FFFF>", "()", "<41424344>"] {
            for mode in [0, 3] {
                let r = verified(&with_font(font, shown, mode, &extras));
                assert_eq!(r.results.len(), 135);
            }
        }
    }
}

/// Content past the page's decode budget may hold what the checker didn't see: untagged content
/// or a font without Unicode mappings. Those conditions are not checked rather than passed.
#[test]
fn content_past_the_decode_budget_is_not_checked() {
    use pdfcraft_cos::{ObjRef, Stream};
    let mut doc = pdf(&ua(), "/Info 8 0 R");
    let r = verify(&doc, Part::Ua1);
    assert_eq!((outcome(&r, "01-005"), outcome(&r, "10-001")), (Outcome::Passed, Outcome::Passed));
    let filler = doc.add(Stream::flate(Dict::new(), &vec![b' '; 1 << 20]));
    let mut contents = vec![Object::Ref(ObjRef::new(4, 0))];
    contents.extend(std::iter::repeat_n(Object::Ref(filler), 256));
    doc.update_dict(ObjRef::new(3, 0), |p| p.set(b"Contents".to_vec(), Object::Array(contents))).unwrap();
    let r = verify(&doc, Part::Ua1);
    assert_eq!((outcome(&r, "01-005"), outcome(&r, "10-001")), (Outcome::NotChecked, Outcome::NotChecked));
}
