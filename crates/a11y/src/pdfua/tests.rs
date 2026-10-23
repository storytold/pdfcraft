//! PDF/UA verification on synthetic files: a file that meets every checked condition, and the same
//! file broken one way at a time.

use std::collections::BTreeSet;

use pdfcraft_cos::{Dict, Object};

use super::checks::{NO_LANGUAGE, security};
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
        "<< /Type /Font /Subtype /TrueType /BaseFont /Demo /FirstChar 32 /LastChar 32 /Widths [500] /Encoding /WinAnsiEncoding /FontDescriptor 13 0 R >>"
            .into(),
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
        stream("not a real font program"),
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
    assert_eq!((r.count(Outcome::Passed), r.count(Outcome::Manual), r.count(Outcome::NotChecked)), (55, 47, 33));
    assert_eq!(r.result("13-005").map(|x| x.outcome), Some(Outcome::Manual), "only a person can judge it");
    assert_eq!(r.result("31-011").map(|x| x.outcome), Some(Outcome::NotChecked), "a machine check PdfCraft lacks is not passed");
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
    // it needn't be embedded, but its codes still have to map to Unicode.
    assert_eq!(failed(&r), ids(&["10-001", "31-009"]));
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
    let source = include_str!("checks.rs");
    let source = source.get(..source.find("#[cfg(test)]").unwrap_or(source.len())).unwrap_or(source);
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
