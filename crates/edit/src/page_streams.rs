//! A page's content streams rewritten one at a time: each stream keeps its own dictionary
//! (and so its marks, such as `/PCMark`), and only the streams that changed are replaced.

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

use crate::EditError;

/// The decoded content streams of one page, in drawing order.
pub(crate) struct PageStreams {
    /// Each stream's object and dictionary.
    entries: Vec<(ObjRef, Dict)>,
    /// Each stream's decoded data, in the same order.
    pub(crate) data: Vec<Vec<u8>>,
}

impl PageStreams {
    /// The streams of `page_dict`'s `/Contents`. `None` when one of them is not a stream or
    /// cannot be decoded: rewriting the others would draw the page differently, so the page is
    /// left alone. A page without content has no streams.
    pub(crate) fn read(doc: &Document, page_dict: &Dict) -> Option<Self> {
        let refs: Vec<Object> = match page_dict.get(b"Contents") {
            None => Vec::new(),
            Some(c) => match &*doc.resolve(c) {
                Object::Array(a) => a.clone(),
                _ => vec![c.clone()],
            },
        };
        let mut entries = Vec::with_capacity(refs.len());
        let mut data = Vec::with_capacity(refs.len());
        for e in &refs {
            let r = e.as_ref()?;
            let (dict, bytes) = match &*doc.get(r) {
                Object::Stream(s) => (s.dict.clone(), s.decoded().ok()?),
                _ => return None,
            };
            data.push(bytes);
            entries.push((r, dict));
        }
        Some(Self { entries, data })
    }

    /// Put `new` in place of the streams that changed (`None` keeps a stream as it is). Each
    /// replacement is a new object with the old dictionary (minus its encoding), so a stream
    /// another page shares is not touched. Returns whether anything changed.
    pub(crate) fn write(self, doc: &mut Document, page: ObjRef, new: Vec<Option<Vec<u8>>>) -> Result<bool, EditError> {
        if new.iter().all(Option::is_none) {
            return Ok(false);
        }
        let mut refs: Vec<Object> = Vec::with_capacity(self.entries.len());
        for (i, (r, dict)) in self.entries.into_iter().enumerate() {
            match new.get(i).and_then(Option::as_ref) {
                Some(bytes) => {
                    let mut dict = dict;
                    for k in [&b"Length"[..], b"Filter", b"DecodeParms", b"DL"] {
                        dict.remove(k);
                    }
                    refs.push(Object::Ref(doc.add(Object::Stream(Stream::flate(dict, bytes)))));
                }
                None => refs.push(Object::Ref(r)),
            }
        }
        let contents = match <[Object; 1]>::try_from(refs) {
            Ok([one]) => one,
            Err(many) => Object::Array(many),
        };
        doc.update_dict(page, |d| d.set(b"Contents".to_vec(), contents))?;
        Ok(true)
    }
}
