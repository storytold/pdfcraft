//! Property objects for XFA scripts: `this.border.fill.color.value`, `this.ui.oneOfChild`,
//! `this.validate.nullTest`, `getElement`, `isPropertySpecified` and the rest.
//!
//! XFA 3.3 ch. 3, "Properties vs. Children" (p. 78): a property is always there for a script,
//! whether the template writes it or not; one the template leaves out reads as its default
//! ("Property Defaults", p. 598; ch. 17 gives each) where forms read it, else as an empty value
//! that compares as "" and never throws, and `isPropertySpecified` says which is which. The
//! defaults here are the ones the property reads of 367 public forms need; more are added as
//! forms need them. The template's properties come in shared ([`XfaProps`] behind an `Arc`, never copied
//! per script); a property object is a Proxy naming its node and its path from there
//! (`border#0/edge#1/color#0`), so defaults need no storage. What a script writes is kept in
//! the run's overlay ([`super::XHost::prop_writes`]) and read back by later reads in the same
//! run; it changes nothing on the form yet.

use std::collections::HashMap;

use boa_engine::object::ObjectInitializer;
use boa_engine::object::builtins::{JsArray, JsProxy};
use boa_engine::property::Attribute;
use boa_engine::{Context, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction, js_string};

use super::host::host;
use super::nodes::node_object;
use super::{XHost, XfaKind};
use crate::host::text_line;
use crate::{arg, error, function, s, text};

/// One property element of a template object, as the template writes it: its tag, attributes,
/// text (elements that hold text, such as `picture`, `toolTip` or a `message`'s `text`) and its
/// own property elements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XfaProp {
    pub tag: String,
    pub attrs: Vec<(String, String)>,
    pub text: Option<std::sync::Arc<str>>,
    pub children: Vec<XfaProp>,
}

/// A template object's own attributes and its property elements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XfaProps {
    pub attrs: Vec<(String, String)>,
    pub elements: Vec<XfaProp>,
}

/// Deepest property path a script may name below an object (`ui > textEdit > border > edge >
/// color` is 5); deeper reads get an object that takes anything and holds nothing.
const MAX_PATH: usize = 8;
/// Most occurrences `getElement` reaches of a property the spec lets occur any number of times
/// (a `message`'s `text`), past those the template has.
const MAX_MULTI: usize = 64;
/// Most attributes a script may write to one property in a run (the spec's elements have at most
/// 20); past it more are left out and noted.
pub(crate) const MAX_WRITTEN_ATTRS: usize = 64;
/// Most properties one SOM expression resolves to (`row[*].border.edge[*]` over many rows):
/// past it the rest are left out.
const MAX_RESOLVED: usize = 4_096;
/// Most bytes of edges and corners copied in one run when a script reaches sides a border
/// doesn't list ([`XHost::instantiate`]): a field's whole border is under 2 KiB, so this is
/// thousands of fields' worth. Kept apart from the run's text budget, which is for what a
/// script writes.
pub(crate) const MAX_COPIED: usize = 8 << 20;

/// A node's members read from its properties: `mandatory`, the validation messages and the
/// colour shorthands, members of a field or draw that forms use in place of the property paths.
pub(crate) const NODE_MEMBERS: &[&str] =
    &["mandatory", "validationMessage", "formatMessage", "mandatoryMessage", "fillColor", "fontColor", "borderColor"];

// Choices for captions and items, each decided in one place:
// - which `<items>` a field's `items` reaches when it has a shown and a saved list:
//   [`FIELD_ITEMS_INDEX`];
// - `items.nodes` lists the entries as the template lists them ([`list_entries`]). The list
//   state `HNode::items` is the hook for PdfCraft's choice-list methods (`addItem`,
//   `clearItems`, from the Series 1 work, not on main yet), which will set it; nothing sets it
//   until then, so reads follow the template;
// - `caption.className` and `caption.value.oneOfChild.className`: the element's tag
//   ([`tag_of`]) and the value's content, `text` when the template gives none ([`schema`]);
// - `items` on a field with no `<items>`: an items object with no entries ([`list_count`]).
// Readers' results agree with each of these.

/// Which `<items>` element a field's `items` names when it has more than one (a shown and a
/// saved list): the first in the template, as a name in a SOM expression reaches the first of
/// its occurrences (XFA 3.3 p. 758 allows two).
pub(crate) const FIELD_ITEMS_INDEX: usize = 0;

/// The path of the property `name` a node reaches by name (`this.border`, `this.items`).
pub(crate) fn node_prop_path(name: &str) -> String {
    join("", name, if name == "items" { FIELD_ITEMS_INDEX } else { 0 })
}

/// The properties of an object scripts reach by name (`this.border`); `value` is the field's
/// value at the node, not its value element.
pub(crate) const NODE_PROPS: &[&str] = &[
    "border",
    "ui",
    "font",
    "margin",
    "para",
    "assist",
    "validate",
    "format",
    "occur",
    "break",
    "breakBefore",
    "breakAfter",
    "bind",
    "desc",
    "extras",
    "keep",
    "caption",
    "items",
    "calculate",
];

/// What the spec says of one property element that scripts lean on: the attributes forms read
/// with their defaults, its own properties with how often each may occur, its one-of properties
/// with the default one, and whether its text is its value. Only the defaults forms are known to
/// read are here; other unspecified attributes read as empty, and more are added as forms need
/// them.
struct Schema {
    attrs: &'static [(&'static str, &'static str)],
    props: &'static [(&'static str, usize)],
    one_of: &'static [&'static str],
    one_of_default: Option<&'static str>,
    content: bool,
}

const fn schema_of(attrs: &'static [(&'static str, &'static str)], props: &'static [(&'static str, usize)]) -> Schema {
    Schema { attrs, props, one_of: &[], one_of_default: None, content: false }
}

const fn holds_text() -> Schema {
    Schema { attrs: &[], props: &[], one_of: &[], one_of_default: None, content: true }
}

/// A colour property only: edges, corners and the fill types.
const COLOR: &[(&str, usize)] = &[("color", 1)];
/// `presence`, where an element has it (default `visible`).
const PRESENCE: &[(&str, &str)] = &[("presence", "visible")];

/// The schema of the element `tag` (XFA 3.3 ch. 17, page cited at each).
fn schema(tag: &str) -> Option<Schema> {
    Some(match tag {
        // p. 629
        "border" => schema_of(PRESENCE, &[("corner", 4), ("edge", 4), ("fill", 1), ("margin", 1)]),
        // p. 687 (and "The default edge color is black"): an edge past those listed is a
        // default one of its own, so all its attributes have their defaults here.
        "edge" => schema_of(&[("cap", "square"), ("presence", "visible"), ("stroke", "solid"), ("thickness", "0.5pt")], COLOR),
        // p. 663 ("The default color for a corner [is] black")
        "corner" => schema_of(
            &[("inverted", "0"), ("join", "square"), ("presence", "visible"), ("radius", "0in"), ("stroke", "solid"), ("thickness", "0.5pt")],
            COLOR,
        ),
        // p. 734: without a fill-type child "the type defaults to solid".
        "fill" => Schema {
            attrs: PRESENCE,
            props: COLOR,
            one_of: &["linear", "pattern", "radial", "solid", "stipple"],
            one_of_default: Some("solid"),
            content: false,
        },
        // p. 655; the default value depends on where the colour is, see `default_attr`.
        "color" => schema_of(&[("value", "0,0,0")], &[]),
        // p. 770, p. 820, p. 806, p. 839
        "linear" => schema_of(&[("type", "toRight")], COLOR),
        "radial" => schema_of(&[("type", "toEdge")], COLOR),
        "pattern" => schema_of(&[("type", "crossHatch")], COLOR),
        "stipple" => schema_of(&[], COLOR),
        // pp. 741-744: typeface "The default is Courier".
        "font" => schema_of(&[("size", "10pt"), ("typeface", "Courier"), ("weight", "normal")], &[("fill", 1)]),
        // p. 800
        "para" => schema_of(&[("hAlign", "left"), ("vAlign", "top")], &[]),
        // p. 776
        "margin" => schema_of(&[("bottomInset", "0in"), ("leftInset", "0in"), ("rightInset", "0in"), ("topInset", "0in")], &[]),
        // p. 877. Without a widget "the application default applies" (p. 186's example): a
        // text edit, as the layout draws such a field.
        "ui" => Schema {
            attrs: &[],
            props: &[("picture", 1)],
            one_of: &[
                "barcode",
                "button",
                "checkButton",
                "choiceList",
                "dateTimeEdit",
                "defaultUi",
                "imageEdit",
                "numericEdit",
                "passwordEdit",
                "signature",
                "textEdit",
            ],
            one_of_default: Some("textEdit"),
            content: false,
        },
        // The widgets' own border and margin: p. 650, 652, 669, 753, 784, 804, 832, 865.
        "checkButton" | "choiceList" | "imageEdit" | "passwordEdit" | "signature" => schema_of(&[], &[("border", 1), ("margin", 1)]),
        // The edit widgets also have a comb (p. 669, 784, 865).
        "dateTimeEdit" | "numericEdit" | "textEdit" => schema_of(&[], &[("border", 1), ("comb", 1), ("margin", 1)]),
        // p. 657: numberOfCells "The default for this attribute is 0."
        "comb" => schema_of(&[("numberOfCells", "0")], &[]),
        // p. 880 (the defaults are spelled out on p. 881)
        "validate" => schema_of(
            &[("formatTest", "warning"), ("nullTest", "disabled"), ("scriptTest", "error")],
            &[("message", 1), ("picture", 1), ("script", 1)],
        ),
        // pp. 641-642; override depends on whether there is a `<calculate>`, see `default_attr`.
        "calculate" => schema_of(&[], &[("extras", 1), ("message", 1), ("script", 1)]),
        // p. 782: any number of texts.
        "message" => schema_of(&[], &[("text", MAX_MULTI)]),
        // p. 863, p. 808, p. 872, p. 837, p. 827
        "text" | "picture" | "toolTip" | "speak" | "script" => holds_text(),
        // A value's content elements and a list's entries hold their value as text (p. 882,
        // p. 758); a rich text's is its words.
        "integer" | "decimal" | "float" | "date" | "time" | "dateTime" | "boolean" | "exData" => holds_text(),
        // p. 643: a label, its own value, font, para and margin.
        "caption" => {
            schema_of(&[("placement", "left"), ("presence", "visible"), ("reserve", "-1")], &[("font", 1), ("margin", 1), ("para", 1), ("value", 1)])
        }
        // p. 758: its entries are children, listed by `nodes`.
        "items" => schema_of(&[("presence", "visible"), ("save", "0")], &[]),
        // p. 786
        "occur" => schema_of(&[("initial", "1"), ("max", "1"), ("min", "1")], &[]),
        // p. 632
        "break" => schema_of(&[("after", "auto"), ("before", "auto")], &[]),
        // p. 623
        "bind" => schema_of(&[("match", "once"), ("ref", "")], &[("picture", 1)]),
        // pp. 762-763; intact depends on the container, see `default_attr`.
        "keep" => schema_of(&[("next", "none"), ("previous", "none")], &[]),
        // p. 609
        "assist" => schema_of(&[], &[("speak", 1), ("toolTip", 1)]),
        // p. 745
        "format" => schema_of(&[], &[("picture", 1)]),
        // p. 882: the content is one of these; a field's holds text unless it says otherwise.
        "value" => Schema {
            attrs: &[],
            props: &[],
            one_of: &["arc", "boolean", "date", "dateTime", "decimal", "exData", "float", "image", "integer", "line", "rectangle", "text", "time"],
            one_of_default: Some("text"),
            content: false,
        },
        _ => return None,
    })
}

/// The steps of a property path (`border#0/edge#1`), as (tag, occurrence); none for the node.
fn steps(path: &str) -> Vec<(&str, usize)> {
    path.split('/').filter(|p| !p.is_empty()).filter_map(|p| p.rsplit_once('#')).map(|(t, i)| (t, i.parse().unwrap_or(0))).collect()
}

/// `path` with the step `tag#index` added.
fn join(path: &str, tag: &str, index: usize) -> String {
    if path.is_empty() { format!("{tag}#{index}") } else { format!("{path}/{tag}#{index}") }
}

/// The tag of the property `path` names, empty for the node itself.
fn tag_of(path: &str) -> &str {
    steps(path).last().map_or("", |(t, _)| t)
}

/// Whether `tag` under an element tagged `parent` is one of a border's four sides: an edge or
/// a corner (a border has four of each, XFA 3.3 p. 629, and so has a rectangle, p. 818).
fn is_side(parent: &str, tag: &str) -> bool {
    matches!(tag, "edge" | "corner") && matches!(parent, "border" | "rectangle")
}

/// Whether a script made anything at `path` or under it in this run.
fn made(h: &XHost, node: usize, path: &str) -> bool {
    let below = format!("{path}/");
    h.prop_writes.get(&node).is_some_and(|m| m.keys().any(|k| k == path || k.starts_with(&below)))
}

/// The element a read of `path` reads: `path` itself, except that an edge or corner the border
/// doesn't list, and no script has made, reads as the one drawn on that side
/// ([`drawn_index`]), what the template and this run's writes say of that one included, so a
/// read shows what is drawn without copying anything. `None` past a border's fourth edge or
/// corner, however many the template lists: a border has four sides, and the layout draws the
/// first four.
fn read_path(h: &XHost, node: usize, path: &str) -> Option<String> {
    let mut parent = "";
    let (mut raw, mut seen) = (String::new(), String::new());
    for (tag, index) in steps(path) {
        raw = join(&raw, tag, index);
        let index = if is_side(parent, tag) {
            if index >= 4 {
                return None;
            }
            // A side's parent is a border or rectangle, never the node itself.
            let listed = template_prop_at(h, node, &seen).map_or(0, |p| p.children.iter().filter(|p| p.tag == tag).count());
            match drawn_index(listed, index) {
                Some(d) if index >= listed && !made(h, node, &raw) => d,
                _ => index,
            }
        } else {
            index
        };
        seen = join(&seen, tag, index);
        parent = tag;
    }
    Some(seen)
}

/// The template's element at exactly `path` under node `node`, when the template has it.
fn template_prop_at<'a>(h: &'a XHost, node: usize, path: &str) -> Option<&'a XfaProp> {
    let props = &h.nodes.get(node)?.props;
    let mut list = &props.elements;
    let mut found = None;
    for (tag, index) in steps(path) {
        let p = list.iter().filter(|p| p.tag == tag).nth(index)?;
        list = &p.children;
        found = Some(p);
    }
    found
}

/// The template's element a read of `path` reads ([`read_path`]), when the template has it.
fn template_prop<'a>(h: &'a XHost, node: usize, path: &str) -> Option<&'a XfaProp> {
    template_prop_at(h, node, &read_path(h, node, path)?)
}

/// What a script wrote in this run to the property a read of `path` reads ([`read_path`]).
fn written<'a>(h: &'a XHost, node: usize, path: &str) -> Option<&'a PropWrite> {
    h.prop_writes.get(&node)?.get(&read_path(h, node, path)?)
}

/// The template's elements directly under `path` (the node's own when `path` is empty).
fn template_children<'a>(h: &'a XHost, node: usize, path: &str) -> &'a [XfaProp] {
    if path.is_empty() {
        return h.nodes.get(node).map_or(&[], |n| n.props.elements.as_slice());
    }
    template_prop(h, node, path).map_or(&[], |p| p.children.as_slice())
}

/// The template's elements directly under `path` that a script reaches: a border's edges and
/// corners past the fourth are left out ([`read_path`]).
fn reachable_children<'a>(h: &'a XHost, node: usize, path: &str) -> Vec<(String, &'a XfaProp)> {
    let parent = tag_of(path);
    let mut seen: HashMap<&str, usize> = HashMap::new();
    template_children(h, node, path)
        .iter()
        .filter_map(|k| {
            let n = seen.entry(k.tag.as_str()).or_insert(0);
            let at = *n;
            *n = n.saturating_add(1);
            (!(is_side(parent, &k.tag) && at >= 4)).then(|| (join(path, &k.tag, at), k))
        })
        .collect()
}

/// The template's attributes at `path` (the node's own when `path` is empty).
fn template_attrs<'a>(h: &'a XHost, node: usize, path: &str) -> &'a [(String, String)] {
    if path.is_empty() {
        return h.nodes.get(node).map_or(&[], |n| n.props.attrs.as_slice());
    }
    template_prop(h, node, path).map_or(&[], |p| p.attrs.as_slice())
}

/// The default of attribute `name` of the property at `path`. A colour's depends on where it
/// is: a border's fill is white ("If a fill element is provided but it has no child color
/// element the color defaults to white", p. 734), a font's fill black (text "is drawn with a
/// solid black fill", p. 734), edges and corners black (p. 687, p. 663). A `keep`'s `intact` depends
/// on its container (p. 762): `contentArea` for a draw and for a subform laid out positioned
/// (its default layout, p. 841, p. 845) or as a row, `none` for a field and a flowing or table subform.
fn default_attr(h: &XHost, node: usize, path: &str, name: &str) -> Option<&'static str> {
    let st = steps(path);
    let tags: Vec<&str> = st.iter().map(|(t, _)| *t).collect();
    // An object with no `<border>` element reads its border's presence as `hidden`, as readers
    // do; with one that doesn't say, `visible` (p. 629).
    if let (["border"], "presence") = (tags.as_slice(), name)
        && template_prop(h, node, path).is_none()
    {
        return Some("hidden");
    }
    // "The default value for the calculate object's override property differs depending upon
    // whether the XML representation includes or does not include a calculate element" (p. 641):
    // `disabled` without one, `error` with one.
    if let (["calculate"], "override") = (tags.as_slice(), name) {
        return Some(if template_prop(h, node, path).is_some() { "error" } else { "disabled" });
    }
    if let (["keep"], "intact") = (tags.as_slice(), name) {
        let n = h.nodes.get(node)?;
        let layout = n.props.attrs.iter().find(|(k, _)| k == "layout").map_or("position", |(_, v)| v.as_str());
        let whole = match n.kind {
            XfaKind::Draw => true,
            XfaKind::Subform => matches!(layout, "position" | "row" | "rl-row"),
            _ => false,
        };
        return Some(if whole { "contentArea" } else { "none" });
    }
    if let ([.., parent, "fill", "color"], "value") = (tags.as_slice(), name)
        && *parent != "font"
    {
        return Some("255,255,255");
    }
    let tag = tags.last()?;
    schema(tag)?.attrs.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
}

/// What a script wrote to one property in this run.
#[derive(Default)]
pub(crate) struct PropWrite {
    attrs: Vec<(String, String)>,
    text: Option<String>,
}

/// The attribute `name` of the property at `path`, if anything gives it: this run's write, the
/// template, the spec's default.
fn attr(h: &XHost, node: usize, path: &str, name: &str) -> Option<String> {
    if let Some(w) = written(h, node, path)
        && let Some((_, v)) = w.attrs.iter().find(|(k, _)| k == name)
    {
        return Some(v.clone());
    }
    if let Some((_, v)) = template_attrs(h, node, path).iter().find(|(k, _)| k == name) {
        return Some(v.clone());
    }
    default_attr(h, node, path, name).map(str::to_string)
}

/// The text of the property at `path`: this run's write, else the template's, else empty.
fn content(h: &XHost, node: usize, path: &str) -> String {
    if let Some(t) = written(h, node, path).and_then(|w| w.text.clone()) {
        return t;
    }
    if let Some(t) = list_entry(h, node, path) {
        return t;
    }
    template_prop(h, node, path).and_then(|p| p.text.as_deref()).map(str::to_string).unwrap_or_default()
}

/// One `<items>` list's entries, each its tag and its text, as a script's list methods will
/// leave them (see `HNode::items`, which nothing sets yet).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ItemList {
    pub(crate) entries: Vec<(String, String)>,
}

/// The entries of the field's `<items>` number `list` (the first is the one the list shows),
/// from the list state (`HNode::items`, which nothing sets yet), else as the template lists
/// them.
pub(crate) fn list_entries(h: &XHost, node: usize, list: usize) -> Vec<(String, String)> {
    if let Some(lists) = h.nodes.get(node).and_then(|n| n.items.as_ref()) {
        return lists.get(list).map(|l| l.entries.clone()).unwrap_or_default();
    }
    template_prop(h, node, &join("", "items", list))
        .map(|p| p.children.iter().map(|c| (c.tag.clone(), c.text.as_deref().unwrap_or_default().to_string())).collect())
        .unwrap_or_default()
}

/// How many `<items>` lists the field has: the list state's, else the template's.
fn list_count(h: &XHost, node: usize) -> usize {
    match h.nodes.get(node).and_then(|n| n.items.as_ref()) {
        Some(lists) => lists.len(),
        None => template_children(h, node, "").iter().filter(|p| p.tag == "items").count(),
    }
}

/// The text of the list entry at `path` (`items#0/text#2`) when there is a list state; `None`
/// for any other path, or while the lists are as the template lists them.
fn list_entry(h: &XHost, node: usize, path: &str) -> Option<String> {
    h.nodes.get(node)?.items.as_ref()?;
    let st = steps(path);
    let [("items", list), (tag, nth)] = st.as_slice() else { return None };
    list_entries(h, node, *list).into_iter().filter(|(t, _)| t == tag).nth(*nth).map(|(_, text)| text)
}

/// The paths of the entries of the list at `path` (`items#1`), as [`list_entries`] gives them.
fn list_paths(h: &XHost, node: usize, path: &str) -> Option<Vec<String>> {
    let st = steps(path);
    let [("items", list)] = st.as_slice() else { return None };
    let mut seen: HashMap<String, usize> = HashMap::new();
    Some(
        list_entries(h, node, *list)
            .into_iter()
            .map(|(tag, _)| {
                let n = seen.entry(tag.clone()).or_insert(0);
                let at = *n;
                *n = n.saturating_add(1);
                join(path, &tag, at)
            })
            .collect(),
    )
}

/// Whether `name` is a property of the element `tag` scripts reach: one the spec gives it, or
/// one the template writes. The node itself (`tag` empty) has [`NODE_PROPS`].
fn is_child_prop(h: &XHost, node: usize, path: &str, name: &str) -> bool {
    let tag = tag_of(path);
    let by_spec = if path.is_empty() {
        NODE_PROPS.contains(&name)
    } else {
        schema(tag).is_some_and(|s| s.props.iter().any(|(p, _)| *p == name)) || one_of_child(h, node, path).as_deref() == Some(name)
    };
    by_spec || template_children(h, node, path).iter().any(|p| p.tag == name)
}

/// Whether `name` is an attribute of the property at `path`.
fn is_attr(h: &XHost, node: usize, path: &str, name: &str) -> bool {
    attr(h, node, path, name).is_some() || matches!(name, "id" | "use" | "usehref")
}

/// The value a property object stands for in comparisons and text: its text when it holds text
/// (`picture`, `toolTip`), else its `value` attribute (`color`), else empty.
fn value_of(h: &XHost, node: usize, path: &str) -> String {
    if schema(tag_of(path)).is_some_and(|s| s.content) {
        return content(h, node, path);
    }
    attr(h, node, path, "value").unwrap_or_default()
}

/// Where a property's `rawValue` is: its own text (an element that holds text), the text of
/// the first element under it that holds text (a `desc`'s first `text`), or else its `value`
/// attribute (a `color`).
enum RawSource {
    Text(String),
    Attr,
}

fn raw_source(h: &XHost, node: usize, path: &str) -> RawSource {
    if schema(tag_of(path)).is_some_and(|s| s.content) {
        return RawSource::Text(path.to_string());
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for k in template_children(h, node, path) {
        let n = seen.entry(k.tag.as_str()).or_insert(0);
        let at = *n;
        *n = n.saturating_add(1);
        if k.text.is_some() {
            return RawSource::Text(join(path, &k.tag, at));
        }
    }
    RawSource::Attr
}

/// `rawValue` of a property: read from where [`raw_source`] says, as a script last set it or as
/// the template gives it.
fn raw_value(h: &XHost, node: usize, path: &str) -> String {
    match raw_source(h, node, path) {
        RawSource::Text(p) => content(h, node, &p),
        RawSource::Attr => attr(h, node, path, "value").unwrap_or_default(),
    }
}

/// Whether `name` is one of the element's one-of properties but not the one it has (a value's
/// `exData` when it holds text).
fn is_other_one_of(h: &XHost, node: usize, path: &str, name: &str) -> bool {
    schema(tag_of(path)).is_some_and(|s| s.one_of.contains(&name)) && one_of_child(h, node, path).as_deref() != Some(name)
}

/// The one-of child of the property at `path` (a `ui`'s widget, a `fill`'s type): the one the
/// template gives, else the spec's default; `None` when the element has no one-of properties.
fn one_of_child(h: &XHost, node: usize, path: &str) -> Option<String> {
    let sch = schema(tag_of(path))?;
    let given = template_children(h, node, path).iter().find(|p| sch.one_of.contains(&p.tag.as_str())).map(|p| p.tag.clone());
    given.or_else(|| sch.one_of_default.map(str::to_string))
}

/// `isPropertySpecified(name)`: the template writes the attribute or property `name` here, or
/// this run's script set it.
fn specified(h: &XHost, node: usize, path: &str, name: &str) -> bool {
    if template_attrs(h, node, path).iter().any(|(k, _)| k == name) || template_children(h, node, path).iter().any(|p| p.tag == name) {
        return true;
    }
    if written(h, node, path).is_some_and(|w| w.attrs.iter().any(|(k, _)| k == name)) {
        return true;
    }
    // A write anywhere under the property `name` (any occurrence) makes it specified.
    let below = if path.is_empty() { format!("{name}#") } else { format!("{path}/{name}#") };
    h.prop_writes.get(&node).is_some_and(|m| m.keys().any(|p| p.starts_with(&below)))
}

/// How many of the property `name` `getElement` may reach under `path`: what the template has,
/// or as many as the spec lets occur.
fn occurrences(h: &XHost, node: usize, path: &str, name: &str) -> usize {
    // A field's lists: the list state's, else the template's.
    let given = if path.is_empty() && name == "items" {
        list_count(h, node)
    } else {
        reachable_children(h, node, path).iter().filter(|(_, p)| p.tag == name).count()
    };
    let by_spec = if path.is_empty() {
        usize::from(NODE_PROPS.contains(&name))
    } else {
        schema(tag_of(path)).map_or(0, |s| {
            s.props.iter().find(|(p, _)| *p == name).map_or(usize::from(one_of_child(h, node, path).as_deref() == Some(name)), |(_, n)| *n)
        })
    };
    given.max(by_spec)
}

/// The path of the element under `path` whose `name` attribute is `name` (a `desc`'s or
/// `message`'s named `text`, an `extras`' named value): the template's, else one a script named
/// so in this run.
fn named_child(h: &XHost, node: usize, path: &str, name: &str) -> Option<String> {
    // The template's children, by the name they have now (a script may have renamed one).
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for k in template_children(h, node, path) {
        let n = seen.entry(k.tag.as_str()).or_insert(0);
        let at = *n;
        *n = n.saturating_add(1);
        let p = join(path, &k.tag, at);
        let renamed = written(h, node, &p).and_then(|w| w.attrs.iter().find(|(a, _)| a == "name")).map(|(_, v)| v.as_str());
        let current = renamed.or_else(|| k.attrs.iter().find(|(a, _)| a == "name").map(|(_, v)| v.as_str()));
        if current == Some(name) {
            return Some(p);
        }
    }
    // Then elements a script added under that name.
    let mut named: Vec<&String> = h
        .prop_writes
        .get(&node)?
        .iter()
        .filter(|(p, w)| {
            p.rsplit_once('/').map_or(path.is_empty(), |(up, _)| up == path)
                && template_prop(h, node, p).is_none()
                && w.attrs.iter().any(|(k, v)| k == "name" && v == name)
        })
        .map(|(p, _)| p)
        .collect();
    named.sort();
    named.first().map(|p| (*p).clone())
}

/// Whether anything under `path` has a `name` attribute, in the template or as a script set it:
/// only then is a key looked up as a child's name.
fn has_named_children(h: &XHost, node: usize, path: &str) -> bool {
    template_children(h, node, path).iter().any(|k| k.attrs.iter().any(|(a, _)| a == "name"))
        || h.prop_writes.get(&node).is_some_and(|m| {
            m.iter().any(|(p, w)| p.rsplit_once('/').map_or(path.is_empty(), |(up, _)| up == path) && w.attrs.iter().any(|(a, _)| a == "name"))
        })
}

/// The SOM expression of the property at `path` of the node at `som`
/// (`form1[0].f[0].border[0].edge[1]`).
fn som_of(som: &str, path: &str) -> String {
    steps(path).iter().fold(som.to_string(), |acc, (tag, i)| format!("{acc}.{tag}[{i}]"))
}

/// The properties a relative SOM expression names from the property at `path`: steps
/// separated by dots, each a name (a child of that name, else the property of that class),
/// `#class`, with `[n]` or `[*]`; [`MAX_PATH`] steps at most.
fn resolve(h: &XHost, node: usize, path: &str, expr: &str) -> Vec<String> {
    let mut at = vec![path.to_string()];
    for step in expr.split('.').map(str::trim).filter(|s| !s.is_empty()).take(MAX_PATH) {
        let (name, index) = match step.split_once('[') {
            Some((n, rest)) => (n, Some(rest.trim_end_matches(']').trim())),
            None => (step, None),
        };
        let mut next = Vec::new();
        for p in &at {
            if steps(p).len() >= MAX_PATH {
                continue;
            }
            // Each element under `p` a script reaches, as (its path, its tag, its name attribute).
            let kids: Vec<(String, &str, Option<&str>)> = reachable_children(h, node, p)
                .into_iter()
                .map(|(at, k)| (at, k.tag.as_str(), k.attrs.iter().find(|(a, _)| a == "name").map(|(_, v)| v.as_str())))
                .collect();
            let all: Vec<String> = match name.strip_prefix('#') {
                // `oneOfChild`: the one-of property the element has.
                None if name == "oneOfChild" => one_of_child(h, node, p).map(|t| join(p, &t, 0)).into_iter().collect(),
                // `*`: every element there, else the one-of child (a `ui`'s widget).
                None if name == "*" => match kids.is_empty() {
                    true => one_of_child(h, node, p).map(|t| join(p, &t, 0)).into_iter().collect(),
                    false => kids.iter().map(|(k, _, _)| k.clone()).collect(),
                },
                // A name: the elements of that name, else the property of that class.
                None if kids.iter().any(|(_, _, n)| *n == Some(name)) => {
                    kids.iter().filter(|(_, _, n)| *n == Some(name)).map(|(k, _, _)| k.clone()).collect()
                }
                tag => {
                    let tag = tag.unwrap_or(name);
                    // `[*]` is what is there; an index may reach a default, as getElement does.
                    let count = match index {
                        Some("*") if p.is_empty() && tag == "items" => list_count(h, node),
                        Some("*") => kids.iter().filter(|(_, t, _)| *t == tag).count(),
                        _ => occurrences(h, node, p, tag),
                    }
                    .min(MAX_MULTI);
                    (0..count).map(|i| join(p, tag, i)).collect()
                }
            };
            match index {
                Some("*") => next.extend(all),
                Some(i) => next.extend(i.parse::<usize>().ok().and_then(|i| all.get(i).cloned())),
                None => next.extend(all.into_iter().take(1)),
            }
            if next.len() >= MAX_RESOLVED {
                next.truncate(MAX_RESOLVED);
                break;
            }
        }
        at = next;
        if at.is_empty() {
            break;
        }
    }
    at
}

/// A SOM expression from node `from` that runs into an object's properties
/// (`resolveNode("ui.#textEdit")`, `xfa.resolveNode("form1.page.f.border.edge[1]")`): the
/// objects the longest leading part names (each instance of `row[*]`), and the properties the
/// rest names from each, as (node, path). A child named as the property comes first, for each
/// object on its own: where one has such a child, what the rest names from it is listed in
/// place, as (child, "") (`row[*].border` with a field `border` in one row and only the property
/// in another). Empty when no part of it names a property, or the expression has more than
/// [`super::model::MAX_DEPTH`] steps.
pub(crate) fn resolve_into_props(h: &XHost, from: usize, expr: &str) -> Vec<(usize, String)> {
    let segs: Vec<&str> = expr.trim().split('.').collect();
    if segs.len() > super::model::MAX_DEPTH {
        return Vec::new();
    }
    for i in (0..segs.len()).rev() {
        let (Some(head), Some(rest)) = (segs.get(..i), segs.get(i..)) else { continue };
        let first = rest.first().map_or("", |s| s.trim());
        let first = first.split('[').next().unwrap_or(first);
        // `#border` names the property by its class (XFA 3.3 p. 96), past a child named `border`.
        let by_class = first.starts_with('#');
        let first = first.strip_prefix('#').unwrap_or(first);
        // Only an object's properties start the rest (the parser's property tags): the
        // prefix is resolved only then.
        if !(NODE_PROPS.contains(&first) || first == "value") {
            continue;
        }
        let nodes = if head.is_empty() { vec![from] } else { super::resolve_som(h, from, &head.join(".")) };
        let rest = rest.join(".");
        let found: Vec<(usize, String)> = nodes
            .into_iter()
            .filter(|&n| is_child_prop(h, n, "", first))
            // A draw's value element holds what it shows, which the property objects don't
            // carry: `value.#text` on a draw stays unresolved (null) rather than reading as
            // empty, until a node's value is its value element.
            .filter(|&n| !(first == "value" && h.nodes.get(n).is_some_and(|x| x.kind == XfaKind::Draw)))
            .flat_map(|n| {
                let named = !by_class
                    && h.nodes.get(n).is_some_and(|x| x.children.iter().any(|&c| h.nodes.get(c).is_some_and(|c| !c.gone && c.name == first)));
                // The child shadows the property for the whole rest: what the child lacks names nothing
                // here (a property of the child is found where the head names the child).
                if named {
                    super::resolve_som(h, n, &rest).into_iter().map(|c| (c, String::new())).collect()
                } else {
                    resolve(h, n, "", &rest).into_iter().map(|p| (n, p)).collect::<Vec<_>>()
                }
            })
            .take(MAX_RESOLVED)
            .collect();
        // Children alone are no property: the expression names nodes, as `resolve_som` finds them.
        if found.iter().any(|(_, p)| !p.is_empty()) {
            return found;
        }
    }
    Vec::new()
}

/// `resolveNode(som)` on a property: the first property the expression names, or null.
fn resolve_one(_: &JsValue, args: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let (node, path) = prop_ref(r, ctx)?;
    let expr = text_line(&arg(args, 0), ctx)?;
    let found = resolve(&host(ctx)?.borrow(), node, &path, &expr).into_iter().next();
    match found {
        Some(p) if p != path => {
            host(ctx)?.borrow_mut().instantiate(node, &p);
            prop_object(ctx, node, &p)
        }
        _ => Ok(JsValue::null()),
    }
}

/// `resolveNodes(som)` on a property: every property the expression names, as a list.
fn resolve_many(_: &JsValue, args: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let (node, path) = prop_ref(r, ctx)?;
    let expr = text_line(&arg(args, 0), ctx)?;
    let found: Vec<(usize, String)> =
        resolve(&host(ctx)?.borrow(), node, &path, &expr).into_iter().filter(|p| *p != path).map(|p| (node, p)).collect();
    {
        let mut h = host(ctx)?.borrow_mut();
        found.iter().for_each(|(n, p)| h.instantiate(*n, p));
    }
    list_object(ctx, found)
}

/// A validation's messages are the `text`s of its `message`, named for the test they report
/// (XFA 3.3 p. 782): what a node's `validationMessage`, `formatMessage` and `mandatoryMessage`
/// read and write.
pub(crate) fn message_test(member: &str) -> Option<&'static str> {
    match member {
        "validationMessage" => Some("scriptTest"),
        "formatMessage" => Some("formatTest"),
        "mandatoryMessage" => Some("nullTest"),
        _ => None,
    }
}

/// A validation's message list.
const MESSAGE: &str = "validate#0/message#0";

/// The text that holds the validation message for `test` of node `node`: the message's text
/// of that name, else its one text when that has no name ("If the message element contains a
/// single un-named text element, the message it contains is used for all messages issued by
/// the enclosing validate element", XFA 3.3 p. 782).
fn message_path(h: &XHost, node: usize, test: &str) -> Option<String> {
    if let Some(p) = named_child(h, node, MESSAGE, test) {
        return Some(p);
    }
    let texts = template_children(h, node, MESSAGE).iter().filter(|p| p.tag == "text").count();
    let unnamed = attr(h, node, &join(MESSAGE, "text", 0), "name").is_none_or(|n| n.is_empty());
    (texts == 1 && unnamed).then(|| join(MESSAGE, "text", 0))
}

/// The tests a validation reports, in the order their added message texts take after the
/// template's.
const TESTS: [&str; 3] = ["scriptTest", "formatTest", "nullTest"];

/// Where a message text named for `test` goes when it is added: after the template's texts,
/// one place for each test.
fn message_slot(h: &XHost, node: usize, test: &str) -> String {
    let given = template_children(h, node, MESSAGE).iter().filter(|p| p.tag == "text").count();
    let slot = TESTS.iter().position(|t| *t == test).unwrap_or(0);
    join(MESSAGE, "text", given.saturating_add(slot))
}

/// The validation message for `test` of node `node` (see [`message_path`]), as a script last
/// set it or as the template gives it, else empty.
pub(crate) fn node_message(h: &XHost, node: usize, test: &str) -> String {
    message_path(h, node, test).map(|p| content(h, node, &p)).unwrap_or_default()
}

/// `mandatory`: the validation's `nullTest` (XFA 3.3 p. 880), `disabled` when the template
/// gives none.
pub(crate) fn node_mandatory(h: &XHost, node: usize) -> String {
    attr(h, node, "validate#0", "nullTest").unwrap_or_default()
}

/// The colour property behind a node's `fillColor`, `fontColor` or `borderColor` shorthand:
/// the border's fill colour, the font's fill colour and the first edge's colour.
pub(crate) fn shorthand_color(member: &str) -> Option<&'static str> {
    match member {
        "fillColor" => Some("border#0/fill#0/color#0"),
        "fontColor" => Some("font#0/fill#0/color#0"),
        "borderColor" => Some("border#0/edge#0/color#0"),
        _ => None,
    }
}

/// A node's colour shorthand read: the colour's `value` (see [`shorthand_color`]), with its
/// default where the template has none.
pub(crate) fn node_color(h: &XHost, node: usize, path: &str) -> String {
    attr(h, node, path, "value").unwrap_or_default()
}

/// Which of the `listed` edges (or corners) a border draws on side `index` when it lists fewer
/// than four. XFA 3.3 p. 38 says the last one listed is reused for the rest; readers draw a
/// side not listed from the edge or corner on the same side instead, and reads follow them:
/// one serves every side, two alternate (0, 1, 0, 1), three reuse the second for the fourth
/// (0, 1, 2, 1). `None` past the four sides, and when none is
/// listed (the side is a default one). The layout picks edges and corners with the same
/// pattern (`drawn_side` in `pdfcraft_xfa`, which this crate doesn't depend on); tests on both
/// sides pin it.
fn drawn_index(listed: usize, index: usize) -> Option<usize> {
    match listed {
        // A border has four sides: edges and corners occur at most four times (p. 629).
        _ if index >= 4 => None,
        0 => None,
        _ if index < listed => Some(index),
        1 => Some(0),
        2 => Some(index % 2),
        _ => Some(1),
    }
}

/// An element under `path` as (its path below the element, its attributes, its text): the
/// template's values with this run's writes over them, to [`MAX_PATH`] levels.
type Copied = Vec<(String, Vec<(String, String)>, Option<String>)>;

fn copy_of_element(h: &XHost, node: usize, path: &str, below: &str, out: &mut Copied, depth: usize) {
    let Some(p) = template_prop(h, node, path) else { return };
    let mut attrs = p.attrs.clone();
    if let Some(w) = written(h, node, path) {
        for (k, v) in &w.attrs {
            match attrs.iter_mut().find(|(a, _)| a == k) {
                Some(slot) => slot.1.clone_from(v),
                None => attrs.push((k.clone(), v.clone())),
            }
        }
    }
    out.push((below.to_string(), attrs, Some(content(h, node, path)).filter(|t| !t.is_empty())));
    if depth >= MAX_PATH {
        return;
    }
    let mut seen: HashMap<&str, usize> = HashMap::new();
    for k in &p.children {
        let n = seen.entry(k.tag.as_str()).or_insert(0);
        let at = *n;
        *n = n.saturating_add(1);
        copy_of_element(h, node, &join(path, &k.tag, at), &join(below, &k.tag, at), out, depth + 1);
    }
    if depth > 0 {
        return;
    }
    // What a script made under the element that the template doesn't list (a colour set on an
    // edge the template gives without one) is part of what is drawn there, so it is copied too.
    let prefix = format!("{path}/");
    let Some(writes) = h.prop_writes.get(&node) else { return };
    let mut made: Vec<(&String, &PropWrite)> =
        writes.iter().filter(|(k, _)| k.starts_with(&prefix) && template_prop(h, node, k).is_none() && steps(k).len() <= MAX_PATH).collect();
    made.sort_by(|a, b| a.0.cmp(b.0));
    for (k, w) in made {
        let rel = k.get(prefix.len()..).unwrap_or_default();
        let at = if below.is_empty() { rel.to_string() } else { format!("{below}/{rel}") };
        out.push((at, w.attrs.clone(), w.text.clone()));
    }
}

impl XHost {
    /// Make every edge or corner `path` names past those its border lists exist, each as a
    /// copy of the one drawn on that side (see [`drawn_index`]), as readers do when a script
    /// reaches it: the drawing is unchanged, and a later write changes only that side. Nothing
    /// is made for an element that exists, one a script already made, or a side of a border
    /// that lists none (that side is a default one already). Copies are charged to their own
    /// budget ([`MAX_COPIED`]), not to the run's text budget, so reads never crowd out a
    /// script's writes; past it nothing more is copied, and such a side still reads as the one
    /// drawn there ([`read_path`]).
    pub(crate) fn instantiate(&mut self, node: usize, path: &str) {
        let st: Vec<(String, usize)> = steps(path).into_iter().map(|(t, i)| (t.to_string(), i)).collect();
        let mut parent = String::new();
        for (tag, index) in st {
            let own = join(&parent, &tag, index);
            if is_side(tag_of(&parent), &tag) {
                let listed = template_children(self, node, &parent).iter().filter(|p| p.tag == tag).count();
                if let Some(d) = drawn_index(listed, index).filter(|_| index >= listed && !made(self, node, &own)) {
                    let mut copied = Copied::new();
                    copy_of_element(self, node, &join(&parent, &tag, d), "", &mut copied, 0);
                    let bytes = copied.iter().fold(0usize, |n, (below, attrs, text)| {
                        let attrs = attrs.iter().fold(0usize, |m, (k, v)| m.saturating_add(k.len()).saturating_add(v.len()));
                        n.saturating_add(own.len())
                            .saturating_add(below.len())
                            .saturating_add(attrs)
                            .saturating_add(text.as_ref().map_or(0, String::len))
                    });
                    let room = self.copied.checked_add(bytes).filter(|c| *c <= MAX_COPIED);
                    if let Some(c) = room {
                        self.copied = c;
                        for (below, attrs, text) in copied {
                            let at = if below.is_empty() { own.clone() } else { format!("{own}/{below}") };
                            for (k, v) in attrs {
                                self.put_prop(node, &at, Some(&k), v);
                            }
                            if let Some(t) = text {
                                self.put_prop(node, &at, None, t);
                            }
                        }
                    }
                }
            }
            parent = own;
        }
    }

    /// A node's colour shorthand (`member`) set: kept for later reads in this run, with no effect
    /// yet. `borderColor` colours all four edges, making the ones the border doesn't list first
    /// (as copies of the edge drawn there, see [`XHost::instantiate`]). Reads come from edge 0.
    pub(crate) fn set_color(&mut self, node: usize, member: &str, value: String) {
        let Some(path) = shorthand_color(member) else { return };
        if member != "borderColor" {
            return self.write_prop(node, path, Some("value"), value);
        }
        for i in 0..4 {
            self.write_prop(node, &format!("border#0/edge#{i}/color#0"), Some("value"), value.clone());
        }
    }

    /// `mandatory`, `validationMessage`, `formatMessage` or `mandatoryMessage` set on node
    /// `node`: kept for later reads in this run (`mandatory` only to `disabled`, `error` or
    /// `warning`), with no effect yet.
    pub(crate) fn set_validation(&mut self, node: usize, member: &str, value: String) {
        match (member, message_test(member)) {
            ("mandatory", _) if matches!(value.as_str(), "disabled" | "error" | "warning") => {
                self.write_prop(node, "validate#0", Some("nullTest"), value)
            }
            (_, Some(test)) => self.set_message(node, test, value),
            _ => {}
        }
    }

    /// Set the message for `test`, as readers do: the text named for that test (as the
    /// template or a script named it), else a single unnamed text (which then reads the same
    /// for every test), else a text of that name added after the template's.
    fn set_message(&mut self, node: usize, test: &str, value: String) {
        let path = match message_path(self, node, test) {
            Some(p) => p,
            None => {
                let p = message_slot(self, node, test);
                self.write_prop(node, &p, Some("name"), test.to_string());
                p
            }
        };
        self.write_prop(node, &path, None, value);
    }

    /// Keep what a script wrote to the property at `path` of node `node` (an attribute, or its
    /// text when `name` is `None`) for later reads in this run, within the run's text budget;
    /// an edge or corner the border doesn't list is made first ([`XHost::instantiate`]).
    fn write_prop(&mut self, node: usize, path: &str, name: Option<&str>, value: String) {
        self.instantiate(node, path);
        self.store_prop(node, path, name, value);
    }

    /// [`XHost::write_prop`] at exactly `path`, without making anything first.
    fn store_prop(&mut self, node: usize, path: &str, name: Option<&str>, value: String) {
        let bytes = path.len().saturating_add(name.map_or(0, str::len)).saturating_add(value.len());
        if self.attr_room(node, path, name) && self.room(bytes) {
            self.put_prop(node, path, name, value);
        }
    }

    /// Whether the property at `path` can take the attribute `name` (one it has, or a new one
    /// within [`MAX_WRITTEN_ATTRS`]); a drop is counted when not.
    fn attr_room(&mut self, node: usize, path: &str, name: Option<&str>) -> bool {
        let existing = self.prop_writes.get(&node).and_then(|m| m.get(path));
        let new_attr = name.is_some_and(|n| !existing.is_some_and(|w| w.attrs.iter().any(|(k, _)| k == n)));
        if new_attr && existing.is_some_and(|w| w.attrs.len() >= MAX_WRITTEN_ATTRS) {
            self.dropped_prop_writes = self.dropped_prop_writes.saturating_add(1);
            return false;
        }
        true
    }

    /// Keep `value` at `path`, its bytes already charged.
    fn put_prop(&mut self, node: usize, path: &str, name: Option<&str>, value: String) {
        let w: &mut PropWrite = self.prop_writes.entry(node).or_default().entry(path.to_string()).or_default();
        match name {
            Some(name) => match w.attrs.iter_mut().find(|(k, _)| k == name) {
                Some(slot) => slot.1 = value,
                None => w.attrs.push((name.to_string(), value)),
            },
            None => w.text = Some(value),
        }
    }
}

/// The overlay: what a script wrote, by node, then by property path.
pub(crate) type PropWrites = HashMap<usize, HashMap<String, PropWrite>>;

// ── property objects ────────────────────────────────────────────────────────────────────────

const NODE: &str = "__xfaPropNode";
const PATH: &str = "__xfaPropPath";

/// A Proxy standing for the property at `path` of node `node`; past [`MAX_PATH`] steps, an
/// object that takes anything.
pub(crate) fn prop_object(ctx: &mut Context, node: usize, path: &str) -> JsResult<JsValue> {
    if steps(path).len() > MAX_PATH {
        return empty(ctx);
    }
    let target = ObjectInitializer::new(ctx)
        .property(JsString::from(NODE), JsValue::from(node as f64), Attribute::READONLY)
        .property(JsString::from(PATH), s(path), Attribute::READONLY)
        .build();
    let proxy = JsProxy::builder(target).get(prop_get).set(prop_set).has(prop_has).build(ctx)?;
    Ok(proxy.into())
}

/// The node and path a property object (or its Proxy target) stands for.
fn prop_ref(o: &JsObject, ctx: &mut Context) -> JsResult<(usize, String)> {
    let n = o.get(JsString::from(NODE), ctx)?.to_number(ctx)?;
    if !n.is_finite() || n < 0.0 {
        return Err(error("not an XFA property"));
    }
    let path = text(&o.get(JsString::from(PATH), ctx)?, ctx)?;
    Ok((n as usize, path))
}

/// A reference object for a node's own methods: the node and an empty path.
pub(crate) fn node_ref(ctx: &mut Context, node: usize) -> JsObject {
    ObjectInitializer::new(ctx)
        .property(JsString::from(NODE), JsValue::from(node as f64), Attribute::READONLY)
        .property(JsString::from(PATH), s(""), Attribute::READONLY)
        .build()
}

/// A method bound to the property (or node) `r` stands for.
fn method(ctx: &mut Context, r: &JsObject, f: fn(&JsValue, &[JsValue], &JsObject, &mut Context) -> JsResult<JsValue>) -> JsValue {
    function(ctx, NativeFunction::from_copy_closure_with_captures(f, r.clone())).into()
}

/// What a property object gives for an attribute or property it neither has nor has a default
/// for here: an object that converts to "" (so comparisons with a string are false and nothing
/// throws), answers its methods emptily, gives itself again for anything else and takes writes
/// without keeping them.
fn empty(ctx: &mut Context) -> JsResult<JsValue> {
    let target = ObjectInitializer::new(ctx).build();
    let proxy = JsProxy::builder(target).get(empty_get).set(empty_set).has(prop_has).build(ctx)?;
    Ok(proxy.into())
}

fn empty_get(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    fn blank(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
        Ok(s(""))
    }
    fn no(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
        Ok(JsValue::from(false))
    }
    fn none(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
        Ok(JsValue::null())
    }
    let key = arg(args, 1);
    if key.as_symbol().is_some() {
        return Ok(JsValue::undefined());
    }
    Ok(match text(&key, ctx)?.as_str() {
        "toString" | "valueOf" | "toJSON" | "getAttribute" => function(ctx, NativeFunction::from_fn_ptr(blank)).into(),
        "isPropertySpecified" => function(ctx, NativeFunction::from_fn_ptr(no)).into(),
        "getElement" => function(ctx, NativeFunction::from_fn_ptr(none)).into(),
        "setAttribute" => function(ctx, NativeFunction::from_fn_ptr(none)).into(),
        "value" | "className" | "name" => s(""),
        "length" | "then" => JsValue::undefined(),
        _ => empty(ctx)?,
    })
}

fn empty_set(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(true))
}

fn no_effect(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::undefined())
}

fn no_xml(_: &JsValue, _: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    Ok(s(""))
}

/// `name in property`: true for every name a read answers, which is all of them (a property
/// reads its default, anything else reads as an empty value), except a fill type the fill
/// doesn't have, whose read fails (see [`prop_get`]): `"pattern" in fill` is false on a solid
/// fill, so a script can test before it reads.
fn prop_has(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let key = arg(args, 1);
    let Some(t) = arg(args, 0).as_object() else { return Ok(JsValue::from(true)) };
    if key.as_symbol().is_some() {
        return Ok(JsValue::from(true));
    }
    let key = text(&key, ctx)?;
    // The empty object (no node or path) answers everything.
    let Ok((node, path)) = prop_ref(&t, ctx) else { return Ok(JsValue::from(true)) };
    let h = host(ctx)?.borrow();
    let fails = tag_of(&path) == "fill" && is_other_one_of(&h, node, &path, key.strip_prefix('#').unwrap_or(&key));
    Ok(JsValue::from(!fails))
}

/// A read on a property object: its class, one-of child, relatives, methods, attributes (with
/// the spec's defaults) and properties (present or default).
fn prop_get(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let key = arg(args, 1);
    if key.as_symbol().is_some() {
        return Ok(JsValue::undefined());
    }
    let key = text(&key, ctx)?;
    let t = arg(args, 0).as_object().ok_or_else(|| error("not an XFA property"))?.clone();
    let (node, path) = prop_ref(&t, ctx)?;
    match key.as_str() {
        NODE => return Ok(JsValue::from(node as f64)),
        PATH => return Ok(s(&path)),
        "className" => return Ok(s(tag_of(&path))),
        "getElement" => return Ok(method(ctx, &t, get_element)),
        "isPropertySpecified" => return Ok(method(ctx, &t, is_property_specified)),
        "getAttribute" => return Ok(method(ctx, &t, get_attribute)),
        "setAttribute" => return Ok(method(ctx, &t, set_attribute)),
        "toString" | "valueOf" | "toJSON" => return Ok(method(ctx, &t, prop_to_string)),
        "nodes" => return nodes_object(ctx, node, &path),
        "resolveNode" => return Ok(method(ctx, &t, resolve_one)),
        "resolveNodes" => return Ok(method(ctx, &t, resolve_many)),
        "somExpression" => {
            let som = host(ctx)?.borrow().nodes.get(node).map(|n| n.som.clone()).unwrap_or_default();
            return Ok(s(&som_of(&som, &path)));
        }
        "length" | "then" => return Ok(JsValue::undefined()),
        // Loading and saving XML (a rich text's `exData.loadXML`) is not modelled yet: the
        // call changes nothing, and saving gives empty text.
        "loadXML" => return Ok(function(ctx, NativeFunction::from_fn_ptr(no_effect)).into()),
        "saveXML" => return Ok(function(ctx, NativeFunction::from_fn_ptr(no_xml)).into()),
        "parent" => {
            return match path.rsplit_once('/') {
                Some((up, _)) => prop_object(ctx, node, up),
                None => node_object(ctx, node),
            };
        }
        _ => {}
    }
    enum Read {
        Text(String),
        Prop(String),
        Null,
        Empty,
        Missing,
        Failed,
    }
    let read = {
        let h = host(ctx)?.borrow();
        // A child named `key` comes before an attribute or property of that name, as for
        // nodes ("XFA SOM resolves the name clash in favor of the explicit naming", XFA 3.3
        // p. 99): `extras.id` is the extras' value named `id`, not its id attribute.
        let named = if key.starts_with('#') || !has_named_children(&h, node, &path) { None } else { named_child(&h, node, &path, &key) };
        match (named, key.as_str()) {
            (Some(p), _) => Read::Prop(p),
            // A message's text named for a test it has none for: readers raise an error.
            (None, k) if tag_of(&path) == "message" && TESTS.contains(&k) => Read::Missing,
            (None, "oneOfChild") => match one_of_child(&h, node, &path) {
                Some(tag) => Read::Prop(join(&path, &tag, 0)),
                None => Read::Null,
            },
            // A caption's `value` is its value element (p. 643), whose content holds the label.
            (None, "value") if is_child_prop(&h, node, &path, "value") => Read::Prop(join(&path, "value", 0)),
            (None, "value") => Read::Text(value_of(&h, node, &path)),
            // A fill type other than the one the fill has (its `pattern` when it is solid): the
            // fill can't hand one out, so the read fails and the fill keeps its type, as readers
            // do (p. 734: one fill type).
            (None, k) if tag_of(&path) == "fill" && is_other_one_of(&h, node, &path, k.strip_prefix('#').unwrap_or(k)) => Read::Failed,
            // A one-of property other than the one the element has (a caption's `exData` when it
            // holds text): there is none, so it reads as null (p. 598: at most one of them).
            (None, k) if is_other_one_of(&h, node, &path, k.strip_prefix('#').unwrap_or(k)) => Read::Null,
            (None, "rawValue") => Read::Text(raw_value(&h, node, &path)),
            (None, "name") => Read::Text(attr(&h, node, &path, "name").unwrap_or_default()),
            (None, k) if is_attr(&h, node, &path, k) => Read::Text(attr(&h, node, &path, k).unwrap_or_default()),
            (None, k) if is_child_prop(&h, node, &path, k) => Read::Prop(join(&path, k, 0)),
            // `#textEdit`: the property by its class name.
            (None, k) if k.strip_prefix('#').is_some_and(|c| is_child_prop(&h, node, &path, c)) => {
                Read::Prop(join(&path, k.get(1..).unwrap_or_default(), 0))
            }
            _ => Read::Empty,
        }
    };
    match read {
        Read::Text(v) => Ok(s(&v)),
        Read::Prop(p) => prop_object(ctx, node, &p),
        Read::Null => Ok(JsValue::null()),
        Read::Empty => empty(ctx),
        Read::Missing => Err(error(format!("the validation's message has no text named {key}"))),
        Read::Failed => Err(general_error(ctx, "operation failed")),
    }
}

/// An error a script can catch, named `GeneralError` as the object model's own failures are.
fn general_error(ctx: &mut Context, message: &str) -> JsError {
    let e = JsNativeError::error().with_message(message.to_string()).into_opaque(ctx);
    // Setting a data property on a fresh ordinary error object cannot fail; if it ever did, the
    // error keeps the name `Error` and is still thrown.
    let _ = e.set(js_string!("name"), js_string!("GeneralError"), false, ctx);
    JsError::from_opaque(e.into())
}

/// Names on a property object that are its methods and relatives, never attributes.
const RESERVED: &[&str] = &[
    NODE,
    PATH,
    "className",
    "oneOfChild",
    "parent",
    "nodes",
    "getElement",
    "isPropertySpecified",
    "getAttribute",
    "setAttribute",
    "resolveNode",
    "resolveNodes",
    "somExpression",
    "toString",
    "valueOf",
    "toJSON",
    "rawValue",
    "loadXML",
    "saveXML",
    "length",
    "then",
];

/// A write on a property object: its `value` (its text, or its `value` attribute) or an
/// attribute is kept for later reads in this run (a name the spec doesn't give it too, as the
/// form may know better); a property's name, or a method's, is ignored.
fn prop_set(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let key = arg(args, 1);
    if key.as_symbol().is_some() {
        return Ok(JsValue::from(true));
    }
    let key = text(&key, ctx)?;
    let t = arg(args, 0).as_object().ok_or_else(|| error("not an XFA property"))?.clone();
    let (node, path) = prop_ref(&t, ctx)?;
    let value = text_line(&arg(args, 2), ctx)?;
    let mut h = host(ctx)?.borrow_mut();
    if path.is_empty() {
        return Ok(JsValue::from(true));
    }
    // As for reads: a child named `key` comes first, and the write sets its value.
    let named = if key.starts_with('#') || !has_named_children(&h, node, &path) { None } else { named_child(&h, node, &path, &key) };
    if let Some(child) = named {
        h.write_raw(node, &child, value);
        return Ok(JsValue::from(true));
    }
    let is_content = schema(tag_of(&path)).is_some_and(|s| s.content);
    match key.as_str() {
        "value" if is_content => h.write_prop(node, &path, None, value),
        "rawValue" => h.write_raw(node, &path, value),
        k if !is_child_prop(&h, node, &path, k) && !RESERVED.contains(&k) => h.write_prop(node, &path, Some(k), value),
        _ => {}
    }
    Ok(JsValue::from(true))
}

impl XHost {
    /// Set the `rawValue` of the property at `path`, where [`raw_value`] reads it from.
    fn write_raw(&mut self, node: usize, path: &str, value: String) {
        match raw_source(self, node, path) {
            RawSource::Text(p) => self.write_prop(node, &p, None, value),
            RawSource::Attr => self.write_prop(node, path, Some("value"), value),
        }
    }
}

/// `toString()` / `valueOf()`: see [`value_of`].
fn prop_to_string(_: &JsValue, _: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let (node, path) = prop_ref(r, ctx)?;
    let v = value_of(&host(ctx)?.borrow(), node, &path);
    Ok(s(&v))
}

/// `getElement(name[, index])`: occurrence `index` (0 when left out) of the property `name`,
/// present or default; null for an occurrence the template doesn't have and the spec doesn't
/// allow, or a name that is no property here.
pub(crate) fn get_element(_: &JsValue, args: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let (node, path) = prop_ref(r, ctx)?;
    let name = text(&arg(args, 0), ctx)?;
    let index = match arg(args, 1) {
        v if v.is_undefined() => 0.0,
        v => v.to_number(ctx)?,
    };
    if !index.is_finite() || index < 0.0 || name.is_empty() || name.contains(['/', '#']) {
        return Ok(JsValue::null());
    }
    let index = index as usize;
    let found = {
        let h = host(ctx)?.borrow();
        (index < occurrences(&h, node, &path, &name)).then(|| join(&path, &name, index))
    };
    match found {
        Some(p) => {
            host(ctx)?.borrow_mut().instantiate(node, &p);
            prop_object(ctx, node, &p)
        }
        None => Ok(JsValue::null()),
    }
}

/// `isPropertySpecified(name)`: see [`specified`]. On a node, its own attributes count too.
pub(crate) fn is_property_specified(_: &JsValue, args: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let (node, path) = prop_ref(r, ctx)?;
    let name = text(&arg(args, 0), ctx)?;
    let yes = specified(&host(ctx)?.borrow(), node, &path, &name);
    Ok(JsValue::from(yes))
}

/// `getAttribute(name)`: the attribute's value (this run's write, the template's, the
/// default), empty when there is none. On a node, its name, presence and access as they are now.
pub(crate) fn get_attribute(_: &JsValue, args: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let (node, path) = prop_ref(r, ctx)?;
    let name = text(&arg(args, 0), ctx)?;
    let h = host(ctx)?.borrow();
    let v = if path.is_empty() {
        match (h.nodes.get(node), name.as_str()) {
            (Some(n), "name") => n.name.clone(),
            (Some(n), "presence") => n.presence.clone(),
            (Some(n), "access") => n.access.clone(),
            _ => template_attrs(&h, node, &path).iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone()).unwrap_or_default(),
        }
    } else {
        attr(&h, node, &path, &name).unwrap_or_default()
    };
    Ok(s(&v))
}

/// `setAttribute(value, name)` on a property: kept for later reads in this run.
fn set_attribute(_: &JsValue, args: &[JsValue], r: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
    let (node, path) = prop_ref(r, ctx)?;
    let value = text_line(&arg(args, 0), ctx)?;
    let name = text(&arg(args, 1), ctx)?;
    if !name.is_empty() && !path.is_empty() {
        host(ctx)?.borrow_mut().write_prop(node, &path, Some(&name), value);
    }
    Ok(JsValue::undefined())
}

/// `nodes` of a property object: the property elements the template writes under it.
fn nodes_object(ctx: &mut Context, node: usize, path: &str) -> JsResult<JsValue> {
    let paths: Vec<(usize, String)> = {
        let h = host(ctx)?.borrow();
        // A list's entries: the list state's, else the template's.
        if let Some(list) = list_paths(&h, node, path) {
            drop(h);
            return list_object(ctx, list.into_iter().map(|p| (node, p)).collect());
        }
        reachable_children(&h, node, path).into_iter().map(|(at, _)| (node, at)).collect()
    };
    list_object(ctx, paths)
}

/// A list of the properties at `items` (node, path), with `length`, `item(i)` and
/// `namedItem(name)` (the first whose `name` attribute is `name`, else whose class is).
pub(crate) fn list_object(ctx: &mut Context, items: Vec<(usize, String)>) -> JsResult<JsValue> {
    let len = items.len();
    let nodes = JsArray::from_iter(items.iter().map(|(n, _)| JsValue::from(*n as f64)), ctx);
    let paths = JsArray::from_iter(items.iter().map(|(_, p)| s(p)), ctx);
    let key = ObjectInitializer::new(ctx)
        .property(js_string!("nodes"), nodes, Attribute::READONLY)
        .property(js_string!("paths"), paths, Attribute::READONLY)
        .build();
    /// The (node, path) pairs the list holds.
    fn listed(key: &JsObject, ctx: &mut Context) -> JsResult<Vec<(usize, String)>> {
        let (Some(nodes), Some(paths)) = (key.get(js_string!("nodes"), ctx)?.as_object(), key.get(js_string!("paths"), ctx)?.as_object()) else {
            return Ok(Vec::new());
        };
        let (nodes, paths) = (JsArray::from_object(nodes)?, JsArray::from_object(paths)?);
        let n = paths.length(ctx)?;
        let mut out = Vec::new();
        for i in 0..n {
            let node = nodes.get(i, ctx)?.to_number(ctx)?;
            if node.is_finite() && node >= 0.0 {
                out.push((node as usize, text(&paths.get(i, ctx)?, ctx)?));
            }
        }
        Ok(out)
    }
    fn item(_: &JsValue, args: &[JsValue], key: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
        let i = arg(args, 0).to_number(ctx)?;
        if !i.is_finite() || i < 0.0 {
            return Ok(JsValue::null());
        }
        match listed(key, ctx)?.get(i as usize) {
            Some((node, p)) if p.is_empty() => super::nodes::node_object(ctx, *node),
            Some((node, p)) => prop_object(ctx, *node, p),
            None => Ok(JsValue::null()),
        }
    }
    fn named_item(_: &JsValue, args: &[JsValue], key: &JsObject, ctx: &mut Context) -> JsResult<JsValue> {
        let name = text(&arg(args, 0), ctx)?;
        let items = listed(key, ctx)?;
        let found = {
            let h = host(ctx)?.borrow();
            let named = |(n, p): &&(usize, String)| {
                if p.is_empty() { h.nodes.get(*n).is_some_and(|x| x.name == name) } else { attr(&h, *n, p, "name").is_some_and(|v| v == name) }
            };
            items.iter().find(named).or_else(|| items.iter().find(|(_, p)| !p.is_empty() && tag_of(p) == name)).cloned()
        };
        match found {
            Some((node, p)) if p.is_empty() => super::nodes::node_object(ctx, node),
            Some((node, p)) => prop_object(ctx, node, &p),
            None => Ok(JsValue::null()),
        }
    }
    let item_fn = function(ctx, NativeFunction::from_copy_closure_with_captures(item, key.clone()));
    let named = function(ctx, NativeFunction::from_copy_closure_with_captures(named_item, key));
    Ok(ObjectInitializer::new(ctx)
        .property(js_string!("length"), JsValue::from(len as f64), Attribute::READONLY)
        .property(js_string!("item"), item_fn, Attribute::all())
        .property(js_string!("namedItem"), named, Attribute::all())
        .build()
        .into())
}

#[cfg(test)]
#[path = "props_tests.rs"]
mod tests;
