//! Typed wire form of a PDF outline item and its destination.
//!
//! The room shares the document outline (ISO 32000-2 §12.3.3) and its named
//! destinations (§12.3.2.3) as addressable operations, so the two shapes an
//! operation carries live here, next to the limits that bound them:
//!
//! - a [`Dest`](BookmarkDest) — where an item goes: a page and a point, or a
//!   named destination of the same document;
//! - a subtree — the copy an `outline.item.delete` keeps for its undo: whole
//!   nodes with their titles, destinations, colours, styles and open state, so
//!   the undo puts the same tree back rather than a flattened one.
//!
//! Parsing and validation are pure and bounded: every nested node is counted,
//! every title is measured, and a view is checked against the destination types
//! the PDF specification lists, before an adapter touches its document. The
//! crate has no socket, clock or file system, and compiles for the browser.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_collab_proto::{MAX_DEST_NAME, MAX_OUTLINE_COPY_NODES, MAX_OUTLINE_DEPTH, MAX_OUTLINE_ITEMS, MAX_OUTLINE_TITLE, MAX_PAGE_INDEX};
use pdfcraft_organize::{BookmarkDest, OutlineNode, view_arity};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutlineWireError {
    #[error("{0} is missing")]
    Missing(&'static str),
    #[error("{0} is not what it must be")]
    Malformed(&'static str),
    #[error("{field} is {len} bytes; the limit is {max}")]
    TooLong { field: &'static str, len: usize, max: usize },
    #[error("{field}: {count} is above the limit of {max}")]
    TooMany { field: &'static str, count: usize, max: usize },
    #[error("{0}")]
    Invalid(String),
}

type Result<T> = std::result::Result<T, OutlineWireError>;

/// A title must be non-blank and at most [`MAX_OUTLINE_TITLE`] bytes.
pub fn check_title(title: &str) -> Result<()> {
    if title.trim().is_empty() {
        return Err(OutlineWireError::Invalid("a bookmark title must not be blank".into()));
    }
    if title.len() > MAX_OUTLINE_TITLE {
        return Err(OutlineWireError::TooLong { field: "bookmark title", len: title.len(), max: MAX_OUTLINE_TITLE });
    }
    Ok(())
}

/// A name must be non-blank and at most [`MAX_DEST_NAME`] bytes.
pub fn check_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(OutlineWireError::Invalid("a named destination needs a name".into()));
    }
    if name.len() > MAX_DEST_NAME {
        return Err(OutlineWireError::TooLong { field: "named destination", len: name.len(), max: MAX_DEST_NAME });
    }
    Ok(())
}

/// Where an outline item goes, from the wire form:
///
/// ```json
/// {"kind": "page", "page": 3, "left": 40.0, "top": 720.0}
/// {"kind": "view", "page": 3, "view": {"kind": "FitR", "values": [0, 0, 200, 300]}}
/// {"kind": "named", "name": "chapter-1"}
/// {"kind": "nowhere"}
/// ```
///
/// `left` and `top` may be absent or `null` (the reader keeps its position in
/// that direction). A page above [`MAX_PAGE_INDEX`], a coordinate that is not
/// finite, a blank name and a view type that is not one of the specification's
/// (or one with the wrong number of values) are refused here, before an adapter
/// resolves them against its document.
pub fn dest_from_value(value: &Value) -> Result<BookmarkDest> {
    let kind = value.get("kind").and_then(Value::as_str).ok_or(OutlineWireError::Missing("destination kind"))?;
    match kind {
        "page" => Ok(BookmarkDest::Page { page: page_of(value)?, left: coordinate(value.get("left"))?, top: coordinate(value.get("top"))? }),
        "view" => {
            let page = page_of(value)?;
            let view = value.get("view").filter(|view| !view.is_null()).ok_or(OutlineWireError::Missing("destination view"))?;
            let (kind, values) = view_from_value(view)?;
            Ok(BookmarkDest::View { page, kind, values })
        }
        "named" => {
            let name = value.get("name").and_then(Value::as_str).ok_or(OutlineWireError::Missing("destination name"))?;
            check_name(name)?;
            Ok(BookmarkDest::Named { name: name.trim().to_owned() })
        }
        "nowhere" => Ok(BookmarkDest::Nowhere),
        other => Err(OutlineWireError::Invalid(format!("there is no destination kind {other:?}"))),
    }
}

fn page_of(value: &Value) -> Result<usize> {
    let page = value.get("page").and_then(Value::as_u64).ok_or(OutlineWireError::Missing("destination page"))?;
    let page = u32::try_from(page).map_err(|_| OutlineWireError::Malformed("destination page"))?;
    if page > MAX_PAGE_INDEX {
        return Err(OutlineWireError::Malformed("destination page"));
    }
    Ok(page as usize)
}

fn coordinate(value: Option<&Value>) -> Result<Option<f64>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let number = value.as_f64().ok_or(OutlineWireError::Malformed("destination coordinate"))?;
            if !number.is_finite() {
                return Err(OutlineWireError::Malformed("destination coordinate"));
            }
            Ok(Some(number))
        }
    }
}

/// The wire form of a destination (see [`dest_from_value`]).
pub fn dest_to_value(dest: &BookmarkDest) -> Value {
    match dest {
        BookmarkDest::Page { page, left, top } => json!({"kind": "page", "page": page, "left": left, "top": top}),
        BookmarkDest::View { page, kind, values } => {
            let view = json!({
                "kind": String::from_utf8_lossy(kind).into_owned(),
                "values": values.iter().map(|value| value.map_or(Value::Null, |v| json!(v))).collect::<Vec<_>>(),
            });
            json!({"kind": "view", "page": page, "view": view})
        }
        BookmarkDest::Named { name } => json!({"kind": "named", "name": name}),
        BookmarkDest::Nowhere => json!({"kind": "nowhere"}),
    }
}

/// How a bookmark looks, as an `outline.item.update`/`add` may change it: its
/// colour, its title's weight and slant, and whether it shows expanded. Every
/// field is optional, so an operation changes only what it names, and `None`
/// everywhere is an operation that changes nothing (the adapter refuses it).
///
/// A colour is `null` for the default black or three numbers from 0 to 1; a
/// style that is not a boolean and a colour that is not those three numbers are
/// refused here, before an adapter touches its document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemLook {
    /// `Some(None)` paints the title in the default black (`/C` removed).
    pub color: Option<Option<[f64; 3]>>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    /// Whether the item shows expanded. Only an item with children has a state
    /// to change; the adapter refuses it on one without.
    pub open: Option<bool>,
}

impl ItemLook {
    /// Whether the operation names no look at all.
    pub fn is_empty(&self) -> bool {
        self.color.is_none() && self.bold.is_none() && self.italic.is_none() && self.open.is_none()
    }
}

/// The look a payload names. An absent field is left alone; a colour that is not
/// `null` or three numbers from 0 to 1 is `Malformed("bookmark colour")`, and a
/// style or open state that is not a boolean is `Malformed` too.
pub fn look_from_value(value: &Value) -> Result<ItemLook> {
    Ok(ItemLook {
        color: match value.get("color") {
            None => None,
            Some(Value::Null) => Some(None),
            Some(color) => Some(Some(rgb_of(color)?)),
        },
        bold: flag(value, "bold")?,
        italic: flag(value, "italic")?,
        open: flag(value, "open")?,
    })
}

fn rgb_of(color: &Value) -> Result<[f64; 3]> {
    let components = color.as_array().ok_or(OutlineWireError::Malformed("bookmark colour"))?;
    let [r, g, b] = components.as_slice() else {
        return Err(OutlineWireError::Malformed("bookmark colour"));
    };
    let channel = |v: &Value| v.as_f64().filter(|v| (0.0..=1.0).contains(v)).ok_or(OutlineWireError::Malformed("bookmark colour"));
    Ok([channel(r)?, channel(g)?, channel(b)?])
}

fn flag(value: &Value, field: &'static str) -> Result<Option<bool>> {
    match value.get(field) {
        None => Ok(None),
        Some(value) => value.as_bool().map(Some).ok_or(OutlineWireError::Malformed(field)),
    }
}

/// The look as an accepted operation carries it: the fields the operation named,
/// and nothing for the ones it left alone.
pub fn look_to_value(look: &ItemLook) -> Value {
    let mut value = serde_json::Map::new();
    match look.color {
        None => {}
        Some(None) => {
            value.insert("color".into(), Value::Null);
        }
        Some(Some(rgb)) => {
            value.insert("color".into(), json!(rgb));
        }
    }
    for (field, flag) in [("bold", look.bold), ("italic", look.italic), ("open", look.open)] {
        if let Some(flag) = flag {
            value.insert(field.into(), Value::Bool(flag));
        }
    }
    Value::Object(value)
}

/// A subtree as the wire carries it: the copy an `outline.item.delete` keeps.
/// Nodes are objects with `title`, `page`, `view`, `color`, `italic`, `bold`,
/// `open` and `children`; a view is `{"kind": "XYZ", "values": [1.0, null, …]}`
/// or `null`. The shape is the engine's own [`OutlineNode`], so a copy writes
/// back through `Edit::ImportBookmarks` and keeps the item's nesting, its view
/// type and its open state.
pub fn tree_to_value(nodes: &[OutlineNode]) -> Value {
    Value::Array(nodes.iter().map(node_to_value).collect())
}

fn node_to_value(node: &OutlineNode) -> Value {
    let view = node.view.as_ref().map(|(kind, values)| {
        json!({
            "kind": String::from_utf8_lossy(kind).into_owned(),
            "values": values.iter().map(|value| value.map_or(Value::Null, |v| json!(v))).collect::<Vec<_>>(),
        })
    });
    json!({
        "title": node.title,
        "page": node.page,
        "view": view,
        "color": node.color.map(|color| json!(color.iter().map(|c| f64::from(*c)).collect::<Vec<_>>())),
        "italic": node.italic,
        "bold": node.bold,
        "open": node.open,
        "children": tree_to_value(&node.children),
    })
}

/// Parse and check a subtree from its wire form. Every node is counted against
/// [`MAX_OUTLINE_COPY_NODES`], every title against [`MAX_OUTLINE_TITLE`], the
/// nesting against [`MAX_OUTLINE_DEPTH`] (counting the levels above the copy),
/// and every view against the destination types the specification lists. A copy
/// that fails any of them is refused as a whole.
pub fn tree_from_value(value: &Value) -> Result<Vec<OutlineNode>> {
    let list = value.as_array().ok_or(OutlineWireError::Malformed("outline subtree"))?;
    let mut count = 0usize;
    let nodes = nodes_from(list, 0, &mut count)?;
    Ok(nodes)
}

fn nodes_from(list: &[Value], depth: usize, count: &mut usize) -> Result<Vec<OutlineNode>> {
    let mut nodes = Vec::with_capacity(list.len());
    for value in list {
        *count = count.saturating_add(1);
        if *count > MAX_OUTLINE_COPY_NODES {
            return Err(OutlineWireError::TooMany { field: "outline subtree nodes", count: *count, max: MAX_OUTLINE_COPY_NODES });
        }
        if depth >= MAX_OUTLINE_DEPTH {
            return Err(OutlineWireError::TooMany { field: "outline depth", count: depth.saturating_add(1), max: MAX_OUTLINE_DEPTH });
        }
        nodes.push(node_from(value, depth, count)?);
    }
    Ok(nodes)
}

fn node_from(value: &Value, depth: usize, count: &mut usize) -> Result<OutlineNode> {
    let title = value.get("title").and_then(Value::as_str).ok_or(OutlineWireError::Missing("bookmark title"))?;
    check_title(title)?;
    let page = match value.get("page") {
        None | Some(Value::Null) => None,
        Some(page) => {
            let page = page.as_u64().ok_or(OutlineWireError::Malformed("bookmark page"))?;
            let page = u32::try_from(page).map_err(|_| OutlineWireError::Malformed("bookmark page"))?;
            if page > MAX_PAGE_INDEX {
                return Err(OutlineWireError::Malformed("bookmark page"));
            }
            Some(page as usize)
        }
    };
    let view = value.get("view").filter(|view| !view.is_null()).map(view_from_value).transpose()?;
    // The node's own colour is an `f32`; a copy writes the file's value back
    // through it.
    let color = match value.get("color") {
        None | Some(Value::Null) => None,
        Some(color) => Some(rgb_of(color)?.map(|c| c as f32)),
    };
    let flag = |field: &'static str| value.get(field).and_then(Value::as_bool).unwrap_or(false);
    let open = value.get("open").and_then(Value::as_bool).unwrap_or(true);
    let children = match value.get("children") {
        None => Vec::new(),
        Some(children) => {
            let list = children.as_array().ok_or(OutlineWireError::Malformed("bookmark children"))?;
            nodes_from(list, depth.saturating_add(1), count)?
        }
    };
    Ok(OutlineNode { title: title.to_owned(), page, view, color, italic: flag("italic"), bold: flag("bold"), open, children })
}

/// A view: a destination type from Table 149 with exactly the number of values
/// that type takes, each finite (a hole is `null`).
fn view_from_value(value: &Value) -> Result<(Vec<u8>, Vec<Option<f64>>)> {
    let kind = value.get("kind").and_then(Value::as_str).ok_or(OutlineWireError::Missing("view kind"))?;
    let arity = view_arity(kind.as_bytes()).ok_or_else(|| OutlineWireError::Invalid(format!("there is no destination type {kind:?}")))?;
    let values = value.get("values").and_then(Value::as_array).ok_or(OutlineWireError::Missing("view values"))?;
    if values.len() != arity {
        return Err(OutlineWireError::Invalid(format!("a {kind} destination takes {arity} values, not {}", values.len())));
    }
    let mut numbers = Vec::with_capacity(arity);
    for value in values {
        let number = match value {
            Value::Null => None,
            value => {
                let number = value.as_f64().filter(|v| v.is_finite()).ok_or(OutlineWireError::Malformed("view value"))?;
                Some(number)
            }
        };
        numbers.push(number);
    }
    // The rectangle of a `/FitR` is all four numbers: one with a hole is not a
    // rectangle, and the engine would refuse to write it.
    if kind.as_bytes() == b"FitR" && numbers.iter().any(Option::is_none) {
        return Err(OutlineWireError::Invalid("a rectangle destination needs all four of its values".into()));
    }
    Ok((kind.as_bytes().to_vec(), numbers))
}

/// How many nodes a subtree holds, for the room's own accounting.
pub fn node_count(nodes: &[OutlineNode]) -> usize {
    let mut count: usize = 0;
    let mut stack: Vec<&[OutlineNode]> = vec![nodes];
    while let Some(list) = stack.pop() {
        count = count.saturating_add(list.len());
        for node in list {
            if !node.children.is_empty() {
                stack.push(&node.children);
            }
        }
    }
    count
}

/// Whether one outline item count fits [`MAX_OUTLINE_ITEMS`], which bounds the
/// document's whole outline rather than one copy.
pub fn check_total(items: usize) -> Result<()> {
    if items > MAX_OUTLINE_ITEMS {
        return Err(OutlineWireError::TooMany { field: "outline items", count: items, max: MAX_OUTLINE_ITEMS });
    }
    Ok(())
}

/// How deep a subtree nests, counting its own top level as 1.
pub fn subtree_depth(nodes: &[OutlineNode]) -> usize {
    if nodes.is_empty() {
        return 0;
    }
    let mut deepest = 0usize;
    let mut stack: Vec<(&[OutlineNode], usize)> = vec![(nodes, 1)];
    while let Some((list, depth)) = stack.pop() {
        deepest = deepest.max(depth);
        for node in list {
            if !node.children.is_empty() {
                stack.push((&node.children, depth.saturating_add(1)));
            }
        }
    }
    deepest
}

/// The encoded size of a subtree copy, for the frame bound.
pub fn copy_bytes(nodes: &[OutlineNode]) -> usize {
    serde_json::to_vec(&tree_to_value(nodes)).map_or(usize::MAX, |bytes| bytes.len())
}

#[cfg(test)]
mod tests;
