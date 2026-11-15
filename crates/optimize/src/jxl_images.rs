//! Storing JPEG images as JPEG XL, and back (the Clean Up options `jpeg_to_jxl` and `jxl_to_jpeg`).
//!
//! A DCT image becomes a `JXLDecode` image whose file the JPEG can be rebuilt from byte for byte
//! (`pdfcraft-jxl`), so the change loses nothing and can be undone. ISO 32000-2 has no JPEG XL
//! filter yet: the name is a placeholder and readers without JPEG XL show nothing for such an
//! image, which is why both options are off unless asked for.

use crate::{Report, Settings};
use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

/// The Clean Up pass: every image in one direction, when exactly one of the two options is on.
pub(crate) fn convert(doc: &mut Document, settings: &Settings, report: &mut Report) {
    if settings.jpeg_to_jxl == settings.jxl_to_jpeg {
        return;
    }
    for num in doc.object_numbers() {
        let r = ObjRef { num, generation: doc.generation(num) };
        let obj = doc.get(r);
        let Object::Stream(s) = &*obj else { continue };
        let (replacement, count) =
            if settings.jpeg_to_jxl { (jpeg_to_jxl(s), &mut report.jpeg_to_jxl) } else { (jxl_to_jpeg(s), &mut report.jxl_to_jpeg) };
        if let Some(replacement) = replacement {
            doc.set(r, Object::Stream(replacement));
            *count += 1;
        }
    }
}

/// `s` as a JPEG XL image when it is a plain JPEG image the conversion keeps exactly and the
/// result is smaller; `None` otherwise (the image stays as it is).
pub(crate) fn jpeg_to_jxl(s: &Stream) -> Option<Stream> {
    if !is_image(s) || filter_of(&s.dict)? != Filter::Dct {
        return None;
    }
    // The JPEG XL file holds the JPEG's own size: a dictionary that says another size is left alone.
    let (w, h, _) = pdfcraft_jxl::jpeg_frame(&s.raw)?;
    if s.dict.int(b"Width") != Some(i64::from(w)) || s.dict.int(b"Height") != Some(i64::from(h)) {
        return None;
    }
    let jxl = pdfcraft_jxl::recompress_jpeg(&s.raw).ok()?;
    (jxl.len() < s.raw.len()).then(|| replaced(&s.dict, "JXLDecode", jxl))
}

/// `s` as the JPEG it was made from, when it is a `JXLDecode` image with reconstruction data.
pub(crate) fn jxl_to_jpeg(s: &Stream) -> Option<Stream> {
    if !is_image(s) || filter_of(&s.dict)? != Filter::Jxl {
        return None;
    }
    let jpeg = pdfcraft_jxl::reconstruct_jpeg(&s.raw).ok()??;
    Some(replaced(&s.dict, "DCTDecode", jpeg))
}

#[derive(PartialEq)]
enum Filter {
    Dct,
    Jxl,
}

fn is_image(s: &Stream) -> bool {
    s.dict.name(b"Subtype") == Some(b"Image") && !s.dict.contains(b"F") && !matches!(s.dict.get(b"ImageMask"), Some(Object::Bool(true)))
}

/// The one filter of the image, when it is DCT or JXL with no parameters (a `/ColorTransform` or
/// anything else there could make readers disagree about the colours, so such images are left).
fn filter_of(d: &Dict) -> Option<Filter> {
    if !matches!(d.get(b"DecodeParms"), None | Some(Object::Null)) {
        return None;
    }
    match d.get(b"Filter")? {
        Object::Name(n) => match n.as_slice() {
            b"DCTDecode" | b"DCT" => Some(Filter::Dct),
            b"JXLDecode" => Some(Filter::Jxl),
            _ => None,
        },
        _ => None,
    }
}

fn replaced(old: &Dict, filter: &str, data: Vec<u8>) -> Stream {
    let mut d = old.clone();
    d.remove(b"Length");
    d.remove(b"DecodeParms");
    d.set(b"Filter".to_vec(), Object::name(filter));
    Stream::from_raw(d, data)
}
