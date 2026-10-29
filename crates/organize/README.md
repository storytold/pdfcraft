# pdfcraft-organize

Page and document-structure edits over the copy-on-write PDF object graph.
Callers keep snapshots for undo; the existing engine and automation layers expose Extract Pages and Split.

## Extract and Split resource preservation

Extraction copies retained pages and prunes page/Form XObject aliases proven unused by those pages.
Annotation appearances, tiling patterns, soft-mask groups and Type 3 fonts can share those resource dictionaries.
Their paint programs are not interpreted by this pruning pass, so their reachable resource owners are retained conservatively.
All normal, rollover and pressed appearance states are protected, including inactive states and appearances on pages without page content.
Resource-less paint also protects the caller's resources; discovery caches include that caller context.

Graph protection is iterative and bounded to 10,000 resource objects/direct children per extraction, with reference cycles visited once per caller and paint role.
Exhausting this budget skips pruning for the entire output rather than guessing that an image is unused.
Independent page resource dictionaries still prune, and shared ordinary graphics-state catalogs are discovered once per owner.
Direct Resources dictionaries embedded in Form XObjects are pruned inside their containing Form; indirect Resources and XObject maps retain their separate owner paths.
A nested resource-less paint program can conservatively retain extra caller XObjects even when a nearer explicit resource dictionary would suffice.
This favors keeping artwork over reducing file size.

The regression suite constructs contributor-original fixtures in Rust source.
`cargo test -p pdfcraft-organize --test paint_resources` covers aliasing, inherited resources, malformed containers, cycles, the work cap, large shared catalogs, splitting and a Poppler pixel comparison when that external oracle is installed.
`extract_and_split_preserve_shared_stamp_pixels_after_save_and_reopen` exercises the public automation tools and rendered output through full save/reopen without changing the source file.
