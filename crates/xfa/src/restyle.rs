//! Property changes scripts make to a form's objects (XFA 3.3 ch. 3, "Properties vs. Children",
//! p. 78): `this.border.fill.color.value = "255,0,0"`, `this.mandatory = "error"`,
//! `Row.occur.max = "5"`. Each change names the property by its path from the object
//! (`border#0/fill#0/color#0`, occurrences counted per tag) and sets one attribute of it, or its
//! text. Applied to the object's property tree, the change is read back by later scripts; the
//! properties it falls under (`border`, `validate`, …) are then read again by the same functions
//! that read the template ([`crate::parse::restyle_from`]), so the page, the field's flags and
//! the instance limits follow it the way they follow the template.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::model::{Area, Common, Draw, ExclGroup, Field, Node, Prop, Props, Subform, Value};
use crate::prop_rules;

/// Most property changes one form keeps, as many as it keeps presence or access changes. Of the
/// 245 dynamic forms of 500, opened and given 50 generic interactions, 79 keep property changes:
/// median 21, p90 186, p99 1,190, at most 1,431 (Ontario Works 0983, a tooltip for each field
/// and a text colour, on open), so 7 times the most measured.
pub const MAX_PROP_OVERRIDES: usize = 10_000;
/// Most bytes of SOM paths, property paths and values those changes hold together. Measured:
/// median 2,140 B, p90 33,835 B, p99 114,864 B, at most 240,936 B (Ontario Works 0983), so
/// about 4 times the most. Every change is read when the form opens and written again when a
/// save follows a change, so the bound is what a script looping over properties can add.
pub const MAX_PROP_OVERRIDE_BYTES: usize = 1 << 20;
/// Longest value one change may set (a tooltip or a screen-reader text): the property budget
/// of one whole object as the template is read ([`crate::parse::MAX_OBJECT_PROP_BYTES`]).
/// The longest measured is 546 B.
pub const MAX_PROP_VALUE: usize = crate::parse::MAX_OBJECT_PROP_BYTES;
/// Most steps a property path takes from its object (the parser keeps 8 levels).
const MAX_STEPS: usize = prop_rules::MAX_STEPS;
const _: () = assert!(
    prop_rules::MAX_STEPS == crate::parse::MAX_PROP_DEPTH
        && prop_rules::MAX_VALUE == crate::parse::MAX_OBJECT_PROP_BYTES
        && prop_rules::ITEM_LISTS == crate::parse::MAX_ITEM_LISTS
);
/// Most property changes one object keeps, copies of the edges and corners the scripting host
/// makes included (see [`crate::script::Overrides::set_prop`]). On the 245 dynamic forms of 500
/// an object takes at most 19, the deepest path 5 steps; a script
/// that sets every attribute forms set on all of a border's edges and corners, with the copies
/// the host makes of them, comes to about 40, so 64 leaves room.
/// What the changes make of one object's tree is bounded by the object's budget
/// ([`apply_props`]), not by this count.
pub const MAX_PROP_CHANGES_PER_OBJECT: usize = 64;

pub use prop_rules::check;
use prop_rules::{is_name, steps};

/// The key a change is kept under: the property path, then `@` and the attribute, or the path
/// alone for the element's text. A copy of an edge or corner the scripting host made (see
/// [`crate::script::Overrides::set_prop_copy`]) is kept under its key with `=` in front.
pub fn prop_key(path: &str, attr: Option<&str>) -> String {
    match attr {
        Some(a) => format!("{path}@{a}"),
        None => path.to_string(),
    }
}

/// The property path and attribute (`None`: the text) of a key [`prop_key`] made, a copy's
/// `=` left off.
pub fn split_key(key: &str) -> (&str, Option<&str>) {
    let key = key.strip_prefix('=').unwrap_or(key);
    match key.split_once('@') {
        Some((p, a)) => (p, Some(a)),
        None => (key, None),
    }
}

/// Elements and bytes (tags, attribute names and values, text) of the elements `list` holds,
/// all levels, as the template reader counts them against an object's budget.
fn size_of(list: &[Prop], depth: usize) -> (usize, usize) {
    if depth > MAX_STEPS.saturating_mul(2) {
        return (0, 0);
    }
    list.iter().fold((0usize, 0usize), |(n, b), p| {
        let own = p
            .tag
            .len()
            .saturating_add(p.attrs.iter().map(|(k, v)| k.len().saturating_add(v.len())).sum::<usize>())
            // A validation's or calculation's script text isn't charged by the reader either
            // (it has its own limit, `parse::MAX_VALIDATE_SCRIPT_BYTES`), and no change sets it.
            .saturating_add(if p.tag == "script" { 0 } else { p.text.as_ref().map_or(0, |t| t.len()) });
        let (cn, cb) = size_of(&p.children, depth + 1);
        (n.saturating_add(1).saturating_add(cn), b.saturating_add(own).saturating_add(cb))
    })
}

/// [`size_of`] the object's property elements `list` as the template reader charges them to
/// the object's budget: the entries of its first `crate::parse::MAX_ITEM_LISTS` item lists
/// are not charged (they have the list's own bounds), only each list's element and attributes.
fn charged_size(list: &[Prop]) -> (usize, usize) {
    let mut lists = 0usize;
    list.iter().fold((0usize, 0usize), |(n, b), p| {
        let (pn, pb) = if p.tag == "items" && lists < crate::parse::MAX_ITEM_LISTS {
            lists += 1;
            let own = p.tag.len().saturating_add(p.attrs.iter().map(|(k, v)| k.len().saturating_add(v.len())).sum::<usize>());
            (1, own)
        } else {
            size_of(std::slice::from_ref(p), 0)
        };
        (n.saturating_add(pn), b.saturating_add(pb))
    })
}

/// The property tree `base` with the changes `sets` (by [`prop_key`]) applied, in the order of
/// their keys, and whether any was left out for the object's budget. A change to a property
/// the tree doesn't have adds it, as an element of its own that starts empty, so its other
/// attributes read as their defaults. An edge or corner past the ones a border lists is added,
/// with those between, each as a copy of the element drawn on its side as it is at that point
/// ([`crate::parse::drawn_side`]; a default one when the border lists none), so only the side
/// changed looks different, as the scripting host reads it; once added, they are listed and
/// drawn as their own. Changes go by path, so a border's lower edges come before its higher
/// ones and a copy takes what was written to the edge it copies, as the scripting host reads
/// it; a copy the host made (an `=` key) gives the copied values itself, ahead of the writes
/// to the same property. The result stays within the
/// template reader's budget for one object (`parse::MAX_OBJECT_PROPS` elements,
/// `parse::MAX_OBJECT_PROP_BYTES` bytes): a change past it is left out. The entries of the
/// object's item lists are outside that budget, as the reader keeps them (see
/// [`charged_size`]), so a change names only an entry a list holds and never adds one: a
/// change to an entry the list doesn't have is left out, and the lists hold no more entries
/// than the template's. Keys that are no property path are skipped.
pub fn apply_props(base: &Props, sets: &BTreeMap<String, String>) -> (Props, bool) {
    let mut out = base.clone();
    let mut bytes = charged_size(&out.elements).1;
    let mut cut = false;
    // By path, a copy before a write to the same key (the host copies an element before a
    // script writes to it).
    let mut order: Vec<(&str, bool, &String, &String)> =
        sets.iter().map(|(k, v)| (k.strip_prefix('=').unwrap_or(k), !k.starts_with('='), k, v)).collect();
    order.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    for (_, _, key, value) in order {
        let (path, attr) = split_key(key);
        let Some(st) = steps(path) else { continue };
        // The tree and its charge before a change that adds elements: put back when the change
        // is left out at any step, so a refused change leaves no empty element behind.
        let mut undo: Option<(Vec<Prop>, usize)> = None;
        // An entry of a list the reader doesn't charge (see `charged_size`): only one the list
        // holds, as no budget would bound the entries a change could add.
        let uncharged = matches!(st.as_slice(), [("items", k), _, ..] if *k < crate::parse::MAX_ITEM_LISTS);
        if uncharged && find_mut(&mut out.elements, &st, 0).is_none() {
            cut = true;
            continue;
        }
        if find_mut(&mut out.elements, &st, 0).is_none() {
            // The change adds elements: made on a copy kept until they fit the budget.
            undo = Some((out.elements.clone(), bytes));
            add_missing(&mut out.elements, &st, 0);
            let (n, b) = charged_size(&out.elements);
            if n > crate::parse::MAX_OBJECT_PROPS || b > crate::parse::MAX_OBJECT_PROP_BYTES {
                if let Some((elements, _)) = undo {
                    out.elements = elements;
                }
                cut = true;
                continue;
            }
            bytes = b;
        }
        // Nor are an entry's bytes charged here: the path's own bounds hold them (an entry the
        // list holds, `MAX_VALUE` bytes).
        let Some(p) = find_mut(&mut out.elements, &st, 0) else { continue };
        let old = match attr {
            Some(name) => p.attrs.iter().find(|(k, _)| k == name).map_or(0, |(k, v)| k.len().saturating_add(v.len())),
            None => p.text.as_ref().map_or(0, |t| t.len()),
        };
        let new = attr.map_or(0, str::len).saturating_add(value.len());
        let grown = if uncharged { bytes } else { bytes.saturating_sub(old).saturating_add(new) };
        let fits = grown <= crate::parse::MAX_OBJECT_PROP_BYTES
            && match attr {
                Some(name) => p.attrs.len() < crate::parse::MAX_PROP_ATTRS || p.attrs.iter().any(|(k, _)| k == name),
                None => true,
            };
        if !fits {
            if let Some((elements, charged)) = undo {
                out.elements = elements;
                bytes = charged;
            }
            cut = true;
            continue;
        }
        match attr {
            Some(name) => match p.attrs.iter_mut().find(|(k, _)| k == name) {
                Some(slot) => slot.1 = value.to_string(),
                None => p.attrs.push((name.to_string(), value.to_string())),
            },
            None => p.text = Some(Arc::from(value.as_str())),
        }
        bytes = grown;
    }
    (out, cut)
}

/// Add what `steps` names under `list` and the tree doesn't have yet (see [`apply_props`]).
fn add_missing(list: &mut Vec<Prop>, steps: &[(&str, usize)], depth: usize) {
    let Some(((tag, index), rest)) = steps.split_first() else { return };
    if depth > MAX_STEPS {
        return;
    }
    let have = list.iter().filter(|p| p.tag == *tag).count();
    if have <= *index {
        // `index` is at most `prop_rules::MAX_INDEX` (an edge or corner at most 3), so this adds
        // a handful of elements: an item list's entries, which go further, are never added
        // (see `apply_props`).
        let added: Vec<Prop> = (have..=*index).map(|k| added_element(list, tag, have, k)).collect();
        let at = list.iter().rposition(|p| p.tag == *tag).map_or(list.len(), |i| i.saturating_add(1));
        for (k, p) in added.into_iter().enumerate() {
            list.insert(at.saturating_add(k).min(list.len()), p);
        }
    }
    if let Some(p) = list.iter_mut().filter(|p| p.tag == *tag).nth(*index) {
        add_missing(&mut p.children, rest, depth + 1);
    }
}

/// The element added as occurrence `k` of `tag` under `list`, which lists `have` of them (see
/// [`apply_props`]): for a border's edge or corner, a copy of the one drawn on that side as it
/// is now (a default edge or corner when none is listed), so a change to it changes only that
/// side; anything else starts empty. Which occurrence a script's change names is the scripting
/// host's to decide (`pdfcraft_js`), not this.
fn added_element(list: &[Prop], tag: &str, have: usize, k: usize) -> Prop {
    let drawn = matches!(tag, "edge" | "corner")
        .then(|| crate::parse::drawn_side(have, k))
        .flatten()
        .and_then(|i| list.iter().filter(|p| p.tag == tag).nth(i));
    drawn.cloned().unwrap_or_else(|| Prop { tag: tag.to_string(), ..Prop::default() })
}

/// The element `steps` names under `list`.
fn find_mut<'a>(list: &'a mut [Prop], steps: &[(&str, usize)], depth: usize) -> Option<&'a mut Prop> {
    let ((tag, index), rest) = steps.split_first()?;
    if depth > MAX_STEPS {
        return None;
    }
    let p = list.iter_mut().filter(|p| p.tag == *tag).nth(*index)?;
    if rest.is_empty() { Some(p) } else { find_mut(&mut p.children, rest, depth + 1) }
}

/// `s` escaped for XML text and attribute values; characters XML 1.0 can't hold are left out.
fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
}

/// `attrs` as XML attributes, each name once (the first), names that are no XML name left out.
fn write_attrs(attrs: &[(String, String)], out: &mut String) {
    let mut seen = BTreeSet::new();
    for (k, v) in attrs {
        if !is_name(k) || !seen.insert(k.as_str()) {
            continue;
        }
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        escape(v, out);
        out.push('"');
    }
}

fn write_element(p: &Prop, out: &mut String, depth: usize) {
    if !is_name(&p.tag) || depth > MAX_STEPS.saturating_mul(2) {
        return;
    }
    out.push('<');
    out.push_str(&p.tag);
    write_attrs(&p.attrs, out);
    out.push('>');
    if let Some(t) = &p.text {
        escape(t, out);
    }
    for c in &p.children {
        write_element(c, out, depth + 1);
    }
    out.push_str("</");
    out.push_str(&p.tag);
    out.push('>');
}

/// An object's property tree as XML: an element with the object's attributes holding its
/// property elements, which the template reader reads like the object itself.
fn props_xml(props: &Props) -> String {
    let mut out = String::from("<object");
    write_attrs(&props.attrs, &mut out);
    out.push('>');
    for e in &props.elements {
        write_element(e, &mut out, 0);
    }
    out.push_str("</object>");
    out
}

/// Whether the change kept under `key` ([`prop_key`]) can change how the object is laid out or
/// what its field is: not one to a screen-reader text (`assist.speak`), the object's metadata
/// (`desc`) or its named values (`extras`), which only scripts read. Changes that can't are put
/// in the property tree without reading the object again.
pub fn draws(key: &str) -> bool {
    let key = key.trim_start_matches('=');
    !(key.starts_with("assist#0/speak#") || key.starts_with("desc#") || key.starts_with("extras#"))
}

/// Whether a change at `path` changes a field's value as the live form gives it to scripts:
/// its items (a radio button's on value, a list's saved values), its data or edit picture
/// (`bind`, `ui`, which also make it numeric) or its value element. The live form is built
/// again for such a change, not only refreshed ([`crate::refresh_props`]).
pub fn changes_live_value(path: &str) -> bool {
    let root = path.trim_start_matches('=').split('#').next().unwrap_or("");
    matches!(root, "items" | "bind" | "ui" | "value")
}

/// Whether the changes `sets` touch what the live form takes from a field besides its property
/// tree: its items (radio on values, saved values), data and edit pictures (`bind`, `ui`), its
/// validation (script, message), its display picture (`format`) or its value element. When
/// they don't, the live form needs only the tree with them applied ([`apply_props`]), not the
/// field read again.
pub(crate) fn live_field_changes(sets: &BTreeMap<String, String>) -> bool {
    sets.keys().any(|k| {
        let root = k.trim_start_matches('=').split('#').next().unwrap_or("");
        matches!(root, "items" | "bind" | "ui" | "validate" | "format" | "value")
    })
}

/// Apply the changes `sets` to `node`: its property tree, and what the properties they fall
/// under give its layout, fields and instances (see [`crate::parse::restyle_from`]). `true`
/// when not all of it reaches the object: a change past the object's budget is left out (see
/// [`apply_props`]), and an object whose template the reader cut short ([`Props::cut`]) is
/// not laid out from its property tree again (that tree lacks what the cut left out), so only
/// scripts see its changes.
pub(crate) fn restyle(node: &mut Node, sets: &BTreeMap<String, String>) -> bool {
    if sets.is_empty() {
        return false;
    }
    #[cfg(test)]
    RESTYLED.with(|c| c.set(c.get().saturating_add(1)));
    let (props, mut cut) = apply_props(&node.common().props, sets);
    if node.common().props.cut {
        cut = true;
    } else if sets.keys().any(|k| draws(k)) {
        let mut roots: BTreeSet<&str> = sets.keys().filter_map(|k| k.trim_start_matches('=').split('#').next()).collect();
        // A change to a caption's label, not only to how it is drawn.
        if sets.keys().any(|k| k.trim_start_matches('=').starts_with("caption#0/value#0")) {
            roots.insert("caption.value");
        }
        let xml = props_xml(&props);
        // Written from names and escaped text above, so it parses; should it not, the object
        // keeps its layout and only scripts see the change.
        match roxmltree::Document::parse(&xml) {
            Ok(doc) => crate::parse::restyle_from(node, doc.root_element(), &|t| roots.contains(t)),
            Err(_) => cut = true,
        }
    }
    node.common_mut().props = Arc::new(props);
    cut
}

#[cfg(test)]
thread_local! {
    /// Objects [`restyle`] read again, on this thread.
    static RESTYLED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many objects [`restyle`] has read again on this thread, since the last call.
#[cfg(test)]
pub(crate) fn take_restyled() -> usize {
    RESTYLED.with(|c| c.replace(0))
}

/// `node` alone, without what it holds (a subform's children and page set, a group's fields, a
/// draw's value) or its scripts: what [`restyle`] reads and changes, copied cheaply.
fn shallow(node: &Node) -> Node {
    match node {
        Node::Subform(s) => Node::Subform(Box::new(Subform {
            common: s.common.clone(),
            layout: s.layout,
            occur: s.occur,
            break_before_page: s.break_before_page,
            break_after_page: s.break_after_page,
            ..Subform::default()
        })),
        Node::Area(a) => Node::Area(Box::new(Area { common: a.common.clone(), children: Vec::new() })),
        Node::Draw(d) => Node::Draw(Box::new(Draw { common: d.common.clone(), value: Value::Empty })),
        Node::ExclGroup(g) => Node::ExclGroup(Box::new(ExclGroup {
            common: g.common.clone(),
            layout: g.layout,
            tooltip: g.tooltip.clone(),
            required: g.required,
            ..ExclGroup::default()
        })),
        Node::Field(f) => Node::Field(f.clone()),
    }
}

/// `node`'s own properties with the changes `sets` applied (see [`restyle`]), without what it
/// holds (see [`shallow`]).
pub(crate) fn restyled(node: &Node, sets: &BTreeMap<String, String>) -> Node {
    let mut n = shallow(node);
    restyle(&mut n, sets);
    n
}

/// Which side of an object the change kept under `key` can make the page break on (XFA 3.3
/// p. 632; see `parse::page_breaks`): before (`breakBefore`'s `targetType`, `break`'s `before`)
/// is `Some(true)`, after (`breakAfter`'s `targetType`) `Some(false)`; `None` for any other.
fn break_side(key: &str) -> Option<bool> {
    let (path, attr) = split_key(key);
    let (tag, _) = path.rsplit_once('#').filter(|_| !path.contains('/'))?;
    match (tag, attr?) {
        ("breakBefore", "targetType") | ("break", "before") => Some(true),
        ("breakAfter", "targetType") => Some(false),
        _ => None,
    }
}

/// Where the page breaks before and after one instance of subform `node` by the changes `sets`
/// made to that instance (XFA 3.3 p. 632), each side `None` when no change is to it. The
/// template's breaks go before the first instance and after the last only (the layout's rule):
/// `template_before` and `template_after` say whether this instance takes them. For one that
/// doesn't, a side the changes reach breaks as the changes alone say: the template's break on
/// that side doesn't come with them, so a change to where a row breaks after it doesn't make it
/// break before as well. An object whose template the reader cut short isn't laid out from its
/// tree (see [`restyle`]): the template's rule stands, `(None, None)`.
pub(crate) fn instance_breaks(
    node: &Node,
    sets: &BTreeMap<String, String>,
    template_before: bool,
    template_after: bool,
) -> (Option<bool>, Option<bool>) {
    let before = sets.keys().any(|k| break_side(k) == Some(true));
    let after = sets.keys().any(|k| break_side(k) == Some(false));
    if !(before || after) || node.common().props.cut {
        return (None, None);
    }
    let mut n = shallow(node);
    let props = Arc::make_mut(&mut n.common_mut().props);
    for e in &mut props.elements {
        let drop = match e.tag.as_str() {
            "breakBefore" if !template_before => "targetType",
            "break" if !template_before => "before",
            "breakAfter" if !template_after => "targetType",
            _ => continue,
        };
        e.attrs.retain(|(k, _)| k != drop);
    }
    restyle(&mut n, sets);
    match n {
        Node::Subform(s) => (before.then_some(s.break_before_page), after.then_some(s.break_after_page)),
        _ => (None, None),
    }
}

/// The common properties `c` of an object (its border, margins, font and paragraph, and its
/// property tree) with the changes `sets` applied (see [`restyle`]).
pub(crate) fn restyled_common(c: &Common, sets: &BTreeMap<String, String>) -> Common {
    let mut n = Node::Area(Box::new(Area { common: c.clone(), children: Vec::new() }));
    restyle(&mut n, sets);
    match n {
        Node::Area(a) => a.common,
        // `restyle` keeps the node's kind.
        other => other.common().clone(),
    }
}

/// Field `f` with the changes `sets` applied (see [`restyle`]).
pub(crate) fn restyled_field(f: &Field, sets: &BTreeMap<String, String>) -> Field {
    let mut n = Node::Field(Box::new(f.clone()));
    restyle(&mut n, sets);
    match n {
        Node::Field(f) => *f,
        // `restyle` keeps the node's kind.
        _ => f.clone(),
    }
}

#[cfg(test)]
#[path = "restyle_tests.rs"]
mod tests;
