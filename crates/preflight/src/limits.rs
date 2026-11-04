//! PDF/A-2 6.1.13: a file stays within the implementation limits of ISO 32000-1 Annex C (Table C.1
//! and the page size limits after it): integers that fit in 32 bits, strings of at most 32767
//! bytes, names of at most 127 bytes, and page boundaries between 3 and 14400 units wide and high.
//!
//! The values are read from the indirect objects and from the content streams, as parsed: a name
//! written with `#xx` escapes counts its decoded bytes. Character identifiers above 65535 (in a
//! CMap) and the real-number range are not checked here.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// Table C.1: the smallest and largest integer values.
const MIN_INT: i64 = i32::MIN as i64;
const MAX_INT: i64 = i32::MAX as i64;
/// Table C.1: the longest string, in bytes.
const MAX_STRING: usize = 32767;
/// Table C.1: the longest name, in bytes.
const MAX_NAME: usize = 127;
/// The smallest and largest page boundary, in default user space units.
const MIN_PAGE: f64 = 3.0;
const MAX_PAGE: f64 = 14400.0;
/// How deep a direct object is followed (arrays and dictionaries inside one another).
const MAX_DEPTH: u8 = 32;
/// The page boundaries, of which only the media and crop boxes are inherited.
const BOXES: [&[u8]; 5] = [b"MediaBox", b"CropBox", b"BleedBox", b"TrimBox", b"ArtBox"];

/// Which limits a value went past.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Exceeded {
    pub(crate) int: bool,
    pub(crate) string: bool,
    pub(crate) name: bool,
}

impl Exceeded {
    fn all(self) -> bool {
        self.int && self.string && self.name
    }

    fn name(&mut self, n: &[u8]) {
        self.name |= n.len() > MAX_NAME;
    }

    /// One value and, within [`MAX_DEPTH`], what it holds: a dictionary's keys count as names.
    fn value(&mut self, o: &Object, depth: u8) {
        if self.all() || depth > MAX_DEPTH {
            return;
        }
        match o {
            Object::Int(i) => self.int |= !(MIN_INT..=MAX_INT).contains(i),
            Object::String(s) => self.string |= s.bytes.len() > MAX_STRING,
            Object::Name(n) => self.name(n),
            Object::Array(a) => {
                for v in a {
                    self.value(v, depth.saturating_add(1));
                }
            }
            Object::Dict(d) => self.dict(d, depth),
            Object::Stream(s) => self.dict(&s.dict, depth),
            Object::Null | Object::Bool(_) | Object::Real(_) | Object::Ref(_) => {}
        }
    }

    fn dict(&mut self, d: &Dict, depth: u8) {
        for (k, v) in d.iter() {
            self.name(k);
            self.value(v, depth.saturating_add(1));
        }
    }
}

/// The limits that the indirect objects `objs`, and the content streams among them (`content`:
/// the ones only a reference says hold content, besides form XObjects and appearance streams), go
/// past.
pub(crate) fn exceeded(objs: &[(ObjRef, std::sync::Arc<Object>)], content: &HashSet<ObjRef>) -> Exceeded {
    let mut found = Exceeded::default();
    for (r, o) in objs {
        if found.all() {
            break;
        }
        found.value(o, 0);
        if let Object::Stream(s) = &**o
            && (content.contains(r) || crate::is_content(&s.dict))
            && let Ok(data) = s.decoded()
        {
            for op in pdfcraft_content::ops(&data) {
                if found.all() {
                    break;
                }
                for v in &op.operands {
                    found.value(v, 0);
                }
                if let Some((image, _)) = &op.inline {
                    found.dict(image, 0);
                }
            }
        }
    }
    found
}

/// Whether one of a page's boundaries (with the media and crop boxes it inherits) is narrower or
/// lower than 3 units, or wider or higher than 14400.
pub(crate) fn page_box_out_of_range(doc: &Document, page: &Dict) -> bool {
    BOXES.iter().filter_map(|k| page.get(k)).any(|b| {
        let b = doc.resolve(b);
        let v: Vec<f64> = b.as_array().map(|a| a.iter().filter_map(|x| doc.resolve(x).as_f64()).collect()).unwrap_or_default();
        let [x0, y0, x1, y1] = v.as_slice() else { return false };
        let (w, h) = ((x1 - x0).abs(), (y1 - y0).abs());
        [w, h].iter().any(|s| s.is_finite() && !(MIN_PAGE..=MAX_PAGE).contains(s))
    })
}
