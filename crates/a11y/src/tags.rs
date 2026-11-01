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
    #[error("the tag has too much or too deeply nested marked content")]
    ContentLimit,
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
    let Some(obj_elem) = reachable.iter().find(|e| e.object == object) else {
        return Err(TagError::NotSiblings);
    };
    let Some(anc_elem) = reachable.iter().find(|e| e.object == anchor) else {
        return Err(TagError::NotSiblings);
    };
    if obj_elem.parent != anc_elem.parent {
        return Err(TagError::NotSiblings);
    }
    let parent_of = |r| doc.get(r).as_dict().and_then(|d| d.get(b"P").and_then(Object::as_ref));
    let Some(parent) = parent_of(object) else { return Err(TagError::NotSiblings) };
    if parent_of(anchor) != Some(parent) {
        return Err(TagError::NotSiblings);
    }
    let valid_parent = match obj_elem.parent {
        Some(expected) => parent == expected,
        None => doc
            .root()
            .and_then(|r| doc.get(r).as_dict().and_then(|c| c.get(b"StructTreeRoot").and_then(Object::as_ref)))
            .is_some_and(|root_ref| parent == root_ref),
    };
    if !valid_parent {
        return Err(TagError::NotSiblings);
    }
    let owner = doc.get(parent);
    let Some(children) = owner.as_dict().and_then(|d| d.get(b"K")).map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) else {
        return Err(TagError::AmbiguousChildren);
    };
    let one_position = |child| {
        let mut matches = children.iter().enumerate().filter(|(_, item)| item.as_ref() == Some(child));
        matches.next().and_then(|(index, _)| matches.next().is_none().then_some(index))
    };
    let (Some(from), Some(to)) = (one_position(object), one_position(anchor)) else { return Err(TagError::AmbiguousChildren) };
    let mut reordered = children;
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

fn tag_type_name(doc: &Document, d: &Dict) -> Option<Vec<u8>> {
    d.get(b"S").map(|s| doc.resolve(s)).and_then(|s| s.as_name().map(<[u8]>::to_vec))
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
                kind: String::from_utf8_lossy(&tag_type_name(doc, &e.dict)?).into_owned(),
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
/// Returns `ContentLimit` rather than a partial list beyond 65,536 marks, one million graph steps,
/// or 512 child/reference levels. Child structure elements do not count as direct marks.
pub fn tag_content(doc: &Document, object: ObjRef) -> Result<Vec<TagContentItem>, TagError> {
    let Some(element) = alt::elements(doc).into_iter().find(|e| e.object == object) else { return Err(TagError::NotATag(object)) };
    let pages = pdfcraft_model::pages(doc);
    let page_index: std::collections::HashMap<ObjRef, usize> = pages.iter().enumerate().map(|(i, p)| (p.obj, i)).collect();
    let mut marks = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(kids) = element.dict.get(b"K") {
        direct_marks(doc, kids, element.page, &mut seen, &mut marks)?;
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

/// Bound both graph work and the queued array entries. Following references never uses the
/// call stack; the depth limit also rejects pathologically nested direct content explicitly.
fn direct_marks(
    doc: &Document,
    item: &Object,
    page: Option<ObjRef>,
    seen: &mut std::collections::HashSet<ObjRef>,
    out: &mut Vec<(Option<ObjRef>, i64)>,
) -> Result<(), TagError> {
    const MAX_DEPTH: usize = 512;
    const MAX_MARKS: usize = 65_536;
    let mut remaining = 1_000_000usize;
    let mut stack = vec![(std::sync::Arc::new(item.clone()), 0usize)];
    while let Some((item, depth)) = stack.pop() {
        remaining = remaining.checked_sub(1).ok_or(TagError::ContentLimit)?;
        if depth > MAX_DEPTH {
            return Err(TagError::ContentLimit);
        }
        match &*item {
            Object::Int(mcid) => {
                if out.len() >= MAX_MARKS {
                    return Err(TagError::ContentLimit);
                }
                out.push((page, *mcid));
            }
            Object::Array(items) => {
                if items.len() > remaining.saturating_sub(stack.len()) {
                    return Err(TagError::ContentLimit);
                }
                stack.extend(items.iter().rev().map(|item| (std::sync::Arc::new(item.clone()), depth + 1)));
            }
            Object::Dict(dict) if dict.get(b"Type").map(|t| doc.resolve(t)).as_deref().and_then(Object::as_name) == Some(b"MCR") => {
                if let Some(mcid) = dict.get(b"MCID").map(|m| doc.resolve(m)).and_then(|m| m.as_int()) {
                    if out.len() >= MAX_MARKS {
                        return Err(TagError::ContentLimit);
                    }
                    out.push((dict.get(b"Pg").and_then(Object::as_ref).or(page), mcid));
                }
            }
            Object::Ref(reference) if seen.insert(*reference) => stack.push((doc.get(*reference), depth + 1)),
            _ => {}
        }
    }
    Ok(())
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
        match (doc.resolve(item).as_int(), out.last_mut()) {
            (Some(_), Some((_, revision @ None))) => *revision = Some(item.clone()),
            _ => out.push((item.clone(), None)),
        }
    }
    (out, true)
}

/// Whether attribute object `o` (resolved) is a Table attribute object.
fn is_table_attributes(doc: &Document, o: &Object) -> bool {
    o.as_dict().and_then(|d| d.get(b"O")).map(|v| doc.resolve(v)).as_deref().and_then(Object::as_name) == Some(b"Table")
}

/// The value of `key` in the first Table attribute object of tag `d` that has it.
fn table_attribute(doc: &Document, d: &Dict, key: &[u8]) -> Option<Object> {
    attribute_entries(doc, d)
        .0
        .iter()
        .map(|(o, _)| doc.resolve(o))
        .filter(|o| is_table_attributes(doc, o))
        .find_map(|o| o.as_dict()?.get(key).map(|v| (*doc.resolve(v)).clone()).filter(|v| !matches!(v, Object::Null)))
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
            let table = Some(doc.resolve(&o)).filter(|a| is_table_attributes(doc, a)).and_then(|a| a.as_dict().cloned());
            (o, r, table)
        })
        .collect();
    let holding =
        |e: &(Object, Option<Object>, Option<Dict>)| e.2.as_ref().and_then(|d| d.get(key)).is_some_and(|v| !matches!(*doc.resolve(v), Object::Null));
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
        let ty = tag_type_name(doc, &element.dict).map(|s| alt::standard(doc, &role_map, &s)).unwrap_or_default();
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn pdf(bodies: &[String]) -> Document {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, b) in bodies.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{b}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", bodies.len() + 1).as_bytes());
        Document::open(Arc::new(out)).unwrap()
    }

    #[test]
    fn tag_content_and_table_attributes_resolve_indirect_entries() {
        let content = "/P << /MCID 0 >> BDC BT /F1 12 Tf (Hello) Tj ET EMC /P << /MCID 1 >> BDC BT /F1 12 Tf (World) Tj ET EMC";
        let doc = pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 5 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>".into(),
            format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
            "<< /Type /StructTreeRoot /K 6 0 R >>".into(),
            "<< /S /Document /P 5 0 R /K [7 0 R 8 0 R] >>".into(),
            "<< /S /Table /P 6 0 R /Pg 3 0 R /A << /O 11 0 R /Summary 12 0 R >> /K 9 0 R >>".into(),
            "<< /S /TH /P 6 0 R /Pg 3 0 R /A [ << /O 11 0 R /Scope 13 0 R >> 14 0 R ] /K 10 0 R >>".into(),
            "[15 0 R << /Type /MCR /MCID 16 0 R >>]".into(),
            "15 0 R".into(),
            "/Table".into(),
            "(Indirect summary)".into(),
            "/Column".into(),
            "1".into(),
            "0".into(),
            "1".into(),
        ]);
        let listed = tags(&doc);
        let table = listed.iter().find(|t| t.object == ObjRef::new(7, 0)).unwrap();
        let th = listed.iter().find(|t| t.object == ObjRef::new(8, 0)).unwrap();
        assert_eq!(table.summary.as_deref(), Some("Indirect summary"));
        assert_eq!(th.scope.as_deref(), Some("Column"));

        let table_marks = tag_content(&doc, ObjRef::new(7, 0)).unwrap();
        assert_eq!(
            table_marks,
            [TagContentItem { page: Some(0), mcid: 0, text: "Hello".into() }, TagContentItem { page: Some(0), mcid: 1, text: "World".into() },]
        );
        let th_marks = tag_content(&doc, ObjRef::new(8, 0)).unwrap();
        assert_eq!(th_marks, [TagContentItem { page: Some(0), mcid: 0, text: "Hello".into() }]);
    }

    #[test]
    fn move_tag_resolves_indirect_array_children_and_cycles() {
        let mut doc = pdf(&[
            "<< /Type /Catalog /StructTreeRoot 2 0 R >>".into(),
            "<< /Type /StructTreeRoot /K 3 0 R >>".into(),
            "<< /S /Document /P 2 0 R /K 6 0 R >>".into(),
            "<< /S /P /P 3 0 R >>".into(),
            "<< /S /P /P 3 0 R >>".into(),
            "[4 0 R 6 0 R 5 0 R]".into(),
        ]);
        let first = ObjRef::new(4, 7);
        let second = ObjRef::new(5, 9);
        doc.set(first, (*doc.get(ObjRef::new(4, 0))).clone());
        doc.set(second, (*doc.get(ObjRef::new(5, 0))).clone());
        doc.set(ObjRef::new(6, 0), Object::Array(vec![Object::Ref(first), Object::Ref(ObjRef::new(6, 0)), Object::Ref(second)]));
        assert_eq!(tags(&doc).iter().map(|t| t.object).collect::<Vec<_>>(), [ObjRef::new(3, 0), first, second]);
        move_tag(&mut doc, second, first, TagPlacement::Before).unwrap();
        assert_eq!(tags(&doc).iter().map(|t| t.object).collect::<Vec<_>>(), [ObjRef::new(3, 0), second, first]);
    }

    #[test]
    fn tag_content_rejects_excessive_indirect_depth() {
        let mut doc =
            pdf(&["<< /Type /Catalog /StructTreeRoot 2 0 R >>".into(), "<< /Type /StructTreeRoot /K 3 0 R >>".into(), "<< /S /P /P 2 0 R >>".into()]);
        let mut kid = doc.add(Object::Int(42));
        for _ in 0..128 {
            kid = doc.add(Object::Ref(kid));
        }
        doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"K".to_vec(), Object::Ref(kid))).unwrap();
        assert_eq!(tag_content(&doc, ObjRef::new(3, 0)).unwrap().iter().map(|m| m.mcid).collect::<Vec<_>>(), [42]);
        for _ in 0..1024 {
            kid = doc.add(Object::Ref(kid));
        }
        doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"K".to_vec(), Object::Ref(kid))).unwrap();
        assert_eq!(tag_content(&doc, ObjRef::new(3, 0)), Err(TagError::ContentLimit));
    }

    #[test]
    fn tag_content_rejects_excessive_mark_count() {
        let mut doc =
            pdf(&["<< /Type /Catalog /StructTreeRoot 2 0 R >>".into(), "<< /Type /StructTreeRoot /K 3 0 R >>".into(), "<< /S /P /P 2 0 R >>".into()]);
        doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"K".to_vec(), Object::Array(vec![Object::Int(1); 65_537]))).unwrap();
        assert_eq!(tag_content(&doc, ObjRef::new(3, 0)), Err(TagError::ContentLimit));
    }

    #[test]
    fn tag_content_accepts_full_mark_limit() {
        let mut doc =
            pdf(&["<< /Type /Catalog /StructTreeRoot 2 0 R >>".into(), "<< /Type /StructTreeRoot /K 3 0 R >>".into(), "<< /S /P /P 2 0 R >>".into()]);
        doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"K".to_vec(), Object::Array(vec![Object::Int(1); 65_536]))).unwrap();
        assert_eq!(tag_content(&doc, ObjRef::new(3, 0)).unwrap().len(), 65_536);
    }

    #[test]
    fn tag_content_skips_many_child_elements_without_spending_mark_budget() {
        let mut doc =
            pdf(&["<< /Type /Catalog /StructTreeRoot 2 0 R >>".into(), "<< /Type /StructTreeRoot /K 3 0 R >>".into(), "<< /S /P /P 2 0 R >>".into()]);
        let mut kids = Vec::new();
        for _ in 0..33_000 {
            let mut child = Dict::new();
            child.set(b"S".to_vec(), Object::name("P"));
            kids.push(Object::Ref(doc.add(Object::Dict(child))));
        }
        kids.push(Object::Int(42));
        doc.update_dict(ObjRef::new(3, 0), |d| d.set(b"K".to_vec(), Object::Array(kids))).unwrap();
        assert_eq!(tag_content(&doc, ObjRef::new(3, 0)).unwrap().iter().map(|m| m.mcid).collect::<Vec<_>>(), [42]);
    }

    #[test]
    fn tag_content_keeps_order_and_stops_indirect_cycles() {
        let doc = pdf(&[
            "<< /Type /Catalog /StructTreeRoot 2 0 R >>".into(),
            "<< /Type /StructTreeRoot /K 3 0 R >>".into(),
            "<< /S /P /P 2 0 R /K 4 0 R >>".into(),
            "[1 5 0 R 4 0 R 3]".into(),
            "<< /Type /MCR /MCID 2 >>".into(),
        ]);
        assert_eq!(tag_content(&doc, ObjRef::new(3, 0)).unwrap().iter().map(|m| m.mcid).collect::<Vec<_>>(), [1, 2, 3]);
    }

    #[test]
    fn move_tag_rejects_non_sibling_tags_sharing_stale_parent_ref() {
        // 8 0 R is a child of 6 0 R, while 9 0 R is a child of 7 0 R, even though both point /P at 6 0 R.
        let mut doc = pdf(&[
            "<< /Type /Catalog /StructTreeRoot 2 0 R >>".into(),
            "<< /Type /StructTreeRoot /K 3 0 R >>".into(),
            "<< /S /Document /P 2 0 R /K [6 0 R 7 0 R] >>".into(),
            "<< /S /P >>".into(),
            "<< /S /P >>".into(),
            "<< /S /Sect /P 3 0 R /K [8 0 R 9 0 R] >>".into(),
            "<< /S /Sect /P 3 0 R /K [9 0 R] >>".into(),
            "<< /S /P /P 6 0 R >>".into(),
            "<< /S /P /P 6 0 R >>".into(),
        ]);
        // Give 6 0 R only [8 0 R] in the reachable tree, and put [8 0 R 9 0 R] on an unreachable object 4 0 R.
        doc.update_dict(ObjRef::new(6, 0), |d| d.set(b"K".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(8, 0))]))).unwrap();
        doc.update_dict(ObjRef::new(4, 0), |d| {
            d.set(b"K".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(8, 0)), Object::Ref(ObjRef::new(9, 0))]));
        })
        .unwrap();
        doc.update_dict(ObjRef::new(8, 0), |d| d.set(b"P".to_vec(), Object::Ref(ObjRef::new(4, 0)))).unwrap();
        doc.update_dict(ObjRef::new(9, 0), |d| d.set(b"P".to_vec(), Object::Ref(ObjRef::new(4, 0)))).unwrap();
        assert_eq!(move_tag(&mut doc, ObjRef::new(9, 0), ObjRef::new(8, 0), TagPlacement::Before), Err(TagError::NotSiblings));
    }
}
