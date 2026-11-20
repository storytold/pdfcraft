//! The document outline and its named destinations as room operations
//! (RFC §3.1.7, `docs/collab/STATUS.md` §5 p.3).
//!
//! Two classes of address meet here. An outline item is addressed by the object
//! reference it has in the document ([`OutlineKey`]), because its path moves
//! whenever an item above it is added or removed, while a page operation — an
//! insert, a split, a combine, a replacement — keeps the items and the page
//! objects they go to. A named destination is addressed by its name, the key
//! the file itself uses in `/Names /Dests`.
//!
//! Every operation resolves its address against the authoritative document
//! *before* it writes (the recalculation the room owes its replicas), so a
//! client that names an item somebody else moved, a cycle, a page this document
//! lacks and a name nobody defines are all refused with the document, the
//! version and the epoch untouched. The accepted operation is self-describing:
//! an add names the key it created, an update the title and destination it
//! replaced, a delete the whole subtree it removed, a move the place it came
//! from. An undo is therefore an ordinary operation — `outline.item.restore`,
//! `outline.item.update`, `outline.item.move` or the named destination's own —
//! that every replica and the journal replay the same way.
//!
//! A delete's copy is the subtree as [`OutlineNode`]s, bounded by
//! [`MAX_OUTLINE_COPY_NODES`] nodes and [`MAX_OUTLINE_COPY_BYTES`]; it carries
//! the item's key so a reader can tell what it was. Restoring it writes the same
//! tree back (nesting, view types, colours, styles and open state) at the place
//! it came from. An item that went to a *named* destination inside a copy comes
//! back going to that destination's page: the wire's copy is a tree of pages,
//! and the name's own entry is a separate operation that may have moved since.

use pdfcraft_collab_outline::{
    ItemLook, OutlineWireError, check_name, check_title, copy_bytes, dest_from_value, dest_to_value, look_from_value, look_to_value, node_count,
    subtree_depth, tree_from_value, tree_to_value,
};
use pdfcraft_collab_proto::{
    ErrorCode, MAX_NAMED_DESTINATIONS, MAX_OUTLINE_COPY_BYTES, MAX_OUTLINE_COPY_NODES, MAX_OUTLINE_DEPTH, MAX_OUTLINE_ITEMS, OutlineKey, Target,
};
use pdfcraft_engine::{BookmarkDest, Edit, OutlineNode};
use serde_json::{Value, json};

use crate::{AdapterError, PdfAdapter};

/// One item of the outline as the room sees it: its key and its path.
struct Item {
    key: OutlineKey,
    path: Vec<usize>,
}

impl PdfAdapter {
    /// Every outline item the document has, in listing order, with the key each
    /// is addressed by. The listing is the engine's own (32 levels), so an item
    /// deeper than the room's [`MAX_OUTLINE_DEPTH`] is not here and cannot be
    /// addressed: the room refuses to create one by an add or a move.
    fn outline_items(&self) -> Result<Vec<Item>, AdapterError> {
        let page = self
            .session()
            .bookmark_page(self.id, 0, usize::MAX)
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the document's outline cannot be read", false))?;
        Ok(page
            .rows
            .into_iter()
            .filter_map(|row| row.object.map(|(num, generation)| Item { key: OutlineKey { num, generation }, path: row.path }))
            .collect())
    }

    /// The path of the item `key`, or `invalid`: an item that was deleted, or one
    /// the room cannot see, is never guessed at by position.
    fn item_path(&self, key: OutlineKey) -> Result<Vec<usize>, AdapterError> {
        self.outline_items()?
            .into_iter()
            .find(|item| item.key == key)
            .map(|item| item.path)
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, format!("there is no outline item {}", key.as_string()), false))
    }

    /// Where an add or a move puts an item: the outline itself (`[]`) for `None`,
    /// else the parent item's path.
    fn parent_path(&self, key: Option<OutlineKey>) -> Result<Vec<usize>, AdapterError> {
        match key {
            None => Ok(Vec::new()),
            Some(key) => self.item_path(key),
        }
    }

    /// The key of the item at `path`, for the undo image of a move.
    fn key_at(&self, path: &[usize]) -> Result<OutlineKey, AdapterError> {
        self.outline_items()?
            .into_iter()
            .find(|item| item.path == path)
            .map(|item| item.key)
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the outline item moved before the operation", false))
    }

    /// The whole subtree under `path`, read for a delete's copy. Refused as
    /// `limit` when it is more than one operation can carry, before anything
    /// changes; `truncated` (deeper than the listing) is refused too, because a
    /// copy that lost its deepest nodes would not put the item back whole.
    fn outline_subtree(&self, path: &[usize]) -> Result<Vec<OutlineNode>, AdapterError> {
        let tree = self
            .session()
            .outline_tree(self.id, path)
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the document's outline cannot be read", false))?
            .map_err(|e| AdapterError::new(ErrorCode::Invalid, e.to_string(), false))?;
        if tree.truncated {
            return Err(AdapterError::new(
                ErrorCode::Limit,
                format!("the subtree nests deeper than the {MAX_OUTLINE_DEPTH} levels the room carries"),
                false,
            ));
        }
        let nodes = node_count(&tree.nodes);
        if nodes > MAX_OUTLINE_COPY_NODES {
            return Err(AdapterError::new(
                ErrorCode::Limit,
                format!("the subtree has {nodes} items; one copy carries at most {MAX_OUTLINE_COPY_NODES}"),
                false,
            ));
        }
        if copy_bytes(&tree.nodes) > MAX_OUTLINE_COPY_BYTES {
            return Err(AdapterError::new(ErrorCode::Limit, "the subtree is too large to carry in one operation", false));
        }
        Ok(tree.nodes)
    }

    /// The item's own node as the document has it: title, page and the view the
    /// file holds, so an update's undo puts back exactly what it replaced.
    fn node_at(&self, path: &[usize]) -> Result<OutlineNode, AdapterError> {
        let mut nodes = self.outline_subtree(path)?;
        nodes
            .first_mut()
            .map(|node| {
                // Only the node itself: its children are not part of "what the
                // update replaced".
                let mut node = node.clone();
                node.children = Vec::new();
                node
            })
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "there is no outline item there", false))
    }

    /// The item's colour at the precision the file keeps it. The listing rounds
    /// each component to an `f32` for the interface; an undo has to write back
    /// the number the file held.
    fn item_color(&self, path: &[usize]) -> Option<[f64; 3]> {
        self.document().and_then(|document| document.bookmark_color(path))
    }

    /// How many children the item at `parent` has (`[]` = the top level).
    fn children_count(&self, items: &[Item], parent: &[usize]) -> Result<usize, AdapterError> {
        Ok(items.iter().filter(|item| item.path.len() == parent.len().saturating_add(1) && item.path.starts_with(parent)).count())
    }

    /// The key an outline operation's target names, and whether the target names
    /// the outline itself.
    fn outline_target(target: &Target) -> Result<Option<OutlineKey>, AdapterError> {
        match target {
            Target::OutlineItem { key } => Ok(*key),
            _ => Err(AdapterError::new(ErrorCode::Invalid, "an outline operation needs an outline target", false)),
        }
    }

    /// The item an update, a delete or a move addresses: the target's key, which
    /// must be an item rather than the outline itself.
    fn outline_item_key(target: &Target) -> Result<OutlineKey, AdapterError> {
        Self::outline_target(target)?.ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the operation names the outline, not an item", false))
    }

    /// Read a destination out of an operation and check it against this document:
    /// the page must exist, the name must resolve. Everything is checked before
    /// the first write.
    fn read_dest(&self, value: &Value) -> Result<BookmarkDest, AdapterError> {
        let dest = dest_from_value(value).map_err(map_outline)?;
        self.validate_dest(&dest)?;
        Ok(dest)
    }

    fn validate_dest(&self, dest: &BookmarkDest) -> Result<(), AdapterError> {
        let pages = self.document().map_or(0, |document| document.info.pages.len());
        match dest {
            BookmarkDest::Page { page, .. } | BookmarkDest::View { page, .. } => {
                if *page >= pages {
                    return Err(AdapterError::new(ErrorCode::Invalid, format!("page {} does not exist", page.saturating_add(1)), false));
                }
                Ok(())
            }
            BookmarkDest::Named { name } => {
                if !self.named_destination_exists(name) {
                    return Err(AdapterError::new(ErrorCode::Invalid, format!("this document has no named destination {name:?}"), false));
                }
                Ok(())
            }
            BookmarkDest::Nowhere => Ok(()),
        }
    }

    fn named_destination_exists(&self, name: &str) -> bool {
        self.document().is_some_and(|document| document.named_destinations().iter().any(|destination| destination.name == name))
    }

    /// Where `name` goes, for the undo of an update or a delete. A name this
    /// document does not have is `conflict` (someone removed it); one that opens
    /// no page — a name of a name — is `unsupported`, because the room keeps a
    /// page for every name and could not put that back.
    fn named_destination_before(&self, name: &str) -> Result<Value, AdapterError> {
        let document = self.document().ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the document is gone", false))?;
        let destination = document
            .named_destinations()
            .into_iter()
            .find(|destination| destination.name == name)
            .ok_or_else(|| AdapterError::new(ErrorCode::Conflict, "the named destination was deleted", false))?;
        let page = destination
            .page
            .ok_or_else(|| AdapterError::new(ErrorCode::Unsupported, "the named destination opens no page, so the room cannot keep it", false))?;
        Ok(dest_value(page, destination.view.as_ref()))
    }

    /// `outline.item.add` (RFC §3.1.7): a title and a destination as child
    /// `index` of the parent the target names (`null` = the top level), with the
    /// look (`color`, `italic`, `bold`) the operation names. A new item has no
    /// children, so an `open` state has nothing to change and is refused.
    pub(crate) fn add_outline_item(&mut self, target: &Target, payload: &mut Value) -> Result<Value, AdapterError> {
        let parent = self.parent_path(Self::outline_target(target)?)?;
        let title = payload.get("title").and_then(Value::as_str).unwrap_or_default().to_owned();
        check_title(&title).map_err(map_outline)?;
        let dest =
            self.read_dest(payload.get("dest").ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the item names no destination", false))?)?;
        let look = look_from_value(payload).map_err(map_outline)?;
        if look.open.is_some() {
            return Err(AdapterError::new(ErrorCode::Invalid, "a new bookmark has no children, so it has no open state to set", false));
        }
        if parent.len().saturating_add(1) > MAX_OUTLINE_DEPTH {
            return Err(AdapterError::new(ErrorCode::Limit, format!("bookmarks may nest at most {MAX_OUTLINE_DEPTH} levels deep"), false));
        }
        let items = self.outline_items()?;
        if items.len() >= MAX_OUTLINE_ITEMS {
            return Err(AdapterError::new(ErrorCode::Limit, format!("a room holds at most {MAX_OUTLINE_ITEMS} bookmarks"), false));
        }
        let index = match payload.get("index").and_then(Value::as_u64) {
            Some(index) => usize::try_from(index).map_err(|_| AdapterError::new(ErrorCode::Invalid, "the position is too large", false))?,
            None => self.children_count(&items, &parent)?,
        };
        self.apply_edit(Edit::AddBookmarkWith { parent: parent.clone(), index, title: title.clone(), dest: dest.clone() })?;
        // The engine clamps a position past the end, so the new item is found by
        // what it is rather than by where the operation asked for it.
        let after = self.outline_items()?;
        let added = after
            .iter()
            .find(|item| !items.iter().any(|before| before.key == item.key))
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the bookmark was added without an object", false))?;
        let key = added.key;
        let path = added.path.clone();
        // The look is written after the item exists, in the same accepted
        // operation. Every value was checked above, so this second step cannot
        // be the one that refuses: the room never accepts half a bookmark.
        if !look.is_empty() {
            self.apply_edit(Edit::UpdateBookmark {
                path: path.clone(),
                title: None,
                dest: None,
                color: look.color,
                bold: look.bold,
                italic: look.italic,
                open: None,
            })?;
        }
        self.epoch = self.epoch.saturating_add(1);
        payload["parent"] = json!(parent);
        payload["path"] = json!(path);
        payload["index"] = json!(path.last().copied().unwrap_or(0));
        payload["title"] = json!(title);
        payload["dest"] = dest_to_value(&dest);
        apply_look_payload(payload, &look);
        payload["key"] = json!(key);
        Ok(json!({ "key": key, "path": path }))
    }

    /// `outline.item.update`: a new title, destination and/or look for an
    /// existing item, as one step. The accepted operation carries the whole node
    /// it replaced (`before`), so the undo is the same action with those values.
    pub(crate) fn update_outline_item(&mut self, target: &Target, payload: &mut Value) -> Result<Value, AdapterError> {
        let key = Self::outline_item_key(target)?;
        let path = self.item_path(key)?;
        let title = payload.get("title").and_then(Value::as_str).map(str::to_owned);
        let dest = match payload.get("dest") {
            Some(value) => Some(self.read_dest(value)?),
            None => None,
        };
        let look = look_from_value(payload).map_err(map_outline)?;
        if title.is_none() && dest.is_none() && look.is_empty() {
            return Err(AdapterError::new(ErrorCode::Invalid, "the update names no title, destination or look", false));
        }
        if let Some(title) = &title {
            check_title(title).map_err(map_outline)?;
        }
        // A bookmark with no children has no `/Count` to open or close: the
        // request says nothing about a state the file could keep.
        if look.open.is_some() {
            let items = self.outline_items()?;
            if self.children_count(&items, &path)? == 0 {
                return Err(AdapterError::new(ErrorCode::Invalid, "this bookmark has no children, so it has no open state to set", false));
            }
        }
        let before = self.node_at(&path)?;
        let before_color = self.item_color(&path);
        self.apply_edit(Edit::UpdateBookmark {
            path: path.clone(),
            title: title.clone(),
            dest: dest.clone(),
            color: look.color,
            bold: look.bold,
            italic: look.italic,
            open: look.open,
        })?;
        self.epoch = self.epoch.saturating_add(1);
        payload["path"] = json!(path);
        if let Some(title) = &title {
            payload["title"] = json!(title);
        }
        if let Some(dest) = &dest {
            payload["dest"] = dest_to_value(dest);
        }
        apply_look_payload(payload, &look);
        payload["before"] = node_before_value(&before, before_color);
        payload["key"] = json!(key);
        Ok(json!({ "key": key, "path": path }))
    }

    /// `outline.item.delete`: the item and everything under it, with the copy the
    /// undo puts back.
    pub(crate) fn delete_outline_item(&mut self, target: &Target, payload: &mut Value) -> Result<Value, AdapterError> {
        let key = Self::outline_item_key(target)?;
        let path = self.item_path(key)?;
        let nodes = self.outline_subtree(&path)?;
        let subtree = tree_to_value(&nodes);
        let parent = path[..path.len().saturating_sub(1)].to_vec();
        let parent_key = if parent.is_empty() { None } else { Some(self.key_at(&parent)?) };
        let index = path.last().copied().unwrap_or(0);
        self.apply_edit(Edit::DeleteBookmark { path: path.clone() })?;
        self.epoch = self.epoch.saturating_add(1);
        payload["path"] = json!(path);
        payload["subtree"] = subtree.clone();
        payload["parent"] = json!(parent);
        payload["parent_key"] = json!(parent_key);
        payload["index"] = json!(index);
        payload["key"] = json!(key);
        Ok(json!({ "key": key, "path": path, "subtree": subtree }))
    }

    /// `outline.item.restore`: put a deleted subtree back where it came from, as
    /// the copy the delete carried describes it. The items come back with the
    /// same nesting, destinations, colours, styles and open state, and with new
    /// object references: the result names the key the first one got.
    pub(crate) fn restore_outline_item(&mut self, target: &Target, payload: &mut Value) -> Result<Value, AdapterError> {
        let parent = self.parent_path(Self::outline_target(target)?)?;
        let nodes =
            tree_from_value(payload.get("subtree").ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the restore names no subtree", false))?)
                .map_err(map_outline)?;
        if nodes.is_empty() {
            return Err(AdapterError::new(ErrorCode::Invalid, "the restore carries no bookmark", false));
        }
        // The copy travels in the accepted operation, which is broadcast and
        // journaled: it has to fit the frame on the way back out, not only on the
        // way in.
        if copy_bytes(&nodes) > MAX_OUTLINE_COPY_BYTES {
            return Err(AdapterError::new(ErrorCode::Limit, "the subtree is too large to carry in one operation", false));
        }
        if parent.len().saturating_add(subtree_depth(&nodes)) > MAX_OUTLINE_DEPTH {
            return Err(AdapterError::new(ErrorCode::Limit, format!("bookmarks may nest at most {MAX_OUTLINE_DEPTH} levels deep"), false));
        }
        let items = self.outline_items()?;
        if items.len().saturating_add(node_count(&nodes)) > MAX_OUTLINE_ITEMS {
            return Err(AdapterError::new(ErrorCode::Limit, format!("a room holds at most {MAX_OUTLINE_ITEMS} bookmarks"), false));
        }
        let index = match payload.get("index").and_then(Value::as_u64) {
            Some(index) => usize::try_from(index).map_err(|_| AdapterError::new(ErrorCode::Invalid, "the position is too large", false))?,
            None => self.children_count(&items, &parent)?,
        };
        self.apply_edit(Edit::ImportBookmarks { parent: parent.clone(), index, bookmarks: nodes.clone(), replace: false })?;
        self.epoch = self.epoch.saturating_add(1);
        let after = self.outline_items()?;
        let restored: Vec<OutlineKey> = after.iter().filter(|item| !items.iter().any(|before| before.key == item.key)).map(|item| item.key).collect();
        let key = *restored.first().ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the bookmark was restored without an object", false))?;
        let path = after
            .iter()
            .find(|item| item.key == key)
            .map(|item| item.path.clone())
            .ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the restored bookmark cannot be found", false))?;
        payload["parent"] = json!(parent);
        payload["path"] = json!(path);
        payload["index"] = json!(path.last().copied().unwrap_or(index));
        payload["key"] = json!(key);
        Ok(json!({ "key": key, "path": path, "keys": restored }))
    }

    /// `outline.replace`: the whole outline becomes `items`, in one structural
    /// operation. The target is the outline itself (`key: null`), and `epoch` is
    /// the compare-and-set: a document whose pages moved since the client built
    /// the tree is `stale` and retryable before anything is read. The accepted
    /// operation carries the tree it wrote and the tree it replaced (`before`),
    /// so its undo is one more replace over `before` and its redo repeats the
    /// operation. Everything is checked before the first write: the wire bounds
    /// (nodes, depth, JSON bytes, titles, views, colours), every page against
    /// this document, and every destination a node names by name.
    pub(crate) fn replace_outline(&mut self, target: &Target, payload: &mut Value) -> Result<Value, AdapterError> {
        if !matches!(target, Target::OutlineItem { key: None }) {
            return Err(invalid("an outline replacement is addressed to the whole outline, not to an item"));
        }
        if let Some(epoch) = payload.get("epoch").and_then(Value::as_u64)
            && epoch != self.epoch
        {
            return Err(AdapterError::new(ErrorCode::Stale, "the pages changed since; re-address the outline", true));
        }
        let items = payload.get("items").ok_or_else(|| invalid("the operation names no outline"))?;
        let nodes = self.outline_replacement(items)?;
        // The accepted operation carries the tree, so it has to fit the frame on
        // the way back out, not only on the way in.
        if copy_bytes(&nodes) > MAX_OUTLINE_COPY_BYTES {
            return Err(limit("the outline is too large to carry in one operation"));
        }
        let before_nodes = self.whole_outline()?;
        if nodes.is_empty() && before_nodes.is_empty() {
            return Err(invalid("the outline is already empty"));
        }
        let items = tree_to_value(&nodes);
        let before = tree_to_value(&before_nodes);
        let count = node_count(&nodes);
        payload["action"] = json!("outline.replace");
        payload["items"] = items.clone();
        payload["before"] = before.clone();
        payload["epoch"] = json!(self.epoch);
        payload["count"] = json!(count);
        self.write_whole_outline(&nodes)?;
        self.epoch = self.epoch.saturating_add(1);
        Ok(json!({ "items": items, "count": count }))
    }

    /// The document's outline as a copy, for a replacement's `before` image: the
    /// accepted operation carries it whole, so an outline over one operation's
    /// budget is refused before anything changes rather than replaced without an
    /// undo.
    fn whole_outline(&self) -> Result<Vec<OutlineNode>, AdapterError> {
        let tree = self
            .session()
            .outline_tree(self.id, &[])
            .ok_or_else(|| invalid("the document's outline cannot be read"))?
            .map_err(|error| invalid(error.to_string()))?;
        if tree.truncated {
            return Err(limit(format!("the outline nests deeper than the {MAX_OUTLINE_DEPTH} levels the room carries")));
        }
        let nodes = node_count(&tree.nodes);
        if nodes > MAX_OUTLINE_COPY_NODES {
            return Err(limit(format!("the outline has {nodes} bookmarks; one operation carries at most {MAX_OUTLINE_COPY_NODES}")));
        }
        if copy_bytes(&tree.nodes) > MAX_OUTLINE_COPY_BYTES {
            return Err(limit("the outline is too large to keep whole in one operation"));
        }
        Ok(tree.nodes)
    }

    /// The items of an `outline.replace`, checked against this document before
    /// the first write. A node may name its destination in the copy wire's own
    /// `page`/`view` fields or as the full `dest` form of an outline operation
    /// (a name included): a name this document does not define, one that opens no
    /// page, a page past the last and a view that is not Table 149 are all
    /// refused here.
    fn outline_replacement(&self, value: &Value) -> Result<Vec<OutlineNode>, AdapterError> {
        let list = value.as_array().ok_or_else(|| invalid("the outline is not a list"))?;
        let normalized = Value::Array(list.iter().map(|item| self.outline_replacement_node(item)).collect::<Result<Vec<_>, _>>()?);
        let nodes = tree_from_value(&normalized).map_err(map_outline)?;
        let pages = self.document().map_or(0, |document| document.info.pages.len());
        check_outline_pages(&nodes, pages)?;
        Ok(nodes)
    }

    fn outline_replacement_node(&self, item: &Value) -> Result<Value, AdapterError> {
        let Value::Object(mut object) = item.clone() else {
            // Not a node at all: `tree_from_value` refuses it with the shape it
            // expected, before anything is written.
            return Ok(item.clone());
        };
        if let Some(value) = object.get("dest").filter(|value| !value.is_null()) {
            let dest = self.read_dest(value)?;
            let (page, view) = match dest {
                BookmarkDest::Page { page, left, top } => (Some(page), Some((b"XYZ".to_vec(), vec![left, top, None]))),
                BookmarkDest::View { page, kind, values } => (Some(page), Some((kind, values))),
                BookmarkDest::Named { name } => {
                    let destination = self
                        .document()
                        .and_then(|document| document.named_destinations().into_iter().find(|destination| destination.name == name))
                        .ok_or_else(|| invalid(format!("this document has no named destination {name:?}")))?;
                    let page = destination.page.ok_or_else(|| {
                        AdapterError::new(ErrorCode::Unsupported, "the named destination opens no page, so the room cannot keep it", false)
                    })?;
                    (Some(page), destination.view)
                }
                BookmarkDest::Nowhere => (None, None),
            };
            object.insert("page".into(), page.map_or(Value::Null, |page| json!(page)));
            object.insert("view".into(), view.map_or(Value::Null, |(kind, values)| outline_view(&kind, &values)));
        }
        if let Some(Value::Array(children)) = object.get("children").cloned() {
            let normalized = children.iter().map(|child| self.outline_replacement_node(child)).collect::<Result<Vec<_>, _>>()?;
            object.insert("children".into(), Value::Array(normalized));
        }
        Ok(Value::Object(object))
    }

    /// Write the whole outline as one edit: the engine's import with `replace`
    /// when there is a tree to write, and a batch of top-level deletions when the
    /// operation empties the outline (the engine's import refuses an empty tree,
    /// and "there was no outline" is exactly "nothing is left").
    fn write_whole_outline(&mut self, nodes: &[OutlineNode]) -> Result<(), AdapterError> {
        if !nodes.is_empty() {
            return self.apply_edit(Edit::ImportBookmarks { parent: Vec::new(), index: 0, bookmarks: nodes.to_vec(), replace: true });
        }
        let top = self.outline_items()?.into_iter().filter(|item| item.path.len() == 1).count();
        let edits = (0..top).map(|_| Edit::DeleteBookmark { path: vec![0] }).collect();
        self.apply_edit(Edit::Batch { label: "Replace bookmarks".into(), edits })
    }

    /// `outline.item.move`: the item (with everything under it) as child `index`
    /// of another parent, and where it came from. A move into itself or into its
    /// own subtree is refused before anything changes, as is one that would nest
    /// deeper than the room carries.
    pub(crate) fn move_outline_item(&mut self, target: &Target, payload: &mut Value) -> Result<Value, AdapterError> {
        let key = Self::outline_item_key(target)?;
        let from = self.item_path(key)?;
        let parent_key = match payload.get("parent") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                serde_json::from_value::<OutlineKey>(value.clone())
                    .map_err(|_| AdapterError::new(ErrorCode::Invalid, "the parent is not an outline key", false))?,
            ),
        };
        let to_parent = self.parent_path(parent_key)?;
        if to_parent.starts_with(&from) {
            return Err(AdapterError::new(ErrorCode::Invalid, "a bookmark cannot be moved inside itself", false));
        }
        let nodes = self.outline_subtree(&from)?;
        if to_parent.len().saturating_add(subtree_depth(&nodes)) > MAX_OUTLINE_DEPTH {
            return Err(AdapterError::new(ErrorCode::Limit, format!("bookmarks may nest at most {MAX_OUTLINE_DEPTH} levels deep"), false));
        }
        let items = self.outline_items()?;
        let index = match payload.get("index").and_then(Value::as_u64) {
            Some(index) => usize::try_from(index).map_err(|_| AdapterError::new(ErrorCode::Invalid, "the position is too large", false))?,
            None => self.children_count(&items, &to_parent)?,
        };
        let old_parent = if from.len() > 1 { Some(self.key_at(&from[..from.len() - 1])?) } else { None };
        let old_index = from.last().copied().unwrap_or(0);
        self.apply_edit(Edit::MoveBookmark { from: from.clone(), to_parent: to_parent.clone(), index })?;
        self.epoch = self.epoch.saturating_add(1);
        let path = self.item_path(key)?;
        payload["to_parent"] = json!(to_parent);
        payload["index"] = json!(index);
        payload["from"] = json!(from);
        payload["before"] = json!({ "parent": old_parent, "index": old_index, "path": from });
        payload["key"] = json!(key);
        Ok(json!({ "key": key, "path": path, "from": from }))
    }

    /// `dest.add`: a name this document does not define yet, pointing at a page.
    pub(crate) fn add_named_destination(&mut self, target: &Target, payload: &mut Value) -> Result<Value, AdapterError> {
        let name = destination_name(target)?;
        let (dest, value) = self.dest_place(payload)?;
        let page = dest_page(&dest);
        if self.named_destination_exists(&name) {
            return Err(AdapterError::new(ErrorCode::Conflict, "a named destination of this name already exists", false));
        }
        self.check_dest_count()?;
        self.apply_edit(Edit::SetNamedDestination { name: name.clone(), dest: dest.clone() })?;
        self.epoch = self.epoch.saturating_add(1);
        payload["name"] = json!(name);
        payload["dest"] = value;
        Ok(json!({ "name": name, "page": page }))
    }

    /// `dest.update`: point an existing name somewhere else. The accepted
    /// operation carries the destination it replaced, so the undo is the same
    /// action with that one.
    pub(crate) fn update_named_destination(&mut self, target: &Target, payload: &mut Value) -> Result<Value, AdapterError> {
        let name = destination_name(target)?;
        let before = self.named_destination_before(&name)?;
        let (dest, value) = self.dest_place(payload)?;
        let page = dest_page(&dest);
        self.apply_edit(Edit::SetNamedDestination { name: name.clone(), dest })?;
        self.epoch = self.epoch.saturating_add(1);
        payload["name"] = json!(name);
        payload["dest"] = value.clone();
        payload["before"] = before;
        Ok(json!({ "name": name, "page": page }))
    }

    /// `dest.delete`: remove an existing name, keeping where it went so the undo
    /// can put it back.
    pub(crate) fn delete_named_destination(&mut self, target: &Target, payload: &mut Value) -> Result<Value, AdapterError> {
        let name = destination_name(target)?;
        let before = self.named_destination_before(&name)?;
        self.apply_edit(Edit::DeleteNamedDestination { name: name.clone() })?;
        self.epoch = self.epoch.saturating_add(1);
        payload["name"] = json!(name);
        payload["before"] = before.clone();
        Ok(json!({ "name": name, "before": before }))
    }

    /// The page and point a `dest.*` operation names, checked against the
    /// document before the first write: the page must exist, and the coordinates
    /// must be finite. A named destination points at a page here — a name that
    /// opens another name is refused, because the room keeps one entry per name.
    fn dest_place(&self, payload: &Value) -> Result<(BookmarkDest, Value), AdapterError> {
        let value = payload.get("dest").ok_or_else(|| AdapterError::new(ErrorCode::Invalid, "the operation names no destination", false))?;
        let dest = self.read_dest(value)?;
        if !matches!(dest, BookmarkDest::Page { .. } | BookmarkDest::View { .. }) {
            return Err(AdapterError::new(
                ErrorCode::Invalid,
                "a named destination must name a page: a name of a name, or a destination that goes nowhere, cannot be kept",
                false,
            ));
        }
        if let Some(epoch) = payload.get("epoch").and_then(Value::as_u64)
            && epoch != self.epoch
        {
            return Err(AdapterError::new(ErrorCode::Stale, "the pages changed since; re-address the page", true));
        }
        Ok((dest.clone(), dest_to_value(&dest)))
    }

    fn check_dest_count(&self) -> Result<(), AdapterError> {
        let count = self.document().map_or(0, |document| document.named_destinations().len());
        if count >= MAX_NAMED_DESTINATIONS {
            return Err(AdapterError::new(ErrorCode::Limit, format!("a room holds at most {MAX_NAMED_DESTINATIONS} named destinations"), false));
        }
        Ok(())
    }
}

fn invalid(message: impl Into<String>) -> AdapterError {
    AdapterError::new(ErrorCode::Invalid, message.into(), false)
}

fn limit(message: impl Into<String>) -> AdapterError {
    AdapterError::new(ErrorCode::Limit, message.into(), false)
}

/// Every node's page must be one this document has: a copy may keep a bookmark
/// whose page the *target* lacks, but a room's own outline is written against the
/// document it shares, and a page past the last is a client's mistake, not a
/// bookmark that goes nowhere.
fn check_outline_pages(nodes: &[OutlineNode], pages: usize) -> Result<(), AdapterError> {
    for node in nodes {
        if let Some(page) = node.page
            && page >= pages
        {
            return Err(invalid(format!("page {} does not exist: the document has {pages}", page.saturating_add(1))));
        }
        check_outline_pages(&node.children, pages)?;
    }
    Ok(())
}

/// A view as the copy wire writes it (`tree_to_value`'s shape), for a node whose
/// destination arrived in the full `dest` form.
fn outline_view(kind: &[u8], values: &[Option<f64>]) -> Value {
    json!({
        "kind": String::from_utf8_lossy(kind).into_owned(),
        "values": values.iter().map(|value| value.map_or(Value::Null, |value| json!(value))).collect::<Vec<_>>(),
    })
}

/// The page a destination names.
fn dest_page(dest: &BookmarkDest) -> usize {
    match dest {
        BookmarkDest::Page { page, .. } | BookmarkDest::View { page, .. } => *page,
        BookmarkDest::Named { .. } | BookmarkDest::Nowhere => 0,
    }
}

/// The name a `dest.*` operation addresses: the target's, checked against the
/// wire's bound before anything is read from the document.
fn destination_name(target: &Target) -> Result<String, AdapterError> {
    let Target::NamedDest { name } = target else {
        return Err(AdapterError::new(ErrorCode::Invalid, "a named destination operation needs a destination target", false));
    };
    check_name(name).map_err(map_outline)?;
    Ok(name.trim().to_owned())
}

/// A destination as the wire describes it: a page and a point for `/XYZ`, the
/// view type and its values for any other, so an undo can put back exactly what
/// a file held.
fn dest_value(page: usize, view: Option<&(Vec<u8>, Vec<Option<f64>>)>) -> Value {
    match view {
        Some((kind, values)) if kind.as_slice() == b"XYZ" => {
            json!({"kind": "page", "page": page, "left": values.first().copied().flatten(), "top": values.get(1).copied().flatten()})
        }
        Some((kind, values)) => json!({
            "kind": "view",
            "page": page,
            "view": {
                "kind": String::from_utf8_lossy(kind).into_owned(),
                "values": values.iter().map(|value| value.map_or(Value::Null, |v| json!(v))).collect::<Vec<_>>(),
            },
        }),
        None => json!({"kind": "page", "page": page, "left": null, "top": null}),
    }
}

/// The destination an item has, as the wire describes it: the page and view the
/// file holds, or "nowhere" for a container.
fn node_dest_value(node: &OutlineNode) -> Value {
    match (node.page, node.view.as_ref()) {
        (Some(page), view) => dest_value(page, view),
        _ => json!({"kind": "nowhere"}),
    }
}

/// The whole node an update replaced, for its undo: its title, destination and
/// look, so `before` puts back every field the update could have changed.
fn node_before_value(node: &OutlineNode, color: Option<[f64; 3]>) -> Value {
    let look = ItemLook { color: Some(color), bold: Some(node.bold), italic: Some(node.italic), open: Some(node.open) };
    let mut value = json!({"title": node.title, "dest": node_dest_value(node)});
    if let Value::Object(look) = look_to_value(&look) {
        for (field, field_value) in look {
            value[field] = field_value;
        }
    }
    value
}

/// Write the look an operation named into its accepted payload, so every replica
/// and the journal see the same fields the client sent.
fn apply_look_payload(payload: &mut Value, look: &ItemLook) {
    if let Value::Object(look) = look_to_value(look) {
        for (field, field_value) in look {
            payload[field] = field_value;
        }
    }
}

fn map_outline(error: OutlineWireError) -> AdapterError {
    match error {
        OutlineWireError::Missing(_) | OutlineWireError::Malformed(_) | OutlineWireError::Invalid(_) => {
            AdapterError::new(ErrorCode::Invalid, error.to_string(), false)
        }
        OutlineWireError::TooLong { .. } | OutlineWireError::TooMany { .. } => AdapterError::new(ErrorCode::Limit, error.to_string(), false),
    }
}
