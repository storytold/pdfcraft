//! The entries of a page's `/Contents`, read one way for every caller.
//!
//! ISO 32000-2 §7.7.3.3 (Table 31) makes `/Contents` one content stream or an array of them,
//! concatenated in order. [`content_entries`] gives that list without deciding what an entry
//! means: each caller still chooses whether a null, a non-stream or an undecodable stream is
//! skipped, an empty piece or an error, because redaction and signing must fail closed where a
//! reader may skip.
//!
//! One resolve policy: the `/Contents` value follows [`Document::resolve`]'s bounded chain (an
//! initial reference plus up to 32 more). A chain still ending in a reference (a cycle, or one too
//! long) is [`ContentsError::UnresolvedReference`]. A reference to a missing object is a single
//! entry, that reference, which reads as `null` like any other.
//!
//! One cap: an array past [`MAX_CONTENT_ENTRIES`] is [`ContentsError::TooMany`], never a prefix,
//! because callers rebuild, rewrite and redact from this list and a silent cut would look like the
//! end of the page. A direct array is read in place and an indirect one through the document's
//! shared copy, so the list is never cloned.

use crate::document::Held;
use crate::{Document, Object};

/// The most entries a page's `/Contents` array may list.
///
/// [`crate::DecodeBudget`] bounds the bytes a page decodes, not its entries: a `null` costs no
/// decode, so without this a file could name millions of them on every page, and each caller
/// would walk them all for each page. This is a conservative product allowance, not a limit ISO
/// 32000-2 sets. It is above every cap a caller keeps for itself (edit's 10,000 streams for
/// selecting objects together, its 4,096 for finding a backdrop), so none of those loosen.
pub const MAX_CONTENT_ENTRIES: usize = 65_536;

/// Why a page's `/Contents` could not be listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ContentsError {
    /// The `/Contents` reference chain ends in a reference: a cycle, or more hops than
    /// [`Document::resolve`] follows.
    #[error("the page contents are a reference that could not be followed")]
    UnresolvedReference,
    /// An array with more than [`MAX_CONTENT_ENTRIES`] entries.
    #[error("the page contents list {len} entries, more than the {cap} allowed")]
    TooMany { len: usize, cap: usize },
}

/// The entries of one page's `/Contents`, in order. See [`content_entries`].
#[derive(Debug)]
pub struct ContentEntries<'o> {
    from: Source<'o>,
}

#[derive(Debug)]
enum Source<'o> {
    /// No `/Contents`: a page with nothing drawn.
    Nothing,
    /// A value that is not an array: the one entry, as the page holds it.
    One(&'o Object),
    /// An array, direct or the document's copy of an indirect one.
    List(Held<'o>),
}

impl ContentEntries<'_> {
    /// The entries, as the page or its array holds them: references stay references, so a caller
    /// can write a list back or keep each entry beside what it decoded.
    pub fn as_slice(&self) -> &[Object] {
        match &self.from {
            Source::Nothing => &[],
            Source::One(o) => std::slice::from_ref(*o),
            Source::List(held) => held.as_array().map_or(&[], Vec::as_slice),
        }
    }

    /// How many entries there are.
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Whether there are none.
    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    /// The entries in order.
    pub fn iter(&self) -> std::slice::Iter<'_, Object> {
        self.as_slice().iter()
    }
}

impl<'e> IntoIterator for &'e ContentEntries<'_> {
    type Item = &'e Object;
    type IntoIter = std::slice::Iter<'e, Object>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// The entries of a page's `/Contents` (pass `page.get(b"Contents")`): none when it is absent;
/// the array's entries when it is an array, directly or through references; otherwise one entry,
/// the value exactly as the page holds it (a reference stays the reference).
///
/// Errors when a reference chain can't be followed to an object, and when the array has more than
/// [`MAX_CONTENT_ENTRIES`] entries. Its length is checked before anything else is read, and the
/// array is never copied.
pub fn content_entries<'o>(doc: &Document, contents: Option<&'o Object>) -> Result<ContentEntries<'o>, ContentsError> {
    let Some(contents) = contents else { return Ok(ContentEntries { from: Source::Nothing }) };
    let held = doc.held(contents);
    let len = match &*held {
        Object::Ref(_) => return Err(ContentsError::UnresolvedReference),
        Object::Array(list) => list.len(),
        _ => return Ok(ContentEntries { from: Source::One(contents) }),
    };
    if len > MAX_CONTENT_ENTRIES {
        return Err(ContentsError::TooMany { len, cap: MAX_CONTENT_ENTRIES });
    }
    Ok(ContentEntries { from: Source::List(held) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dict, ObjRef, Stream};

    fn stream(doc: &mut Document, bytes: &[u8]) -> Object {
        Object::Ref(doc.add(Object::Stream(Stream::from_raw(Dict::new(), bytes.to_vec()))))
    }

    #[test]
    fn absent_contents_list_nothing() {
        let doc = Document::new_empty();
        let entries = content_entries(&doc, None).unwrap();
        assert!(entries.is_empty());
        assert_eq!(entries.len(), 0);
    }

    /// One stream by reference is one entry, the reference itself (not the stream), so a caller
    /// that writes `/Contents` back keeps it pointing at the same object.
    #[test]
    fn a_single_stream_is_one_entry_as_the_page_holds_it() {
        let mut doc = Document::new_empty();
        let s = stream(&mut doc, b"0 0 m");
        assert_eq!(content_entries(&doc, Some(&s)).unwrap().as_slice(), std::slice::from_ref(&s));
    }

    #[test]
    fn an_array_by_reference_lists_its_entries_in_order() {
        let mut doc = Document::new_empty();
        let (a, b) = (stream(&mut doc, b"a"), stream(&mut doc, b"b"));
        let list = vec![a, Object::Null, b];
        let contents = Object::Ref(doc.add(Object::Array(list.clone())));
        assert_eq!(content_entries(&doc, Some(&contents)).unwrap().as_slice(), list.as_slice());
    }

    /// A reference to a reference to the array lists the array. The copies that followed one hop
    /// (`doc.get`) took the inner reference for a single stream.
    #[test]
    fn a_chain_of_references_to_the_array_is_followed() {
        let mut doc = Document::new_empty();
        let list = vec![stream(&mut doc, b"a"), stream(&mut doc, b"b")];
        let array = doc.add(Object::Array(list.clone()));
        let contents = Object::Ref(doc.add(Object::Ref(array)));
        assert_eq!(content_entries(&doc, Some(&contents)).unwrap().as_slice(), list.as_slice());
    }

    /// A reference to nothing reads as `null` in its own place, for the caller to judge.
    #[test]
    fn a_reference_to_a_missing_object_is_one_entry() {
        let doc = Document::new_empty();
        let missing = Object::Ref(ObjRef::new(9_999, 0));
        assert_eq!(content_entries(&doc, Some(&missing)).unwrap().as_slice(), std::slice::from_ref(&missing));
    }

    #[test]
    fn a_reference_cycle_is_an_error() {
        let mut doc = Document::new_empty();
        let r = doc.add(Object::Null);
        doc.set(r, Object::Ref(r));
        assert_eq!(content_entries(&doc, Some(&Object::Ref(r))).unwrap_err(), ContentsError::UnresolvedReference);
    }

    /// A direct array is read where it is: the entries are the page's own, not a copy.
    #[test]
    fn a_direct_array_is_not_copied() {
        let doc = Document::new_empty();
        let contents = Object::Array(vec![Object::Null; 3]);
        let entries = content_entries(&doc, Some(&contents)).unwrap();
        let Object::Array(list) = &contents else { panic!("not an array") };
        assert!(std::ptr::eq(entries.as_slice().as_ptr(), list.as_ptr()));
    }

    /// An indirect array is the document's shared copy, not a fresh one.
    #[test]
    fn an_indirect_array_is_not_copied() {
        let mut doc = Document::new_empty();
        let array = doc.add(Object::Array(vec![Object::Null; 3]));
        let contents = Object::Ref(array);
        let entries = content_entries(&doc, Some(&contents)).unwrap();
        let held = doc.get(array);
        let Object::Array(list) = &*held else { panic!("not an array") };
        assert!(std::ptr::eq(entries.as_slice().as_ptr(), list.as_ptr()));
    }

    /// At the cap every entry is listed; one past it is an error with no prefix, direct or not.
    #[test]
    fn the_cap_is_an_error_never_a_prefix() {
        let mut doc = Document::new_empty();
        let at = Object::Array(vec![Object::Null; MAX_CONTENT_ENTRIES]);
        assert_eq!(content_entries(&doc, Some(&at)).unwrap().len(), MAX_CONTENT_ENTRIES);
        let over = Object::Array(vec![Object::Null; MAX_CONTENT_ENTRIES + 1]);
        let too_many = ContentsError::TooMany { len: MAX_CONTENT_ENTRIES + 1, cap: MAX_CONTENT_ENTRIES };
        assert_eq!(content_entries(&doc, Some(&over)).unwrap_err(), too_many);
        let indirect = Object::Ref(doc.add(over));
        assert_eq!(content_entries(&doc, Some(&indirect)).unwrap_err(), too_many);
    }

    /// A value that is neither a stream nor an array is still one entry: what it means is the
    /// caller's to decide (a reader skips it, redaction refuses the page).
    #[test]
    fn any_other_value_is_one_entry() {
        let doc = Document::new_empty();
        let odd = Object::Int(7);
        assert_eq!(content_entries(&doc, Some(&odd)).unwrap().as_slice(), std::slice::from_ref(&odd));
    }
}
