//! The page-tree nodes that hold resources, shared by Type 3 pruning and resource unlinking.

use std::collections::HashSet;

use pdfcraft_cos::{Document, ObjRef, Object};

/// Most page-tree nodes, or forms, one walk visits.
pub(crate) const MAX_HOLDERS: usize = 1_000_000;

/// Every page-tree node under `top` with a /Resources of its own, and the pages that draw
/// through it (the nearest such node above a page, or the page itself). Cycles are cut by a
/// seen set. At most [`MAX_HOLDERS`] distinct nodes are visited; past that the nodes still queued
/// are dropped without visiting their children, so the pages under them are not listed.
pub(crate) fn resource_holders(doc: &Document, top: ObjRef) -> Vec<(ObjRef, Vec<ObjRef>)> {
    let mut holders: Vec<(ObjRef, Vec<ObjRef>)> = Vec::new();
    let mut seen = HashSet::new();
    let mut stack: Vec<(ObjRef, Option<usize>)> = vec![(top, None)];
    while let Some((node, mut holder)) = stack.pop() {
        if !seen.insert(node) || seen.len() > MAX_HOLDERS {
            continue;
        }
        let obj = doc.get(node);
        let Some(d) = obj.as_dict() else { continue };
        if d.contains(b"Resources") {
            holders.push((node, Vec::new()));
            holder = Some(holders.len() - 1);
        }
        let is_pages = d.name(b"Type") == Some(b"Pages") || (d.contains(b"Kids") && d.name(b"Type") != Some(b"Page"));
        if is_pages {
            let kids = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
            stack.extend(kids.iter().filter_map(Object::as_ref).map(|k| (k, holder)));
        } else if let Some(h) = holder.and_then(|i| holders.get_mut(i)) {
            h.1.push(node);
        }
    }
    holders
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_cos::{Document, ObjRef};
    use pdfcraft_testkit::Pdf;

    use super::resource_holders;

    fn r(num: u32) -> ObjRef {
        ObjRef { num, generation: 0 }
    }

    /// Pages draw through the nearest node above them with its own /Resources; a page with its
    /// own is its own holder; a node listed twice, or a cycle, is visited once.
    #[test]
    fn pages_go_to_the_nearest_resource_holder() {
        let bytes = Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>") // 1
            .obj("<< /Type /Pages /Kids [3 0 R 4 0 R 3 0 R] /Count 3 /Resources << >> >>") // 2
            .obj("<< /Type /Pages /Parent 2 0 R /Kids [5 0 R 6 0 R 2 0 R] /Count 3 >>") // 3
            .obj("<< /Type /Page /Parent 2 0 R /Resources << >> >>") // 4
            .obj("<< /Type /Page /Parent 3 0 R >>") // 5
            .obj("<< /Type /Page /Parent 3 0 R >>") // 6
            .build();
        let doc = Document::open(Arc::new(bytes)).unwrap();
        let mut holders = resource_holders(&doc, r(2));
        for (_, pages) in &mut holders {
            pages.sort_by_key(|p| p.num);
        }
        holders.sort_by_key(|(node, _)| node.num);
        assert_eq!(holders, [(r(2), vec![r(5), r(6)]), (r(4), vec![r(4)])]);
    }
}
