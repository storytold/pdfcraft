//! The content list of a page an edit rewrote holds content streams only.
//!
//! ISO 32000-2 §7.7.3.3 (Table 31) makes a page's `/Contents` "a content stream" or "an array of
//! these streams". A damaged file can list more: a reference to an object the file doesn't
//! contain (it reads as null, §7.3.10), a `null`, or an object that is not a stream. Such an entry
//! draws nothing, but an edit that rewrites the list (a watermark, a header, wrapping the
//! content in `q`/`Q`, a text edit) used to carry it along, and a full save then wrote it as
//! `null` inside the array. After an edit, each page whose list changed keeps only its streams;
//! the entries left out are said ([`EditNotice::ContentsLeftOut`](crate::EditNotice)), never
//! dropped silently. Pages the edit didn't touch keep their list as the file has it.

use pdfcraft_cos::{Document, ObjRef, Object};

use crate::history::Scope;
use crate::{EditError, EditNotice, guard};

/// What [`streams_only`] left out of one page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LeftOut {
    /// The page, 0-based.
    page: usize,
    /// How many entries of its content list were left out.
    entries: usize,
}

/// [`streams_only`] after an edit of `scope` took `prev` to `next`, and what it wants said. Only
/// content and whole-document edits rewrite content lists.
pub(crate) fn after_edit(scope: Scope, prev: &Document, next: &mut Document) -> Result<Option<EditNotice>, EditError> {
    if !matches!(scope, Scope::Content | Scope::Full) {
        return Ok(None);
    }
    let left_out = match guard(|| streams_only(prev, next)) {
        Ok(done) => done.map_err(|e| EditError::Write(e.to_string()))?,
        Err(m) => return Err(EditError::Write(m)),
    };
    if left_out.is_empty() {
        return Ok(None);
    }
    let pages = left_out.iter().map(|l| l.page).collect();
    let entries = left_out.iter().fold(0usize, |n, l| n.saturating_add(l.entries));
    Ok(Some(EditNotice::ContentsLeftOut { pages, entries }))
}

/// The entries of a page's `/Contents`: an array (direct or by reference), else the one value.
fn entries(doc: &Document, contents: Option<&Object>) -> Vec<Object> {
    match contents {
        None => Vec::new(),
        Some(c) => match &*doc.resolve(c) {
            Object::Array(a) => a.clone(),
            _ => vec![c.clone()],
        },
    }
}

/// Whether `contents` reads the same in `prev` and `next`: the entry itself, and the array it
/// refers to.
fn unchanged(prev: &Document, next: &Document, before: Option<&Object>, after: Option<&Object>) -> bool {
    if before != after {
        return false;
    }
    match after {
        Some(Object::Ref(r)) => {
            let (a, b) = (prev.get(*r), next.get(*r));
            std::sync::Arc::ptr_eq(&a, &b) || a == b
        }
        _ => true,
    }
}

/// Whether the next save appends to the file as it was read, so the bytes of an object listed
/// in it stay there (ISO 32000-2 §7.5.6). A file rebuilt from its object headers, or one that
/// must be rewritten, is written in full.
fn appends(doc: &Document) -> bool {
    !doc.reconstructed() && !doc.full_save_required() && !doc.encryption_changed()
}

/// An object the file lists under `r` but that couldn't be read. While saves append, the
/// reference keeps it reachable for a reader that can read it.
fn unreadable(doc: &Document, r: ObjRef) -> bool {
    doc.accepted_generations(r.num).contains(&r.generation) && doc.try_get(r.num).is_err()
}

/// Whether `entry` of a content list stays: a stream, or a reference to one. A reference to an
/// object the file lists but that can't be read stays while `appends`.
fn kept(doc: &Document, entry: &Object, appends: bool) -> bool {
    match entry {
        Object::Stream(_) => true,
        Object::Ref(r) => matches!(&*doc.get(*r), Object::Stream(_)) || (appends && unreadable(doc, *r)),
        _ => false,
    }
}

/// Leave out of each page of `next` whose content list differs from `prev`'s the entries that
/// name no content stream (see the module doc). Returns what was left out, by page.
fn streams_only(prev: &Document, next: &mut Document) -> Result<Vec<LeftOut>, pdfcraft_cos::CosError> {
    let appends = appends(next);
    let mut out = Vec::new();
    for (page, p) in pdfcraft_model::pages(next).into_iter().enumerate() {
        let before = prev.get(p.obj);
        let before = before.as_dict().and_then(|d| d.get(b"Contents"));
        let after = p.dict.get(b"Contents");
        if unchanged(prev, next, before, after) {
            continue;
        }
        let list = entries(next, after);
        let streams: Vec<Object> = list.iter().filter(|e| kept(next, e, appends)).cloned().collect();
        let left = list.len().saturating_sub(streams.len());
        if left == 0 {
            continue;
        }
        next.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), Object::Array(streams)))?;
        out.push(LeftOut { page, entries: left });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::after_edit;
    use crate::history::Scope;
    use crate::{Edit, EditNotice, Session, Watermark};
    use pdfcraft_cos::{Document, ObjRef, Object};

    /// A two-page file with a cross-reference table. Page 1 lists `/Contents [4 0 R n 0 R null
    /// 5 0 R]` and page 2 `[6 0 R n 0 R]`: object 90 is not in the file, or (with `listed`)
    /// object 9 is listed at an offset holding no object. Without `xref` there is no table, so
    /// the file is rebuilt from its object headers.
    fn damaged(listed: bool, xref: bool) -> Vec<u8> {
        let n = if listed { 9 } else { 90 };
        let stream = |text: &str| format!("<< /Length {} >> stream\n{text}\nendstream", text.len());
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 7 0 R] /Count 2 >>".to_string(),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents [4 0 R {n} 0 R null 5 0 R] >>"),
            stream("0 0 1 rg 10 10 50 50 re f"),
            stream("1 0 0 rg 100 100 50 50 re f"),
            stream("0 1 0 rg 10 100 50 50 re f"),
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents [6 0 R {n} 0 R] >>"),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let garbage = out.len();
        out.extend_from_slice(b"% not an object\n");
        if !xref {
            out.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF\n");
            return out;
        }
        let start = out.len();
        out.extend_from_slice(b"xref\n0 10\n0000000000 65535 f \n");
        for o in &offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(b"0000000000 00000 f \n");
        match listed {
            true => out.extend_from_slice(format!("{garbage:010} 00000 n \n").as_bytes()),
            false => out.extend_from_slice(b"0000000000 00000 f \n"),
        }
        out.extend_from_slice(format!("trailer << /Size 10 /Root 1 0 R >>\nstartxref\n{start}\n%%EOF\n").as_bytes());
        out
    }

    /// Each page's `/Contents` in `bytes` as written: what the array holds.
    fn lists(bytes: &[u8]) -> Vec<Vec<Object>> {
        let doc = Document::open_with_password(Arc::new(bytes.to_vec()), None).unwrap();
        pdfcraft_model::pages(&doc)
            .iter()
            .map(|p| match &*doc.resolve(p.dict.get(b"Contents").unwrap()) {
                Object::Array(a) => a.clone(),
                other => vec![other.clone()],
            })
            .collect()
    }

    fn streams(doc: &Document, list: &[Object]) -> usize {
        list.iter().filter(|o| matches!(&*doc.resolve(o), Object::Stream(_))).count()
    }

    /// The decoded content of the streams `list` names, in order.
    fn content(doc: &Document, list: &[Object]) -> String {
        list.iter()
            .filter_map(|o| match &*doc.resolve(o) {
                Object::Stream(st) => st.decoded().ok().map(|d| String::from_utf8_lossy(&d).into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(
                "
",
            )
    }

    fn watermark() -> Edit {
        Edit::AddWatermark { pages: vec![0], settings: Watermark { text: "DRAFT".into(), ..Watermark::default() }, file: None, replace: false }
    }

    /// A watermark rewrites page 1's content list. The reference to the missing object 90 and the
    /// `null` are left out, and said; neither a save nor a full save writes a `null` into it.
    /// Page 2, which the edit didn't touch, keeps its list as the file has it. Undo brings page
    /// 1's back.
    #[test]
    fn a_rewritten_content_list_leaves_out_what_is_not_a_stream_and_says_so() {
        for xref in [true, false] {
            let mut s = Session::new();
            let id = s.open("a.pdf", None, Arc::new(damaged(false, xref)), None).unwrap();
            s.apply(id, watermark()).unwrap();
            let said: Vec<String> = s.get(id).unwrap().edit_notices.iter().map(ToString::to_string).collect();
            assert_eq!(s.get(id).unwrap().edit_notices, [EditNotice::ContentsLeftOut { pages: vec![0], entries: 2 }], "xref {xref}");
            assert!(said[0].starts_with("2 entries in the content list of page 1 were left out"), "{said:?}");
            for bytes in [s.save_bytes(id).unwrap(), s.save_full_bytes(id).unwrap()] {
                let doc = Document::open_with_password(bytes.clone(), None).unwrap();
                let [first, second] = <[Vec<Object>; 2]>::try_from(lists(&bytes)).unwrap();
                assert!(!first.is_empty() && streams(&doc, &first) == first.len(), "xref {xref}: page 1 lists {first:?}");
                // Both original paint streams are still drawn, blue before red.
                let painted = content(&doc, &first);
                let blue = painted.find("0 0 1 rg 10 10 50 50 re f");
                let red = painted.find("1 0 0 rg 100 100 50 50 re f");
                assert!(blue.zip(red).is_some_and(|(b, r)| b < r), "xref {xref}: {painted}");
                assert_eq!(second.len(), 2, "xref {xref}: page 2 untouched: {second:?}");
            }
            s.undo(id).unwrap();
            let doc = &s.get(id).unwrap().editor.as_ref().unwrap().cos;
            let page = pdfcraft_model::pages(doc).swap_remove(0);
            assert!(matches!(page.dict.get(b"Contents"), Some(Object::Array(a)) if a.len() == 4), "undo restores the list");
        }
    }

    /// An object the file lists but that can't be read is not known to be missing: while saves
    /// append to the file, the rewritten list keeps the reference to it (its bytes stay in the
    /// file for a reader that can read them). Only the `null` goes.
    #[test]
    fn an_unreadable_listed_object_stays_while_saves_append() {
        let mut s = Session::new();
        let id = s.open("a.pdf", None, Arc::new(damaged(true, true)), None).unwrap();
        assert!(!s.get(id).unwrap().editor.as_ref().unwrap().cos.reconstructed());
        s.apply(id, watermark()).unwrap();
        assert_eq!(s.get(id).unwrap().edit_notices, [EditNotice::ContentsLeftOut { pages: vec![0], entries: 1 }]);
        let doc = &s.get(id).unwrap().editor.as_ref().unwrap().cos;
        let page = pdfcraft_model::pages(doc).swap_remove(0);
        let Some(Object::Array(list)) = page.dict.get(b"Contents") else { panic!("{:?}", page.dict) };
        assert!(list.iter().any(|o| matches!(o, Object::Ref(r) if r.num == 9)), "{list:?}");
        assert!(!list.iter().any(|o| matches!(o, Object::Null)), "{list:?}");
    }

    /// When the next save rewrites the file, an object the file lists but that can't be read
    /// goes from a rewritten list too. It is not known to be missing or to draw nothing, so the
    /// notice names it as one that could not be read.
    #[test]
    fn an_unreadable_object_left_out_is_said_to_be_unreadable() {
        let prev = Document::open_with_password(Arc::new(damaged(true, true)), None).unwrap();
        let mut next = prev.clone();
        next.require_full_save();
        let page = pdfcraft_model::pages(&next).swap_remove(0).obj;
        next.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Array(vec![Object::Ref(ObjRef::new(4, 0)), Object::Ref(ObjRef::new(9, 0))])))
            .unwrap();
        let said = after_edit(Scope::Content, &prev, &mut next).unwrap().map(|n| n.to_string()).unwrap_or_default();
        assert!(said.starts_with("1 entry in the content list of page 1 was left out"), "{said}");
        assert!(said.contains("could not be read") && !said.contains("drew nothing"), "{said}");
    }
}
