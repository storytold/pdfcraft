//! Prototypes (`use`, `usehref`): synthetic templates only.

use crate::layout::{BorderShape, Item, WidgetKind};
use crate::model::{Node, Placement, Template, Ui, Value};
use crate::{layout_xml, parse};

const MM: f64 = 72.0 / 25.4;

/// A form whose root subform holds `styles` in a Designer-style sheet (a named subform inside
/// `proto`) and positions `body` on one page.
fn form(styles: &str, body: &str) -> String {
    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<xdp:xdp xmlns:xdp="http://ns.adobe.com/xdp/"><template xmlns="http://www.xfa.org/schema/xfa-template/3.3/"><subform name="form1" layout="tb">
 <pageSet><pageArea name="page"><contentArea x="0.5in" y="0.5in" w="7.5in" h="10in"/><medium short="8.5in" long="11in"/></pageArea></pageSet>
 <subform name="body" w="190mm" h="250mm">{body}</subform>
 <proto><subform name="designer__stylesheet">{styles}</subform></proto>
</subform></template></xdp:xdp>"##
    )
}

fn close(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() < 0.01)
}

/// The nodes of the body subform.
fn nodes(t: &Template) -> &[Node] {
    let Some(Node::Subform(b)) = t.root.children.first() else { panic!("no body subform: {:?}", t.root.children) };
    &b.children
}

fn field<'t>(t: &'t Template, name: &str) -> &'t crate::model::Field {
    nodes(t)
        .iter()
        .find_map(|n| match n {
            Node::Field(f) if f.common.name.as_deref() == Some(name) => Some(f.as_ref()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no field {name}"))
}

const STYLES: &str = r#"
  <draw name="Heading" w="190.5mm" h="6mm" anchorType="middleLeft"><border><edge presence="hidden"/><fill><color value="153,153,153"/></fill></border><font typeface="Arial" size="12pt" weight="bold"/></draw>
  <field name="Numeric" h="5mm"><ui><numericEdit><border><edge stroke="lowered"/></border></numericEdit></ui><font typeface="Arial" size="9pt"/></field>
  <field name="Checkbox" h="6mm" anchorType="middleCenter"><ui><checkButton size="3.5278mm"><border><edge/></border></checkButton></ui><caption placement="right" reserve="30mm"><font typeface="Arial" size="9pt"/></caption><font typeface="Arial" size="9pt"/><items><integer>1</integer><integer>0</integer></items></field>"#;

const BODY: &str = r##"
  <draw name="Section" x="0mm" y="10mm" usehref=".#som($template.#subform.designer__stylesheet.Heading)"><value><text>Section</text></value></draw>
  <field name="Count" x="0mm" y="30mm" w="30mm" usehref="#som($template.#subform.designer__stylesheet.Numeric)"><ui><numericEdit/></ui></field>
  <field name="Agree" x="20mm" y="50mm" w="40mm" usehref=".#som($template.#subform.designer__stylesheet.Checkbox[0])"><caption><value><text>Agree</text></value></caption></field>"##;

#[test]
fn fields_and_draws_take_size_border_fill_caption_and_font_from_a_style_sheet() {
    let xml = form(STYLES, BODY);
    let (t, warnings) = parse(&xml).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let Some(Node::Draw(section)) = nodes(&t).first() else { panic!("{:?}", nodes(&t)) };
    let c = &section.common;
    assert_eq!(c.name.as_deref(), Some("Section"));
    assert!(close(c.w, 190.5 * MM) && close(c.h, 6.0 * MM), "{c:?}");
    assert_eq!((c.anchor.h, c.anchor.v), (0.0, 0.5));
    assert_eq!(c.border.as_ref().and_then(|b| b.fill).map(|f| f.0), Some([0.6; 3]));
    assert_eq!((c.font.typeface.as_deref(), c.font.bold), (Some("Arial"), Some(true)));
    assert_eq!(section.value, Value::Text("Section".into()));

    let count = field(&t, "Count");
    assert_eq!(count.ui, Ui::NumericEdit);
    assert!(close(count.common.w, 30.0 * MM) && close(count.common.h, 5.0 * MM));
    assert_eq!(count.ui_border.as_ref().map(|b| b.visible_edges()), Some([true; 4]));
    assert_eq!(count.common.font.typeface.as_deref(), Some("Arial"));

    let agree = field(&t, "Agree");
    assert_eq!(agree.ui, Ui::CheckButton);
    assert!(close(agree.check_size, 3.5278 * MM) && close(agree.common.h, 6.0 * MM));
    assert_eq!((agree.common.anchor.h, agree.common.anchor.v), (0.5, 0.5));
    let cap = agree.caption.as_ref().expect("caption");
    assert_eq!((cap.placement, cap.value.clone()), (Placement::Right, Value::Text("Agree".into())));
    assert_eq!(cap.font.typeface.as_deref(), Some("Arial"));
    assert_eq!(agree.items, vec!["1".to_string(), "0".to_string()]);

    // Laid out: the heading's grey bar, the numeric field's box and the check box left of its
    // caption. The style sheet itself lays nothing out.
    let laid = layout_xml(&xml).unwrap();
    let items = &laid.pages.first().expect("a page").items;
    assert!(items.iter().any(|i| matches!(i, Item::Fill { rect, color } if *color == [0.6; 3] && (rect.w - 190.5 * MM).abs() < 0.01)), "{items:?}");
    let widgets: Vec<_> = items.iter().filter_map(|i| if let Item::Widget(w) = i { Some(w) } else { None }).collect();
    assert_eq!(widgets.len(), 2, "{widgets:?}");
    let count = widgets.iter().find(|w| w.name.contains("Count")).expect("Count");
    assert_eq!(count.border.shape, BorderShape::Full);
    let check = widgets.iter().find(|w| matches!(w.kind, WidgetKind::CheckBox { .. })).expect("check box");
    let caption = items.iter().find_map(|i| if let Item::Text(s) = i { (s.text == "Agree").then_some(s.x) } else { None }).expect("caption");
    assert!(check.rect.x + check.rect.w <= caption, "box {:?}, caption at {caption}", check.rect);
}

#[test]
fn what_an_element_declares_overrides_what_it_inherits() {
    let styles = r#"<field id="P" name="Proto" w="50mm" h="10mm"><font typeface="Courier" size="14pt" weight="bold"/><border><edge thickness="2pt"/><edge thickness="1pt"/></border><value><text>proto</text></value></field>
        <field id="Q" w="20mm"/><field id="R"><ui><numericEdit/></ui><value><decimal/></value></field>
        <draw id="T"><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p>Proto <b>bold</b></p><p>second</p></body></exData></value></draw>"#;
    let body = r##"<field name="f" use="#P" h="7mm"><font size="9pt"/><border><edge thickness="3pt"/></border><value><text>own</text></value></field>
        <field use="#P"/>
        <field name="g" use="#P" usehref="#Q"/>
        <field name="t" use="#R"><ui><textEdit/></ui><value><text>7</text></value></field>
        <draw name="rich" use="#T"><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p>Own</p></body></exData></value></draw>"##;
    let (t, warnings) = parse(&form(styles, body)).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let f = field(&t, "f");
    assert!(close(f.common.h, 7.0 * MM) && close(f.common.w, 50.0 * MM), "{:?}", f.common);
    assert_eq!((f.common.font.typeface.as_deref(), f.common.font.size, f.common.font.bold), (Some("Courier"), Some(9.0), Some(true)));
    // Edges match by position: the first overridden, the second inherited (Example 6.12).
    let edges = f.common.border.as_ref().expect("border").edges.map(|e| e.thickness);
    assert_eq!(edges, [3.0, 1.0, 3.0, 1.0]);
    assert_eq!(f.value, Value::Text("own".into()));
    assert_eq!(f.common.id, None, "a prototype's id is its own");
    // Neither the name nor the id is inherited, but the content is.
    let Some(Node::Field(unnamed)) = nodes(&t).get(1) else { panic!() };
    assert_eq!((unnamed.common.name.as_deref(), unnamed.value.clone()), (None, Value::Text("proto".into())));
    // usehref wins over use.
    assert!(close(field(&t, "g").common.w, 20.0 * MM));
    // A ui holds one widget and a value one kind of content: the nearer one replaces the other.
    let t_field = field(&t, "t");
    assert_eq!((t_field.ui.clone(), t_field.numeric, t_field.value.clone()), (Ui::TextEdit, false, Value::Text("7".into())));
    // Rich text is content: the nearer layer's is taken whole.
    let Some(Node::Draw(rich)) = nodes(&t).get(4) else { panic!() };
    let Value::Rich(r) = &rich.value else { panic!("{:?}", rich.value) };
    assert_eq!(r.plain(), "Own");
}

#[test]
fn prototypes_inherit_from_prototypes() {
    // Example 6.5: a font prototype using another, the second by a bare id.
    let styles = r##"<font id="HELV" typeface="Helvetica" size="10pt"/>
        <font id="HELV-RED" use="HELV"><fill><color value="255,0,0"/></fill></font>
        <field id="A" w="40mm"/><field id="B" use="#A" h="8mm"/><field id="C" usehref="#B"><font use="#HELV-RED"/></field>"##;
    let body = r##"<field name="f" usehref=".#C"/>"##;
    let (t, warnings) = parse(&form(styles, body)).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let f = field(&t, "f");
    assert!(close(f.common.w, 40.0 * MM) && close(f.common.h, 8.0 * MM), "{:?}", f.common);
    assert_eq!((f.common.font.typeface.as_deref(), f.common.font.size), (Some("Helvetica"), Some(10.0)));
    assert_eq!(f.common.font.color.map(|c| c.0), Some([1.0, 0.0, 0.0]));
}

#[test]
fn prototype_loops_end_with_a_warning() {
    let styles = r##"<field id="A" use="#B" w="10mm"/><field id="B" use="#A" h="12mm"/>
        <subform name="Client" id="ClientSubform" w="50mm"><subform name="ClientName" use="#ClientSubform"/></subform>"##;
    let body = r##"<field name="self" id="S" use="#S" w="1in"/>
        <field name="cycle" use="#A"/>
        <subform name="client" use="#ClientSubform"/>"##;
    let (t, warnings) = parse(&form(styles, body)).unwrap();
    assert!(warnings.iter().any(|w| w.contains("leads back to itself")), "{warnings:?}");
    assert!(!warnings.iter().any(|w| w.contains("longer than")), "a loop is cut where it closes: {warnings:?}");
    assert!(close(field(&t, "self").common.w, 72.0));
    let cycle = field(&t, "cycle");
    assert!(close(cycle.common.w, 10.0 * MM) && close(cycle.common.h, 12.0 * MM), "{:?}", cycle.common);
    // Example 6.8: the inner subform does not take its parent in again.
    let Some(Node::Subform(client)) = nodes(&t).get(2) else { panic!() };
    assert!(close(client.common.w, 50.0 * MM));
    let Some(Node::Subform(inner)) = client.children.first() else { panic!("{:?}", client.children) };
    assert_eq!(inner.common.name.as_deref(), Some("ClientName"));
    assert!(inner.children.is_empty() && inner.common.w.is_none(), "{inner:?}");
}

#[test]
fn a_reference_to_nothing_or_another_document_is_left_as_it_is() {
    let styles = r#"<field id="F" w="99mm"/>"#;
    let body = r##"<field name="nowhere" w="1in" usehref=".#som($template.#subform.designer__stylesheet.Missing)"/>
        <field name="noid" w="1in" use="#nope"/>
        <field name="far" w="1in" usehref="https://example.com/styles.xdp#F"/>
        <field name="bad" w="1in" usehref="#som($template.#subform[*])"/>
        <draw name="other" w="1in" use="#F"/>"##;
    let (t, warnings) = parse(&form(styles, body)).unwrap();
    assert!(warnings.iter().any(|w| w.contains("names nothing")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("another document")), "{warnings:?}");
    for name in ["nowhere", "noid", "far", "bad"] {
        assert!(close(field(&t, name).common.w, 72.0), "{name}");
    }
    let Some(Node::Draw(other)) = nodes(&t).get(4) else { panic!() };
    assert!(close(other.common.w, 72.0), "a prototype is of the referencing element's class");
}

#[test]
fn an_expansion_past_its_budget_leaves_the_template_unresolved_with_a_warning() {
    // 200 references to a 20 kB prototype would write 4 MB from a template of a few kB.
    let styles = format!(r#"<draw id="Big" w="10mm"><value><text>{}</text></value></draw>"#, "x".repeat(20_000));
    let body: String = (0..200).map(|i| format!(r##"<draw name="d{i}" use="#Big"/>"##)).collect();
    let started = std::time::Instant::now();
    let (t, warnings) = parse(&form(&styles, &body)).unwrap();
    assert!(started.elapsed().as_secs() < 10);
    assert!(warnings.iter().any(|w| w.contains("larger than 1 MB")), "{warnings:?}");
    let Some(Node::Draw(d)) = nodes(&t).first() else { panic!() };
    assert_eq!((d.common.w, &d.value), (None, &Value::Empty), "read as written");
}

#[test]
fn hostile_references_neither_panic_nor_run_away() {
    let refs = [
        "#som(",
        "#som()",
        "#som($template.[)",
        "#som($template.#subform[99999999999999999999999])",
        "#som(.)",
        "#som(#)",
        "#",
        ".#",
        "#som($template.#subform.designer__stylesheet.ü[0].x[1])",
        "#som(xfa.template.#subform[0].#proto)",
        "#\u{feff}",
    ];
    let body: String = refs
        .iter()
        .enumerate()
        .map(|(i, r)| format!(r#"<field name="f{i}" w="1in" usehref="{r}"/><field name="u{i}" w="1in" use="{r}"/>"#))
        .collect();
    let (t, _) = parse(&form("", &body)).unwrap();
    assert!(close(field(&t, "f0").common.w, 72.0));
    // Prototypes six levels deep, each holding ten references to the next: a million elements.
    let mut styles = String::from(r#"<subform id="P6"><field w="1mm"/></subform>"#);
    for level in (1..6).rev() {
        let next = level + 1;
        let refs: String = (0..10).map(|_| format!(r##"<subform use="#P{next}"/>"##)).collect();
        styles.push_str(&format!(r#"<subform id="P{level}">{refs}</subform>"#));
    }
    let started = std::time::Instant::now();
    let (_, warnings) = parse(&form(&styles, r##"<subform name="big" use="#P1"/>"##)).unwrap();
    assert!(started.elapsed().as_secs() < 10);
    assert!(warnings.iter().any(|w| w.contains("larger than")), "{warnings:?}");
}

#[test]
fn a_som_search_past_its_budget_leaves_the_template_unresolved_with_one_warning() {
    // 300 references, each to a style 10,000 elements into the sheet: 3 million elements looked
    // at, past the 2 million the look-ups share. The first ones resolve before the budget runs
    // out, so the template is either resolved in full or not at all.
    let styles: String = (0..10_000).map(|i| format!(r#"<field name="s{i}" w="1mm"/>"#)).collect();
    let body: String =
        (0..300).map(|i| format!(r##"<field name="f{i}" usehref="#som($template.#subform.designer__stylesheet.s{})"/>"##, 9_999 - i)).collect();
    let (t, warnings) = parse(&form(&styles, &body)).unwrap();
    assert!(warnings.iter().any(|w| w.contains("SOM references") && w.contains("read without resolving")), "{warnings:?}");
    assert!(!warnings.iter().any(|w| w.contains("names nothing")), "an interrupted search is not a missing target: {warnings:?}");
    assert_eq!(field(&t, "f0").common.w, None, "read as written");
}

#[test]
fn prototype_children_that_write_nothing_still_count_against_a_budget() {
    // A prototype with 20,000 `proto` children, used by 20,000 fields: the children write
    // nothing, but merging would look at 400 million of them.
    let protos = "<proto/>".repeat(20_000);
    let styles = format!(r#"<field id="P" w="1mm">{protos}</field>"#);
    let body = r##"<field use="#P"/>"##.repeat(20_000).replacen("<field ", r#"<field name="first" "#, 1);
    let started = std::time::Instant::now();
    let (t, warnings) = parse(&form(&styles, &body)).unwrap();
    assert!(started.elapsed().as_secs() < 10, "{:?}", started.elapsed());
    assert!(warnings.iter().any(|w| w.contains("children, attributes and namespaces") && w.contains("read without resolving")), "{warnings:?}");
    assert_eq!(field(&t, "first").common.w, None, "read as written");
}

/// The template of `xml` written again by `expand`, parsed.
fn expanded(xml: &str) -> String {
    let doc = roxmltree::Document::parse(xml).unwrap();
    let template = doc.descendants().find(|n| n.tag_name().name() == "template").unwrap();
    let (out, warnings) = crate::proto::expand(template);
    assert!(warnings.is_empty(), "{warnings:?}");
    out.unwrap()
}

#[test]
fn namespaces_bound_again_inside_the_template_keep_their_own_uri() {
    const XHTML: &str = "http://www.w3.org/1999/xhtml";
    // Rich text in a prototype and in a plain element: XHTML's default namespace inside the
    // template's (XFA's), and a prefix bound to one URI around an element and another inside.
    let styles = r#"<draw id="Note" w="40mm"><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml"><p>See <a href="https://example.com/">the guide</a></p></body></exData></value></draw>"#;
    let body = r##"<draw name="note" x="0mm" y="0mm" use="#Note"/>
  <draw name="plain" x="0mm" y="20mm" w="40mm" xmlns:q="urn:outer" q:a="1"><value><exData contentType="text/html"><body xmlns="http://www.w3.org/1999/xhtml" xmlns:q="urn:inner"><p q:b="2">Plain</p></body></exData></value></draw>
  <draw name="none" x="0mm" y="40mm" w="40mm"><extras><free xmlns=""/></extras></draw>"##;
    let xml = form(styles, body);
    let out = expanded(&xml);
    let doc = roxmltree::Document::parse(&out).unwrap();
    let bodies: Vec<_> = doc.descendants().filter(|n| n.tag_name().name() == "body").collect();
    assert_eq!(bodies.len(), 2, "{out}");
    for b in &bodies {
        assert_eq!(b.tag_name().namespace(), Some(XHTML), "{out}");
        assert!(b.descendants().filter(|n| n.is_element()).all(|n| n.tag_name().namespace() == Some(XHTML)), "{out}");
    }
    let p = doc.descendants().find(|n| n.attribute(("urn:inner", "b")).is_some());
    assert!(p.is_some(), "a prefix bound again keeps the inner URI: {out}");
    assert!(doc.descendants().any(|n| n.attribute(("urn:outer", "a")) == Some("1")), "{out}");
    let free = doc.descendants().find(|n| n.tag_name().name() == "free").unwrap();
    assert_eq!(free.tag_name().namespace().unwrap_or_default(), "", "an undeclared default namespace stays undeclared: {out}");
    // And the form still reads the rich text and its link.
    let (t, warnings) = parse(&xml).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    let Some(Node::Draw(note)) = nodes(&t).first() else { panic!() };
    let Value::Rich(paras) = &note.value else { panic!("{:?}", note.value) };
    assert!(
        paras.paragraphs.iter().flat_map(|p| &p.runs).any(|r| r.text.contains("the guide") && r.link.as_deref() == Some("https://example.com/")),
        "{paras:?}"
    );
}

#[test]
fn inherited_attributes_keep_their_namespace_where_the_prefix_is_bound_again() {
    // The element binds `q` to one URI and its prototype to another: the element's binding
    // wins, so the prototype's `q:` attributes need another prefix to keep their own URI.
    let styles = r#"<field id="P" xmlns:q="urn:prototype" q:kind="k" q:own="2" xml:lang="en" w="1in"/>"#;
    let body = r##"<field name="f" xmlns:q="urn:instance" q:own="1" use="#P"/>"##;
    let out = expanded(&form(styles, body));
    let doc = roxmltree::Document::parse(&out).unwrap();
    let f = doc.descendants().find(|n| n.attribute("name") == Some("f")).unwrap();
    assert_eq!(f.attribute(("urn:instance", "own")), Some("1"), "{out}");
    assert_eq!(f.attribute(("urn:prototype", "own")), Some("2"), "{out}");
    assert_eq!(f.attribute(("urn:prototype", "kind")), Some("k"), "{out}");
    assert_eq!(f.attribute(("urn:instance", "kind")), None, "{out}");
    assert_eq!(f.attribute(("http://www.w3.org/XML/1998/namespace", "lang")), Some("en"), "{out}");
    assert_eq!(f.attribute("w"), Some("1in"), "{out}");
}

/// `expand` on the template of `xml`, timed.
fn expand_timed(xml: &str) -> (Option<String>, Vec<String>, std::time::Duration) {
    let doc = roxmltree::Document::parse(xml).unwrap();
    let template = doc.descendants().find(|n| n.tag_name().name() == "template").unwrap();
    let started = std::time::Instant::now();
    let (out, warnings) = crate::proto::expand(template);
    (out, warnings, started.elapsed())
}

#[test]
fn billion_laughs_prototypes_fail_whole_within_the_output_cap() {
    // Ten levels of prototypes, each holding ten references to the level below: a template of
    // under 3 kB that would write 10^10 fields. Expanding fails as a whole, quickly, and
    // nothing of it is used (no truncated template): the form is read as written.
    let mut styles = String::from(r#"<subform id="L10"><field w="1mm"/></subform>"#);
    for level in (1..10).rev() {
        let refs = format!(r##"<subform use="#L{}"/>"##, level + 1).repeat(10);
        styles.push_str(&format!(r#"<subform id="L{level}">{refs}</subform>"#));
    }
    let xml = form(&styles, r##"<subform name="bomb" use="#L1"/><field name="plain" w="1in"/>"##);
    assert!(xml.len() < 3_000, "{}", xml.len());
    let (out, warnings, took) = expand_timed(&xml);
    assert!(took.as_secs() < 5, "{took:?}");
    assert!(out.is_none(), "a failed expansion writes nothing for the parser to read");
    assert!(warnings.iter().any(|w| w.contains("larger than 1 MB") && w.contains("read without resolving")), "{warnings:?}");
    let (t, _) = parse(&xml).unwrap();
    let Some(Node::Subform(bomb)) = nodes(&t).first() else { panic!() };
    assert!(bomb.children.is_empty(), "read as written: {bomb:?}");
    assert!(close(field(&t, "plain").common.w, 72.0));
}

#[test]
fn a_designer_style_sheet_on_a_long_form_resolves_in_full() {
    // 3,000 fields and draws on Designer's style sheet, each through `#som(…)`: well inside
    // every budget, so all of them take their style and nothing is warned about.
    let styles: String = (0..40).map(|i| format!(r#"<field name="S{i}" h="{}mm"><font typeface="Arial" size="9pt"/></field>"#, i + 1)).collect();
    let body: String = (0..3_000)
        .map(|i| format!(r##"<field name="f{i}" w="20mm" usehref=".#som($template.#subform.designer__stylesheet.S{})"/>"##, i % 40))
        .collect();
    let xml = form(&styles, &body);
    let (out, warnings, took) = expand_timed(&xml);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(out.is_some() && took.as_secs() < 5, "{took:?}");
    let (t, warnings) = parse(&xml).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    for i in [0usize, 39, 1_234, 2_999] {
        let f = field(&t, &format!("f{i}"));
        assert!(close(f.common.h, (i % 40 + 1) as f64 * MM), "f{i}: {:?}", f.common);
        assert_eq!(f.common.font.size, Some(9.0), "f{i}");
    }
}

#[test]
fn a_long_reference_shared_by_many_elements_is_resolved_once() {
    // One prototype whose own reference is a 1 MB `use` id, used by 60,000 fields: the output
    // stays small, so only reading that reference again for every field (60 GB of text) could
    // make this slow. A reference is resolved once per element.
    let long = "x".repeat(1 << 20);
    let styles = format!(r#"<field id="P" w="1mm" use="{long}"/><field id="{long}" h="2mm"/>"#);
    let body = r##"<field use="#P"/>"##.repeat(60_000).replacen("<field ", r#"<field name="first" "#, 1);
    let xml = form(&styles, &body);
    let (out, warnings, took) = expand_timed(&xml);
    assert!(took.as_secs() < 3, "{took:?}");
    assert!(out.is_some(), "{warnings:?}");
    let (t, _) = parse(&xml).unwrap();
    let first = field(&t, "first");
    assert!(close(first.common.w, MM) && close(first.common.h, 2.0 * MM), "{:?}", first.common);
}

#[test]
fn a_som_expression_past_its_length_limit_is_not_followed() {
    // A SOM expression longer than any Designer writes is not evaluated: it is warned about as
    // a reference naming nothing, and the rest of the template resolves.
    let padded = format!("$template.{}#subform.designer__stylesheet.Q", " ".repeat(5_000));
    let styles = r#"<field name="Q" h="2mm"/>"#;
    let body = format!(
        r##"<field name="long" w="1in" usehref="#som({padded})"/><field name="short" w="1in" usehref="#som($template.#subform.designer__stylesheet.Q)"/>"##
    );
    let (t, warnings) = parse(&form(styles, &body)).unwrap();
    assert!(warnings.iter().any(|w| w.contains("names nothing")), "{warnings:?}");
    assert_eq!(field(&t, "long").common.h, None);
    assert!(close(field(&t, "short").common.h, 2.0 * MM));
}
