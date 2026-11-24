//! Reading content streams within one allowance: a page's `/Contents` array is one content stream
//! split across several (§7.8.2), so its pieces are read joined, and a stream that several pages'
//! arrays share is decoded once.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use super::ctx;
use super::scan::Notes;

/// Pages whose `/Contents` is an array (direct or by reference), with the streams in it.
pub(super) fn content_arrays(doc: &Document, all: &[ObjRef], reached: &HashMap<ObjRef, u64>) -> HashMap<ObjRef, Vec<ObjRef>> {
    let bits = |r: &ObjRef| reached.get(r).copied().unwrap_or(0);
    let mut out = HashMap::new();
    for &r in all {
        let obj = doc.get(r);
        let Object::Dict(d) = &*obj else { continue };
        if (ctx::own(d, false) | bits(&r)) & ctx::PAGE == 0 {
            continue;
        }
        if let Some(mut members) = array_members(doc, d) {
            members.retain(|m| bits(m) & ctx::ENCRYPT == 0);
            out.insert(r, members);
        }
    }
    out
}

/// The streams page `d`'s `/Contents` array names; `None` when it is no array.
fn array_members(doc: &Document, d: &Dict) -> Option<Vec<ObjRef>> {
    let contents = doc.resolve(d.get(b"Contents")?);
    let Object::Array(list) = &*contents else { return None };
    Some(list.iter().filter_map(Object::as_ref).filter(|m| matches!(&*doc.get(*m), Object::Stream(_))).collect())
}

/// Decodes streams within the check's allowance and remembers those it could not read.
///
/// `left` bounds the bytes decoded, each stream charged once however many pages join it.
/// `copy_left` bounds the bytes of shared streams copied into pages' joined content, which are
/// scanned again but not decoded again. `page_cap` bounds one page's joined content; a stream
/// that would pass it is left out and counted. Held at once, at most: the shared streams still to
/// be joined (within `left`) and one page's joined content (within `page_cap`).
pub(super) struct Decoder {
    left: usize,
    copy_left: usize,
    page_cap: usize,
    unread: HashSet<ObjRef>,
    /// Shared streams decoded and still to be joined; `None` for one that could not be read.
    shared: HashMap<ObjRef, Option<Rc<Vec<u8>>>>,
    /// Per member list already scanned, what it needs: pages sharing one array are scanned once.
    lists: HashMap<Vec<ObjRef>, Notes>,
    /// Per stream, how many member lists still to be joined name it.
    uses: HashMap<ObjRef, usize>,
}

impl Decoder {
    pub(super) fn new(allowance: usize, copy_allowance: usize, page_cap: usize, arrays: &HashMap<ObjRef, Vec<ObjRef>>) -> Self {
        let mut uses: HashMap<ObjRef, usize> = HashMap::new();
        let distinct: HashSet<&Vec<ObjRef>> = arrays.values().collect();
        for list in distinct {
            let once: HashSet<&ObjRef> = list.iter().collect();
            for m in once {
                let n = uses.entry(*m).or_insert(0);
                *n = n.saturating_add(1);
            }
        }
        Decoder { left: allowance, copy_left: copy_allowance, page_cap, unread: HashSet::new(), shared: HashMap::new(), lists: HashMap::new(), uses }
    }

    /// Streams that could not be read, each counted once.
    pub(super) fn unread(&self) -> usize {
        self.unread.len()
    }

    /// What the content stream `members` make together needs, by `scan` of their decoded bytes
    /// joined by a space, as renderers join them (`pdfcraft_content::Pieces::join`): a comment left
    /// open at a piece's end runs on into the next piece. A stream that can't be read, or past an
    /// allowance or the page's cap, is left out and counted. `None` once `keep_going` stops.
    pub(super) fn joined(
        &mut self,
        doc: &Document,
        members: &[ObjRef],
        keep_going: &mut dyn FnMut() -> bool,
        scan: &mut dyn FnMut(&[u8], &mut Notes),
    ) -> Option<Notes> {
        if let Some(notes) = self.lists.get(members) {
            return Some(notes.clone());
        }
        let last: HashMap<ObjRef, usize> = members.iter().enumerate().map(|(i, m)| (*m, i)).collect();
        for m in last.keys() {
            self.use_up(*m);
        }
        let mut data = Vec::new();
        for (i, &m) in members.iter().enumerate() {
            if !keep_going() {
                return None;
            }
            let later_here = last.get(&m).is_some_and(|l| *l > i);
            let more = later_here || self.uses.get(&m).is_some_and(|n| *n > 0);
            let Some((bytes, copy)) = self.piece(doc, m, more) else { continue };
            if self.admit(data.len(), bytes.len(), copy) {
                data.extend_from_slice(&bytes);
                data.push(b' ');
            } else {
                self.unread.insert(m);
            }
        }
        let mut notes = Notes::new();
        scan(&data, &mut notes);
        self.lists.insert(members.to_vec(), notes.clone());
        Some(notes)
    }

    /// Whether a piece of `len` bytes, plus its joining space, fits after `held` bytes of the page,
    /// charging a copy of a shared stream to `copy_left`.
    fn admit(&mut self, held: usize, len: usize, copy: bool) -> bool {
        if held.saturating_add(len) >= self.page_cap {
            return false;
        }
        if !copy {
            return true;
        }
        match self.copy_left.checked_sub(len) {
            Some(rest) => {
                self.copy_left = rest;
                true
            }
            None => false,
        }
    }

    fn use_up(&mut self, m: ObjRef) {
        if let Some(n) = self.uses.get_mut(&m) {
            *n = n.saturating_sub(1);
        }
    }

    /// Stream `m`'s data, decoded once however many lists name it, and whether it is held for
    /// lists still to come (`more`) or was held for one before; `None` when it can't be read.
    fn piece(&mut self, doc: &Document, m: ObjRef, more: bool) -> Option<(Rc<Vec<u8>>, bool)> {
        let decoded = match self.shared.get(&m) {
            Some(cached) => cached.clone(),
            None => {
                let read = match &*doc.get(m) {
                    Object::Stream(s) => self.read(m, s),
                    _ => None,
                };
                let read = read.map(Rc::new);
                if !more {
                    return read.map(|b| (b, false));
                }
                self.shared.insert(m, read.clone());
                read
            }
        };
        if !more {
            self.shared.remove(&m);
        }
        decoded.map(|b| (b, true))
    }

    /// Stream `r`'s data within the allowance; `None`, and `r` counted, when it can't be read.
    pub(super) fn read(&mut self, r: ObjRef, s: &Stream) -> Option<Vec<u8>> {
        match s.decoded_within(self.left) {
            Ok(data) => {
                self.left = self.left.saturating_sub(data.len());
                Some(data)
            }
            Err(_) => {
                self.unread.insert(r);
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_pages_joined_content_stays_within_its_cap() {
        // A page naming one 4-byte stream four times: each copy is 5 bytes joined, so a 12-byte cap
        // keeps two and counts the stream unread once.
        let mut doc = Document::new_empty();
        let s = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"/OC ".to_vec())));
        let page = doc.add(Object::Dict(Dict::new()));
        let members = vec![s, s, s, s];
        let arrays = HashMap::from([(page, members.clone())]);
        let mut decoder = Decoder::new(1 << 20, 1 << 20, 12, &arrays);
        let mut held = 0;
        let notes = decoder.joined(&doc, &members, &mut || true, &mut |data, _| held = data.len());
        assert!(notes.is_some());
        assert_eq!(held, 10, "two copies joined, the rest left out");
        assert_eq!(decoder.unread(), 1);
    }
}
