//! Structure-derived paragraph levels for read-only Office export.
//!
//! Match by (page, optional form stream, MCID), never by the displayed text: a paragraph
//! and a heading can have exactly the same words.

use crate::alt::standard;
use pdfcraft_cos::{Dict, Document, ObjRef, Object};
use std::collections::{HashMap, HashSet};

type Mark = (ObjRef, Option<ObjRef>, i64);
const MAX_DEPTH: usize = 256;
const MAX_VISITS: usize = 100_000;

/// Explicit heading levels of marked content: 1–6 for H1–H6, 0 for a paragraph.
/// Missing, ambiguous and unnumbered H content has no override; callers retain their heuristic.
#[derive(Debug, Default)]
pub struct HeadingLevels {
    levels: HashMap<Mark, (usize, u8)>,
}

impl HeadingLevels {
    /// The stream is None for page content, or the MCR's /Stm for a form XObject.
    pub fn get(&self, page: ObjRef, stream: Option<ObjRef>, mcid: i64) -> Option<u8> {
        self.get_with_scope(page, stream, mcid).map(|(_, level)| level)
    }

    /// The stable paragraph scope within this lookup, and its level. Marks belonging to
    /// the same structure paragraph share a scope, including its nested spans.
    pub fn get_with_scope(&self, page: ObjRef, stream: Option<ObjRef>, mcid: i64) -> Option<(usize, u8)> {
        self.levels.get(&(page, stream, mcid)).copied()
    }
}

/// Read the structure tree with limits on depth, visits and retained entries.
/// Dictionary and role lookups still depend on the input dictionaries' sizes.
/// Inline elements, inherited /Pg, nested spans, MCR dictionaries and RoleMap are supported.
pub fn heading_levels(doc: &Document) -> HeadingLevels {
    struct Walker<'a> {
        doc: &'a Document,
        roles: &'a Dict,
        out: HeadingLevels,
        ambiguous: HashSet<Mark>,
        path: HashSet<ObjRef>,
        visits: usize,
        owners: HashMap<ObjRef, usize>,
    }
    impl Walker<'_> {
        /// Record a paragraph scope and level, permanently discarding conflicting claims to the same mark.
        fn mark(&mut self, page: Option<ObjRef>, stream: Option<ObjRef>, mcid: i64, level: Option<(usize, u8)>) {
            let (Some(page), Some(level)) = (page, level) else { return };
            if mcid < 0 {
                return;
            }
            let key = (page, stream, mcid);
            if self.ambiguous.contains(&key) {
                return;
            }
            if self.out.levels.get(&key).is_some_and(|old| *old != level) {
                self.out.levels.remove(&key);
                self.ambiguous.insert(key);
            } else {
                self.out.levels.insert(key, level);
            }
        }

        /// Traverse structure children with inherited page and paragraph scope, cycle checks, and visit limits.
        fn walk(
            &mut self,
            item: &Object,
            page: Option<ObjRef>,
            stream: Option<ObjRef>,
            level: Option<(usize, u8)>,
            depth: usize,
            object: Option<ObjRef>,
        ) {
            if self.visits >= MAX_VISITS {
                return;
            }
            self.visits += 1;
            if depth > MAX_DEPTH {
                return;
            }
            match item {
                Object::Ref(r) => {
                    if self.path.insert(*r) {
                        self.walk(&self.doc.get(*r), page, stream, level, depth + 1, Some(*r));
                        self.path.remove(r);
                    }
                }
                Object::Array(items) => {
                    for item in items {
                        if self.visits >= MAX_VISITS {
                            break;
                        }
                        self.walk(item, page, stream, level, depth + 1, None);
                    }
                }
                Object::Int(mcid) => self.mark(page, stream, *mcid, level),
                Object::Dict(d) => {
                    let page = d.get(b"Pg").and_then(Object::as_ref).or(page);
                    if d.name(b"Type") == Some(b"MCR") {
                        if let Some(mcid) = d.get(b"MCID").and_then(Object::as_int) {
                            let stream = match d.get(b"Stm") {
                                None => None,
                                Some(Object::Ref(stream)) => Some(*stream),
                                _ => return,
                            };
                            self.mark(page, stream, mcid, level);
                        }
                    } else if let Some(kind) = d.name(b"S") {
                        let kind = standard(self.doc, self.roles, kind);
                        let owner = object.map_or(self.visits, |r| *self.owners.entry(r).or_insert(self.visits));
                        let level = match kind.as_ref() {
                            [b'H', n @ b'1'..=b'6'] => Some((owner, n - b'0')),
                            b"P" => Some((owner, 0)),
                            b"H" => None,
                            _ => level,
                        };
                        if let Some(kids) = d.get(b"K") {
                            self.walk(kids, page, stream, level, depth + 1, None);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    // Borrow direct dictionaries and hold indirect objects alive: cloning the root's /K
    // or the entire RoleMap would allocate before our traversal budget can take effect.
    let Some(catalog) = doc.root().map(|r| doc.get(r)) else { return HeadingLevels::default() };
    let Some(root) = catalog.as_dict().and_then(|d| d.get(b"StructTreeRoot")) else { return HeadingLevels::default() };
    let indirect_root = root.as_ref().map(|r| doc.get(r));
    let Some(root) = indirect_root.as_deref().unwrap_or(root).as_dict() else { return HeadingLevels::default() };
    let roles = root.get(b"RoleMap");
    let indirect_roles = roles.and_then(Object::as_ref).map(|r| doc.get(r));
    let empty = Dict::new();
    let roles = indirect_roles.as_deref().or(roles).and_then(Object::as_dict).unwrap_or(&empty);
    let mut walker =
        Walker { doc, roles, out: HeadingLevels::default(), ambiguous: HashSet::new(), path: HashSet::new(), visits: 0, owners: HashMap::new() };
    if let Some(kids) = root.get(b"K") {
        walker.walk(kids, None, None, None, 0, None);
    }
    walker.out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::{Dict, Object};
    use std::sync::Arc;

    /// Create a minimal page document for synthetic structure-tree tests.
    fn doc() -> (Document, ObjRef, ObjRef) {
        let bytes = Arc::new(b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [] /Count 0 >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n".to_vec());
        let mut doc = Document::open(bytes).unwrap();
        let page = doc.add(Object::Dict(Dict::new()));
        let root = doc.add(Object::Dict(Dict::new()));
        let catalog = doc.root().unwrap();
        doc.update_dict(catalog, |d| d.set("StructTreeRoot", Object::Ref(root))).unwrap();
        (doc, page, root)
    }
    /// Build a synthetic structure element with a role, page, and child content.
    fn element(kind: &str, page: ObjRef, kids: Object) -> Object {
        let mut d = Dict::new();
        d.set("S", Object::name(kind));
        d.set("Pg", Object::Ref(page));
        d.set("K", kids);
        Object::Dict(d)
    }
    /// Attach test structure children and a RoleMap to the document catalog.
    fn attach(doc: &mut Document, root: ObjRef, kids: Vec<Object>) {
        doc.update_dict(root, |d| d.set("K", Object::Array(kids))).unwrap();
    }

    /// Resolve heading aliases across inline and indirect elements and inherit levels through nested spans.
    #[test]
    fn levels_follow_rolemap_inline_and_indirect_elements_and_nested_spans() {
        let (mut doc, page, root) = doc();
        let span = element("Span", page, Object::Array(vec![Object::Int(0), Object::Int(1)]));
        let heading = doc.add(element("Chapter", page, span));
        let mut roles = Dict::new();
        roles.set("Chapter", Object::name("Alias"));
        roles.set("Alias", Object::name("H3"));
        doc.update_dict(root, |d| d.set("RoleMap", Object::Dict(roles))).unwrap();
        attach(&mut doc, root, vec![Object::Ref(heading), element("P", page, Object::Int(2)), element("H", page, Object::Int(3))]);
        let levels = heading_levels(&doc);
        assert_eq!(levels.get(page, None, 0), Some(3));
        assert_eq!(levels.get(page, None, 1), Some(3));
        assert_eq!(levels.get(page, None, 2), Some(0));
        assert_eq!(levels.get_with_scope(page, None, 0), levels.get_with_scope(page, None, 1));
        assert_ne!(levels.get_with_scope(page, None, 0).map(|x| x.0), levels.get_with_scope(page, None, 2).map(|x| x.0));
        assert_eq!(levels.get(page, None, 3), None);
        // A direct root and an indirect RoleMap have the same semantics.
        let mut direct = doc.get(root).as_dict().unwrap().clone();
        let roles = doc.add(direct.get(b"RoleMap").unwrap().clone());
        direct.set("RoleMap", Object::Ref(roles));
        doc.update_dict(doc.root().unwrap(), |d| d.set("StructTreeRoot", Object::Dict(direct))).unwrap();
        assert_eq!(heading_levels(&doc).get(page, None, 0), Some(3));
    }

    /// Distinguish identical MCIDs belonging to different pages or form streams.
    #[test]
    fn mcr_stream_and_page_identity_do_not_collide() {
        let (mut doc, page, root) = doc();
        let other_page = doc.add(Object::Null);
        let form = doc.add(Object::Null);
        let mut mcr = Dict::new();
        mcr.set("Type", Object::name("MCR"));
        mcr.set("MCID", Object::Int(7));
        mcr.set("Stm", Object::Ref(form));
        mcr.set("Pg", Object::Ref(other_page));
        let mcr = doc.add(Object::Dict(mcr));
        attach(&mut doc, root, vec![element("H2", page, Object::Int(7)), element("H5", page, Object::Ref(mcr))]);
        let levels = heading_levels(&doc);
        assert_eq!(levels.get(page, None, 7), Some(2));
        assert_eq!(levels.get(other_page, Some(form), 7), Some(5));
        assert_eq!(levels.get(page, Some(form), 7), None);
        assert_eq!(levels.get(other_page, None, 7), None);
    }

    /// Fall back when tags are malformed, conflicting, cyclic, or lack numbered heading semantics.
    #[test]
    fn malformed_ambiguous_and_cyclic_tags_leave_no_override() {
        let (mut doc, page, root) = doc();
        let cycle = doc.add(Object::Null);
        doc.set(cycle, element("Span", page, Object::Ref(cycle)));
        attach(
            &mut doc,
            root,
            vec![
                Object::Ref(cycle),
                element("H1", page, Object::Int(0)),
                element("P", page, Object::Int(0)),
                element("H6", page, Object::Int(-1)),
                element("H6", page, Object::Int(1)),
            ],
        );
        let levels = heading_levels(&doc);
        assert_eq!(levels.get(page, None, 0), None);
        assert_eq!(levels.get(page, None, -1), None);
        assert_eq!(levels.get(page, None, 1), Some(6));
    }

    /// Stop excessive structure nesting and cyclic role aliases without producing an override.
    #[test]
    fn excessive_depth_and_rolemap_cycles_are_bounded() {
        let (mut doc, page, root) = doc();
        let mut deep = Object::Int(0);
        for _ in 0..300 {
            deep = Object::Ref(doc.add(element("Span", page, deep)));
        }
        let mut roles = Dict::new();
        roles.set("Loop", Object::name("Again"));
        roles.set("Again", Object::name("Loop"));
        doc.update_dict(root, |d| d.set("RoleMap", Object::Dict(roles))).unwrap();
        attach(&mut doc, root, vec![element("H1", page, deep), element("Loop", page, Object::Int(1))]);
        let levels = heading_levels(&doc);
        assert_eq!(levels.get(page, None, 0), None);
        assert_eq!(levels.get(page, None, 1), None);
    }

    /// Preserve shared paragraph identity while stopping repeated references at the visit limit.
    #[test]
    fn repeated_aliases_keep_their_identity_and_exhaust_the_work_budget() {
        let (mut doc, page, root) = doc();
        let heading = doc.add(element("H4", page, Object::Int(0)));
        let mut kids = vec![Object::Ref(heading); MAX_VISITS];
        kids.push(element("H1", page, Object::Int(1)));
        attach(&mut doc, root, kids);
        let levels = heading_levels(&doc);
        assert_eq!(levels.get(page, None, 0), Some(4));
        assert_eq!(levels.get(page, None, 1), None);
    }
}
