use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, Object, SaveOptions, write_incremental};

use super::*;
use crate::data::{
    DatasetsWrite, FieldData, FieldDatum, build_data, iso_to_pattern, parse_datasets, pattern_to_iso, read_values, som_to_path, write_datasets,
};
use crate::data::{add_data_instances, remove_data_instance, write_data_value};
use crate::fixtures::{
    TINY_GIF, bound_date_template, fields_template, scripted_template, shell, static_shell, template, template_with_data, tiny_bmp, tiny_png,
    tiny_tiff, unnamed_hidden_template,
};
use crate::layout::Item;
use crate::layout::WidgetKind;
use crate::model::*;
use crate::script::{NodeKind, Overrides, apply_overrides, fields_by_som, form_tree, overrides, rerender, set_overrides};

/// The widgets laid out on `page`, in order.
fn widgets(page: &Page) -> Vec<&Widget> {
    page.items.iter().filter_map(|i| if let Item::Widget(w) = i { Some(w.as_ref()) } else { None }).collect()
}

/// The text of each text span laid out on `page`, in order.
fn texts(page: &Page) -> Vec<String> {
    page.items.iter().filter_map(|i| if let Item::Text(s) = i { Some(s.text.clone()) } else { None }).collect()
}

/// Measurements in points, inches, millimeters and centimeters convert to points, a bare
/// number is points, and an `em` length, a non-number or a NaN is `None`.
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

/// Two draws of the same text, one at `fontHorizontalScale="80%"`, each 90% as wide as the
/// text is at full width, and a rich-text draw condensed to half by its span style.
fn scaled_text_template() -> (String, &'static str) {
    const TEXT: &str = "Approving authority name and title";
    let w = pdfcraft_fonts::helvetica_width(TEXT, 10.0) * 0.9;
    let xdp = format!(
        r##"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <draw name="narrow" w="{w}pt" h="0.5in"><value><text>{TEXT}</text></value><font typeface="Arial" size="10pt" fontHorizontalScale="80%"/></draw>
 <draw name="plain" w="{w}pt" h="0.5in"><value><text>{TEXT}</text></value><font typeface="Arial" size="10pt"/></draw>
 <draw name="rich" w="7in" h="0.3in"><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p>Full <span style="xfa-font-horizontal-scale:50%">half</span></p></body></exData></value><font typeface="Arial" size="10pt"/></draw>
</subform>
</template></xdp:xdp>"##
    );
    (xdp, TEXT)
}

/// `fontHorizontalScale` is read as a percentage and text is measured at it: the 80% draw
/// fits its box on one line where the full-width one takes two, and a span's own scale
/// applies to that span alone.
#[test]
fn horizontally_scaled_text_is_measured_and_drawn_narrower() {
    let (xdp, text) = scaled_text_template();
    let (tpl, _) = parse(&xdp).unwrap();
    let Node::Draw(narrow) = &tpl.root.children[0] else { panic!() };
    assert_eq!(narrow.common.font.h_scale, Some(0.8), "fontHorizontalScale is a percentage");
    let form = layout_xml(&xdp).unwrap();
    let spans: Vec<&text::Span> = form.pages[0].items.iter().filter_map(|i| if let Item::Text(s) = i { Some(s) } else { None }).collect();
    let lines = |scale: f64, words: &[&str]| {
        let mut b: Vec<i64> = spans
            .iter()
            .filter(|s| s.face.h_scale == scale && words.iter().any(|w| s.text.contains(w)))
            .map(|s| (s.baseline * 100.0).round() as i64)
            .collect();
        b.dedup();
        b.len()
    };
    let words: Vec<&str> = text.split(' ').collect();
    // At 80% the text is 72% of its full width: it fits the 90% box on one line.
    assert_eq!(lines(0.8, &words), 1, "{spans:?}");
    // At full width it doesn't: two lines.
    assert_eq!(lines(1.0, &["Approving", "title"]), 2, "{spans:?}");
    // A span's own scale applies to it alone.
    let half = spans.iter().find(|s| s.text.contains("half")).expect("the half span");
    let full = spans.iter().find(|s| s.text.contains("Full")).expect("the full span");
    assert_eq!((half.face.h_scale, full.face.h_scale), (0.5, 1.0));
    assert!((half.face.width("half") - full.face.width("half") / 2.0).abs() < 1e-9);
}

/// A growing non-interactive field is measured in the face its text is drawn in: condensed,
/// its text takes fewer lines and the field grows only to hold them.
#[test]
fn a_non_interactive_field_is_measured_at_the_scale_its_text_is_drawn() {
    // A growing, non-interactive text field (drawn as text by the layout, not as a widget),
    // then a draw below it. However condensed the field's text is, the draw sits the same
    // distance below its last line: the field is as tall as the lines it shows.
    // Long enough to take more than a multi-line field's three lines at either scale.
    const TEXT: &str = "Remarks that run over several lines of a narrow box, and then over several more lines again, until the box has grown well past the three lines it starts with";
    let gap = |scale: u32| {
        let xdp = format!(
            r##"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <field name="note" w="100pt" access="nonInteractive"><ui><textEdit multiLine="1"/></ui><value><text>{TEXT}</text></value><font typeface="Arial" size="10pt" fontHorizontalScale="{scale}%"/></field>
 <draw name="after" w="100pt" h="20pt"><value><text>AFTER</text></value><font typeface="Arial" size="10pt"/></draw>
</subform>
</template></xdp:xdp>"##
        );
        let form = layout_xml(&xdp).unwrap();
        let spans: Vec<&text::Span> = form.pages[0].items.iter().filter_map(|i| if let Item::Text(s) = i { Some(s) } else { None }).collect();
        let after = spans.iter().find(|s| s.text.contains("AFTER")).expect("the draw").baseline;
        let lines: std::collections::BTreeSet<i64> =
            spans.iter().filter(|s| !s.text.contains("AFTER")).map(|s| (s.baseline * 100.0).round() as i64).collect();
        let last = *lines.iter().next_back().expect("the field's lines") as f64 / 100.0;
        (lines.len(), after - last)
    };
    let (full_lines, full_gap) = gap(100);
    let (narrow_lines, narrow_gap) = gap(60);
    assert!(narrow_lines < full_lines, "condensed, the text takes fewer lines: {narrow_lines} vs {full_lines}");
    assert!((narrow_gap - full_gap).abs() < 0.01, "the field shrinks with its text: gap {narrow_gap} vs {full_gap}");
}

/// Condensed text is written with `Tz` inside a `q`/`Q` pair of its own, and full-width text
/// sets no scale, so none carries over from the text before it.
#[test]
fn horizontally_scaled_text_is_written_with_tz_inside_its_own_graphics_state() {
    let (xdp, _) = scaled_text_template();
    let mut doc = Document::open(Arc::new(shell(&xdp))).unwrap();
    render_into(&mut doc).unwrap();
    let root = doc.get(doc.root().unwrap());
    let pages = doc.resolve(root.as_dict().unwrap().get(b"Pages").unwrap());
    let kid = pages.as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap()[0].clone();
    let page = doc.resolve(&kid);
    let contents = doc.resolve(page.as_dict().unwrap().get(b"Contents").unwrap());
    let Object::Stream(c) = &*contents else { panic!("a content stream") };
    let ops = String::from_utf8_lossy(&c.decoded().unwrap()).into_owned();
    let scaled: Vec<&str> = ops.lines().filter(|l| l.contains(" Tz ")).collect();
    assert!(!scaled.is_empty() && scaled.iter().all(|l| l.starts_with("q BT ") && l.ends_with(" ET Q")), "{ops}");
    assert!(scaled.iter().any(|l| l.contains(" 80 Tz ")) && scaled.iter().any(|l| l.contains(" 50 Tz ")), "{ops}");
    // Full-width text sets no scale, and none is left over from the condensed text before it.
    assert!(ops.lines().filter(|l| l.contains(" Tj ") && !l.contains(" Tz ")).all(|l| l.starts_with("BT ")), "{ops}");
}

/// The sample template parses without warnings into what the layout uses: its page areas,
/// page-number draws, rich-text title, field font, caption and widget border, check box,
/// date field, radio group, table columns and page break all come through.
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
fn rich_text_max_length_reaches_the_field_and_layout() {
    for (attribute, expected) in [
        (r#"maxLength="5250""#, Some(5250)),
        (r#"maxLength=" 10500 ""#, Some(10500)),
        (r#"maxLength="0""#, None),
        (r#"maxLength="-1""#, None),
        (r#"maxLength="invalid""#, None),
        (r#"maxLength="999999999999999999999999999999999999""#, None),
        ("", None),
    ] {
        let xml = template(0).replace(
            r#"<text maxChars="30"/>"#,
            &format!(r#"<exData contentType="text/html" {attribute}><body xmlns="http://www.w3.org/1999/xhtml"><p>Hello</p></body></exData>"#),
        );
        let (tpl, _) = parse(&xml).unwrap();
        let Node::Subform(head) = &tpl.root.children[0] else { panic!() };
        let Node::Field(field) = &head.children[2] else { panic!() };
        assert_eq!(field.max_chars, expected, "{attribute}");
        assert_eq!(field.value.plain().as_deref(), Some("Hello"));
        let form = layout_xml(&xml).unwrap();
        let field = form.pages.iter().flat_map(widgets).find(|w| w.name == "familyName").unwrap();
        assert_eq!(field.max_chars, expected, "{attribute}");
    }
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
    // Each one its own object: Acrobat shows nothing in a text field it redraws after typing
    // when the /DR font is written inline (#3441).
    for (name, font) in fonts.as_dict().unwrap().iter() {
        let r = font.as_ref().unwrap_or_else(|| panic!("{} is an object", String::from_utf8_lossy(name)));
        assert_eq!(doc.get(r).as_dict().and_then(|f| f.name(b"Type")), Some(&b"Font"[..]));
    }
    // A field carries its SOM path for data round trips.
    let first = doc.resolve(&doc.resolve(acro.get(b"Fields").unwrap()).as_array().unwrap()[0]);
    assert!(first.as_dict().unwrap().contains(SOM_KEY));
    // Not dynamic twice: a second pass is refused rather than doubling the pages.
    assert!(!is_dynamic(&doc));
}

/// A dynamic form's own text that the standard fonts can't draw is laid out anyway and reported
/// once, never written as `?` silently.
#[test]
fn form_text_the_standard_fonts_cannot_draw_is_reported() {
    let xdp = r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
<draw w="3in" h="0.3in"><value><text>申请表</text></value></draw><draw w="3in" h="0.3in"><value><text>签名</text></value></draw><draw w="3in" h="0.3in"><value><text>Name</text></value></draw>
</subform></template></xdp:xdp>"##;
    let mut doc = Document::open(Arc::new(shell(xdp))).unwrap();
    let report = render_into(&mut doc).unwrap();
    let hits: Vec<&String> = report.warnings.iter().filter(|w| w.contains("standard fonts can't draw")).collect();
    assert_eq!(hits.len(), 1, "reported once: {:?}", report.warnings);
    assert!(hits[0].contains("\"申\" (U+7533)"), "{hits:?}");
    // Latin text alone reports nothing.
    let latin = xdp.replace("申请表", "Application").replace("签名", "Signature");
    let mut doc = Document::open(Arc::new(shell(&latin))).unwrap();
    assert!(!render_into(&mut doc).unwrap().warnings.iter().any(|w| w.contains("standard fonts")));
}

/// A field whose ui doesn't scroll in the direction its text runs out (`hScrollPolicy="off"` for
/// one line, `vScrollPolicy="off"` for several) is written with DoNotScroll (`/Ff` bit 24), so it
/// takes no more text than it shows; the other direction's policy, or the default, doesn't.
#[test]
fn fields_that_dont_scroll_are_written_with_do_not_scroll() {
    let field = |name: &str, ui: &str| format!(r#"<field name="{name}" w="2in" h="0.4in"><ui>{ui}</ui></field>"#);
    let xdp = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
{}{}{}{}{}{}{}
</subform></template></xdp:xdp>"##,
        field("line", r#"<textEdit hScrollPolicy="off"/>"#),
        field("lines", r#"<textEdit multiLine="1" vScrollPolicy="off"/>"#),
        field("number", r#"<numericEdit hScrollPolicy="off"/>"#),
        field("when", r#"<dateTimeEdit hScrollPolicy="off"/>"#),
        field("linesAcross", r#"<textEdit multiLine="1" hScrollPolicy="off"/>"#),
        field("lineDown", r#"<textEdit vScrollPolicy="off"/>"#),
        field("free", r#"<textEdit/>"#),
    );
    let mut doc = Document::open(Arc::new(shell(&xdp))).unwrap();
    render_into(&mut doc).unwrap();
    let catalog = doc.get(doc.root().unwrap());
    let acro = doc.resolve(catalog.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(acro.as_dict().unwrap().get(b"Fields").unwrap()).as_array().unwrap().clone();
    let mut no_scroll = std::collections::BTreeMap::new();
    for r in fields.iter().filter_map(Object::as_ref) {
        let f = doc.get(r);
        let d = f.as_dict().unwrap();
        let name = d.get(b"T").and_then(Object::as_string).map(|t| t.to_text()).unwrap_or_default();
        no_scroll.insert(name, d.int(b"Ff").unwrap_or(0) & (1 << 23) != 0);
    }
    let expected: std::collections::BTreeMap<String, bool> =
        [("line", true), ("lines", true), ("number", true), ("when", true), ("linesAcross", false), ("lineDown", false), ("free", false)]
            .into_iter()
            .map(|(n, v)| (n.to_string(), v))
            .collect();
    assert_eq!(no_scroll, expected);
}

/// A check box or radio button laid out from a round check button (`shape="round"`) carries
/// `ROUND_KEY`, so the forms layer draws its background and border round; a square one (the
/// default) doesn't.
#[test]
fn round_check_buttons_are_marked_for_a_round_appearance() {
    let check = |name: &str, shape: &str, items: &str| {
        format!(r#"<field name="{name}" w="1in" h="0.3in"><ui><checkButton{shape}/></ui><items>{items}</items></field>"#)
    };
    let on_off = "<integer>1</integer><integer>0</integer>";
    let xdp = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
{}{}<exclGroup name="pick" layout="tb">{}{}</exclGroup>
</subform></template></xdp:xdp>"##,
        check("roundBox", r#" shape="round""#, on_off),
        check("squareBox", "", on_off),
        check("roundRadio", r#" shape="round""#, "<text>a</text>"),
        check("squareRadio", r#" shape="square""#, "<text>b</text>"),
    );
    let mut doc = Document::open(Arc::new(shell(&xdp))).unwrap();
    render_into(&mut doc).unwrap();
    // Every widget among the form's fields and their kids (a radio group's buttons), by the
    // last step of its SOM path.
    let catalog = doc.get(doc.root().unwrap());
    let acro = doc.resolve(catalog.as_dict().unwrap().get(b"AcroForm").unwrap());
    let mut todo: Vec<_> =
        doc.resolve(acro.as_dict().unwrap().get(b"Fields").unwrap()).as_array().unwrap().iter().filter_map(Object::as_ref).collect();
    let mut round = std::collections::BTreeMap::new();
    while let Some(r) = todo.pop() {
        let node = doc.get(r);
        let d = node.as_dict().unwrap();
        if let Some(kids) = d.get(b"Kids").map(|k| doc.resolve(k)) {
            todo.extend(kids.as_array().unwrap().iter().filter_map(Object::as_ref));
            continue;
        }
        let som = d.get(SOM_KEY).and_then(Object::as_string).map(|s| s.to_text()).unwrap_or_default();
        let name = som.rsplit('.').next().unwrap_or_default().split('[').next().unwrap_or_default().to_string();
        round.insert(name, matches!(d.get(crate::pdf::ROUND_KEY), Some(Object::Bool(true))));
    }
    let expected: std::collections::BTreeMap<String, bool> = [("roundBox", true), ("squareBox", false), ("roundRadio", true), ("squareRadio", false)]
        .into_iter()
        .map(|(n, r)| (n.to_string(), r))
        .collect();
    assert_eq!(round, expected);
}

/// A template whose one draw holds `body` as rich text, in a box `w` wide.
fn rich_draw_form(w: &str, body: &str) -> String {
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb">
 <pageSet><pageArea name="page"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <draw name="note" w="{w}"><font typeface="Arial" size="10pt"/><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p>{body}</p></body></exData></value></draw>
</subform></template></xdp:xdp>"##
    )
}

/// The link annotations on the pages of `doc`: each one's URI and rectangle.
fn text_links(doc: &Document) -> Vec<(String, [f64; 4])> {
    let catalog = doc.get(doc.root().unwrap());
    let pages = doc.resolve(catalog.as_dict().unwrap().get(b"Pages").unwrap());
    let kids = doc.resolve(pages.as_dict().unwrap().get(b"Kids").unwrap()).as_array().unwrap().clone();
    let mut out = Vec::new();
    for page in &kids {
        let page = doc.resolve(page);
        let Some(annots) = page.as_dict().unwrap().get(b"Annots").map(|a| doc.resolve(a).as_array().unwrap().clone()) else { continue };
        for a in &annots {
            let a = doc.resolve(a);
            let d = a.as_dict().unwrap();
            if d.name(b"Subtype") != Some(&b"Link"[..]) {
                continue;
            }
            let action = doc.resolve(d.get(b"A").unwrap());
            let uri =
                action.as_dict().unwrap().get(b"URI").and_then(Object::as_string).map(|s| String::from_utf8_lossy(&s.bytes).into_owned()).unwrap();
            let r: Vec<f64> = doc.resolve(d.get(b"Rect").unwrap()).as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
            out.push((uri, [r[0], r[1], r[2], r[3]]));
        }
    }
    out
}

/// An `<a href>` in rich text keeps where it links to on every run of its text, styled or not;
/// the text around it and a named anchor without an address link nowhere.
#[test]
fn rich_text_links_keep_their_address() {
    let (tpl, _) = crate::parse(&rich_draw_form(
        "4in",
        r#"Write to <a href=" mailto:forms@example.com ">forms@<b>example</b>.com</a> or see <a name="top">the top</a>."#,
    ))
    .unwrap();
    let Node::Draw(d) = &tpl.root.children[0] else { panic!("the draw") };
    let Value::Rich(rich) = &d.value else { panic!("rich text") };
    let runs: Vec<(&str, Option<&str>)> = rich.paragraphs.iter().flat_map(|p| &p.runs).map(|r| (r.text.as_str(), r.link.as_deref())).collect();
    assert_eq!(
        runs,
        [
            ("Write to ", None),
            ("forms@", Some("mailto:forms@example.com")),
            ("example", Some("mailto:forms@example.com")),
            (".com", Some("mailto:forms@example.com")),
            (" or see ", None),
            ("the top", None),
            (".", None),
        ]
    );
}

/// A link's address is held once, however many runs, words and lines its text takes: every
/// piece shares it rather than copying it, so a long address over many words stays one string.
#[test]
fn a_links_pieces_share_one_address() {
    let words = "one two three four five six seven eight nine ten ".repeat(8);
    let href = format!("https://example.com/{}", "x".repeat(1500));
    let xdp = rich_draw_form("2in", &format!(r#"<a href="{href}">{words}<b>bold</b> {words}</a>"#));
    let form = layout_xml(&xdp).unwrap();
    let links: Vec<std::sync::Arc<str>> =
        form.pages.iter().flat_map(|p| &p.items).filter_map(|i| if let Item::Text(s) = i { s.link.clone() } else { None }).collect();
    assert!(links.len() > 10, "the link's text takes many lines: {}", links.len());
    assert!(links.iter().all(|l| std::sync::Arc::ptr_eq(l, &links[0]) && **l == *href), "every piece shares one address");
}

/// A link's text becomes a link annotation on the page, one over each line it takes when it
/// wraps, each over its own text; a second layout replaces them rather than adding more.
#[test]
fn links_in_rich_text_become_link_annotations_one_per_line() {
    let body = r#"See <a href="https://example.com/forms/help">the help pages for this form, which explain every block</a> first."#;
    let mut doc = Document::open(Arc::new(shell(&rich_draw_form("2in", body)))).unwrap();
    let report = render_into(&mut doc).unwrap();
    assert!(report.warnings.iter().all(|w| !w.contains("link")), "{:?}", report.warnings);
    let links = text_links(&doc);
    assert!(links.len() >= 2, "the link wraps onto at least two lines: {links:?}");
    assert!(links.iter().all(|(uri, _)| uri == "https://example.com/forms/help"));
    // One per line: each below the one before, all inside the 2-inch box (from x = 36pt).
    for pair in links.windows(2) {
        assert!(pair[1].1[3] <= pair[0].1[1] + 0.01, "a line below the one before: {pair:?}");
    }
    for (_, r) in &links {
        assert!(r[0] >= 36.0 - 0.01 && r[2] <= 36.0 + 144.0 + 0.01 && r[2] > r[0] && r[3] > r[1], "{r:?}");
    }
    // The first line's link starts after "See ", not at the box's edge.
    assert!(links[0].1[0] > 36.0 + 5.0, "{:?}", links[0].1);
    // A second layout replaces the pages, and their links with them.
    let tpl = crate::script::template_of(&doc).unwrap();
    rerender(&mut doc, &tpl).unwrap();
    assert_eq!(text_links(&doc), links, "laid out again: the same links, not twice as many");
    // Saved and opened again, they are still there.
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert_eq!(text_links(&Document::open(Arc::new(bytes)).unwrap()), links);

    // A link whose text changes style partway is one link on its line, over all its text.
    let mut doc =
        Document::open(Arc::new(shell(&rich_draw_form("4in", r#"Write to <a href="mailto:forms@example.com">forms@<b>example</b>.com</a> today."#))))
            .unwrap();
    render_into(&mut doc).unwrap();
    let links = text_links(&doc);
    assert_eq!(links.len(), 1, "{links:?}");
    let arial = Font { typeface: "Arial".into(), size: 10.0, ..Default::default() };
    let plain = crate::text::Face::of(&arial);
    let bold = crate::text::Face::of(&Font { bold: true, ..arial.clone() });
    let width = plain.width("forms@") + bold.width("example") + plain.width(".com");
    let r = links[0].1;
    assert!((r[2] - r[0] - width).abs() < 0.01, "over the whole address: {} against {width}", r[2] - r[0]);
}

/// Two pieces of one link side by side on a line join into one link, but a rule drawn between
/// them keeps them apart, so the link never covers what lies between its pieces; an underline
/// (a level line drawn with its text) doesn't.
#[test]
fn a_rule_between_two_pieces_of_a_link_keeps_them_apart() {
    let piece = |name: &str, x: &str, underline: bool| {
        let u = if underline { ";text-decoration:underline" } else { "" };
        format!(
            r#"<draw name="{name}" x="{x}" y="0in" w="0.4in" h="0.3in"><font typeface="Arial" size="10pt"/><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p style="margin:0{u}"><a href="https://example.com/form">Go</a></p></body></exData></value></draw>"#
        )
    };
    let rule = r#"<draw name="rule" x="0.21in" y="0in" w="0in" h="0.3in"><value><line><edge/></line></value></draw>"#;
    let level_rule = r#"<draw name="dash" x="0.2in" y="0.1in" w="0.04in" h="0in"><value><line><edge/></line></value></draw>"#;
    let form = |between: &str, underline: bool| {
        format!(
            r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="position">
 <pageSet><pageArea name="page"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 {}{between}{}
</subform></template></xdp:xdp>"##,
            piece("first", "0in", underline),
            piece("second", "0.25in", underline)
        )
    };
    let links = |xdp: &str| {
        let mut doc = Document::open(Arc::new(shell(xdp))).unwrap();
        render_into(&mut doc).unwrap();
        text_links(&doc).len()
    };
    assert_eq!(links(&form("", false)), 1, "side by side, the pieces join");
    assert_eq!(links(&form("", true)), 1, "an underline doesn't part them");
    assert_eq!(links(&form(rule, false)), 2, "a rule between them does");
    assert_eq!(links(&form(level_rule, false)), 2, "and so does a level one, which isn't an underline");
}

/// Only web and email addresses become links. Scripts, files, data, relative paths, web
/// addresses without a host, and empty or overlong addresses stay plain text (still drawn), with
/// one warning for the form.
#[test]
fn links_to_anything_but_web_and_email_addresses_stay_plain_text() {
    let long = format!("https://example.com/{}", "a".repeat(2100));
    let body = format!(
        r#"<a href="javascript:app.alert(1)">one</a> <a href="file:///etc/passwd">two</a> <a href="data:text/html,x">three</a> <a href="help.html">four</a> <a href="HTTPS:">five</a> <a href="{long}">six</a> <a href="MailTo:a@example.com">seven</a> <a href="https:///path">eight</a> <a href="https://user@:8080/x">nine</a> <a href="https://\example.com/path">nine and a half</a> <a href="https://user@example.com:8443/x">ten</a>"#
    );
    let mut doc = Document::open(Arc::new(shell(&rich_draw_form("7in", &body)))).unwrap();
    let report = render_into(&mut doc).unwrap();
    assert_eq!(text_links(&doc).into_iter().map(|(u, _)| u).collect::<Vec<_>>(), ["MailTo:a@example.com", "https://user@example.com:8443/x"]);
    let warned: Vec<&String> = report.warnings.iter().filter(|w| w.contains("link")).collect();
    assert_eq!(warned.len(), 1, "one warning for the form: {:?}", report.warnings);
    assert!(warned[0].contains("http, https") && warned[0].contains("mailto"), "{warned:?}");
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
fn repeated_pictures_are_bounded_in_total_bytes() {
    // Each laid-out picture keeps its own copy of the bytes, so one picture in a repeating
    // subform used to multiply without bound and exhaust memory (a failed allocation aborts the
    // app). "AAAA" base64 decodes to zero bytes; 2_097_152 groups is 6 MiB, under the 32 MiB
    // per-picture cap. 50 instances (occur initial, clamped to MAX_INSTANCES) would be ~300 MiB,
    // past the aggregate cap, so layout must stop with TooLarge rather than retain it all.
    let big = "AAAA".repeat(2_097_152);
    let tpl = format!(
        "<template><subform layout=\"tb\"><subform layout=\"tb\"><occur initial=\"50\" max=\"-1\"/>\
         <draw w=\"0.2in\" h=\"0.2in\"><value><image contentType=\"image/png\">{big}</image></value></draw>\
         </subform></subform></template>"
    );
    assert!(matches!(layout_xml(&tpl), Err(XfaError::TooLarge(m)) if m.contains("too large")), "the repeated picture must be refused");

    // A single small picture still lays out: the cap does not reject an ordinary form.
    let small = "AAAA".repeat(8);
    let ok = format!(
        "<template><subform layout=\"tb\"><draw w=\"1in\" h=\"1in\"><value><image contentType=\"image/png\">{small}</image></value></draw></subform></template>"
    );
    assert!(layout_xml(&ok).is_ok(), "one small picture must lay out");
}

#[test]
fn empty_repeating_subforms_spend_the_layout_budget() {
    // Four nested levels of 50 empty instances are 6.25 million instances that place nothing
    // and measure nothing; each one counts against the layout budget.
    let level = "<subform layout=\"tb\"><occur initial=\"50\" max=\"-1\"/>";
    let tpl = format!("<template><subform layout=\"tb\">{}{}</subform></template>", level.repeat(4), "</subform>".repeat(4));
    assert!(matches!(layout_xml(&tpl), Err(XfaError::TooLarge(_))));
    // Three levels (125,000 instances) still lay out.
    let tpl = format!("<template><subform layout=\"tb\">{}{}</subform></template>", level.repeat(3), "</subform>".repeat(3));
    assert!(layout_xml(&tpl).is_ok());
}

#[test]
fn a_packet_stream_listed_many_times_is_read_once() {
    let mut doc = Document::open(Arc::new(shell("<template><subform/></template>"))).unwrap();
    let packet = pdfcraft_cos::ObjRef::new(5, 0);
    let items: Vec<Object> =
        (0..1000).flat_map(|_| [Object::String(pdfcraft_cos::PdfString::literal(b"template".to_vec())), Object::Ref(packet)]).collect();
    let root = doc.root().unwrap();
    doc.update_dict(root, |c| {
        let mut acro = c.get(b"AcroForm").and_then(Object::as_dict).cloned().unwrap();
        acro.set(b"XFA".to_vec(), Object::Array(items));
        c.set(b"AcroForm".to_vec(), Object::Dict(acro));
    })
    .unwrap();
    let p = read_packets(&doc).unwrap().unwrap();
    assert_eq!(p.xdp.matches("<template").count(), 1);
}

#[test]
fn empty_repeating_subforms_drawn_in_a_positioned_root_spend_the_layout_budget() {
    // The same four levels inside a positioned root are drawn, not flowed.
    let level = "<subform layout=\"tb\"><occur initial=\"50\" max=\"-1\"/>";
    let nest =
        |n: usize| format!("<template><subform><subform layout=\"tb\">{}{}</subform></subform></template>", level.repeat(n), "</subform>".repeat(n));
    assert!(matches!(layout_xml(&nest(4)), Err(XfaError::TooLarge(_))));
    assert!(layout_xml(&nest(3)).is_ok());
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

const DATA: &str = "<form><head><familyName>Singh</familyName><agree>1</agree><born>2001-02-03</born><answer>N</answer></head><table><row><from0>2019</from0><what0>Studied</what0><where0>Delhi</where0></row><row><from0>2021</from0><what0>Worked</what0><where0>Toronto</where0></row><row><from0>2023</from0><what0>Moved</what0><where0>Ottawa</where0></row></table></form>";

/// The datasets packet parses into a tree whose nodes the fields' data paths find.
#[test]
fn datasets_parse_into_a_tree_and_som_paths_find_their_nodes() {
    let d = parse_datasets(&template_with_data(1, DATA)).expect("datasets");
    assert_eq!(d.name, "data");
    assert_eq!(d.text_at(&som_to_path("form[0].head[0].familyName[0]")), Some("Singh"));
    assert_eq!(d.text_at(&som_to_path("form[0].#subform[0].head[0].answer[0]")), Some("N"), "unnamed containers have no data node");
    assert_eq!(d.text_at(&som_to_path("form[0].table[0].row[2].where0[0]")), Some("Ottawa"));
    assert_eq!(d.text_at(&som_to_path("form[0].table[0].row[3].where0[0]")), None);
    assert_eq!(d.count(&som_to_path("form[0].table[0]"), "row"), 3);
    assert_eq!(som_to_path("a[1].b"), vec![("a".to_string(), 1), ("b".to_string(), 0)]);
    assert!(parse_datasets("<xdp:xdp xmlns:xdp=\"x\"/>").is_none());
}

/// ISO dates convert to and from the date patterns fields show; text that isn't a date is left
/// alone or refused.
#[test]
fn dates_convert_between_iso_and_acrobat_patterns() {
    assert_eq!(iso_to_pattern("2001-02-03", "yyyy-mm-dd"), "2001-02-03");
    assert_eq!(iso_to_pattern("2001-02-03", "mm/dd/yyyy"), "02/03/2001");
    assert_eq!(iso_to_pattern("2001-02-03", "d.m.yy"), "3.2.01");
    assert_eq!(iso_to_pattern("not a date", "mm/dd/yyyy"), "not a date");
    assert_eq!(pattern_to_iso("02/03/2001", "mm/dd/yyyy").as_deref(), Some("2001-02-03"));
    assert_eq!(pattern_to_iso("3.2.01", "d.m.yy").as_deref(), Some("2001-02-03"));
    assert_eq!(pattern_to_iso("2001-02-03", "yyyy-mm-dd").as_deref(), Some("2001-02-03"));
    assert_eq!(pattern_to_iso("13/40/2001", "mm/dd/yyyy"), None);
    assert_eq!(pattern_to_iso("hello", "mm/dd/yyyy"), None);
}

/// What scripts read as `formattedValue`: a date display picture shows the ISO value in its
/// pattern (the first of several alternatives); other kinds of picture, and values that aren't
/// ISO dates, leave the raw value.
#[test]
fn formatted_values_follow_date_display_pictures() {
    assert_eq!(crate::formatted_value("date{YYYYMMDD}", "2026-10-20").as_deref(), Some("20261020"));
    assert_eq!(crate::formatted_value("date{MM/DD/YYYY}", "2026-10-20").as_deref(), Some("10/20/2026"));
    assert_eq!(crate::formatted_value("date{YYYYMMDD}|date{DDMMMYY}", "2026-10-20").as_deref(), Some("20261020"));
    assert_eq!(crate::formatted_value("date{YYYY-MM-DD}", "2026-10-20"), None, "shown as it is");
    assert_eq!(crate::formatted_value("num{zzz9}", "2026-10-20"), None);
    assert_eq!(crate::formatted_value("date{YYYYMMDD}", "20261020"), None, "not an ISO date");
    assert_eq!(crate::formatted_value("date{YYYYMMDD}", ""), None);
    // Month and weekday names aren't shown yet: the raw value stands rather than `20 mmm 2026`.
    assert_eq!(crate::formatted_value("date{DD MMM YYYY}", "2026-10-20"), None);
    assert_eq!(crate::formatted_value("date{EEE MM/DD/YYYY}", "2026-10-20"), None);
    // Only the ISO date's own shape is formatted.
    assert_eq!(crate::formatted_value("date{YYYYMMDD}", "2026-1-2"), None);
    // Parentheses hold the picture's locale and braces its pattern: one without braces has none.
    assert_eq!(crate::formatted_value("date(en_US){YYYYMMDD}", "2026-10-20").as_deref(), Some("20261020"));
    assert_eq!(crate::formatted_value("date(YYYYMMDD)", "2026-10-20"), None);
}

/// A field's `<format><picture>` is its display picture, kept apart from its `<ui>` edit
/// picture; a field without one has none.
#[test]
fn display_pictures_are_parsed_apart_from_edit_pictures() {
    let (tpl, _) = parse(&crate::fixtures::date_row_template()).unwrap();
    let Node::Subform(page) = &tpl.root.children[0] else { panic!() };
    let Node::Field(from) = &page.children[0] else { panic!() };
    assert_eq!((from.picture.as_deref(), from.display_picture.as_deref()), (Some("date{YYYYMMDD}"), Some("date{YYYYMMDD}")));
    let Node::Field(days) = &page.children[1] else { panic!() };
    assert_eq!(days.display_picture, None);
}

/// A date field's data picture (`<bind><picture>`) is how its value is written in the data:
/// `due` keeps `20261018` there and shows `10/18/2026`. The stored text used to be shown as it
/// was, because the data was taken to hold ISO dates only.
#[test]
fn a_date_is_read_from_the_data_in_its_data_picture() {
    let form = layout_xml(&bound_date_template("<form><page><due>20261018</due><when>2026-10-18</when></page></form>")).unwrap();
    let all: Vec<&Widget> = form.pages.iter().flat_map(widgets).collect();
    let by = |n: &str| all.iter().find(|w| w.name == n).unwrap_or_else(|| panic!("no widget {n}"));
    assert_eq!(by("due").value.as_deref(), Some("10/18/2026"));
    assert_eq!(by("when").value.as_deref(), Some("10/18/2026"), "no data picture: the data holds ISO");
}

/// A SOM path names the template field its segment numbers count to, repeating rows included.
#[test]
fn som_paths_name_the_template_field_their_numbers_count_to() {
    let xml = r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form">
 <field name="x"><bind><picture>first</picture></bind></field>
 <subform name="row"><occur min="1" max="-1"/><field name="d"><bind><picture>row</picture></bind></field></subform>
 <field name="x"><bind><picture>second</picture></bind></field>
 <subform><field name="u"><bind><picture>inner</picture></bind></field></subform>
</subform>
</template></xdp:xdp>"#;
    let (tpl, _) = parse(xml).unwrap();
    let picture = |som: &str| crate::data::template_field(&tpl, som).and_then(|f| f.data_picture.clone());
    // Same-named siblings count up; each SOM number is its own field.
    assert_eq!(picture("form[0].x[0]").as_deref(), Some("first"));
    assert_eq!(picture("form[0].x[1]").as_deref(), Some("second"));
    assert_eq!(picture("form[0].x[2]"), None, "no third x");
    // The instances of a repeating subform follow its number: all are the one template node.
    assert_eq!(picture("form[0].row[0].d[0]").as_deref(), Some("row"));
    assert_eq!(picture("form[0].row[3].d[0]").as_deref(), Some("row"));
    // An unnamed subform is looked through, as data paths do.
    assert_eq!(picture("form[0].u[0]").as_deref(), Some("inner"));
}

/// Refreshing a value in place uses the field's own template field, not a same-named one's.
#[test]
fn a_refreshed_field_takes_its_value_from_its_own_template_field() {
    // Two text fields named `x` with different defaults; the data holds only the first.
    let xml = r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form">
 <field name="x"><ui><textEdit/></ui><value><text>first</text></value></field>
 <field name="x"><ui><textEdit/></ui><value><text>second</text></value></field>
</subform>
</template><xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form><x>a</x></form></xfa:data></xfa:datasets></xdp:xdp>"#;
    let (tpl, _) = parse(xml).unwrap();
    let data = parse_datasets(xml).unwrap();
    let mut live = form_tree(&tpl, Some(&data), &Overrides::default()).root;
    let value = |live: &crate::script::FormNode, som: &str| node_at(live, som).map(|n| n.value.clone());
    assert_eq!(value(&live, "form[0].x[1]").as_deref(), Some("second"));
    assert_eq!(crate::script::refresh_value(&mut live, &tpl, Some(&data), "form[0].x[1]"), Some(false), "nothing to change");
    assert_eq!(value(&live, "form[0].x[1]").as_deref(), Some("second"), "not the first x's default");
}

/// Scripts read a date as the ISO date another viewer gives them as `rawValue`
/// (`2026-10-18`), whatever the field's data picture writes in the data (`20261018`), so a
/// script's `String(rawValue).length == 10` holds. A field without a data picture keeps the
/// data's ISO date, and refreshing a value in place reads it the same way.
#[test]
fn scripts_read_a_date_as_iso_whatever_its_data_picture_writes() {
    let xml = bound_date_template("<form><page><due>20261018</due><when>2026-10-18</when></page></form>");
    let (tpl, _) = parse(&xml).unwrap();
    let data = parse_datasets(&xml).unwrap();
    let mut live = form_tree(&tpl, Some(&data), &Overrides::default()).root;
    let value = |live: &crate::script::FormNode, som: &str| node_at(live, som).map(|n| n.value.clone());
    assert_eq!(value(&live, "form[0].page[0].due[0]").as_deref(), Some("2026-10-18"));
    assert_eq!(value(&live, "form[0].page[0].when[0]").as_deref(), Some("2026-10-18"));
    let later = parse_datasets(&bound_date_template("<form><page><due>20261019</due><when>2026-10-18</when></page></form>")).unwrap();
    assert_eq!(crate::script::refresh_value(&mut live, &tpl, Some(&later), "form[0].page[0].due[0]"), Some(true));
    assert_eq!(value(&live, "form[0].page[0].due[0]").as_deref(), Some("2026-10-19"));
}

/// The node of `live` at `som`.
fn node_at<'a>(live: &'a crate::script::FormNode, som: &str) -> Option<&'a crate::script::FormNode> {
    if live.som == som {
        return Some(live);
    }
    live.children.iter().find_map(|c| node_at(c, som))
}

/// A date its data picture can't hold (2050 in a two-digit year) is written as ISO and reads
/// back as written.
#[test]
fn a_date_its_data_picture_cant_hold_is_written_as_iso() {
    use crate::data::{data_to_iso, iso_to_data};
    assert_eq!(iso_to_data("2026-01-02".into(), Some("yymmdd")), "260102");
    // A two-digit year reads back as 1950 to 2049: 2050 and 1949 keep ISO, and read back as written.
    for iso in ["2050-01-02", "1949-12-31"] {
        assert_eq!(iso_to_data(iso.into(), Some("yymmdd")), iso);
        assert_eq!(data_to_iso(iso, Some("yymmdd")), iso);
    }
}

/// The datasets fill the fields and add the rows of repeating subforms.
#[test]
fn month_and_weekday_names_in_date_patterns() {
    // 2001-02-03 was a Saturday.
    assert_eq!(iso_to_pattern("2001-02-03", "mmm d, yyyy"), "Feb 3, 2001");
    assert_eq!(iso_to_pattern("2001-02-03", "mmmm d, yyyy"), "February 3, 2001");
    assert_eq!(iso_to_pattern("2001-02-03", "ddd, mmm d"), "Sat, Feb 3");
    assert_eq!(iso_to_pattern("2001-02-03", "dddd, mmmm dd, yyyy"), "Saturday, February 03, 2001");
    assert_eq!(pattern_to_iso("Feb 3, 2001", "mmm d, yyyy").as_deref(), Some("2001-02-03"));
    assert_eq!(pattern_to_iso("February 3, 2001", "mmmm d, yyyy").as_deref(), Some("2001-02-03"));
    assert_eq!(pattern_to_iso("Saturday, February 03, 2001", "dddd, mmmm dd, yyyy").as_deref(), Some("2001-02-03"));
    assert_eq!(pattern_to_iso("Smarch 3, 2001", "mmm d, yyyy"), None);
    // A token the converter can't render leaves the value as it is.
    assert_eq!(iso_to_pattern("2001-02-03", "mmmmm d"), "2001-02-03");
    assert_eq!(pattern_to_iso("Feb 3", "mmmmm d"), None);
}

#[test]
fn data_fills_the_fields_and_adds_rows() {
    let form = layout_xml(&template_with_data(1, DATA)).unwrap();
    let all: Vec<&Widget> = form.pages.iter().flat_map(widgets).collect();
    let by = |n: &str| all.iter().find(|w| w.name == n).unwrap_or_else(|| panic!("no widget {n}"));
    assert_eq!(by("familyName").value.as_deref(), Some("Singh"));
    assert_eq!(by("agree").value.as_deref(), Some("1"), "checked by its on value");
    assert_eq!(by("born").value.as_deref(), Some("2001-02-03"));
    let no = all.iter().find(|w| matches!(&w.kind, WidgetKind::Radio { on, .. } if on == "N")).unwrap();
    let yes = all.iter().find(|w| matches!(&w.kind, WidgetKind::Radio { on, .. } if on == "Y")).unwrap();
    assert!(no.value.is_some() && yes.value.is_none(), "the group's data picks the N button");
    // One row in the template, three in the data: three rows laid out, each with its values.
    let wheres: Vec<&str> = all.iter().filter(|w| w.name.starts_with("where0")).filter_map(|w| w.value.as_deref()).collect();
    assert_eq!(wheres, ["Delhi", "Toronto", "Ottawa"]);
    assert_eq!(by("where0_3").som, "form[0].table[0].row[2].where0[0]");
    assert_eq!(by("agree").items, vec!["1".to_string(), "0".to_string()]);
}

/// The AcroForm's terminal fields as the engine would hand them over.
fn field_data(doc: &Document) -> Vec<FieldDatum> {
    let root = doc.get(doc.root().unwrap());
    let acro = doc.resolve(root.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(acro.as_dict().unwrap().get(b"Fields").unwrap()).as_array().unwrap().clone();
    let mut out = Vec::new();
    for f in fields {
        let r = f.as_ref().unwrap();
        let d = doc.get(r);
        let d = d.as_dict().unwrap();
        let name = d.get(b"T").and_then(|t| t.as_string()).map(|s| s.to_text()).unwrap_or_default();
        let v = d.get(b"V").map(|v| doc.resolve(v));
        let data = match d.name(b"FT") {
            Some(b"Tx") => FieldData::Text(v.and_then(|v| v.as_string().map(|s| s.to_text())).unwrap_or_default()),
            Some(b"Btn") if d.int(b"Ff").unwrap_or(0) & (1 << 15) != 0 => {
                FieldData::Radio(v.and_then(|v| v.as_name().map(|n| String::from_utf8_lossy(n).into_owned())).filter(|n| n != "Off"))
            }
            Some(b"Btn") if d.int(b"Ff").unwrap_or(0) & (1 << 16) != 0 => FieldData::None,
            Some(b"Btn") => FieldData::Check(v.and_then(|v| v.as_name().map(|n| n != b"Off")).unwrap_or(false)),
            _ => FieldData::None,
        };
        out.push(FieldDatum { obj: r, name, data });
    }
    out
}

#[test]
fn the_datasets_packet_is_written_from_the_fields_and_read_back() {
    let mut doc = Document::open(Arc::new(shell(&template_with_data(1, DATA)))).unwrap();
    render_into(&mut doc).unwrap();
    // The laid-out fields hold the data's values; writing them back reproduces it.
    let fields = field_data(&doc);
    assert!(fields.iter().any(|f| f.name == "familyName" && f.data == FieldData::Text("Singh".into())), "{fields:?}");
    assert!(fields.iter().any(|f| f.name == "answer" && f.data == FieldData::Radio(Some("N".into()))), "{fields:?}");
    // The data already holds what the fields hold: nothing to write.
    assert_eq!(write_datasets(&mut doc, &fields).unwrap(), DatasetsWrite::default());
    let doc = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let p = read_packets(&doc).unwrap().unwrap();
    let d = parse_datasets(&p.xdp).unwrap();
    assert_eq!(d.text_at(&som_to_path("form[0].head[0].familyName[0]")), Some("Singh"));
    assert_eq!(d.text_at(&som_to_path("form[0].head[0].agree[0]")), Some("1"));
    assert_eq!(d.text_at(&som_to_path("form[0].head[0].answer[0]")), Some("N"));
    assert_eq!(d.text_at(&som_to_path("form[0].table[0].row[2].where0[0]")), Some("Ottawa"));
    assert_eq!(p.xdp.matches("<xfa:datasets").count(), 1, "the packet is replaced, not duplicated");
    // Change a value, write again: the data follows, and reading gives the field its value.
    let mut doc = doc;
    let mut fields = field_data(&doc);
    for f in &mut fields {
        if f.name == "familyName" {
            f.data = FieldData::Text("Kaur".into());
        }
        if f.name == "agree" {
            f.data = FieldData::Check(false);
        }
    }
    let w = write_datasets(&mut doc, &fields).unwrap();
    assert!(w.written && w.warnings.is_empty(), "{w:?}");
    let values = read_values(&doc, &fields);
    assert!(values.contains(&("familyName".to_string(), FieldData::Text("Kaur".into()))), "{values:?}");
    assert!(values.contains(&("agree".to_string(), FieldData::Check(false))), "{values:?}");
    // A form without a datasets packet gets one, before the postamble.
    let mut doc = Document::open(Arc::new(shell(&template(1)))).unwrap();
    render_into(&mut doc).unwrap();
    let fields = field_data(&doc);
    assert!(write_datasets(&mut doc, &fields).unwrap().written);
    let p = read_packets(&doc).unwrap().unwrap();
    assert!(parse_datasets(&p.xdp).is_some());
    // No XFA at all: nothing to write.
    let mut plain = Document::new_empty();
    assert!(!write_datasets(&mut plain, &[]).unwrap().written);
}

/// A radio item whose text contains a space keeps that text in the datasets. The appearance
/// state is a PDF name (`New_York`); writing it back used to clear the selection on the next layout.
#[test]
fn spaced_radio_values_stay_in_the_datasets() {
    for (value, state) in [("New York", "New_York"), ("Y", "Y")] {
        let xdp = format!(
            r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
<pageSet><pageArea name="page"><contentArea x="36pt" y="36pt" w="540pt" h="720pt"/><medium short="612pt" long="792pt"/></pageArea></pageSet>
<exclGroup name="city" layout="lr-tb">
<field name="first" w="108pt" h="18pt"><ui><checkButton/></ui><items><text>{value}</text></items></field>
<field name="second" w="108pt" h="18pt"><ui><checkButton/></ui><items><text>Boston</text></items></field>
</exclGroup>
<field name="note" w="108pt" h="18pt"><ui><textEdit/></ui></field>
<field name="agree" w="108pt" h="18pt"><ui><checkButton/></ui><items><text>{value}</text><text>No</text></items></field>
</subform>
</template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"><xfa:data><form><city>{value}</city><note>before</note><agree>{value}</agree></form></xfa:data></xfa:datasets>
</xdp:xdp>"#
        );
        let mut doc = Document::open(Arc::new(shell(&xdp))).unwrap();
        let report = render_into(&mut doc).unwrap();
        assert!(report.warnings.is_empty(), "{value:?} {:?}", report.warnings);
        let fields = field_data(&doc);
        assert!(fields.iter().any(|f| f.name == "city" && f.data == FieldData::Radio(Some(state.into()))), "{value:?} {fields:?}");
        let unchanged = write_datasets(&mut doc, &fields).unwrap();
        assert!(!unchanged.written && unchanged.warnings.is_empty(), "{value:?} {unchanged:?}");
        let mut fields = fields;
        for f in &mut fields {
            if f.name == "note" {
                f.data = FieldData::Text("after".into());
            }
        }
        let write = write_datasets(&mut doc, &fields).unwrap();
        assert!(write.written && write.warnings.is_empty(), "{value:?} {write:?}");
        let packets = read_packets(&doc).unwrap().unwrap();
        let data = parse_datasets(&packets.xdp).unwrap();
        assert_eq!(data.text_at(&som_to_path("form[0].city[0]")), Some(value));
        assert_eq!(data.text_at(&som_to_path("form[0].agree[0]")), Some(value));
        assert_eq!(data.text_at(&som_to_path("form[0].note[0]")), Some("after"));
        let selected: Vec<(String, Option<String>)> = layout_xml(&packets.xdp)
            .unwrap()
            .pages
            .iter()
            .flat_map(|p| &p.items)
            .filter_map(|item| match item {
                Item::Widget(w) if w.name == "first" || w.name == "second" => Some((w.name.clone(), w.value.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(selected, vec![("first".into(), Some(value.into())), ("second".into(), None)], "{value:?}");
    }
}

#[test]
fn static_forms_read_their_values_from_the_datasets() {
    let doc = Document::open(Arc::new(static_shell("<form1><page1><name>Ada</name><agree>1</agree></page1></form1>"))).unwrap();
    assert!(!is_dynamic(&doc));
    let fields = vec![
        FieldDatum { obj: pdfcraft_cos::ObjRef::new(8, 0), name: "form1[0].page1[0].name[0]".into(), data: FieldData::Text(String::new()) },
        FieldDatum { obj: pdfcraft_cos::ObjRef::new(9, 0), name: "form1[0].page1[0].agree[0]".into(), data: FieldData::Check(false) },
    ];
    let values = read_values(&doc, &fields);
    assert_eq!(
        values,
        vec![
            ("form1[0].page1[0].name[0]".to_string(), FieldData::Text("Ada".into())),
            ("form1[0].page1[0].agree[0]".to_string(), FieldData::Check(true))
        ]
    );
    assert_eq!(build_data(&doc, &fields), "<xfa:data><form1><page1><name></name><agree></agree></page1></form1></xfa:data>");
}

#[test]
fn nested_width_less_subforms_lay_out_in_polynomial_time() {
    // Regression: a container without a width measured its content once for its width and
    // again for its height, at every level: 2^depth measurements. Two children per level make
    // it worse still. This must finish at once, and lay the innermost fields out.
    for depth in [40, 60] {
        let open = "<subform layout=\"lr-tb\"><field name=\"a\" w=\"20pt\" h=\"10pt\"/>".repeat(depth);
        let xml = format!(
            "<template><subform layout=\"tb\" name=\"form\">{open}<field name=\"inner\" w=\"30pt\" h=\"10pt\"/>{}</subform></template>",
            "</subform>".repeat(depth)
        );
        let start = std::time::Instant::now();
        let form = layout_xml(&xml).expect("lays out");
        assert!(start.elapsed() < std::time::Duration::from_secs(20), "took {:?}", start.elapsed());
        let names: Vec<String> = form.pages.iter().flat_map(widgets).map(|w| w.name.clone()).collect();
        assert!(names.iter().any(|n| n == "inner") || depth > 60, "{depth}: {names:?}");
        assert!(names.len() > 1);
    }
}

/// The static fixture's two fields, holding `name` and `agree`.
fn static_fields(name: &str, agree: bool) -> Vec<FieldDatum> {
    vec![
        FieldDatum { obj: pdfcraft_cos::ObjRef::new(8, 0), name: "form1[0].page1[0].name[0]".into(), data: FieldData::Text(name.into()) },
        FieldDatum { obj: pdfcraft_cos::ObjRef::new(9, 0), name: "form1[0].page1[0].agree[0]".into(), data: FieldData::Check(agree) },
    ]
}

/// The decoded XFA stream (object 5 in the static fixture, or whatever `/XFA` now points to).
fn xfa_bytes(doc: &Document) -> Vec<u8> {
    let root = doc.get(doc.root().unwrap());
    let acro = doc.resolve(root.as_dict().unwrap().get(b"AcroForm").unwrap());
    let xfa = doc.resolve(acro.as_dict().unwrap().get(b"XFA").unwrap());
    let Object::Stream(s) = &*xfa else { panic!("a stream") };
    s.decoded_within(1 << 20).unwrap()
}

/// Writing field values into the datasets keeps every data node no field binds to as written,
/// changes the bound values in place and adds a missing one beside them. Writing the same values
/// again changes nothing, and a field bound to structured content is reported, not flattened.
#[test]
fn writing_values_keeps_the_data_no_field_binds_to() {
    // Regression: the data element was rebuilt from the fields alone, dropping everything else.
    let data = concat!(
        "<form1 a=\"1\"><page1><name>Ada</name><notes xmlns:my=\"urn:my\"><my:extra kind='x>y'>keep me</my:extra></notes>",
        "<rich><p>bold <b>text</b></p></rich></page1><deep><a><b><c>deep value</c></b></a></deep></form1>",
        "<other xmlns=\"urn:other\"><x>unbound</x></other><!-- a comment --><empty/>"
    );
    let mut doc = Document::open(Arc::new(static_shell(data))).unwrap();
    let w = write_datasets(&mut doc, &static_fields("Grace & <Co>\r\n", true)).unwrap();
    assert!(w.written && w.warnings.is_empty(), "{w:?}");
    let saved = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let xdp = String::from_utf8(xfa_bytes(&saved)).unwrap();
    // Everything no field binds to is still there, as written.
    for kept in [
        "<form1 a=\"1\">",
        "<notes xmlns:my=\"urn:my\"><my:extra kind='x>y'>keep me</my:extra></notes>",
        "<rich><p>bold <b>text</b></p></rich>",
        "<deep><a><b><c>deep value</c></b></a></deep>",
        "<other xmlns=\"urn:other\"><x>unbound</x></other><!-- a comment --><empty/>",
        "<template xmlns=",
    ] {
        assert!(xdp.contains(kept), "lost {kept:?}: {xdp}");
    }
    // The bound value changed in place; the missing check box node was added beside it.
    assert!(xdp.contains("<name>Grace &amp; &lt;Co&gt;&#xD;\n</name>"), "{xdp}");
    let d = parse_datasets(&xdp).unwrap();
    assert_eq!(d.text_at(&som_to_path("form1[0].page1[0].name[0]")), Some("Grace & <Co>\r\n"));
    assert_eq!(d.text_at(&som_to_path("form1[0].page1[0].agree[0]")), Some("1"));
    assert_eq!(xdp.matches("<page1>").count(), 1, "added into the existing node, not a second one");
    // Writing the same values again changes nothing.
    let mut again = saved;
    assert_eq!(write_datasets(&mut again, &static_fields("Grace & <Co>\r\n", true)).unwrap(), DatasetsWrite::default());
    // A field bound to a node holding structured content is reported, not flattened.
    let mut doc = Document::open(Arc::new(static_shell("<form1><page1><name><p>rich</p></name></page1></form1>"))).unwrap();
    let w = write_datasets(&mut doc, &static_fields("plain", false)).unwrap();
    assert!(w.warnings.iter().any(|m| m.contains("structured content") && m.contains("name[0]")), "{w:?}");
    assert!(String::from_utf8(xfa_bytes(&doc)).unwrap().contains("<name><p>rich</p></name>"));
}

/// A UTF-16 packet in either byte order, with a byte order mark, has its values read exactly and
/// is written back as UTF-16 with its mark. A packet in neither UTF-8 nor UTF-16 is left alone,
/// and that is reported.
#[test]
fn structured_data_whitespace_changes_are_reported_without_flattening() {
    let data = "<form1><page1><name>\n<p>Short</p>\n</name><agree/></page1></form1>";
    for value in ["Short", "Short\n", "\nShort", " Short ", "\tShort\t"] {
        let mut doc = Document::open(Arc::new(static_shell(data))).unwrap();
        let before = xfa_bytes(&doc);
        let result = write_datasets(&mut doc, &static_fields(value, false)).unwrap();
        assert!(!result.written, "{value:?}: {result:?}");
        assert_eq!(xfa_bytes(&doc), before, "structured XML must stay intact");
        if value == "Short" {
            assert!(result.warnings.is_empty(), "existing XML indentation is not a field edit: {result:?}");
        } else {
            assert!(result.warnings.iter().any(|w| w.contains("structured content")), "{value:?}: {result:?}");
        }
    }
}

#[test]
fn utf16_packets_are_read_and_written_back_as_utf16() {
    let mut doc = Document::open(Arc::new(static_shell("<form1><page1><name>Zoë 日本</name><other>ünbound</other></page1></form1>"))).unwrap();
    let text = String::from_utf8(xfa_bytes(&doc)).unwrap();
    for (encoding, bom) in [(Encoding::Utf16Le { bom: true }, &[0xFF, 0xFE][..]), (Encoding::Utf16Be { bom: true }, &[0xFE, 0xFF][..])] {
        let mut doc16 = doc.clone();
        doc16.set(
            pdfcraft_cos::ObjRef::new(5, 0),
            Object::Stream(pdfcraft_cos::Stream::flate(pdfcraft_cos::Dict::new(), &encode_packet(&text, encoding))),
        );
        // Read exactly, not as mojibake.
        let values = read_values(&doc16, &static_fields("", false));
        assert_eq!(values.first(), Some(&("form1[0].page1[0].name[0]".to_string(), FieldData::Text("Zoë 日本".into()))));
        let w = write_datasets(&mut doc16, &static_fields("Łódź", false)).unwrap();
        assert!(w.written && w.warnings.is_empty(), "{w:?}");
        let bytes = xfa_bytes(&doc16);
        assert!(bytes.starts_with(bom), "still UTF-16 with its byte order mark");
        let (back, enc) = decode_packet(&bytes).unwrap();
        assert_eq!(enc, encoding);
        assert!(back.contains("<name>Łódź</name>") && back.contains("<other>ünbound</other>"), "{back}");
    }
    // A packet in neither UTF-8 nor UTF-16 (Latin-1 here) is left alone, and that is reported.
    let latin1: Vec<u8> = text.replace("Zoë 日本", "Zo\u{eb}").chars().map(|c| c as u32 as u8).collect();
    doc.set(pdfcraft_cos::ObjRef::new(5, 0), Object::Stream(pdfcraft_cos::Stream::flate(pdfcraft_cos::Dict::new(), &latin1)));
    let w = write_datasets(&mut doc, &static_fields("x", true)).unwrap();
    assert!(!w.written && !w.warnings.is_empty(), "{w:?}");
    assert_eq!(xfa_bytes(&doc), latin1);
}

/// Hundreds of fields with absurd SOM indices finish within seconds, both writing the datasets and
/// building fresh data, create at most the node budget and report what they could not write. A
/// moderate index still works, creating the empty instances before it.
#[test]
fn huge_som_indices_are_capped_not_built() {
    // Regression: each index created its missing siblings one count at a time (quadratic), with
    // no limit on the total. Many fields with absurd indices must finish at once, reporting
    // what they could not write.
    let mut doc = Document::open(Arc::new(static_shell("<form1/>"))).unwrap();
    let start = std::time::Instant::now();
    let fields: Vec<FieldDatum> = (0..300)
        .map(|i| FieldDatum {
            obj: pdfcraft_cos::ObjRef::new(1000 + i, 0),
            name: format!("form1[0].a[{}].b[99999].c[99999].d[{i}]", 9_000 + i),
            data: FieldData::Text("v".into()),
        })
        .collect();
    let w = write_datasets(&mut doc, &fields).unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(10), "took {:?}", start.elapsed());
    assert!(w.warnings.iter().any(|m| m.contains("repeats too often")), "{w:?}");
    let xdp = String::from_utf8(xfa_bytes(&doc)).unwrap();
    assert!(xdp.matches("<a>").count() <= 20_000, "at most the node budget is created");
    // Building fresh data is capped the same way.
    let start = std::time::Instant::now();
    let data = build_data(&doc, &fields);
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    assert!(data.len() < 2_000_000);
    // And a moderate index still works.
    let mut doc = Document::open(Arc::new(static_shell("<form1><row><v>1</v></row></form1>"))).unwrap();
    let row = |i: usize| FieldDatum {
        obj: pdfcraft_cos::ObjRef::new(500, 0),
        name: format!("form1[0].row[{i}].v[0]"),
        data: FieldData::Text(i.to_string()),
    };
    let w = write_datasets(&mut doc, &[row(0), row(3)]).unwrap();
    assert!(w.written && w.warnings.is_empty(), "{w:?}");
    let d = parse_datasets(&String::from_utf8(xfa_bytes(&doc)).unwrap()).unwrap();
    assert_eq!(d.count(&som_to_path("form1[0]"), "row"), 4);
    assert_eq!(d.text_at(&som_to_path("form1[0].row[3].v[0]")), Some("3"));
    assert!(d.get(&som_to_path("form1[0].row[2]")).is_some(), "the instances in between exist, empty");
}

/// The live form tree carries each object's SOM path, value, presence and access, one instance of
/// a repeating row without data, and the template's event, calculate and validate scripts (FormCalc
/// when no content type is given). Data adds rows and values, and an override changes presence.
#[test]
fn the_form_tree_carries_values_presence_instances_and_the_events() {
    let xml = scripted_template();
    let (tpl, _) = parse(&xml).unwrap();
    let crate::script::LiveForm { root, events, truncated } = form_tree(&tpl, None, &Overrides::default());
    assert!(!truncated);
    assert_eq!((root.name.as_str(), root.som.as_str()), ("form", "form[0]"));
    let page = &root.children[0];
    assert_eq!(page.som, "form[0].page1[0]");
    let by = |n: &str| page.children.iter().find(|c| c.name == n).unwrap_or_else(|| panic!("no {n}"));
    assert_eq!((by("price").value.as_str(), by("price").numeric, by("total").access.as_str()), ("5", true, "readOnly"));
    assert_eq!(by("details").presence, "hidden");
    let table = by("table");
    assert_eq!(table.children.len(), 1, "one row without data");
    assert!(table.children[0].repeatable && table.children[0].occur_max.is_none());
    assert_eq!(table.children[0].children[1].som, "form[0].page1[0].table[0].row[0].amount[0]");
    let ev = |som: &str, act: &str| events.iter().find(|e| e.som == som && e.activity == act).unwrap_or_else(|| panic!("no {act} on {som}"));
    assert!(ev("form[0].page1[0]", "initialize").script.contains("qty.rawValue = 2"));
    assert!(ev("form[0].page1[0].total[0]", "calculate").script.contains("qty.rawValue * price.rawValue"));
    assert_eq!(ev("form[0].page1[0].qty[0]", "validate").message.as_deref(), Some("Quantity must be 100 or less"));
    assert!(!ev("form[0].page1[0].addRow[0]", "click").formcalc);
    assert!(ev("form[0].page1[0].legacy[0]", "click").formcalc, "no contentType means FormCalc");
    // Data adds rows and values; overrides change presence.
    let data = parse_datasets("<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data><form><page1><qty>7</qty><table><row><amount>1</amount></row><row><amount>2</amount></row><row><amount>3</amount></row></table></page1></form></xfa:data></xfa:datasets>").unwrap();
    let mut ov = Overrides::default();
    ov.presence.insert("form[0].page1[0].details[0]".into(), "visible".into());
    let root = form_tree(&tpl, Some(&data), &ov).root;
    let page = &root.children[0];
    assert_eq!(page.children.iter().find(|c| c.name == "qty").unwrap().value, "7");
    assert_eq!(page.children.iter().find(|c| c.name == "table").unwrap().children.len(), 3);
    assert_eq!(page.children.iter().find(|c| c.name == "details").unwrap().presence, "visible");
    assert_eq!(page.children.iter().find(|c| c.name == "details").unwrap().kind, Some(NodeKind::Subform));
}

/// Presence and access overrides are kept on the AcroForm and applied to the template: `note` has
/// no widget while `details` is hidden and gets one once an override shows it and the form is laid
/// out again. Saving and reopening keeps the overrides and one set of fields; storing none removes the key.
#[test]
fn overrides_live_in_the_acroform_and_hide_objects_in_the_layout() {
    let mut doc = Document::open(Arc::new(shell(&scripted_template()))).unwrap();
    render_into(&mut doc).unwrap();
    assert!(overrides(&doc).is_empty());
    let names = |doc: &Document| fields_by_som(doc).into_values().collect::<Vec<_>>();
    assert!(!names(&doc).iter().any(|n| n == "note"), "details is hidden: {:?}", names(&doc));
    let mut ov = Overrides::default();
    ov.presence.insert("form[0].page1[0].details[0]".into(), "visible".into());
    ov.access.insert("form[0].page1[0].qty[0]".into(), "readOnly".into());
    set_overrides(&mut doc, &ov).unwrap();
    assert_eq!(overrides(&doc), ov);
    let (tpl, _) = parse(&scripted_template()).unwrap();
    let laid = apply_overrides(&tpl, &ov);
    let Node::Subform(page) = &laid.root.children[0] else { panic!() };
    let Node::Subform(details) = page.children.iter().find(|c| c.common().name.as_deref() == Some("details")).unwrap() else { panic!() };
    assert_eq!(details.common.presence, Presence::Visible);
    let Node::Field(qty) = page.children.iter().find(|c| c.common().name.as_deref() == Some("qty")).unwrap() else { panic!() };
    assert_eq!(qty.access, Access::ReadOnly);
    // Laying out again shows the note field and keeps the others.
    let report = rerender(&mut doc, &tpl).unwrap();
    let after = names(&doc);
    assert!(after.iter().any(|n| n == "note"), "{after:?}");
    assert!(after.iter().any(|n| n == "qty"));
    assert_eq!(report.fields, after.len());
    // Reopening the written bytes finds one set of fields, not two.
    let doc = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    assert_eq!(names(&doc).len(), after.len());
    assert_eq!(overrides(&doc), ov);
    // Clearing removes the key.
    let mut doc = doc;
    set_overrides(&mut doc, &Overrides::default()).unwrap();
    assert!(overrides(&doc).is_empty());
}

/// Showing a hidden subform through an override leaves its unnamed hidden draw and subform hidden,
/// in the layout and in the live form scripts see: they share the subform's SOM path, so its
/// override is not theirs.
#[test]
fn showing_a_subform_leaves_its_unnamed_hidden_objects_hidden() {
    let (tpl, _) = parse(&unnamed_hidden_template()).unwrap();
    let mut ov = Overrides::default();
    ov.presence.insert("form[0].page1[0].details[0]".into(), "visible".into());
    let form = layout(&apply_overrides(&tpl, &ov), None).unwrap();
    let shown = texts(&form.pages[0]);
    assert!(shown.iter().any(|t| t == "Current caption"), "{shown:?}");
    assert!(!shown.iter().any(|t| t == "Old caption" || t == "Old section"), "{shown:?}");
    assert_eq!(form.fields, 1);
    // Scripts see the same: the hidden draw stays hidden in the live form.
    let root = form_tree(&tpl, None, &ov).root;
    let details = &root.children[0].children[0];
    assert_eq!(details.presence, "visible");
    let presence = |text: &str| details.children.iter().find(|c| c.value == text).map(|c| c.presence.clone());
    assert_eq!(presence("Current caption").as_deref(), Some("visible"));
    assert_eq!(presence("Old caption").as_deref(), Some("hidden"));
    assert_eq!(details.children.iter().find(|c| c.kind == Some(NodeKind::Subform)).map(|c| c.presence.as_str()), Some("hidden"));
}

/// Adding and removing data instances edits the datasets packet, creating it when missing: asking
/// for no more rows than exist, or removing one that is not there, changes nothing, and removing a
/// row moves the later ones up. The layout follows the data, and an absurd count is capped, not built.
#[test]
fn data_instances_are_added_and_removed_in_the_packet() {
    let mut doc = Document::open(Arc::new(shell(&scripted_template()))).unwrap();
    render_into(&mut doc).unwrap();
    let parent = som_to_path("form[0].page1[0].table[0]");
    // No datasets packet yet: one is created with the rows.
    assert!(add_data_instances(&mut doc, &parent, "row", 3).unwrap().written);
    let data = crate::script::data_of(&doc).unwrap();
    assert_eq!(data.count(&parent, "row"), 3);
    // Asking for fewer changes nothing; more adds.
    assert!(!add_data_instances(&mut doc, &parent, "row", 2).unwrap().written);
    assert!(add_data_instances(&mut doc, &parent, "row", 4).unwrap().written);
    let mut p = parent.clone();
    p.push(("row".into(), 1));
    p.push(("amount".into(), 0));
    write_data_value(&mut doc, &p, "42").unwrap();
    let data = crate::script::data_of(&doc).unwrap();
    assert_eq!((data.count(&parent, "row"), data.text_at(&p)), (4, Some("42")));
    // Remove the second row: the third's value moves up.
    let mut second = parent.clone();
    second.push(("row".into(), 1));
    assert!(remove_data_instance(&mut doc, &second).unwrap().written);
    let data = crate::script::data_of(&doc).unwrap();
    assert_eq!(data.count(&parent, "row"), 3);
    assert_eq!(data.text_at(&p), None, "the row holding 42 is gone");
    // Removing what isn't there is nothing.
    let mut tenth = parent.clone();
    tenth.push(("row".into(), 9));
    assert!(!remove_data_instance(&mut doc, &tenth).unwrap().written);
    // The layout follows the data: three rows of fields.
    let (tpl, _) = parse(&scripted_template()).unwrap();
    rerender(&mut doc, &tpl).unwrap();
    let names: Vec<String> = fields_by_som(&doc).into_values().collect();
    assert_eq!(names.iter().filter(|n| n.starts_with("amount")).count(), 3, "{names:?}");
    // Hostile: absurd indices are capped, not built.
    assert!(add_data_instances(&mut doc, &parent, "row", 1_000_000).is_ok());
}

#[test]
fn formcalc_button_idioms_keep_their_native_actions() {
    let xml = scripted_template()
        .replace("<script>$host.messageBox(\"FormCalc\")</script>", "<script>$host.resetData()</script>")
        .replace("xfa.host.messageBox(\"Hello \" + qty.rawValue);", "if (qty.rawValue > 1) { xfa.host.resetData(); }");
    let form = layout_xml(&xml).unwrap();
    let w: Vec<&Widget> = form.pages.iter().flat_map(widgets).collect();
    let by = |n: &str| w.iter().find(|w| w.name == n).unwrap_or_else(|| panic!("no {n}"));
    assert_eq!(by("legacy").action, Some(Action::Reset), "a one-statement FormCalc idiom is a native action");
    assert!(matches!(by("hello").action, Some(Action::Script(_))), "anything else runs as a script: {:?}", by("hello").action);
    assert!(matches!(by("addRow").action, Some(Action::Script(_))));
}

/// Five levels of subforms that each repeat 50 times, with a scripted field at the bottom:
/// 50⁵ instances if nothing stopped the walk.
fn nested_repeats_template() -> String {
    let mut body = r#"<field name="leaf" w="1in" h="0.3in"><ui><textEdit/></ui><calculate><script contentType="application/x-javascript">1</script></calculate></field>"#.to_string();
    for level in (0..5).rev() {
        body = format!(r#"<subform name="s{level}" layout="tb"><occur min="1" max="-1" initial="50"/>{body}</subform>"#);
    }
    format!(
        r#"<?xml version="1.0"?><xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb"><pageSet><pageArea name="p"><contentArea x="0" y="0" w="8in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>{body}</subform></template></xdp:xdp>"#
    )
}

#[test]
fn the_live_form_of_nested_repeating_subforms_stops_at_its_node_budget() {
    fn count(n: &crate::script::FormNode) -> usize {
        1 + n.children.iter().map(count).sum::<usize>()
    }
    let (tpl, _) = parse(&nested_repeats_template()).unwrap();
    assert!(crate::script::has_scripts(&tpl));
    let started = std::time::Instant::now();
    let live = form_tree(&tpl, None, &Overrides::default());
    assert!(live.truncated, "the walk was cut short");
    assert!(count(&live.root) <= crate::script::MAX_FORM_NODES + 1, "{} nodes", count(&live.root));
    assert!(live.events.len() <= 20_000);
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    // A template without scripts has nothing to run.
    let plain_xml = nested_repeats_template().replace(r#"<calculate><script contentType="application/x-javascript">1</script></calculate>"#, "");
    let (plain, _) = parse(&plain_xml).unwrap();
    assert!(!crate::script::has_scripts(&plain));
}

#[test]
fn data_ops_of_one_event_are_one_datasets_rewrite() {
    use crate::data::{DataOp, write_data_ops};
    let mut doc = Document::open(Arc::new(shell(&scripted_template()))).unwrap();
    render_into(&mut doc).unwrap();
    let before = doc.object_numbers().len();
    let ops = vec![
        DataOp::Value { path: som_to_path("form[0].page1[0].qty[0]"), text: "1".into() },
        DataOp::Value { path: som_to_path("form[0].page1[0].qty[0]"), text: "7".into() },
        DataOp::Instances { parent: som_to_path("form[0].page1[0].table[0]"), name: "row".into(), count: 3 },
        DataOp::Value { path: som_to_path("form[0].page1[0].table[0].row[2].amount[0]"), text: "5".into() },
        DataOp::Remove(som_to_path("form[0].page1[0].table[0].row[0]")),
    ];
    let r = write_data_ops(&mut doc, &ops, None).unwrap();
    assert!(r.written && r.stream.is_some(), "{r:?}");
    assert_eq!(doc.object_numbers().len(), before + 1, "one new datasets stream");
    let data = crate::data_of(&doc).unwrap();
    assert_eq!(data.text_at(&som_to_path("form[0].page1[0].qty[0]")), Some("7"), "the last value wins");
    assert_eq!(data.count(&som_to_path("form[0].page1[0].table[0]"), "row"), 2);
    assert_eq!(data.text_at(&som_to_path("form[0].page1[0].table[0].row[1].amount[0]")), Some("5"));
    // The in-memory model agrees with what was written.
    let mut model = DataNode::default();
    model.children.push(DataNode { name: "form".into(), ..Default::default() });
    for op in &ops {
        model.apply(op);
    }
    assert_eq!(model.text_at(&som_to_path("form[0].page1[0].qty[0]")), Some("7"));
    assert_eq!(model.count(&som_to_path("form[0].page1[0].table[0]"), "row"), 2);
    assert_eq!(model.text_at(&som_to_path("form[0].page1[0].table[0].row[1].amount[0]")), Some("5"));
    // A second write for the same event replaces that stream instead of adding another; an
    // empty value clears the node.
    let again = write_data_ops(&mut doc, &[DataOp::Value { path: som_to_path("form[0].page1[0].qty[0]"), text: String::new() }], r.stream).unwrap();
    assert_eq!(again.stream, r.stream);
    assert_eq!(doc.object_numbers().len(), before + 1);
    assert_eq!(crate::data_of(&doc).unwrap().text_at(&som_to_path("form[0].page1[0].qty[0]")), Some(""));
}

#[test]
fn script_buttons_carry_no_javascript_action_for_other_viewers() {
    let mut doc = Document::open(Arc::new(shell(&scripted_template()))).unwrap();
    render_into(&mut doc).unwrap();
    let fields = top_fields(&doc);
    let add_row = fields.iter().find(|(name, _)| name == "addRow").map(|(_, d)| d.clone()).expect("addRow");
    assert!(add_row.contains(crate::CLICK_KEY), "found by its SOM path");
    assert!(add_row.get(b"A").is_none() && add_row.get(b"AA").is_none(), "no XFA source as a PDF JavaScript action");
}

/// The generated fields' dictionaries by name (top-level fields only).
fn top_fields(doc: &Document) -> Vec<(String, pdfcraft_cos::Dict)> {
    let root = doc.root().unwrap();
    let catalog = doc.get(root);
    let acro = doc.resolve(catalog.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(acro.as_dict().unwrap().get(b"Fields").unwrap());
    fields
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| {
            let d = doc.resolve(f).as_dict().cloned()?;
            let t = d.get(b"T").and_then(|t| t.as_string()).map(|t| t.to_text())?;
            Some((t, d))
        })
        .collect()
}

/// Choice lists, a password, a signature and pictures in the template become their kinds of
/// widget, and their AcroForm fields and image XObjects once written.
#[test]
fn choice_lists_signatures_passwords_and_pictures_become_fields_and_images() {
    let form = layout_xml(&fields_template("")).unwrap();
    let p1 = &form.pages[0];
    let w = widgets(p1);
    let country = w.iter().find(|w| w.name == "country").unwrap();
    assert_eq!(
        country.kind,
        WidgetKind::Choice {
            options: vec![("CA".into(), "Canada".into()), ("FR".into(), "France".into()), ("JP".into(), "Japan".into())],
            list_box: false,
            multi: false,
            editable: false,
        }
    );
    assert_eq!(country.value.as_deref(), Some("FR"), "the template default, as a saved value");
    let langs = w.iter().find(|w| w.name == "langs").unwrap();
    assert!(
        matches!(&langs.kind, WidgetKind::Choice { options, list_box: true, multi: true, editable: false } if options.len() == 3 && options[0] == ("English".into(), "English".into()))
    );
    let other = w.iter().find(|w| w.name == "other").unwrap();
    assert!(matches!(&other.kind, WidgetKind::Choice { list_box: false, multi: false, editable: true, .. }));
    assert_eq!(w.iter().find(|w| w.name == "pin").unwrap().kind, WidgetKind::Password);
    assert_eq!(w.iter().find(|w| w.name == "sign").unwrap().kind, WidgetKind::Signature);
    assert_eq!(w.iter().find(|w| w.name == "photo").unwrap().kind, WidgetKind::Image, "an image field is a button that picks a picture");
    assert!(w.iter().all(|w| w.name != "code"), "barcode fields make no widget");
    // The PNG in the image field and the GIF in the draw are pictures, fitted to their boxes.
    let images: Vec<(&Rect, usize)> =
        p1.items.iter().filter_map(|i| if let Item::Image { rect, data, .. } = i { Some((rect, data.len())) } else { None }).collect();
    assert_eq!(images.len(), 2, "{images:?}");
    assert!((images[0].0.w - 72.0).abs() < 0.01 && (images[0].0.h - 72.0).abs() < 0.01, "a square picture fills a 1in square: {:?}", images[0].0);
    assert_eq!(images[1].1, TINY_GIF.len());
    let t = texts(p1);
    assert!(t.iter().any(|s| s == "12345"), "the barcode's value as text: {t:?}");
    assert!(form.warnings.iter().any(|w| w.contains("barcode")), "{:?}", form.warnings);
    assert!(!form.warnings.iter().any(|w| w.contains("can't be chosen")), "pictures can be chosen now: {:?}", form.warnings);

    // Written into the PDF: /Ch fields with their options and flags, a /Sig field, a password
    // field, and Flate images (the PNG with a soft mask).
    let mut doc = Document::open(Arc::new(shell(&fields_template("")))).unwrap();
    let report = render_into(&mut doc).unwrap();
    assert_eq!(report.fields, 7, "country, langs, other, pin, sign, photo, summary");
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let fields = pdfcraft_forms_free_fields(&doc);
    let field = |n: &str| fields.iter().find(|(name, _)| name == n).map(|(_, d)| d.clone()).unwrap_or_else(|| panic!("no field {n}"));
    let country = field("country");
    assert_eq!(country.name(b"FT"), Some(b"Ch".as_slice()));
    assert_eq!(country.int(b"Ff"), Some(1 << 17), "a combo box");
    let opt = doc.resolve(country.get(b"Opt").unwrap()).as_array().unwrap().clone();
    assert_eq!(opt.len(), 3);
    let pair = doc.resolve(&opt[1]).as_array().unwrap().clone();
    assert_eq!(pair[0].as_string().unwrap().to_text(), "FR");
    assert_eq!(pair[1].as_string().unwrap().to_text(), "France");
    assert_eq!(country.get(b"V").and_then(|v| v.as_string()).map(|s| s.to_text()).as_deref(), Some("FR"));
    assert_eq!(country.get(b"DV").and_then(|v| v.as_string()).map(|s| s.to_text()).as_deref(), Some("FR"));
    let langs = field("langs");
    assert_eq!(langs.int(b"Ff"), Some(1 << 21), "a multi-select list box");
    let opt = doc.resolve(langs.get(b"Opt").unwrap()).as_array().unwrap().clone();
    assert_eq!(opt[0].as_string().unwrap().to_text(), "English", "same shown text and value: a plain string");
    assert_eq!(field("other").int(b"Ff"), Some((1 << 17) | (1 << 18)), "an editable combo box");
    assert_eq!(field("pin").int(b"Ff"), Some(1 << 13), "a password field");
    // The signature field is named by its SOM path, under a generated field for each step above it.
    assert!(fields.iter().all(|(name, _)| name != "sign"));
    let named = full_names(&doc);
    let sign = named.iter().find(|(name, _)| name == "form[0].page1[0].sign[0]").map(|(_, d)| d).expect("named by its SOM path");
    assert_eq!(sign.name(b"FT"), Some(b"Sig".as_slice()));
    let streams: Vec<pdfcraft_cos::Stream> = doc
        .object_numbers()
        .into_iter()
        .filter_map(|n| if let Object::Stream(st) = &*doc.get(pdfcraft_cos::ObjRef { num: n, generation: 0 }) { Some(st.clone()) } else { None })
        .filter(|s| s.dict.name(b"Subtype") == Some(b"Image"))
        .collect();
    assert_eq!(streams.len(), 3, "the PNG, its soft mask, the GIF");
    let png = streams.iter().find(|s| s.dict.contains(b"SMask")).expect("the PNG has a soft mask");
    assert_eq!(png.dict.name(b"ColorSpace"), Some(b"DeviceRGB".as_slice()));
    assert_eq!(png.dict.name(b"Filter"), Some(b"FlateDecode".as_slice()));
    assert_eq!(png.decoded().unwrap().len(), 12, "2 × 2 RGB samples");
    let gif = streams.iter().find(|s| s.dict.int(b"Width") == Some(1)).expect("the 1 × 1 GIF");
    assert_eq!(gif.dict.name(b"ColorSpace"), Some(b"DeviceGray".as_slice()), "a white pixel is gray");
    assert_eq!(gif.decoded().unwrap(), vec![255]);
}

/// The terminal field dictionaries by name (one level: this fixture has no hierarchy).
fn pdfcraft_forms_free_fields(doc: &Document) -> Vec<(String, pdfcraft_cos::Dict)> {
    let root = doc.get(doc.root().unwrap());
    let acro = doc.resolve(root.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(acro.as_dict().unwrap().get(b"Fields").unwrap()).as_array().unwrap().clone();
    fields
        .iter()
        .filter_map(|f| {
            let d = doc.resolve(f).as_dict()?.clone();
            let name = d.get(b"T").map(|t| doc.resolve(t)).and_then(|t| t.as_string().map(|s| s.to_text()))?;
            Some((name, d))
        })
        .collect()
}

#[test]
fn choice_values_come_from_the_data_as_saved_values_and_several_for_lists() {
    // Shown text in the data is taken too (an editable list the user typed into saves text).
    let xml = fields_template("<form><page1><country>Japan</country><langs>English\nSpanish</langs><other>typed</other></page1></form>");
    let form = layout_xml(&xml).unwrap();
    let w = widgets(&form.pages[0]);
    assert_eq!(w.iter().find(|w| w.name == "country").unwrap().value.as_deref(), Some("JP"));
    assert_eq!(w.iter().find(|w| w.name == "langs").unwrap().value.as_deref(), Some("English\nSpanish"));
    assert_eq!(w.iter().find(|w| w.name == "other").unwrap().value.as_deref(), Some("typed"));
    let mut doc = Document::open(Arc::new(shell(&xml))).unwrap();
    render_into(&mut doc).unwrap();
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let fields = pdfcraft_forms_free_fields(&doc);
    let langs = &fields.iter().find(|(n, _)| n == "langs").unwrap().1;
    let v = doc.resolve(langs.get(b"V").unwrap()).as_array().unwrap().clone();
    assert_eq!(v.iter().map(|s| s.as_string().unwrap().to_text()).collect::<Vec<_>>(), vec!["English", "Spanish"]);
    let i = doc.resolve(langs.get(b"I").unwrap()).as_array().unwrap().clone();
    assert_eq!(i.iter().map(|o| o.as_int().unwrap()).collect::<Vec<_>>(), vec![0, 2]);
}

#[test]
fn pictures_are_sized_from_their_headers_and_hostile_ones_are_refused_unallocated() {
    use crate::image::{Kind, decode, kind, size};
    assert_eq!(kind(&tiny_png()), Some(Kind::Png));
    assert_eq!(size(&tiny_png()), Some((2, 2)));
    assert_eq!(kind(TINY_GIF), Some(Kind::Gif));
    assert_eq!(size(TINY_GIF), Some((1, 1)));
    assert_eq!(kind(b"BM..."), None);
    assert_eq!(size(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"), None, "a header cut short");
    let decoded = decode(&tiny_png()).unwrap();
    assert_eq!((decoded.width, decoded.height, decoded.color_space), (2, 2, "DeviceRGB"));
    assert_eq!(decoded.samples, vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0]);
    assert_eq!(decoded.alpha, Some(vec![255, 255, 255, 0]));
    assert_eq!(decode(TINY_GIF).unwrap().samples, vec![255]);
    // A PNG claiming 100,000 × 100,000 pixels is refused from its header, before any buffer.
    let mut huge = tiny_png();
    huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
    huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
    assert_eq!(size(&huge), None, "no size for layout: the box is used");
    assert!(decode(&huge).unwrap_err().contains("million pixels"));
    // Truncated and corrupt data are errors, not panics.
    let cut = &tiny_png()[..30];
    assert!(decode(cut).is_err());
    let mut bad = tiny_png();
    for b in bad.iter_mut().skip(40) {
        *b ^= 0x55;
    }
    assert!(decode(&bad).is_err() || decode(&bad).is_ok());
    let mut gif = TINY_GIF.to_vec();
    gif.truncate(12);
    assert!(decode(&gif).is_err());
    // A form whose picture is broken still lays out, with a warning, and keeps the rest.
    let xml = fields_template("")
        .replace("image/gif", "image/gif\" transferEncoding=\"none")
        .replace("<image contentType=\"image/png\">", "<image contentType=\"image/png\">AAAA");
    let mut doc = Document::open(Arc::new(shell(&xml))).unwrap();
    let report = render_into(&mut doc).unwrap();
    assert_eq!(report.fields, 7);
    assert!(report.warnings.iter().any(|w| w.contains("image")), "{:?}", report.warnings);
}

#[test]
fn choice_values_are_saved_values_first_deduplicated_and_defaults_are_saved_values_too() {
    // Designer's "specify item values": shown 0..3, saved 1..4. The data's 1 means the item
    // shown as 0, not the item shown as 1.
    let tpl = fields_template("<form><page1><country>1</country></page1></form>")
        .replace("<text>Canada</text><text>France</text><text>Japan</text>", "<text>0</text><text>1</text><text>2</text><text>3</text>")
        .replace("<text>CA</text><text>FR</text><text>JP</text>", "<text>1</text><text>2</text><text>3</text><text>4</text>")
        .replace("<value><text>FR</text></value></field>", "<value><text>1</text></value></field>");
    let form = layout_xml(&tpl).unwrap();
    let w = widgets(&form.pages[0]);
    assert_eq!(w.iter().find(|w| w.name == "country").unwrap().value.as_deref(), Some("1"));
    let (t, _) = parse(&tpl).unwrap();
    let tree = form_tree(&t, parse_datasets(&tpl).as_ref(), &Overrides::default());
    let node = tree.root.children.iter().find(|n| n.name == "page1").unwrap().children.iter().find(|n| n.name == "country").unwrap();
    assert_eq!(node.value, "1", "scripts read the saved value");
    // Several values: each once, never more than there are items, written as ascending indices;
    // a single-select list takes the first line; defaults are saved values.
    let tpl = fields_template("<form><page1><country>Japan\nCanada</country><langs>Spanish\nEnglish\nEnglish\nSpanish</langs></page1></form>")
        .replace("<value><text>FR</text></value></field>", "<value><text>France</text></value></field>")
        .replace(
            "<items><text>English</text><text>French</text><text>Spanish</text></items>",
            "<items><text>English</text><text>French</text><text>Spanish</text></items><value><text>English\nFrench</text></value>",
        );
    let form = layout_xml(&tpl).unwrap();
    let w = widgets(&form.pages[0]);
    assert_eq!(w.iter().find(|w| w.name == "langs").unwrap().value.as_deref(), Some("Spanish\nEnglish"));
    assert_eq!(w.iter().find(|w| w.name == "country").unwrap().value.as_deref(), Some("JP"));
    assert_eq!(w.iter().find(|w| w.name == "country").unwrap().default.as_deref(), Some("FR"));
    let mut doc = Document::open(Arc::new(shell(&tpl))).unwrap();
    render_into(&mut doc).unwrap();
    let doc = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let fields = pdfcraft_forms_free_fields(&doc);
    let langs = &fields.iter().find(|(n, _)| n == "langs").unwrap().1;
    let i = doc.resolve(langs.get(b"I").unwrap()).as_array().unwrap().clone();
    assert_eq!(i.iter().map(|o| o.as_int().unwrap()).collect::<Vec<_>>(), vec![0, 2]);
    let dv = doc.resolve(langs.get(b"DV").unwrap()).as_array().unwrap().clone();
    assert_eq!(dv.iter().map(|s| s.as_string().unwrap().to_text()).collect::<Vec<_>>(), vec!["English", "French"]);
    let country = &fields.iter().find(|(n, _)| n == "country").unwrap().1;
    assert_eq!(country.get(b"DV").and_then(|v| v.as_string()).map(|s| s.to_text()).as_deref(), Some("FR"));
    assert_eq!(doc.resolve(country.get(b"I").unwrap()).as_array().unwrap().len(), 1);
    // Picks stored as <value> children are read too.
    let tpl = fields_template("<form><page1><langs><value>English</value><value>Spanish</value></langs></page1></form>");
    let form = layout_xml(&tpl).unwrap();
    assert_eq!(widgets(&form.pages[0]).iter().find(|w| w.name == "langs").unwrap().value.as_deref(), Some("English\nSpanish"));
    // More items than the cap: kept to the cap, and said so.
    let many: String = (0..1500).map(|i| format!("<text>i{i}</text>")).collect();
    let tpl = fields_template("").replace("<items><text>A</text><text>B</text></items>", &format!("<items>{many}</items>"));
    let form = layout_xml(&tpl).unwrap();
    let other = widgets(&form.pages[0]).into_iter().find(|w| w.name == "other").unwrap();
    assert!(matches!(&other.kind, WidgetKind::Choice { options, .. } if options.len() == 1000));
    assert!(form.warnings.iter().any(|w| w.contains("other") && w.contains("1000 items")), "{:?}", form.warnings);
}

#[test]
fn pictures_are_embedded_once_across_relayouts_and_bad_data_pictures_are_reported() {
    use crate::image::{decode, size};
    let tpl = fields_template("<form><page1><country>JP</country></page1></form>");
    let mut doc = Document::open(Arc::new(shell(&tpl))).unwrap();
    render_into(&mut doc).unwrap();
    let (t, _) = parse(&tpl).unwrap();
    rerender(&mut doc, &t).unwrap();
    rerender(&mut doc, &t).unwrap();
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let images = doc
        .object_numbers()
        .into_iter()
        .filter(
            |n| matches!(&*doc.get(pdfcraft_cos::ObjRef { num: *n, generation: 0 }), Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image")),
        )
        .count();
    assert_eq!(images, 3, "the PNG, its soft mask and the GIF, however many times the form was laid out");
    // No private map in the catalog: earlier pictures are found on the pages, by their hash.
    let root = doc.get(doc.root().unwrap());
    assert!(!root.as_dict().unwrap().contains(b"PCXfaImages"));
    // Pictures in the data that aren't pictures: a warning naming the field, the template's
    // picture shown instead.
    for (data, what) in [("Qk0AAAAA", "not a JPEG, PNG, GIF, BMP or TIFF"), ("not base64!", "not base64")] {
        let tpl = fields_template(&format!("<form><page1><photo>{data}</photo></page1></form>"));
        let form = layout_xml(&tpl).unwrap();
        assert!(form.warnings.iter().any(|w| w.contains("photo") && w.contains(what)), "{what}: {:?}", form.warnings);
        assert_eq!(form.pages[0].items.iter().filter(|i| matches!(i, Item::Image { .. })).count(), 2);
    }
    // A hostile PNG header: no size for layout (the draw keeps its box), refused at decode.
    let mut tall = tiny_png();
    tall[16..20].copy_from_slice(&1u32.to_be_bytes());
    tall[20..24].copy_from_slice(&4_000_000_000u32.to_be_bytes());
    assert_eq!(size(&tall), None);
    assert!(decode(&tall).unwrap_err().contains("million pixels"));
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&tall);
    let tpl = fields_template("").replace(
        r#"<draw name="logo" w="0.5in" h="0.5in">"#,
        &format!(
            r#"<draw name="tall" w="2in"><value><image contentType="image/png">{b64}</image></value></draw><draw name="logo" w="0.5in" h="0.5in">"#
        ),
    );
    let form = layout_xml(&tpl).unwrap();
    assert_eq!(form.pages.len(), 1, "a picture of unknown size takes its box, not a page");
    // A GIF whose frame is far larger than its 1 × 1 screen is refused before it is allocated.
    let mut gif = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\xff\xff\xff\x00\x00\x00,\x00\x00\x00\x00".to_vec();
    gif.extend_from_slice(&11_000u16.to_le_bytes());
    gif.extend_from_slice(&11_000u16.to_le_bytes());
    gif.extend_from_slice(b"\x00\x02\x02D\x01\x00;");
    let started = std::time::Instant::now();
    assert!(decode(&gif).unwrap_err().contains("million pixels"));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    // A template picture over 1 MiB is read whole (text is cut at 1 MiB, pictures are not).
    let big = "A".repeat(1_600_000);
    let tpl = fields_template("").replace(r#"<image contentType="image/gif">"#, &format!(r#"<image contentType="image/gif">{big}"#));
    let (t, _) = parse(&tpl).unwrap();
    fn find_image(nodes: &[Node]) -> Option<usize> {
        nodes.iter().find_map(|n| match n {
            Node::Draw(d) => match &d.value {
                Value::Image { data, .. } => Some(data.len()),
                _ => None,
            },
            Node::Subform(s) => find_image(&s.children),
            _ => None,
        })
    }
    assert!(find_image(&t.root.children).is_some_and(|n| n > 1_200_000));
}

/// Designer forms often carry BMP and TIFF pictures, declared as `image/bmp` and `image/tif`
/// (the Government of Canada's IMM 5257 and IMM 5710 have both). They were left out.
#[test]
fn bmp_and_tiff_pictures_are_decoded_and_drawn() {
    use crate::image::{Kind, decode, kind, size};
    for palette in [false, true] {
        let bmp = tiny_bmp(palette);
        assert_eq!(kind(&bmp), Some(Kind::Bmp));
        assert_eq!(size(&bmp), Some((2, 1)));
        let d = decode(&bmp).unwrap();
        assert_eq!((d.width, d.height, d.color_space), (2, 1, "DeviceRGB"), "palette: {palette}");
        assert_eq!(d.samples, vec![255, 0, 0, 0, 0, 255], "red then blue, palette: {palette}");
        assert_eq!(d.alpha, None);
    }
    for lzw in [false, true] {
        let tiff = tiny_tiff(lzw);
        assert_eq!(kind(&tiff), Some(Kind::Tiff));
        assert_eq!(size(&tiff), Some((1, 1)));
        let d = decode(&tiff).unwrap();
        assert_eq!((d.width, d.height, d.color_space), (1, 1, "DeviceRGB"), "lzw: {lzw}");
        assert_eq!(d.samples, vec![0x12, 0x34, 0x56], "lzw: {lzw}");
    }
    assert_eq!(kind(b"MM\0*\0\0\0\x08"), Some(Kind::Tiff), "big-endian");
    let mut cut = tiny_tiff(true);
    cut.truncate(60);
    assert!(decode(&cut).is_err());
    // In a form: drawn, with no warning, whatever the declared type says.
    use base64::Engine;
    for (data, declared) in
        [(tiny_bmp(false), "image/bmp"), (tiny_bmp(true), "image/x-ms-bmp"), (tiny_tiff(false), "image/tif"), (tiny_tiff(true), "image/tiff")]
    {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
        let gif = base64::engine::general_purpose::STANDARD.encode(TINY_GIF);
        let xml =
            fields_template("").replace(&format!(r#"<image contentType="image/gif">{gif}"#), &format!(r#"<image contentType="{declared}">{b64}"#));
        assert!(xml.contains(declared));
        let mut doc = Document::open(Arc::new(shell(&xml))).unwrap();
        let report = render_into(&mut doc).unwrap();
        assert!(!report.warnings.iter().any(|w| w.contains("left out")), "{declared}: {:?}", report.warnings);
        let samples: Vec<u8> = if declared.contains("bmp") { vec![255, 0, 0, 0, 0, 255] } else { vec![0x12, 0x34, 0x56] };
        let drawn = doc.object_numbers().into_iter().any(|n| {
            matches!(&*doc.get(pdfcraft_cos::ObjRef { num: n, generation: 0 }), Object::Stream(st)
                if st.dict.name(b"Subtype") == Some(b"Image") && st.decoded().ok().as_ref() == Some(&samples))
        });
        assert!(drawn, "{declared}: its pixels are an image XObject");
    }
}

/// A repeating subform drawn inside positioned content (a top-to-bottom table in an area of a
/// positioned subform) gets its `occur` instances, as in Adobe's viewers: real Designer forms
/// have tables like this whose calculations read every row. Before, only one row was laid
/// out, and the field after the table was placed as if there were one.
#[test]
fn repeating_subforms_inside_positioned_content_get_their_instances() {
    let xdp = r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="page1" w="7.5in" h="8in">
  <area name="box" x="0.5in" y="1in">
   <subform name="wrap" layout="tb" w="4in">
    <subform name="table" layout="tb" w="4in">
     <subform name="row" w="4in" h="0.3in"><occur min="3" max="-1"/>
      <field name="reason" w="4in" h="0.3in"><ui><textEdit/></ui></field>
     </subform>
    </subform>
    <field name="after" w="4in" h="0.3in"><ui><textEdit/></ui></field>
   </subform>
  </area>
 </subform>
</subform>
</template>
</xdp:xdp>
"##;
    let form = layout_xml(xdp).unwrap();
    let all: Vec<&Widget> = form.pages.iter().flat_map(widgets).collect();
    let rows: Vec<&Widget> = all.iter().copied().filter(|w| w.som.ends_with(".reason[0]")).collect();
    let soms: Vec<&str> = rows.iter().map(|w| w.som.as_str()).collect();
    assert_eq!(
        soms,
        [
            "form[0].page1[0].box[0].wrap[0].table[0].row[0].reason[0]",
            "form[0].page1[0].box[0].wrap[0].table[0].row[1].reason[0]",
            "form[0].page1[0].box[0].wrap[0].table[0].row[2].reason[0]",
        ],
        "one widget per instance, each with its own SOM index"
    );
    // The rows are stacked, not drawn on top of each other…
    let step = rows[1].rect.y - rows[0].rect.y;
    assert!(step.abs() > 1.0, "rows overlap: {:?}", rows.iter().map(|w| w.rect).collect::<Vec<_>>());
    assert!(((rows[2].rect.y - rows[1].rect.y) - step).abs() < 0.01, "rows are evenly spaced");
    // …and what follows the table is placed after all of them (the table was measured with
    // its instances, not with one row).
    let after = all.iter().find(|w| w.som.ends_with(".after[0]")).expect("the field after the table");
    assert!(((after.rect.y - rows[2].rect.y) - step).abs() < 0.01, "after: {:?}, last row: {:?}", after.rect, rows[2].rect);
}

/// A positioned subform holding `body`, laid out; its page's items.
fn positioned(body: &str) -> Vec<Item> {
    let xdp = format!(
        r##"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="box" w="7.5in" h="3in">{body}</subform>
</subform>
</template></xdp:xdp>"##
    );
    layout_xml(&xdp).unwrap().pages.into_iter().next().unwrap().items
}

/// A text draw without a width is as wide as its text, so right-aligned text starts at its x; a
/// minimum width wider than the text still wins, and text longer than the room left still wraps.
#[test]
fn a_label_without_a_width_is_as_wide_as_its_text() {
    const TEXT: &str = "PERSONNEL ONLY: from date (YYYYMMDD):";
    let natural = pdfcraft_fonts::helvetica_width(TEXT, 8.0);
    // Right-aligned, with a minimum narrower than the text: the box is as wide as the text and
    // starts where it is placed, so the text does too (not at the far right of the subform).
    let items = positioned(&format!(
        r#"<draw name="a" x="1in" y="0" minW="50pt"><value><text>{TEXT}</text></value><font typeface="Arial" size="8pt"/><para hAlign="right"/></draw>"#
    ));
    let span = items.iter().find_map(|i| if let Item::Text(s) = i { Some(s) } else { None }).expect("the text");
    assert!((span.x - (36.0 + 72.0)).abs() < 0.05, "starts at its x: {}", span.x);
    // With a minimum wider than the text, the box is that wide and the text ends at its right.
    let items = positioned(&format!(
        r#"<draw name="a" x="1in" y="0" minW="{}pt"><value><text>{TEXT}</text></value><font typeface="Arial" size="8pt"/><para hAlign="right"/></draw>"#,
        natural + 40.0
    ));
    let span = items.iter().find_map(|i| if let Item::Text(s) = i { Some(s) } else { None }).expect("the text");
    assert!((span.x + span.face.width(&span.text) - (36.0 + 72.0 + natural + 40.0)).abs() < 0.05, "ends at its right: {}", span.x);
    // Text longer than the room left still wraps at the container's width.
    let long = "word ".repeat(200);
    let items = positioned(&format!(r#"<draw name="a" x="0" y="0"><value><text>{long}</text></value><font typeface="Arial" size="8pt"/></draw>"#));
    let lines: std::collections::BTreeSet<i64> =
        items.iter().filter_map(|i| if let Item::Text(s) = i { Some((s.baseline * 100.0).round() as i64) } else { None }).collect();
    assert!(lines.len() > 1, "wraps: {} line(s)", lines.len());
    // And within the room it has: no line's drawn text runs past the subform's right edge.
    let right = items
        .iter()
        .filter_map(|i| if let Item::Text(s) = i { Some(s.x + s.face.width(s.text.trim_end())) } else { None })
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(right <= 36.0 + 7.5 * 72.0 + 0.05, "the text stays within the available width: {right}");
}

/// A draw without a width and without text (a bordered spacer or rule) keeps the width it has
/// room for: with no text to size it to, it doesn't shrink to its margins.
#[test]
fn an_empty_draw_without_a_width_keeps_the_width_it_has_room_for() {
    let items = positioned(r#"<draw name="rule" x="0" y="0" h="12pt"><value><text></text></value><border><edge thickness="1pt"/></border></draw>"#);
    let widest =
        items.iter().filter_map(|i| if let Item::Line { from, to, .. } = i { Some((to.0 - from.0).abs()) } else { None }).fold(0.0_f64, f64::max);
    assert!((widest - 540.0).abs() < 0.5, "the border spans the 7.5in box: {widest}");
}

/// A button without a width is as wide as its caption, not the subform it sits in, unless its
/// minimum width is wider.
#[test]
fn a_button_without_a_width_is_as_wide_as_its_caption() {
    // No width, a minimum narrower than its caption: as wide as the caption, not the subform.
    let caption = "Open the instructions for this form";
    let items = positioned(&format!(
        r#"<field name="help" x="1in" y="0" minW="40pt" minH="12pt"><ui><button/></ui><caption><value><text>{caption}</text></value><font typeface="Arial" size="8pt"/></caption></field>"#
    ));
    let button = items.iter().find_map(|i| if let Item::Widget(w) = i { Some(w) } else { None }).expect("the button");
    let natural = pdfcraft_fonts::helvetica_width(caption, 8.0);
    assert!(button.rect.w >= natural && button.rect.w < natural + 10.0, "{} for a caption of {natural}", button.rect.w);
    // A minimum wider than the caption wins.
    let items = positioned(
        r#"<field name="help" x="1in" y="0" minW="200pt" minH="12pt"><ui><button/></ui><caption><value><text>Help</text></value><font typeface="Arial" size="8pt"/></caption></field>"#,
    );
    let button = items.iter().find_map(|i| if let Item::Widget(w) = i { Some(w) } else { None }).expect("the button");
    assert!((button.rect.w - 200.0).abs() < 0.05, "{}", button.rect.w);
}

/// A button without a height is as tall as one line of its caption in the caption's own font,
/// with none of a text field's padding, and never shorter than its minimum height.
#[test]
fn a_button_without_a_height_is_as_tall_as_its_caption() {
    // A 10pt field font, an 8pt caption: the button is as tall as its caption's line (no
    // text-field padding), and never below its minimum height.
    let button = |min_h: &str| {
        let items = positioned(&format!(
            r#"<field name="help" x="1in" y="0" w="100pt" minH="{min_h}"><ui><button/></ui><font typeface="Times New Roman" size="10pt"/><caption><value><text>Help</text></value><font typeface="Arial" size="8pt"/></caption></field>"#
        ));
        items.iter().find_map(|i| if let Item::Widget(w) = i { Some(w.rect.h) } else { None }).expect("the button")
    };
    assert!((button("2pt") - text::line_height(8.0)).abs() < 0.05, "{}", button("2pt"));
    assert!((button("10.5pt") - 10.5).abs() < 0.05, "{}", button("10.5pt"));
}

/// A button's widget is its whole field, at the field's position and size: the field's margins
/// pad the caption, not the box.
#[test]
fn a_buttons_box_is_the_whole_field_whatever_its_margins() {
    // The button's box is its field's border: its margins pad the caption, not the box.
    let items = positioned(
        r#"<field name="help" x="1in" y="10pt" w="100pt" h="20pt"><ui><button/></ui><margin topInset="5pt" leftInset="3pt"/><border><edge/></border><caption><value><text>Help</text></value><font typeface="Arial" size="8pt"/></caption></field>"#,
    );
    let button = items.iter().find_map(|i| if let Item::Widget(w) = i { Some(w.rect) } else { None }).expect("the button");
    assert!((button.w - 100.0).abs() < 0.05 && (button.h - 20.0).abs() < 0.05, "{button:?}");
    assert!((button.x - (36.0 + 72.0)).abs() < 0.05, "{button:?}");
}

/// A page area without an `<occur>` repeats for as long as the body needs pages. Before, it was
/// treated like a subform (at most once), so once the first body page was out the layout fell
/// back to the last page area, here one limited to a single use, and drew its content over every
/// body page (and listed a copy of its fields per page).
#[test]
fn page_area_without_occur_serves_every_body_page() {
    let section = |n: u32| {
        format!(
            r#"<subform name="s{n}" layout="tb"><breakBefore targetType="pageArea" startNew="1"/><field name="f{n}" w="2in" h="0.3in"><ui><textEdit/></ui></field></subform>"#
        )
    };
    let xdp = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet>
  <pageArea name="Body"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea>
  <pageArea name="Once"><occur min="1" max="1"/><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/>
   <field name="PrintButton" x="1in" y="1in" w="1in" h="0.3in"><ui><button/></ui></field>
  </pageArea>
 </pageSet>
 {}{}{}{}
</subform>
</template>
</xdp:xdp>
"##,
        section(1),
        section(2),
        section(3),
        section(4)
    );
    let form = layout_xml(&xdp).unwrap();
    let soms: Vec<Vec<String>> = form.pages.iter().map(|p| widgets(p).iter().map(|w| w.som.clone()).collect()).collect();
    let has_button = |page: &Vec<String>| page.iter().any(|s| s.contains("PrintButton"));
    // Four body pages, then the once-only page: its button is on that page alone, not over the body.
    assert_eq!(form.pages.len(), 5, "{soms:?}");
    assert!(soms[..4].iter().all(|p| !has_button(p)), "the single-use page area is not drawn over body pages: {soms:?}");
    assert_eq!(soms[4].iter().filter(|s| s.contains("PrintButton")).count(), 1, "the required page is laid out once: {soms:?}");
}

/// A page area's `occur min` is a page it must have even when the body never reaches it, and a
/// body that already used it does not get another. An area with no `<occur>` needs none.
#[test]
fn page_area_minimum_adds_pages_after_the_body_only_when_unused() {
    let area = |name: &str, occur: &str| {
        format!(
            r#"<pageArea name="{name}">{occur}<contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea>"#
        )
    };
    let pages = |areas: String| {
        let xdp = format!(
            r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb"><pageSet>{areas}</pageSet><field name="f" w="2in" h="0.3in"><ui><textEdit/></ui></field></subform>
</template></xdp:xdp>"##
        );
        layout_xml(&xdp).unwrap().pages.len()
    };
    // The body is one page; the required second master adds one, a third with min 2 adds two.
    assert_eq!(pages(area("A", "") + &area("B", r#"<occur min="1" max="1"/>"#)), 2);
    assert_eq!(pages(area("A", "") + &area("B", r#"<occur min="1" max="1"/>"#) + &area("C", r#"<occur min="2" max="3"/>"#)), 4);
    // An area the body already used (it is first, so page 1 is its own) adds nothing.
    assert_eq!(pages(area("A", r#"<occur min="1" max="1"/>"#)), 1);
    assert_eq!(pages(area("A", r#"<occur min="1" max="1"/>"#) + &area("B", "")), 1);
    // No `<occur>` and `min="0"` need no page.
    assert_eq!(pages(area("A", "") + &area("B", r#"<occur min="0" max="1"/>"#)), 1);
}

/// Areas are named in SOM expressions but are not data scopes; unnamed subforms and areas have
/// no segment at all. What the template doesn't have is kept as it is.
#[test]
fn data_paths_leave_areas_out() {
    let (tpl, _) = parse(&crate::fixtures::leave_template("")).unwrap();
    let path = |som: &str| data_path(&tpl, som).into_iter().map(|(n, i)| format!("{n}[{i}]")).collect::<Vec<_>>().join(".");
    // The area `box` (inside an unnamed subform) is dropped; indexes are kept.
    assert_eq!(path("form[0].page[0].box[0].table[0].row[2].days[0]"), "form[0].page[0].table[0].row[2].days[0]");
    assert_eq!(path("form[0].page[0].box[0].rest[0]"), "form[0].page[0].rest[0]");
    // No area on the way: as som_to_path.
    assert_eq!(path("form[0].page[0]"), "form[0].page[0]");
    // Unknown segments, past a leaf, or a SOM from another template: kept.
    assert_eq!(path("form[0].page[0].box[0].nothing[0].x[1]"), "form[0].page[0].nothing[0].x[1]");
    assert_eq!(path("form[0].page[0].box[0].rest[0].box[0]"), "form[0].page[0].rest[0].box[0]");
    assert_eq!(path("other[0].box[0].rest[0]"), "other[0].box[0].rest[0]");
    assert_eq!(data_path(&tpl, ""), Vec::new());
}

/// A template nested thousands of elements deep must be refused. roxmltree recurses with the
/// nesting depth, so it used to overflow the stack - and a stack overflow aborts the process,
/// which no `catch_unwind` can intercept.
#[test]
fn deeply_nested_template_is_refused_instead_of_overflowing_the_stack() {
    let mut xml = String::from(r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">"#);
    for i in 0..10_000 {
        xml.push_str(&format!(r#"<subform name="s{i}">"#));
    }
    xml.push_str(&"</subform>".repeat(10_000));
    xml.push_str("</template>");
    assert!(matches!(crate::parse::parse(&xml), Err(crate::XfaError::Malformed(_))), "ten thousand nested subforms must be refused, not parsed");
}

/// The depth scanner skips what is not an element and cannot under-count one.
#[test]
fn nesting_depth_scanner_counts_what_matters() {
    use crate::xml_nesting_depth as d;
    assert_eq!(d(""), 0);
    assert_eq!(d("<a><b><c/></b></a>"), 3);
    assert_eq!(d(r#"<?xml version="1.0"?><!-- <x> --><a/>"#), 1);
    assert_eq!(d("<a><![CDATA[<not-an-element>]]></a>"), 1);
    assert_eq!(d(r#"<a b="<"><c/></a>"#), 2);
    assert_eq!(d("<a/>"), 1);
    assert_eq!(d("<a><b/></a>"), 2);
    assert_eq!(d(&"<empty/>".repeat(2_000)), 1, "elements that close themselves must not accumulate");
}

/// A template holding thousands of self-closing elements is two levels deep, not thousands. Before
/// the scanner knew that, a form with a lot of styling elements was refused as too deeply nested.
#[test]
fn many_self_closing_elements_do_not_accumulate_into_the_bound() {
    let mut xml = String::from(r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform layout="tb">"#);
    for i in 0..2_000 {
        xml.push_str(&format!(r#"<draw name="d{i}"/>"#));
    }
    xml.push_str("</subform></template>");
    assert!(crate::parse::parse(&xml).is_ok(), "two thousand self-closing draws are two levels deep, not two thousand");
}

/// The bound must hold on a worker thread too, whose default stack (2 MiB) is a quarter of the
/// main thread's: a template exactly at the limit parses there without overflowing.
#[test]
fn template_at_the_depth_limit_parses_on_a_worker_thread() {
    let depth = crate::MAX_XML_DEPTH - 1;
    let mut xml = String::from(r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">"#);
    for i in 0..depth {
        xml.push_str(&format!(r#"<subform name="s{i}">"#));
    }
    xml.push_str(&"</subform>".repeat(depth));
    xml.push_str("</template>");
    let parsed = std::thread::spawn(move || crate::parse::parse(&xml).is_ok()).join();
    assert!(matches!(parsed, Ok(true)), "a template at the limit parses on a default-stack thread");
}

/// A form of one page area (7.5in × 10in of content from 0.5in) around `body`.
fn one_area_form(root_attrs: &str, body: &str) -> String {
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" {root_attrs}>
 <pageSet><pageArea name="page"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
{body}
</subform>
</template>
</xdp:xdp>
"##
    )
}

/// Every field is inside the content area of its page, and the fields come in template order.
fn assert_paginated(form: &Form, fields: usize, min_pages: usize) {
    assert!(form.pages.len() >= min_pages, "{} pages", form.pages.len());
    let mut names = Vec::new();
    for (i, p) in form.pages.iter().enumerate() {
        for w in widgets(p) {
            assert!(
                w.rect.y >= 36.0 - 0.01 && w.rect.bottom() <= 36.0 + 720.0 + 0.01,
                "page {}: {} is outside the content area: {:?}",
                i + 1,
                w.name,
                w.rect
            );
            names.push(w.som.clone());
        }
    }
    assert_eq!(names.len(), fields, "{names:?}");
    let expected: Vec<String> = (0..fields).map(|i| format!("f{i}[0]")).collect();
    for (n, e) in names.iter().zip(&expected) {
        assert!(n.ends_with(e), "{n} where {e} belongs: {names:?}");
    }
}

#[test]
fn a_positioned_subform_taller_than_a_page_continues_on_the_next_pages() {
    // 30 fields 0.9in apart: 27in of positioned content, where a content area holds 10in.
    let fields: String =
        (0..30).map(|i| format!(r#"<field name="f{i}" x="0.5in" y="{}in" w="4in" h="0.4in"><ui><textEdit/></ui></field>"#, i as f64 * 0.9)).collect();
    // As the only child of a flowed root …
    let form = layout_xml(&one_area_form(
        r#"layout="tb""#,
        &format!(r#"<subform name="body"><border><fill><color value="200,200,200"/></fill></border>{fields}</subform>"#),
    ))
    .unwrap();
    assert_paginated(&form, 30, 3);
    // … its fill is cut at the breaks, a piece on every page, none past the content area.
    for (i, p) in form.pages.iter().enumerate() {
        let fills: Vec<&Rect> = p.items.iter().filter_map(|it| if let Item::Fill { rect, .. } = it { Some(rect) } else { None }).collect();
        assert!(!fills.is_empty(), "page {}: no fill", i + 1);
        for r in fills {
            assert!(r.bottom() <= 36.0 + 720.0 + 0.01, "page {}: fill {r:?}", i + 1);
        }
    }
    // … after other content on the page …
    let form = layout_xml(&one_area_form(
        r#"layout="tb""#,
        &format!(r#"<draw name="head" w="4in" h="3in"><value><text>Head</text></value></draw><subform name="body">{fields}</subform>"#),
    ))
    .unwrap();
    assert_paginated(&form, 30, 3);
    assert!(!widgets(&form.pages[0]).is_empty(), "the block starts on the page the heading is on");
    // … and as the root itself (positioned is the default layout).
    let form = layout_xml(&one_area_form("", &fields)).unwrap();
    assert_paginated(&form, 30, 3);
}

#[test]
fn wrapped_and_fixed_height_blocks_taller_than_a_page_are_split_too() {
    let fields: String = (0..60).map(|i| format!(r#"<field name="f{i}" w="3.5in" h="0.8in"><ui><textEdit/></ui></field>"#)).collect();
    // Left-to-right wrapping: 30 lines of 0.8in.
    let form = layout_xml(&one_area_form(r#"layout="tb""#, &format!(r#"<subform name="body" layout="lr-tb" w="7.5in">{fields}</subform>"#))).unwrap();
    assert_paginated(&form, 60, 3);
    // Top-to-bottom with a fixed height the content outgrows.
    let form =
        layout_xml(&one_area_form(r#"layout="tb""#, &format!(r#"<subform name="body" layout="tb" w="7.5in" h="9in">{fields}</subform>"#))).unwrap();
    assert_paginated(&form, 60, 5);
}

#[test]
fn a_block_that_fits_a_page_is_still_moved_whole() {
    let block = |n: usize| {
        format!(r#"<subform name="b{n}" w="7in" h="6in"><field name="f{n}" x="0in" y="5in" w="4in" h="0.4in"><ui><textEdit/></ui></field></subform>"#)
    };
    let form = layout_xml(&one_area_form(r#"layout="tb""#, &format!("{}{}{}", block(0), block(1), block(2)))).unwrap();
    assert_eq!(form.pages.len(), 3);
    for p in &form.pages {
        let w = widgets(p);
        assert_eq!(w.len(), 1);
        assert!((w[0].rect.y - (36.0 + 360.0)).abs() < 0.01, "{:?}", w[0].rect);
    }
}

#[test]
fn a_field_taller_than_a_page_is_cut_through_without_looping() {
    let body = r#"<field name="f0" w="4in" h="25in"><ui><textEdit/></ui></field><field name="f1" w="4in" h="0.4in"><ui><textEdit/></ui></field>"#;
    let form = layout_xml(&one_area_form(r#"layout="tb""#, body)).unwrap();
    let all: Vec<&Widget> = form.pages.iter().flat_map(widgets).collect();
    assert_eq!(all.len(), 2);
    assert!(form.pages.len() <= 4, "{} pages", form.pages.len());
}

/// A form built the way Designer builds a flowed one: everything in one left-to-right wrapped
/// body, sections of a heading over repeating rows of fields, and a text field that grows.
fn wrapped_form(data: &str) -> String {
    let body = r#"<subform name="body" layout="lr-tb" w="7.5in">
  <subform name="section" layout="lr-tb" w="7.5in" minH="0.5in">
   <draw name="heading" w="7.5in" h="0.2in"><value><text>Entries</text></value></draw>
   <subform name="row" layout="lr-tb" w="7.5in" minH="0.5in"><occur max="-1"/>
    <field name="what" w="3.5in" h="1in"><ui><textEdit multiLine="1"/></ui></field>
    <field name="when" w="0.5in" h="1in"><ui><textEdit/></ui></field>
    <field name="why" w="3.5in" h="1in"><ui><textEdit multiLine="1"/></ui></field>
   </subform>
  </subform>
  <field name="notes" w="7.5in" minH="1.5in"><ui><textEdit multiLine="1"/></ui></field>
  <field name="after" w="7.5in" h="0.3in"><ui><textEdit/></ui></field>
 </subform>"#;
    let form = one_area_form(r#"layout="tb""#, body);
    let packet =
        format!("<xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data>{data}</xfa:data></xfa:datasets>\n</xdp:xdp>");
    form.replace("</xdp:xdp>", &packet)
}

#[test]
fn repeating_rows_in_a_wrapped_container_get_the_instances_the_data_holds() {
    let rows: String = (0..4).map(|i| format!("<row><what>what {i}</what><when>{i}</when><why>why {i}</why></row>")).collect();
    let form = layout_xml(&wrapped_form(&format!("<form><body><section>{rows}</section><notes>n</notes></body></form>"))).unwrap();
    let all: Vec<&Widget> = form.pages.iter().flat_map(widgets).collect();
    let whats: Vec<(&str, Option<&str>, f64)> =
        all.iter().filter(|w| w.som.ends_with(".what[0]")).map(|w| (w.som.as_str(), w.value.as_deref(), w.rect.y)).collect();
    assert_eq!(whats.len(), 4, "{whats:?}");
    for (i, (som, value, _)) in whats.iter().enumerate() {
        assert_eq!(*som, format!("form[0].body[0].section[0].row[{i}].what[0]"));
        assert_eq!(*value, Some(format!("what {i}").as_str()));
    }
    // One under another, the section growing with them: what follows comes after the last.
    assert!(whats.windows(2).all(|p| (p[1].2 - p[0].2 - 72.0).abs() < 0.01), "{whats:?}");
    let notes = all.iter().find(|w| w.name == "notes").unwrap();
    assert!(notes.rect.y >= whats[3].2 + 72.0 - 0.01, "{:?} after {whats:?}", notes.rect);
    // Without data a row is there once.
    let form = layout_xml(&wrapped_form("<form/>")).unwrap();
    assert_eq!(form.pages.iter().flat_map(widgets).filter(|w| w.name.starts_with("what")).count(), 1);
}

/// A multi-line field with a minimum height and no fixed one grows to hold its text, moving
/// what follows down; one with a fixed height keeps it.
#[test]
fn a_multi_line_field_without_a_height_grows_to_hold_its_text() {
    let place = |data: &str| {
        let form = layout_xml(&wrapped_form(data)).unwrap();
        let on = |name: &str| {
            form.pages.iter().enumerate().find_map(|(i, p)| widgets(p).into_iter().find(|w| w.name == name).map(|w| (i, w.rect))).unwrap()
        };
        (on("notes"), on("after"))
    };
    // Little text: the minimum height.
    let ((_, notes), (_, after)) = place("<form><body><notes>Short.</notes></body></form>");
    assert!((notes.h - 108.0).abs() < 0.01, "{notes:?}");
    assert!((after.y - notes.bottom()).abs() < 0.01);
    // Forty lines: the field holds them, and what follows moves down.
    let long: String = (0..40).map(|i| format!("Line {i} of the notes.\n")).collect();
    let ((notes_page, grown), (after_page, after)) = place(&format!("<form><body><notes>{long}</notes></body></form>"));
    assert!(grown.h > 40.0 * 11.0 && grown.h < 720.0, "{grown:?}");
    assert_eq!(notes_page, after_page);
    assert!((after.y - grown.bottom()).abs() < 0.01, "{after:?} after {grown:?}");
    // A fixed height stays what it is.
    let fixed = wrapped_form(&format!("<form><body><notes>{long}</notes></body></form>"))
        .replace(r#"minH="1.5in"><ui><textEdit multiLine"#, r#"h="1.5in"><ui><textEdit multiLine"#);
    let form = layout_xml(&fixed).unwrap();
    let notes = form.pages.iter().flat_map(widgets).find(|w| w.name == "notes").unwrap();
    assert!((notes.rect.h - 108.0).abs() < 0.01, "{:?}", notes.rect);
}

/// The rounded boxes laid out on `page`, in order.
fn rounded(page: &Page) -> Vec<&crate::layout::Rounded> {
    page.items.iter().filter_map(|i| if let Item::Rounded(r) = i { Some(r.as_ref()) } else { None }).collect()
}

/// A border or rectangle with rounded corners is one shape, filled and edged along its curves:
/// one corner applies to all four, a corner not given repeats the last one given, a radius is at
/// most half the box's shorter side, and inverted corners are kept. Hidden corners, and edges
/// that differ from each other, keep the square box.
#[test]
fn rounded_corners_make_a_border_one_rounded_shape() {
    let draw = |name: &str, y: f64, border: &str| {
        format!(r#"<draw name="{name}" x="0in" y="{y}in" w="2in" h="1in"><value><text>{name}</text></value>{border}</draw>"#)
    };
    let body = [
        draw("round", 0.0, r#"<border><edge thickness="1pt"><color value="255,0,0"/></edge><corner radius="0.25in"/><fill><color value="230,230,230"/></fill></border>"#),
        draw("mixed", 1.2, r#"<border><edge/><corner radius="0.1in"/><corner radius="0.2in" inverted="1"/></border>"#),
        draw("huge", 2.4, r#"<border><edge/><corner radius="3in"/></border>"#),
        draw("hiddenCorner", 3.6, r#"<border><edge/><corner radius="0.25in" presence="hidden"/></border>"#),
        draw("unlikeEdges", 4.8, r#"<border><edge/><edge presence="hidden"/><corner radius="0.25in"/></border>"#),
        draw("fillOnly", 6.0, r#"<border><edge presence="hidden"/><corner radius="0.25in"/><fill><color value="200,200,200"/></fill></border>"#),
        r#"<draw name="shape" x="0in" y="7.2in" w="2in" h="1in"><value><rectangle><edge thickness="0.5pt"/><corner radius="5.08mm"/></rectangle></value></draw>"#.to_string(),
    ]
    .concat();
    let form = layout_xml(&one_area_form(r#"layout="tb""#, &format!(r#"<subform name="body" layout="position" w="7.5in" h="9in">{body}</subform>"#)))
        .unwrap();
    let shapes = rounded(&form.pages[0]);
    let at = |y: f64| shapes.iter().copied().find(|r| (r.rect.y - 36.0 - y * 72.0).abs() < 0.01);
    let round = at(0.0).expect("the rounded border is one shape");
    assert_eq!((round.rect.w, round.rect.h, round.radii), (144.0, 72.0, [18.0; 4]));
    assert_eq!((round.fill, round.edge), (Some([230.0 / 255.0; 3]), Some((1.0, [1.0, 0.0, 0.0], false))));
    let mixed = at(1.2).expect("the box with two corners given");
    assert_eq!((mixed.radii, mixed.inverted), ([7.2, 14.4, 14.4, 14.4], [false, true, true, true]));
    assert_eq!(at(2.4).expect("the huge radius").radii, [36.0; 4], "at most half the shorter side");
    assert!(at(3.6).is_none() && at(4.8).is_none(), "hidden corners and unlike edges keep a square box");
    let fill_only = at(6.0).expect("a fill with hidden edges is rounded too");
    assert_eq!((fill_only.fill.is_some(), fill_only.edge), (true, None));
    let shape = at(7.2).expect("a rectangle drawn as a value");
    assert!((shape.radii[0] - 5.08 * 72.0 / 25.4).abs() < 1e-9 && shape.edge.is_some());
    // The square boxes are drawn as before: edges as lines.
    let lines_at = |y: f64| {
        form.pages[0]
            .items
            .iter()
            .filter(|i| matches!(i, Item::Line { from, .. } if from.1 >= 36.0 + y * 72.0 - 1.0 && from.1 <= 36.0 + (y + 1.0) * 72.0 + 1.0))
            .count()
    };
    assert_eq!((lines_at(3.6), lines_at(4.8)), (4, 2));
}

/// A rounded box a page break cuts keeps its rounded corners on each page, square where it was
/// cut, and its edge stops there instead of closing across the cut.
#[test]
fn a_rounded_box_cut_by_a_page_break_keeps_its_corners_on_each_page() {
    // A 12-inch box in a 10-inch content area: it is cut at the area's bottom.
    let body = r#"<subform name="body" layout="tb" w="7.5in"><subform name="tall" layout="position" w="7.5in" h="12in"><border><edge/><corner radius="0.25in"/></border><draw name="note" x="0in" y="0in" w="2in" h="0.3in"><value><text>Note</text></value></draw></subform></subform>"#;
    let form = layout_xml(&one_area_form(r#"layout="tb""#, body)).unwrap();
    assert_eq!(form.pages.len(), 2);
    let (first, last) = (rounded(&form.pages[0]), rounded(&form.pages[1]));
    assert_eq!((first.len(), last.len()), (1, 1), "{first:?} {last:?}");
    assert_eq!((first[0].radii, first[0].open_top, first[0].open_bottom), ([18.0, 18.0, 0.0, 0.0], false, true));
    assert_eq!((last[0].radii, last[0].open_top, last[0].open_bottom), ([0.0, 0.0, 18.0, 18.0], true, false));
    assert!((first[0].rect.h + last[0].rect.h - 12.0 * 72.0).abs() < 0.01, "the two parts make the whole box");
}

/// A rounded box is written as one path: filled along its curves, then edged half its width
/// inside, the curves as quarter circles. An inverted corner curves into the box, and the edge
/// of a box a page break cut doesn't close across the cut.
#[test]
fn rounded_boxes_are_written_as_curved_paths() {
    use crate::layout::Rounded;
    // A 2-by-1-inch box at the page's top left, on a page 792pt tall.
    let shape = |inverted: [bool; 4], edge: bool, open: (bool, bool)| Rounded {
        rect: Rect::new(36.0, 36.0, 144.0, 72.0),
        radii: [18.0; 4],
        inverted,
        fill: Some([0.9, 0.9, 0.9]),
        edge: edge.then_some((2.0, [1.0, 0.0, 0.0], false)),
        open_top: open.0,
        open_bottom: open.1,
    };
    let content = |r: Rounded| crate::pdf::rounded(&r, 792.0);
    let ops = content(shape([false; 4], true, (false, false)));
    // The fill: from the top-left corner's start, a quarter circle to (54, 36), closed.
    assert!(ops.contains("0.9 0.9 0.9 rg 36 738 m 36 747.941 44.059 756 54 756 c "), "{ops}");
    assert!(ops.contains(" h f\n"), "{ops}");
    // The edge: 1pt inside, the corners 1pt tighter.
    assert!(ops.contains("q 1 0 0 RG 2 w 37 738 m 37 747.389 44.611 755 54 755 c "), "{ops}");
    assert!(ops.contains(" h S Q\n"), "{ops}");
    // Inverted: the curve bends towards the box's inside.
    let inv = content(shape([true; 4], false, (false, false)));
    assert!(inv.contains("36 738 m 45.941 738 54 746.059 54 756 c "), "{inv}");
    // Cut at the bottom: the edge runs from the bottom left round the top and back down, open.
    let cut = content(Rounded { radii: [18.0, 18.0, 0.0, 0.0], ..shape([false; 4], true, (false, true)) });
    let edge = cut.lines().find(|l| l.contains(" RG ")).unwrap_or_default();
    assert!(edge.starts_with("q 1 0 0 RG 2 w 37 685 m 37 738 l ") && edge.ends_with(" 179 685 l S Q") && !edge.contains(" h "), "{edge}");
}

/// A flowed subform without a fixed height gets its border and fill round all it takes, margins
/// included, drawn under its content; a flowed table too. One a page break cuts between its
/// children gets a closed border round its part on each page, its bottom margin ending the first
/// part and its top margin starting the next, as another viewer draws it on a synthetic form. One
/// holding an item that doesn't fit on its page keeps no border, and the layout says so.
#[test]
fn a_flowed_subform_is_drawn_with_its_border_under_its_content() {
    let field = |name: &str| format!(r#"<field name="{name}" w="3in" h="0.5in"><ui><textEdit/></ui></field>"#);
    let boxed = |inner: &str| {
        format!(
            r#"<subform name="box" layout="tb" w="7.5in"><border><edge thickness="1pt"/><fill><color value="230,230,230"/></fill></border><margin topInset="0.1in" bottomInset="0.2in"/>{inner}</subform>"#
        )
    };
    let lay = |body: &str| {
        layout_xml(&one_area_form(r#"layout="tb""#, &format!(r#"<subform name="body" layout="tb" w="7.5in">{body}</subform>"#))).unwrap()
    };
    // Two half-inch fields between a tenth and a fifth of an inch of margin: 1.3in from the top.
    let form = lay(&boxed(&(field("a") + &field("b"))));
    let items = &form.pages[0].items;
    let fill = items.iter().position(|i| matches!(i, Item::Fill { .. })).expect("the subform's fill");
    let Some(Item::Fill { rect, .. }) = items.get(fill) else { panic!("item {fill} is the fill") };
    assert!((rect.y - 36.0).abs() < 0.01 && (rect.h - 1.3 * 72.0).abs() < 0.01 && (rect.w - 540.0).abs() < 0.01, "{rect:?}");
    assert_eq!(items.iter().filter(|i| matches!(i, Item::Line { .. })).count(), 4, "its four edges");
    let first_widget = items.iter().position(|i| matches!(i, Item::Widget(_))).unwrap();
    assert!(fill < first_widget, "the fill is under the fields");
    assert!(form.warnings.is_empty(), "{:?}", form.warnings);
    // A flowed table's border too.
    let table = r#"<subform name="grid" layout="table" columnWidths="2in 2in" w="4in"><border><edge/></border><subform name="row" layout="row"><draw name="c1" h="0.4in"><value><text>One</text></value></draw><draw name="c2" h="0.4in"><value><text>Two</text></value></draw></subform></subform>"#;
    let form = lay(table);
    assert_eq!(form.pages[0].items.iter().filter(|i| matches!(i, Item::Line { .. })).count(), 4, "{:?}", form.pages[0].items);
    // Thirty fields: 19 fit the first page after the top margin, 11 go to the second.
    let many: String = (0..30).map(|i| field(&format!("f{i}"))).collect();
    let form = lay(&boxed(&many));
    assert_eq!(form.pages.len(), 2);
    let fills = |page: usize| -> Vec<Rect> {
        form.pages[page].items.iter().filter_map(|i| if let Item::Fill { rect, .. } = i { Some(*rect) } else { None }).collect()
    };
    let (first, second) = (fills(0), fills(1));
    assert_eq!((first.len(), second.len()), (1, 1), "a part on each page");
    // Page 1: from the top (36) past 0.1in of margin and 19 fields, then 0.2in of margin.
    assert!((first[0].y - 36.0).abs() < 0.01 && (first[0].bottom() - (36.0 + 7.2 + 19.0 * 36.0 + 14.4)).abs() < 0.01, "{:?}", first[0]);
    // Page 2: the top margin again, 11 fields, the bottom margin.
    assert!((second[0].y - 36.0).abs() < 0.01 && (second[0].bottom() - (36.0 + 7.2 + 11.0 * 36.0 + 14.4)).abs() < 0.01, "{:?}", second[0]);
    let lines = |page: usize| form.pages[page].items.iter().filter(|i| matches!(i, Item::Line { .. })).count();
    assert_eq!((lines(0), lines(1)), (4, 4), "each part's border is closed");
    let f19 = form.pages[1].items.iter().find_map(|i| if let Item::Widget(w) = i { (w.name == "f19").then_some(w.rect) } else { None }).unwrap();
    assert!((f19.y - 43.2).abs() < 0.01, "the second part starts after the top margin: {}", f19.y);
    assert!(form.warnings.is_empty(), "{:?}", form.warnings);
    // A bottom margin that would run past the content area stops at its edge: 39 quarter-inch
    // fields end at 738, and 0.4in more would reach 766.8, past 756.
    let quarter: String = (0..39).map(|i| format!(r#"<field name="q{i}" w="3in" h="0.25in"><ui><textEdit/></ui></field>"#)).collect();
    let low = format!(r#"<subform name="box" layout="tb" w="7.5in"><border><edge/><fill/></border><margin bottomInset="0.4in"/>{quarter}</subform>"#);
    let form = lay(&low);
    let edge = form.pages[0].items.iter().find_map(|i| if let Item::Fill { rect, .. } = i { Some(rect.bottom()) } else { None }).unwrap();
    assert!((edge - 756.0).abs() < 0.01, "the box ends at the content area's bottom: {edge}");
    // The margins come back without a border too.
    let bare = format!(r#"<subform name="box" layout="tb" w="7.5in"><margin topInset="0.1in" bottomInset="0.2in"/>{many}</subform>"#);
    let form = lay(&bare);
    let f19 = form.pages[1].items.iter().find_map(|i| if let Item::Widget(w) = i { (w.name == "f19").then_some(w.rect) } else { None }).unwrap();
    assert!((f19.y - 43.2).abs() < 0.01, "{}", f19.y);
    // A text of its own longer than a page, cut across pages: no border, one warning.
    let long = format!(
        r#"<draw name="long" w="7in"><font typeface="Arial" size="10pt"/><value><text>{}</text></value></draw>"#,
        "A line of the long text.\n".repeat(80)
    );
    let form = lay(&boxed(&(field("a") + &long)));
    assert!(form.pages.len() >= 2, "the long text runs onto another page");
    assert!(form.pages.iter().all(|p| !p.items.iter().any(|i| matches!(i, Item::Fill { .. }))), "no fill drawn");
    assert_eq!(form.warnings.iter().filter(|w| w.contains("bordered section")).count(), 1, "{:?}", form.warnings);
    // A field that fits a fresh page (720pt) but not after the 7.2pt top margin: the margin
    // gives way, so the field ends at the content area's bottom, inside the second part.
    let tall = r#"<field name="tall" w="3in" h="715pt"><ui><textEdit/></ui></field>"#;
    let form = lay(&boxed(&(field("a") + tall)));
    assert_eq!(form.pages.len(), 2);
    let tall = form.pages[1].items.iter().find_map(|i| if let Item::Widget(w) = i { (w.name == "tall").then_some(w.rect) } else { None }).unwrap();
    assert!(tall.bottom() <= 756.01, "the field stays in the content area: {tall:?}");
    let part = form.pages[1].items.iter().find_map(|i| if let Item::Fill { rect, .. } = i { Some(*rect) } else { None }).expect("the second part");
    assert!(part.y <= tall.y + 0.01 && part.bottom() >= tall.bottom() - 0.01, "the border holds the field: {part:?} {tall:?}");
    assert!(form.warnings.is_empty(), "{:?}", form.warnings);
    // A section that starts near a page's bottom, where its first field doesn't fit: the field
    // moves to the next page whole, and the border starts there with it, none left behind.
    let spacer = r#"<field name="spacer" w="3in" h="684pt"><ui><textEdit/></ui></field>"#;
    let form = lay(&format!("{spacer}{}", boxed(&(field("a") + &field("b")))));
    assert_eq!(form.pages.len(), 2);
    let fills = |page: usize| -> Vec<Rect> {
        form.pages[page].items.iter().filter_map(|i| if let Item::Fill { rect, .. } = i { Some(*rect) } else { None }).collect()
    };
    assert!(fills(0).is_empty(), "no border left behind on page 1: {:?}", fills(0));
    let moved = fills(1);
    assert!(moved.len() == 1 && (moved[0].y - 36.0).abs() < 0.01 && (moved[0].h - (7.2 + 2.0 * 36.0 + 14.4)).abs() < 0.01, "{moved:?}");
    assert!(form.warnings.is_empty(), "{:?}", form.warnings);
    // The same field first in the section, at the top of the page: after the section's own top
    // margin it runs past the bottom, so the section has no border round it, and says so.
    let form = lay(&boxed(r#"<field name="tall" w="3in" h="715pt"><ui><textEdit/></ui></field>"#));
    assert!(form.pages.iter().all(|p| !p.items.iter().any(|i| matches!(i, Item::Fill { .. }))), "no fill drawn");
    assert_eq!(form.warnings.iter().filter(|w| w.contains("bordered section")).count(), 1, "{:?}", form.warnings);
}

/// A flowed table a page break cuts between its rows gets a closed border round its part on
/// each page, as a flowed subform does: its margins repeat, and its overflow leader starts the
/// second part after the top margin.
#[test]
fn a_flowed_table_cut_between_its_rows_keeps_its_border_on_each_page() {
    let row = |name: &str, text: &str| {
        format!(
            r#"<subform name="{name}" layout="row"><draw name="{name}a" h="0.3in"><value><text>{text}</text></value></draw><draw name="{name}b" h="0.3in"><value><text>-</text></value></draw></subform>"#
        )
    };
    let rows: String = (0..40).map(|i| row(&format!("r{i}"), &format!("Row {i}"))).collect();
    let table = format!(
        r#"<subform name="grid" layout="table" columnWidths="2in 2in" w="4in"><border><edge/><fill><color value="230,230,230"/></fill></border><margin topInset="0.1in" bottomInset="0.2in"/><overflow leader="head"/>{}{rows}</subform>"#,
        row("head", "Head")
    );
    let body = format!(r#"<subform name="body" layout="tb" w="7.5in">{table}</subform>"#);
    let form = layout_xml(&one_area_form(r#"layout="tb""#, &body)).unwrap();
    assert_eq!(form.pages.len(), 2);
    let fills = |page: usize| -> Vec<Rect> {
        form.pages[page].items.iter().filter_map(|i| if let Item::Fill { rect, .. } = i { Some(*rect) } else { None }).collect()
    };
    let lines = |page: usize| form.pages[page].items.iter().filter(|i| matches!(i, Item::Line { .. })).count();
    let (first, second) = (fills(0), fills(1));
    assert_eq!((first.len(), second.len()), (1, 1), "a part on each page");
    assert_eq!((lines(0), lines(1)), (4, 4), "each part's border is closed");
    // Page 1: 0.1in of margin, the leader and 32 rows of 0.3in fill the area to its bottom.
    assert!((first[0].y - 36.0).abs() < 0.01 && (first[0].bottom() - 756.0).abs() < 0.01, "{:?}", first[0]);
    // Page 2: the top margin again, the leader, the other 8 rows, the bottom margin.
    assert!((second[0].y - 36.0).abs() < 0.01 && (second[0].bottom() - (36.0 + 7.2 + 9.0 * 21.6 + 14.4)).abs() < 0.01, "{:?}", second[0]);
    let head = form.pages[1].items.iter().find_map(|i| if let Item::Text(t) = i { (t.text == "Head").then_some(t.baseline) } else { None });
    assert!(head.is_some_and(|b| b > 43.2 && b < 64.8), "the leader starts the second part after its top margin: {head:?}");
    assert!(form.warnings.is_empty(), "{:?}", form.warnings);
}

/// A flowed table's overflow leader and the row that moved are given room together on the new
/// page; a leader and row too tall for any page, and a first row taller than its page, cost the
/// table its border, with the warning, rather than a border round what runs off the page.
#[test]
fn a_tables_leader_and_oversized_rows_fit_its_border_or_lose_it() {
    let row = |name: &str, h: &str| {
        format!(r#"<subform name="{name}" layout="row"><draw name="{name}a" h="{h}"><value><text>{name}</text></value></draw></subform>"#)
    };
    let table = |rows: &str| {
        format!(
            r#"<subform name="body" layout="tb" w="7.5in"><subform name="grid" layout="table" columnWidths="3in" w="3in"><border><edge/><fill/></border><margin topInset="0.1in"/><overflow leader="head"/>{}{rows}</subform></subform>"#,
            row("head", "0.3in")
        )
    };
    let lay = |rows: &str| layout_xml(&one_area_form(r#"layout="tb""#, &table(rows))).unwrap();
    let fills = |form: &Form, page: usize| -> Vec<Rect> {
        form.pages[page].items.iter().filter_map(|i| if let Item::Fill { rect, .. } = i { Some(*rect) } else { None }).collect()
    };
    let small: String = (0..10).map(|i| row(&format!("r{i}"), "0.3in")).collect();
    // A 695pt row after ten small ones moves; it and the 21.6pt leader leave 3.4pt of the 720pt
    // page for the top margin, so the row ends at the content area's bottom, in the border.
    let form = lay(&(small.clone() + &row("big", "695pt")));
    assert_eq!(form.pages.len(), 2);
    let part = fills(&form, 1);
    assert!(part.len() == 1 && (part[0].y - 36.0).abs() < 0.01 && (part[0].bottom() - 756.0).abs() < 0.01, "{part:?}");
    assert!(form.warnings.is_empty(), "{:?}", form.warnings);
    // 710pt and the leader don't fit any page: no border, one warning.
    let form = lay(&(small + &row("big", "710pt")));
    assert!(form.pages.iter().enumerate().all(|(p, _)| fills(&form, p).is_empty()), "no fill drawn");
    assert_eq!(form.warnings.iter().filter(|w| w.contains("bordered section")).count(), 1, "{:?}", form.warnings);
    // A first row taller than the page: no border round it, one warning.
    let form = lay(&row("tall", "800pt"));
    assert!(fills(&form, 0).is_empty(), "{:?}", fills(&form, 0));
    assert_eq!(form.warnings.iter().filter(|w| w.contains("bordered section")).count(), 1, "{:?}", form.warnings);
}

/// A child's own page break (breakBefore, breakAfter) breaks the flowed subform around it as a
/// break between children does: a closed part on each page, no warning.
#[test]
fn a_childs_page_break_keeps_the_border_around_it_on_each_page() {
    let field = |name: &str| format!(r#"<field name="{name}" w="3in" h="0.5in"><ui><textEdit/></ui></field>"#);
    let body = format!(
        r#"<subform name="body" layout="tb" w="7.5in"><subform name="box" layout="tb" w="7.5in"><border><edge/><fill/></border><margin topInset="0.1in" bottomInset="0.2in"/>{}<subform name="s1" layout="tb"><breakBefore targetType="pageArea"/>{}</subform><subform name="s2" layout="tb"><breakAfter targetType="pageArea"/>{}</subform>{}</subform></subform>"#,
        field("a"),
        field("b"),
        field("c"),
        field("d")
    );
    let form = layout_xml(&one_area_form(r#"layout="tb""#, &body)).unwrap();
    assert_eq!(form.pages.len(), 3, "a, then b and c, then d");
    for (page, fields) in [(0, 1.0), (1, 2.0), (2, 1.0)] {
        let fills: Vec<Rect> =
            form.pages[page].items.iter().filter_map(|i| if let Item::Fill { rect, .. } = i { Some(*rect) } else { None }).collect();
        let lines = form.pages[page].items.iter().filter(|i| matches!(i, Item::Line { .. })).count();
        assert_eq!((fills.len(), lines), (1, 4), "page {}: one closed part", page + 1);
        // The top margin, its fields, the bottom margin.
        assert!((fills[0].y - 36.0).abs() < 0.01 && (fills[0].h - (7.2 + fields * 36.0 + 14.4)).abs() < 0.01, "page {}: {:?}", page + 1, fills[0]);
    }
    assert!(form.warnings.is_empty(), "{:?}", form.warnings);
}

#[test]
fn a_page_break_keeps_a_fields_border_with_the_field() {
    // 6in of heading, then a bordered 5in field: it doesn't fit the 10in area and moves to
    // the next page whole, leaving no piece of its border behind.
    let body = r#"<subform name="body" layout="lr-tb" w="7.5in">
  <draw name="head" w="7.5in" h="6in"><value><text>Head</text></value></draw>
  <field name="f0" w="7.5in" h="5in"><border><edge/><fill><color value="230,230,230"/></fill></border><ui><textEdit multiLine="1"/></ui></field>
  <field name="f1" w="7.5in" h="0.4in"><ui><textEdit/></ui></field>
 </subform>"#;
    let form = layout_xml(&one_area_form(r#"layout="tb""#, body)).unwrap();
    assert_paginated(&form, 2, 2);
    let below_head = |it: &&Item| match it {
        Item::Fill { rect, .. } => rect.bottom() > 36.0 + 432.0 + 1.0,
        Item::Line { from, to, .. } => from.1.max(to.1) > 36.0 + 432.0 + 1.0,
        _ => false,
    };
    assert_eq!(form.pages[0].items.iter().filter(below_head).count(), 0, "{:?}", form.pages[0].items);
    assert!(
        form.pages[1].items.iter().any(|it| matches!(it, Item::Fill { rect, .. } if (rect.y - 36.0).abs() < 0.01 && (rect.h - 360.0).abs() < 0.01))
    );
}

#[test]
fn text_too_long_for_its_fixed_height_is_drawn_smaller_instead_of_over_what_follows() {
    let words = "certification statement wording ".repeat(12);
    let body = format!(
        r#"<draw name="note" w="7.5in" h="0.3in"><font typeface="Arial" size="8pt"/><value><text>{words}</text></value></draw>
 <draw name="short" w="7.5in" h="0.3in"><font typeface="Arial" size="8pt"/><value><text>Short</text></value></draw>"#
    );
    let form = layout_xml(&one_area_form(r#"layout="tb""#, &body)).unwrap();
    let spans: Vec<&crate::text::Span> = form.pages[0].items.iter().filter_map(|i| if let Item::Text(s) = i { Some(s) } else { None }).collect();
    let (short, note): (Vec<_>, Vec<_>) = spans.into_iter().partition(|s| s.text == "Short");
    // Three lines at 8pt need 27.6pt; the box is 21.6pt high.
    assert!(note.iter().all(|s| s.face.size < 8.0 && s.face.size >= 5.6), "{:?}", note.iter().map(|s| s.face.size).collect::<Vec<_>>());
    assert!(note.iter().all(|s| s.baseline <= 36.0 + 21.6 + 0.5), "{:?}", note.iter().map(|s| s.baseline).collect::<Vec<_>>());
    assert!(note.iter().all(|s| s.x + s.face.width(&s.text) <= 36.0 + 540.0 + 0.5));
    // Text that fits keeps its size.
    assert!((short[0].face.size - 8.0).abs() < 1e-9);
}

#[test]
fn a_text_field_that_outgrows_a_page_continues_on_the_next_as_text() {
    let long: String = (0..150).map(|i| format!("Line {i} of the notes.\n")).collect();
    let form = layout_xml(&wrapped_form(&format!("<form><body><notes>{long}</notes><after>end</after></body></form>"))).unwrap();
    assert!(form.pages.len() >= 3, "{} pages", form.pages.len());
    assert!(form.warnings.iter().any(|w| w.starts_with("notes: its text is longer than a page")), "{:?}", form.warnings);
    // Every line is there, in order, inside the content area of its page; no widget holds it.
    let mut lines = Vec::new();
    for (i, p) in form.pages.iter().enumerate() {
        for it in &p.items {
            if let Item::Text(s) = it
                && s.text.starts_with("Line ")
            {
                assert!(s.baseline > 36.0 && s.baseline <= 36.0 + 720.0 + 0.01, "page {}: {:?} at {}", i + 1, s.text, s.baseline);
                lines.push(s.text.clone());
            }
        }
        assert!(widgets(p).iter().all(|w| w.name != "notes"));
    }
    assert_eq!(lines, (0..150).map(|i| format!("Line {i} of the notes.")).collect::<Vec<_>>());
    // What follows comes after the last line, on the last page.
    let last = form.pages.last().unwrap();
    let after = widgets(last).into_iter().find(|w| w.name == "after").unwrap();
    let last_line = last.items.iter().filter_map(|it| if let Item::Text(s) = it { Some(s.baseline) } else { None }).fold(0.0, f64::max);
    assert!(after.rect.y >= last_line - 0.01 && after.rect.bottom() <= 36.0 + 720.0 + 0.01, "{:?} after {last_line}", after.rect);
}

#[test]
fn the_form_packet_restores_the_presence_and_access_the_form_was_saved_with() {
    // Three rows hidden in the template; a script showed the first two before the form was
    // saved, and locked a field.
    let row = |n: usize, presence: &str| {
        format!(
            r#"<subform name="row{n}" layout="lr-tb" w="7.5in"{presence}><field name="who" w="3in" h="0.4in"><ui><textEdit/></ui></field></subform>"#
        )
    };
    let body = format!(
        r#"<subform name="body" layout="lr-tb" w="7.5in">{}{}{}{}<subform name="rep" layout="lr-tb" w="7.5in"><occur max="-1"/><field name="x" w="3in" h="0.4in"><ui><textEdit/></ui></field></subform></subform>"#,
        row(0, ""),
        row(1, r#" presence="hidden""#),
        row(2, r#" presence="hidden""#),
        row(3, r#" presence="hidden""#)
    );
    let template = one_area_form(r#"layout="tb""#, &body);
    let visible = |xdp: &str| -> Vec<String> { layout_xml(xdp).unwrap().pages.iter().flat_map(widgets).map(|w| w.som.clone()).collect() };
    assert_eq!(visible(&template), ["form[0].body[0].row0[0].who[0]", "form[0].body[0].rep[0].x[0]"]);
    let saved = r#"<form xmlns="http://www.xfa.org/schema/xfa-form/2.8/" checksum="x"><subform name="form">
 <instanceManager name="_body"/>
 <subform name="body">
  <subform name="row0"><field name="who" access="readOnly"/></subform>
  <subform name="row1" presence="visible"><field name="who" access="readOnly"><value override="1"/></field></subform>
  <subform name="row2" presence="visible"/>
  <subform name="row3" presence="hidden"/>
  <subform name="rep"><field name="x"/></subform>
  <subform name="rep" presence="hidden"><field name="x" access="readOnly"/></subform>
  <subform name="nowhere" presence="hidden"/>
 </subform>
 <pageSet><pageArea name="page"/></pageSet>
</subform></form>
</xdp:xdp>"#;
    let xdp = template.replace("</xdp:xdp>", saved);
    assert_eq!(
        visible(&xdp),
        ["form[0].body[0].row0[0].who[0]", "form[0].body[0].row1[0].who[0]", "form[0].body[0].row2[0].who[0]", "form[0].body[0].rep[0].x[0]"],
        "a later instance of a repeating subform says nothing about the template's one"
    );
    let form = layout_xml(&xdp).unwrap();
    let locked: Vec<bool> = form.pages.iter().flat_map(widgets).map(|w| w.read_only).collect();
    assert_eq!(locked, [true, true, false, false]);
}

#[test]
fn a_flowed_subform_is_as_tall_as_its_content_whatever_its_minimum_height() {
    // A bar with a stale 3in `minH` holds one 0.2in line; the row after it follows directly.
    let body = |bar: &str| {
        format!(
            r#"<subform name="body" layout="lr-tb" w="7.5in"><subform name="bar" {bar} w="7.5in" minH="3in"><draw w="7.5in" h="0.2in"><value><text>Attachments</text></value></draw></subform><field name="who" w="3in" h="0.4in"><ui><textEdit/></ui></field></subform>"#
        )
    };
    let top = |bar: &str| {
        let form = layout_xml(&one_area_form(r#"layout="tb""#, &body(bar))).unwrap();
        let w = widgets(&form.pages[0]);
        w[0].rect.y
    };
    let flowed = top(r#"layout="lr-tb""#);
    assert!((top(r#"layout="tb""#) - flowed).abs() < 0.01);
    // A positioned subform keeps its minimum: nothing else says how tall it is.
    assert!((top("") - flowed - (216.0 - 14.4)).abs() < 0.01, "{} against {flowed}", top(""));
}

#[test]
fn the_form_packet_restores_a_caption_a_script_changed() {
    let body = r#"<subform name="body" layout="tb" w="7.5in"><field name="bar" w="2in" h="0.3in"><ui><button/></ui><caption><value><text>Show Bar</text></value></caption></field></subform>"#;
    let template = one_area_form(r#"layout="tb""#, body);
    let caption = |xdp: &str| match &widgets(&layout_xml(xdp).unwrap().pages[0])[0].kind {
        WidgetKind::Button { caption } => caption.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(caption(&template), "Show Bar");
    let saved = r#"<form xmlns="http://www.xfa.org/schema/xfa-form/2.8/"><subform name="form"><subform name="body">
 <field name="bar"><caption><value><text>Hide Bar</text></value></caption></field>
</subform></subform></form>
</xdp:xdp>"#;
    assert_eq!(caption(&template.replace("</xdp:xdp>", saved)), "Hide Bar");
}

#[test]
fn a_text_field_without_a_border_of_its_own_has_no_box() {
    let body = r#"<subform name="body" layout="tb" w="7.5in"><field name="plain" w="3in" h="0.4in"><ui><textEdit/></ui></field><field name="boxed" w="3in" h="0.4in"><ui><textEdit><border><edge/></border></textEdit></ui></field><field name="tick" w="3in" h="0.4in"><ui><checkButton/></ui></field></subform>"#;
    let form = layout_xml(&one_area_form(r#"layout="tb""#, body)).unwrap();
    let shapes: Vec<BorderShape> = widgets(&form.pages[0]).iter().map(|w| w.border.shape).collect();
    assert_eq!(shapes, [BorderShape::None, BorderShape::Full, BorderShape::Full]);
}

/// One measured check box: its name, width, box size, check box children, field children, and
/// where its box was drawn (x and y in inches from the field's top left).
type CheckCase<'a> = (&'a str, &'a str, Option<&'a str>, String, String, f64, f64);

/// A check box field for the placement tests: `extra` holds its margin, paragraph and caption,
/// `ui` the check box's own children, and `size` its box size (none for the default 10pt).
fn placed_check(name: &str, x: f64, y: f64, w: &str, size: Option<&str>, ui: &str, extra: &str) -> String {
    let size = size.map(|s| format!(r#" size="{s}""#)).unwrap_or_default();
    format!(
        r#"<field name="{name}" x="{x}in" y="{y}in" w="{w}" h="0.6in"><ui><checkButton{size}>{ui}</checkButton></ui><font typeface="Arial" size="9pt"/>{extra}<items><integer>1</integer><integer>0</integer></items></field>"#
    )
}

/// Where a check box is drawn inside its field, measured in inches from the field's top left in
/// another viewer on synthetic probes: with no caption the box sits at the right of the content
/// area whatever the paragraph's alignment, beside a reserved caption the alignment places it,
/// a caption without a reserve takes all the box leaves, and the box's own margin doesn't move it.
#[test]
fn a_check_box_sits_where_its_margins_caption_and_alignment_put_it() {
    let m = |a: &str| format!("<margin {a}/>");
    let p = |a: &str| format!("<para {a}/>");
    let cap = |a: &str, text: &str| format!(r#"<caption {a}><font typeface="Arial" size="9pt"/><value><text>{text}</text></value></caption>"#);
    // (name, width, box size, check box children, field children, box x and y in inches)
    let cases: Vec<CheckCase> = vec![
        ("none", "1.2in", Some("0.25in"), String::new(), String::new(), 0.95, 0.0),
        ("left_margin", "1.2in", Some("0.25in"), String::new(), m(r#"leftInset="0.3in""#), 0.95, 0.0),
        ("right_margin", "1.2in", Some("0.25in"), String::new(), m(r#"rightInset="0.3in""#), 0.65, 0.0),
        ("both_margins", "1.2in", Some("0.25in"), String::new(), m(r#"leftInset="0.1in" rightInset="0.4in""#), 0.55, 0.0),
        ("h_left", "1.2in", Some("0.25in"), String::new(), p(r#"hAlign="left""#), 0.95, 0.0),
        ("h_center", "1.2in", Some("0.25in"), String::new(), p(r#"hAlign="center""#), 0.95, 0.0),
        ("h_right", "1.2in", Some("0.25in"), String::new(), p(r#"hAlign="right""#), 0.95, 0.0),
        ("h_justify", "1.2in", Some("0.25in"), String::new(), p(r#"hAlign="justify""#), 0.95, 0.0),
        ("center_right_margin", "1.2in", Some("0.25in"), String::new(), m(r#"rightInset="0.3in""#) + &p(r#"hAlign="center""#), 0.65, 0.0),
        ("left_left_margin", "1.2in", Some("0.25in"), String::new(), m(r#"leftInset="0.3in""#) + &p(r#"hAlign="left""#), 0.95, 0.0),
        ("v_top_margin", "1.2in", Some("0.25in"), String::new(), m(r#"topInset="0.15in""#) + &p(r#"vAlign="top""#), 0.95, 0.15),
        ("v_middle_margin", "1.2in", Some("0.25in"), String::new(), m(r#"topInset="0.2in""#) + &p(r#"vAlign="middle""#), 0.95, 0.275),
        ("v_bottom_margin", "1.2in", Some("0.25in"), String::new(), m(r#"bottomInset="0.15in""#) + &p(r#"vAlign="bottom""#), 0.95, 0.2),
        ("v_bottom", "1.2in", Some("0.25in"), String::new(), p(r#"vAlign="bottom""#), 0.95, 0.35),
        ("cap_left_reserve", "1.5in", Some("0.25in"), String::new(), cap(r#"placement="left" reserve="0.5in""#, "Cap"), 0.5, 0.0),
        ("cap_right_reserve", "1.5in", Some("0.25in"), String::new(), cap(r#"placement="right" reserve="0.5in""#, "Cap"), 0.0, 0.0),
        (
            "cap_left_reserve_center",
            "1.5in",
            Some("0.25in"),
            String::new(),
            cap(r#"placement="left" reserve="0.5in""#, "Cap") + &p(r#"hAlign="center""#),
            0.875,
            0.0,
        ),
        (
            "cap_left_reserve_right",
            "1.5in",
            Some("0.25in"),
            String::new(),
            cap(r#"placement="left" reserve="0.5in""#, "Cap") + &p(r#"hAlign="right""#),
            1.25,
            0.0,
        ),
        (
            "cap_left_reserve_middle",
            "1.5in",
            Some("0.25in"),
            String::new(),
            cap(r#"placement="left" reserve="0.5in""#, "Cap") + &p(r#"vAlign="middle""#),
            0.5,
            0.175,
        ),
        (
            "cap_right_reserve_center",
            "1.5in",
            Some("0.25in"),
            String::new(),
            cap(r#"placement="right" reserve="0.5in""#, "Cap") + &p(r#"hAlign="center""#),
            0.375,
            0.0,
        ),
        ("cap_top_reserve", "1.5in", Some("0.25in"), String::new(), cap(r#"placement="top" reserve="0.25in""#, "Cap"), 0.0, 0.25),
        ("cap_bottom_reserve", "1.5in", Some("0.25in"), String::new(), cap(r#"placement="bottom" reserve="0.25in""#, "Cap"), 0.0, 0.0),
        (
            "cap_hidden_reserve",
            "1.5in",
            Some("0.25in"),
            String::new(),
            cap(r#"placement="left" reserve="0.5in" presence="hidden""#, "Cap"),
            1.25,
            0.0,
        ),
        ("cap_empty_reserve", "1.5in", Some("0.25in"), String::new(), r#"<caption placement="left" reserve="0.5in"/>"#.to_string(), 0.5, 0.0),
        ("cap_left_auto", "1.5in", Some("0.25in"), String::new(), cap(r#"placement="left""#, "Cap"), 1.25, 0.0),
        ("cap_left_auto_center", "1.5in", Some("0.25in"), String::new(), cap(r#"placement="left""#, "Cap") + &p(r#"hAlign="center""#), 1.25, 0.0),
        ("cap_right_auto", "1.5in", Some("0.25in"), String::new(), cap(r#"placement="right""#, "Cap"), 0.0, 0.0),
        ("cap_right_auto_middle", "1.5in", Some("0.25in"), String::new(), cap(r#"placement="right""#, "Cap") + &p(r#"vAlign="middle""#), 0.0, 0.175),
        ("cap_top_auto", "1.5in", Some("0.25in"), String::new(), cap(r#"placement="top""#, "Cap"), 0.0, 0.35),
        ("cap_top_auto_center", "1.5in", Some("0.25in"), String::new(), cap(r#"placement="top""#, "Cap") + &p(r#"hAlign="center""#), 0.625, 0.35),
        ("cap_bottom_auto", "1.5in", Some("0.25in"), String::new(), cap(r#"placement="bottom""#, "Cap"), 0.0, 0.0),
        ("cap_inline", "1.5in", Some("0.25in"), String::new(), cap(r#"placement="inline""#, "Cap"), 1.25, 0.0),
        ("cap_hidden_auto", "1.5in", Some("0.25in"), String::new(), cap(r#"placement="left" presence="hidden""#, "Cap"), 1.25, 0.0),
        ("box_margin_left", "1.5in", Some("0.25in"), m(r#"leftInset="0.1in""#), String::new(), 1.25, 0.0),
        (
            "box_margin_all",
            "1.5in",
            Some("0.25in"),
            m(r#"leftInset="0.1in" rightInset="0.1in" topInset="0.1in" bottomInset="0.1in""#),
            String::new(),
            1.25,
            0.0,
        ),
        ("default_size", "1.2in", None, String::new(), String::new(), 1.2 - 10.0 / 72.0, 0.0),
        ("default_size_center", "1.2in", None, String::new(), p(r#"hAlign="center""#), 1.2 - 10.0 / 72.0, 0.0),
    ];
    let fields: String = cases
        .iter()
        .enumerate()
        .map(|(i, (name, w, size, ui, extra, _, _))| placed_check(name, (i % 4) as f64 * 1.8, (i / 4) as f64 * 0.8, w, *size, ui, extra))
        .collect();
    // The form in the measured case the probes started from (DD Form 2946's checklist boxes).
    let cell = r#"<field name="cell" x="0in" y="8in" w="12.7mm" h="0.6in"><ui><checkButton size="4.9389mm"><border><edge/><fill/></border></checkButton></ui><margin leftInset="0mm" rightInset="3.556mm"/><para vAlign="middle"/><items><integer>1</integer><integer>0</integer></items></field>"#;
    let body = format!(r#"<subform name="body" layout="position" w="7.5in" h="9in">{fields}{cell}</subform>"#);
    let form = layout_xml(&one_area_form(r#"layout="tb""#, &body)).unwrap();
    let laid = widgets(&form.pages[0]);
    let at = |name: &str| laid.iter().find(|w| w.name == name).map(|w| w.rect).unwrap_or_else(|| panic!("no widget {name}"));
    // The content area starts half an inch in.
    for (i, (name, _, size, _, _, x, y)) in cases.iter().enumerate() {
        let r = at(name);
        let (fx, fy) = (36.0 + (i % 4) as f64 * 1.8 * 72.0, 36.0 + (i / 4) as f64 * 0.8 * 72.0);
        let s = if size.is_some() { 18.0 } else { 10.0 };
        assert!((r.x - fx - x * 72.0).abs() < 0.01, "{name}: box x {} in, expected {x}", (r.x - fx) / 72.0);
        assert!((r.y - fy - y * 72.0).abs() < 0.01, "{name}: box y {} in, expected {y}", (r.y - fy) / 72.0);
        assert!((r.w - s).abs() < 0.01 && (r.h - s).abs() < 0.01, "{name}: box size {}x{}", r.w, r.h);
    }
    // Its box sits 4.2mm in, against its 3.556mm right margin, and in the middle of its height.
    let r = at("cell");
    let mm = 72.0 / 25.4;
    assert!((r.x - 36.0 - (12.7 - 3.556 - 4.9389) * mm).abs() < 0.01, "cell box x {}", r.x - 36.0);
    assert!((r.y - 36.0 - 8.0 * 72.0 - (0.6 * 72.0 - 4.9389 * mm) / 2.0).abs() < 0.01, "cell box y {}", r.y - 36.0 - 8.0 * 72.0);
}

/// Radio buttons in an exclusion group sit as check boxes do: at the right without a caption,
/// moved in by a right margin, and placed by the alignment or the caption beside a caption.
#[test]
fn a_radio_button_sits_where_a_check_box_would() {
    let radio = |name: &str, x: f64, extra: &str| {
        format!(
            r#"<field name="{name}" x="{x}in" y="0in" w="1.5in" h="0.6in"><ui><checkButton size="0.25in" shape="round"/></ui><font typeface="Arial" size="9pt"/>{extra}<items><text>{name}</text></items></field>"#
        )
    };
    let cap = |a: &str| format!(r#"<caption {a}><font typeface="Arial" size="9pt"/><value><text>Cap</text></value></caption>"#);
    // (name, field children, box x and y in inches from the field's top left)
    let cases = [
        ("plain", String::new(), 1.25, 0.0),
        ("right_margin", r#"<margin rightInset="0.3in"/>"#.to_string(), 0.95, 0.0),
        ("centered", r#"<para hAlign="center"/>"#.to_string(), 1.25, 0.0),
        ("cap_right_center", cap(r#"placement="right" reserve="0.5in""#) + r#"<para hAlign="center"/>"#, 0.375, 0.0),
        ("cap_left_auto", cap(r#"placement="left""#), 1.25, 0.0),
        ("cap_top_auto", cap(r#"placement="top""#), 0.0, 0.35),
    ];
    let fields: String = cases.iter().enumerate().map(|(i, (name, extra, _, _))| radio(name, i as f64 * 1.6, extra)).collect();
    let body = format!(
        r#"<subform name="body" layout="position" w="7.5in" h="9in"><exclGroup name="group" x="0in" y="0in" w="9.6in" h="0.6in" layout="position">{fields}</exclGroup></subform>"#
    );
    let form = layout_xml(&one_area_form(r#"layout="tb""#, &body)).unwrap();
    let laid = widgets(&form.pages[0]);
    for (i, (name, _, x, y)) in cases.iter().enumerate() {
        let w = laid.iter().find(|w| w.name == *name).unwrap_or_else(|| panic!("no radio button {name}"));
        assert!(matches!(w.kind, WidgetKind::Radio { .. }), "{name} is a radio button");
        let fx = 36.0 + i as f64 * 1.6 * 72.0;
        assert!((w.rect.x - fx - x * 72.0).abs() < 0.01, "{name}: box x {} in, expected {x}", (w.rect.x - fx) / 72.0);
        assert!((w.rect.y - 36.0 - y * 72.0).abs() < 0.01, "{name}: box y {} in, expected {y}", (w.rect.y - 36.0) / 72.0);
    }
}

/// A caption without a reserve leaves room for the box at the size it is drawn, a box larger than
/// its field shrinking to fit; and a field without a height is as tall as that caption drawn in
/// all the box leaves, not at some narrower width.
#[test]
fn a_check_box_caption_without_a_reserve_is_measured_as_it_is_drawn() {
    let cap =
        r#"<caption placement="left"><font typeface="Arial" size="9pt"/><value><text>Caption text running on one line</text></value></caption>"#;
    let tall = r#"<field name="tall" x="0in" y="0in" w="1.5in" h="0.25in"><ui><checkButton size="0.75in"/></ui><font typeface="Arial" size="9pt"/>"#
        .to_string()
        + cap
        + "<items><integer>1</integer><integer>0</integer></items></field>";
    let form = layout_xml(&one_area_form(r#"layout="tb""#, &format!(r#"<subform name="body" layout="position" w="7.5in" h="1in">{tall}</subform>"#)))
        .unwrap();
    let r = widgets(&form.pages[0]).iter().find(|w| w.name == "tall").map(|w| w.rect).unwrap();
    assert!((r.w - 18.0).abs() < 0.01, "the box shrinks to the field's height: {}", r.w);
    assert!((r.x - 36.0 - 1.25 * 72.0).abs() < 0.01, "and still sits at the right: {} in", (r.x - 36.0) / 72.0);

    let grows = r#"<field name="grows" w="3in"><ui><checkButton size="0.25in"/></ui><font typeface="Arial" size="9pt"/>"#.to_string()
        + cap
        + r#"<items><integer>1</integer><integer>0</integer></items></field><field name="next" w="3in" h="0.3in"><ui><textEdit/></ui></field>"#;
    let form = layout_xml(&one_area_form(r#"layout="tb""#, &format!(r#"<subform name="body" layout="tb" w="7.5in">{grows}</subform>"#))).unwrap();
    let next = widgets(&form.pages[0]).iter().find(|w| w.name == "next").map(|w| w.rect).unwrap();
    assert!((next.y - 36.0 - 18.0).abs() < 0.01, "a one-line caption beside the box keeps the field the box's height: {}", next.y - 36.0);
}

/// A caption without a reserve beside a check box takes all the room the box leaves, and starts
/// right after it; an inline caption has no text to run with and is not drawn.
#[test]
fn a_check_box_caption_without_a_reserve_takes_the_rest_of_the_field() {
    let cap = |place: &str, text: &str| {
        format!(r#"<caption placement="{place}"><font typeface="Arial" size="9pt"/><value><text>{text}</text></value></caption>"#)
    };
    let body = format!(
        r#"<subform name="body" layout="position" w="7.5in" h="9in">{}{}{}{}</subform>"#,
        placed_check("after", 0.0, 0.0, "1.5in", Some("0.25in"), "", &cap("right", "Right")),
        placed_check("before", 2.0, 0.0, "1.5in", Some("0.25in"), "", &cap("left", "Left")),
        placed_check("below", 4.0, 0.0, "1.5in", Some("0.25in"), "", &cap("bottom", "Below")),
        placed_check("inline", 0.0, 1.0, "1.5in", Some("0.25in"), "", &cap("inline", "Inline")),
    );
    let form = layout_xml(&one_area_form(r#"layout="tb""#, &body)).unwrap();
    let span = |t: &str| {
        form.pages[0].items.iter().find_map(|i| match i {
            Item::Text(s) if s.text == t => Some(s.clone()),
            _ => None,
        })
    };
    let right = span("Right").expect("the right caption is drawn");
    assert!((right.x - 36.0 - 18.0).abs() < 0.01, "a right caption starts after the box: {}", right.x - 36.0);
    let left = span("Left").expect("the left caption is drawn");
    assert!((left.x - 36.0 - 2.0 * 72.0).abs() < 0.01, "a left caption starts at the field's left: {}", left.x - 36.0);
    let below = span("Below").expect("the bottom caption is drawn");
    assert!(below.baseline - 36.0 > 18.0, "a bottom caption is below the box: {}", below.baseline - 36.0);
    assert!(span("Inline").is_none(), "an inline caption beside a check box is not drawn");
}

/// A caption reserve of zero sizes the caption to its content, as no reserve does: beside a check
/// box it takes the rest of the field, and before a text field it is as wide as its text.
#[test]
fn a_zero_caption_reserve_sizes_the_caption_as_none_does() {
    let cap = |reserve: &str| {
        format!(r#"<caption placement="right"{reserve}><font typeface="Arial" size="9pt"/><value><text>Right</text></value></caption>"#)
    };
    let text = |name: &str, y: &str, reserve: &str| {
        format!(
            r#"<field name="{name}" x="0in" y="{y}" w="3in" h="0.3in"><ui><textEdit/></ui><font typeface="Arial" size="9pt"/><caption placement="left"{reserve}><font typeface="Arial" size="9pt"/><value><text>Name</text></value></caption></field>"#
        )
    };
    let body = format!(
        r#"<subform name="body" layout="position" w="7.5in" h="9in">{}{}{}</subform>"#,
        placed_check("zero", 0.0, 0.0, "1.5in", Some("0.25in"), "", &cap(r#" reserve="0""#)),
        text("withZero", "1in", r#" reserve="0in""#),
        text("without", "2in", ""),
    );
    let form = layout_xml(&one_area_form(r#"layout="tb""#, &body)).unwrap();
    let right = form.pages[0].items.iter().find_map(|i| if let Item::Text(s) = i { (s.text == "Right").then_some(s.x) } else { None });
    assert!(right.is_some_and(|x| (x - 36.0 - 18.0).abs() < 0.01), "the caption starts after the box: {right:?}");
    let x_of = |name: &str| widgets(&form.pages[0]).iter().find(|w| w.name == name).map(|w| w.rect.x).unwrap();
    assert!(x_of("withZero") > 36.0 + 1.0, "the caption takes its text's width: {}", x_of("withZero"));
    assert!((x_of("withZero") - x_of("without")).abs() < 0.01, "as without a reserve: {} {}", x_of("withZero"), x_of("without"));
}

/// The terminal fields by full name, down the field tree.
fn full_names(doc: &Document) -> Vec<(String, Dict)> {
    let root = doc.get(doc.root().unwrap());
    let acro = doc.resolve(root.as_dict().unwrap().get(b"AcroForm").unwrap());
    let mut todo: Vec<(Object, String)> =
        doc.resolve(acro.as_dict().unwrap().get(b"Fields").unwrap()).as_array().unwrap().iter().map(|f| (f.clone(), String::new())).collect();
    let mut out = Vec::new();
    while let Some((f, prefix)) = todo.pop() {
        let d = doc.resolve(&f).as_dict().unwrap().clone();
        let t = d.get(b"T").map(|t| doc.resolve(t)).and_then(|t| t.as_string().map(|t| t.to_text()));
        let name = match t {
            Some(t) if prefix.is_empty() => t,
            Some(t) => format!("{prefix}.{t}"),
            None => prefix,
        };
        match d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) {
            Some(kids) if kids.iter().any(|k| doc.resolve(k).as_dict().is_some_and(|k| k.contains(b"T"))) => {
                todo.extend(kids.into_iter().map(|k| (k, name.clone())));
            }
            _ => out.push((name, d)),
        }
    }
    out
}

/// A laid-out signature field is named by its SOM path, under a generated field for each step
/// above it, as other viewers lay a dynamic form out again and find its signature by that name.
/// Boxes in one subform share those fields; a new layout replaces them rather than adding more;
/// taking the XFA out leaves them as ordinary fields.
#[test]
fn signature_fields_are_named_by_their_som_path() {
    let xdp = r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="page"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="body" layout="tb" w="7.5in">
  <field name="name" w="3in" h="0.3in"><ui><textEdit/></ui></field>
  <field name="sign" w="3in" h="1in"><ui><signature/></ui></field>
  <field name="other" w="3in" h="1in"><ui><signature/></ui></field>
 </subform>
 <subform name="end" layout="tb" w="7.5in">
  <field name="last" w="3in" h="1in"><ui><signature/></ui></field>
 </subform>
</subform>
</template>
</xdp:xdp>
"##;
    let mut doc = Document::open(Arc::new(shell(xdp))).unwrap();
    let report = render_into(&mut doc).unwrap();
    assert_eq!(report.fields, 4, "name and the three signature fields");
    let doc = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    let check = |doc: &Document| {
        let top: Vec<String> = pdfcraft_forms_free_fields(doc).into_iter().map(|(name, _)| name).collect();
        assert_eq!(top.iter().filter(|n| *n == "form[0]").count(), 1, "one field for the root subform: {top:?}");
        assert!(top.iter().any(|n| n == "name"), "other fields keep their names: {top:?}");
        let named = full_names(doc);
        for som in ["form[0].body[0].sign[0]", "form[0].body[0].other[0]", "form[0].end[0].last[0]"] {
            let (_, d) = named.iter().find(|(name, _)| name == som).unwrap_or_else(|| panic!("{som} in {named:?}"));
            assert_eq!(d.name(b"FT"), Some(b"Sig".as_slice()));
            assert_eq!(fields_by_som(doc).get(som).map(String::as_str), Some(som), "the data finds it by its SOM path");
        }
        assert_eq!(named.len(), 4, "{named:?}");
    };
    check(&doc);
    let body = |doc: &Document| {
        let named = full_names(doc);
        let parent = |som: &str| named.iter().find(|(name, _)| name == som).and_then(|(_, d)| d.get(b"Parent").and_then(Object::as_ref));
        (parent("form[0].body[0].sign[0]"), parent("form[0].body[0].other[0]"))
    };
    let (a, b) = body(&doc);
    assert!(a.is_some() && a == b, "the two boxes of one subform share its field");
    // Laid out again: the same fields, not a second set.
    let mut doc = doc;
    let tpl = crate::script::template_of(&doc).unwrap();
    rerender(&mut doc, &tpl).unwrap();
    check(&doc);
    // The XFA taken out: ordinary fields with the same names, without the private key.
    strip(&mut doc).unwrap();
    let named = full_names(&doc);
    assert!(named.iter().any(|(name, _)| name == "form[0].end[0].last[0]"), "{named:?}");
    let top = pdfcraft_forms_free_fields(&doc);
    let node = top.iter().find(|(name, _)| name == "form[0]").map(|(_, d)| d.clone()).expect("the root subform's field");
    assert!(!node.contains(NODE_KEY));
}

/// The terminal field named `full`, down the field tree.
fn field_ref(doc: &Document, full: &str) -> Option<pdfcraft_cos::ObjRef> {
    let root = doc.get(doc.root()?);
    let acro = doc.resolve(root.as_dict()?.get(b"AcroForm")?);
    let mut todo: Vec<(pdfcraft_cos::ObjRef, String)> =
        doc.resolve(acro.as_dict()?.get(b"Fields")?).as_array()?.iter().filter_map(Object::as_ref).map(|r| (r, String::new())).collect();
    while let Some((r, prefix)) = todo.pop() {
        let d = doc.get(r).as_dict()?.clone();
        let t = d.get(b"T").map(|t| doc.resolve(t)).and_then(|t| t.as_string().map(|t| t.to_text())).unwrap_or_default();
        let name = if prefix.is_empty() { t } else { format!("{prefix}.{t}") };
        if name == full {
            return Some(r);
        }
        let kids = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
        todo.extend(kids.iter().filter_map(Object::as_ref).map(|k| (k, name.clone())));
    }
    None
}

/// A box signed on a laid-out form stays signed when the form is laid out again (as after a
/// script adds a row): the new layout's field, named by the same SOM path, holds the same
/// signature and is locked, and the old field is gone from the AcroForm.
#[test]
fn a_signature_on_a_laid_out_box_survives_a_new_layout() {
    let mut doc = Document::open(Arc::new(shell(&fields_template("")))).unwrap();
    render_into(&mut doc).unwrap();
    let som = "form[0].page1[0].sign[0]";
    let old = field_ref(&doc, som).expect("the box");
    // Signed as signing does it: a signature dictionary as the field's value, and a picture.
    let mut sig = Dict::new();
    sig.set(b"Type".to_vec(), Object::name("Sig"));
    let sig = doc.add(Object::Dict(sig));
    let picture = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), b"0 0 1 rg 0 0 10 10 re f".to_vec())));
    doc.update_dict(old, |d| {
        d.set(b"V".to_vec(), Object::Ref(sig));
        let mut ap = Dict::new();
        ap.set(b"N".to_vec(), Object::Ref(picture));
        d.set(b"AP".to_vec(), Object::Dict(ap));
    })
    .unwrap();
    let tpl = crate::script::template_of(&doc).unwrap();
    rerender(&mut doc, &tpl).unwrap();
    let new = field_ref(&doc, som).expect("still named by its SOM path");
    assert_ne!(new, old, "a field of the new layout");
    let d = doc.get(new).as_dict().unwrap().clone();
    assert_eq!(d.get(b"V").and_then(Object::as_ref), Some(sig), "the same signature");
    let shown = d.get(b"AP").map(|ap| doc.resolve(ap)).and_then(|ap| ap.as_dict().and_then(|ap| ap.get(b"N").and_then(Object::as_ref)));
    assert_eq!(shown, Some(picture), "and the same picture");
    assert!(matches!(&*doc.get(picture), Object::Stream(_)), "which is still in the file");
    assert!(matches!(&*doc.get(sig), Object::Dict(_)), "as is the signature");
    assert_eq!(d.int(b"Ff").unwrap_or(0) & 1, 1, "a signed box is not signed again");
    let top = pdfcraft_forms_free_fields(&doc);
    assert_eq!(top.iter().filter(|(name, _)| name == "form[0]").count(), 1, "the old fields are gone");
}

/// A box right in the form's root subform, signed and then laid out again, keeps its signature
/// and picture in the file: its field sits just under the one naming it, and the old fields that
/// give way to the new layout's (freed when this session made them) take nothing it still uses.
#[test]
fn a_signed_box_in_the_root_subform_keeps_its_picture_through_a_new_layout() {
    let xdp = r#"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb"><pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
<field name="sign" w="3in" h="1in"><ui><signature/></ui></field>
</subform></template></xdp:xdp>"#;
    let mut doc = Document::open(Arc::new(shell(xdp))).unwrap();
    render_into(&mut doc).unwrap();
    let som = "form[0].sign[0]";
    let old = field_ref(&doc, som).expect("the box");
    let sig = doc.add(Object::Dict(Dict::new()));
    let picture = doc.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), b"0 0 1 rg 0 0 10 10 re f".to_vec())));
    doc.update_dict(old, |d| {
        d.set(b"V".to_vec(), Object::Ref(sig));
        let mut ap = Dict::new();
        ap.set(b"N".to_vec(), Object::Ref(picture));
        d.set(b"AP".to_vec(), Object::Dict(ap));
    })
    .unwrap();
    let tpl = crate::script::template_of(&doc).unwrap();
    rerender(&mut doc, &tpl).unwrap();
    let new = field_ref(&doc, som).expect("still named by its SOM path");
    let d = doc.get(new).as_dict().unwrap().clone();
    let shown = d.get(b"AP").map(|ap| doc.resolve(ap)).and_then(|ap| ap.as_dict().and_then(|ap| ap.get(b"N").and_then(Object::as_ref)));
    assert_eq!(shown, Some(picture));
    assert!(matches!(&*doc.get(picture), Object::Stream(_)), "the picture is still in the file");
    assert!(matches!(&*doc.get(sig), Object::Dict(_)), "and the signature");
}

/// A field of the same pass keeps a signature field named by its SOM path from repeating its
/// name, written before the box or after it: with a text field named `form[0]` (as a hostile
/// template may name one), the box keeps its own name and no two fields share one.
#[test]
fn a_signature_field_does_not_repeat_a_name_another_field_has() {
    let text = r#"<field name="form[0]" w="3in" h="0.3in"><ui><textEdit/></ui></field>"#;
    let sign = r#"<field name="sign" w="3in" h="1in"><ui><signature/></ui></field>"#;
    for (first, second) in [(text, sign), (sign, text)] {
        let xdp = format!(
            r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="page"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="body" layout="tb" w="7.5in">{first}{second}</subform>
</subform>
</template>
</xdp:xdp>
"##
        );
        let mut doc = Document::open(Arc::new(shell(&xdp))).unwrap();
        render_into(&mut doc).unwrap();
        let mut top: Vec<String> = pdfcraft_forms_free_fields(&doc).into_iter().map(|(name, _)| name).collect();
        assert!(top.iter().any(|n| n == "sign"), "the box keeps its own name: {top:?}");
        top.sort();
        let len = top.len();
        top.dedup();
        assert_eq!(top.len(), len, "no name twice: {top:?}");
    }
}

/// A signature box the file already holds a signature for shows that signature, the file's own
/// objects, and is locked; the other box stays unsigned.
#[test]
fn a_signed_signature_box_shows_the_signature_the_file_holds() {
    let mut doc = Document::open(Arc::new(crate::fixtures::signed_shell())).unwrap();
    render_into(&mut doc).unwrap();
    let catalog = doc.get(doc.root().unwrap());
    let acro = doc.resolve(catalog.as_dict().unwrap().get(b"AcroForm").unwrap());
    let fields = doc.resolve(acro.as_dict().unwrap().get(b"Fields").unwrap()).as_array().unwrap().clone();
    let laid: Vec<_> = fields.iter().map(|f| doc.resolve(f)).filter(|f| f.as_dict().unwrap().contains(SOM_KEY)).collect();
    // The file has a field named `form[0]` already: the unsigned box keeps its own name rather
    // than repeat that one.
    let (signed, unsigned) = (laid[0].as_dict().unwrap(), laid[1].as_dict().unwrap());
    assert_eq!(unsigned.get(b"T").and_then(|t| t.as_string()).map(|t| t.to_text()).as_deref(), Some("other"));
    // The signature dictionary and the picture are the file's own objects, not copies.
    let value = doc.resolve(signed.get(b"V").unwrap());
    assert_eq!(value.as_dict().unwrap().name(b"Type"), Some(&b"Sig"[..]));
    assert!(signed.get(b"V").unwrap().as_ref().is_some());
    let shown = doc.resolve(signed.get(b"AP").unwrap());
    let picture = doc.resolve(shown.as_dict().unwrap().get(b"N").unwrap());
    assert!(matches!(&*picture, Object::Stream(s) if s.dict.name(b"Subtype") == Some(&b"Form"[..])));
    assert_eq!(signed.int(b"Ff").unwrap_or(0) & 1, 1, "a signed box is not signed again");
    assert!(!unsigned.contains(b"V") && !unsigned.contains(b"AP"));
}

#[test]
fn a_carriage_return_and_line_feed_are_one_line_break() {
    let lines = |t: &str| crate::text::plain(t).paragraphs.len();
    assert_eq!(lines("one\r\ntwo\r\nthree"), 3);
    assert_eq!(lines("one\ntwo\rthree"), 3);
    assert_eq!(lines("one\r\n\r\ntwo"), 3, "an empty line stays");
    // As the data holds it: character references, which the XML parser leaves alone.
    let form = layout_xml(&wrapped_form("<form><body><notes>one&#xD;&#xA;two&#xD;&#xA;three</notes></body></form>")).unwrap();
    let crlf = form.pages.iter().flat_map(widgets).find(|w| w.name == "notes").unwrap().rect.h;
    let form = layout_xml(&wrapped_form("<form><body><notes>one&#xA;two&#xA;three</notes></body></form>")).unwrap();
    let lf = form.pages.iter().flat_map(widgets).find(|w| w.name == "notes").unwrap().rect.h;
    assert!((crlf - lf).abs() < 0.01);
}

#[test]
fn data_exports_as_datasets_xml_or_an_xdp_and_imports_from_any_of_them() {
    use crate::data::{export_data, import_data};
    let data = "<form><page1><country>JP</country><langs>English\nSpanish</langs></page1></form>";
    let mut doc = Document::open(Arc::new(shell(&fields_template(data)))).unwrap();
    render_into(&mut doc).unwrap();
    // XML: the data alone, rooted at the form's root data node, as Acrobat's XML export is.
    let xml = export_data(&doc, false, "fields.pdf").unwrap();
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<form"), "{xml}");
    assert!(xml.contains("<country>JP</country>") && xml.contains("<langs>English\nSpanish</langs>"), "{xml}");
    assert!(xml.trim_end().ends_with("</form>"), "{xml}");
    assert_eq!(crate::parse_data_file(&xml, Some("form")).unwrap().len(), 1, "the export is importable");
    // XDP: the datasets inside an xdp envelope with a reference to the PDF.
    let xdp = export_data(&doc, true, "my \"form\".pdf").unwrap();
    assert!(xdp.contains("<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\">") && xdp.contains("</xdp:xdp>"), "{xdp}");
    assert!(xdp.contains("<pdf href=\"my &quot;form&quot;.pdf\" xmlns=\"http://ns.adobe.com/xdp/pdf/\"/>"), "{xdp}");
    assert_eq!(parse_datasets(&xdp).unwrap().text_at(&som_to_path("form[0].page1[0].country[0]")), Some("JP"));
    // A form without a datasets packet exports empty data.
    let bare = Document::open(Arc::new(shell(&fields_template("")))).unwrap();
    let xml = export_data(&bare, false, "x.pdf").unwrap();
    assert!(xml.contains("<xfa:data></xfa:data>") || xml.contains("<xfa:data/>"), "{xml}");
    // A datasets packet alone (no template) still exports its data; no XFA at all is an error.
    let static_doc = Document::open(Arc::new(static_shell("<form1><page1><name>Ada</name></page1></form1>"))).unwrap();
    assert!(export_data(&static_doc, false, "s.pdf").unwrap().contains("<name>Ada</name>"));
    assert!(export_data(&static_doc, true, "s.pdf").unwrap().contains("<xfa:datasets"));
    // An XFA entry without a template or datasets exports empty data rather than failing.
    let plain = Document::open(Arc::new(shell("<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"></xdp:xdp>"))).unwrap();
    assert!(export_data(&plain, false, "p.pdf").unwrap().contains("<xfa:data"));
    // Import: each shape of file replaces the data; what no field binds to in the datasets
    // (a data description) stays.
    let with_desc = fields_template(data).replace(
        "<xfa:data>",
        "<dd:dataDescription xmlns:dd=\"http://ns.adobe.com/data-description/\" dd:name=\"form\"><form/></dd:dataDescription><xfa:data>",
    );
    let shapes = [
        format!(
            "<?xml version=\"1.0\"?>\n<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"><xfa:datasets xmlns:xfa=\"{XFA_NS}\"><xfa:data><form><page1><country>CA</country></page1></form></xfa:data></xfa:datasets></xdp:xdp>"
        ),
        format!("<xfa:datasets xmlns:xfa=\"{XFA_NS}\"><xfa:data><form><page1><country>CA</country></page1></form></xfa:data></xfa:datasets>"),
        format!("\u{feff}<xfa:data xmlns:xfa=\"{XFA_NS}\"><form><page1><country>CA</country></page1></form></xfa:data>"),
        "<form><page1><country>CA</country></page1></form>".to_string(),
    ];
    for file in &shapes {
        let mut doc = Document::open(Arc::new(shell(&with_desc))).unwrap();
        render_into(&mut doc).unwrap();
        let r = import_data(&mut doc, file, Some("form"), None).unwrap();
        assert_eq!(r.nodes, 3, "{file}");
        let d = crate::data_of(&doc).unwrap();
        assert_eq!(d.text_at(&som_to_path("form[0].page1[0].country[0]")), Some("CA"), "{file}");
        assert_eq!(d.text_at(&som_to_path("form[0].page1[0].langs[0]")), None, "the old data is gone: {file}");
        let xdp = export_data(&doc, true, "x.pdf").unwrap();
        assert!(xdp.contains("dataDescription"), "kept: {xdp}");
        assert!(!export_data(&doc, false, "x.pdf").unwrap().contains("dataDescription"), "the XML export is the data alone");
        // Laid out again, the form shows the imported value.
        let (t, _) = parse(&with_desc).unwrap();
        rerender(&mut doc, &t).unwrap();
        let fields = pdfcraft_forms_free_fields(&doc);
        let c = &fields.iter().find(|(n, _)| n == "country").unwrap().1;
        assert_eq!(c.get(b"V").and_then(|v| v.as_string()).map(|s| s.to_text()).as_deref(), Some("CA"), "{file}");
    }
    // Not XFA data: refused, the document untouched.
    let mut doc = Document::open(Arc::new(shell(&fields_template(data)))).unwrap();
    render_into(&mut doc).unwrap();
    for bad in [
        "<xfdf xmlns=\"http://ns.adobe.com/xfdf/\"><fields/></xfdf>",
        "<fields><field name=\"a\"><value>1</value></field></fields>",
        "not xml at all",
        "<a><b></a>",
    ] {
        assert!(import_data(&mut doc, bad, Some("form"), None).is_err(), "{bad}");
    }
    assert_eq!(crate::data_of(&doc).unwrap().text_at(&som_to_path("form[0].page1[0].country[0]")), Some("JP"));
    // Empty data: the data element is emptied, not removed.
    let r = import_data(&mut doc, &format!("<xfa:data xmlns:xfa=\"{XFA_NS}\"/>"), Some("form"), None).unwrap();
    assert_eq!(r.nodes, 0);
    assert!(crate::data_of(&doc).is_some());
    assert_eq!(crate::data_of(&doc).unwrap().children.len(), 0);
    // Hostile: a file over the size cap, and one with more nodes than the parser allows.
    assert!(import_data(&mut doc, &"x".repeat(17 << 20), None, None).is_err());
    let many = format!("<form>{}</form>", "<a/>".repeat(1_100_000));
    assert!(import_data(&mut doc, &many, None, None).is_err());
    // A self-closing data element opens up for an import.
    let empty = fields_template("")
        .replace("</template>", "</template><xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data/></xfa:datasets>");
    let mut doc = Document::open(Arc::new(shell(&empty))).unwrap();
    render_into(&mut doc).unwrap();
    import_data(&mut doc, "<form><page1><country>FR</country></page1></form>", Some("form"), None).unwrap();
    assert_eq!(crate::data_of(&doc).unwrap().text_at(&som_to_path("form[0].page1[0].country[0]")), Some("FR"));
}

const XFA_NS: &str = "http://www.xfa.org/schema/xfa-data/1.0/";

#[test]
fn a_picture_chosen_for_an_image_field_is_kept_in_the_data_with_its_content_type() {
    use crate::data::write_data_image;
    let mut doc = Document::open(Arc::new(shell(&fields_template("")))).unwrap();
    render_into(&mut doc).unwrap();
    let path = som_to_path("form[0].page1[0].photo[0]");
    // No data node yet: it is made.
    let r = write_data_image(&mut doc, &path, "image/gif", TINY_GIF, None).unwrap();
    assert!(r.written, "{:?}", r.warnings);
    let xdp = crate::read_packets(&doc).unwrap().unwrap().xdp;
    assert!(xdp.contains("<photo xfa:contentType=\"image/gif\">R0lGODlh"), "{xdp}");
    // Written again: replaced, not duplicated; a hostile content type is cleaned.
    let r = write_data_image(&mut doc, &path, "image/png\"><evil/><x a=\"", &tiny_png(), None).unwrap();
    assert!(r.written);
    let xdp = crate::read_packets(&doc).unwrap().unwrap().xdp;
    assert_eq!(xdp.matches("<photo ").count(), 1, "{xdp}");
    assert!(xdp.contains("<photo xfa:contentType=\"image/pngevil/xa\">iVBORw0KGgo"), "{xdp}");
    assert!(!xdp.contains("<evil/>"));
    // The form shows it: laid out again, the page's picture is the PNG from the data.
    let (t, _) = parse(&fields_template("")).unwrap();
    rerender(&mut doc, &t).unwrap();
    let d = crate::data_of(&doc).unwrap();
    assert!(d.text_at(&path).unwrap().starts_with("iVBORw0KGgo"));
    // Too big is refused before anything is written.
    assert!(write_data_image(&mut doc, &path, "image/png", &vec![0u8; (8 << 20) + 1], None).is_err());
    assert!(write_data_image(&mut doc, &Vec::new(), "image/png", TINY_GIF, None).is_err());
    // The image field is a push button marked as one, with no action and nothing drawn.
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let doc = Document::open(Arc::new(bytes)).unwrap();
    let fields = pdfcraft_forms_free_fields(&doc);
    let photo = &fields.iter().find(|(n, _)| n == "photo").unwrap().1;
    assert_eq!(photo.name(b"FT"), Some(b"Btn".as_slice()));
    assert_eq!(photo.int(b"Ff"), Some(1 << 16));
    assert_eq!(photo.get(b"PCXfaImage"), Some(&Object::Bool(true)));
    assert!(!photo.contains(b"A") && !photo.contains(b"MK"));
}

#[test]
fn imported_data_is_written_with_the_packets_own_prefix_and_never_leaves_it_unreadable() {
    use crate::data::{export_data, import_data, write_data_image};
    // A file binding the data namespace to another prefix, with an attribute in it.
    let file = format!(
        "<x:datasets xmlns:x=\"{XFA_NS}\"><x:data><form x:dataNode=\"dataGroup\"><page1><country>CA</country></page1></form></x:data></x:datasets>"
    );
    let mut doc = Document::open(Arc::new(shell(&fields_template("<form><page1><country>JP</country></page1></form>")))).unwrap();
    render_into(&mut doc).unwrap();
    import_data(&mut doc, &file, Some("form"), None).unwrap();
    let xdp = crate::read_packets(&doc).unwrap().unwrap().xdp;
    assert!(xdp.contains("<form xfa:dataNode=\"dataGroup\">"), "{xdp}");
    assert_eq!(crate::data_of(&doc).unwrap().text_at(&som_to_path("form[0].page1[0].country[0]")), Some("CA"));
    // An xsi:nil declared on an envelope the file's data element doesn't carry.
    let file = "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data><form><page1><country xsi:nil=\"true\"/><pin>1</pin></page1></form></xfa:data></xfa:datasets></xdp:xdp>";
    import_data(&mut doc, file, Some("form"), None).unwrap();
    let xdp = crate::read_packets(&doc).unwrap().unwrap().xdp;
    assert!(xdp.contains("xmlns:n0=\"http://www.w3.org/2001/XMLSchema-instance\" n0:nil=\"true\""), "{xdp}");
    assert!(crate::data_of(&doc).is_some(), "the packet still parses");
    // Our own export, from a packet using another prefix, imports back.
    let other = fields_template("<form><page1><pin>7</pin></page1></form>")
        .replace("xfa:datasets", "xd:datasets")
        .replace("xfa:data>", "xd:data>")
        .replace("xmlns:xfa=", "xmlns:xd=");
    let mut doc2 = Document::open(Arc::new(shell(&other))).unwrap();
    render_into(&mut doc2).unwrap();
    let exported = export_data(&doc2, true, "o.pdf").unwrap();
    import_data(&mut doc, &exported, Some("form"), None).unwrap();
    assert_eq!(crate::data_of(&doc).unwrap().text_at(&som_to_path("form[0].page1[0].pin[0]")), Some("7"));
    // A picture into that other-prefix packet uses its prefix.
    let path = som_to_path("form[0].page1[0].photo[0]");
    write_data_image(&mut doc2, &path, "image/gif", TINY_GIF, None).unwrap();
    let xdp = crate::read_packets(&doc2).unwrap().unwrap().xdp;
    assert!(xdp.contains("<photo xd:contentType=\"image/gif\">"), "{xdp}");
    assert!(crate::data_of(&doc2).is_some());
    // A picture where the data node is a group is refused, the group kept.
    let grouped = fields_template("<form><page1><photo><note>keep me</note></photo></page1></form>");
    let mut doc3 = Document::open(Arc::new(shell(&grouped))).unwrap();
    render_into(&mut doc3).unwrap();
    let r = write_data_image(&mut doc3, &path, "image/gif", TINY_GIF, None).unwrap();
    assert!(!r.written && r.warnings.iter().any(|w| w.contains("structured")), "{r:?}");
    assert!(crate::read_packets(&doc3).unwrap().unwrap().xdp.contains("<note>keep me</note>"));
    // A field name XML can't hold is written under the same cleaned name the values use.
    let odd = fields_template("").replace("name=\"photo\"", "name=\"Foto_Straße\"");
    let mut doc4 = Document::open(Arc::new(shell(&odd))).unwrap();
    render_into(&mut doc4).unwrap();
    let r = write_data_image(&mut doc4, &som_to_path("form[0].page1[0].Foto_Straße[0]"), "image/gif", TINY_GIF, None).unwrap();
    assert!(r.written, "{r:?}");
    let xdp = crate::read_packets(&doc4).unwrap().unwrap().xdp;
    assert_eq!(xdp.matches("<Foto_Stra_e").count(), 1, "{xdp}");
    assert!(xdp.contains("<Foto_Stra_e xfa:contentType=\"image/gif\">R0lG"), "{xdp}");
    // What is not this form's data is refused: an SVG, an XDP without datasets, an XDP whose
    // config has a <data> element before the datasets, a data document with another root.
    let mut doc = Document::open(Arc::new(shell(&fields_template("<form><page1><country>JP</country></page1></form>")))).unwrap();
    render_into(&mut doc).unwrap();
    for bad in [
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect/></svg>".to_string(),
        "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"><template/></xdp:xdp>".to_string(),
        "<other><page1><country>CA</country></page1></other>".to_string(),
        format!("<x:datasets xmlns:x=\"{XFA_NS}\"><x:data><form><page1><country>CA</country></page1></form></x:data></x:datasets>")
            .replace("x:data", "x:nodata"),
    ] {
        assert!(import_data(&mut doc, &bad, Some("form"), None).is_err(), "{bad}");
    }
    let config_first = format!(
        "<xdp:xdp xmlns:xdp=\"http://ns.adobe.com/xdp/\"><config xmlns=\"http://www.xfa.org/schema/xci/3.0/\"><present><data>junk</data></present></config><xfa:datasets xmlns:xfa=\"{XFA_NS}\"><xfa:data><form><page1><country>CA</country></page1></form></xfa:data></xfa:datasets></xdp:xdp>"
    );
    import_data(&mut doc, &config_first, Some("form"), None).unwrap();
    assert_eq!(crate::data_of(&doc).unwrap().text_at(&som_to_path("form[0].page1[0].country[0]")), Some("CA"));
    // Deep nesting is refused before it is parsed; a data path deeper than the data limit too.
    let deep = format!("{}x{}", "<a>".repeat(20_000), "</a>".repeat(20_000));
    assert!(import_data(&mut doc, &deep, None, None).is_err());
    let deepish = format!("<form>{}x{}</form>", "<a>".repeat(100), "</a>".repeat(100));
    assert!(import_data(&mut doc, &deepish, Some("form"), None).is_err());
    assert_eq!(crate::data_of(&doc).unwrap().text_at(&som_to_path("form[0].page1[0].country[0]")), Some("CA"), "untouched");
    // The data holds more rows than are laid out: said, not hidden.
    let rows: String = (0..60).map(|i| format!("<row><item>r{i}</item><amount>1</amount></row>")).collect();
    let many = scripted_template().replace(
        "</template>",
        &format!(
            "</template><xfa:datasets xmlns:xfa=\"{XFA_NS}\"><xfa:data><form><page1><table>{rows}</table></page1></form></xfa:data></xfa:datasets>"
        ),
    );
    let form = layout_xml(&many).unwrap();
    assert!(form.warnings.iter().any(|w| w.contains("60 rows") && w.contains("first 50")), "{:?}", form.warnings);
}

#[test]
fn an_anchor_type_places_the_named_point_of_the_box_at_its_x_and_y() {
    // The page label of IRCC IMM 5532E: x is the centre of the box, so its right edge sits
    // 10.795mm right of x, inside the page, not 21.59mm.
    let xml = r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="page"><contentArea x="0.5in" y="1.5in" w="7.5in" h="8in"/><medium short="8.5in" long="11in"/>
  <draw anchorType="topCenter" name="PageOf" x="197.485mm" y="13.97mm" w="21.59mm" h="3.81mm"><value><text>PAGE 1 OF 8</text></value><font size="7pt" typeface="Arial"/><para hAlign="right"/></draw>
  <draw anchorType="bottomRight" name="foot" x="8in" y="10.5in" w="1in" h="0.25in"><value><text>Footer</text></value><font size="8pt" typeface="Arial"/></draw>
  <field anchorType="topRight" name="ref" x="8in" y="1in" w="2in" h="0.3in"><ui><textEdit/></ui></field>
 </pageArea></pageSet>
 <draw name="body" w="7.5in" h="0.3in"><value><text>Body</text></value></draw>
</subform>
</template>
</xdp:xdp>
"##;
    let form = layout_xml(xml).unwrap();
    let page = &form.pages[0];
    let span = |t: &str| {
        page.items
            .iter()
            .find_map(|i| {
                if let Item::Text(s) = i
                    && s.text == t
                {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .unwrap()
    };
    let label = span("PAGE 1 OF 8");
    let right = (197.485 + 21.59 / 2.0) * 72.0 / 25.4;
    assert!(
        (label.x + label.face.width(&label.text) - right).abs() < 0.5,
        "label ends at {}, box at {right}",
        label.x + label.face.width(&label.text)
    );
    let foot = span("Footer");
    assert!((foot.x - 7.0 * 72.0).abs() < 0.01, "footer at x {}", foot.x);
    assert!(foot.baseline > 10.25 * 72.0 && foot.baseline < 10.5 * 72.0, "footer baseline {}", foot.baseline);
    let field = widgets(page).into_iter().find(|w| w.name == "ref").unwrap();
    assert!((field.rect.x - 6.0 * 72.0).abs() < 0.01 && (field.rect.y - 72.0).abs() < 0.01, "{:?}", field.rect);
}

#[test]
fn laying_a_form_out_takes_off_the_usage_rights_it_voids() {
    use crate::fixtures::usage_rights_shell;
    let xdp = fields_template("<form><page1><country>JP</country></page1></form>");
    // What `/Perms` holds once the layout is saved and the file read again.
    let laid_out = |bytes: Vec<u8>| {
        let mut doc = Document::open(Arc::new(bytes)).unwrap();
        let report = render_into(&mut doc).unwrap();
        let doc = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
        let catalog = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
        let indirect = catalog.get(b"Perms").and_then(Object::as_ref).is_some();
        let perms = catalog.get(b"Perms").map(|p| doc.resolve(p)).and_then(|p| p.as_dict().cloned());
        let said = report.warnings.iter().any(|w| w.contains("usage rights"));
        (perms, indirect, said, report.warnings)
    };
    // Usage rights alone: `/Perms` goes, and the user is told.
    let (perms, _, said, warnings) = laid_out(usage_rights_shell(&xdp, "", false));
    assert!(perms.is_none(), "{perms:?}");
    assert!(said, "{warnings:?}");
    // Beside a certification, in an object of its own: only the usage rights go, and the
    // object is rewritten where it is.
    let (perms, indirect, said, warnings) = laid_out(usage_rights_shell(&xdp, "/DocMDP 6 0 R", true));
    let perms = perms.expect("the certification stays");
    assert!(perms.contains(b"DocMDP") && !perms.contains(b"UR3"), "{perms:?}");
    assert!(indirect, "/Perms is still its own object");
    assert!(said, "{warnings:?}");
    // A form without usage rights says nothing about them.
    let (perms, _, said, warnings) = laid_out(shell(&xdp));
    assert!(perms.is_none() && !said, "{perms:?} {warnings:?}");
}

#[test]
fn stripping_takes_the_xfa_out_and_leaves_an_ordinary_document() {
    use crate::pdf::strip;
    let mut doc = Document::open(Arc::new(shell(&fields_template("<form><page1><country>JP</country></page1></form>")))).unwrap();
    render_into(&mut doc).unwrap();
    assert!(crate::read_packets(&doc).unwrap().is_some());
    // Reader usage rights on the catalog go too (changing the form voids them).
    let root = doc.root().unwrap();
    let mut ur = pdfcraft_cos::Dict::new();
    ur.set(b"UR3".to_vec(), Object::Dict(pdfcraft_cos::Dict::new()));
    doc.update_dict(root, |d| d.set(b"Perms".to_vec(), Object::Dict(ur))).unwrap();
    let r = strip(&mut doc).unwrap();
    assert!(doc.full_save_required());
    assert_eq!(r.packets, vec!["template".to_string(), "datasets".to_string()]);
    assert_eq!(r.unbound_data, Some(0));
    assert_eq!((r.script_buttons, r.image_fields), (0, 1), "{r:?}");
    assert!(r.usage_rights);
    assert!(r.field_keys >= 7, "{r:?}");
    assert!(
        r.notes().iter().any(|n| n.contains("template, datasets")) && r.notes().iter().any(|n| n.contains("extended features")),
        "{:?}",
        r.notes()
    );
    let bytes = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert!(crate::read_packets(&doc).unwrap().is_none(), "no XFA entry");
    assert!(!is_dynamic(&doc));
    assert!(crate::existing_layout(&doc).is_none(), "the layout mark is gone");
    let root = doc.get(doc.root().unwrap());
    let catalog = root.as_dict().unwrap();
    assert!(!catalog.contains(b"NeedsRendering") && !catalog.contains(b"Perms"));
    let acro = doc.resolve(catalog.get(b"AcroForm").unwrap());
    let acro = acro.as_dict().unwrap();
    assert!(!acro.contains(b"XFA") && !acro.contains(b"PCXfaLayout") && !acro.contains(b"PCXfaOverrides"));
    // The fields stay, with their values, and without the private keys.
    let fields = pdfcraft_forms_free_fields(&doc);
    assert_eq!(fields.len(), 7);
    let country = &fields.iter().find(|(n, _)| n == "country").unwrap().1;
    assert_eq!(country.get(b"V").and_then(|v| v.as_string()).map(|s| s.to_text()).as_deref(), Some("JP"));
    for (name, d) in &fields {
        for k in [b"PCSom".as_slice(), b"PCXfaClick", b"PCXfaImage", b"PCItems"] {
            assert!(!d.contains(k), "{name} still has {}", String::from_utf8_lossy(k));
        }
    }
    // The pages are untouched.
    let pages = doc.resolve(catalog.get(b"Pages").unwrap());
    assert_eq!(pages.as_dict().unwrap().int(b"Count"), Some(1));
    // Twice is nothing: a document without XFA is left as it is.
    let mut doc = doc;
    let r = strip(&mut doc).unwrap();
    assert_eq!(r, crate::Stripped::default());
    assert!(!doc.is_modified());
    // Data no field holds is counted; a form whose click buttons ran scripts too; the shape
    // check needs no packet decoded and tells a never-laid-out dynamic form from one to strip.
    let mut doc = Document::open(Arc::new(shell(&scripted_template().replace("</template>", "</template><xfa:datasets xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\"><xfa:data><form><page1><qty>3</qty><extra>kept nowhere</extra><more2>x</more2></page1></form></xfa:data></xfa:datasets>")))).unwrap();
    let before = crate::xfa_shape(&doc).unwrap();
    assert_eq!(before, crate::XfaShape { needs_rendering: true, has_fields: false, laid_out: false });
    assert!(!before.strippable(), "nothing laid out yet: stripping would leave the placeholder page");
    render_into(&mut doc).unwrap();
    assert!(crate::xfa_shape(&doc).unwrap().strippable());
    let r = strip(&mut doc).unwrap();
    assert_eq!(r.unbound_data, Some(2), "{r:?}");
    assert_eq!(r.script_buttons, 4, "addRow, removeRow, hello, legacy: {r:?}");
    assert!(crate::xfa_shape(&doc).is_none());
    // A field tree whose dictionaries share one Kids array (every kid listing every other) is
    // walked once per dictionary, not once per listing.
    let mut doc = Document::open(Arc::new(shell(&fields_template("")))).unwrap();
    render_into(&mut doc).unwrap();
    let root = doc.root().unwrap();
    let acro_ref = doc.get(root).as_dict().unwrap().get(b"AcroForm").and_then(Object::as_ref);
    let fields: Vec<Object> = (0..2000).map(|_| Object::Ref(doc.add(Object::Dict(pdfcraft_cos::Dict::new())))).collect();
    let kids = doc.add(Object::Array(fields.clone()));
    for f in &fields {
        if let Some(r) = f.as_ref() {
            doc.update_dict(r, |d| {
                d.set(b"Kids".to_vec(), Object::Ref(kids));
                d.set(b"PCSom".to_vec(), Object::String(pdfcraft_cos::PdfString::text("x")));
            })
            .unwrap();
        }
    }
    let update = |d: &mut pdfcraft_cos::Dict| d.set(b"Fields".to_vec(), Object::Array(fields.clone()));
    match acro_ref {
        Some(r) => doc.update_dict(r, update).unwrap(),
        None => doc
            .update_dict(root, |c| {
                let mut a = c.get(b"AcroForm").and_then(|a| a.as_dict().cloned()).unwrap_or_default();
                update(&mut a);
                c.set(b"AcroForm".to_vec(), Object::Dict(a));
            })
            .unwrap(),
    }
    let started = std::time::Instant::now();
    let r = strip(&mut doc).unwrap();
    assert_eq!(r.field_keys, 2000);
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
}

#[test]
fn strip_marks_document_as_requiring_full_save() {
    use crate::pdf::strip;
    let mut doc = Document::open(Arc::new(shell(&fields_template("<form><page1><country>JP</country></page1></form>")))).unwrap();
    render_into(&mut doc).unwrap();
    assert!(!doc.full_save_required());
    strip(&mut doc).unwrap();
    assert!(doc.full_save_required(), "stripping XFA packets must require a full save to prevent data exposure in incremental saves");
}

#[test]
fn som_indices_count_earlier_same_named_siblings() {
    let draw = |name: Option<&str>| {
        Node::Draw(Box::new(Draw { common: Common { name: name.map(str::to_string), ..Default::default() }, ..Default::default() }))
    };
    let children = [draw(Some("a")), draw(Some("b")), draw(Some("a")), draw(None), draw(Some("a")), draw(Some("b"))];
    let sibs = SomIndices::of(&children);
    assert_eq!((0..=children.len()).map(|i| sibs.at(i)).collect::<Vec<_>>(), [0, 0, 1, 0, 2, 1, 0]);
}

/// A TIFF is drawn as viewers show it, turned by its Orientation tag: a 2 × 1 picture (red,
/// blue) tagged 6 (turn 90° clockwise) is 1 × 2, red above blue, in layout and in the PDF.
#[test]
fn a_tiff_picture_is_turned_by_its_orientation_tag() {
    let strip: &[u8] = &[255, 0, 0, 0, 0, 255];
    let (bps_at, strip_at) = (8 + 2 + 10 * 12 + 4, 8 + 2 + 10 * 12 + 4 + 6);
    let mut b = b"II*\0".to_vec();
    b.extend_from_slice(&8u32.to_le_bytes());
    b.extend_from_slice(&10u16.to_le_bytes());
    for (tag, ty, count, value) in [
        (256u16, 3u16, 1u32, 2u32),
        (257, 3, 1, 1),
        (258, 3, 3, bps_at as u32),
        (259, 3, 1, 1),
        (262, 3, 1, 2),
        (273, 4, 1, strip_at as u32),
        (274, 3, 1, 6),
        (277, 3, 1, 3),
        (278, 3, 1, 1),
        (279, 4, 1, strip.len() as u32),
    ] {
        b.extend_from_slice(&tag.to_le_bytes());
        b.extend_from_slice(&ty.to_le_bytes());
        b.extend_from_slice(&count.to_le_bytes());
        b.extend_from_slice(&value.to_le_bytes());
    }
    b.extend_from_slice(&0u32.to_le_bytes());
    for _ in 0..3 {
        b.extend_from_slice(&8u16.to_le_bytes());
    }
    b.extend_from_slice(strip);
    let d = crate::image::decode(&b).unwrap();
    assert_eq!(crate::image::size(&b), Some((1, 2)), "layout size");
    assert_eq!((d.width, d.height), (1, 2), "decoded size");
    assert_eq!(d.samples, vec![255, 0, 0, 0, 0, 255]);
}

/// A draw `w` points wide holding `text` in Arial 10 pt, aligned `h_align`.
fn one_draw(text: &str, w: f64, h_align: &str) -> String {
    format!(
        r##"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <draw name="label" w="{w}pt" h="0.5in"><value><text>{text}</text></value><font typeface="Arial" size="10pt"/><para hAlign="{h_align}"/></draw>
</subform>
</template></xdp:xdp>"##
    )
}

/// The text spans of the only page, with their baselines.
fn draw_spans(xdp: &str) -> Vec<text::Span> {
    layout_xml(xdp).unwrap().pages[0].items.iter().filter_map(|i| if let Item::Text(s) = i { Some(s.clone()) } else { None }).collect()
}

/// A label that fits its box but is written with trailing spaces stays on one line and measures
/// as tall as without them; text wider than the box still wraps.
#[test]
fn a_space_at_the_end_of_a_line_does_not_count_when_fitting_it() {
    // The box is just wide enough for "a. DATE"; the label is written "a. DATE " (as forms
    // often are). The trailing space is never drawn, so the label stays on one line.
    let w = pdfcraft_fonts::helvetica_width("a. DATE", 10.0) + 0.01;
    let spans = draw_spans(&one_draw("a. DATE ", w, "left"));
    let baselines: std::collections::BTreeSet<i64> = spans.iter().map(|s| (s.baseline * 100.0).round() as i64).collect();
    assert_eq!(baselines.len(), 1, "{spans:?}");
    // Several spaces after it don't count either.
    let spans = draw_spans(&one_draw("a. DATE   ", w, "left"));
    let baselines: std::collections::BTreeSet<i64> = spans.iter().map(|s| (s.baseline * 100.0).round() as i64).collect();
    assert_eq!(baselines.len(), 1, "{spans:?}");
    // Nor do they add a blank line: the text measures as tall as without them.
    let face = text::Face::of(&Font { typeface: "Arial".into(), size: 10.0, ..Font::default() });
    let height = |t: &str| text::measure_height(&text::plain(t), w, &Para::default(), &face, &|_, _| String::new());
    assert_eq!(height("a. DATE   "), height("a. DATE"));
    // Text that is wider than the box still wraps.
    let spans = draw_spans(&one_draw("a. DATES ", w, "left"));
    let baselines: std::collections::BTreeSet<i64> = spans.iter().map(|s| (s.baseline * 100.0).round() as i64).collect();
    assert_eq!(baselines.len(), 2, "{spans:?}");
}

/// Centered and right-aligned text is placed by its drawn width, whatever spaces follow it.
#[test]
fn trailing_spaces_do_not_shift_centered_or_right_aligned_text() {
    let w = 200.0;
    let ink = pdfcraft_fonts::helvetica_width("TITLE", 10.0);
    let left_edge = |align: &str, text: &str| draw_spans(&one_draw(text, w, align)).first().map(|s| s.x).unwrap();
    // The draw's left edge is the content area's (0.5in); centered and right-aligned text is
    // placed by its drawn width, whatever spaces follow it.
    assert!((left_edge("center", "TITLE   ") - (36.0 + (w - ink) / 2.0)).abs() < 1e-6);
    assert!((left_edge("right", "TITLE   ") - (36.0 + w - ink)).abs() < 1e-6);
}

/// `typed_date` rewrites a date typed by another of a field's edit-picture patterns in the first
/// one, and leaves alone a date the first pattern reads, text no pattern reads, a field with one
/// pattern and a field that isn't a date.
#[test]
fn a_date_typed_by_another_of_its_edit_pictures_patterns_is_written_in_the_first() {
    let (tpl, _) = crate::parse(&crate::fixtures::date_alternatives_template()).unwrap();
    let typed = |som: &str, text: &str| crate::typed_date(&tpl, som, text);
    let leave = "form[0].leave[0]";
    assert_eq!(typed(leave, "261018").as_deref(), Some("20261018"), "by yymmdd");
    assert_eq!(typed(leave, "2026118").as_deref(), Some("20261108"), "by yyyymd");
    assert_eq!(typed(leave, "26 10 18").as_deref(), Some("20261018"), "by yy mm dd");
    assert_eq!(typed(leave, "20261018"), None, "the first pattern reads it: it stays as typed");
    assert_eq!(typed(leave, "not a date"), None);
    assert_eq!(typed(leave, "2618"), None, "no pattern has room for four digits");
    assert_eq!(typed("form[0].born[0]", "181026"), None, "one pattern: nothing else to try");
    assert_eq!(typed("form[0].name[0]", "261018"), None, "not a date field");
}

/// Only date clauses are other ways to type a date, and a date is rewritten only when the first
/// pattern keeps it: a later `text{…}` clause is no date pattern, a year a two-digit first
/// pattern can't hold leaves the text as typed instead of becoming another date, and a pattern
/// whose date the first can't keep gives way to a later one whose date it can.
#[test]
fn a_typed_date_is_rewritten_only_by_date_clauses_into_a_first_pattern_that_keeps_it() {
    let (tpl, _) = crate::parse(&crate::fixtures::date_alternatives_template()).unwrap();
    let typed = |som: &str, text: &str| crate::typed_date(&tpl, som, text);
    assert_eq!(typed("form[0].code[0]", "261018"), None, "text{{YYMMDD}} is not a date alternative");
    let short = "form[0].short[0]";
    assert_eq!(typed(short, "20261018").as_deref(), Some("261018"), "a year the first pattern keeps");
    assert_eq!(typed(short, "19011018"), None, "1901 as 01 would read back as 2001");
    // DDMMYYYY reads 20011203 as 1203-01-20, which YYMMDD can't keep; YYYYMMDD reads 2001-12-03.
    assert_eq!(typed("form[0].pick[0]", "20011203").as_deref(), Some("011203"), "the next pattern that the first keeps");
}

/// A word longer than its line is broken by character in work linear in its length.
#[test]
fn a_long_word_breaks_in_linear_work() {
    let face = text::Face::of(&Font { typeface: "Arial".into(), size: 1.0, ..Font::default() });
    let word = "i".repeat(20_000);
    text::MEASURED.with(|m| m.set(0));
    let b = text::layout(&text::plain(&word), Rect::new(0.0, 0.0, 1000.0, 0.0), &Para::default(), &face, &|_, _| String::new());
    let measured = text::MEASURED.with(|m| m.get());
    assert!(b.spans.len() > 2, "{} lines", b.spans.len());
    assert_eq!(b.spans.iter().map(|s| s.text.len()).sum::<usize>(), 20_000);
    assert!(measured < 200_000, "{measured} bytes measured for a 20000-byte word");
}

/// Rich text that embeds one long field value many times lays out a bounded amount of it.
#[test]
fn embedded_field_values_expand_to_a_bounded_amount_of_text() {
    let face = text::Face::of(&Font { typeface: "Arial".into(), size: 10.0, ..Font::default() });
    let value = "word ".repeat(3_300);
    let run = Run { embed: Some("f".into()), ..Run::default() };
    let rich = RichText { paragraphs: vec![Paragraph { runs: vec![run; 100], h_align: None }] };
    let b = text::layout(&rich, Rect::new(0.0, 0.0, 500.0, 0.0), &Para::default(), &face, &|_, _| value.clone());
    let kept: usize = b.spans.iter().map(|s| s.text.len()).sum();
    assert!(kept > 0);
    assert!(kept <= text::MAX_EMBEDDED, "{kept} bytes of embedded text laid out");
    assert!(text::natural_width(&rich, &Para::default(), &face, &|_, _| value.clone()) <= face.width(&value) * 70.0);
}

/// Once the embedded-text budget is nearly spent, a long value embedded again is copied out only
/// as far as the budget reaches, not whole each time.
#[test]
fn embedded_values_are_copied_out_only_as_far_as_the_budget() {
    let first: String = "xxxxxxx ".repeat(text::MAX_EMBEDDED / 8 + 1).chars().take(text::MAX_EMBEDDED - 1).collect();
    let second = "\u{e9}".repeat(100_000);
    let span = |id: &str| format!(r##"<span xfa:embed="#{id}" xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/"/>"##);
    let spans = format!("{}{}", span("a"), span("b").repeat(200));
    let tpl = format!(
        r#"<template><subform layout="tb">
<field id="a" presence="hidden" w="1in" h="0.2in"><ui><textEdit/></ui><value><text>{first}</text></value></field>
<field id="b" presence="hidden" w="1in" h="0.2in"><ui><textEdit/></ui><value><text>{second}</text></value></field>
<draw w="7in" h="9in"><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p>{spans}</p></body></exData></value></draw>
</subform></template>"#
    );
    crate::layout::EMBED_COPIED.with(|c| c.set(0));
    layout_xml(&tpl).unwrap();
    let copied = crate::layout::EMBED_COPIED.with(|c| c.get());
    // The draw is measured at most seven times before it is laid out once.
    assert!(copied <= 8 * text::MAX_EMBEDDED as u64 + 8 * 4 * 201, "{copied} bytes of embedded values copied");
}

/// Repeated subforms each carry their own copy of a long script: the live form holds a bounded
/// amount of script text and says it left the rest out.
#[test]
fn repeated_script_events_hold_a_bounded_amount_of_script() {
    let script = format!("var a = 1; //{}", "x".repeat(32 * 1024));
    let xml = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <subform name="outer" layout="tb"><occur min="1" max="-1" initial="50"/>
  <subform name="inner" layout="tb"><occur min="1" max="-1" initial="50"/>
   <field name="f" w="1in" h="0.3in"><ui><textEdit/></ui>
    <event activity="initialize"><script contentType="application/x-javascript">{script}</script></event></field>
  </subform>
 </subform>
</subform>
</template></xdp:xdp>"##
    );
    let (tpl, _) = parse(&xml).unwrap();
    let live = form_tree(&tpl, None, &Overrides::default());
    assert!(!live.events.is_empty());
    let held: usize = live.events.iter().map(|e| e.script.len()).sum();
    assert!(held <= crate::script::MAX_EVENT_SCRIPT_BYTES, "{held} bytes of script held by {} events", live.events.len());
    assert!(live.truncated, "{held} bytes, {} events", live.events.len());
}

/// Rows added one at a time (each op asking for one more instance) queue each instance node
/// once, and removals in between don't let the queue grow without bound.
#[test]
fn added_instances_are_queued_once_each() {
    use crate::data::{DataOp, ENSURED, write_data_ops};
    let mut doc = Document::open(Arc::new(shell(&scripted_template()))).unwrap();
    render_into(&mut doc).unwrap();
    let table = som_to_path("form[0].page1[0].table[0]");
    let ops: Vec<DataOp> = (1..=1000).map(|count| DataOp::Instances { parent: table.clone(), name: "row".into(), count }).collect();
    ENSURED.with(|n| n.set(0));
    let r = write_data_ops(&mut doc, &ops, None).unwrap();
    assert!(r.written, "{r:?}");
    assert_eq!(crate::data_of(&doc).unwrap().count(&table, "row"), 1000);
    let queued = ENSURED.with(|n| n.get());
    assert!(queued <= 2_000, "{queued} instance nodes queued for 1000 rows");
    // Removals split the ops into separate passes; together they stay within the budget.
    let ops: Vec<DataOp> = (0..300)
        .flat_map(|_| {
            [
                DataOp::Instances { parent: table.clone(), name: "row".into(), count: 1000 },
                DataOp::Remove(som_to_path("form[0].page1[0].table[0].row[999]")),
            ]
        })
        .collect();
    ENSURED.with(|n| n.set(0));
    let r = write_data_ops(&mut doc, &ops, None).unwrap();
    let queued = ENSURED.with(|n| n.get());
    assert!(queued <= 200_000, "{queued} instance nodes queued");
    assert!(r.warnings.iter().any(|w| w.contains("instances")), "{r:?}");
}

/// A namespace declared once and inherited by many imported elements is not copied into each
/// past the import's byte budget.
#[test]
fn an_inherited_namespace_does_not_multiply_an_import() {
    use crate::data::parse_data_file;
    let uri = format!("urn:{}", "n".repeat(70_000));
    let xml = format!("<form1 xmlns:p=\"{uri}\">{}</form1>", "<p:a/>".repeat(1_000));
    fn ns_bytes(nodes: &[crate::data::ImportNode]) -> usize {
        nodes.iter().map(|n| n.ns.as_ref().map_or(0, String::len) + ns_bytes(&n.children)).sum()
    }
    match parse_data_file(&xml, None) {
        Ok(nodes) => assert!(ns_bytes(&nodes) <= crate::data::MAX_IMPORT_BYTES, "{} bytes of namespaces copied", ns_bytes(&nodes)),
        Err(e) => assert!(matches!(e, XfaError::TooLarge(_)), "{e:?}"),
    }
}

/// A long data name with a high index is not copied into every sibling created before it.
#[test]
fn long_names_repeated_by_an_index_are_capped() {
    let doc = Document::open(Arc::new(static_shell("<form1/>"))).unwrap();
    let fields = vec![FieldDatum {
        obj: pdfcraft_cos::ObjRef::new(1000, 0),
        name: format!("form1[0].{}[9999]", "n".repeat(2_000)),
        data: FieldData::Text("v".into()),
    }];
    let data = build_data(&doc, &fields);
    assert!(data.len() < 2_000_000, "{} bytes of data built", data.len());
}

/// #3441: a font the form already holds as an object stays that object when the XFA layout
/// writes its fields' fonts into `/DR`.
#[test]
fn rendering_keeps_the_forms_own_font_objects() {
    let mut doc = Document::open(Arc::new(shell(&template(2)))).unwrap();
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type1"));
    font.set(b"BaseFont".to_vec(), Object::name("Symbol"));
    let mine = doc.add(Object::Dict(font));
    let mut fonts = Dict::new();
    fonts.set(b"Mine".to_vec(), Object::Ref(mine));
    let mut dr = Dict::new();
    dr.set(b"Font".to_vec(), Object::Dict(fonts));
    let root = doc.root().unwrap();
    match doc.get(root).as_dict().unwrap().get(b"AcroForm").cloned().unwrap() {
        Object::Ref(af) => doc.update_dict(af, |d| d.set(b"DR".to_vec(), Object::Dict(dr))).unwrap(),
        _ => doc
            .update_dict(root, |c| {
                if let Some(Object::Dict(af)) = c.get(b"AcroForm").cloned() {
                    let mut af = af;
                    af.set(b"DR".to_vec(), Object::Dict(dr));
                    c.set(b"AcroForm".to_vec(), Object::Dict(af));
                }
            })
            .unwrap(),
    }
    render_into(&mut doc).unwrap();
    let catalog = doc.get(root);
    let acro = doc.resolve(catalog.as_dict().unwrap().get(b"AcroForm").unwrap());
    let dr = doc.resolve(acro.as_dict().unwrap().get(b"DR").unwrap());
    let fonts = doc.resolve(dr.as_dict().unwrap().get(b"Font").unwrap());
    assert_eq!(fonts.as_dict().unwrap().reference(b"Mine"), Some(mine), "the form's own font object");
    assert!(fonts.as_dict().unwrap().reference(b"Helv").is_some(), "and the layout's fonts as objects");
}

/// Text is measured in the standard-14 face it is drawn in, at the widths ISO 32000-2 Annex D
/// gives: bold and Times are no longer Helvetica scaled, and the horizontal scale still applies.
#[test]
fn faces_measure_text_at_their_standard_14_widths() {
    use pdfcraft_fonts::Std14;
    let face = |family: text::Family, bold: bool, italic: bool| text::Face {
        family,
        bold,
        italic,
        size: 10.0,
        color: [0.0; 3],
        underline: false,
        h_scale: 1.0,
    };
    let sample = "Mill Wax 0123 Departure Date";
    for (family, bold, italic, std14) in [
        (text::Family::Helvetica, false, false, Std14::Helvetica),
        (text::Family::Helvetica, true, false, Std14::HelveticaBold),
        (text::Family::Helvetica, true, true, Std14::HelveticaBoldOblique),
        (text::Family::Times, false, false, Std14::TimesRoman),
        (text::Family::Times, true, false, Std14::TimesBold),
        (text::Family::Times, false, true, Std14::TimesItalic),
        (text::Family::Times, true, true, Std14::TimesBoldItalic),
        (text::Family::Courier, true, false, Std14::CourierBold),
    ] {
        let f = face(family, bold, italic);
        assert!(
            (f.width(sample) - std14.text_width(sample, 10.0)).abs() < 1e-9,
            "{std14:?}: {} vs {}",
            f.width(sample),
            std14.text_width(sample, 10.0)
        );
        assert_eq!(f.family.std14(bold, italic), std14);
        assert_eq!(f.family.base_font(bold, italic), std14.base_font());
    }
    // A condensed face is narrower by its scale.
    let condensed = text::Face { h_scale: 0.8, ..face(text::Family::Times, true, false) };
    assert!((condensed.width(sample) - 0.8 * Std14::TimesBold.text_width(sample, 10.0)).abs() < 1e-9);
}

/// #3944: clearing XFA password values removes values typed into `<passwordEdit/>` fields from the
/// datasets packet (including across repeating subforms), restores a field's template default
/// `<value><text>…</text></value>` when it has one, and replaces the datasets stream in place so
/// the typed password is not left in an unreferenced stream in the overlay.
#[test]
fn clear_password_values_removes_typed_passwords_from_datasets_in_place() {
    let xdp = r#"<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/">
<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="front"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="page1" layout="tb">
  <field name="user" w="2in" h="0.3in"><ui><textEdit/></ui></field>
  <field name="pin" w="2in" h="0.3in"><ui><passwordEdit/></ui></field>
  <field name="preset" w="2in" h="0.3in"><ui><passwordEdit/></ui><value><text>0000</text></value></field>
  <subform name="row" layout="lr-tb"><occur min="1" max="-1"/>
   <field name="secret" w="2in" h="0.3in"><ui><passwordEdit/></ui></field>
  </subform>
 </subform>
</subform>
</template>
<xfa:datasets xmlns:xfa="http://www.xfa.org/schema/xfa-data/1.0/">
<xfa:data><form><page1><user>Ada</user><pin>hunter2</pin><preset>9999</preset><row><secret>first</secret></row><row><secret>second</secret></row></page1></form></xfa:data>
</xfa:datasets>
</xdp:xdp>"#;
    let mut doc = Document::open(Arc::new(shell(xdp))).unwrap();
    render_into(&mut doc).unwrap();
    assert!(has_password_values(&doc));
    let cleared = clear_password_values(&mut doc).unwrap();
    assert_eq!(cleared, 4);
    assert!(!has_password_values(&doc), "only default and non-password values remain");
    assert_eq!(clear_password_values(&mut doc).unwrap(), 0);

    let packets = read_packets(&doc).unwrap().unwrap();
    assert!(packets.xdp.contains("<user>Ada</user>"), "non-password fields stay: {}", packets.xdp);
    assert!(packets.xdp.contains("<preset>0000</preset>"), "template default restored: {}", packets.xdp);
    for secret in ["hunter2", "9999", "first", "second"] {
        assert!(!packets.xdp.contains(secret), "{secret} still in datasets: {}", packets.xdp);
    }
    let written = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let reopened = Document::open(Arc::new(written)).unwrap();
    for num in reopened.object_numbers() {
        if let Object::Stream(st) = &*reopened.get(pdfcraft_cos::ObjRef::new(num, 0)) {
            let decoded = st.decoded().unwrap_or_default();
            let text = String::from_utf8_lossy(&decoded);
            for secret in ["hunter2", "9999", "first", "second"] {
                assert!(!text.contains(secret), "object {num} still holds {secret}: {text}");
            }
        }
    }
}

/// #3944: a static XFA form whose AcroForm field sets the Password flag (`/Ff 8192`) also has its
/// typed value cleared from the datasets packet.
#[test]
fn clear_password_values_clears_static_xfa_password_fields() {
    let pdf = String::from_utf8(static_shell("<form1><page1><name>hunter2</name></page1></form1>"))
        .unwrap()
        .replace("/T (name[0])", "/T (name[0]) /Ff 8192");
    let mut doc = Document::open(Arc::new(pdf.into_bytes())).unwrap();
    assert!(has_password_values(&doc));
    assert_eq!(clear_password_values(&mut doc).unwrap(), 1);
    assert!(!has_password_values(&doc));
    let packets = read_packets(&doc).unwrap().unwrap();
    assert!(!packets.xdp.contains("hunter2"), "{}", packets.xdp);
}
