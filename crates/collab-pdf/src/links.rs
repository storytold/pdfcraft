//! Links as room operations (RFC §3.1.9, `docs/collab/STATUS.md` §5 p.3).
//!
//! A link is an annotation of `/Subtype /Link` (ISO 32000-2 §12.5.6.5). Its unit in
//! the room is one page's whole list: `page.link.set` makes the page's links exactly
//! the list it names, and `page.link.clear` removes one of them. A list, not a link
//! index, is the address because the undo image has to be the links the page had: a
//! delete that travelled as "the fifth link" could not put back what a page operation
//! in between moved, and the client that built the operation would name a link nobody
//! else has.
//!
//! Everything a write needs is read and checked before the first one: every link's
//! area (a rectangle, or the quads of a text link), its target (a page with a view, a
//! named destination, a page of another file, or a web address) and its look. A page
//! that already holds a link this model cannot carry is refused with `unsupported`
//! rather than rewritten blind, since a rewrite would drop a destination no undo
//! brings back. The accepted operation carries both lists (`links` and `before`), so
//! every replica and the journal write the same page and the undo is a plain set.

use pdfcraft_collab_link::{LinkWireError, bodies_from_value, bodies_to_value};
use pdfcraft_collab_proto::{ErrorCode, MAX_LINKS, PageRef, Target};
use pdfcraft_engine::{Edit, LinkAction, LinkContent};
use serde_json::{Value, json};

use crate::{AdapterError, PdfAdapter};

/// The page a link operation is addressed to.
fn page_of(target: &Target) -> Result<PageRef, AdapterError> {
    match target {
        Target::Page(page) => Ok(*page),
        _ => Err(invalid("a link operation needs a page target")),
    }
}

fn invalid(message: impl Into<String>) -> AdapterError {
    AdapterError::new(ErrorCode::Invalid, message.into(), false)
}

fn limit(message: impl Into<String>) -> AdapterError {
    AdapterError::new(ErrorCode::Limit, message.into(), false)
}

/// A wire link this protocol cannot carry: too long or too many is `limit` (the
/// document could hold it, one operation cannot), anything else is `invalid`.
fn wire(error: LinkWireError) -> AdapterError {
    match &error {
        LinkWireError::TooLong { .. } | LinkWireError::TooMany { .. } => limit(error.to_string()),
        LinkWireError::Missing(_) | LinkWireError::Malformed(_) | LinkWireError::Invalid(_) => invalid(error.to_string()),
    }
}

impl PdfAdapter {
    /// The links on `page`, in `/Annots` order, as the model writes them back. A link
    /// the model has no form for (a JavaScript action, an area that is not numbers) is
    /// `unsupported`: the room never rewrites a page it cannot describe whole.
    fn page_links(&self, page: usize) -> Result<Vec<LinkContent>, AdapterError> {
        let document = self.document().ok_or_else(|| invalid("the document is gone"))?;
        let mut bodies = Vec::new();
        for link in document.links().iter().filter(|link| link.page == page) {
            if matches!(link.action, LinkAction::Other(_)) {
                return Err(AdapterError::new(ErrorCode::Unsupported, "a link on this page goes somewhere this room cannot write back", false));
            }
            bodies.push(LinkContent { rect: link.rect, quads: link.quads.clone(), action: link.action.clone(), style: link.style });
        }
        Ok(bodies)
    }

    /// `page.link.set` (RFC §3.1.9): the page's links become exactly `links`, in
    /// order. With `before`, the page's current links must be that list — the undo's
    /// compare-and-set, so a link someone changed in between is a `conflict`, not an
    /// overwrite.
    pub(crate) fn set_page_links(&mut self, target: &Target, payload: &mut Value) -> Result<Value, AdapterError> {
        let page = self.page_index(page_of(target)?)?;
        let links = payload.get("links").ok_or_else(|| invalid("the operation names no links"))?;
        let links = bodies_from_value(links).map_err(wire)?;
        let before = self.page_links(page)?;
        if let Some(expected) = payload.get("before") {
            let expected = bodies_from_value(expected).map_err(wire)?;
            if expected != before {
                return Err(AdapterError::new(ErrorCode::Conflict, "the page's links changed since the operation was built", false));
            }
        }
        let all = self.document_or_error()?.links().len();
        let total = all.saturating_sub(before.len()).saturating_add(links.len());
        if total > MAX_LINKS {
            return Err(limit(format!("a room holds at most {MAX_LINKS} links: this page's list would make {total}")));
        }
        payload["action"] = json!("page.link.set");
        payload["page"] = json!(page);
        payload["links"] = bodies_to_value(&links);
        payload["before"] = bodies_to_value(&before);
        self.apply_edit(Edit::SetPageLinks { page, links })?;
        self.epoch = self.epoch.saturating_add(1);
        Ok(json!({ "page": page, "links": payload["links"] }))
    }

    /// `page.link.clear`: remove one link of the page, named by its position among the
    /// page's links. The accepted operation carries the list before and after, so the
    /// undo is the same `page.link.set` over the list the page had.
    pub(crate) fn clear_page_link(&mut self, target: &Target, payload: &mut Value) -> Result<Value, AdapterError> {
        let page = self.page_index(page_of(target)?)?;
        let before = self.page_links(page)?;
        let index = payload
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok())
            .ok_or_else(|| invalid("the operation names no link"))?;
        if index >= before.len() {
            return Err(invalid(format!("page {} has no link {} of {}", page + 1, index + 1, before.len())));
        }
        let mut links = before.clone();
        let removed = links.remove(index);
        payload["action"] = json!("page.link.clear");
        payload["page"] = json!(page);
        payload["index"] = json!(index);
        payload["links"] = bodies_to_value(&links);
        payload["before"] = bodies_to_value(&before);
        payload["removed"] = pdfcraft_collab_link::body_to_value(&removed);
        self.apply_edit(Edit::SetPageLinks { page, links })?;
        self.epoch = self.epoch.saturating_add(1);
        Ok(json!({ "page": page, "index": index }))
    }
}
