//! Ungroup: grouped Form artwork becomes page content (`crate::ungroup`).

use pdfcraft_cos::{Document, ObjRef, Object};

use super::{build, reopen, stream, streams};
use crate::{BlockStyle, images, text, ungroup_form};

/// A one-page document whose page draws `form` (object 6) through `/Fm1`, with page font `/F1`
/// Courier (object 5) and `extra` objects from 7 on.
fn grouped_page(page_content: &str, form_dict: &str, form_content: &str, extra: &[&str]) -> Document {
    let page = stream("", page_content);
    let form = stream(&format!("/Type /XObject /Subtype /Form {form_dict}"), form_content);
    let mut objs: Vec<&str> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 5 0 R >> /XObject << /Fm1 6 0 R >> >> /Contents 4 0 R >>",
        &page,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>",
        &form,
    ];
    objs.extend_from_slice(extra);
    build(&objs)
}

fn page_resources(doc: &Document) -> pdfcraft_cos::Dict {
    let p = &pdfcraft_model::pages(doc)[0];
    doc.resolve(p.dict.get(b"Resources").unwrap()).as_dict().cloned().unwrap()
}

fn category(doc: &Document, name: &[u8]) -> pdfcraft_cos::Dict {
    doc.resolve(page_resources(doc).get(name).unwrap()).as_dict().cloned().unwrap()
}

/// WSCAD and similar CAD exports draw a whole page as one Form XObject. Ungrouping puts its
/// content on the page, through its matrix and inside its bounding box, so its text becomes
/// editable paragraphs and its images separate objects, while the page keeps its own text.
#[test]
fn ungrouping_a_page_sized_form_makes_its_content_editable() {
    let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", "A");
    let mut doc = grouped_page(
        "q 0.5 0 0 0.5 0 0 cm /Fm1 Do Q BT /F1 12 Tf 72 100 Td (Page text) Tj ET",
        "/BBox [0 0 1200 1600] /Matrix [1 0 0 1 0 40] /Resources << /Font << /F2 7 0 R >> /XObject << /Im1 8 0 R >> >>",
        "BT /F2 40 Tf 100 1400 Td (Schema title) Tj ET q 200 0 0 100 100 100 cm /Im1 Do Q",
        &["<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>", &image],
    );
    let before = images::page_images(&doc, 0).unwrap();
    assert_eq!(before.len(), 1);
    assert!(before[0].is_form);
    assert_eq!(text::text_blocks(&doc, 0).unwrap().iter().map(|b| b.text.as_str()).collect::<Vec<_>>(), ["Page text"]);

    ungroup_form(&mut doc, 0, 0).unwrap();
    let doc = reopen(&doc);

    let blocks = text::text_blocks(&doc, 0).unwrap();
    let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(texts, ["Schema title", "Page text"]);
    let title = &blocks[0];
    // (100, 1400) moved by the form's (0, 40), then halved by the page.
    assert_eq!(title.base_font, "Helvetica");
    assert!((title.rect[0] - 50.0).abs() < 0.01 && (title.size - 20.0).abs() < 0.01, "{title:?}");
    let after = images::page_images(&doc, 0).unwrap();
    assert_eq!(after.len(), 1, "{after:?}");
    assert!(!after[0].is_form);
    let r = after[0].rect;
    assert!((r[0] - 50.0).abs() < 0.01 && (r[1] - 70.0).abs() < 0.01 && (r[2] - 150.0).abs() < 0.01 && (r[3] - 120.0).abs() < 0.01, "{r:?}");
    // The form's bounding box clips the inlined content, as it clipped the form.
    let content = streams(&doc, 0).concat();
    assert!(content.contains("0 40 cm") && content.contains("1200 1600 re W n"), "{content}");
    // The form object is unchanged; other users of it are not affected.
    assert!(matches!(&*doc.get(ObjRef::new(6, 0)), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Form")));

    // The inlined paragraph is now editable like any other.
    let mut edited = doc.clone();
    text::rewrite_block(&mut edited, 0, 0, Some("New title"), &BlockStyle::default()).unwrap();
    let edited = reopen(&edited);
    assert!(text::text_blocks(&edited, 0).unwrap().iter().any(|b| b.text == "New title"));
}

/// A form resource whose name the page uses for something else is renamed in the inlined content
/// only; names that mean the same thing are shared, and the page's own text keeps its font.
#[test]
fn ungrouping_renames_clashing_resources() {
    let mut doc = grouped_page(
        "/Fm1 Do BT /F1 12 Tf 72 100 Td (Page) Tj ET",
        "/BBox [0 0 600 800] /Resources << /Font << /F1 7 0 R /F9 5 0 R >> /ExtGState << /G1 << /LW 3 >> >> >>",
        "/G1 gs BT /F1 20 Tf 72 700 Td (Form) Tj /F9 10 Tf (Shared) Tj ET",
        &["<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"],
    );
    ungroup_form(&mut doc, 0, 0).unwrap();
    let doc = reopen(&doc);
    let fonts = category(&doc, b"Font");
    assert_eq!(fonts.get(b"F1"), Some(&Object::Ref(ObjRef::new(5, 0))), "the page's /F1 stays Courier");
    assert_eq!(fonts.get(b"F1_PC1"), Some(&Object::Ref(ObjRef::new(7, 0))), "{fonts:?}");
    assert_eq!(fonts.get(b"F9"), Some(&Object::Ref(ObjRef::new(5, 0))));
    let content = streams(&doc, 0).concat();
    assert!(content.contains("/F1_PC1 20 Tf") && content.contains("/F9 10 Tf") && content.contains("/G1 gs"), "{content}");
    let fonts_used: Vec<(String, String)> = text::text_blocks(&doc, 0).unwrap().into_iter().map(|b| (b.text, b.base_font)).collect();
    assert!(fonts_used.contains(&("Page".into(), "Courier".into())), "{fonts_used:?}");
    assert!(fonts_used.iter().any(|(t, f)| t.starts_with("Form") && f == "Helvetica"), "{fonts_used:?}");
}

/// What a form leaves open at its end is closed when its content is inlined, so the page's own
/// operators after it keep their graphics state; what it closes without opening is refused.
#[test]
fn ungrouping_keeps_the_page_state_balanced() {
    let mut doc =
        grouped_page("/Fm1 Do BT /F1 12 Tf 72 100 Td (Page) Tj ET", "/BBox [0 0 600 800]", "q 2 0 0 2 0 0 cm q /OC /L1 BDC BT 1 0 0 rg", &[]);
    ungroup_form(&mut doc, 0, 0).unwrap();
    let doc = reopen(&doc);
    let content = streams(&doc, 0).concat();
    assert!(content.contains("1 0 0 rg\nET\nEMC\nQ\nQ\nQ"), "{content}");
    let page = &text::text_blocks(&doc, 0).unwrap()[0];
    assert_eq!(page.text, "Page");
    assert!((page.rect[0] - 72.0).abs() < 0.01, "the page's text is not scaled by the form: {page:?}");

    for (content, why) in [
        ("Q 0 0 1 rg", "restores more"),
        ("BT ET ET", "did not start"),
        ("EMC", "did not start"),
        ("BT BT ET", "inside another"),
        ("BT /Im1 Do ET", "inside a text object"),
        ("(unterminated", "damaged"),
    ] {
        let mut doc = grouped_page("/Fm1 Do", "/BBox [0 0 600 800]", content, &[]);
        let before = streams(&doc, 0);
        let err = ungroup_form(&mut doc, 0, 0).unwrap_err().to_string();
        assert!(err.contains(why), "{content}: {err}");
        assert_eq!(streams(&doc, 0), before, "a refusal changes nothing");
    }
    // A group drawn (invalidly) inside the page's own text object would nest text objects.
    let mut doc = grouped_page("BT /F1 12 Tf /Fm1 Do ET", "/BBox [0 0 600 800]", "BT ET", &[]);
    assert_eq!(images::page_images(&doc, 0).unwrap().len(), 1);
    assert!(ungroup_form(&mut doc, 0, 0).unwrap_err().to_string().contains("drawn inside a text object"));
}

/// A transparency group stays grouped when ungrouping would change its blending: a knockout
/// group, transparency inside it, another blending space, or alpha applied to the group as a
/// whole. A group around opaque content (what most exporters write) is ungrouped.
#[test]
fn ungrouping_refuses_groups_whose_blending_matters() {
    let group = "/Group << /S /Transparency /I true >>";
    let mut plain = grouped_page("/Fm1 Do", &format!("/BBox [0 0 600 800] {group}"), "0 0 1 rg 10 10 100 100 re f", &[]);
    ungroup_form(&mut plain, 0, 0).unwrap();

    let cases = [
        "/BBox [0 0 600 800] /Group << /S /Transparency /K true >>",
        "/BBox [0 0 600 800] /Group << /S /Transparency >> /Resources << /ExtGState << /A << /ca 0.5 >> >> >>",
        "/BBox [0 0 600 800] /Group << /S /Transparency >> /Resources << /ExtGState << /A << /BM /Multiply >> >> >>",
        "/BBox [0 0 600 800] /Group << /S /Transparency /CS /DeviceCMYK >>",
    ];
    for dict in cases {
        let mut doc = grouped_page("/Fm1 Do", dict, "/A gs 10 10 100 100 re f", &[]);
        let err = ungroup_form(&mut doc, 0, 0).unwrap_err().to_string();
        assert!(err.contains("transparency group"), "{dict}: {err}");
    }
    // Alpha set on the page before the group applies to the group as a whole.
    let page = stream("", "/H gs /Fm1 Do");
    let form = stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 600 800] {group}"), "10 10 100 100 re f 50 50 100 100 re f");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /ExtGState << /H << /CA 0.5 /ca 0.5 >> >> /XObject << /Fm1 5 0 R >> >> /Contents 4 0 R >>",
        &page,
        &form,
    ]);
    assert!(ungroup_form(&mut doc, 0, 0).unwrap_err().to_string().contains("transparency group"));
}

/// Tagged forms, forms with other default colour spaces, forms without a box and images are not
/// ungrouped.
#[test]
fn ungrouping_refuses_what_it_cannot_keep() {
    let mut tagged = grouped_page("/Fm1 Do", "/BBox [0 0 600 800] /StructParents 3", "BT ET", &[]);
    assert!(ungroup_form(&mut tagged, 0, 0).unwrap_err().to_string().contains("accessibility"));
    let mut tagged = grouped_page("/Fm1 Do", "/BBox [0 0 600 800] /StructParent 1", "BT ET", &[]);
    assert!(ungroup_form(&mut tagged, 0, 0).unwrap_err().to_string().contains("accessibility"));
    let mut defaults =
        grouped_page("/Fm1 Do", "/BBox [0 0 600 800] /Resources << /ColorSpace << /DefaultRGB /DeviceGray >> >>", "1 0 0 rg 0 0 9 9 re f", &[]);
    assert!(ungroup_form(&mut defaults, 0, 0).unwrap_err().to_string().contains("default colour spaces"));
    // A form without a bounding box is not drawn, so it is not offered as a group at all.
    let no_box = grouped_page("/Fm1 Do", "", "0 0 9 9 re f", &[]);
    assert!(images::page_images(&no_box, 0).unwrap().is_empty());
    let image = stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8", "A");
    let mut picture = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /XObject << /Im1 5 0 R >> >> /Contents 4 0 R >>",
        &stream("", "q 9 0 0 9 0 0 cm /Im1 Do Q"),
        &image,
    ]);
    assert!(ungroup_form(&mut picture, 0, 0).unwrap_err().to_string().contains("this is an image"));
    assert!(ungroup_form(&mut picture, 0, 1).is_err());
    assert!(ungroup_form(&mut picture, 3, 0).is_err());
}

/// Optional content on the form marks the inlined content with the same membership, and an
/// inline image's colour space resource follows a rename.
#[test]
fn ungrouping_keeps_optional_content_and_inline_image_spaces() {
    let mut doc = grouped_page(
        "/Fm1 Do",
        "/BBox [0 0 600 800] /OC 7 0 R /Resources << /ColorSpace << /F1 [/Indexed /DeviceRGB 1 <000000FFFFFF>] >> >>",
        "q 10 0 0 10 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /F1 ID \x01 EI Q",
        &["<< /Type /OCG /Name (Layer) >>"],
    );
    // The page's /F1 is a font, not a colour space: names in other categories don't clash.
    ungroup_form(&mut doc, 0, 0).unwrap();
    let doc = reopen(&doc);
    let content = streams(&doc, 0).concat();
    assert!(content.contains("/OC /PCOC1 BDC") && content.trim_end().ends_with("EMC\nQ"), "{content}");
    assert!(content.contains("/CS /F1"), "{content}");
    assert_eq!(category(&doc, b"Properties").get(b"PCOC1"), Some(&Object::Ref(ObjRef::new(7, 0))));

    let mut clash = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /ColorSpace << /S1 /DeviceGray >> /XObject << /Fm1 5 0 R >> >> /Contents 4 0 R >>",
        &stream("", "/Fm1 Do"),
        &stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 600 800] /Resources << /ColorSpace << /S1 [/Indexed /DeviceRGB 1 <000000FFFFFF>] >> >>",
            "q 10 0 0 10 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /S1 ID \x01 EI Q /S1 cs 1 sc 0 0 5 5 re f",
        ),
    ]);
    ungroup_form(&mut clash, 0, 0).unwrap();
    let content = streams(&clash, 0).concat();
    assert!(content.contains("/CS /S1_PC1") && content.contains("/S1_PC1 cs"), "{content}");
}

/// Ungrouping one placement leaves other pages that draw the same form, and other placements
/// on the same page, as they were.
#[test]
fn ungrouping_changes_only_the_chosen_placement() {
    let form = stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Resources << /Font << /F2 7 0 R >> >>", "BT /F2 10 Tf 5 5 Td (Part) Tj ET");
    let mut doc = build(&[
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 8 0 R] /Count 2 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /XObject << /Fm1 6 0 R >> >> /Contents 4 0 R >>",
        &stream("", "/Fm1 Do q 1 0 0 1 200 0 cm /Fm1 Do Q"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier >>",
        &form,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /XObject << /Fm1 6 0 R >> >> /Contents 9 0 R >>",
        &stream("", "/Fm1 Do"),
    ]);
    let other_before = streams(&doc, 1);
    ungroup_form(&mut doc, 0, 1).unwrap();
    let doc = reopen(&doc);
    let own = images::page_images(&doc, 0).unwrap();
    assert_eq!(own.len(), 1);
    assert!(own[0].is_form && own[0].rect[0].abs() < 0.01, "the first placement stays a group: {own:?}");
    let blocks = text::text_blocks(&doc, 0).unwrap();
    assert_eq!(blocks.len(), 1);
    assert!((blocks[0].rect[0] - 205.0).abs() < 0.01, "{blocks:?}");
    assert_eq!(streams(&doc, 1), other_before);
}

/// A nested Form without `/Resources` names its resources from the group that draws it. Once a
/// clashing name is renamed for the page, inlining would leave the nested Form reading the page's
/// resource of that name instead: refused. Without a clash it is ungrouped as before.
#[test]
fn ungrouping_refuses_renames_a_nested_form_would_miss() {
    let nested = stream("/Type /XObject /Subtype /Form /BBox [0 0 100 100]", "BT /F1 10 Tf 5 5 Td (Inner) Tj ET");
    let mut clash = grouped_page(
        "/Fm1 Do",
        "/BBox [0 0 600 800] /Resources << /Font << /F1 7 0 R >> /XObject << /In 8 0 R >> >>",
        "BT /F1 12 Tf 72 700 Td (Outer) Tj ET /In Do",
        &["<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>", &nested],
    );
    let before = streams(&clash, 0);
    let err = ungroup_form(&mut clash, 0, 0).unwrap_err().to_string();
    assert!(err.contains("nested group"), "{err}");
    assert_eq!(streams(&clash, 0), before, "a refusal changes nothing");

    // The same nested Form, but the group's /F1 is the page's /F1: nothing is renamed.
    let mut shared = grouped_page(
        "/Fm1 Do",
        "/BBox [0 0 600 800] /Resources << /Font << /F1 5 0 R >> /XObject << /In 7 0 R >> >>",
        "BT /F1 12 Tf 72 700 Td (Outer) Tj ET /In Do",
        &[&nested],
    );
    ungroup_form(&mut shared, 0, 0).unwrap();
}
