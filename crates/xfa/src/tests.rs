use std::sync::Arc;

use printcraft_cos::{Document, Object, SaveOptions, write_incremental};

use super::*;
use crate::fixtures::{shell, template};
use crate::layout::Item;
use crate::model::*;

fn widgets(page: &Page) -> Vec<&Widget> {
    page.items.iter().filter_map(|i| if let Item::Widget(w) = i { Some(w.as_ref()) } else { None }).collect()
}

fn texts(page: &Page) -> Vec<String> {
    page.items.iter().filter_map(|i| if let Item::Text(s) = i { Some(s.text.clone()) } else { None }).collect()
}

#[test]
fn measurements_convert_to_points() {
    assert_eq!(measure("72pt"), Some(72.0));
    assert_eq!(measure("1in"), Some(72.0));
    assert!((measure("25.4mm").unwrap() - 72.0).abs() < 1e-9);
    assert!((measure("2.54cm").unwrap() - 72.0).abs() < 1e-9);
    assert_eq!(measure(" 12 "), Some(12.0));
    assert_eq!(measure("3em"), None);
    assert_eq!(measure("abc"), None);
    assert_eq!(measure("NaNpt"), None);
}

#[test]
fn the_template_parses_with_fonts_captions_and_rich_text() {
    let (tpl, warnings) = parse(&template(2)).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(tpl.root.layout, Layout::Tb);
    let ps = tpl.root.page_set.as_ref().unwrap();
    assert_eq!(ps.areas.len(), 2);
    assert_eq!((ps.areas[0].width, ps.areas[0].height), (612.0, 792.0));
    assert_eq!(ps.areas[0].occur_max, Some(1));
    assert_eq!(ps.areas[1].occur_max, None);
    assert_eq!(ps.areas[0].content[0], Rect::new(36.0, 36.0, 540.0, 720.0));
    assert_eq!(tpl.page_roles, vec![("pageNo".to_string(), PageRole::Number), ("pageCount".to_string(), PageRole::Count)]);
    let Node::Subform(head) = &tpl.root.children[0] else { panic!() };
    assert_eq!(head.layout, Layout::LrTb);
    let Node::Draw(title) = &head.children[0] else { panic!() };
    let Value::Rich(r) = &title.value else { panic!("rich text") };
    assert_eq!(r.paragraphs.len(), 2, "{r:?}");
    assert_eq!(r.paragraphs[0].runs[0].bold, Some(true));
    assert_eq!(r.plain(), "Sample Form\nSecond line");
    let Node::Field(f) = &head.children[2] else { panic!() };
    assert_eq!(f.common.name.as_deref(), Some("familyName"));
    assert_eq!(f.common.font.typeface.as_deref(), Some("Courier New"));
    assert_eq!(f.max_chars, Some(30));
    assert_eq!(f.tooltip.as_deref(), Some("Your family name"));
    let cap = f.caption.as_ref().unwrap();
    assert_eq!((cap.placement, cap.reserve), (Placement::Top, Some(14.4)));
    // The widget border: three hidden edges and a visible bottom one.
    assert_eq!(f.ui_border.as_ref().unwrap().visible_edges(), [false, false, true, false]);
    let Node::Field(check) = &head.children[3] else { panic!() };
    assert_eq!((check.ui.clone(), check.items.clone()), (Ui::CheckButton, vec!["1".to_string(), "0".to_string()]));
    let Node::Field(date) = &head.children[4] else { panic!() };
    assert_eq!((date.ui.clone(), date.picture.as_deref()), (Ui::DateTimeEdit, Some("date{YYYY-MM-DD}")));
    let Node::ExclGroup(g) = &head.children[6] else { panic!() };
    assert_eq!(g.fields.len(), 2);
    let Node::Subform(table) = &tpl.root.children[1] else { panic!() };
    assert_eq!(table.column_widths, vec![144.0, 216.0, 180.0]);
    assert_eq!(table.overflow_leader.as_deref(), Some("header"));
    let Node::Subform(last) = &tpl.root.children[2] else { panic!() };
    assert!(last.break_before_page);
}

#[test]
fn the_xdp_template_wins_over_the_config_packets_template_element() {
    // The config packet has a <template> too; the real one is in the xfa-template namespace.
    let (tpl, _) = parse(&template(0)).unwrap();
    assert_eq!(tpl.root.common.name.as_deref(), Some("form"));
}

#[test]
fn layout_places_captions_fields_and_sizes_containers_to_their_content() {
    let form = layout_xml(&template(1)).unwrap();
    assert_eq!(form.pages.len(), 2, "head and table on page 1, the last section after its break");
    let p1 = &form.pages[0];
    let t = texts(p1);
    assert!(t.iter().any(|s| s == "Sample Form") && t.iter().any(|s| s == "Second line"), "{t:?}");
    assert!(t.iter().any(|s| s == "Family name"), "caption drawn: {t:?}");
    assert!(!t.iter().any(|s| s.contains("never shown")), "hidden draws take no space and draw nothing");
    assert!(t.iter().any(|s| s == "Page 1 of 2"), "embedded page number and count: {t:?}");
    assert!(!texts(&form.pages[1]).iter().any(|s| s.starts_with("Page ")), "the second master has no page-number draw");
    let w = widgets(p1);
    let family = w.iter().find(|w| w.name == "familyName").unwrap();
    // Below the 0.4in title, on a line of its own (3in × 0.5in); the caption takes the top 0.2in.
    assert!((family.rect.x - 36.0).abs() < 0.01 && (family.rect.y - 79.2).abs() < 0.01, "{:?}", family.rect);
    assert!((family.rect.w - 216.0).abs() < 0.01 && (family.rect.h - 21.6).abs() < 0.01, "{:?}", family.rect);
    assert_eq!(family.kind, WidgetKind::Text);
    assert_eq!(family.border.shape, BorderShape::Underline);
    assert_eq!(family.max_chars, Some(30));
    assert_eq!(family.tooltip.as_deref(), Some("Your family name"));
    assert_eq!(family.face.family, crate::text::Family::Courier);
    assert_eq!(family.som, "form[0].head[0].familyName[0]");
    let agree = w.iter().find(|w| w.name == "agree").unwrap();
    assert_eq!(agree.kind, WidgetKind::CheckBox { on: "1".into(), round: false });
    assert!((agree.rect.w - 10.0).abs() < 0.01, "a 10 pt square: {:?}", agree.rect);
    let born = w.iter().find(|w| w.name == "born").unwrap();
    assert_eq!(born.kind, WidgetKind::Date("yyyy-mm-dd".into()));
    // The radio group has no width: it sits beside the question on the same line.
    let q_y = p1
        .items
        .iter()
        .find_map(|i| {
            if let Item::Text(s) = i
                && s.text == "Have you ever?"
            {
                Some(s.baseline)
            } else {
                None
            }
        })
        .unwrap();
    let yes = w.iter().find(|w| matches!(&w.kind, WidgetKind::Radio { group, on, .. } if group == "answer" && on == "Y")).unwrap();
    assert!(yes.rect.x > 6.5 * 72.0 && yes.rect.y < q_y, "beside the question, not under it: {:?} vs baseline {q_y}", yes.rect);
    let go = w.iter().find(|w| w.name == "go").unwrap();
    assert_eq!(go.kind, WidgetKind::Button { caption: "Reset".into() });
    assert_eq!(go.action, Some(Action::Reset));
    assert_eq!(go.border.fill, Some([212.0 / 255.0, 208.0 / 255.0, 200.0 / 255.0]));
    // Table cells take the column widths; the second column has no width of its own.
    let what = w.iter().find(|w| w.name == "what0").unwrap();
    assert!((what.rect.x - (36.0 + 144.0)).abs() < 0.01 && (what.rect.w - 216.0).abs() < 0.01, "{:?}", what.rect);
    assert_eq!(
        form.fields,
        9,
        "familyName, agree, born, yes, no, go, 3 cells; pageNo/pageCount are hidden: {:?}",
        w.iter().map(|w| &w.name).collect::<Vec<_>>()
    );
}

#[test]
fn tables_break_across_pages_and_repeat_their_header_row() {
    let form = layout_xml(&template(40)).unwrap();
    assert!(form.pages.len() >= 3, "{} pages", form.pages.len());
    // The header row's "Activity" cell appears at the top of every page the table continues on.
    for (i, p) in form.pages.iter().enumerate().take(form.pages.len() - 1) {
        let headers = p.items.iter().filter(|it| matches!(it, Item::Text(s) if s.text == "Activity")).count();
        assert_eq!(headers, 1, "page {}: header rows {headers}", i + 1);
    }
    // Rows are whole: no cell straddles the bottom of the content area.
    for p in &form.pages {
        for w in widgets(p) {
            assert!(w.rect.bottom() <= 36.0 + 720.0 + 0.01, "{} ends below the content area: {:?}", w.name, w.rect);
        }
    }
    let all: Vec<&str> = form.pages.iter().flat_map(|p| widgets(p).into_iter().map(|w| w.name.as_str()).collect::<Vec<_>>()).collect();
    assert_eq!(all.iter().filter(|n| n.starts_with("from")).count(), 40);
    assert_eq!(all.len(), all.iter().collect::<std::collections::HashSet<_>>().len(), "field names are unique");
}

#[test]
fn rendering_into_the_pdf_replaces_the_placeholder_page_and_adds_fields() {
    let mut doc = Document::open(Arc::new(shell(&template(2)))).unwrap();
    assert!(is_dynamic(&doc));
    let report = render_into(&mut doc).unwrap();
    assert_eq!((report.pages, report.fields), (2, 11), "fields: 5 on the head, the radio group once, 6 cells");
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let root = doc.get(doc.root().unwrap());
    let catalog = root.as_dict().unwrap();
    let pages = doc.resolve(catalog.get(b"Pages").unwrap());
    assert_eq!(pages.as_dict().unwrap().int(b"Count"), Some(2));
    let kids = doc.resolve(pages.as_dict().unwrap().get(b"Kids").unwrap()).as_array().unwrap().clone();
    let page1 = doc.resolve(&kids[0]);
    let annots = doc.resolve(page1.as_dict().unwrap().get(b"Annots").unwrap()).as_array().unwrap().len();
    assert!(annots >= 8, "{annots} widgets on page 1");
    let acro = doc.resolve(catalog.get(b"AcroForm").unwrap());
    let acro = acro.as_dict().unwrap();
    assert!(acro.contains(b"XFA"), "the XFA packets stay");
    assert_eq!(catalog.get(b"NeedsRendering"), Some(&Object::Bool(true)), "still a dynamic form for Adobe's viewers");
    let fields = doc.resolve(acro.get(b"Fields").unwrap()).as_array().unwrap().len();
    assert_eq!(fields, 11, "top-level fields (the radio group counts once)");
    let dr = doc.resolve(acro.get(b"DR").unwrap());
    let fonts = doc.resolve(dr.as_dict().unwrap().get(b"Font").unwrap());
    assert!(fonts.as_dict().unwrap().contains(b"Cour") && fonts.as_dict().unwrap().contains(b"ZaDb"));
    // A field carries its SOM path for data round trips.
    let first = doc.resolve(&doc.resolve(acro.get(b"Fields").unwrap()).as_array().unwrap()[0]);
    assert!(first.as_dict().unwrap().contains(SOM_KEY));
    // Not dynamic twice: a second pass is refused rather than doubling the pages.
    assert!(!is_dynamic(&doc));
}

#[test]
fn documents_without_xfa_are_not_touched() {
    let mut doc = Document::new_empty();
    assert!(!is_dynamic(&doc));
    assert!(matches!(render_into(&mut doc), Err(XfaError::NotXfa)));
}

#[test]
fn hostile_templates_fail_without_panicking() {
    assert!(layout_xml("not xml").is_err());
    assert!(layout_xml("<template/>").is_err());
    assert!(layout_xml("<template><subform/></template>").is_ok(), "an empty form is one empty page");
    // Deep nesting is cut off, not followed.
    let deep = format!("<template><subform layout=\"tb\">{}{}</subform></template>", "<subform layout=\"tb\">".repeat(500), "</subform>".repeat(500));
    assert!(layout_xml(&deep).is_ok());
    // Absurd sizes are clamped; a page can't be a mile wide.
    let huge = r##"<template><subform layout="tb"><pageSet><pageArea><medium short="1e9in" long="-5in"/></pageArea></pageSet><draw w="1e12pt" h="1e12pt"><value><text>x</text></value></draw></subform></template>"##;
    let f = layout_xml(huge).unwrap();
    assert!(f.pages[0].width <= 14_400.0 && f.pages[0].height >= 1.0);
    // Too many pages is an error, not an endless loop.
    let many = format!(
        "<template><subform layout=\"tb\"><pageSet><pageArea><contentArea w=\"100pt\" h=\"100pt\"/><medium short=\"100pt\" long=\"100pt\"/></pageArea></pageSet>{}</subform></template>",
        "<draw h=\"90pt\"><value><text>x</text></value></draw>".repeat(MAX_PAGES + 5)
    );
    assert!(matches!(layout_xml(&many), Err(XfaError::TooLarge(_))));
    // A bad image and a bad base64 are warnings, not failures.
    let img = r##"<template><subform layout="tb"><draw w="1in" h="1in"><value><image contentType="image/png">!!!notbase64</image></value></draw></subform></template>"##;
    let f = layout_xml(img).unwrap();
    assert!(!f.warnings.is_empty());
}

#[test]
fn the_packets_can_be_one_stream_or_named_parts() {
    // Named parts, with the template split from the rest.
    let doc = Document::open(Arc::new(shell(&template(0)))).unwrap();
    let p = read_packets(&doc).unwrap().unwrap();
    assert!(p.needs_rendering && !p.has_fields);
    assert!(p.xdp.contains("<template"));
    // UTF-16 packets are read too.
    let mut utf16 = vec![0xFF, 0xFE];
    for u in "<template><subform/></template>".encode_utf16() {
        utf16.extend_from_slice(&u.to_le_bytes());
    }
    let xdp = String::from_utf8_lossy(&utf16).into_owned();
    let _ = xdp;
    let doc = Document::open(Arc::new(shell_bytes(&utf16))).unwrap();
    let p = read_packets(&doc).unwrap().unwrap();
    assert!(p.xdp.starts_with("<template>"), "{:?}", &p.xdp[..20.min(p.xdp.len())]);
}

/// Like [`shell`], but the packet bytes are given as-is.
fn shell_bytes(packet: &[u8]) -> Vec<u8> {
    let text = shell("PLACEHOLDER");
    let needle = b"<< /Length 11 >>\nstream\nPLACEHOLDER\nendstream";
    let pos = text.windows(needle.len()).position(|w| w == needle).unwrap();
    let mut out = text[..pos].to_vec();
    out.extend_from_slice(format!("<< /Length {} >>\nstream\n", packet.len()).as_bytes());
    out.extend_from_slice(packet);
    out.extend_from_slice(b"\nendstream");
    out.extend_from_slice(&text[pos + needle.len()..]);
    // The xref offsets after the stream are off; the reader reconstructs them.
    out
}

#[test]
fn jpeg_headers_give_the_size() {
    // A minimal SOF0 header: 300 × 200, 3 components.
    let mut j = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    j.extend_from_slice(&[0; 14]);
    j.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0xC8, 0x01, 0x2C, 0x03]);
    assert_eq!(pdf::jpeg_size(&j), Some((300, 200)));
    assert_eq!(pdf::jpeg_size(b"\x89PNG"), None);
    assert_eq!(pdf::jpeg_size(&[0xFF, 0xD8, 0xFF]), None);
}
