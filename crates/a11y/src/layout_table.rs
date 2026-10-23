//! A layout table as plain blocks: a table used only to place content (label and value pairs, a
//! letterhead grid) is read as a data table, with headers and cell navigation it doesn't have. One
//! edit turns the table, its row groups and rows into Div elements and its cells into P elements
//! (Div when a cell holds tags of its own), keeping the content, its order and everything inside
//! the cells, and drops the Table attributes (summary, scope, spans) they no longer have (#4105).

use std::collections::HashSet;

use pdfcraft_cos::{CosError, Dict, Document, ObjRef, Object};

use crate::alt;

/// The most structure elements one table may have: past it the edit is refused.
const MAX_ELEMENTS: usize = 100_000;
/// The deepest nesting of direct (inline) elements followed.
const MAX_DEPTH: usize = 256;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum LayoutTableError {
    #[error("object {0} is not a reachable indirect structure tag")]
    NotATag(ObjRef),
    #[error("the tag is not a table")]
    NotATable,
    #[error("the table has too many tags, or tags nested too deeply")]
    TooLarge,
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

struct Retag<'a> {
    doc: &'a Document,
    role_map: Dict,
    /// Indirect elements under the table still to look at.
    queue: Vec<ObjRef>,
    /// How many elements changed.
    changed: usize,
}

impl Retag<'_> {
    fn standard(&self, d: &Dict) -> Option<Vec<u8>> {
        Some(alt::standard(self.doc, &self.role_map, d.name(b"S")?))
    }

    /// Whether children `k` include structure elements (indirect or direct).
    fn has_tags(&self, k: &Object) -> bool {
        match k {
            Object::Array(items) => items.iter().any(|i| self.has_tags(i)),
            Object::Ref(r) => self.doc.get(*r).as_dict().is_some_and(|d| d.contains(b"S")),
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
        let kids = d.get(b"K").map(|k| self.doc.resolve(k));
        let has_tags = kids.as_deref().is_some_and(|k| self.has_tags(k));
        let Some(block) = block_type(&ty, has_tags, root) else { return Ok(None) };
        let mut out = d.clone();
        out.set(b"S".to_vec(), Object::name(block));
        self.drop_table_attributes(&mut out);
        if let Some(kids) = kids {
            let kids = self.convert_kids(&kids, depth + 1)?;
            out.set(b"K".to_vec(), kids);
        }
        self.changed = self.changed.saturating_add(1);
        Ok(Some(out))
    }

    /// Children `k` with direct table parts converted and indirect elements queued.
    fn convert_kids(&mut self, k: &Object, depth: usize) -> Result<Object, LayoutTableError> {
        if depth > MAX_DEPTH {
            return Err(LayoutTableError::TooLarge);
        }
        Ok(match k {
            Object::Array(items) => Object::Array(items.iter().map(|i| self.convert_kids(i, depth)).collect::<Result<_, _>>()?),
            Object::Ref(r) => {
                if self.doc.get(*r).as_dict().is_some_and(|d| d.contains(b"S")) {
                    self.queue.push(*r);
                }
                k.clone()
            }
            Object::Dict(d) if d.contains(b"S") => match self.convert(d, false, depth)? {
                Some(block) => Object::Dict(block),
                None => k.clone(),
            },
            _ => k.clone(),
        })
    }

    /// Remove the Table attribute objects (`/O /Table`) from `d`'s `/A`, keeping the others and
    /// their revision numbers; an `/A` left empty goes.
    fn drop_table_attributes(&self, d: &mut Dict) {
        let Some(a) = d.get(b"A").cloned() else { return };
        let is_table = |o: &Object| self.doc.resolve(o).as_dict().is_some_and(|x| x.name(b"O") == Some(b"Table"));
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
}

/// Turn layout table `table` (a reachable indirect tag whose type is Table after the role map) into
/// Div and P elements as the module documentation says; returns how many elements changed. Tables
/// nested in its cells stay tables.
pub fn layout_table_to_blocks(doc: &mut Document, table: ObjRef) -> Result<usize, LayoutTableError> {
    let Some(element) = alt::elements(doc).into_iter().find(|e| e.object == table) else {
        return Err(LayoutTableError::NotATag(table));
    };
    let role_map = alt::tree_root(doc).and_then(|r| r.get(b"RoleMap").map(|o| doc.resolve(o))).and_then(|o| o.as_dict().cloned()).unwrap_or_default();
    let mut retag = Retag { doc: &*doc, role_map, queue: Vec::new(), changed: 0 };
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
    let changed = retag.changed;
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
    fn rows_that_point_at_each_other_end() {
        // A row listed twice and a cell that lists its own row: each is converted once.
        let mut d =
            doc("", &["<< /S /Document /K [4 0 R] >>", "<< /S /Table /K [5 0 R 5 0 R] >>", "<< /S /TR /K [6 0 R] >>", "<< /S /TD /K [5 0 R] >>"]);
        assert_eq!(layout_table_to_blocks(&mut d, r(4)), Ok(3));
        assert_eq!((ty(&d, 5), ty(&d, 6)), (b"Div".to_vec(), b"Div".to_vec()));
    }
}
