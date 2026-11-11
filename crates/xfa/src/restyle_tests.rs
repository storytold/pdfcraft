use std::collections::BTreeMap;

use super::*;
use crate::layout::{Item, Widget};
use crate::model::{Node, Rect, Template};
use crate::script::{Overrides, PropRefused};

const TEMPLATE: &str = r##"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <field name="plain" w="2in" h="0.3in"><ui><textEdit/></ui></field>
 <field name="boxed" w="2in" h="0.3in"><ui><textEdit/></ui><border><edge thickness="1pt"><color value="10,20,30"/></edge><fill><color value="200,200,200"/></fill></border>
  <validate><script contentType="application/x-javascript">this.rawValue != "x"</script><message><text>Not x</text></message></validate></field>
 <draw name="label" w="2in" h="0.3in"><value><text>Hello</text></value></draw>
 <subform name="rows" layout="tb"><subform name="row" layout="tb"><occur min="1" max="3"/><field name="cell" w="1in" h="0.3in"/></subform></subform>
 <subform name="later" layout="tb"><field name="last" w="1in" h="0.3in"/></subform>
 <exclGroup name="choice" layout="lr-tb"><field name="a" w="1in" h="0.3in"><ui><checkButton shape="round"/></ui><items><text>A</text></items></field></exclGroup>
</subform></template>"##;

fn template() -> Template {
    crate::parse(TEMPLATE).unwrap().0
}

fn sets(pairs: &[(&str, Option<&str>, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(p, a, v)| (prop_key(p, *a), v.to_string())).collect()
}

fn overrides(som: &str, pairs: &[(&str, Option<&str>, &str)]) -> Overrides {
    let mut ov = Overrides::default();
    for (p, a, v) in pairs {
        ov.set_prop(som, p, *a, v).unwrap();
    }
    ov
}

fn field<'a>(t: &'a Template, name: &str) -> &'a crate::model::Field {
    t.root
        .children
        .iter()
        .find_map(|n| match n {
            Node::Field(f) if f.common.name.as_deref() == Some(name) => Some(&**f),
            _ => None,
        })
        .unwrap()
}

fn widgets(t: &Template) -> Vec<Widget> {
    crate::layout(t, None)
        .unwrap()
        .pages
        .iter()
        .flat_map(|p| p.items.iter())
        .filter_map(|i| match i {
            Item::Widget(w) => Some((**w).clone()),
            _ => None,
        })
        .collect()
}

/// Every colour the laid-out pages paint: fills, lines, rounded boxes and text.
fn page_colours(t: &Template) -> Vec<[f64; 3]> {
    let form = crate::layout(t, None).unwrap();
    let mut out = Vec::new();
    for i in form.pages.iter().flat_map(|p| p.items.iter()) {
        match i {
            Item::Fill { color, .. } | Item::Line { color, .. } => out.push(*color),
            Item::Rounded(r) => out.extend(r.fill.into_iter().chain(r.edge.map(|e| e.1))),
            Item::Text(s) => out.push(s.face.color),
            _ => {}
        }
    }
    out
}

fn rgb(r: f64, g: f64, b: f64) -> [f64; 3] {
    [r / 255.0, g / 255.0, b / 255.0]
}

#[test]
fn values_a_property_cannot_take_are_refused() {
    assert!(check("border#0/fill#0/color#0", Some("value"), "255,0,0").is_ok());
    assert!(check("border#0/fill#0/color#0", Some("value"), "red").is_err(), "not r,g,b");
    assert!(check("border#0/edge#0", Some("thickness"), "2pt").is_ok());
    assert!(check("border#0/edge#0", Some("thickness"), "thick").is_err());
    assert!(check("font#0", Some("size"), "12").is_ok());
    assert!(check("occur#0", Some("max"), "-1").is_ok());
    assert!(check("occur#0", Some("max"), "many").is_err());
    assert!(check("value#0/text#0", Some("maxChars"), "1.5").is_err());
    // Enumerations take any value, as the template's do: an unknown one reads as the default.
    assert!(check("break#0", Some("before"), "").is_ok());
    // Texts: tooltips, messages, pictures; not a script, not the value.
    assert!(check("assist#0/toolTip#0", None, "Help").is_ok());
    assert!(check("validate#0/script#0", None, "true").is_err());
    assert!(check("value#0/text#0", None, "x").is_err());
    // Paths: a known root, `tag#n` steps, no more than eight, occurrences up to 63.
    assert!(check("traversal#0/traverse#0", Some("ref"), "x").is_err(), "traversal is not a property scripts change here");
    assert!(check("caption#0/font#0", Some("weight"), "bold").is_ok());
    assert!(check("border/edge#0", Some("presence"), "hidden").is_err());
    assert!(check("border#0/edge#64", Some("presence"), "hidden").is_err());
    // A border has four sides (XFA 3.3 p. 629): no edge or corner past the fourth.
    assert!(check("border#0/edge#3", Some("presence"), "hidden").is_ok());
    assert!(check("border#0/edge#4", Some("presence"), "hidden").is_err());
    assert!(check("ui#0/textEdit#0/border#0/corner#5/color#0", Some("value"), "1,2,3").is_err());
    let t = template();
    let p = apply_props(&field(&t, "boxed").common.props, &sets(&[("border#0/edge#6/color#0", Some("value"), "1,2,3")])).0;
    assert_eq!(p, *field(&t, "boxed").common.props, "nothing is made past the fourth side");
    assert!(check(&["border#0"; 9].join("/"), Some("presence"), "hidden").is_err());
    assert!(check("border#0", Some("bad name"), "x").is_err());
    // What the form does not read again once open: how a script runs, how the data binds.
    assert!(check("validate#0/script#0", Some("contentType"), "application/x-javascript").is_err());
    assert!(check("bind#0", Some("ref"), "$.other").is_err());
    assert!(check("bind#0", Some("match"), "none").is_err());
    assert!(check("bind#0/picture#0", None, "date{YYYYMMDD}").is_ok());
    assert!(check("extras#0/foo#8", Some("x"), "1").is_err(), "an occurrence past what forms use");
    assert!(check("assist#0/toolTip#0", None, &"x".repeat(MAX_PROP_VALUE + 1)).is_err());
}

#[test]
fn changes_to_missing_properties_add_them() {
    let t = template();
    let plain = &field(&t, "plain").common.props;
    // A border the field doesn't have: added with only its fill.
    let p = apply_props(plain, &sets(&[("border#0/fill#0/color#0", Some("value"), "255,0,0")])).0;
    let border = p.elements.iter().find(|e| e.tag == "border").unwrap();
    assert_eq!(border.children.len(), 1);
    assert_eq!(border.children[0].children[0].attrs, [("value".to_string(), "255,0,0".to_string())]);
    // An edge where the border lists none starts as a default edge.
    let p = apply_props(plain, &sets(&[("border#0/edge#0", Some("presence"), "visible")])).0;
    let border = p.elements.iter().find(|e| e.tag == "border").unwrap();
    assert_eq!(border.children[0].tag, "edge");
    assert_eq!(border.children[0].attrs, [("presence".to_string(), "visible".to_string())]);
    // A side with no edge of its own starts as the edge drawn there (one listed: all four),
    // and only that side changes.
    let boxed = &field(&t, "boxed").common.props;
    let p = apply_props(boxed, &sets(&[("border#0/edge#2/color#0", Some("value"), "255,0,0")])).0;
    let edges: Vec<&Prop> = p.elements.iter().find(|e| e.tag == "border").unwrap().children.iter().filter(|e| e.tag == "edge").collect();
    assert_eq!(edges.len(), 3);
    let colour = |e: &Prop| e.children[0].attrs[0].1.clone();
    assert_eq!([colour(edges[0]), colour(edges[1]), colour(edges[2])], ["10,20,30", "10,20,30", "255,0,0"]);
    assert_eq!(edges[1].attrs, [("thickness".to_string(), "1pt".to_string())], "a copy of the edge drawn there");
    // The template's tree is not touched.
    assert_eq!(field(&t, "boxed").common.props.elements.iter().find(|e| e.tag == "border").unwrap().children.len(), 2);
}

#[test]
fn colours_borders_and_fonts_change_the_page() {
    let t = template();
    let red = rgb(255.0, 0.0, 0.0);
    let blue = rgb(0.0, 0.0, 255.0);
    let green = rgb(0.0, 128.0, 0.0);
    assert!(!page_colours(&t).contains(&red));
    let ov = overrides(
        "form[0].boxed[0]",
        &[("border#0/fill#0/color#0", Some("value"), "255,0,0"), ("border#0/edge#0/color#0", Some("value"), "0,0,255")],
    );
    let laid = crate::apply_overrides(&t, &ov);
    let colours = page_colours(&laid);
    assert!(colours.contains(&red) && colours.contains(&blue), "{colours:?}");
    assert!(!colours.contains(&rgb(200.0, 200.0, 200.0)), "the old fill is gone");
    // A draw's font colour and a hidden border fill.
    let ov = overrides("form[0].label[0]", &[("font#0/fill#0/color#0", Some("value"), "0,128,0")]);
    assert!(page_colours(&crate::apply_overrides(&t, &ov)).contains(&green));
    let ov = overrides("form[0].boxed[0]", &[("border#0/fill#0", Some("presence"), "hidden")]);
    assert!(!page_colours(&crate::apply_overrides(&t, &ov)).contains(&rgb(200.0, 200.0, 200.0)));
    // A border's presence and an edge's thickness.
    let ov = overrides("form[0].boxed[0]", &[("border#0", Some("presence"), "hidden")]);
    assert!(!page_colours(&crate::apply_overrides(&t, &ov)).contains(&rgb(10.0, 20.0, 30.0)));
    let ov = overrides("form[0].boxed[0]", &[("border#0/edge#0", Some("thickness"), "3pt")]);
    let laid = crate::apply_overrides(&t, &ov);
    assert_eq!(field(&laid, "boxed").common.border.as_ref().unwrap().edges[0].thickness, 3.0);
}

#[test]
fn field_flags_tooltips_and_pictures_follow_their_properties() {
    let t = template();
    let w = |t: &Template, som: &str| widgets(t).into_iter().find(|w| w.som == som).unwrap();
    assert!(!w(&t, "form[0].plain[0]").required);
    let ov = overrides(
        "form[0].plain[0]",
        &[
            ("validate#0", Some("nullTest"), "error"),
            ("assist#0/toolTip#0", None, "Your name"),
            ("value#0/text#0", Some("maxChars"), "5"),
            ("font#0", Some("size"), "14pt"),
            ("para#0", Some("hAlign"), "right"),
            ("format#0/picture#0", None, "text{999}"),
        ],
    );
    let laid = crate::apply_overrides(&t, &ov);
    let got = w(&laid, "form[0].plain[0]");
    assert!(got.required, "nullTest=\"error\" makes the field required (XFA 3.3 p. 881)");
    assert_eq!(got.tooltip.as_deref(), Some("Your name"));
    assert_eq!(got.max_chars, Some(5));
    assert_eq!(got.face.size, 14.0);
    assert_eq!(got.h_align, crate::model::HAlign::Right);
    assert_eq!(field(&laid, "plain").display_picture.as_deref(), Some("text{999}"));
    // `warning` only recommends a value; turning the test off makes it optional again.
    let ov = overrides("form[0].plain[0]", &[("validate#0", Some("nullTest"), "warning")]);
    assert!(!w(&crate::apply_overrides(&t, &ov), "form[0].plain[0]").required);
    // An exclusion group that must be answered.
    let ov = overrides("form[0].choice[0]", &[("validate#0", Some("nullTest"), "error")]);
    let laid = crate::apply_overrides(&t, &ov);
    assert!(laid.root.children.iter().any(|n| matches!(n, Node::ExclGroup(g) if g.required)));
    // The validation's message, and its script turned off (p. 881: "Do not perform this test").
    let ov = overrides("form[0].boxed[0]", &[("validate#0/message#0/text#0", None, "Anything but x")]);
    assert_eq!(field(&crate::apply_overrides(&t, &ov), "boxed").validate_message.as_deref(), Some("Anything but x"));
    let ov = overrides("form[0].boxed[0]", &[("validate#0", Some("scriptTest"), "disabled")]);
    let laid = crate::apply_overrides(&t, &ov);
    assert!(field(&laid, "boxed").validate.is_none());
    assert!(!crate::form_tree(&laid, None, &ov).events.iter().any(|e| e.activity == "validate"), "{:?}", crate::form_tree(&laid, None, &ov).events);
    assert!(!crate::form_tree(&t, None, &ov).events.iter().any(|e| e.activity == "validate"), "the live form agrees");
}

#[test]
fn occurrences_and_breaks_follow_their_properties() {
    let t = template();
    let ov = overrides("form[0].rows[0].row[0]", &[("occur#0", Some("initial"), "3"), ("occur#0", Some("max"), "5")]);
    let live = crate::form_tree(&t, None, &ov);
    let rows = live.root.children.iter().find(|n| n.name == "rows").unwrap();
    assert_eq!(rows.children.len(), 3, "the initial count");
    assert_eq!(rows.children[0].occur_max, Some(5), "what the instance manager allows");
    assert_eq!(widgets(&crate::apply_overrides(&t, &ov)).iter().filter(|w| w.som.contains(".row[")).count(), 3);
    // A subform made to start on a new page.
    let pages = |t: &Template| crate::layout(t, None).unwrap().pages.len();
    assert_eq!(pages(&t), 1);
    let ov = overrides("form[0].later[0]", &[("break#0", Some("before"), "pageArea")]);
    assert_eq!(pages(&crate::apply_overrides(&t, &ov)), 2);
}

#[test]
fn scripts_read_the_changes_back_from_the_live_form() {
    let t = template();
    let ov = overrides("form[0].plain[0]", &[("border#0/fill#0/color#0", Some("value"), "1,2,3")]);
    let live = crate::form_tree(&t, None, &ov);
    let plain = live.root.children.iter().find(|n| n.name == "plain").unwrap();
    let border = plain.props.elements.iter().find(|e| e.tag == "border").unwrap();
    assert_eq!(border.children[0].children[0].attrs[0].1, "1,2,3");
    // In place, as a fresh build gives it.
    let mut stale = crate::form_tree(&t, None, &Overrides::default());
    assert!(crate::refresh_props(&mut stale.root, &ov, "form[0].plain[0]"));
    assert_eq!(stale.root.children.iter().find(|n| n.name == "plain").unwrap().props, plain.props);
    assert!(!crate::refresh_props(&mut stale.root, &ov, "form[0].nothing[0]"));
}

#[test]
fn property_changes_are_kept_in_the_file_within_limits() {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(crate::fixtures::shell(TEMPLATE))).unwrap();
    let mut ov = overrides("form[0].plain[0]", &[("border#0/fill#0/color#0", Some("value"), "255,0,0"), ("assist#0/toolTip#0", None, "Tip")]);
    ov.presence.insert("form[0].later[0]".into(), "hidden".into());
    crate::set_overrides(&mut doc, &ov).unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let reopened = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)).unwrap();
    assert_eq!(crate::overrides(&reopened), ov);
    // Refused: what isn't a value the property takes, and past the form's limit.
    let mut ov = Overrides::default();
    assert!(matches!(ov.set_prop("s", "border#0/fill#0/color#0", Some("value"), "red"), Err(PropRefused::Invalid(_))));
    assert_eq!(ov.set_prop("s", "border#0/fill#0/color#0", Some("value"), "1,2,3"), Ok(true));
    assert_eq!(ov.set_prop("s", "border#0/fill#0/color#0", Some("value"), "1,2,3"), Ok(false), "already so");
    for i in 1..MAX_PROP_OVERRIDES {
        assert_eq!(ov.set_prop(&format!("f{i}"), "font#0", Some("size"), "9"), Ok(true));
    }
    assert_eq!(ov.set_prop("one-more", "font#0", Some("size"), "9"), Err(PropRefused::Full));
    assert_eq!(ov.set_prop("s", "border#0/fill#0/color#0", Some("value"), "4,5,6"), Ok(true), "a change to a kept one still fits");
    let mut ov = Overrides::default();
    let long = "x".repeat(MAX_PROP_VALUE);
    let mut kept = 0usize;
    while ov.set_prop(&format!("f{kept}"), "assist#0/toolTip#0", None, &long).is_ok() {
        kept += 1;
    }
    assert!(kept > 0 && kept * MAX_PROP_VALUE <= MAX_PROP_OVERRIDE_BYTES, "{kept}");
    // A file holding more, or values a script couldn't set, keeps only what fits.
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(crate::fixtures::shell(TEMPLATE))).unwrap();
    let mut hostile = Overrides::default();
    hostile.presence.insert("x".into(), "hidden".into());
    crate::set_overrides(&mut doc, &hostile).unwrap();
    let root = doc.root().unwrap();
    let acro = doc.get(root).as_dict().unwrap().get(b"AcroForm").cloned().unwrap();
    let mut d = doc.resolve(&acro).as_dict().unwrap().clone();
    let mut o = doc.resolve(d.get(crate::OVERRIDES_KEY).unwrap()).as_dict().unwrap().clone();
    let mut objects = pdfcraft_cos::Dict::new();
    let mut bad = pdfcraft_cos::Dict::new();
    bad.set(b"border#0/fill#0/color#0@value".to_vec(), pdfcraft_cos::Object::String(pdfcraft_cos::PdfString::text("not a colour")));
    bad.set(b"traversal#0@x".to_vec(), pdfcraft_cos::Object::String(pdfcraft_cos::PdfString::text("1")));
    bad.set(b"font#0@size".to_vec(), pdfcraft_cos::Object::String(pdfcraft_cos::PdfString::text("9pt")));
    objects.set(b"a".to_vec(), pdfcraft_cos::Object::Dict(bad));
    for i in 0..MAX_PROP_OVERRIDES + 10 {
        let mut one = pdfcraft_cos::Dict::new();
        one.set(b"font#0@size".to_vec(), pdfcraft_cos::Object::String(pdfcraft_cos::PdfString::text("9")));
        objects.set(format!("g{i:06}").into_bytes(), pdfcraft_cos::Object::Dict(one));
    }
    o.set(b"Props".to_vec(), pdfcraft_cos::Object::Dict(objects));
    d.set(crate::OVERRIDES_KEY.to_vec(), pdfcraft_cos::Object::Dict(o));
    match acro.as_ref() {
        Some(r) => doc.set(r, pdfcraft_cos::Object::Dict(d)),
        None => doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), pdfcraft_cos::Object::Dict(d))).unwrap(),
    }
    let read = crate::overrides(&doc);
    assert_eq!(read.props().get("a").map(|m| m.keys().cloned().collect::<Vec<_>>()), Some(vec!["font#0@size".to_string()]));
    assert_eq!(read.props().values().map(BTreeMap::len).sum::<usize>(), MAX_PROP_OVERRIDES);
    assert_eq!(read.dropped, 13, "two not a script's, and the eleven past the limit, counted for a note");
}

/// Edges past the ones a border lists (two here, drawn top/bottom then right/left): a change to
/// edge 3 creates it as a copy of the edge drawn on the left and changes only the left side;
/// the bottom keeps the edge drawn there. A change to edge 2 changes only the bottom. Kept
/// through a save.
#[test]
fn a_change_past_the_listed_edges_changes_only_that_side() {
    let tpl = TEMPLATE.replace(
        r#"<draw name="label""#,
        r#"<field name="two" w="2in" h="0.3in"><border><edge thickness="2pt"><color value="10,20,30"/></edge><edge thickness="1pt"><color value="40,50,60"/></edge></border></field><draw name="label""#,
    );
    let t = crate::parse(&tpl).unwrap().0;
    let colours = |t: &Template| field(t, "two").common.border.as_ref().unwrap().edges.map(|e| e.color.0);
    let (a, b, red) = (rgb(10.0, 20.0, 30.0), rgb(40.0, 50.0, 60.0), rgb(255.0, 0.0, 0.0));
    assert_eq!(colours(&t), [a, b, a, b], "two listed: top/bottom, right/left");
    let ov = overrides("form[0].two[0]", &[("border#0/edge#3/color#0", Some("value"), "255,0,0")]);
    let laid = crate::apply_overrides(&t, &ov);
    assert_eq!(colours(&laid), [a, b, a, red]);
    let edges = field(&laid, "two").common.border.as_ref().unwrap().edges;
    assert_eq!((edges[2].thickness, edges[3].thickness), (2.0, 1.0), "each a copy of the edge drawn on its side");
    let ov2 = overrides("form[0].two[0]", &[("border#0/edge#2/color#0", Some("value"), "255,0,0")]);
    assert_eq!(colours(&crate::apply_overrides(&t, &ov2)), [a, b, red, b], "three listed: the bottom is the third");
    // An edge that is listed is drawn on every side it stands for.
    let ov3 = overrides("form[0].two[0]", &[("border#0/edge#1/color#0", Some("value"), "255,0,0")]);
    assert_eq!(colours(&crate::apply_overrides(&t, &ov3)), [a, red, a, red]);
    // A border with no edge: the change creates the edge it names; drawing follows the pattern
    // over the edges there are.
    let ov4 = overrides("form[0].plain[0]", &[("border#0/edge#1", Some("thickness"), "3pt")]);
    let plain = field(&crate::apply_overrides(&t, &ov4), "plain").common.border.clone().unwrap();
    assert_eq!(plain.edges.map(|e| e.thickness), [0.5, 3.0, 0.5, 3.0]);
    // Saved and read again.
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(crate::fixtures::shell(&tpl))).unwrap();
    crate::set_overrides(&mut doc, &ov).unwrap();
    let bytes = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
    let reopened = pdfcraft_cos::Document::open(std::sync::Arc::new(bytes)).unwrap();
    assert_eq!(colours(&crate::apply_overrides(&t, &crate::overrides(&reopened))), [a, b, a, red]);
}

/// Each instance of a repeating subform is drawn with its own property changes: a fill and a
/// mandatory flag set on the third row's cell, and an edge colour on the second row, change
/// those rows only; the live form scripts read agrees.
#[test]
fn each_instance_of_a_repeating_subform_has_its_own_changes() {
    let tpl = TEMPLATE.replace(r#"<occur min="1" max="3"/>"#, r#"<occur min="3" max="3"/>"#);
    let t = crate::parse(&tpl).unwrap().0;
    let mut ov = overrides(
        "form[0].rows[0].row[2].cell[0]",
        &[("border#0/fill#0/color#0", Some("value"), "255,0,0"), ("validate#0", Some("nullTest"), "error")],
    );
    // The first instance's own: green and mandatory, and on none of the others.
    ov.set_prop("form[0].rows[0].row[0].cell[0]", "border#0/fill#0/color#0", Some("value"), "0,128,0").unwrap();
    ov.set_prop("form[0].rows[0].row[0].cell[0]", "validate#0", Some("nullTest"), "error").unwrap();
    ov.set_prop("form[0].rows[0].row[1]", "border#0/edge#0/color#0", Some("value"), "0,0,255").unwrap();
    let form = laid_out(&t, &ov);
    let items: Vec<&Item> = form.pages.iter().flat_map(|p| p.items.iter()).collect();
    let required: Vec<(String, bool)> = items
        .iter()
        .filter_map(|i| match i {
            Item::Widget(w) if w.som.contains(".row[") => Some((w.som.clone(), w.required)),
            _ => None,
        })
        .collect();
    assert_eq!(
        required,
        [
            ("form[0].rows[0].row[0].cell[0]".to_string(), true),
            ("form[0].rows[0].row[1].cell[0]".to_string(), false),
            ("form[0].rows[0].row[2].cell[0]".to_string(), true),
        ]
    );
    let (red, green, blue) = (rgb(255.0, 0.0, 0.0), rgb(0.0, 128.0, 0.0), rgb(0.0, 0.0, 255.0));
    let count = |c: [f64; 3]| items.iter().filter(|i| matches!(i, Item::Fill { color, .. } if *color == c)).count();
    let blue_lines = items.iter().filter(|i| matches!(i, Item::Line { color, .. } if *color == blue)).count();
    assert_eq!((count(red), count(green), blue_lines), (1, 1, 4), "one red cell, one green cell, one row's four edges blue");
    // The live form scripts read agrees, instance by instance.
    let live = crate::form_tree(&t, None, &ov);
    let rows = &live.root.children.iter().find(|n| n.name == "rows").unwrap().children;
    let fill_of = |n: &crate::FormNode| {
        n.children[0].props.elements.iter().find(|e| e.tag == "border").map(|b| b.children[0].children[0].attrs[0].1.clone()).unwrap_or_default()
    };
    assert_eq!(rows.iter().map(fill_of).collect::<Vec<_>>(), ["0,128,0", "", "255,0,0"]);
}

/// The form laid out as `rerender` does: the template with the changes of its single
/// objects, each instance of a repeating subform with its own.
fn laid_out(t: &Template, ov: &Overrides) -> crate::Form {
    let applied = crate::apply_overrides_split(t, ov);
    crate::layout::layout_with_overrides(&applied.template, None, ov, &applied.per_instance).unwrap()
}

fn one_edge() -> Template {
    let tpl = TEMPLATE.replace(
        r#"<draw name="label""#,
        r#"<field name="one" w="2in" h="0.3in"><border><edge thickness="1pt"><color value="0,0,0"/></edge></border></field><draw name="label""#,
    );
    crate::parse(&tpl).unwrap().0
}

/// A change refreshed in place on the live form gives what building it again gives: the bottom
/// (not listed) made red first leaves edge 1 a black copy on the way; the listed edge made green
/// next makes the sides it stands for green, edge 1 too, as the layout draws them.
#[test]
fn a_change_refreshed_in_place_gives_what_a_fresh_build_gives() {
    let t = one_edge();
    let mut ov = overrides("form[0].one[0]", &[("border#0/edge#2/color#0", Some("value"), "255,0,0")]);
    let mut live = crate::form_tree(&t, None, &ov);
    ov.set_prop("form[0].one[0]", "border#0/edge#0/color#0", Some("value"), "0,128,0").unwrap();
    assert!(crate::refresh_props(&mut live.root, &ov, "form[0].one[0]"));
    let fresh = crate::form_tree(&t, None, &ov);
    let one = |f: &crate::LiveForm| f.root.children.iter().find(|n| n.name == "one").unwrap().props.clone();
    assert_eq!(one(&live), one(&fresh));
}

/// A field whose property tree the reader cut short keeps the display picture the template
/// gives when a change is refreshed in place, as a fresh build keeps it.
#[test]
fn a_cut_tree_keeps_the_templates_display_picture_when_refreshed() {
    // More edges than the reader keeps elements, before the format: the cut falls on it.
    let edges = "<edge/>".repeat(300);
    let tpl = TEMPLATE.replace(
        r#"<draw name="label""#,
        &format!(
            r#"<field name="cut" w="2in" h="0.3in"><border>{edges}</border><format><picture>num{{z9}}</picture></format></field><draw name="label""#
        ),
    );
    let t = crate::parse(&tpl).unwrap().0;
    let cut = |f: &crate::LiveForm| f.root.children.iter().find(|n| n.name == "cut").unwrap().clone();
    let mut live = crate::form_tree(&t, None, &Overrides::default());
    assert!(cut(&live).props.cut, "the reader cuts the tree short");
    let ov = overrides("form[0].cut[0]", &[("border#0/fill#0/color#0", Some("value"), "255,0,0")]);
    assert!(crate::refresh_props(&mut live.root, &ov, "form[0].cut[0]"));
    let fresh = crate::form_tree(&t, None, &ov);
    assert_eq!(cut(&live).display_picture, cut(&fresh).display_picture);
    assert!(cut(&fresh).display_picture.is_some());
}

/// Writes across sides: the one listed edge made red, then the bottom (edge 2, not listed)
/// given 2pt. The bottom, and edge 1 made on the way to it, start as the edge drawn on their
/// side as it is by then (red), as the scripting host reads them, so the only thing that
/// changes is the bottom's thickness: every side stays red.
#[test]
fn a_write_past_the_listed_edges_copies_the_drawn_edge_as_written() {
    let t = one_edge();
    let ov = overrides("form[0].one[0]", &[("border#0/edge#0/color#0", Some("value"), "255,0,0"), ("border#0/edge#2", Some("thickness"), "2pt")]);
    let laid = crate::apply_overrides(&t, &ov);
    let edges = field(&laid, "one").common.border.as_ref().unwrap().edges;
    let red = rgb(255.0, 0.0, 0.0);
    assert_eq!(edges.map(|e| e.color.0), [red; 4], "{edges:?}");
    assert_eq!(edges.map(|e| e.thickness), [1.0, 1.0, 2.0, 1.0]);
    // Scripts read what is drawn: the live form's edge 1 is red too.
    let live = crate::form_tree(&t, None, &ov);
    let one = live.root.children.iter().find(|n| n.name == "one").unwrap();
    let border = one.props.elements.iter().find(|e| e.tag == "border").unwrap();
    let colour = |i: usize| border.children.iter().filter(|e| e.tag == "edge").nth(i).unwrap().children[0].attrs[0].1.clone();
    assert_eq!([colour(0), colour(1), colour(2)], ["255,0,0", "255,0,0", "255,0,0"]);
    // A copy the scripting host made comes before the writes to the same property, and keeps
    // what it copied while the edge it came from changes later.
    let mut ov = Overrides::default();
    ov.set_prop_copy("form[0].one[0]", "border#0/edge#2/color#0", Some("value"), "0,0,0").unwrap();
    ov.set_prop("form[0].one[0]", "border#0/edge#0/color#0", Some("value"), "255,0,0").unwrap();
    ov.set_prop("form[0].one[0]", "border#0/edge#2", Some("thickness"), "2pt").unwrap();
    let edges = field(&crate::apply_overrides(&t, &ov), "one").common.border.clone().unwrap().edges;
    assert_eq!(edges[2].color.0, [0.0; 3], "the copy's black, as copied");
}

/// A hostile file (or script) can't make laying the form out slow: a change past
/// [`MAX_PROP_CHANGES_PER_OBJECT`] on one object, an occurrence past `prop_rules::MAX_INDEX` or a
/// path deeper than the reader keeps is refused, an object's tree stays within the template's
/// budget, and the most changes a form keeps, as deep and wide as they may be, are laid out
/// and read in well under a second.
#[test]
fn hostile_property_changes_stay_bounded() {
    // 200 fields, each given as many changes as an object keeps: more than the form keeps.
    let fields: String = (1..200).map(|i| format!(r#"<field name="x{i}" w="1in" h="0.2in"><border><edge/></border></field>"#)).collect();
    let t = crate::parse(&TEMPLATE.replace(r#"<draw name="label""#, &format!(r#"{fields}<draw name="label""#))).unwrap().0;
    let mut ov = Overrides::default();
    let som = |i: usize| if i == 0 { "form[0].plain[0]".to_string() } else { format!("form[0].x{i}[0]") };
    let mut kept = 0usize;
    'all: for o in 0..1_000 {
        for i in 0..MAX_PROP_CHANGES_PER_OBJECT + 1 {
            let path = format!("extras#7/a{i}#7/b#7/c#7/d#7/e#7/f#7/g#7");
            match ov.set_prop(&som(o), &path, Some("x"), "1") {
                Ok(_) => kept += 1,
                Err(PropRefused::Full) if i == MAX_PROP_CHANGES_PER_OBJECT => {}
                Err(PropRefused::Full) => break 'all,
                Err(e) => panic!("{e:?}"),
            }
        }
    }
    assert_eq!(kept, MAX_PROP_OVERRIDES, "the form's limit");
    assert!(ov.props().values().all(|m| m.len() <= MAX_PROP_CHANGES_PER_OBJECT), "each object within its own limit");
    assert_eq!(ov.props().values().filter(|m| m.len() == MAX_PROP_CHANGES_PER_OBJECT).count(), MAX_PROP_OVERRIDES / MAX_PROP_CHANGES_PER_OBJECT);
    assert!(ov.set_prop("form[0].plain[0]", "extras#8", Some("x"), "1").is_err(), "an occurrence past the limit");
    assert!(ov.set_prop("form[0].plain[0]", &["extras#0"; 9].join("/"), Some("x"), "1").is_err(), "deeper than the reader keeps");
    let start = std::time::Instant::now();
    let applied = crate::apply_overrides_split(&t, &ov);
    let form = crate::layout::layout_with_overrides(&applied.template, None, &ov, &applied.per_instance).unwrap();
    let live = crate::form_tree(&t, None, &ov);
    let spent = start.elapsed();
    // Generous for a loaded machine: unbounded work would take minutes.
    assert!(spent < std::time::Duration::from_secs(5), "{spent:?}");
    assert!(!form.pages.is_empty());
    let plain = live.root.children.iter().find(|n| n.name == "plain").unwrap();
    assert!(size_of(&plain.props.elements, 0).0 <= crate::parse::MAX_OBJECT_PROPS);
    assert!(applied.cut > 0, "the object's budget cut it short: {}", applied.cut);
    assert!(!crate::script::property_notes(&ov, &applied).is_empty());
    // Copies the scripting host makes don't count against the form's limit.
    assert!(ov.set_prop("form[0].x199[0]", "border#0/edge#0/color#0", Some("value"), "1,2,3") == Err(PropRefused::Full));
    assert!(ov.set_prop_copy("form[0].x199[0]", "border#0/edge#1/color#0", Some("value"), "1,2,3").is_ok());
}

/// A removed instance's changes go with it, and the later instances' move down with them:
/// presence and property changes alike.
#[test]
fn a_removed_instances_changes_go_and_the_later_ones_move_down() {
    let mut ov = overrides("form[0].rows[0].row[1].cell[0]", &[("border#0/fill#0/color#0", Some("value"), "1,1,1")]);
    ov.set_prop("form[0].rows[0].row[2].cell[0]", "validate#0", Some("nullTest"), "error").unwrap();
    ov.set_prop("form[0].rows[0].row[0]", "border#0/edge#0/color#0", Some("value"), "2,2,2").unwrap();
    ov.set_prop("form[0].rows[0].row[10]", "border#0/edge#0/color#0", Some("value"), "3,3,3").unwrap();
    ov.set_prop("form[0].rows[0].rowx[2]", "border#0/edge#0/color#0", Some("value"), "4,4,4").unwrap();
    ov.presence.insert("form[0].rows[0].row[2]".into(), "hidden".into());
    assert!(ov.remove_instance("form[0].rows[0].row[1]"));
    let keys: Vec<&String> = ov.props().keys().collect();
    assert_eq!(keys, ["form[0].rows[0].row[0]", "form[0].rows[0].row[1].cell[0]", "form[0].rows[0].row[9]", "form[0].rows[0].rowx[2]"]);
    assert_eq!(ov.props()["form[0].rows[0].row[1].cell[0]"].keys().collect::<Vec<_>>(), ["validate#0@nullTest"]);
    assert_eq!(ov.presence.keys().collect::<Vec<_>>(), ["form[0].rows[0].row[1]"]);
    assert!(!ov.remove_instance("form[0].other[0].row[5]"), "nothing there");
}

/// A template object the reader cut short isn't laid out from its property tree again (the
/// tree lacks what the cut left out): its border stays the template's, scripts still read the
/// change, and a note says so. Changes kept in a file that a script couldn't make are left out
/// with a note too.
#[test]
fn changes_that_cant_be_drawn_or_kept_are_noted() {
    // A validation with 300 message texts ahead of the border: the reader's budget runs out
    // before the border, so the property tree has none.
    let texts: String = (0..300).map(|i| format!("<text name=\"n{i}\">v</text>")).collect();
    let tpl = TEMPLATE.replace(
        r#"<draw name="label""#,
        &format!(r#"<field name="big" w="2in" h="0.3in"><validate><message>{texts}</message></validate><border><edge thickness="2pt"/><edge/></border></field><draw name="label""#),
    );
    let t = crate::parse(&tpl).unwrap().0;
    assert!(field(&t, "big").common.props.cut);
    assert!(!field(&t, "big").common.props.elements.iter().any(|e| e.tag == "border"));
    assert!(field(&t, "big").common.border.is_some());
    let ov = overrides("form[0].big[0]", &[("border#0/edge#0", Some("thickness"), "3pt")]);
    let applied = crate::apply_overrides_split(&t, &ov);
    assert_eq!(field(&applied.template, "big").common.border, field(&t, "big").common.border, "laid out as the template");
    assert_eq!(applied.cut, 1);
    assert!(crate::script::property_notes(&ov, &applied).iter().any(|n| n.contains("not drawn")));
    let mut ov = Overrides::default();
    ov.dropped = 2;
    assert!(crate::script::property_notes(&ov, &crate::apply_overrides_split(&t, &ov)).iter().any(|n| n.contains("2 property change")));
}

/// A field on an unnamed page area is named by the live form as `#pageArea[i]…`: its changes
/// are drawn under that name too.
#[test]
fn an_unnamed_page_areas_fields_take_their_changes() {
    let tpl = TEMPLATE.replace(r#"<pageArea name="p">"#, r#"<pageArea><field name="pf" x="1in" y="0.2in" w="1in" h="0.3in"/>"#);
    let t = crate::parse(&tpl).unwrap().0;
    let live = crate::form_tree(&t, None, &Overrides::default());
    assert!(live.root.children.iter().any(|n| n.som == "#pageArea[0]" && n.children.iter().any(|c| c.som == "#pageArea[0].pf[0]")));
    let ov = overrides("#pageArea[0].pf[0]", &[("border#0/fill#0/color#0", Some("value"), "255,0,0")]);
    let form = laid_out(&t, &ov);
    let red = rgb(255.0, 0.0, 0.0);
    assert!(form.pages.iter().flat_map(|p| p.items.iter()).any(|i| matches!(i, Item::Fill { color, .. } if *color == red)));
}

fn items(form: &crate::Form) -> Vec<&Item> {
    form.pages.iter().flat_map(|p| p.items.iter()).collect()
}

/// A fill's pattern, gradient or stipple (XFA 3.3 p. 734, 806, 770, 820, 839), set by a script
/// on a box and on a widget: hatching strokes in the pattern's colour over the fill, bands
/// from the fill colour to the gradient's, the stipple's blend. A widget's background is then
/// page content under it.
#[test]
fn fill_patterns_gradients_and_stipples_are_drawn() {
    let t = template();
    let red = rgb(255.0, 0.0, 0.0);
    let grey = rgb(200.0, 200.0, 200.0);
    let at = |o: &[(&str, Option<&str>, &str)]| laid_out(&t, &overrides("form[0].boxed[0]", o));
    // Horizontal hatching: red strokes across the box, over the grey fill.
    let form = at(&[("border#0/fill#0/pattern#0", Some("type"), "horizontal"), ("border#0/fill#0/pattern#0/color#0", Some("value"), "255,0,0")]);
    let lines: Vec<((f64, f64), (f64, f64))> = items(&form)
        .iter()
        .filter_map(|i| match i {
            Item::Line { from, to, color, .. } if *color == red => Some((*from, *to)),
            _ => None,
        })
        .collect();
    assert!(lines.len() >= 4 && lines.iter().all(|(f, t)| f.1 == t.1), "{lines:?}");
    assert!(items(&form).iter().any(|i| matches!(i, Item::Fill { color, .. } if *color == grey)), "the fill under it");
    // Diagonal strokes stay inside the box.
    let form = at(&[("border#0/fill#0/pattern#0", Some("type"), "crossDiagonal")]);
    let field = form.pages[0].items.iter().find_map(|i| match i {
        Item::Fill { rect, color } if *color == grey => Some(*rect),
        _ => None,
    });
    let field = field.unwrap();
    let diagonals: Vec<&Item> =
        items(&form).into_iter().filter(|i| matches!(i, Item::Line { from, to, .. } if from.0 != to.0 && from.1 != to.1)).collect();
    assert!(diagonals.len() > 4);
    for d in diagonals {
        if let Item::Line { from, to, .. } = d {
            for p in [from, to] {
                assert!(
                    p.0 >= field.x - 0.01 && p.0 <= field.right() + 0.01 && p.1 >= field.y - 0.01 && p.1 <= field.bottom() + 0.01,
                    "{p:?} in {field:?}"
                );
            }
        }
    }
    // A linear gradient to red: bands from grey to red, left to right.
    let form = at(&[("border#0/fill#0/linear#0", Some("type"), "toRight"), ("border#0/fill#0/linear#0/color#0", Some("value"), "255,0,0")]);
    let bands: Vec<(f64, [f64; 3])> = items(&form)
        .iter()
        .filter_map(|i| match i {
            Item::Fill { rect, color } if rect.w < 10.0 => Some((rect.x, *color)),
            _ => None,
        })
        .collect();
    assert_eq!(bands.len(), 64);
    assert!(bands.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1[0] <= w[1].1[0]), "redder to the right");
    let rects: Vec<Rect> = items(&form)
        .iter()
        .filter_map(|i| match i {
            Item::Fill { rect, .. } if rect.w < 10.0 => Some(*rect),
            _ => None,
        })
        .collect();
    assert!(rects.windows(2).all(|w| w[0].right() > w[1].x + 0.1), "each band reaches under the next: no seam shows between them");
    assert!(rects.last().is_some_and(|r| (r.right() - rects[0].x - 144.0).abs() < 0.01), "and the last ends at the box's edge");
    // A stipple at 50%: half way from grey to red.
    let form = at(&[("border#0/fill#0/stipple#0", Some("rate"), "50"), ("border#0/fill#0/stipple#0/color#0", Some("value"), "255,0,0")]);
    let half = std::array::from_fn::<f64, 3, _>(|i| (grey[i] + red[i]) / 2.0);
    assert!(items(&form).iter().any(|i| matches!(i, Item::Fill { color, .. } if *color == half)));
    // A radial gradient: nested ovals.
    let form = at(&[("border#0/fill#0/radial#0/color#0", Some("value"), "255,0,0")]);
    assert_eq!(items(&form).iter().filter(|i| matches!(i, Item::Rounded(_))).count(), 64);
    // On a widget: no background of its own, the pattern on the page under it.
    let ov = overrides(
        "form[0].plain[0]",
        &[
            ("ui#0/textEdit#0/border#0/fill#0/color#0", Some("value"), "200,200,200"),
            ("ui#0/textEdit#0/border#0/fill#0/pattern#0/color#0", Some("value"), "255,0,0"),
        ],
    );
    let form = laid_out(&t, &ov);
    let w = items(&form)
        .iter()
        .find_map(|i| match i {
            Item::Widget(w) if w.som == "form[0].plain[0]" => Some((**w).clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(w.border.fill, None);
    assert!(items(&form).iter().any(|i| matches!(i, Item::Fill { rect, color } if *color == grey && *rect == w.rect)));
    assert!(items(&form).iter().any(|i| matches!(i, Item::Line { color, .. } if *color == red)));
}

/// `font.lineThrough` (XFA 3.3 p. 742) draws a line through the text a draw shows, two for 2.
#[test]
fn line_through_strikes_the_text() {
    let t = template();
    let lines = |v: &str| {
        let form = laid_out(&t, &overrides("form[0].label[0]", &[("font#0", Some("lineThrough"), v)]));
        let text = items(&form)
            .iter()
            .find_map(|i| match i {
                Item::Text(s) if s.text.contains("Hello") => Some(s.clone()),
                _ => None,
            })
            .unwrap();
        items(&form)
            .iter()
            .filter(|i| matches!(i, Item::Line { from, to, .. } if from.1 == to.1 && from.1 < text.baseline && from.1 > text.baseline - text.face.size * 0.6 && (from.0 - text.x).abs() < 0.01))
            .count()
    };
    assert_eq!((lines("0"), lines("1"), lines("2")), (0, 1, 2));
}

/// A field's format test and its calculation's override, as the template and a script's
/// changes make them (for a row's field, that row's changes), for the checks after the user
/// changes its value; the template's defaults otherwise (`warning`, XFA 3.3 p. 880).
#[test]
fn a_fields_format_test_and_override_follow_the_changes() {
    use crate::validation_rules::{CalcOverride, Test};
    let tpl = TEMPLATE.replace(
        r#"<field name="plain" w="2in" h="0.3in"><ui><textEdit/></ui></field>"#,
        r#"<field name="plain" w="2in" h="0.3in"><ui><textEdit/></ui><calculate override="warning"><script>1</script><message><text>Calculated</text></message></calculate><validate><picture>text{99}</picture><message><text name="formatTest">Two digits</text></message></validate></field>"#,
    );
    let t = crate::parse(&tpl).unwrap().0;
    let f = crate::field_now(&t, &Overrides::default(), "form[0].plain[0]").unwrap();
    assert_eq!(
        (f.format_test, f.format_picture.as_deref(), f.format_message.as_deref(), f.calc_override, f.calc_message.as_deref()),
        (Test::Warning, Some("text{99}"), Some("Two digits"), CalcOverride::Warning, Some("Calculated"))
    );
    let mut ov = Overrides::default();
    ov.set_prop("form[0].plain[0]", "validate#0", Some("formatTest"), "error").unwrap();
    ov.set_prop("form[0].plain[0]", "validate#0/message#0/text#0", None, "Changed").unwrap();
    ov.set_prop("form[0].rows[0].row[2].cell[0]", "validate#0/picture#0", None, "num{9}").unwrap();
    let f = crate::field_now(&t, &ov, "form[0].plain[0]").unwrap();
    assert_eq!((f.format_test, f.format_message.as_deref()), (Test::Error, Some("Changed")));
    let row = |i: usize| crate::field_now(&t, &ov, &format!("form[0].rows[0].row[{i}].cell[0]")).unwrap().format_picture;
    assert_eq!((row(0), row(2)), (None, Some("num{9}".to_string())));
    // The user's override of the calculation, kept with the form.
    assert!(!crate::value_overridden(&t, &ov, "form[0].plain[0]"));
    assert!(crate::mark_value_override(&mut ov, "form[0].plain[0]"));
    assert!(crate::value_overridden(&t, &ov, "form[0].plain[0]"));
}

/// A field's `font.lineThrough` strikes its value (the widget is laid out to draw the lines),
/// not its caption when the caption's own font says `lineThrough="0"` (a caption's font takes
/// what it doesn't set from its field's).
#[test]
fn a_fields_line_through_strikes_its_value_not_its_caption() {
    let tpl = TEMPLATE.replace(
        r#"<draw name="label""#,
        r#"<field name="w6" w="5in" h="0.5in"><ui><textEdit/></ui><font typeface="Helvetica" size="18pt"/><value><text>struck text</text></value><caption reserve="1.2in"><font typeface="Helvetica" size="10pt" lineThrough="0"/><value><text>W6 caption</text></value></caption></field><draw name="label""#,
    );
    let t = crate::parse(&tpl).unwrap().0;
    let form = laid_out(&t, &overrides("form[0].w6[0]", &[("font#0", Some("lineThrough"), "1")]));
    let widget = items(&form).into_iter().find_map(|i| match i {
        Item::Widget(w) if w.som == "form[0].w6[0]" => Some(w.clone()),
        _ => None,
    });
    assert_eq!(widget.map(|w| w.face.line_through), Some(1));
    let caption = items(&form)
        .into_iter()
        .find_map(|i| match i {
            Item::Text(s) if s.text.contains("W6") => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(caption.face.line_through, 0);
    let near: Vec<&Item> = items(&form).into_iter().filter(|i| matches!(i, Item::Line { from, to, color, .. } if from.1 == to.1 && *color == caption.face.color && (from.1 - caption.baseline).abs() < caption.face.size)).collect();
    assert!(near.is_empty(), "{near:?} {caption:?}");
}

/// `para.tabDefault` (XFA 3.3 p. 802) puts the text after a tab at the next tab stop; without
/// it a tab is a space.
#[test]
fn tabs_go_to_the_default_tab_stops() {
    let tpl = TEMPLATE.replace(r#"<value><text>Hello</text></value></draw>"#, "<value><text>a\tb</text></value></draw>");
    let t = crate::parse(&tpl).unwrap().0;
    let xs = |o: &[(&str, Option<&str>, &str)]| {
        let form = laid_out(&t, &overrides("form[0].label[0]", o));
        items(&form)
            .iter()
            .filter_map(|i| match i {
                Item::Text(s) if s.text.trim() == "a" || s.text.trim() == "b" || s.text.contains('a') => Some((s.text.clone(), s.x)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let with = xs(&[("para#0", Some("tabDefault"), "1in")]);
    assert_eq!(with.len(), 2, "{with:?}");
    assert!((with[1].1 - with[0].1 - 72.0).abs() < 0.01, "b at the first stop, an inch from a: {with:?}");
    let without = xs(&[("para#0", Some("hAlign"), "left")]);
    assert!(without.len() == 2 && without[1].1 - without[0].1 < 20.0, "a tab is a space: {without:?}");
}

/// Fields with a caption, a check button with its own on value, a choice list that takes typed
/// text and a calculated field, for the caption, item and calculation changes.
const CAPTIONED: &str = r##"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <field name="named" w="5in" h="0.4in"><ui><textEdit/></ui><font typeface="Helvetica" size="10pt"/><caption reserve="1in"><font typeface="Helvetica" size="10pt" weight="bold"/><value><text>Name:</text></value></caption></field>
 <field name="rich" w="5in" h="0.4in"><ui><textEdit/></ui><caption reserve="2in"><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p>Date of birth:</p></body></exData></value></caption></field>
 <field name="box" w="2in" h="0.4in"><ui><checkButton/></ui><items><integer>1</integer><integer>0</integer></items><caption><value><text>Agree</text></value></caption></field>
 <field name="list" w="2in" h="0.4in"><ui><choiceList textEntry="1"/></ui><items><text>A</text><text>B</text></items></field>
 <field name="sum" w="2in" h="0.4in"><ui><numericEdit/></ui><calculate override="error"><script>1 + 1</script></calculate></field>
 <field name="bare" w="2in" h="0.4in"><ui><textEdit/></ui></field>
</subform></template>"##;

/// The text items laid out whose text contains `needle`.
fn texts<'a>(form: &'a crate::Form, needle: &str) -> Vec<&'a crate::text::Span> {
    items(form).into_iter().filter_map(|i| if let Item::Text(s) = i { s.text.contains(needle).then_some(s) } else { None }).collect()
}

/// What a script changes on a caption is drawn: its font's colour, weight, posture and
/// underline, its reserve (the value moves over) and its rich text's words; a field the template
/// gives no caption still has none (an empty caption would take room). An item list's entry, a
/// choice list's typed text and a calculation's override reach the field, and a field's font
/// posture its value.
#[test]
fn caption_item_and_calculation_changes_take_effect() {
    let t = crate::parse(CAPTIONED).unwrap().0;
    let before = laid_out(&t, &Overrides::default());
    let label = texts(&before, "Name:")[0].clone();
    assert!(label.face.bold && !label.face.italic && !label.face.underline);
    let widget_x = |form: &crate::Form, som: &str| {
        items(form).into_iter().find_map(|i| if let Item::Widget(w) = i { (w.som == som).then_some(w.rect.x) } else { None }).unwrap()
    };
    let mut ov = Overrides::default();
    for (path, attr, value) in [
        ("caption#0/font#0/fill#0/color#0", Some("value"), "244,15,55"),
        ("caption#0/font#0", Some("weight"), "normal"),
        ("caption#0/font#0", Some("posture"), "italic"),
        ("caption#0/font#0", Some("underline"), "1"),
        ("caption#0", Some("reserve"), "2in"),
        ("font#0", Some("posture"), "italic"),
    ] {
        ov.set_prop("form[0].named[0]", path, attr, value).unwrap();
    }
    ov.set_prop("form[0].rich[0]", "caption#0/value#0/exData#0", None, "Date of birth* :").unwrap();
    ov.set_prop("form[0].box[0]", "items#0/integer#0", None, "Agree").unwrap();
    ov.set_prop("form[0].list[0]", "ui#0/choiceList#0", Some("textEntry"), "0").unwrap();
    ov.set_prop("form[0].sum[0]", "calculate#0", Some("override"), "warning").unwrap();
    ov.set_prop("form[0].bare[0]", "caption#0/font#0", Some("weight"), "bold").unwrap();
    let after = laid_out(&t, &ov);
    let label = texts(&after, "Name:")[0].clone();
    assert_eq!(label.face.color, rgb(244.0, 15.0, 55.0));
    assert!(!label.face.bold && label.face.italic && label.face.underline, "{label:?}");
    assert!((widget_x(&after, "form[0].named[0]") - widget_x(&before, "form[0].named[0]") - 72.0).abs() < 0.01, "the reserve grew by 1in");
    assert!(texts(&after, "Date of birth* :").len() == 1 && texts(&after, "Date of birth:").is_empty());
    let widget = |form: &crate::Form, som: &str| {
        items(form).into_iter().find_map(|i| if let Item::Widget(w) = i { (w.som == som).then(|| (**w).clone()) } else { None }).unwrap()
    };
    assert!(matches!(widget(&before, "form[0].box[0]").kind, crate::layout::WidgetKind::CheckBox { ref on, .. } if on == "1"));
    assert!(matches!(widget(&after, "form[0].box[0]").kind, crate::layout::WidgetKind::CheckBox { ref on, .. } if on == "Agree"));
    assert!(matches!(widget(&before, "form[0].list[0]").kind, crate::layout::WidgetKind::Choice { editable: true, .. }));
    assert!(matches!(widget(&after, "form[0].list[0]").kind, crate::layout::WidgetKind::Choice { editable: false, .. }));
    assert!(widget(&after, "form[0].named[0]").face.italic && !widget(&before, "form[0].named[0]").face.italic);
    // No caption is made where the template has none, so the field keeps its whole width.
    assert_eq!(widget_x(&after, "form[0].bare[0]"), widget_x(&before, "form[0].bare[0]"));
    let applied = crate::apply_overrides_split(&t, &ov);
    let sum = field(&applied.template, "sum");
    assert_eq!(sum.calc_override, crate::validation_rules::CalcOverride::Warning);
    assert!(field(&applied.template, "bare").caption.is_none());
}

/// Review: the field a check after the user's entry reads (`field_now`) is restyled alone, not
/// the whole form: on a form with 5,000 changed fields, 200 look-ups take well under a second
/// (they rebuilt every field's properties each time before). The field still has its own
/// changes, and a calculation override a script wrote counts (`value_overridden`).
#[test]
fn checks_after_an_entry_restyle_only_the_field() {
    let fields: String = (0..5_000).map(|i| format!(r#"<field name="f{i}" w="1in" h="0.3in"><ui><textEdit/></ui></field>"#)).collect();
    let tpl = TEMPLATE.replace(r#"<draw name="label""#, &format!(r#"{fields}<draw name="label""#));
    let t = crate::parse(&tpl).unwrap().0;
    let mut ov = Overrides::default();
    for i in 0..5_000 {
        ov.set_prop(&format!("form[0].f{i}[0]"), "border#0/fill#0/color#0", Some("value"), "1,2,3").unwrap();
    }
    ov.set_prop("form[0].plain[0]", "validate#0", Some("formatTest"), "error").unwrap();
    ov.set_prop("form[0].plain[0]", "value#0", Some("override"), "1").unwrap();
    let start = std::time::Instant::now();
    for _ in 0..200 {
        let f = crate::field_now(&t, &ov, "form[0].plain[0]").unwrap();
        assert_eq!(f.format_test, crate::validation_rules::Test::Error);
    }
    assert!(start.elapsed() < std::time::Duration::from_secs(2), "{:?}", start.elapsed());
    assert!(crate::value_overridden(&t, &ov, "form[0].plain[0]"), "a script's value@override counts");
    assert!(!crate::value_overridden(&t, &Overrides::default(), "form[0].plain[0]"));
}

/// Review: changes the pages don't show (a screen-reader text, metadata, named values) are kept
/// in the property tree without reading the object again or laying the form out; the others
/// are read again.
#[test]
fn changes_the_pages_dont_show_need_no_restyle() {
    for key in ["assist#0/speak#0", "assist#0/speak#0@disable", "desc#0/text#1", "extras#0@x", "=desc#0/text#0"] {
        assert!(!draws(key), "{key}");
    }
    for key in ["assist#0/toolTip#0", "border#0/fill#0/color#0@value", "caption#0/font#0@weight", "=border#0/edge#1@thickness"] {
        assert!(draws(key), "{key}");
    }
    // The tree still takes the change.
    let t = template();
    let ov = overrides("form[0].plain[0]", &[("assist#0/speak#0", None, "Say this")]);
    let live = crate::form_tree(&t, None, &ov);
    let plain = live.root.children.iter().find(|n| n.name == "plain").unwrap();
    let speak = plain.props.elements.iter().find(|e| e.tag == "assist").and_then(|a| a.children.iter().find(|c| c.tag == "speak"));
    assert_eq!(speak.and_then(|s| s.text.as_deref()), Some("Say this"));
}
