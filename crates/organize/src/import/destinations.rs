//! Carry catalog destinations (ISO 32000-2 §12.3.2.4) into a new document.

use std::collections::BTreeMap;

use pdfcraft_cos::PdfString;

use super::carry::set_name_tree;
use super::{Dict, Document, HashMap, HashSet, ObjRef, Object, OrganizeError, visit_name_tree};

#[derive(Default)]
pub(super) struct Destinations {
    names: BTreeMap<Vec<u8>, Object>,
    legacy: BTreeMap<Vec<u8>, Object>,
}

impl Destinations {
    /// Each table has one target per key. The first source with a copied target keeps the
    /// original key. Local links and bookmarks already use explicit destinations.
    pub(super) fn collect(&mut self, src: &Document, pages: &HashMap<ObjRef, ObjRef>) {
        if pages.is_empty() {
            return;
        }
        let Some(root) = src.root() else { return };
        let catalog = src.get(root);
        let Some(catalog) = catalog.as_dict() else { return };
        if let Some(legacy) = catalog.get(b"Dests").map(|v| src.resolve(v))
            && let Some(legacy) = legacy.as_dict()
        {
            for (key, value) in legacy.iter() {
                collect_entry(&mut self.legacy, src, pages, key, value);
            }
        }
        if let Some(names) = catalog.get(b"Names").map(|v| src.resolve(v))
            && let Some(tree) = names.as_dict().and_then(|d| d.get(b"Dests"))
        {
            visit_name_tree(src, tree, 0, &mut HashSet::new(), &mut |key, value| {
                collect_entry(&mut self.names, src, pages, key, value);
            });
        }
    }

    pub(super) fn write(self, dst: &mut Document) -> Result<(), OrganizeError> {
        if !self.legacy.is_empty() {
            let mut legacy = Dict::new();
            for (key, value) in self.legacy {
                legacy.set(key, value);
            }
            let root = dst.root().ok_or(OrganizeError::NoPageTree)?;
            let legacy = dst.add(legacy);
            dst.update_dict(root, |d| d.set(b"Dests".to_vec(), Object::Ref(legacy)))?;
        }
        if !self.names.is_empty() {
            let names = self.names.into_iter().flat_map(|(key, value)| [Object::String(PdfString::literal(key)), value]).collect();
            set_name_tree(dst, b"Dests", names)?;
        }
        Ok(())
    }
}

fn collect_entry(table: &mut BTreeMap<Vec<u8>, Object>, src: &Document, pages: &HashMap<ObjRef, ObjRef>, key: &[u8], value: &Object) {
    if !table.contains_key(key)
        && let Some(dest) = remap(src, pages, value)
    {
        table.insert(key.to_vec(), dest);
    }
}

/// Copy only the destination array (/D for a dictionary value). A view consists of a name
/// and at most four numeric or null operands. Resolve operands in the source so no source
/// reference can end up in the output after the page reference has been replaced.
fn remap(src: &Document, pages: &HashMap<ObjRef, ObjRef>, value: &Object) -> Option<Object> {
    let value = src.resolve(value);
    let value = match value.as_dict() {
        Some(dict) => src.resolve(dict.get(b"D")?),
        None => value,
    };
    let [page, mode, operands @ ..] = value.as_array()?.as_slice() else { return None };
    let page = *pages.get(&page.as_ref()?)?;
    let mode = src.resolve(mode);
    if operands.len() > 4 || !matches!(mode.as_name()?, b"XYZ" | b"Fit" | b"FitH" | b"FitV" | b"FitR" | b"FitB" | b"FitBH" | b"FitBV") {
        return None;
    }
    let mut out = vec![Object::Ref(page), mode.as_ref().clone()];
    for operand in operands {
        let operand = src.resolve(operand);
        match operand.as_ref() {
            Object::Int(_) | Object::Real(_) | Object::Null => out.push(operand.as_ref().clone()),
            _ => return None,
        }
    }
    Some(Object::Array(out))
}
