//! Named page templates (ISO 32000-2 7.7.4), retaining their hidden pages and field graphs.
use crate::{Dict, Document, ObjRef, Object, OrganizeError, PdfString, rebuild, walk};
use std::collections::{HashMap, HashSet};

const MAX_NAMES: usize = 4096;
const MAX_EDITED_NAMES: usize = 100_000;
#[derive(Clone, Debug, PartialEq)]
/// A named page, retained even when absent from the visible page tree.
pub struct Template {
    /// Decoded name used to look up the template.
    pub name: String,
    /// Whether the template is absent from the visible page tree.
    pub hidden: bool,
    /// Original page object, including for hidden templates.
    pub page: ObjRef,
    /// Zero-based visible page index, or `None` when hidden.
    pub visible_page: Option<usize>,
}

fn entries(doc: &Document, kind: &[u8], limit: usize) -> (Vec<(String, ObjRef)>, bool) {
    let Some(root) = doc.root() else { return (vec![], true) };
    let root = doc.get(root);
    let Some(names) = root.as_dict().and_then(|d| d.get(b"Names")) else { return (vec![], true) };
    let names = doc.resolve(names);
    let Some(names) = names.as_dict() else { return (vec![], false) };
    let Some(tree) = names.get(kind) else { return (vec![], true) };
    let mut stack = vec![(tree.clone(), 0usize)];
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut steps = 0usize;
    let mut scanned = 0;
    let mut complete = true;
    while let Some((node, depth)) = stack.pop() {
        steps += 1;
        if steps > 4096 || depth > 32 {
            return (out, false);
        }
        if node.as_ref().is_some_and(|r| !seen.insert(r)) {
            continue;
        }
        let node = doc.resolve(&node);
        let Some(d) = node.as_dict() else {
            complete = false;
            continue;
        };
        if let Some(pairs) = d.get(b"Names") {
            let pairs = doc.resolve(pairs);
            if let Some(pairs) = pairs.as_array() {
                complete &= pairs.len() % 2 == 0;
                for [key, value] in pairs.as_chunks::<2>().0 {
                    if scanned >= limit {
                        return (out, false);
                    }
                    scanned += 1;
                    if let Some(page) = value.as_ref() {
                        if let Some(name) = doc.resolve(key).as_string() {
                            out.push((name.to_text(), page));
                        } else {
                            complete = false;
                        }
                    }
                }
            } else {
                complete = false;
            }
        }
        if let Some(kids) = d.get(b"Kids") {
            let kids = doc.resolve(kids);
            if let Some(kids) = kids.as_array() {
                let remaining = 4096usize.saturating_sub(steps + stack.len());
                complete &= kids.len() <= remaining;
                for child in kids.iter().rev().take(remaining) {
                    if child.as_ref().is_some() {
                        stack.push((child.clone(), depth + 1));
                    } else {
                        complete = false;
                    }
                }
            } else {
                complete = false;
            }
        }
    }
    (out, complete)
}
/// List valid named templates with hidden templates first and each group ordered by name.
pub fn templates(doc: &Document) -> Vec<Template> {
    let pages: HashMap<_, _> = walk(doc).unwrap_or_default().into_iter().enumerate().map(|(i, (r, _))| (r, i)).collect();
    let mut out = std::collections::BTreeMap::new();
    for (name, page) in entries(doc, b"Templates", MAX_NAMES).0.into_iter().chain(entries(doc, b"Pages", MAX_NAMES).0) {
        if doc.get(page).as_dict().is_some_and(|d| matches!(d.name(b"Type"), Some(b"Page" | b"Template"))) {
            out.insert(name.clone(), Template { name, page, hidden: !pages.contains_key(&page), visible_page: pages.get(&page).copied() });
        }
    }
    let mut templates: Vec<_> = out.into_values().collect();
    templates.sort_by_key(|template| !template.hidden);
    templates
}
fn named(doc: &Document, name: &str) -> Result<Template, OrganizeError> {
    let page = [b"Pages".as_slice(), b"Templates"]
        .into_iter()
        .find_map(|kind| {
            entries(doc, kind, MAX_EDITED_NAMES).0.into_iter().rev().find_map(|(key, page)| {
                (key == name && doc.get(page).as_dict().is_some_and(|d| matches!(d.name(b"Type"), Some(b"Page" | b"Template")))).then_some(page)
            })
        })
        .ok_or_else(|| OrganizeError::Invalid(format!("there is no page template named {name:?}")))?;
    let visible_page = walk(doc)?.iter().position(|(r, _)| *r == page);
    Ok(Template { name: name.into(), page, hidden: visible_page.is_none(), visible_page })
}
fn set_entry(doc: &mut Document, kind: &[u8], name: &str, page: Option<ObjRef>) -> Result<(), OrganizeError> {
    set_entries(doc, kind, &HashSet::from([name]), page)
}

fn set_entries(doc: &mut Document, kind: &[u8], keys: &HashSet<&str>, page: Option<ObjRef>) -> Result<(), OrganizeError> {
    let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
    if doc.get(root).as_dict().and_then(|d| d.get(b"Names")).is_some_and(|v| doc.resolve(v).as_dict().is_none()) {
        return Err(OrganizeError::Invalid("the catalog Names entry is not a dictionary; its data was preserved".into()));
    }
    let names_ref = doc.get(root).as_dict().and_then(|d| d.reference(b"Names"));
    let mut names = doc.get(root).as_dict().and_then(|d| d.get(b"Names")).and_then(|v| doc.resolve(v).as_dict().cloned()).unwrap_or_default();
    let old = names.get(kind).cloned();
    if old.is_none() && page.is_none() {
        return Ok(());
    }
    if old.as_ref().is_some_and(|v| doc.resolve(v).as_dict().is_none()) {
        return Err(OrganizeError::Invalid("the template name tree is not a dictionary; its data was preserved".into()));
    }
    let mut tree = old.as_ref().and_then(|v| doc.resolve(v).as_dict().cloned()).unwrap_or_default();
    // Flattening a branched name tree would discard metadata on its children. Keep the
    // graph intact until this writer can edit those nodes in place.
    if tree.contains(b"Kids") {
        return Err(OrganizeError::Invalid("editing a branched template name tree is not supported; its data was preserved".into()));
    }
    if tree.get(b"Names").is_some_and(|v| doc.resolve(v).as_array().is_none()) {
        return Err(OrganizeError::Invalid("the template name list is not an array; its data was preserved".into()));
    }
    let array = tree.get(b"Names").map(|v| doc.resolve(v)).and_then(|v| v.as_array().cloned()).unwrap_or_default();
    if array.len() > MAX_EDITED_NAMES * 2 || array.len() % 2 != 0 {
        return Err(OrganizeError::Invalid("invalid or oversized template name tree".into()));
    }
    let mut values = Vec::new();
    for pair in array.as_chunks::<2>().0 {
        let [key, value] = pair;
        let key_text = doc
            .resolve(key)
            .as_string()
            .map(|s| s.to_text())
            .ok_or_else(|| OrganizeError::Invalid("template name tree has a non-string key".into()))?;
        if !keys.contains(key_text.as_str()) {
            let bytes = doc.resolve(key).as_string().map(|s| s.bytes.clone()).unwrap_or_default();
            values.push((bytes, key.clone(), value.clone()));
        }
    }
    if let Some(page) = page {
        for name in keys {
            let key = PdfString::text(name);
            values.push((key.bytes.clone(), Object::String(key), Object::Ref(page)));
        }
    }
    if values.len() > MAX_EDITED_NAMES {
        return Err(OrganizeError::Invalid("too many page templates".into()));
    }
    values.sort_by(|a, b| a.0.cmp(&b.0));
    tree.remove(b"Limits");
    tree.set(b"Names".to_vec(), Object::Array(values.into_iter().flat_map(|(_, key, value)| [key, value]).collect()));
    if let Some(r) = old.as_ref().and_then(Object::as_ref) {
        doc.set(r, Object::Dict(tree));
    } else {
        names.set(kind.to_vec(), Object::Dict(tree));
    }
    if let Some(r) = names_ref {
        doc.set(r, Object::Dict(names));
    } else {
        doc.update_dict(root, |d| d.set(b"Names".to_vec(), Object::Dict(names)))?;
    }
    Ok(())
}
/// Name a zero-based visible page, replacing any existing template with this name.
/// Refuses empty/oversized names, missing pages, and malformed or branched name trees.
/// Like the other template writers, use a working clone to roll back on error.
pub fn create_template(doc: &mut Document, name: &str, page: usize) -> Result<(), OrganizeError> {
    if name.is_empty() || name.len() > 4096 {
        return Err(OrganizeError::Invalid("a template name needs 1 through 4096 bytes".into()));
    }
    let page = walk(doc)?.get(page).map(|(r, _)| *r).ok_or(OrganizeError::NoSuchPage(page))?;
    set_entry(doc, b"Templates", name, None)?;
    set_entry(doc, b"Pages", name, Some(page))
}
/// Remove a name without deleting its page. Refuses missing names or unsupported name trees.
/// Use a working clone for rollback if either name-tree update fails.
pub fn remove_template(doc: &mut Document, name: &str) -> Result<(), OrganizeError> {
    named(doc, name)?;
    set_entry(doc, b"Templates", name, None)?;
    set_entry(doc, b"Pages", name, None)
}
/// Effective inherited page entries, bounded independently of the page tree walk.
pub(crate) fn inherited(doc: &Document, page: ObjRef) -> Dict {
    let mut out = Dict::new();
    let mut seen = HashSet::new();
    let mut next = Some(page);
    while let Some(r) = next
        && seen.len() < 256
        && seen.insert(r)
    {
        let value = doc.get(r);
        let Some(d) = value.as_dict() else { break };
        for key in crate::INHERITABLE {
            if !out.contains(key)
                && let Some(value) = d.get(key)
            {
                out.set(key.to_vec(), value.clone());
            }
        }
        next = d.reference(b"Parent");
    }
    out
}
/// A standalone template may omit attributes normally inherited from the page tree.
fn template_attributes(doc: &Document, page: ObjRef, order: &[(ObjRef, Dict)]) -> Dict {
    let mut attrs = inherited(doc, page);
    let root = crate::pages_root(doc).ok().map(|r| doc.get(r));
    for key in crate::INHERITABLE {
        if !attrs.contains(key)
            && let Some(value) =
                root.as_deref().and_then(Object::as_dict).and_then(|d| d.get(key)).or_else(|| order.first().and_then(|(_, attrs)| attrs.get(key)))
        {
            attrs.set(key.to_vec(), value.clone());
        }
    }
    attrs
}

/// Effective inherited page dictionary and boxes (media, crop, bleed, trim, art) of a template.
pub fn template_geometry(doc: &Document, page: ObjRef) -> (Dict, [[f64; 4]; 5]) {
    let attrs = template_attributes(doc, page, &[]);
    let original = doc.get(page);
    let leaf = original.as_dict().cloned().unwrap_or_default();
    let boxes = crate::boxes::effective_boxes(doc, &leaf, &attrs);
    (attrs, boxes)
}

pub(crate) fn preserve_hidden_attributes(doc: &mut Document, order: &[(ObjRef, Dict)]) -> Result<(), OrganizeError> {
    let visible: HashSet<_> = order.iter().map(|(page, _)| *page).collect();
    let hidden: HashSet<_> = [b"Templates".as_slice(), b"Pages"]
        .into_iter()
        .flat_map(|kind| entries(doc, kind, MAX_EDITED_NAMES).0)
        .map(|(_, page)| page)
        .filter(|page| !visible.contains(page) && doc.get(*page).as_dict().is_some_and(|d| matches!(d.name(b"Type"), Some(b"Page" | b"Template"))))
        .collect();
    for page in hidden {
        let attrs = template_attributes(doc, page, order);
        doc.update_dict(page, |d| {
            for (key, value) in attrs.iter() {
                if !d.contains(key) {
                    d.set(key.clone(), value.clone());
                }
            }
        })?;
    }
    Ok(())
}

fn complete_entries(doc: &Document, kind: &[u8]) -> Result<Vec<(String, ObjRef)>, OrganizeError> {
    let (entries, complete) = entries(doc, kind, MAX_EDITED_NAMES);
    if !complete {
        return Err(OrganizeError::Invalid("could not inspect the complete template name tree".into()));
    }
    Ok(entries)
}

/// Hide the template's visible page, or append its hidden page to the visible pages.
/// Already matching visibility is a no-op. Refuses a missing name, hiding the last
/// page, invalid page trees, or unsupported name trees.
///
/// Callers requiring rollback must clone and commit only on success:
/// ```
/// # fn update(doc: &mut pdfcraft_cos::Document) -> Result<(), pdfcraft_organize::OrganizeError> {
/// let mut working = doc.clone();
/// pdfcraft_organize::set_template_hidden(&mut working, "Notes", true)?;
/// *doc = working;
/// # Ok(()) }
/// ```
pub fn set_template_hidden(doc: &mut Document, name: &str, hidden: bool) -> Result<(), OrganizeError> {
    let template = named(doc, name)?;
    let mut pages = walk(doc)?;
    if hidden == template.visible_page.is_none() {
        return Ok(());
    }
    if hidden && pages.len() <= 1 {
        return Err(OrganizeError::WouldRemoveAllPages);
    }
    let visible_names = complete_entries(doc, b"Pages")?;
    let hidden_names = complete_entries(doc, b"Templates")?;
    let entries: Vec<_> = visible_names.iter().chain(&hidden_names).collect();
    let keys: HashSet<_> = entries.iter().filter(|(_, page)| *page == template.page).map(|(name, _)| name.as_str()).collect();
    if entries.iter().any(|(name, page)| *page != template.page && keys.contains(name.as_str())) {
        return Err(OrganizeError::Invalid("a template alias also names a different page".into()));
    }
    let (source, destination) = if hidden { (b"Pages".as_slice(), b"Templates".as_slice()) } else { (b"Templates".as_slice(), b"Pages".as_slice()) };
    set_entries(doc, source, &keys, None)?;
    set_entries(doc, destination, &keys, Some(template.page))?;
    if let Some(index) = template.visible_page {
        preserve_hidden_attributes(doc, &pages)?;
        let (_, attrs) = pages.remove(index);
        doc.update_dict(template.page, |d| {
            d.set(b"Type".to_vec(), Object::name("Template"));
            d.remove(b"Parent");
            for (key, value) in attrs.iter() {
                if !d.contains(key) {
                    d.set(key.clone(), value.clone());
                }
            }
        })?;
        rebuild(doc, &pages)?;
        crate::labels::drop_deleted_pages(doc, &[index])?;
    } else {
        preserve_hidden_attributes(doc, &pages)?;
        let attrs = template_attributes(doc, template.page, &pages);
        pages.push((template.page, attrs));
        doc.update_dict(template.page, |d| d.set(b"Type".to_vec(), Object::name("Page")))?;
        rebuild(doc, &pages)?;
    }
    Ok(())
}
/// Insert a template copy before zero-based `at`, clamped to the page count (append).
/// Return its visible index. With renaming, copied fields get a new page namespace.
/// Without renaming, widgets share the original fields. Use a working clone for rollback.
pub fn spawn_template(doc: &mut Document, name: &str, at: usize, rename: bool) -> Result<usize, OrganizeError> {
    let at = at.min(crate::page_count(doc)?);
    spawn_template_page(doc, name, at, rename)?;
    Ok(at)
}

fn spawn_template_page(doc: &mut Document, name: &str, at: usize, rename: bool) -> Result<ObjRef, OrganizeError> {
    let template = named(doc, name)?;
    let attrs = template_attributes(doc, template.page, &walk(doc)?);
    crate::import::copy_template_page(doc, template.page, attrs, at, rename.then_some(name))
}
/// Deleting a visible named template hides it; ordinary spawned pages are removed normally.
/// Indices are zero-based; duplicates count once. Refuses missing indices or deleting
/// all visible pages. Use a working clone for rollback on page/name-tree errors.
pub fn delete_script_pages(doc: &mut Document, pages: &[usize]) -> Result<(), OrganizeError> {
    let all = walk(doc)?;
    crate::check(pages, all.len())?;
    let unique: HashSet<_> = pages.iter().copied().collect();
    if unique.len() >= all.len() {
        return Err(OrganizeError::WouldRemoveAllPages);
    }
    if unique.is_empty() {
        return Ok(());
    }
    let mut named_pages = HashMap::new();
    for (name, page) in complete_entries(doc, b"Templates")?.into_iter().chain(complete_entries(doc, b"Pages")?) {
        named_pages.insert(page, name);
    }
    let (mut named, mut ordinary) = (Vec::new(), Vec::new());
    for (index, (page, _)) in all.iter().enumerate().rev().filter(|(i, _)| unique.contains(i)) {
        if let Some(name) = named_pages.get(page) {
            named.push(name);
        } else {
            ordinary.push(index);
        }
    }
    if !ordinary.is_empty() {
        preserve_hidden_attributes(doc, &all)?;
        crate::delete_pages(doc, &ordinary)?;
    }
    for name in named {
        set_template_hidden(doc, name, true)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn fixture() -> Document {
        let bodies = [
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /CO [5 0 R] >> /Names << /Templates << /Names [(Notes) 4 0 R] >> >> >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] /Resources << /Font << /Helv 6 0 R >> >> >>",
            "<< /Type /Page /Parent 2 0 R >>",
            "<< /Type /Template /Parent 2 0 R /Annots 8 0 R /Sentinel (retained) /Contents 7 0 R >>",
            "<< /Type /Annot /Subtype /Widget /FT /Tx /T (note) /V (original) /Rect [10 10 100 30] /P 4 0 R /DA (/Helv 12 Tf 0 g) /Custom (keep) >>",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            "<< /Length 0 >>\nstream\n\nendstream",
            "[5 0 R]",
        ];
        let mut bytes = b"%PDF-1.7\n".to_vec();
        let mut offsets = vec![0];
        for (i, body) in bodies.iter().enumerate() {
            offsets.push(bytes.len());
            bytes.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
        for offset in offsets.iter().skip(1) {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(format!("trailer\n<< /Root 1 0 R /Size {} >>\nstartxref\n{xref}\n%%EOF\n", offsets.len()).as_bytes());
        Document::open(Arc::new(bytes)).unwrap()
    }
    #[test]
    fn renamed_template_copies_fields_and_shares_resources() {
        let mut doc = fixture();
        assert_eq!(templates(&doc)[0].visible_page, None);
        let page = spawn_template_page(&mut doc, "Notes", 1, true).unwrap();
        assert_eq!(crate::page_count(&doc).unwrap(), 2);
        let widget =
            doc.get(page).as_dict().and_then(|d| d.get(b"Annots")).and_then(|v| doc.resolve(v).as_array().cloned()).unwrap()[0].as_ref().unwrap();
        assert_eq!(doc.get(widget).as_dict().and_then(|d| d.get(b"T")).and_then(|v| v.as_string().map(|s| s.to_text())).as_deref(), Some("note"));
        assert_eq!(doc.get(page).as_dict().and_then(|d| d.reference(b"Contents")), Some(ObjRef::new(7, 0)));
        assert!(doc.get(page).as_dict().unwrap().contains(b"Sentinel"));
        let saved = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
        let reopened = Document::open(Arc::new(saved)).unwrap();
        assert_eq!(crate::page_count(&reopened).unwrap(), 2);
        assert_eq!(templates(&reopened)[0].name, "Notes");
    }
    #[test]
    fn later_spawns_keep_hidden_templates_inherited_page_attributes() {
        let mut doc = fixture();
        let first = spawn_template_page(&mut doc, "Notes", 1, true).unwrap();
        let second = spawn_template_page(&mut doc, "Notes", 2, true).unwrap();
        for page in [ObjRef::new(4, 0), first, second] {
            let page = doc.get(page);
            let page = page.as_dict().unwrap();
            assert!(page.contains(b"MediaBox"));
            assert!(page.contains(b"Resources"));
        }
    }
    #[test]
    fn renamed_template_copies_indirect_action_containers() {
        let mut doc = fixture();
        let mut action = Dict::new();
        action.set(b"S".to_vec(), Object::name("ResetForm"));
        action.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(5, 0))]));
        let action = doc.add(Object::Dict(action));
        let mut aa = Dict::new();
        aa.set(b"U".to_vec(), Object::Ref(action));
        let aa = doc.add(Object::Dict(aa));
        doc.update_dict(ObjRef::new(5, 0), |d| d.set(b"AA".to_vec(), Object::Ref(aa))).unwrap();
        let page = spawn_template_page(&mut doc, "Notes", 1, true).unwrap();
        let widget = doc.resolve(doc.get(page).as_dict().unwrap().get(b"Annots").unwrap()).as_array().unwrap()[0].as_ref().unwrap();
        let copied_aa = doc.get(widget).as_dict().unwrap().reference(b"AA").unwrap();
        assert_ne!(copied_aa, aa);
        let copied_action = doc.get(copied_aa).as_dict().unwrap().reference(b"U").unwrap();
        assert_ne!(copied_action, action);
        let fields = doc.get(copied_action).as_dict().unwrap().get(b"Fields").unwrap().as_array().unwrap().clone();
        assert_eq!(fields, vec![Object::Ref(widget)]);
        assert_eq!(doc.get(action).as_dict().unwrap().get(b"Fields").unwrap().as_array().unwrap(), &vec![Object::Ref(ObjRef::new(5, 0))]);
    }
    #[test]
    fn template_name_tree_sorts_encoded_bytes_including_indirect_keys() {
        let mut doc = fixture();
        let encoded = PdfString::literal(vec![0xfe, 0xff, 0, b'A']);
        let key = doc.add(Object::String(encoded.clone()));
        let mut tree = Dict::new();
        tree.set(b"Names".to_vec(), Object::Array(vec![Object::Ref(key), Object::Int(17)]));
        let root = doc.root().unwrap();
        doc.update_dict(root, |d| {
            let mut names = Dict::new();
            names.set(b"Pages".to_vec(), Object::Dict(tree));
            d.set(b"Names".to_vec(), Object::Dict(names));
        })
        .unwrap();
        create_template(&mut doc, "Z", 0).unwrap();
        let root = doc.get(root);
        let pairs = root
            .as_dict()
            .unwrap()
            .get(b"Names")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Pages")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Names")
            .unwrap()
            .as_array()
            .unwrap();
        let bytes: Vec<_> = pairs.as_chunks::<2>().0.iter().map(|p| doc.resolve(&p[0]).as_string().unwrap().bytes.clone()).collect();
        assert!(bytes.windows(2).all(|p| p[0] < p[1]));
        assert!(pairs.contains(&Object::Ref(key)));
    }
    #[test]
    fn editing_template_names_keeps_unrecognized_values() {
        let mut doc = fixture();
        let root = doc.root().unwrap();
        doc.update_dict(root, |d| {
            let mut names = d.get(b"Names").unwrap().as_dict().unwrap().clone();
            let mut tree = names.get(b"Templates").unwrap().as_dict().unwrap().clone();
            let mut pairs = tree.get(b"Names").unwrap().as_array().unwrap().clone();
            pairs.extend([Object::String(PdfString::text("opaque")), Object::Int(17)]);
            tree.set(b"Names".to_vec(), Object::Array(pairs));
            names.set(b"Templates".to_vec(), Object::Dict(tree));
            d.set(b"Names".to_vec(), Object::Dict(names));
        })
        .unwrap();
        remove_template(&mut doc, "Notes").unwrap();
        let root = doc.get(root);
        let pairs = root
            .as_dict()
            .unwrap()
            .get(b"Names")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Templates")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Names")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(pairs, &vec![Object::String(PdfString::text("opaque")), Object::Int(17)]);
    }
    #[test]
    fn unrenamed_template_keeps_the_field_identity_and_adds_widget_siblings() {
        let mut doc = fixture();
        spawn_template_page(&mut doc, "Notes", 1, false).unwrap();
        spawn_template_page(&mut doc, "Notes", 2, false).unwrap();
        let field = doc.get(ObjRef::new(5, 0));
        let field = field.as_dict().unwrap();
        assert!(!field.contains(b"Subtype"));
        assert!(field.contains(b"Custom"));
        let kids = field.get(b"Kids").and_then(Object::as_array).unwrap();
        assert_eq!(kids.len(), 3);
        for widget in kids {
            assert_eq!(doc.resolve(widget).as_dict().and_then(|d| d.reference(b"Parent")), Some(ObjRef::new(5, 0)));
        }
    }
    #[test]
    fn hiding_a_page_preserves_other_templates_and_moves_all_aliases() {
        let mut doc = fixture();
        let root = crate::pages_root(&doc).unwrap();
        let mut leaf = Dict::new();
        leaf.set(b"Type".to_vec(), Object::name("Page"));
        leaf.set(b"Parent".to_vec(), Object::Ref(root));
        let middle = doc.add(Object::Dict(leaf.clone()));
        let last = doc.add(Object::Dict(leaf));
        doc.update_dict(root, |d| {
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(3, 0)), Object::Ref(middle), Object::Ref(last)]));
            d.set(b"Count".to_vec(), Object::Int(3));
        })
        .unwrap();
        crate::number_pages(&mut doc, 2, 2, crate::LabelStyle::Decimal, "Last-", 1).unwrap();
        create_template(&mut doc, "Main", 0).unwrap();
        create_template(&mut doc, "Alias", 0).unwrap();

        set_template_hidden(&mut doc, "Main", true).unwrap();
        assert!(entries(&doc, b"Pages", MAX_NAMES).0.is_empty());
        assert_eq!(named(&doc, "Alias").unwrap().visible_page, None);
        assert_eq!(crate::page_labels(&doc).unwrap()[1], "Last-1");
        let retained = named(&doc, "Notes").unwrap();
        assert!(doc.get(retained.page).as_dict().unwrap().contains(b"Resources"));
        let copy = spawn_template_page(&mut doc, "Notes", 2, true).unwrap();
        assert!(doc.get(copy).as_dict().unwrap().contains(b"Resources"));

        set_template_hidden(&mut doc, "Alias", false).unwrap();
        assert_eq!(named(&doc, "Main").unwrap().visible_page, named(&doc, "Alias").unwrap().visible_page);
        assert!(entries(&doc, b"Templates", MAX_NAMES).0.iter().all(|(name, _)| name == "Notes"));
    }

    #[test]
    fn hiding_named_pages_keeps_templates_and_refuses_removing_the_last_page() {
        let mut doc = fixture();
        create_template(&mut doc, "Main", 0).unwrap();
        assert!(set_template_hidden(&mut doc, "Main", true).is_err());
        spawn_template_page(&mut doc, "Notes", 1, true).unwrap();
        set_template_hidden(&mut doc, "Main", true).unwrap();
        assert_eq!(crate::page_count(&doc).unwrap(), 1);
        set_template_hidden(&mut doc, "Main", false).unwrap();
        assert_eq!(crate::page_count(&doc).unwrap(), 2);
        assert!(delete_script_pages(&mut doc, &[0, 1]).is_err());
    }
}

#[cfg(test)]
#[path = "templates/tests.rs"]
mod upstream_tests;
