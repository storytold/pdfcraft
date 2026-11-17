//! PDF/A-2 and -3 6.2.6: rendering intents. Where a rendering intent is given, it is one of the four
//! of ISO 32000-1 8.6.5.8, Table 70: `AbsoluteColorimetric`, `RelativeColorimetric`, `Saturation`
//! or `Perceptual`. An intent is given by a graphics state's `/RI` (read with the graphics states,
//! `gstate.rs`), by an image's `/Intent`, and in content by the `ri` operator and an inline
//! image's `/Intent` (ISO 32000-2 Tables 56, 57, 87 and 91; Table 69 lists the same four).
//!
//! Content is read as the implementation limits scan reads it ([`crate::limits`]): a page's
//! `/Contents` streams joined, as one content stream (ISO 32000-1 7.8.2), and every other content
//! stream on its own, operator by operator, so strings and image data are never taken for
//! operators. Content is decoded within one allowance for the whole document, with `/DecodeParms`
//! given by reference read too; what decodes of a damaged stream is read.

use std::collections::{BTreeSet, HashSet};

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object};

/// The intents of ISO 32000-1 Table 70, as the graphics state scan has them too.
const INTENTS: [&[u8]; 4] = [b"AbsoluteColorimetric", b"RelativeColorimetric", b"Saturation", b"Perceptual"];
/// The decoded content, across all the document's content streams, the scan reads.
const MAX_CONTENT_BYTES: usize = 256 << 20;
/// How many different names of each kind are reported.
const MAX_NAMES: usize = 8;
/// The longest name shown, in bytes.
const MAX_SHOWN: usize = 64;

/// The rendering intents given outside graphics states that aren't among [`INTENTS`].
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Findings {
    /// Names that content sets with `ri`.
    pub(crate) content: BTreeSet<String>,
    /// Content sets with `ri` a rendering intent that isn't a name.
    pub(crate) content_not_name: bool,
    /// Names that images, image XObjects and inline images, give as `/Intent`.
    pub(crate) image: BTreeSet<String>,
    /// An image's `/Intent` isn't a name.
    pub(crate) image_not_name: bool,
}

impl Findings {
    fn name(found: &mut BTreeSet<String>, name: &[u8]) {
        if !INTENTS.contains(&name) && found.len() < MAX_NAMES {
            found.insert(String::from_utf8_lossy(name.get(..MAX_SHOWN).unwrap_or(name)).into_owned());
        }
    }

    /// An image's `/Intent`, resolved with `doc` when it is an image XObject's.
    fn image_intent(&mut self, doc: Option<&Document>, image: &Dict) {
        let Some(intent) = image.get(b"Intent") else { return };
        let intent = match doc {
            Some(doc) => doc.resolve(intent),
            None => std::sync::Arc::new(intent.clone()),
        };
        match &*intent {
            Object::Name(n) => Self::name(&mut self.image, n),
            // A null value counts as no entry (ISO 32000-1 7.3.9).
            Object::Null => {}
            _ => self.image_not_name = true,
        }
    }

    /// The `ri` operators and inline images of one content stream.
    fn content(&mut self, data: &[u8]) {
        for op in pdfcraft_content::ops(data) {
            if op.op.as_slice() == b"ri" {
                match op.operands.last() {
                    Some(Object::Name(n)) => Self::name(&mut self.content, n),
                    _ => self.content_not_name = true,
                }
            }
            if let Some((image, _)) = &op.inline {
                self.image_intent(None, image);
            }
        }
    }
}

/// The rendering intents that the image XObjects among `objs` and the content streams give
/// outside graphics states. `content` holds the content streams only a reference identifies (a
/// page's `/Contents`, Type 3 glyph procedures), besides form XObjects and appearance streams,
/// which say so themselves.
pub(crate) fn check(doc: &Document, objs: &[(ObjRef, std::sync::Arc<Object>)], content: &HashSet<ObjRef>) -> Findings {
    let mut found = Findings::default();
    for (_, o) in objs {
        if let Object::Stream(s) = &**o
            && s.dict.name(b"Subtype") == Some(b"Image")
        {
            found.image_intent(Some(doc), &s.dict);
        }
    }
    let mut budget = DecodeBudget::new(MAX_CONTENT_BYTES, 0);
    let mut read: HashSet<ObjRef> = HashSet::new();
    for page in pdfcraft_model::pages(doc) {
        if budget.remaining() == 0 {
            break;
        }
        let refs = crate::limits::page_contents(doc, &page.dict);
        let mut joined = Vec::new();
        for r in &refs {
            if let Object::Stream(s) = &*doc.get(*r)
                && let Ok((data, _)) = budget.decode_with_damage(doc, s, usize::MAX)
            {
                joined.extend_from_slice(&data);
                joined.push(b' ');
            }
        }
        read.extend(refs);
        found.content(&joined);
    }
    for (r, o) in objs {
        if budget.remaining() == 0 {
            break;
        }
        if let Object::Stream(s) = &**o
            && !read.contains(r)
            && (content.contains(r) || crate::is_content(&s.dict))
            && let Ok((data, _)) = budget.decode_with_damage(doc, s, usize::MAX)
        {
            found.content(&data);
        }
    }
    found
}
