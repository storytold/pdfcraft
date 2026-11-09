//! The property elements the parser keeps for scripts (XFA 3.3 ch. 3, "Properties vs.
//! Children"), their limits, and the live form sharing them rather than copying.

use std::sync::Arc;

use crate::model::{Node, Prop};
use crate::parse::{MAX_OBJECT_PROP_BYTES, MAX_OBJECT_PROPS, PROPS_CUT_WARNING};
use crate::{Overrides, form_tree, parse};

fn template(body: &str) -> String {
    format!(
        r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form" layout="tb"><pageSet><pageArea name="p"><contentArea w="8in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>{body}</subform></template>"#
    )
}

fn first_field(xml: &str) -> (crate::model::Field, Vec<String>) {
    let (t, warnings) = parse(xml).expect("parses");
    let f = t.root.children.iter().find_map(|n| if let Node::Field(f) = n { Some((**f).clone()) } else { None }).expect("a field");
    (f, warnings)
}

fn count(p: &Prop) -> usize {
    1 + p.children.iter().map(count).sum::<usize>()
}

fn depth(p: &Prop) -> usize {
    1 + p.children.iter().map(depth).max().unwrap_or(0)
}

#[test]
fn a_fields_property_elements_are_kept_for_scripts() {
    let xml = template(
        r#"<field name="f" w="2in" access="readOnly"><ui><choiceList open="always"><border><edge><color value="9,9,9"/></edge></border></choiceList><extras><x/></extras></ui>
<border presence="hidden"><edge thickness="2pt"><color value="255,0,0"/></edge><fill><color value="1,2,3"/><linear type="toTop"/></fill></border>
<caption><value><text>Label</text></value></caption>
<validate nullTest="error"><message><text name="nullTest">Required</text></message><script>true</script></validate>
<assist><toolTip>Tip</toolTip></assist><format><picture>num{z9}</picture></format>
<value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p>Hi</p></body></exData></value>
<keep intact="contentArea"/><event activity="click"><script>1</script></event></field>"#,
    );
    let (f, warnings) = first_field(&xml);
    assert!(warnings.iter().all(|w| w != PROPS_CUT_WARNING), "{warnings:?}");
    let mut props = (*f.common.props).clone();
    assert!(props.attrs.contains(&("w".into(), "2in".into())) && props.attrs.contains(&("access".into(), "readOnly".into())), "{props:?}");
    let tags: Vec<&str> = props.elements.iter().map(|p| p.tag.as_str()).collect();
    // Not events. The caption is read whole (its label's text), the object's own value only
    // as its content element's tag.
    assert_eq!(tags, ["ui", "border", "caption", "validate", "assist", "format", "value", "keep"]);
    let caption = props.elements.remove(2);
    assert_eq!(caption.children[0].children[0].text.as_deref(), Some("Label"));
    let ui = &props.elements[0];
    assert_eq!(ui.children.len(), 1, "extras left out: {ui:?}");
    assert_eq!(ui.children[0].tag, "choiceList");
    assert_eq!(ui.children[0].children[0].children[0].children[0].attrs, [("value".to_string(), "9,9,9".to_string())]);
    let validate = &props.elements[2];
    assert_eq!(validate.children[0].children[0].text.as_deref(), Some("Required"));
    // A validation's script is kept (forms check whether there is one); event scripts are not.
    assert_eq!((validate.children[1].tag.as_str(), validate.children[1].text.as_deref()), ("script", Some("true")));
    assert!(props.elements.iter().all(|p| p.tag != "event"));
    assert_eq!(props.elements[3].children[0].text.as_deref(), Some("Tip"));
    assert_eq!(props.elements[4].children[0].text.as_deref(), Some("num{z9}"));
    // The value's content element, not its data.
    let value = &props.elements[5];
    assert_eq!((value.children.len(), value.children[0].tag.as_str(), value.children[0].children.len()), (1, "exData", 0));
    assert_eq!(value.children[0].text, None);
}

/// A hostile template: thousands of edges, deep nesting, a huge attribute and many attributes.
/// What scripts are given stays within the per-object limits, and a warning says so once.
#[test]
fn hostile_property_trees_are_cut_to_the_limits() {
    let edges = "<edge><color value=\"1,1,1\"/></edge>".repeat(5_000);
    let deep = format!("{}{}", "<fill>".repeat(200), "</fill>".repeat(200));
    let big = "x".repeat(200_000);
    let attrs: String = (0..500).map(|i| format!(" a{i}=\"{i}\"")).collect();
    // Many lists of many entries: only two lists are kept (a field has two at most), each to the
    // list's own entry limit, and the lists count against the budget like any property.
    let entries = "<text>e</text>".repeat(1_200);
    let lists = format!("<items>{entries}</items>").repeat(40);
    for body in [
        format!("<field name=\"f\"><border>{edges}</border></field>"),
        format!("<field name=\"f\"><border>{deep}</border></field>"),
        format!("<field name=\"f\"><font typeface=\"{big}\"/></field>"),
        format!("<field name=\"f\"><font{attrs}/></field>"),
        format!("<field name=\"f\" {}><font/></field>", attrs.trim()),
        format!("<field name=\"f\">{lists}</field>"),
    ] {
        let (f, warnings) = first_field(&template(&format!("{body}{body}")));
        assert_eq!(warnings.iter().filter(|w| *w == PROPS_CUT_WARNING).count(), 1, "once: {warnings:?}");
        let props = &f.common.props;
        // The first two lists' entries are the lists' own (up to 1,000 each), not property
        // elements; any further list, entries and all, counts against the budget.
        let mut lists = 0;
        let elements: usize = props
            .elements
            .iter()
            .map(|p| match p.tag.as_str() {
                "items" if lists < 2 => {
                    lists += 1;
                    assert!(p.children.len() <= 1_000, "entries past the list's limit");
                    1
                }
                _ => count(p),
            })
            .sum();
        assert!(elements <= MAX_OBJECT_PROPS, "{elements}");
        assert!(props.elements.iter().map(depth).max().unwrap_or(0) <= 9, "deep");
        let bytes = |p: &Prop| -> usize { p.attrs.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() + p.tag.len() };
        fn all<'a>(p: &'a Prop, out: &mut Vec<&'a Prop>) {
            out.push(p);
            p.children.iter().for_each(|c| all(c, out));
        }
        let mut flat = Vec::new();
        props.elements.iter().for_each(|p| all(p, &mut flat));
        let total: usize = flat.iter().map(|p| bytes(p)).sum::<usize>() + props.attrs.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>();
        assert!(total <= MAX_OBJECT_PROP_BYTES, "{total}");
        assert!(flat.iter().all(|p| p.attrs.len() <= 64) && props.attrs.len() <= 64);
    }
}

/// Every instance of a repeating subform, and every live form built, points at the template's
/// property tree: nothing is copied per instance or per build.
#[test]
fn the_live_form_shares_the_templates_property_trees() {
    let xml = template(
        r#"<subform name="row" layout="tb"><occur min="3" max="-1" initial="3"/><field name="f"><border><edge/></border></field></subform>"#,
    );
    let (t, _) = parse(&xml).expect("parses");
    let a = form_tree(&t, None, &Overrides::default());
    let b = form_tree(&t, None, &Overrides::default());
    let rows: Vec<_> = a.root.children.iter().filter(|c| c.name == "row").collect();
    assert_eq!(rows.len(), 3);
    let Some(Node::Subform(row)) = t.root.children.first() else { panic!("a row subform") };
    for r in &rows {
        assert!(Arc::ptr_eq(&r.props, &row.common.props));
        assert!(Arc::ptr_eq(&r.children[0].props, &rows[0].children[0].props));
    }
    assert_eq!(rows[0].props.elements[0].tag, "occur");
    assert!(Arc::ptr_eq(&a.root.children[0].props, &b.root.children[0].props));
}

/// A property element whose children are all left out (an `extras`, a `script`) has only its
/// own text, not theirs, and nothing is reported cut.
#[test]
fn a_property_keeps_its_own_text_not_that_of_children_left_out() {
    let big = "x".repeat(20_000);
    let xml = template(&format!(
        r#"<field name="f"><font typeface="Arial"><extras><text>{big}</text></extras></font><assist><toolTip>Tip<extras><text>hidden</text></extras></toolTip></assist><breakBefore targetType="pageArea"><script>xfa.host.messageBox("{big}")</script></breakBefore><validate><script>{big}</script></validate><border><edge thickness="2pt"/></border></field>"#
    ));
    let (f, warnings) = first_field(&xml);
    assert!(warnings.iter().all(|w| w != PROPS_CUT_WARNING), "{warnings:?}");
    let tags: Vec<(&str, Option<&str>)> = f.common.props.elements.iter().map(|p| (p.tag.as_str(), p.text.as_deref())).collect();
    assert_eq!(tags, [("font", None), ("assist", None), ("breakBefore", None), ("validate", None), ("border", None)]);
    assert_eq!(f.common.props.elements[1].children[0].text.as_deref(), Some("Tip"));
    // A validation's script keeps its text, outside the budget: the border after it is kept.
    assert_eq!(f.common.props.elements[3].children[0].text.as_ref().map(|t| t.len()), Some(20_000));
    assert_eq!(f.common.props.elements[4].children[0].attrs, [("thickness".to_string(), "2pt".to_string())]);
}

/// A validation keeps its first script only (it has one, XFA 3.3 p. 880), and that script's
/// text only within the script engine's limit on a script.
#[test]
fn a_validation_keeps_its_first_script_within_the_script_limit() {
    let xml = template(r#"<field name="f"><validate><script>first</script><script>second</script></validate></field>"#);
    let (f, _) = first_field(&xml);
    let scripts: Vec<Option<&str>> = f.common.props.elements[0].children.iter().map(|s| s.text.as_deref()).collect();
    assert_eq!(scripts, [Some("first")]);
    let huge = "x".repeat(300 * 1024);
    let (f, warnings) =
        first_field(&template(&format!(r#"<field name="f"><validate><script>{huge}</script></validate><font typeface="Arial"/></field>"#)));
    assert_eq!(f.common.props.elements[0].children[0].text, None);
    assert_eq!(f.common.props.elements[1].attrs, [("typeface".to_string(), "Arial".to_string())]);
    assert!(warnings.iter().any(|w| w == PROPS_CUT_WARNING), "{warnings:?}");
}

/// A field with a third `<items>` (a public form carries an empty saved list twice) keeps it
/// too, as an ordinary property, with no warning.
#[test]
fn a_third_items_list_is_kept_as_a_property() {
    let xml =
        template(r#"<field name="f"><items/><items save="1" presence="hidden"/><items save="1" presence="hidden"><text>0</text></items></field>"#);
    let (f, warnings) = first_field(&xml);
    assert!(warnings.iter().all(|w| w != PROPS_CUT_WARNING), "{warnings:?}");
    let lists: Vec<usize> = f.common.props.elements.iter().filter(|p| p.tag == "items").map(|p| p.children.len()).collect();
    assert_eq!(lists, [0, 0, 1]);
}

/// A calculation is kept with its override, message and first script (XFA 3.3 p. 641: one
/// script), as a validation is; the field still calculates with that script.
#[test]
fn a_calculation_is_kept_with_its_override_message_and_script() {
    let xml = template(
        r#"<field name="f"><calculate override="warning"><message><text>Computed</text></message><script>a + b</script><script>c</script></calculate></field>"#,
    );
    let (f, _) = first_field(&xml);
    let calc = f.common.props.elements.first().expect("the calculation");
    assert_eq!((calc.tag.as_str(), calc.attrs.as_slice()), ("calculate", [("override".to_string(), "warning".to_string())].as_slice()));
    let kids: Vec<(&str, Option<&str>)> = calc.children.iter().map(|k| (k.tag.as_str(), k.text.as_deref())).collect();
    assert_eq!(kids, [("message", None), ("script", Some("a + b"))]);
    assert_eq!(calc.children[0].children[0].text.as_deref(), Some("Computed"));
    assert_eq!(f.calculate.as_ref().map(|s| s.text.as_str()), Some("a + b"));
}

/// A caption's rich text is kept as its words, and a field's items are kept whole, each entry
/// with its text as the list shows it, outside the object's property budget (a long list of
/// countries must not crowd out the other properties).
#[test]
fn a_caption_s_rich_text_and_a_list_s_items_are_kept() {
    let entries: String = (0..400).map(|i| format!("<text>Country {i}</text>")).collect();
    let xml = template(&format!(
        r#"<field name="f"><caption><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p>Date of <span style="font-weight:bold">birth</span>:</p></body></exData></value></caption><items>{entries}</items><items save="1"><integer>1</integer></items><border><edge/></border></field>"#
    ));
    let (f, warnings) = first_field(&xml);
    assert!(warnings.iter().all(|w| w != PROPS_CUT_WARNING), "{warnings:?}");
    let tags: Vec<&str> = f.common.props.elements.iter().map(|p| p.tag.as_str()).collect();
    assert_eq!(tags, ["caption", "items", "items", "border"]);
    let exdata = &f.common.props.elements[0].children[0].children[0];
    assert_eq!((exdata.tag.as_str(), exdata.text.as_deref()), ("exData", Some("Date of birth:")));
    let shown = &f.common.props.elements[1];
    assert_eq!((shown.children.len(), shown.children[399].text.as_deref()), (400, Some("Country 399")));
    assert_eq!(f.common.props.elements[2].attrs, [("save".to_string(), "1".to_string())]);
    assert_eq!(f.items.len(), 400, "the list shows the same entries");
}

/// A template handed to `parse` directly is held to what a document's packets may come to as text:
/// one byte past it is refused before any of it is parsed (the error giving its size in bytes), and
/// a template of exactly that size, as a UTF-16 packet the reader accepted can become, is parsed.
#[test]
fn a_template_past_the_packet_limit_is_refused_before_parsing() {
    let limit = crate::packets::MAX_TEMPLATE_TEXT;
    assert!(limit > crate::packets::MAX_XDP, "room for a UTF-16 packet grown into UTF-8");
    let head = r#"<template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form"><field name="f"><items><text>"#;
    let tail = "</text></items></field></subform></template>";
    let xml = format!("{head}{}{tail}", "a".repeat(limit + 1 - head.len() - tail.len()));
    assert_eq!(xml.len(), limit + 1);
    match crate::parse(&xml) {
        Err(crate::XfaError::TooLarge(m)) => assert!(m.contains(&format!("{} bytes", limit + 1)), "{m}"),
        other => panic!("{other:?}"),
    }
    let at = format!("{head}{}{tail}", "a".repeat(limit - head.len() - tail.len()));
    assert!(crate::parse(&at).is_ok(), "a template at the limit is parsed");
}
