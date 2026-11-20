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

/// A document of [`TEMPLATE`] whose kept property changes are `props`, as a file holding them.
fn doc_with_props(props: pdfcraft_cos::Dict) -> pdfcraft_cos::Document {
    doc_with_kept(b"Props", props)
}

/// A document of [`TEMPLATE`] whose kept overrides hold `kept` under `key`.
fn doc_with_kept(key: &[u8], kept: pdfcraft_cos::Dict) -> pdfcraft_cos::Document {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(crate::fixtures::shell(TEMPLATE))).unwrap();
    keep(&mut doc, key, kept);
    doc
}

/// Put `kept` under `key` in the overrides `doc` keeps.
fn keep(doc: &mut pdfcraft_cos::Document, key: &[u8], kept: pdfcraft_cos::Dict) {
    let mut some = Overrides::default();
    some.access.insert("x".into(), "readOnly".into());
    crate::set_overrides(doc, &some).unwrap();
    let root = doc.root().unwrap();
    let acro = doc.get(root).as_dict().unwrap().get(b"AcroForm").cloned().unwrap();
    let mut d = doc.resolve(&acro).as_dict().unwrap().clone();
    let mut o = doc.resolve(d.get(crate::OVERRIDES_KEY).unwrap()).as_dict().unwrap().clone();
    o.set(key.to_vec(), pdfcraft_cos::Object::Dict(kept));
    d.set(crate::OVERRIDES_KEY.to_vec(), pdfcraft_cos::Object::Dict(o));
    match acro.as_ref() {
        Some(r) => doc.set(r, pdfcraft_cos::Object::Dict(d)),
        None => doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), pdfcraft_cos::Object::Dict(d))).unwrap(),
    }
}

/// `text` as a file holds it in UTF-16 (ISO 32000-2 7.9.2.2), whatever its characters.
fn utf16(text: &str) -> pdfcraft_cos::Object {
    let bytes = [0xFE, 0xFF].into_iter().chain(text.encode_utf16().flat_map(u16::to_be_bytes)).collect();
    pdfcraft_cos::Object::String(pdfcraft_cos::PdfString { bytes, hex: false })
}

/// One path's changes: the tool tip `value`.
fn tip_of(value: pdfcraft_cos::Object) -> pdfcraft_cos::Object {
    pdfcraft_cos::Object::Dict(pdfcraft_cos::Dict::from_iter([(b"assist#0/toolTip#0".to_vec(), value)]))
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
    assert!(check("ui#0/textEdit#0/comb#0", Some("numberOfCells"), "many").is_err());
    assert!(check("occur#0", Some("max"), "-1").is_err(), "how often a subform occurs is not changed");
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

/// A subform made to start on a new page.
#[test]
fn breaks_follow_their_properties() {
    let t = template();
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
    let read = crate::overrides(&doc_with_props(objects));
    assert_eq!(read.props().get("a").map(|m| m.keys().cloned().collect::<Vec<_>>()), Some(vec!["font#0@size".to_string()]));
    assert_eq!(read.props().values().map(BTreeMap::len).sum::<usize>(), MAX_PROP_OVERRIDES);
    assert_eq!(read.dropped, 13, "two not a script's, and the eleven past the limit, counted for a note");
}

/// Copies of drawn edges and corners have a limit of their own, as many as the changes: a form
/// holding as many of each as it may is saved and read back whole (reading looks at that many
/// entries and no more), and a copy past the limit is refused, not kept to be lost on reading.
#[test]
fn copies_are_kept_within_what_reading_takes_back() {
    let mut ov = Overrides::default();
    for i in 0..MAX_PROP_OVERRIDES {
        assert_eq!(ov.set_prop_copy(&format!("c{i}"), "border#0/edge#1", Some("presence"), "visible"), Ok(true));
        assert_eq!(ov.set_prop(&format!("c{i}"), "border#0/edge#1", Some("thickness"), "1pt"), Ok(true));
    }
    assert_eq!(ov.set_prop_copy("one-more", "border#0/edge#1", Some("presence"), "visible"), Err(PropRefused::Full));
    assert_eq!(ov.set_prop_copy("c0", "border#0/edge#1", Some("presence"), "hidden"), Ok(true), "a kept copy can still change");
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(crate::fixtures::shell(TEMPLATE))).unwrap();
    crate::set_overrides(&mut doc, &ov).unwrap();
    let read = crate::overrides(&doc);
    assert_eq!(read.props(), ov.props());
    assert_eq!(read.dropped, 0);
}

/// Reading a file's property changes stops looking at entries once it has looked at as many as
/// the limits allow: of 30 paths holding 1,500 changes each, 20,000 are looked at and the rest
/// counted unread. A path's dictionary named by reference is not followed, whatever it names
/// (here, 300 times, one of 10,000 entries): each is counted as one refused.
#[test]
fn reading_change_dictionaries_stops_at_the_limit() {
    let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(crate::fixtures::shell(TEMPLATE))).unwrap();
    let entries = |n: usize| {
        let mut sets = pdfcraft_cos::Dict::new();
        for i in 0..n {
            sets.set(format!("extras#0@k{i}").into_bytes(), pdfcraft_cos::Object::String(pdfcraft_cos::PdfString::text("1")));
        }
        sets
    };
    let shared = doc.add(entries(10_000));
    let mut objects = pdfcraft_cos::Dict::new();
    for i in 0..300 {
        objects.set(format!("r{i:03}").into_bytes(), pdfcraft_cos::Object::Ref(shared));
        if i % 10 == 0 {
            objects.set(format!("o{i:03}").into_bytes(), pdfcraft_cos::Object::Dict(entries(1_500)));
        }
    }
    keep(&mut doc, b"Props", objects);
    crate::script::take_entries_looked();
    let read = crate::overrides(&doc);
    let looked = crate::script::take_entries_looked();
    assert!(looked <= 2 * MAX_PROP_OVERRIDES + 1, "{looked}");
    let kept = read.props().values().map(BTreeMap::len).sum::<usize>();
    assert!(kept > 0);
    assert_eq!(kept.saturating_add(read.dropped), 30 * 1_500 + 300, "every entry kept or counted, a reference as one");
}

/// Review r5 (D1): a kept value is measured as the file holds it before it is decoded. The
/// longest that may be kept ([`MAX_PROP_VALUE`] bytes, twice that in UTF-16 with its mark) is
/// read whole; one character longer is refused without being decoded.
#[test]
fn a_value_longer_than_any_kept_is_not_decoded() {
    let longest = "x".repeat(MAX_PROP_VALUE);
    let mut objects = pdfcraft_cos::Dict::new();
    objects.set(b"form[0].plain[0]".to_vec(), tip_of(utf16(&longest)));
    objects.set(b"form[0].boxed[0]".to_vec(), tip_of(utf16(&format!("{longest}x"))));
    let doc = doc_with_props(objects);
    crate::script::take_kept_read();
    let read = crate::overrides(&doc);
    assert_eq!(crate::script::take_kept_read(), 1, "the longer one is not decoded");
    assert_eq!(read.props().get("form[0].plain[0]").and_then(|m| m.get("assist#0/toolTip#0")), Some(&longest));
    assert_eq!(read.props().len(), 1);
    assert_eq!(read.dropped, 1);
}

/// Review r5 (D1): no more bytes of kept values are decoded than all that may be kept could
/// take in a file. Of 300 values of the longest kept length, as many are decoded as that
/// allows (65), not all of them to refuse each past the limit; every one is kept or counted.
#[test]
fn values_are_decoded_within_what_all_kept_could_take() {
    let longest = utf16(&"x".repeat(MAX_PROP_VALUE));
    let mut objects = pdfcraft_cos::Dict::new();
    for i in 0..300 {
        objects.set(format!("f{i:03}").into_bytes(), tip_of(longest.clone()));
    }
    let doc = doc_with_props(objects);
    crate::script::take_kept_read();
    let read = crate::overrides(&doc);
    let decoded = crate::script::take_kept_read();
    let most = (2 * MAX_PROP_OVERRIDE_BYTES + 4 * MAX_PROP_OVERRIDES) / (2 + 2 * MAX_PROP_VALUE);
    assert!(decoded <= most, "{decoded} > {most}");
    assert!(read.props().len() >= MAX_PROP_OVERRIDE_BYTES / MAX_PROP_VALUE - 1, "{}", read.props().len());
    assert_eq!(read.props().len().saturating_add(read.dropped), 300);
}

/// The limits on what is decoded take nothing a script may keep: values with characters a file
/// holds in UTF-16, as long and as many as the limits on kept changes allow, are saved and read
/// back whole.
#[test]
fn all_that_may_be_kept_is_read_back_whatever_its_characters() {
    // The longest value, in the characters that grow most in a file; and short ones, many.
    for value in [format!("{}\u{e9}", "x".repeat(MAX_PROP_VALUE - 2)), format!("\u{e9}{}", "x".repeat(98))] {
        let mut ov = Overrides::default();
        let mut kept = 0usize;
        while ov.set_prop(&format!("f{kept}"), "assist#0/toolTip#0", None, &value).is_ok() {
            kept += 1;
        }
        assert!(kept > 0 && kept <= MAX_PROP_OVERRIDES, "{kept}");
        let mut doc = pdfcraft_cos::Document::open(std::sync::Arc::new(crate::fixtures::shell(TEMPLATE))).unwrap();
        crate::set_overrides(&mut doc, &ov).unwrap();
        let read = crate::overrides(&doc);
        assert_eq!(read.dropped, 0, "{kept} values of {} bytes", value.len());
        assert!(read.props() == ov.props(), "{kept} values of {} bytes", value.len());
    }
}

/// Review r5 (D1): a kept presence or access longer than any a script sets is not decoded; the
/// longest read is 64 bytes as the file holds it. (A typed-over mark is `1` or not read.)
#[test]
fn a_kept_state_longer_than_any_is_not_decoded() {
    for key in [&b"Presence"[..], &b"Access"[..]] {
        let mut kept = pdfcraft_cos::Dict::new();
        kept.set(b"a".to_vec(), utf16("nonInteractive"));
        kept.set(b"b".to_vec(), pdfcraft_cos::Object::String(pdfcraft_cos::PdfString::literal("x".repeat(64))));
        kept.set(b"c".to_vec(), pdfcraft_cos::Object::String(pdfcraft_cos::PdfString::literal("x".repeat(65))));
        let read = crate::overrides(&doc_with_kept(key, kept));
        let map = if key == b"Presence" { &read.presence } else { &read.access };
        assert_eq!(map.get("a").map(String::as_str), Some("nonInteractive"));
        assert_eq!(map.get("b").map(String::len), Some(64));
        assert_eq!(map.get("c"), None);
    }
}

/// Review r5 (D2): a path holding no change is charged as one looked at, without asking whose
/// path it is: after as many of them as the limits allow, nothing more is read.
#[test]
fn paths_holding_nothing_are_charged() {
    let tpl = template();
    for (empty, read_after) in [(2 * MAX_PROP_OVERRIDES - 1, true), (2 * MAX_PROP_OVERRIDES, false)] {
        let mut objects = pdfcraft_cos::Dict::new();
        for i in 0..empty {
            objects.set(format!("form[0].rows[0].row[{i}]").into_bytes(), pdfcraft_cos::Object::Dict(pdfcraft_cos::Dict::new()));
        }
        objects.set(b"form[0].plain[0]".to_vec(), tip_of(utf16("Held")));
        let doc = doc_with_props(objects);
        let _ = (crate::script::take_entries_looked(), crate::script::take_owners_looked());
        let read = crate::script::overrides_of(&doc, &tpl);
        assert_eq!(crate::script::take_entries_looked(), usize::from(read_after), "{empty} empty");
        assert!(crate::script::take_owners_looked() <= 1, "{empty} empty");
        assert_eq!(read.props().len(), usize::from(read_after), "{empty} empty");
        assert_eq!(read.dropped, usize::from(!read_after), "{empty} empty");
    }
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

/// The form laid out as `rerender` does: the template with the kept changes applied.
fn laid_out(t: &Template, ov: &Overrides) -> crate::Form {
    let applied = crate::apply_overrides_split(t, ov);
    crate::layout::layout_with_overrides(&applied.template, None, ov).unwrap()
}

/// A radio button takes the changes to its tooltip and on value, as its other properties: the
/// group's tooltip stands in only for a button without one, and the template's on value no
/// longer names the button.
#[test]
fn a_radio_buttons_tooltip_and_on_value_follow_its_changes() {
    let t = crate::parse(
        r##"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="once" layout="tb">
  <exclGroup name="g" layout="lr-tb"><assist><toolTip>Group</toolTip></assist>
   <field name="a" w="1in" h="0.3in"><ui><checkButton shape="round"/></ui><items><text>A</text></items><assist><toolTip>Old</toolTip></assist></field>
   <field name="b" w="1in" h="0.3in"><ui><checkButton shape="round"/></ui><items><text>B</text></items></field>
  </exclGroup></subform>
</subform></template>"##,
    )
    .unwrap()
    .0;
    let before = laid_out(&t, &Overrides::default());
    let mut ov = Overrides::default();
    ov.set_prop("form[0].once[0].g[0].a[0]", "assist#0/toolTip#0", None, "New").unwrap();
    ov.set_prop("form[0].once[0].g[0].a[0]", "items#0/text#0", None, "Z").unwrap();
    let form = laid_out(&t, &ov);
    let radio = |form: &crate::Form, som: &str| {
        items(form)
            .into_iter()
            .find_map(|i| match i {
                Item::Widget(w) if w.som == som => match &w.kind {
                    crate::layout::WidgetKind::Radio { on, .. } => Some((on.clone(), w.tooltip.clone())),
                    _ => None,
                },
                _ => None,
            })
            .unwrap()
    };
    assert_eq!(radio(&before, "form[0].once[0].g[0].a[0]"), ("A".to_string(), Some("Old".to_string())));
    assert_eq!(radio(&form, "form[0].once[0].g[0].a[0]"), ("Z".to_string(), Some("New".to_string())));
    assert_eq!(radio(&form, "form[0].once[0].g[0].b[0]"), ("B".to_string(), Some("Group".to_string())));
}

/// A button's box is its field border, so a fill there that is more than a colour (here a
/// gradient) is drawn under it as page content, as a widget's is: the button's own background
/// is left clear for it, and it was then transparent.
#[test]
fn a_buttons_gradient_fill_is_drawn_under_it() {
    let tpl = TEMPLATE.replace(
        r#"<draw name="label""#,
        r#"<field name="push" w="1in" h="0.3in"><ui><button/></ui><border><edge/><fill><color value="200,200,200"/><linear type="toRight"><color value="255,0,0"/></linear></fill></border><caption><value><text>Go</text></value></caption></field><draw name="label""#,
    );
    let t = crate::parse(&tpl).unwrap().0;
    let form = laid_out(&t, &Overrides::default());
    let button =
        items(&form).into_iter().find_map(|i| if let Item::Widget(w) = i { (w.som == "form[0].push[0]").then_some(w.rect) } else { None }).unwrap();
    let grey = rgb(200.0, 200.0, 200.0);
    assert!(
        items(&form).iter().any(
            |i| matches!(i, Item::Fill { rect, color } if *color == grey && (rect.x - button.x).abs() < 0.01 && (rect.y - button.y).abs() < 0.01)
        ),
        "the gradient's base colour under the button"
    );
}

/// The entries of a field's item lists aren't charged to the object's budget when a change is
/// applied, as the template reader doesn't charge them: a choice list of 300 countries still
/// takes a font change, a new border fill and a change to one of its entries.
#[test]
fn a_long_item_list_leaves_room_for_changes() {
    let entries: String = (0..300).map(|i| format!("<text>Country number {i:03} with a long name</text>")).collect();
    let tpl = TEMPLATE.replace(
        r#"<draw name="label""#,
        &format!(r#"<field name="countries" w="2in" h="0.3in"><ui><choiceList/></ui><items>{entries}</items><items save="1">{entries}</items></field><draw name="label""#),
    );
    let t = crate::parse(&tpl).unwrap().0;
    let props = &field(&t, "countries").common.props;
    assert!(!props.cut);
    assert!(size_of(&props.elements, 0).0 > crate::parse::MAX_OBJECT_PROPS, "the lists alone are past the budget");
    let (after, cut) = apply_props(
        props,
        &sets(&[("font#0", Some("size"), "14pt"), ("border#0/fill#0/color#0", Some("value"), "1,2,3"), ("items#0/text#5", None, "Elsewhere")]),
    );
    assert!(!cut);
    let el = |tag: &str| after.elements.iter().find(|e| e.tag == tag).unwrap();
    assert_eq!(el("font").attrs.iter().find(|(k, _)| k == "size").map(|(_, v)| v.as_str()), Some("14pt"));
    assert!(el("border").children.iter().any(|c| c.tag == "fill"));
    assert_eq!(el("items").children.get(5).and_then(|c| c.text.as_deref()), Some("Elsewhere"));
}

/// A validation script the template turns off (`scriptTest="disabled"`) runs once a script
/// turns the test on again, and a script turning it off sets it aside the same way.
#[test]
fn a_validation_script_turned_on_again_runs() {
    let tpl = TEMPLATE.replace(
        r#"<draw name="label""#,
        r#"<field name="off" w="2in" h="0.3in"><ui><textEdit/></ui><validate scriptTest="disabled"><script contentType="application/x-javascript">this.rawValue != "x"</script></validate></field><draw name="label""#,
    );
    let t = crate::parse(&tpl).unwrap().0;
    let validates =
        |ov: &Overrides, som: &str| crate::form_tree(&t, None, ov).events.iter().filter(|e| e.som == som && e.activity == "validate").count();
    assert_eq!(validates(&Overrides::default(), "form[0].off[0]"), 0);
    assert_eq!(validates(&overrides("form[0].off[0]", &[("validate#0", Some("scriptTest"), "error")]), "form[0].off[0]"), 1);
    assert_eq!(validates(&Overrides::default(), "form[0].boxed[0]"), 1);
    assert_eq!(validates(&overrides("form[0].boxed[0]", &[("validate#0", Some("scriptTest"), "disabled")]), "form[0].boxed[0]"), 0);
}

/// A validation's script text isn't charged to the object's budget when changes are applied, as
/// the template reader doesn't charge it: a field with a 20 KiB validation script still takes a
/// font change.
#[test]
fn a_long_validation_script_leaves_room_for_changes() {
    let script = format!("this.rawValue != \"x\"; // {}", "x".repeat(20 << 10));
    let tpl = TEMPLATE.replace(
        r#"<draw name="label""#,
        &format!(r#"<field name="checked" w="2in" h="0.3in"><validate><script contentType="application/x-javascript">{script}</script></validate></field><draw name="label""#),
    );
    let t = crate::parse(&tpl).unwrap().0;
    let props = &field(&t, "checked").common.props;
    assert!(!props.cut);
    let (after, cut) = apply_props(props, &sets(&[("font#0", Some("size"), "14pt")]));
    assert!(!cut);
    assert!(after.elements.iter().any(|e| e.tag == "font"));
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

/// A widget's own flags follow a change as the template's do: a text field made multi-line and
/// not to scroll, a radio button drawn square (XFA 3.3 p. 865, p. 650).
#[test]
fn a_widgets_own_flags_follow_their_properties() {
    let t = template();
    let w = |t: &Template, som: &str| widgets(t).into_iter().find(|w| w.som == som).unwrap();
    let before = w(&t, "form[0].plain[0]");
    assert!(!before.multiline && !before.no_scroll);
    let ov = overrides("form[0].plain[0]", &[("ui#0/textEdit#0", Some("multiLine"), "1"), ("ui#0/textEdit#0", Some("vScrollPolicy"), "off")]);
    let got = w(&crate::apply_overrides(&t, &ov), "form[0].plain[0]");
    assert!(got.multiline && got.no_scroll, "{got:?}");
    let radio = |t: &Template| match w(t, "form[0].choice[0].a[0]").kind {
        crate::WidgetKind::Radio { round, .. } => round,
        other => panic!("{other:?}"),
    };
    assert!(radio(&t), "the template's shape=\"round\"");
    let ov = overrides("form[0].choice[0].a[0]", &[("ui#0/checkButton#0", Some("shape"), "square")]);
    assert!(!radio(&crate::apply_overrides(&t, &ov)));
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
                Err(PropRefused::ObjectFull) if i == MAX_PROP_CHANGES_PER_OBJECT => {}
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
    let form = crate::layout::layout_with_overrides(&applied.template, None, &ov).unwrap();
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

/// Presence and access are not drawn per instance yet: scripts read each instance's own (the
/// live form), and the page takes them from the template object, which the first instance of a
/// repeating subform stands for. So a cell hidden on a later row still shows, and a cell hidden
/// on the first row is hidden on every row.
#[test]
fn presence_set_on_one_row_is_that_rows_to_scripts_and_the_first_rows_on_the_page() {
    let tpl = TEMPLATE.replace(r#"<occur min="1" max="3"/>"#, r#"<occur min="3" max="3"/>"#);
    let t = crate::parse(&tpl).unwrap().0;
    let cells = |ov: &Overrides| {
        let form = laid_out(&t, ov);
        form.pages.iter().flat_map(|p| p.items.iter()).filter(|i| matches!(i, Item::Widget(w) if w.som.contains(".row["))).count()
    };
    let read = |ov: &Overrides| -> Vec<String> {
        let live = crate::form_tree(&t, None, ov);
        let rows = &live.root.children.iter().find(|n| n.name == "rows").unwrap().children;
        rows.iter().map(|r| r.children[0].presence.clone()).collect()
    };
    let mut ov = Overrides::default();
    ov.presence.insert("form[0].rows[0].row[1].cell[0]".into(), "hidden".into());
    assert_eq!(read(&ov), ["visible", "hidden", "visible"]);
    assert_eq!(cells(&ov), 3, "a later row's is not drawn");
    let mut ov = Overrides::default();
    ov.presence.insert("form[0].rows[0].row[0].cell[0]".into(), "hidden".into());
    assert_eq!(read(&ov), ["hidden", "visible", "visible"]);
    assert_eq!(cells(&ov), 0, "the first row's is every row's on the page");
}

/// Presence and access are kept by path alone, also at a path more than one object has (no
/// property change is kept there): two unnamed fields of one subform, or the second instance
/// of a repeating subform and a same-named object after it. Set through such a path it is
/// every such object's to scripts, and on the page each template object's that has the path.
#[test]
fn presence_at_a_path_objects_share_is_each_ones() {
    let tpl = TEMPLATE.replace(r#"<occur min="1" max="3"/>"#, r#"<occur min="2" max="3"/>"#).replace(
        r#"<field name="cell" w="1in" h="0.3in"/></subform></subform>"#,
        r#"<field name="cell" w="1in" h="0.3in"/></subform><field name="row" w="1in" h="0.3in"/><field w="1in" h="0.3in"/><field w="1in" h="0.3in"/></subform>"#,
    );
    let t = crate::parse(&tpl).unwrap().0;
    let page = |ov: &Overrides| -> Vec<String> {
        let form = laid_out(&t, ov);
        let items = form.pages.iter().flat_map(|p| p.items.iter());
        let soms = items.filter_map(|i| match i {
            Item::Widget(w) => Some(w.som.clone()),
            _ => None,
        });
        soms.filter(|s| s.contains(".rows[")).collect()
    };
    let live = |ov: &Overrides| -> Vec<String> {
        let live = crate::form_tree(&t, None, ov);
        let rows = &live.root.children.iter().find(|n| n.name == "rows").unwrap().children;
        rows.iter().filter(|r| r.presence == "hidden").map(|r| r.som.clone()).collect()
    };
    const ROW: &str = "form[0].rows[0].row[1]";
    const UNNAMED: &str = "form[0].rows[0].field[0]";
    const CELLS: [&str; 2] = ["form[0].rows[0].row[0].cell[0]", "form[0].rows[0].row[1].cell[0]"];
    assert!(crate::som_is_shared(&t, ROW) && crate::som_is_shared(&t, UNNAMED));
    assert_eq!(page(&Overrides::default()), [CELLS[0], CELLS[1], ROW, UNNAMED, UNNAMED]);

    let mut ov = Overrides::default();
    ov.presence.insert(UNNAMED.into(), "hidden".into());
    assert_eq!(live(&ov), [UNNAMED, UNNAMED]);
    assert_eq!(page(&ov), [CELLS[0], CELLS[1], ROW], "both unnamed fields go");

    let mut ov = Overrides::default();
    ov.presence.insert(ROW.into(), "hidden".into());
    assert_eq!(live(&ov), [ROW, ROW], "the second row and the field after the rows");
    assert_eq!(page(&ov), [CELLS[0], CELLS[1], UNNAMED, UNNAMED], "the field goes; rows take the first row's");
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
    let notes = crate::script::property_notes(&ov, &crate::apply_overrides_split(&t, &ov));
    assert!(notes.iter().any(|n| n.contains("2 property change")));
    // Review r2 (L2): they stay in the file until the overrides are next written (see `Overrides::dropped`),
    // so the note says only that they aren't applied.
    assert!(notes.iter().any(|n| n.contains("not applied") && !n.contains("saved")), "{notes:?}");
}

/// A property change is kept for an object the template makes once, and the walk that applies
/// it tells that as the rule does ([`crate::som_made_once`]). An unnamed subform adds no SOM
/// segment and data makes no instance of it, so what it holds is made once unless its `occur`
/// starts it more than once, whatever its `occur` maximum: a field in one that starts once
/// (`max="-1"`) takes its change, a field in one that starts twice doesn't, and neither does a
/// field in a named subform that may repeat. The changes left out are counted and noted.
#[test]
fn a_change_is_applied_where_the_template_makes_the_object_once() {
    let tpl = TEMPLATE.replace(
        r#"<draw name="label""#,
        r#"<subform layout="tb"><occur min="1" max="-1"/><field name="inner" w="2in" h="0.3in"><ui><textEdit/></ui></field></subform><subform layout="tb"><occur min="2" max="2"/><field name="twice" w="2in" h="0.3in"><ui><textEdit/></ui></field></subform><draw name="label""#,
    );
    let t = crate::parse(&tpl).unwrap().0;
    let (inner, twice, cell) = ("form[0].inner[0]", "form[0].twice[0]", "form[0].rows[0].row[0].cell[0]");
    assert_eq!([inner, twice, cell].map(|som| crate::som_made_once(&t, som)), [true, false, false]);
    let (red, green, blue) = (rgb(255.0, 0.0, 0.0), rgb(0.0, 128.0, 0.0), rgb(0.0, 0.0, 255.0));
    let mut ov = overrides(inner, &[("border#0/fill#0/color#0", Some("value"), "255,0,0")]);
    ov.set_prop(twice, "border#0/fill#0/color#0", Some("value"), "0,128,0").unwrap();
    ov.set_prop(cell, "border#0/fill#0/color#0", Some("value"), "0,0,255").unwrap();
    let applied = crate::apply_overrides_split(&t, &ov);
    let form = crate::layout::layout_with_overrides(&applied.template, None, &ov).unwrap();
    let count = |c: [f64; 3]| items(&form).iter().filter(|i| matches!(i, Item::Fill { color, .. } if *color == c)).count();
    assert_eq!((count(red), count(green), count(blue)), (1, 0, 0));
    assert_eq!(applied.unplaced, 2);
    let notes = crate::script::property_notes(&ov, &applied);
    assert!(notes.iter().any(|n| n.starts_with("2 object(s)") && n.contains("not drawn")), "{notes:?}");
    // Nothing left out, nothing noted.
    let ov = overrides(inner, &[("border#0/fill#0/color#0", Some("value"), "255,0,0")]);
    let applied = crate::apply_overrides_split(&t, &ov);
    assert_eq!(applied.unplaced, 0);
    assert!(crate::script::property_notes(&ov, &applied).is_empty());
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

/// A field's format test and whether its value is marked an override, as the template and a
/// script's changes make them (for a row's field, that row's changes), for the checks after
/// the user changes its value; the template's defaults otherwise (`warning`, XFA 3.3 p. 880).
#[test]
fn a_fields_format_test_and_override_follow_the_changes() {
    use crate::validation_rules::Test;
    let tpl = TEMPLATE.replace(
        r#"<field name="plain" w="2in" h="0.3in"><ui><textEdit/></ui></field>"#,
        r#"<field name="plain" w="2in" h="0.3in"><ui><textEdit/></ui><calculate override="warning"><script>1</script><message><text>Calculated</text></message></calculate><validate><picture>text{99}</picture><message><text name="formatTest">Two digits</text></message></validate></field>"#,
    );
    let t = crate::parse(&tpl).unwrap().0;
    let f = crate::field_now(&t, &Overrides::default(), "form[0].plain[0]").unwrap();
    assert_eq!((f.format_test, f.format_picture.as_deref(), f.format_message.as_deref()), (Test::Warning, Some("text{99}"), Some("Two digits")));
    let mut ov = Overrides::default();
    ov.set_prop("form[0].plain[0]", "validate#0", Some("formatTest"), "error").unwrap();
    ov.set_prop("form[0].plain[0]", "validate#0/message#0/text#0", None, "Changed").unwrap();
    ov.set_prop("form[0].rows[0].row[2].cell[0]", "validate#0/picture#0", None, "num{9}").unwrap();
    let f = crate::field_now(&t, &ov, "form[0].plain[0]").unwrap();
    assert_eq!((f.format_test, f.format_message.as_deref()), (Test::Error, Some("Changed")));
    let row = |i: usize| crate::field_now(&t, &ov, &format!("form[0].rows[0].row[{i}].cell[0]")).unwrap().format_picture;
    assert_eq!((row(0), row(2)), (None, Some("num{9}".to_string())));
    // A script marks the value an override, and takes the mark off again.
    assert!(!crate::value_overridden(&t, &ov, "form[0].plain[0]"));
    ov.set_prop("form[0].plain[0]", "value#0", Some("override"), "1").unwrap();
    assert!(crate::value_overridden(&t, &ov, "form[0].plain[0]"));
    ov.set_prop("form[0].plain[0]", "value#0", Some("override"), "0").unwrap();
    assert!(!crate::value_overridden(&t, &ov, "form[0].plain[0]"));
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
fn caption_and_item_changes_take_effect() {
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

/// A row subform outside a table is as tall as its cells and its margins, once (its cells
/// already take them): its cells keep their height and what follows starts where it ends.
#[test]
fn a_rows_margins_count_once_in_its_height() {
    let t = crate::parse(
        r##"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/">
<subform name="form" layout="tb">
 <pageSet><pageArea name="p"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="line" layout="row"><margin topInset="0.25in" bottomInset="0.25in"/><field name="a" w="2in" h="0.3in"><ui><textEdit/></ui></field></subform>
 <field name="after" w="2in" h="0.3in"><ui><textEdit/></ui></field>
</subform></template>"##,
    )
    .unwrap()
    .0;
    let form = laid_out(&t, &Overrides::default());
    let rect =
        |som: &str| items(&form).into_iter().find_map(|i| if let Item::Widget(w) = i { (w.som == som).then_some(w.rect) } else { None }).unwrap();
    let (a, after) = (rect("form[0].line[0].a[0]"), rect("form[0].after[0]"));
    // The cell stretches to the row's inner height: its own 0.3in, not a margin's worth more.
    assert!((a.y - 36.0 - 18.0).abs() < 0.01 && (a.h - 21.6).abs() < 0.01, "{a:?}");
    assert!((after.y - (a.bottom() + 18.0)).abs() < 0.01, "the row's bottom margin once: {a:?} {after:?}");
}

/// A change left out for the object's budget leaves nothing behind: the elements it would have
/// added (an edge under a new border) are not kept empty, where they would be drawn as a
/// default border.
#[test]
fn a_change_left_out_for_the_budget_adds_no_empty_element() {
    use crate::model::{Prop, Props};
    // Room for the border and the edge (10 bytes of tags), not for the thickness on top.
    let filler =
        Prop { tag: "desc".into(), text: Some(std::sync::Arc::from("x".repeat(crate::parse::MAX_OBJECT_PROP_BYTES - 11 - 4))), ..Default::default() };
    let base = Props { elements: vec![filler], ..Default::default() };
    let (out, cut) = apply_props(&base, &sets(&[("border#0/edge#0", Some("thickness"), "3pt")]));
    assert!(cut);
    assert_eq!(out.elements.iter().map(|e| e.tag.as_str()).collect::<Vec<_>>(), ["desc"], "no empty border kept");
}

/// A path that numbers a later instance of a subform the template doesn't repeat is one the
/// form doesn't write; read leniently, it names the template's subform, so the checks that look
/// a field up by its SOM path (its format test, a reset's default, whether it grows) find it.
#[test]
fn a_later_number_of_a_subform_that_doesnt_repeat_names_the_subform() {
    let xdp = crate::fixtures::scripted_template().replace(r#"<occur min="1" max="-1"/>"#, r#"<occur min="1" max="1"/>"#).replace(
        r#"<field name="item" h="0.3in"><ui><textEdit/></ui></field>"#,
        r#"<field name="item" h="0.3in"><ui><textEdit/></ui><value><text>Default</text></value></field>"#,
    );
    let (t, _) = crate::parse(&xdp).unwrap();
    let som = "form[0].page1[0].table[0].row[2].item[0]";
    assert!(crate::data::template_field(&t, som).is_some());
    assert_eq!(crate::reset_default(&t, som).as_deref(), Some("Default"));
    assert!(crate::field_now(&t, &Overrides::default(), som).is_some());
    // A field is not a subform: no second instance of one.
    assert!(crate::data::template_field(&t, "form[0].page1[0].qty[1]").is_none());
}

/// Review r2 (H1): an item list's entries are outside the object's budget, as the reader keeps
/// them, so a change names only an entry a list holds and never adds one: a change to entry 999
/// of a two-entry list, or to an entry of a list the field doesn't have, is left out (noted as
/// cut) and adds nothing. A change to an entry there is still applied. Nothing goes under an
/// entry, and a list past the field's two is an ordinary property, its entries up to 7.
#[test]
fn a_change_names_only_an_entry_the_list_holds() {
    let tpl = TEMPLATE.replace(
        r#"<draw name="label""#,
        r#"<field name="pick" w="2in" h="0.3in"><ui><choiceList/></ui><items><text>A</text><text>B</text></items></field><draw name="label""#,
    );
    let t = crate::parse(&tpl).unwrap().0;
    let props = &field(&t, "pick").common.props;
    let entries = |p: &Props| p.elements.iter().filter(|e| e.tag == "items").map(|e| e.children.len()).collect::<Vec<_>>();
    let (after, cut) = apply_props(props, &sets(&[("items#0/text#1", None, "Z")]));
    assert!(!cut);
    assert_eq!(after.elements.iter().find(|e| e.tag == "items").and_then(|e| e.children.get(1)).and_then(|c| c.text.as_deref()), Some("Z"));
    for change in ["items#0/text#999", "items#0/text#2", "items#0/integer#0", "items#1/text#0"] {
        let (after, cut) = apply_props(props, &sets(&[(change, None, "")]));
        assert!(cut, "{change}");
        assert_eq!(entries(&after), [2], "{change}: no entry added");
    }
    assert!(check("items#0/text#0/x#0", Some("a"), "1").is_err(), "nothing under an entry");
    assert!(check("items#2/text#8", None, "x").is_err(), "a third list is an ordinary property");
    assert!(check("items#1/text#999", None, "x").is_ok());
    let ov = overrides("form[0].pick[0]", &[("items#0/text#0", None, "First"), ("items#0/text#999", None, "")]);
    let applied = crate::apply_overrides_split(&t, &ov);
    let pick = field(&applied.template, "pick");
    assert_eq!(pick.items.iter().map(|i| i.as_str()).collect::<Vec<_>>(), ["First", "B"]);
    assert_eq!(applied.cut, 1, "noted");
}

/// Review r2 (H1): a form holding as many changes as it keeps, each to entry 999 of an item
/// list of a field that has none (two to each of 5,000 fields), adds no entry: the live form's
/// trees stay small and the fields' lists empty, the memo keeps every field for the next build
/// (none made again), and laying it out takes well under its bound.
#[test]
fn changes_to_far_list_entries_make_nothing() {
    let fields: String = (0..5_000).map(|i| format!(r#"<field name="f{i}" w="1in" h="0.2in"><ui><choiceList/></ui></field>"#)).collect();
    let t = crate::parse(&TEMPLATE.replace(r#"<draw name="label""#, &format!(r#"{fields}<draw name="label""#))).unwrap().0;
    let mut ov = Overrides::default();
    for i in 0..5_000 {
        for list in 0..2 {
            ov.set_prop(&format!("form[0].f{i}[0]"), &format!("items#{list}/text#999"), None, "").unwrap();
        }
    }
    assert_eq!(ov.props().values().map(BTreeMap::len).sum::<usize>(), MAX_PROP_OVERRIDES);
    let start = std::time::Instant::now();
    let applied = crate::apply_overrides_split(&t, &ov);
    let form = crate::layout::layout_with_overrides(&applied.template, None, &ov).unwrap();
    assert!(!form.pages.is_empty());
    let _ = crate::memo::take_fields_made();
    let live = crate::form_tree(&t, None, &ov);
    let made = crate::memo::take_fields_made();
    let live_again = crate::form_tree(&t, None, &ov);
    let spent = start.elapsed();
    assert_eq!(made, 5_000);
    assert_eq!(crate::memo::take_fields_made(), 0, "the memo keeps every field for the next build");
    let elements: usize = live.root.children.iter().map(|n| size_of(&n.props.elements, 0).0).sum();
    assert!(elements <= 5_000 * 4, "{elements}");
    let items: usize = applied.template.root.children.iter().map(|n| if let Node::Field(f) = n { f.items.len() } else { 0 }).sum();
    assert_eq!(items, 0);
    assert_eq!(live_again.root.children.len(), live.root.children.len());
    assert!(spent < std::time::Duration::from_secs(5), "{spent:?}");
}

/// Review r2 (L1): the checks after an entry take the field from the template's memo
/// (`field_now`): made once, then handed back; whether a calculation is overridden reads the
/// field again only when a change to its value's `override` is kept; and building the live
/// form again reads no object again.
#[test]
fn checks_and_the_live_form_restyle_nothing_again() {
    let t = template();
    let mut ov = overrides("form[0].plain[0]", &[("validate#0", Some("formatTest"), "error")]);
    let _ = (crate::memo::take_fields_made(), take_restyled());
    assert!(!crate::value_overridden(&t, &ov, "form[0].plain[0]"));
    assert_eq!((crate::memo::take_fields_made(), take_restyled()), (0, 0), "no override change: the template's");
    for _ in 0..3 {
        assert!(crate::field_now(&t, &ov, "form[0].plain[0]").is_some());
    }
    assert_eq!(crate::memo::take_fields_made(), 1, "made once, then handed back");
    ov.set_prop("form[0].plain[0]", "value#0", Some("override"), "1").unwrap();
    assert!(crate::value_overridden(&t, &ov, "form[0].plain[0]"));
    ov.set_prop("form[0].later[0]", "border#0/edge#0/color#0", Some("value"), "1,2,3").unwrap();
    let _ = crate::form_tree(&t, None, &ov);
    let _ = take_restyled();
    let _ = crate::form_tree(&t, None, &ov);
    assert_eq!(take_restyled(), 0, "the live form built again reads no object again");
}
