//! What every tagging operation must leave true, as a list of problems (ISO 32000-2 §14.7.4,
//! §14.7.5.4): each tag's `/P` names what lists it, each content item an indirect tag owns has a
//! parent-tree entry pointing back at it and is drawn on its page, no parent-tree entry points
//! at a tag that doesn't own the content, MCIDs are unique on a page, `/ParentTreeNextKey` is
//! past every key, and every type is standard or role-mapped.

use std::collections::{HashMap, HashSet};

use pdfcraft_cos::{Document, ObjRef, Object};

use super::content::PageContent;
use super::parent_tree::Value;
use super::tree::{Container, Owner, Tree, root_of};

/// The structure's consistency problems; empty when it is consistent (or untagged).
pub fn structure_problems(doc: &Document) -> Vec<String> {
    let mut out = Vec::new();
    let Some((_, root_dict)) = root_of(doc) else { return out };
    // Read through a copy whose root and parent tree are indirect, as the edits see them.
    let mut copy = doc.clone();
    let root = match super::ensure_structure(&mut copy, false) {
        Ok(r) => r,
        Err(e) => {
            out.push(format!("the tag tree can't be read: {e}"));
            return out;
        }
    };
    let tree = Tree::read(&copy);
    if tree.truncated {
        out.push("the tag tree is too large to check".into());
        return out;
    }
    let pages = pdfcraft_model::pages(&copy);
    let number: HashMap<ObjRef, usize> = pages.iter().enumerate().map(|(i, p)| (p.obj, i.saturating_add(1))).collect();
    let page_no = |p: ObjRef| number.get(&p).map_or_else(|| format!("{p}"), |n| n.to_string());
    let role_map = root_dict.get(b"RoleMap").map(|m| doc.resolve(m)).and_then(|m| m.as_dict().cloned()).unwrap_or_default();
    for node in &tree.nodes {
        if node.shared {
            out.push(format!("tag {} is listed more than once", node.object));
        }
        let expected = match node.container {
            Container::Root => Some(root),
            Container::Element(p) => Some(p),
            Container::Inline => None,
        };
        let actual = node.dict.get(b"P").and_then(Object::as_ref);
        if expected.is_some() && actual != expected {
            out.push(format!("tag {}: /P is {actual:?}, but {expected:?} lists it", node.object));
        }
        match node.dict.name(b"S") {
            Some(s) if crate::structure::is_standard(&crate::alt::standard(doc, &role_map, s)) => {}
            Some(s) => out.push(format!("tag {}: type {} is neither standard nor role-mapped", node.object, String::from_utf8_lossy(s))),
            None => out.push(format!("tag {} has no type", node.object)),
        }
    }
    let Ok(pt) = super::parent_tree(&copy, root) else {
        out.push("the parent tree is missing".into());
        return out;
    };
    for key in pt.keys().filter(|k| pt.is_duplicate(*k)) {
        out.push(format!("parent tree key {key} is listed twice"));
    }
    let greatest = pt.keys().max();
    if let (Some(next), Some(greatest)) = (copy.get(root).as_dict().and_then(|d| d.int(b"ParentTreeNextKey")), greatest)
        && next <= greatest
    {
        out.push(format!("/ParentTreeNextKey {next} is not past key {greatest}"));
    }
    let key_of = |holder: ObjRef, entry: &[u8]| copy.get(holder).as_dict().and_then(|d| d.get(entry).and_then(Object::as_int));
    // Every owned content item points back at its tag.
    let mut pages_with_content: HashSet<ObjRef> = HashSet::new();
    for ((page, mcid), owners) in &tree.mcid_owners {
        pages_with_content.insert(*page);
        if owners.len() > 1 {
            out.push(format!("marked content {mcid} on page {} is owned by {} tags", page_no(*page), owners.len()));
            continue;
        }
        let Some(Owner::Element(tag)) = owners.first() else { continue };
        match key_of(*page, b"StructParents") {
            None => out.push(format!("page {} has tagged content but no /StructParents", page_no(*page))),
            Some(key) => {
                let at = pt.element_at(&copy, key, *mcid);
                if at != Some(*tag) {
                    out.push(format!("the parent tree maps page {} MCID {mcid} to {at:?}, not its tag {tag}", page_no(*page)));
                }
            }
        }
    }
    for (object, owners) in &tree.object_owners {
        if owners.len() > 1 {
            out.push(format!("object {object} is owned by {} tags", owners.len()));
            continue;
        }
        let Some(Owner::Element(tag)) = owners.first() else { continue };
        match key_of(*object, b"StructParent") {
            None => out.push(format!("object {object} has a tag but no /StructParent")),
            Some(key) => match pt.get(&copy, key) {
                Some(Value::Single(Object::Ref(r))) if r == *tag => {}
                other => out.push(format!("the parent tree maps object {object} (key {key}) to {other:?}, not its tag {tag}")),
            },
        }
    }
    // No parent-tree entry points at a tag that doesn't own the content.
    for page in &pages {
        let Some(key) = key_of(page.obj, b"StructParents") else { continue };
        match pt.get(&copy, key) {
            Some(Value::Array(entries)) => {
                for (m, entry) in entries.iter().enumerate() {
                    let Some(tag) = entry.as_ref() else { continue };
                    let owned = i64::try_from(m)
                        .ok()
                        .and_then(|m| tree.mcid_owners.get(&(page.obj, m)))
                        .is_some_and(|owners| owners.contains(&Owner::Element(tag)));
                    if !owned {
                        out.push(format!("the parent tree maps page {} MCID {m} to {tag}, which doesn't own it", page_no(page.obj)));
                    }
                }
            }
            Some(Value::Single(_)) => out.push(format!("page {}'s key {key} maps one element, not its content", page_no(page.obj))),
            None if pages_with_content.contains(&page.obj) => {
                out.push(format!("the parent tree has no entry for page {} (key {key})", page_no(page.obj)))
            }
            None => {}
        }
    }
    // Owned marked content is drawn, once.
    for page in pages.iter().filter(|p| pages_with_content.contains(&p.obj)) {
        let content = PageContent::read(&copy, page);
        for (m, uses) in &content.mcid_uses {
            if *uses > 1 {
                out.push(format!("MCID {m} is used {uses} times on page {}", page_no(page.obj)));
            }
        }
        for ((p, mcid), owners) in &tree.mcid_owners {
            if *p == page.obj && owners.iter().any(|o| matches!(o, Owner::Element(_))) && !content.mcid_uses.contains_key(mcid) {
                out.push(format!("page {} doesn't draw MCID {mcid}, which a tag owns", page_no(page.obj)));
            }
        }
    }
    out.sort();
    out
}
