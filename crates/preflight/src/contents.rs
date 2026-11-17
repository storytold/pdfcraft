//! The content streams a check reads as written: each page's `/Contents` streams joined as one
//! stream (ISO 32000-2 7.8.2: an operator's operands may end one part and the operator start the
//! next), then every other content stream, once each. Those are the streams the caller names,
//! which only a reference identifies (Type 3 glyph procedures), and the streams whose own
//! dictionary says they hold content: form XObjects, which appearances are, and tiling patterns.

use std::collections::HashSet;

use pdfcraft_cos::{DecodeBudget, Document, ObjRef, Object};

/// Calls `read` with each content stream of `doc`, decoded, until it returns `false`. `objs` are
/// the document's objects and `named` the content streams only a reference identifies. The streams
/// together decode to at most `max` bytes; past that, the rest aren't read.
pub(crate) fn each(
    doc: &Document,
    objs: &[(ObjRef, std::sync::Arc<Object>)],
    named: &HashSet<ObjRef>,
    max: usize,
    read: &mut dyn FnMut(&[u8]) -> bool,
) {
    let mut budget = DecodeBudget::new(max, 0);
    let mut done: HashSet<ObjRef> = HashSet::new();
    for page in pdfcraft_model::pages(doc) {
        if budget.remaining() == 0 {
            return;
        }
        let refs = crate::limits::page_contents(doc, &page.dict);
        let mut joined = Vec::new();
        for r in &refs {
            if let Object::Stream(s) = &*doc.get(*r)
                && let Ok(data) = budget.decode(s)
            {
                joined.extend_from_slice(&data);
                joined.push(b' ');
            }
        }
        done.extend(refs);
        if !read(&joined) {
            return;
        }
    }
    for (r, o) in objs {
        if budget.remaining() == 0 {
            return;
        }
        if let Object::Stream(s) = &**o
            && !done.contains(r)
            && (named.contains(r) || crate::is_content(&s.dict))
            && let Ok(data) = budget.decode(s)
            && !read(&data)
        {
            return;
        }
    }
}
