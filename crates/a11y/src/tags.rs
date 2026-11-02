//! Properties of reachable indirect structure elements.

use pdfcraft_cos::{CosError, Dict, Document, ObjRef, Object, PdfString};

use crate::alt;

/// A tag that can be addressed by an edit. Inline dictionaries have no stable object reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagInfo {
    pub object: ObjRef,
    /// Nearest reachable indirect parent, if any. Inline ancestors are skipped.
    pub parent: Option<ObjRef>,
    /// Indentation level among reachable indirect tags.
    pub depth: usize,
    pub kind: String,
    pub title: Option<String>,
    pub actual_text: Option<String>,
    pub alt: Option<String>,
    pub language: Option<String>,
    /// A table's `Summary` attribute.
    pub summary: Option<String>,
    /// A header cell's `Scope` attribute (`Row`, `Column` or `Both`).
    pub scope: Option<String>,
    /// Zero-based page when `/Pg` is known.
    pub page: Option<usize>,
}

/// A marked-content reference directly owned by a structure tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagContentItem {
    pub page: Option<usize>,
    pub mcid: i64,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagProperty {
    Type,
    Title,
    ActualText,
    AlternateText,
    Language,
    /// A table's `Summary`: a Table attribute (an attribute object with `/O /Table` in `/A`).
    Summary,
    /// A header cell's `Scope` (a Table attribute): `Row`, `Column` or `Both`.
    Scope,
}

/// Where to place a structure tag relative to one of its direct siblings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagPlacement {
    Before,
    After,
}

impl TagProperty {
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "type" => Some(Self::Type),
            "title" => Some(Self::Title),
            "actual_text" => Some(Self::ActualText),
            "alternate_text" => Some(Self::AlternateText),
            "language" => Some(Self::Language),
            "summary" => Some(Self::Summary),
            "scope" => Some(Self::Scope),
            _ => None,
        }
    }

    fn key(self) -> &'static [u8] {
        match self {
            Self::Type => b"S",
            Self::Title => b"T",
            Self::ActualText => b"ActualText",
            Self::AlternateText => b"Alt",
            Self::Language => b"Lang",
            Self::Summary => b"Summary",
            Self::Scope => b"Scope",
        }
    }

    /// The standard structure type a Table attribute belongs to; `None` for the tag's own entries.
    fn table_attribute_of(self) -> Option<&'static str> {
        match self {
            Self::Summary => Some("Table"),
            Self::Scope => Some("TH"),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TagError {
    #[error("object {0} is not a reachable indirect structure tag")]
    NotATag(ObjRef),
    #[error("a tag type must be a nonempty PDF name using letters, digits, colon, period, underscore or hyphen")]
    InvalidType,
    #[error("tag property text is too long (maximum 65536 bytes)")]
    TooLong,
    #[error("both tags must be distinct, reachable, direct children of the same structure parent")]
    NotSiblings,
    #[error("the parent must list each tag exactly once in its /K array")]
    AmbiguousChildren,
    #[error("the tag is already at the requested position")]
    AlreadyThere,
    #[error("{0} applies only to {1} tags")]
    WrongTagType(&'static str, &'static str),
    #[error("a scope must be Row, Column or Both")]
    InvalidScope,
    #[error(transparent)]
    Cos(#[from] CosError),
}

/// Reorder two direct sibling tags in their parent's `/K` array. Content and page appearance
/// stay untouched; tagged reading order follows the new array order.
pub fn move_tag(doc: &mut Document, object: ObjRef, anchor: ObjRef, placement: TagPlacement) -> Result<(), TagError> {
    if object == anchor {
        return Err(TagError::NotSiblings);
    }
    let reachable = alt::elements(doc);
    if !reachable.iter().any(|e| e.object == object) || !reachable.iter().any(|e| e.object == anchor) {
        return Err(TagError::NotSiblings);
    }
    let parent_of = |r| doc.get(r).as_dict().and_then(|d| d.get(b"P").and_then(Object::as_ref));
    let Some(parent) = parent_of(object) else { return Err(TagError::NotSiblings) };
    if parent_of(anchor) != Some(parent) {
        return Err(TagError::NotSiblings);
    }
    let owner = doc.get(parent);
    let Some(children) = owner.as_dict().and_then(|d| d.get(b"K").and_then(Object::as_array)) else {
        return Err(TagError::AmbiguousChildren);
    };
    let one_position = |child| {
        let mut matches = children.iter().enumerate().filter(|(_, item)| item.as_ref() == Some(child));
        matches.next().and_then(|(index, _)| matches.next().is_none().then_some(index))
    };
    let (Some(from), Some(to)) = (one_position(object), one_position(anchor)) else { return Err(TagError::AmbiguousChildren) };
    let mut reordered = children.clone();
    let moved = reordered.remove(from);
    let target = if from < to { to - 1 } else { to };
    let destination = target + usize::from(placement == TagPlacement::After);
    if destination == from {
        return Err(TagError::AlreadyThere);
    }
    reordered.insert(destination, moved);
    doc.update_dict(parent, |d| d.set(b"K".to_vec(), Object::Array(reordered)))?;
    Ok(())
}

fn text(doc: &Document, d: &pdfcraft_cos::Dict, key: &[u8]) -> Option<String> {
    d.get(key).map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(PdfString::to_text))
}

/// List reachable, indirect tags in structure-tree order.
pub fn tags(doc: &Document) -> Vec<TagInfo> {
    let pages = pdfcraft_model::pages(doc);
    let page_index: std::collections::HashMap<ObjRef, usize> = pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
    alt::elements(doc)
        .into_iter()
        .filter_map(|e| {
            Some(TagInfo {
                object: e.object,
                parent: e.parent,
                depth: e.depth,
                kind: String::from_utf8_lossy(e.dict.name(b"S")?).into_owned(),
                title: text(doc, &e.dict, b"T"),
                actual_text: text(doc, &e.dict, b"ActualText"),
                alt: text(doc, &e.dict, b"Alt"),
                language: text(doc, &e.dict, b"Lang"),
                summary: table_attribute(doc, &e.dict, b"Summary").and_then(|o| o.as_string().map(PdfString::to_text)),
                scope: table_attribute(doc, &e.dict, b"Scope").and_then(|o| o.as_name().map(|n| String::from_utf8_lossy(n).into_owned())),
                page: e.page.and_then(|p| page_index.get(&p).copied()),
            })
        })
        .collect()
}

/// List marked-content references directly owned by an indirect tag.
pub fn tag_content(doc: &Document, object: ObjRef) -> Result<Vec<TagContentItem>, TagError> {
    let Some(element) = alt::elements(doc).into_iter().find(|e| e.object == object) else { return Err(TagError::NotATag(object)) };
    let pages = pdfcraft_model::pages(doc);
    let page_index: std::collections::HashMap<ObjRef, usize> = pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
    let mut marks = Vec::new();
    if let Some(kids) = element.dict.get(b"K") {
        direct_marks(doc, kids, element.page, &mut marks);
    }
    // Each page's content is parsed once, however many of its MCIDs the tag owns.
    let mut texts: std::collections::HashMap<usize, std::collections::HashMap<i64, String>> = std::collections::HashMap::new();
    Ok(marks
        .into_iter()
        .map(|(page, mcid)| {
            let page = page.and_then(|p| page_index.get(&p).copied());
            let text = page
                .and_then(|i| {
                    let page_texts = match texts.entry(i) {
                        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                        std::collections::hash_map::Entry::Vacant(e) => {
                            e.insert(pages.get(i).map(|p| crate::headings::mcid_texts(doc, &p.dict)).unwrap_or_default())
                        }
                    };
                    page_texts.get(&mcid).cloned()
                })
                .unwrap_or_default();
            TagContentItem { page, mcid, text }
        })
        .collect())
}

fn direct_marks(doc: &Document, item: &Object, page: Option<ObjRef>, out: &mut Vec<(Option<ObjRef>, i64)>) {
    match item {
        Object::Int(mcid) => out.push((page, *mcid)),
        Object::Array(items) => items.iter().for_each(|item| direct_marks(doc, item, page, out)),
        Object::Dict(dict) if dict.name(b"Type") == Some(b"MCR") => {
            if let Some(mcid) = dict.get(b"MCID").and_then(Object::as_int) {
                out.push((dict.get(b"Pg").and_then(Object::as_ref).or(page), mcid));
            }
        }
        Object::Ref(reference) => {
            if let Some(dict) = doc.get(*reference).as_dict()
                && dict.name(b"Type") == Some(b"MCR")
                && let Some(mcid) = dict.get(b"MCID").and_then(Object::as_int)
            {
                out.push((dict.get(b"Pg").and_then(Object::as_ref).or(page), mcid));
            }
        }
        _ => {}
    }
}

/// The attribute objects of a tag's `/A` entry (one object, or an array of them each optionally
/// followed by its revision number), as written: each entry and its revision number.
fn attribute_entries(doc: &Document, d: &Dict) -> (Vec<(Object, Option<Object>)>, bool) {
    let Some(a) = d.get(b"A") else { return (Vec::new(), false) };
    // An indirect array is read as its items.
    let resolved = doc.resolve(a);
    let Object::Array(items) = &*resolved else { return (vec![(a.clone(), None)], false) };
    let mut out: Vec<(Object, Option<Object>)> = Vec::new();
    for item in items {
        match (item, out.last_mut()) {
            (Object::Int(_), Some((_, revision @ None))) => *revision = Some(item.clone()),
            _ => out.push((item.clone(), None)),
        }
    }
    (out, true)
}

/// Whether attribute object `o` (resolved) is a Table attribute object.
fn is_table_attributes(o: &Object) -> bool {
    o.as_dict().is_some_and(|d| d.name(b"O") == Some(b"Table"))
}

/// The value of `key` in the first Table attribute object of tag `d` that has it.
fn table_attribute(doc: &Document, d: &Dict, key: &[u8]) -> Option<Object> {
    attribute_entries(doc, d).0.iter().map(|(o, _)| doc.resolve(o)).filter(|o| is_table_attributes(o)).find_map(|o| o.as_dict()?.get(key).cloned())
}

/// Set `key` in the Table attribute object of `object`'s `/A` entry that holds it (else the first
/// one, else a new one), or with `None` remove it from every Table attribute object that holds it.
/// An edited object is written into the tag's own `/A`, so an attribute object other tags share
/// keeps its value for them; one left with nothing but its owner goes, and so does an `/A` left
/// empty.
fn set_table_attribute(doc: &mut Document, object: ObjRef, key: &[u8], value: Option<Object>) -> Result<(), TagError> {
    let dict = doc.get(object).as_dict().cloned().ok_or(TagError::NotATag(object))?;
    let (entries, array) = attribute_entries(doc, &dict);
    // Each entry with its Table attributes (resolved), when it is a Table attribute object.
    let mut entries: Vec<(Object, Option<Object>, Option<Dict>)> = entries
        .into_iter()
        .map(|(o, r)| {
            let table = Some(doc.resolve(&o)).filter(|a| is_table_attributes(a)).and_then(|a| a.as_dict().cloned());
            (o, r, table)
        })
        .collect();
    let holding = |e: &(Object, Option<Object>, Option<Dict>)| e.2.as_ref().is_some_and(|d| d.contains(key));
    match value {
        Some(v) => match entries.iter().position(holding).or_else(|| entries.iter().position(|e| e.2.is_some())) {
            Some(i) => {
                if let Some((o, _, Some(attributes))) = entries.get_mut(i) {
                    attributes.set(key.to_vec(), v);
                    *o = Object::Dict(attributes.clone());
                }
            }
            None => {
                let mut attributes = Dict::new();
                attributes.set(b"O".to_vec(), Object::name("Table"));
                attributes.set(key.to_vec(), v);
                entries.push((Object::Dict(attributes), None, None));
            }
        },
        None => {
            if !entries.iter().any(holding) {
                // Nothing to clear.
                return Ok(());
            }
            for (o, _, table) in &mut entries {
                if let Some(attributes) = table.as_mut().filter(|a| a.contains(key)) {
                    attributes.remove(key);
                    *o = Object::Dict(attributes.clone());
                }
            }
            entries.retain(|(_, _, table)| !table.as_ref().is_some_and(|a| a.iter().all(|(k, _)| k.as_slice() == b"O")));
        }
    }
    let entries: Vec<(Object, Option<Object>)> = entries.into_iter().map(|(o, r, _)| (o, r)).collect();
    doc.update_dict(object, |d| match entries.as_slice() {
        [] => {
            d.remove(b"A");
        }
        [(only, None)] if !array => d.set(b"A".to_vec(), only.clone()),
        _ => d.set(b"A".to_vec(), Object::Array(entries.into_iter().flat_map(|(o, r)| std::iter::once(o).chain(r)).collect())),
    })?;
    Ok(())
}

/// Change one property of a reachable tag; `None` removes a text property. A table's summary and
/// a header cell's scope are Table attributes ([`TagProperty::Summary`], [`TagProperty::Scope`]),
/// set only on tags of that type after the role map.
pub fn set_tag_property(doc: &mut Document, object: ObjRef, property: TagProperty, value: Option<&str>) -> Result<(), TagError> {
    let Some(element) = alt::elements(doc).into_iter().find(|e| e.object == object) else {
        return Err(TagError::NotATag(object));
    };
    // ActualText may intentionally contain leading or trailing whitespace; preserve it.
    let value = value.filter(|s| !s.is_empty());
    if value.is_some_and(|s| s.len() > 65_536) {
        return Err(TagError::TooLong);
    }
    if property == TagProperty::Type
        && !value.is_some_and(|s| s.len() <= 127 && s.as_bytes().iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'.' | b'_' | b'-')))
    {
        return Err(TagError::InvalidType);
    }
    if let Some(owner) = property.table_attribute_of() {
        let role_map =
            alt::tree_root(doc).and_then(|r| r.get(b"RoleMap").map(|o| doc.resolve(o))).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
        let ty = element.dict.name(b"S").map(|s| alt::standard(doc, &role_map, s)).unwrap_or_default();
        if ty != owner.as_bytes() {
            let name = if property == TagProperty::Summary { "summary" } else { "scope" };
            return Err(TagError::WrongTagType(name, owner));
        }
        let value = match value {
            Some(s) if property == TagProperty::Scope => {
                if !matches!(s, "Row" | "Column" | "Both") {
                    return Err(TagError::InvalidScope);
                }
                Some(Object::name(s))
            }
            Some(s) => Some(Object::String(PdfString::text(s))),
            None => None,
        };
        return set_table_attribute(doc, object, property.key(), value);
    }
    doc.update_dict(object, |d| match value {
        Some(s) if property == TagProperty::Type => d.set(property.key().to_vec(), Object::name(s)),
        Some(s) => d.set(property.key().to_vec(), Object::String(PdfString::text(s))),
        None => {
            d.remove(property.key());
        }
    })?;
    Ok(())
}
