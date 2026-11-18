//! Named page templates (ISO 32000-2 7.7.4), retaining their hidden pages and field graphs.
use crate::{Dict, Document, ObjRef, Object, OrganizeError, PdfString, rebuild, walk};
use std::collections::{HashMap, HashSet};
#[derive(Clone, Debug, PartialEq)]
/// A named page, retained even when absent from the visible page tree.
pub struct Template {
    /// Decoded name used to look up the template.
    pub name: String,
    /// Original page object, including for hidden templates.
    pub page: ObjRef,
    /// Zero-based visible page index, or `None` when hidden.
    pub visible_page: Option<usize>,
}

fn entries(doc: &Document, kind: &[u8]) -> Vec<(String, ObjRef)> {
    let Some(tree) = doc
        .root()
        .and_then(|r| doc.get(r).as_dict().cloned())
        .and_then(|d| d.get(b"Names").map(|v| doc.resolve(v)))
        .and_then(|v| v.as_dict().and_then(|d| d.get(kind)).cloned())
    else {
        return vec![];
    };
    let mut stack = vec![(tree, 0usize)];
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut steps = 0usize;
    while let Some((node, depth)) = stack.pop() {
        steps += 1;
        if steps > 4096 || depth > 32 {
            break;
        }
        if node.as_ref().is_some_and(|r| !seen.insert(r)) {
            continue;
        }
        let node = doc.resolve(&node);
        let Some(d) = node.as_dict() else { continue };
        if let Some(pairs) = d.get(b"Names").map(|v| doc.resolve(v)) {
            for pair in pairs.as_array().into_iter().flat_map(|a| a.chunks(2)).take(4096) {
                if out.len() >= 4096 {
                    break;
                }
                if let [key, value] = pair
                    && let Some(page) = value.as_ref()
                    && let Some(name) = doc.resolve(key).as_string()
                {
                    out.push((name.to_text(), page));
                }
            }
        }
        if let Some(kids) = d.get(b"Kids").map(|v| doc.resolve(v)) {
            for child in kids.as_array().into_iter().flatten().rev().take(4096usize.saturating_sub(steps)) {
                if let Some(r) = child.as_ref() {
                    stack.push((Object::Ref(r), depth + 1));
                }
            }
        }
    }
    out
}
/// List valid named templates in decoded-name order; malformed entries are skipped.
pub fn templates(doc: &Document) -> Vec<Template> {
    let pages: HashMap<_, _> = walk(doc).unwrap_or_default().into_iter().enumerate().map(|(i, (r, _))| (r, i)).collect();
    let mut out = std::collections::BTreeMap::new();
    for (name, page) in entries(doc, b"Templates").into_iter().chain(entries(doc, b"Pages")) {
        if doc.get(page).as_dict().is_some_and(|d| matches!(d.name(b"Type"), Some(b"Page" | b"Template"))) {
            out.insert(name.clone(), Template { name, page, visible_page: pages.get(&page).copied() });
        }
    }
    out.into_values().collect()
}
fn named(doc: &Document, name: &str) -> Result<Template, OrganizeError> {
    templates(doc).into_iter().find(|t| t.name == name).ok_or_else(|| OrganizeError::Invalid(format!("there is no page template named {name:?}")))
}
fn set_entry(doc: &mut Document, kind: &[u8], name: &str, page: Option<ObjRef>) -> Result<(), OrganizeError> {
    let root = doc.root().ok_or(OrganizeError::NoPageTree)?;
    if doc.get(root).as_dict().and_then(|d| d.get(b"Names")).is_some_and(|v| doc.resolve(v).as_dict().is_none()) {
        return Err(OrganizeError::Invalid("the catalog Names entry is not a dictionary; its data was preserved".into()));
    }
    let mut names = doc.get(root).as_dict().and_then(|d| d.get(b"Names")).and_then(|v| doc.resolve(v).as_dict().cloned()).unwrap_or_default();
    let old = names.get(kind).cloned();
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
    if array.len() > 8192 || array.len() % 2 != 0 {
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
        if key_text != name {
            let bytes = doc.resolve(key).as_string().map(|s| s.bytes.clone()).unwrap_or_default();
            values.push((bytes, key.clone(), value.clone()));
        }
    }
    if let Some(page) = page {
        let key = PdfString::text(name);
        values.push((key.bytes.clone(), Object::String(key), Object::Ref(page)));
    }
    if values.len() > 4096 {
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
    doc.update_dict(root, |d| d.set(b"Names".to_vec(), Object::Dict(names)))?;
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
/// Hide the template's visible page, or append its hidden page to the visible pages.
/// Already matching visibility is a no-op. Refuses a missing name, hiding the last
/// page, invalid page trees, or unsupported name trees.
///
/// This may change the page tree before a branched name tree is refused. Callers
/// requiring rollback must clone and commit only on success:
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
    if hidden {
        if template.visible_page.is_none() {
            return Ok(());
        }
        if pages.len() <= 1 {
            return Err(OrganizeError::WouldRemoveAllPages);
        }
        let attrs = inherited(doc, template.page);
        doc.update_dict(template.page, |d| {
            d.set(b"Type".to_vec(), Object::name("Template"));
            for (k, v) in attrs.iter() {
                if !d.contains(k) {
                    d.set(k.clone(), v.clone());
                }
            }
        })?;
        pages.retain(|(r, _)| *r != template.page);
        rebuild(doc, &pages)?;
        set_entry(doc, b"Pages", name, None)?;
        set_entry(doc, b"Templates", name, Some(template.page))?;
    } else {
        if template.visible_page.is_some() {
            return Ok(());
        }
        pages.push((template.page, inherited(doc, template.page)));
        doc.update_dict(template.page, |d| d.set(b"Type".to_vec(), Object::name("Page")))?;
        rebuild(doc, &pages)?;
        set_entry(doc, b"Templates", name, None)?;
        set_entry(doc, b"Pages", name, Some(template.page))?;
    }
    Ok(())
}
/// Insert a template copy before zero-based `at`, clamped to the page count (append).
/// `rename` prefixes copied fields with `P{at}.{name}.`; a taken prefix is refused.
/// Without renaming, widgets share the original fields. Overlay is unsupported.
/// Missing names, invalid graphs and copy limits return errors; use a working clone
/// to discard partial changes on failure.
pub fn spawn_template(doc: &mut Document, name: &str, at: usize, rename: bool, overlay: bool) -> Result<ObjRef, OrganizeError> {
    if overlay {
        return Err(OrganizeError::Invalid("overlay template spawning is not supported; use bOverlay=false".into()));
    }
    let template = named(doc, name)?;
    let at = at.min(crate::page_count(doc)?);
    crate::import::copy_template_page(doc, template.page, inherited(doc, template.page), at, rename.then(|| format!("P{at}.{name}.")))
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
    let targets: Vec<_> = all.iter().enumerate().filter(|(i, _)| unique.contains(i)).map(|(_, (r, _))| *r).collect();
    for page in targets.into_iter().rev() {
        if let Some(t) = templates(doc).into_iter().find(|t| t.page == page) {
            set_template_hidden(doc, &t.name, true)?;
        } else if let Some(i) = walk(doc)?.iter().position(|(r, _)| *r == page) {
            crate::delete_pages(doc, &[i])?;
        }
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
        let page = spawn_template(&mut doc, "Notes", 1, true, false).unwrap();
        assert_eq!(crate::page_count(&doc).unwrap(), 2);
        let widget =
            doc.get(page).as_dict().and_then(|d| d.get(b"Annots")).and_then(|v| doc.resolve(v).as_array().cloned()).unwrap()[0].as_ref().unwrap();
        assert_eq!(
            doc.get(widget).as_dict().and_then(|d| d.get(b"T")).and_then(|v| v.as_string().map(|s| s.to_text())).as_deref(),
            Some("P1.Notes.note")
        );
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
        let first = spawn_template(&mut doc, "Notes", 1, true, false).unwrap();
        let second = spawn_template(&mut doc, "Notes", 2, true, false).unwrap();
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
        let page = spawn_template(&mut doc, "Notes", 1, true, false).unwrap();
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
        spawn_template(&mut doc, "Notes", 1, false, false).unwrap();
        spawn_template(&mut doc, "Notes", 2, false, false).unwrap();
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
    fn hiding_named_pages_keeps_templates_and_refuses_removing_the_last_page() {
        let mut doc = fixture();
        create_template(&mut doc, "Main", 0).unwrap();
        assert!(set_template_hidden(&mut doc, "Main", true).is_err());
        spawn_template(&mut doc, "Notes", 1, true, false).unwrap();
        set_template_hidden(&mut doc, "Main", true).unwrap();
        assert_eq!(crate::page_count(&doc).unwrap(), 1);
        set_template_hidden(&mut doc, "Main", false).unwrap();
        assert_eq!(crate::page_count(&doc).unwrap(), 2);
        assert!(delete_script_pages(&mut doc, &[0, 1]).is_err());
    }
}
