//! The page-tree nodes that hold resources, shared by Type 3 pruning and resource unlinking.

use std::collections::HashMap;

use pdfcraft_cos::{Document, ObjRef};

use crate::RedactError;

/// Every page-tree node with a `/Resources` of its own, and the pages that draw through it (the
/// nearest such node above a page, or the page itself), in document order.
///
/// Built on `pdfcraft_model::PageNodes`, not the leaves: a node whose resources no page inherits
/// still keeps what it names linked, so redaction must see it. A walk limit is an error, since a
/// resource a page past the cut uses would look unused.
pub(crate) fn resource_holders(doc: &Document) -> Result<Vec<(ObjRef, Vec<ObjRef>)>, RedactError> {
    let mut holders: Vec<(ObjRef, Vec<ObjRef>)> = Vec::new();
    // Holder index by object number.
    let mut index: HashMap<u32, usize> = HashMap::new();
    for node in pdfcraft_model::PageNodes::new(doc) {
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
    use pdfcraft_model::{Limits, PageNodes, PageTreeLimit};
    use pdfcraft_testkit::Pdf;

    use super::resource_holders;
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
        assert_eq!(resource_holders(&doc).unwrap(), [(r(2), vec![r(3)])]);
        nodes.by_ref().for_each(drop);
        // The root's 2,000 kids and the shared array's 2,000 entries once, not 2,000 times over.
        assert!(nodes.entries_read() <= 3 * n, "{} kids entries read", nodes.entries_read());
    }

    /// The live `3 0 R` is the page; the stale `3 1 R` and `3 2 R` after it name nothing, as cos
    /// resolves them: one page, listed once.
    #[test]
    fn stale_generations_beside_the_live_one_add_no_page() {
        let doc = open(
            Pdf::new()
                .obj("<< /Type /Catalog /Pages 2 0 R >>")
                .obj("<< /Type /Pages /Kids [3 0 R 3 1 R 3 2 R] /Count 1 /Resources << >> >>")
                .obj("<< /Type /Page /Parent 2 0 R >>"),
        );
        assert_eq!(resource_holders(&doc).unwrap(), [(r(2), vec![r(3)])]);
    }

    fn capped<T>(limits: Limits, f: impl FnOnce() -> T) -> T {
        pdfcraft_model::with_limits(limits, f)
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
        let limited = |nodes, kids_entries| capped(Limits { nodes, kids_entries, ..Limits::DEFAULT }, || resource_holders(&doc));
        assert!(limited(100, 100).is_ok(), "within the caps");
        assert!(matches!(limited(100, 5), Err(RedactError::PageTree(PageTreeLimit::KidsEntries))), "past the entry cap, nulls included");
        assert!(matches!(limited(1, 100), Err(RedactError::PageTree(PageTreeLimit::Nodes))), "past the node cap");
        assert!(limited(2, 100).is_ok(), "the tree's two nodes, then only nulls: every node visited");
        assert!(limited(100, 11).is_ok(), "exactly at the entry cap");
    }

    /// Two pages inheriting a Type 3 font from the tree node: the holders walk copies nothing, so
    /// only Type 3's own page scan meets an inherited-copy limit, and it refuses rather than judge
    /// glyphs unused from the first page alone.
    #[test]
    fn type3_cleanup_refuses_a_page_scan_cut_by_the_copy_budget() {
        let mut doc = open(
            Pdf::new()
                .obj("<< /Type /Catalog /Pages 2 0 R >>") // 1
                .obj("<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /Resources << /Font << /T3 4 0 R >> >> >>") // 2
                .obj("<< /Type /Page /Parent 2 0 R >>") // 3
                .obj("<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] /CharProcs << >> /Encoding << /Differences [] >> /FirstChar 0 /LastChar 0 /Widths [0] >>") // 4
                .obj("<< /Type /Page /Parent 2 0 R >>"), // 5
        );
        assert!(crate::type3::unlink_type3(&mut doc.clone()).is_ok(), "within the default limits");
        let refused = capped(Limits { inherited_copies: 300, ..Limits::DEFAULT }, || {
            assert_eq!(pdfcraft_model::pages(&doc).len(), 1, "the budget covers one page's copy");
            assert!(resource_holders(&doc).is_ok(), "the holders walk copies nothing");
            crate::type3::unlink_type3(&mut doc)
        });
        assert!(matches!(refused, Err(RedactError::PageTree(PageTreeLimit::InheritedCopies))), "{refused:?}");
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
