//! The page-tree nodes that hold resources, shared by Type 3 pruning and resource unlinking.

use std::collections::HashSet;
use std::sync::Arc;

use pdfcraft_cos::{CosError, Document, ObjRef, Object};

use crate::RedactError;

/// Most page-tree nodes, or forms, one walk visits.
pub(crate) const MAX_HOLDERS: usize = 1_000_000;

/// Every page-tree node under `top` with a /Resources of its own, and the pages that draw
/// through it (the nearest such node above a page, or the page itself).
///
/// The walk keeps one cursor per open `/Kids` array and reads its entries in document order, as
/// `pdfcraft_model::pages` does, so a page listed under two nodes draws through the one it inherits
/// from there (the first). It is hardened as `pdfcraft_model::PageLeaves` is: an
/// indirect `/Kids` array shared by several nodes is read under the first one only (re-reading it
/// made trees whose nodes all share one long array take quadratic work and memory), objects are
/// told apart by number as `Document::get` finds them, and cycles end at the seen check.
///
/// A tree past [`MAX_HOLDERS`] nodes or [`pdfcraft_model::MAX_KIDS_ENTRIES`] `/Kids` entries (the
/// tree's own: the one-entry list the walk starts from does not count; the entry cap matters because
/// an object stream can give many numbers one node's bytes) is an error: the pages past that point
/// would not be listed, so resources they use could look unused and a redaction could leave a
/// removed object linked. It fails closed instead.
pub(crate) fn resource_holders(doc: &Document, top: ObjRef) -> Result<Vec<(ObjRef, Vec<ObjRef>)>, RedactError> {
    let (holders, _, complete) = walk(doc, top, MAX_HOLDERS, pdfcraft_model::MAX_KIDS_ENTRIES);
    if !complete {
        return Err(CosError::Syntax { offset: 0, detail: "redaction could not walk the whole page tree".into() }.into());
    }
    Ok(holders)
}

/// [`resource_holders`] with explicit caps: the holders, how many `/Kids` entries were read, and
/// whether the whole tree was walked.
fn walk(doc: &Document, top: ObjRef, max_nodes: usize, max_entries: usize) -> (Vec<(ObjRef, Vec<ObjRef>)>, usize, bool) {
    let mut holders: Vec<(ObjRef, Vec<ObjRef>)> = Vec::new();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut seen_kids: HashSet<u32> = HashSet::new();
    let mut entries = 0usize;
    // (the /Kids array, the next entry to read, the holder its entries draw through, and whether
    // its entries count toward the cap: all but the one-entry list the walk starts from)
    let mut pending: Vec<(Arc<Object>, usize, Option<usize>, bool)> = vec![(Arc::new(Object::Array(vec![Object::Ref(top)])), 0, None, false)];
    while let Some((kids, next, holder, counted)) = pending.last_mut() {
        let Some(entry) = kids.as_array().and_then(|a| a.get(*next)).map(Object::as_ref) else {
            pending.pop();
            continue;
        };
        *next += 1;
        let mut holder = *holder;
        // Every entry counts, so a long run of nulls or numbers can't get past the cap.
        if *counted {
            entries = entries.saturating_add(1);
        }
        if entries > max_entries {
            return (holders, entries, false);
        }
        let Some(node) = entry else { continue };
        if seen.contains(&node.num) {
            continue;
        }
        // Checked for a node about to be visited, so a repeat after the last one isn't a cut.
        if seen.len() >= max_nodes {
            return (holders, entries, false);
        }
        seen.insert(node.num);
        let obj = doc.get(node);
        let Some(d) = obj.as_dict() else { continue };
        if d.contains(b"Resources") {
            holders.push((node, Vec::new()));
            holder = Some(holders.len() - 1);
        }
        let is_pages = d.name(b"Type") == Some(b"Pages") || (d.contains(b"Kids") && d.name(b"Type") != Some(b"Page"));
        if is_pages {
            let kids = match d.get(b"Kids") {
                // Read a shared array under the first node that lists it only.
                Some(Object::Ref(r)) if seen_kids.insert(r.num) => doc.get(*r),
                Some(array @ Object::Array(_)) => Arc::new(array.clone()),
                _ => continue,
            };
            pending.push((kids, 0, holder, true));
        } else if let Some(h) = holder.and_then(|i| holders.get_mut(i)) {
            h.1.push(node);
        }
    }
    (holders, entries, true)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_cos::{Document, ObjRef};
    use pdfcraft_testkit::Pdf;

    use super::{MAX_HOLDERS, resource_holders, walk};

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
        let mut holders = resource_holders(&doc, r(2)).unwrap();
        for (_, pages) in &mut holders {
            pages.sort_by_key(|p| p.num);
        }
        holders.sort_by_key(|(node, _)| node.num);
        assert_eq!(holders, [(r(2), vec![r(5), r(6)]), (r(4), vec![r(4)])]);
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
        let doc = Document::open(Arc::new(pdf.build())).unwrap();
        let (holders, entries, complete) = walk(&doc, r(2), MAX_HOLDERS, pdfcraft_model::MAX_KIDS_ENTRIES);
        assert!(complete);
        assert_eq!(holders, [(r(2), vec![r(3)])]);
        // The root's 2,000 kids and the shared array's 2,000 entries once, not 2,000 times over.
        assert!(entries <= 3 * n, "{entries} kids entries read");
    }

    /// `3 1 R` names object 3, as `3 0 R` does: one page, listed once.
    #[test]
    fn references_differing_only_in_generation_are_one_page() {
        let bytes = Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R 3 1 R 3 2 R] /Count 1 /Resources << >> >>")
            .obj("<< /Type /Page /Parent 2 0 R >>")
            .build();
        let doc = Document::open(Arc::new(bytes)).unwrap();
        let holders = resource_holders(&doc, r(2)).unwrap();
        assert_eq!(holders.len(), 1);
        assert_eq!(holders[0].1.len(), 1);
    }

    /// Past either cap the walk reports itself incomplete, every entry counted (references or
    /// not), so redaction can refuse rather than miss pages.
    #[test]
    fn a_walk_past_its_caps_is_incomplete() {
        let nulls = "null ".repeat(10);
        let bytes = Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj(format!("<< /Type /Pages /Kids [3 0 R {nulls}] /Count 1 /Resources << >> >>"))
            .obj("<< /Type /Page /Parent 2 0 R >>")
            .build();
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert!(walk(&doc, r(2), 100, 100).2, "within the caps");
        assert!(!walk(&doc, r(2), 100, 5).2, "past the entry cap, nulls included");
        assert!(!walk(&doc, r(2), 1, 100).2, "past the node cap");
        assert!(walk(&doc, r(2), 2, 100).2, "the tree's two nodes, then only nulls: every node visited");
        assert!(resource_holders(&doc, r(2)).is_ok());
    }

    /// A page listed under two nodes with resources draws through the first, as `pages` inherits.
    #[test]
    fn a_page_under_two_holders_draws_through_the_first() {
        let bytes = Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>") // 1
            .obj("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>") // 2
            .obj("<< /Type /Pages /Kids [5 0 R] /Count 1 /Resources << /Font << >> >> >>") // 3: first
            .obj("<< /Type /Pages /Kids [5 0 R] /Count 1 /Resources << >> >>") // 4: second
            .obj("<< /Type /Page /Parent 3 0 R >>") // 5
            .build();
        let doc = Document::open(Arc::new(bytes)).unwrap();
        let holders = resource_holders(&doc, r(2)).unwrap();
        let drawn_through: Vec<ObjRef> = holders.iter().filter(|(_, users)| users.contains(&r(5))).map(|(node, _)| *node).collect();
        assert_eq!(drawn_through, [r(3)]);
    }

    /// The cap counts the tree's own `/Kids` entries, not the start: a tree with exactly that many
    /// is walked whole.
    #[test]
    fn the_entry_cap_counts_only_the_tree_s_kids_entries() {
        let bytes = Pdf::new()
            .obj("<< /Type /Catalog /Pages 2 0 R >>")
            .obj("<< /Type /Pages /Kids [3 0 R 3 0 R] /Count 1 /Resources << >> >>")
            .obj("<< /Type /Page /Parent 2 0 R >>")
            .build();
        let doc = Document::open(Arc::new(bytes)).unwrap();
        let (_, entries, complete) = walk(&doc, r(2), 100, 2);
        assert_eq!(entries, 2);
        assert!(complete, "exactly at the cap");
        assert!(!walk(&doc, r(2), 100, 1).2, "one past the cap");
    }
}
