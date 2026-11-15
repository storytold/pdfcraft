//! A layout table as plain blocks: a table used only to place content (label and value pairs, a
//! letterhead grid) is read as a data table, with headers and cell navigation it doesn't have. One
//! edit turns the table, its row groups and rows into Div elements and its cells into P elements
//! (Div when a cell holds tags of its own), keeping the content, its order and everything inside
//! the cells, and drops the Table attributes (summary, scope, spans) they no longer have (#4105),
//! whether attached directly (`/A`) or through a class (`/C` and the `/ClassMap`, #7412).

use std::collections::{BTreeMap, HashMap, HashSet};

use pdfcraft_cos::{CosError, Dict, Document, ObjRef, Object};

use crate::alt;

/// The most structure elements one table may have: past it the edit is refused.
const MAX_ELEMENTS: usize = 100_000;
/// The deepest nesting of direct (inline) elements followed.
const MAX_DEPTH: usize = 256;
/// How many names are tried for a class without the Table attributes before giving up.
const MAX_CLASS_NAMES: usize = 1_000;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum LayoutTableError {
    #[error("object {0} is not a reachable indirect structure tag")]
    NotATag(ObjRef),
    #[error("the tag is not a table")]
    NotATable,
    #[error("the table has too many tags, or tags nested too deeply")]
    TooLarge,
    #[error("couldn't add a class without the Table attributes to the structure class map")]
    ClassMap,
    #[error(transparent)]
    Cos(#[from] CosError),
}

/// What a table part becomes; `None` for anything else (which keeps its type and contents).
fn block_type(ty: &[u8], has_tags: bool, root: bool) -> Option<&'static str> {
    match ty {
        b"Table" if root => Some("Div"),
        b"THead" | b"TBody" | b"TFoot" | b"TR" => Some("Div"),
        b"TH" | b"TD" | b"Caption" => Some(if has_tags { "Div" } else { "P" }),
        _ => None,
    }
}

/// What a class becomes on a converted element.
#[derive(Clone)]
enum ClassChange {
    /// No Table attributes: it stays.
    Keep,
    /// Only Table attributes: it goes, with its revision number.
    Drop,
    /// Table and other attributes: this class holds the others, at the same place in `/C`.
    Rename(Vec<u8>),
}

struct Retag<'a> {
    doc: &'a Document,
    role_map: Dict,
    /// The structure tree root's `/ClassMap`, resolved.
    class_map: Dict,
    /// What each class named on a converted element becomes.
    classes: HashMap<Vec<u8>, ClassChange>,
    /// Classes without the Table attributes, to add to the class map: by name, so a lookup is
    /// indexed and they are written in the same order every time.
    new_classes: BTreeMap<Vec<u8>, Object>,
    /// Indirect elements under the table still to look at.
    queue: Vec<ObjRef>,
    /// How many elements changed.
    changed: usize,
}

impl Retag<'_> {
    fn standard(&self, d: &Dict) -> Option<Vec<u8>> {
        Some(alt::standard(self.doc, &self.role_map, d.name(b"S")?))
    }

    /// Whether children `k` include structure elements (indirect or direct, also in an indirect
    /// array of children).
    fn has_tags(&self, k: &Object, depth: usize) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        match k {
            Object::Array(items) => items.iter().any(|i| self.has_tags(i, depth + 1)),
            Object::Ref(r) => match &*self.doc.get(*r) {
                Object::Dict(d) => d.contains(b"S"),
                array @ Object::Array(_) => self.has_tags(array, depth + 1),
                _ => false,
            },
            Object::Dict(d) => d.contains(b"S"),
            _ => false,
        }
    }

    /// Element `d` as a block when it is a part of the table (`root`: the table itself), with its
    /// direct children converted the same way and its indirect ones queued; `None` otherwise.
    fn convert(&mut self, d: &Dict, root: bool, depth: usize) -> Result<Option<Dict>, LayoutTableError> {
        if depth > MAX_DEPTH {
            return Err(LayoutTableError::TooLarge);
        }
        let Some(ty) = self.standard(d) else { return Ok(None) };
        // As written: a single child is often a reference to it, which must stay one.
        let kids = d.get(b"K").cloned();
        let has_tags = kids.as_ref().is_some_and(|k| self.has_tags(k, 0));
        let Some(block) = block_type(&ty, has_tags, root) else { return Ok(None) };
        let mut out = d.clone();
        out.set(b"S".to_vec(), Object::name(block));
        self.drop_table_attributes(&mut out);
        self.drop_table_classes(&mut out)?;
        if let Some(kids) = kids {
            let kids = self.convert_kids(&kids, depth + 1)?;
            out.set(b"K".to_vec(), kids);
        }
        self.changed = self.changed.saturating_add(1);
        Ok(Some(out))
    }

    /// Children `k` with direct table parts converted and indirect elements queued (references
    /// stay references; an indirect array of children is written out only when a direct table
    /// part in it changed).
    fn convert_kids(&mut self, k: &Object, depth: usize) -> Result<Object, LayoutTableError> {
        if depth > MAX_DEPTH {
            return Err(LayoutTableError::TooLarge);
        }
        Ok(match k {
            Object::Array(items) => Object::Array(items.iter().map(|i| self.convert_kids(i, depth)).collect::<Result<_, _>>()?),
            Object::Ref(r) => match &*self.doc.get(*r) {
                Object::Dict(d) if d.contains(b"S") => {
                    self.queue.push(*r);
                    k.clone()
                }
                array @ Object::Array(_) => {
                    let converted = self.convert_kids(array, depth + 1)?;
                    if converted == *array { k.clone() } else { converted }
                }
                _ => k.clone(),
            },
            Object::Dict(d) if d.contains(b"S") => match self.convert(d, false, depth)? {
                Some(block) => Object::Dict(block),
                None => k.clone(),
            },
            _ => k.clone(),
        })
    }

    /// Remove the Table attribute objects (`/O /Table`) from `d`'s `/A`, keeping the others and
    /// their revision numbers; an `/A` left empty goes.
    fn is_table(&self, o: &Object) -> bool {
        self.doc.resolve(o).as_dict().is_some_and(|x| x.name(b"O") == Some(b"Table"))
    }

    fn drop_table_attributes(&self, d: &mut Dict) {
        let Some(a) = d.get(b"A").cloned() else { return };
        let is_table = |o: &Object| self.is_table(o);
        let resolved = self.doc.resolve(&a);
        let Object::Array(items) = &*resolved else {
            if is_table(&a) {
                d.remove(b"A");
            }
            return;
        };
        let mut kept = Vec::with_capacity(items.len());
        let mut dropping = false;
        for item in items {
            match item {
                // A revision number goes with the attribute object before it.
                Object::Int(_) if dropping => {}
                _ => {
                    dropping = is_table(item);
                    if !dropping {
                        kept.push(item.clone());
                    }
                }
            }
        }
        if kept.is_empty() {
            d.remove(b"A");
        } else if kept.len() != items.len() {
            d.set(b"A".to_vec(), Object::Array(kept));
        }
    }

    /// Take the Table attributes `d` gets through its classes (`/C`, a name or an array of names
    /// each optionally followed by a revision number) away from `d` alone: a class holding only
    /// Table attributes leaves its `/C`, and one holding others too is replaced, at the same place
    /// and with the same revision number, by a class holding just the others. The classes
    /// themselves stay, for the other tags that use them. `/C` is left as written when no class
    /// changes.
    fn drop_table_classes(&mut self, d: &mut Dict) -> Result<(), LayoutTableError> {
        let Some(c) = d.get(b"C").cloned() else { return Ok(()) };
        let resolved = self.doc.resolve(&c);
        let (items, single) = match &*resolved {
            Object::Name(_) => (vec![(*resolved).clone()], true),
            Object::Array(items) => (items.clone(), false),
            _ => return Ok(()),
        };
        let mut kept = Vec::with_capacity(items.len());
        let mut changed = false;
        let mut dropping = false;
        for item in items {
            match item {
                // A revision number goes with the class before it.
                Object::Int(_) if dropping => {}
                Object::Name(name) => {
                    dropping = false;
                    match self.class_change(&name)? {
                        ClassChange::Keep => kept.push(Object::Name(name)),
                        ClassChange::Drop => (changed, dropping) = (true, true),
                        ClassChange::Rename(new) => {
                            changed = true;
                            kept.push(Object::Name(new));
                        }
                    }
                }
                other => {
                    dropping = false;
                    kept.push(other);
                }
            }
        }
        if !changed {
            return Ok(());
        }
        match kept.as_slice() {
            [] => {
                d.remove(b"C");
            }
            [name @ Object::Name(_)] if single => d.set(b"C".to_vec(), name.clone()),
            _ => d.set(b"C".to_vec(), Object::Array(kept)),
        }
        Ok(())
    }

    /// What class `name` becomes on a converted element, deciding (and naming a class without
    /// the Table attributes) the first time it is asked.
    fn class_change(&mut self, name: &[u8]) -> Result<ClassChange, LayoutTableError> {
        if let Some(change) = self.classes.get(name) {
            return Ok(change.clone());
        }
        // An attribute object or an array of them (§14.7.6.2), either possibly by reference.
        let change = match self.class_map.get(name).map(|v| (v.clone(), self.doc.resolve(v))) {
            None => ClassChange::Keep,
            Some((value, resolved)) => {
                let members = match &*resolved {
                    Object::Array(items) => items.clone(),
                    _ => vec![value],
                };
                let others: Vec<Object> = members.iter().filter(|m| !self.is_table(m)).cloned().collect();
                if others.len() == members.len() {
                    ClassChange::Keep
                } else if others.is_empty() {
                    ClassChange::Drop
                } else {
                    let value = match <[Object; 1]>::try_from(others) {
                        Ok([one]) => one,
                        Err(others) => Object::Array(others),
                    };
                    ClassChange::Rename(self.class_name(name, value)?)
                }
            }
        };
        self.classes.insert(name.to_vec(), change.clone());
        Ok(change)
    }

    /// A class name for `value` (class `name` without its Table attributes): `<name>_NoTable`, or
    /// that with a number, reusing one that already holds exactly `value` (an earlier conversion).
    fn class_name(&mut self, name: &[u8], value: Object) -> Result<Vec<u8>, LayoutTableError> {
        for n in 0..MAX_CLASS_NAMES {
            let mut candidate = [name, b"_NoTable"].concat();
            if n > 0 {
                candidate.extend_from_slice(n.to_string().as_bytes());
            }
            let pending = self.new_classes.get(&candidate);
            match pending.or_else(|| self.class_map.get(&candidate)) {
                None => {
                    self.new_classes.insert(candidate.clone(), value);
                    return Ok(candidate);
                }
                Some(existing) if *existing == value => return Ok(candidate),
                Some(_) => {}
            }
        }
        Err(LayoutTableError::ClassMap)
    }
}

/// Add `classes` to the structure tree root's class map, where it is written: its own object, or
/// inline in the structure tree root (itself an object, or inline in the catalog).
fn add_classes(doc: &mut Document, classes: &BTreeMap<Vec<u8>, Object>) -> Result<(), LayoutTableError> {
    if classes.is_empty() {
        return Ok(());
    }
    let add = |map: &mut Dict| {
        for (name, value) in classes {
            map.set(name.clone(), value.clone());
        }
    };
    let catalog = doc.root().ok_or(LayoutTableError::ClassMap)?;
    let root = doc.get(catalog).as_dict().and_then(|c| c.get(b"StructTreeRoot").cloned()).ok_or(LayoutTableError::ClassMap)?;
    let mut root_dict = doc.resolve(&root).as_dict().cloned().ok_or(LayoutTableError::ClassMap)?;
    if let Some(Object::Ref(map)) = root_dict.get(b"ClassMap") {
        doc.update_dict(*map, add)?;
        return Ok(());
    }
    // Inline in the structure tree root: that root, with its class map grown.
    let mut map = root_dict.get(b"ClassMap").and_then(Object::as_dict).cloned().unwrap_or_default();
    add(&mut map);
    root_dict.set(b"ClassMap".to_vec(), Object::Dict(map));
    match root.as_ref() {
        Some(root) => doc.update_dict(root, |d| *d = root_dict)?,
        None => doc.update_dict(catalog, |c| c.set(b"StructTreeRoot".to_vec(), Object::Dict(root_dict)))?,
    }
    Ok(())
}

/// Turn layout table `table` (a reachable indirect tag whose type is Table after the role map) into
/// Div and P elements as the module documentation says; returns how many elements changed. Tables
/// nested in its cells stay tables.
pub fn layout_table_to_blocks(doc: &mut Document, table: ObjRef) -> Result<usize, LayoutTableError> {
    let Some(element) = alt::elements(doc).into_iter().find(|e| e.object == table) else {
        return Err(LayoutTableError::NotATag(table));
    };
    let root_map =
        |key: &[u8]| alt::tree_root(doc).and_then(|r| r.get(key).map(|o| doc.resolve(o))).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let (role_map, class_map) = (root_map(b"RoleMap"), root_map(b"ClassMap"));
    let mut retag = Retag { doc: &*doc, role_map, class_map, classes: HashMap::new(), new_classes: BTreeMap::new(), queue: Vec::new(), changed: 0 };
    if retag.standard(&element.dict).as_deref() != Some(b"Table".as_slice()) {
        return Err(LayoutTableError::NotATable);
    }
    // Read everything first, then write: a refused table changes nothing.
    let mut updates: Vec<(ObjRef, Dict)> = Vec::new();
    let mut seen = HashSet::new();
    if let Some(block) = retag.convert(&element.dict, true, 0)? {
        seen.insert(table);
        updates.push((table, block));
    }
    while let Some(r) = retag.queue.pop() {
        if !seen.insert(r) {
            continue;
        }
        if seen.len() > MAX_ELEMENTS {
            return Err(LayoutTableError::TooLarge);
        }
        let Some(d) = retag.doc.get(r).as_dict().cloned() else { continue };
        if let Some(block) = retag.convert(&d, false, 0)? {
            updates.push((r, block));
        }
    }
    let (changed, new_classes) = (retag.changed, retag.new_classes);
    add_classes(doc, &new_classes)?;
    for (r, block) in updates {
        doc.update_dict(r, |d| *d = block)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_cos::{Document, ObjRef, Object};

    use super::*;

    /// A tagged document: the catalog (1), the structure tree root (2, `root` its other entries)
    /// over the Document element (3), then `objects` from 4 on.
    fn doc(root: &str, objects: &[&str]) -> Document {
        let mut bodies = vec!["<< /Type /Catalog /StructTreeRoot 2 0 R >>".to_string(), format!("<< /Type /StructTreeRoot /K 3 0 R {root} >>")];
        bodies.extend(objects.iter().map(|s| s.to_string()));
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

    fn r(n: u32) -> ObjRef {
        ObjRef::new(n, 0)
    }

    fn dict(doc: &Document, n: u32) -> Dict {
        doc.get(r(n)).as_dict().cloned().unwrap()
    }

    fn ty(doc: &Document, n: u32) -> Vec<u8> {
        dict(doc, n).name(b"S").unwrap().to_vec()
    }

    #[test]
    fn a_label_and_value_table_becomes_divs_and_paragraphs() {
        let mut d = doc(
            "",
            &[
                "<< /S /Document /K [4 0 R] >>",
                // 4: the table, with a layout attribute and a summary (revision 1).
                "<< /S /Table /K [5 0 R 8 0 R] /A [ << /O /Layout /Placement /Block >> << /O /Table /Summary (s) >> 1 ] >>",
                "<< /S /TR /K [6 0 R 7 0 R] >>",
                "<< /S /TH /K 0 /A << /O /Table /Scope /Row >> >>",
                "<< /S /TD /K 1 >>",
                "<< /S /TR /K [9 0 R 10 0 R] >>",
                "<< /S /TD /K 2 >>",
                "<< /S /TD /K 3 /A [ << /O /Table /ColSpan 1 >> << /O /Layout /TextAlign /End >> ] >>",
            ],
        );
        let before: Vec<ObjRef> = crate::tags(&d).iter().map(|t| t.object).collect();
        assert_eq!(layout_table_to_blocks(&mut d, r(4)), Ok(7));
        for (n, want) in [(4, "Div"), (5, "Div"), (6, "P"), (7, "P"), (8, "Div"), (9, "P"), (10, "P")] {
            assert_eq!(ty(&d, n), want.as_bytes(), "object {n}");
        }
        // The content and the order stay; Table attributes go, layout ones stay.
        assert_eq!(crate::tags(&d).iter().map(|t| t.object).collect::<Vec<_>>(), before);
        assert_eq!(dict(&d, 6).get(b"K"), Some(&Object::Int(0)));
        assert_eq!(dict(&d, 6).get(b"A"), None);
        let Some(Object::Array(a)) = dict(&d, 4).get(b"A").cloned() else { panic!("{:?}", dict(&d, 4)) };
        assert_eq!(a.len(), 1, "the summary and its revision number go: {a:?}");
        assert_eq!(a[0].as_dict().and_then(|x| x.name(b"O")), Some(&b"Layout"[..]));
        let Some(Object::Array(a)) = dict(&d, 10).get(b"A").cloned() else { panic!() };
        assert_eq!(a.len(), 1);
    }

    fn class_map(doc: &Document) -> Dict {
        let map = dict(doc, 2).get(b"ClassMap").cloned().unwrap();
        doc.resolve(&map).as_dict().cloned().unwrap()
    }

    fn name(n: &str) -> Object {
        Object::name(n)
    }

    #[test]
    fn table_attributes_given_through_a_shared_class_leave_only_the_converted_table() {
        // #7412: two tables share a class holding a Table summary and a Layout placement.
        let mut d = doc(
            "/ClassMap << /SharedStyle [8 0 R 9 0 R] >>",
            &[
                "<< /S /Document /K [4 0 R 6 0 R] >>",
                "<< /S /Table /K [5 0 R] /C [/SharedStyle 1] /R 1 >>",
                "<< /S /TR /K [ << /S /TD /K 0 >> ] >>",
                "<< /S /Table /K [7 0 R] /C [/SharedStyle 1] /R 1 >>",
                "<< /S /TR /K [ << /S /TD /K 1 >> ] >>",
                "<< /O /Table /Summary (Layout table summary) >>",
                "<< /O /Layout /Placement /Block >>",
            ],
        );
        let shared = class_map(&d).get(b"SharedStyle").cloned();
        assert_eq!(layout_table_to_blocks(&mut d, r(4)), Ok(3));
        // The Div's class keeps its place and revision number, without the summary.
        assert_eq!(dict(&d, 4).get(b"C"), Some(&Object::Array(vec![name("SharedStyle_NoTable"), Object::Int(1)])));
        assert_eq!(class_map(&d).get(b"SharedStyle_NoTable"), Some(&Object::Ref(r(9))));
        // The class itself, and the other table using it, stay as they were.
        assert_eq!(class_map(&d).get(b"SharedStyle").cloned(), shared);
        assert_eq!(dict(&d, 6).get(b"C"), Some(&Object::Array(vec![name("SharedStyle"), Object::Int(1)])));
        assert_eq!(ty(&d, 6), b"Table");

        // Converting the other table reuses the class made for the first.
        assert_eq!(layout_table_to_blocks(&mut d, r(6)), Ok(3));
        assert_eq!(dict(&d, 6).get(b"C"), dict(&d, 4).get(b"C"));
        assert_eq!(class_map(&d).len(), 2, "{:?}", class_map(&d));
    }

    #[test]
    fn a_class_of_table_attributes_alone_goes_and_a_taken_name_is_not_reused() {
        // The class map is its own object (8); /TableOnly is an indirect array of Table attributes;
        // /Mixed_NoTable is already a class of its own, so the new one takes another name.
        let mut d = doc(
            "/ClassMap 8 0 R",
            &[
                "<< /S /Document /K [4 0 R 6 0 R] >>",
                "<< /S /Table /K [5 0 R] /C 11 0 R >>",
                "<< /S /TR /K [ << /S /TD /K 0 /C /TableOnly >> << /S /TD /K 1 /C /Plain >> ] >>",
                "<< /S /Table /K [7 0 R] /C /Mixed >>",
                "<< /S /TR /K [ << /S /TD /K 2 >> ] >>",
                "<< /TableOnly 9 0 R /Plain << /O /Layout /TextAlign /End >> /Mixed [ << /O /Table /Summary (s) >> << /O /List /ListNumbering /Disc >> ] \
                 /Mixed_NoTable << /O /Layout /Placement /Inline >> >>",
                "[ << /O /Table /Scope /Row >> << /O /Table /ColSpan 2 >> ]",
                "<< /O /Layout >>",
                "[/Plain /TableOnly 2 /Plain]",
            ],
        );
        let before = class_map(&d);
        assert_eq!(layout_table_to_blocks(&mut d, r(4)), Ok(4));
        // An indirect /C: the Table-only class and its revision number go, the others stay.
        assert_eq!(dict(&d, 4).get(b"C"), Some(&Object::Array(vec![name("Plain"), name("Plain")])));
        let Some(Object::Array(cells)) = dict(&d, 5).get(b"K").cloned() else { panic!("{:?}", dict(&d, 5)) };
        assert_eq!(cells[0].as_dict().map(|c| c.contains(b"C")), Some(false), "a /C of one Table-only class goes");
        assert_eq!(cells[1].as_dict().and_then(|c| c.get(b"C")), Some(&name("Plain")));
        assert_eq!(*d.get(r(11)), Object::Array(vec![name("Plain"), name("TableOnly"), Object::Int(2), name("Plain")]), "written as a new /C");
        assert_eq!(class_map(&d), before, "no class was needed");

        assert_eq!(layout_table_to_blocks(&mut d, r(6)), Ok(3));
        assert_eq!(dict(&d, 6).get(b"C"), Some(&name("Mixed_NoTable1")));
        let added = class_map(&d).get(b"Mixed_NoTable1").cloned().unwrap();
        assert_eq!(added.as_dict().and_then(|a| a.name(b"O")), Some(&b"List"[..]));
        assert_eq!(class_map(&d).get(b"Mixed_NoTable"), before.get(b"Mixed_NoTable"), "the class already there is kept");
        assert_eq!(class_map(&d).get(b"Mixed"), before.get(b"Mixed"));
    }

    #[test]
    fn what_the_cells_hold_stays_and_a_table_in_a_cell_stays_a_table() {
        let mut d = doc(
            "",
            &[
                "<< /S /Document /K [4 0 R] >>",
                "<< /S /Table /K [5 0 R] >>",
                "<< /S /TR /K [6 0 R 7 0 R] >>",
                // 6: a cell holding a paragraph; 7: a cell holding a table of its own.
                "<< /S /TD /K [8 0 R] >>",
                "<< /S /TD /K [9 0 R] >>",
                "<< /S /P /K 0 >>",
                "<< /S /Table /K [10 0 R] >>",
                "<< /S /TR /K [11 0 R] >>",
                "<< /S /TD /K 1 >>",
            ],
        );
        layout_table_to_blocks(&mut d, r(4)).unwrap();
        for (n, want) in [(4, "Div"), (5, "Div"), (6, "Div"), (7, "Div"), (8, "P"), (9, "Table"), (10, "TR"), (11, "TD")] {
            assert_eq!(ty(&d, n), want.as_bytes(), "object {n}");
        }
    }

    #[test]
    fn direct_rows_and_cells_are_converted_in_place() {
        let mut d = doc(
            "/RoleMap << /Grid /Table /Row /TR >>",
            &[
                "<< /S /Document /K [4 0 R] >>",
                "<< /S /Grid /K [ << /S /Row /K [ << /S /TD /K 0 >> << /S /TD /K [5 0 R] >> ] >> ] >>",
                "<< /S /Span /K 1 >>",
            ],
        );
        assert_eq!(layout_table_to_blocks(&mut d, r(4)), Ok(4));
        let table = dict(&d, 4);
        assert_eq!(table.name(b"S"), Some(&b"Div"[..]), "the role map makes /Grid a table");
        let Some(Object::Array(rows)) = table.get(b"K").cloned() else { panic!() };
        let row = rows[0].as_dict().unwrap();
        assert_eq!(row.name(b"S"), Some(&b"Div"[..]));
        let Some(Object::Array(cells)) = row.get(b"K").cloned() else { panic!() };
        assert_eq!(cells[0].as_dict().and_then(|c| c.name(b"S")), Some(&b"P"[..]));
        assert_eq!(cells[1].as_dict().and_then(|c| c.name(b"S")), Some(&b"Div"[..]), "it holds a Span tag");
        assert_eq!(ty(&d, 5), b"Span");
    }

    #[test]
    fn only_a_reachable_table_is_changed() {
        let mut d = doc("", &["<< /S /Document /K [4 0 R] >>", "<< /S /P /K 0 >>", "<< /S /Table >>"]);
        assert_eq!(layout_table_to_blocks(&mut d, r(4)), Err(LayoutTableError::NotATable));
        assert_eq!(layout_table_to_blocks(&mut d, r(5)), Err(LayoutTableError::NotATag(r(5))), "not in the structure tree");
        assert_eq!(layout_table_to_blocks(&mut d, ObjRef::new(4, 1)), Err(LayoutTableError::NotATag(ObjRef::new(4, 1))));
        assert_eq!(ty(&d, 5), b"Table");
    }

    #[test]
    fn a_single_child_written_as_a_reference_stays_its_own_object() {
        // /K 5 0 R: the table's one row, the row's one cell; 8: a row whose cells are an indirect
        // array (object 9) holding a direct cell and a reference.
        let mut d = doc(
            "",
            &[
                "<< /S /Document /K [4 0 R 7 0 R] >>",
                "<< /S /Table /K 5 0 R >>",
                "<< /S /TR /K 6 0 R >>",
                "<< /S /TD /K 0 >>",
                "<< /S /Table /K 8 0 R >>",
                "<< /S /TR /K 9 0 R >>",
                "[ << /S /TD /K 1 >> 10 0 R ]",
                "<< /S /TD /K 2 >>",
            ],
        );
        assert_eq!(layout_table_to_blocks(&mut d, r(4)), Ok(3));
        assert_eq!(dict(&d, 4).get(b"K"), Some(&Object::Ref(r(5))), "the row stays a reference");
        assert_eq!(dict(&d, 5).get(b"K"), Some(&Object::Ref(r(6))));
        for (n, want) in [(4, "Div"), (5, "Div"), (6, "P")] {
            assert_eq!(ty(&d, n), want.as_bytes(), "object {n}");
        }
        assert!(crate::tags(&d).iter().any(|t| t.object == r(6)), "the cell is still listed");
        assert_eq!(layout_table_to_blocks(&mut d, r(7)), Ok(4));
        let Some(Object::Array(cells)) = dict(&d, 8).get(b"K").cloned() else { panic!("{:?}", dict(&d, 8)) };
        assert_eq!(cells[0].as_dict().and_then(|c| c.name(b"S")), Some(&b"P"[..]));
        assert_eq!(cells[1], Object::Ref(r(10)));
        assert_eq!(ty(&d, 10), b"P");
    }

    #[test]
    fn rows_that_point_at_each_other_end() {
        // A row listed twice and a cell that lists its own row: each is converted once.
        let mut d =
            doc("", &["<< /S /Document /K [4 0 R] >>", "<< /S /Table /K [5 0 R 5 0 R] >>", "<< /S /TR /K [6 0 R] >>", "<< /S /TD /K [5 0 R] >>"]);
        assert_eq!(layout_table_to_blocks(&mut d, r(4)), Ok(3));
        assert_eq!((ty(&d, 5), ty(&d, 6)), (b"Div".to_vec(), b"Div".to_vec()));
    }
}
