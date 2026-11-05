//! The page-tree nodes that hold resources, shared by Type 3 pruning and resource unlinking.

use std::collections::HashMap;

use pdfcraft_cos::{Document, ObjRef};

use crate::RedactError;

/// Every page-tree node with a `/Resources` of its own, and the pages that draw through it (the
/// nearest such node above a page, or the page itself), in document order.
///
/// This is `pdfcraft_model::PageNodes`, the walk every page list comes from, with the nodes kept:
/// a leaf-only walk would miss a node whose resources no page inherits (every page under it has
/// its own, or its `/Kids` is broken), and an image or Type 3 font it still names would stay linked
/// after redaction. A page listed under two holders draws through the first, as `pages` inherits.
/// A tree too large to walk whole is an error rather than a shorter list: a resource a page past
/// the cut uses could look unused, and one a redaction removed could stay linked.
pub(crate) fn resource_holders(doc: &Document) -> Result<Vec<(ObjRef, Vec<ObjRef>)>, RedactError> {
    holders_in(pdfcraft_model::PageNodes::new(doc))
}

/// [`resource_holders`] over `nodes`.
fn holders_in(nodes: pdfcraft_model::PageNodes<'_>) -> Result<Vec<(ObjRef, Vec<ObjRef>)>, RedactError> {
    let mut holders: Vec<(ObjRef, Vec<ObjRef>)> = Vec::new();
    // Holder index by object number.
    let mut index: HashMap<u32, usize> = HashMap::new();
    for node in nodes {
        let node = node?;
        if node.dict().is_some_and(|d| d.contains(b"Resources")) {
            index.insert(node.obj.num, holders.len());
            holders.push((node.obj, Vec::new()));
        }
        // A node is walked before the nodes under it, so the holder a page draws through is listed.
        if node.is_leaf()
            && let Some(holder) = node.inherited_from(b"Resources").and_then(|h| index.get(&h.num)).and_then(|i| holders.get_mut(*i))
        {
            holder.1.push(node.obj);
        }
    }
    Ok(holders)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_cos::{Document, ObjRef};
    use pdfcraft_model::{PageNodes, PageTreeLimit};
    use pdfcraft_testkit::Pdf;

    use super::{holders_in, resource_holders};
    use crate::RedactError;

    fn r(num: u32) -> ObjRef {
        ObjRef { num, generation: 0 }
    }

    fn open(pdf: Pdf) -> Document {
        Document::open(Arc::new(pdf.build())).unwrap()
    }

    /// Pages draw through the nearest node above them with its own /Resources; a page with its
    /// own is its own holder; a node listed twice, or a cycle, is visited once.
    #[test]
    fn pages_go_to_the_nearest_resource_holder() {
        let doc = open(
            Pdf::new()
                .obj("<< /Type /Catalog /Pages 2 0 R >>") // 1
                .obj("<< /Type /Pages /Kids [3 0 R 4 0 R 3 0 R] /Count 3 /Resources << >> >>") // 2
                .obj("<< /Type /Pages /Parent 2 0 R /Kids [5 0 R 6 0 R 2 0 R] /Count 3 >>") // 3
                .obj("<< /Type /Page /Parent 2 0 R /Resources << >> >>") // 4
                .obj("<< /Type /Page /Parent 3 0 R >>") // 5
                .obj("<< /Type /Page /Parent 3 0 R >>"), // 6
        );
        assert_eq!(resource_holders(&doc).unwrap(), [(r(2), vec![r(5), r(6)]), (r(4), vec![r(4)])]);
    }

    /// A node whose resources no page draws through is still a holder: what it names stays
    /// reachable, so redaction must see it.
    #[test]
    fn a_holder_no_page_draws_through_is_listed() {
        let doc = open(
            Pdf::new()
                .obj("<< /Type /Catalog /Pages 2 0 R >>") // 1
                .obj("<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 1 >>") // 2
                .obj("<< /Type /Pages /Kids [4 0 R] /Count 1 /Resources << /Font << >> >> >>") // 3: no page inherits
                .obj("<< /Type /Page /Parent 3 0 R /Resources << >> >>") // 4
                .obj("<< /Type /Pages /Kids 9 0 R /Count 0 /Resources << >> >>"), // 5: broken /Kids
        );
        assert_eq!(resource_holders(&doc).unwrap(), [(r(3), vec![]), (r(4), vec![r(4)]), (r(5), vec![])]);
    }

    /// Many page-tree nodes sharing one indirect `/Kids` array read it once, not once per node.
    #[test]
    fn a_kids_array_shared_by_many_nodes_is_read_once() {
        let mut pdf = Pdf::new().obj("<< /Type /Catalog /Pages 2 0 R >>"); // 1
        let n = 2_000;
        let nodes: String = (0..n).map(|i| format!("{} 0 R ", 5 + i)).collect();
        pdf = pdf.obj(format!("<< /Type /Pages /Kids [{nodes}] /Count {n} /Resources << >> >>")); // 2
        pdf = pdf.obj("<< /Type /Page /Parent 2 0 R >>"); // 3
        pdf = pdf.obj(format!("[{}]", "3 0 R ".repeat(n))); // 4: the shared array
        for _ in 0..n {
            pdf = pdf.obj("<< /Type /Pages /Kids 4 0 R >>");
        }
        let doc = open(pdf);
        let mut nodes = PageNodes::new(&doc);
        assert_eq!(holders_in(PageNodes::new(&doc)).unwrap(), [(r(2), vec![r(3)])]);
        nodes.by_ref().for_each(drop);
        // The root's 2,000 kids and the shared array's 2,000 entries once, not 2,000 times over.
        assert!(nodes.entries_read() <= 3 * n, "{} kids entries read", nodes.entries_read());
    }

    /// `3 1 R` names object 3, as `3 0 R` does: one page, listed once.
    #[test]
    fn references_differing_only_in_generation_are_one_page() {
        let doc = open(
            Pdf::new()
                .obj("<< /Type /Catalog /Pages 2 0 R >>")
                .obj("<< /Type /Pages /Kids [3 0 R 3 1 R 3 2 R] /Count 1 /Resources << >> >>")
                .obj("<< /Type /Page /Parent 2 0 R >>"),
        );
        assert_eq!(resource_holders(&doc).unwrap(), [(r(2), vec![r(3)])]);
    }

    /// Past either cap the walk is an error, every entry counted (references or not), so redaction
    /// refuses rather than miss pages.
    #[test]
    fn a_walk_past_its_caps_is_an_error() {
        let nulls = "null ".repeat(10);
        let doc = open(
            Pdf::new()
                .obj("<< /Type /Catalog /Pages 2 0 R >>")
                .obj(format!("<< /Type /Pages /Kids [3 0 R {nulls}] /Count 1 /Resources << >> >>"))
                .obj("<< /Type /Page /Parent 2 0 R >>"),
        );
        let limited = |nodes, entries| holders_in(PageNodes::with_limits(&doc, r(2), nodes, entries));
        assert!(limited(100, 100).is_ok(), "within the caps");
        assert!(matches!(limited(100, 5), Err(RedactError::PageTree(PageTreeLimit::KidsEntries))), "past the entry cap, nulls included");
        assert!(matches!(limited(1, 100), Err(RedactError::PageTree(PageTreeLimit::Nodes))), "past the node cap");
        assert!(limited(2, 100).is_ok(), "the tree's two nodes, then only nulls: every node visited");
        assert!(limited(100, 11).is_ok(), "exactly at the entry cap");
    }

    /// Type 3 cleanup and resource unlinking refuse a tree past the walk's entry cap, rather than
    /// judge glyphs and images unused from the pages before it. (One test, so the 4M-entry tree is
    /// built once.)
    #[test]
    fn type3_cleanup_and_unlinking_fail_closed_at_the_walk_cap() {
        let mut doc = open(
            Pdf::new()
                .obj("<< /Type /Catalog /Pages 2 0 R >>") // 1
                .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /Font << /T3 4 0 R >> >> >>") // 2
                .obj("<< /Type /Page /Parent 2 0 R >>") // 3
                .obj("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /CharProcs << >> /Encoding << /Differences [] >> /FirstChar 0 /LastChar 0 /Widths [0] >>"), // 4
        );
        let mut kids = vec![pdfcraft_cos::Object::Null; pdfcraft_model::MAX_KIDS_ENTRIES];
        kids.push(pdfcraft_cos::Object::Ref(r(3)));
        doc.update_dict(r(2), |d| d.set(b"Kids".to_vec(), pdfcraft_cos::Object::Array(kids))).unwrap();
        let refused = |result: Result<(), RedactError>| matches!(result, Err(RedactError::PageTree(PageTreeLimit::KidsEntries)));
        assert!(refused(crate::type3::unlink_type3(&mut doc)), "Type 3 cleanup");
        assert!(refused(crate::interp::unlink_doomed(&mut doc, &[r(9)], &[])), "resource unlinking");
    }

    /// A page listed under two nodes with resources draws through the first, as `pages` inherits.
    #[test]
    fn a_page_under_two_holders_draws_through_the_first() {
        let doc = open(
            Pdf::new()
                .obj("<< /Type /Catalog /Pages 2 0 R >>") // 1
                .obj("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>") // 2
                .obj("<< /Type /Pages /Kids [5 0 R] /Count 1 /Resources << /Font << >> >> >>") // 3: first
                .obj("<< /Type /Pages /Kids [5 0 R] /Count 1 /Resources << >> >>") // 4: second
                .obj("<< /Type /Page /Parent 3 0 R >>"), // 5
        );
        let holders = resource_holders(&doc).unwrap();
        let drawn_through: Vec<ObjRef> = holders.iter().filter(|(_, users)| users.contains(&r(5))).map(|(node, _)| *node).collect();
        assert_eq!(drawn_through, [r(3)]);
    }
}
