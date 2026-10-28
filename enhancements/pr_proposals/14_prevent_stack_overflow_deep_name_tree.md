# PR: Prevent Stack Overflow When Deleting Pages with Deeply Chained Name Trees (#3309)

## Summary
- **Issue Reference:** Resolves #3309
- **Crate:** `crates/organize`
- **Scope:** Name tree pruning in `prune_tree_slot` (`crates/organize/src/lib.rs`).

## Root Cause
When a page is deleted, `drop_named_destinations` traverses the `/Names /Dests` name tree to remove entries pointing to deleted pages.
Previously, `prune_tree_ref` and `prune_tree_node` traversed the tree recursively. When encountering a PDF whose name tree consists of a deep linear chain of indirect `/Kids` nodes (e.g., > 2,048 nodes), recursion consumed the thread's call stack and caused an unrecoverable process crash via stack overflow (`SIGABRT`).

## Fix
- Replaced the recursive traversal in `prune_tree_slot` with an iterative heap-allocated worklist (`Vec<ObjRef>`).
- Utilized `HashSet<ObjRef>` for `O(1)` cycle prevention and fast deduplication.
- Bounded maximum tree node evaluations to 100,000 steps to protect against malicious exponential or cyclical structures (complying with `AGENTS.md` §4 "Treat every input-derived number as hostile / Bound recursion").
- Added synthetic regression test `deleting_a_page_prunes_deep_named_destination_trees` in `crates/organize/src/tests.rs` with a 2,500-node chain.
