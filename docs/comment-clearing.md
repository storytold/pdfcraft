# Clearing comments

The Comments panel's More options menu offers **Delete comments on this page…** and **Delete all comments…**.
Both open a confirmation before changing the document.
For custom page labels, the page confirmation also shows the page's position in the document.
The scope includes comments hidden by search, filters, collapsed groups or PDF visibility flags, along with replies, review status entries and associated pop-ups.
Cancelling leaves the document and comment drafts alone.
Clearing stored comments also preserves uncommitted new-comment composers, replacement-text quads and unfinished drawings, since they do not reference a removed annotation.
Existing annotation-index selections, edit drafts and requests in the affected scope are discarded after success because retained entries' indices can shift too.
Independent new-link geometry, form drafts and page content are preserved.
The confirmation owns input while it is open, so background Delete or Escape cannot edit the document or discard an editor's draft.
If the document or its edit generation changes before confirmation, reopen the action for the current document.

The headless equivalent uses the same engine edit:

```json
{"tool":"comment_clear","args":{"doc":1,"scope":"page","page":2}}
{"tool":"comment_clear","args":{"doc":1,"scope":"all"}}
```

Page numbers are positive and one-based in automation, and zero-based in `Edit::ClearComments { page: Some(index) }`.
`page: None` in the engine means the whole document.
The tool requires an explicit scope and rejects a page with `scope: "all"`, missing or invalid pages, unknown scopes and unknown arguments.
It returns the usual document summary plus `scope`, `page` when applicable, and `changed`.
`changed: false` means no membership was removed; dirty state, edit generation, Undo and Redo remain unchanged.
Counts in the visible comment list are thread roots, rather than the number of removed annotation entries.

## Scope and preservation

The operation follows PdfCraft's existing Comments subtype taxonomy.
It includes sticky notes, markups, drawings, stamps, redaction marks, file attachment comments and other nonempty named comment subtypes, including unknown named subtypes.
It preserves links, form widgets, pop-ups without a selected comment owner, multimedia annotations and production annotations such as printer marks and watermarks.
Entries without a valid named subtype remain unclassified and are preserved.
Only a typed pop-up associated with a selected comment is removed; misleading relationships on a link or widget never broaden the scope.

All selected entries are checked before any change.
A locked target, annotation permission denial, invalid scope or unreadable membership refuses the operation without a partial clear.
Reference resolution, page traversal and annotation work are bounded; inputs beyond these limits return an error rather than clearing a prefix.
The planner permits at most 64 reference hops, one million scoped annotation memberships and four million combined work units; the combined limit can be reached before either individual maximum.
Catalog, page-tree node and Kids-array references must resolve in a single hop, matching the current page model.
Multi-hop aliases in those structures are refused because they can change page numbering; annotation membership, subtype, flags and pop-up aliases retain the separate 64-hop bound.
The action is available even when filters hide every comment, and filters never restrict removal.

A successful change creates one Undo step for the requested scope.
One Undo restores the memberships; Redo removes them again.
Page content, resources, form fields, retained annotation dictionaries and unknown keys are preserved.
When pages share an indirect annotation array, clearing one page replaces only that page's membership and leaves the old shared array intact.
In-memory clearing does not write the source file; an explicit normal save remains incremental where supported.

This is comment membership removal, not secure redaction or document sanitization.
It does not erase the removed objects or earlier revisions, flatten appearances, or remove arbitrary selected subsets.
