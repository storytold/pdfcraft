//! The print-ready PDF as the system spooler gets it.

use std::sync::Arc;

use pdfcraft_cos::{SaveOptions, write_full};

use crate::guard;

/// The print-ready PDF `bytes` ([`crate::Session::print_pdf`]) as they go to the system spooler.
/// On Windows, whose PDF renderer prints an image of more than 2²² pixels from a shrunk copy,
/// large images are drawn as tiles ([`pdfcraft_optimize::tile_large_images`], #5131), so that a
/// scan prints at the printer's resolution. Elsewhere, with no large image, and when the copy
/// can't be read or written, the bytes go as they are: tiling never stops a print.
pub fn spooler_copy(bytes: Vec<u8>) -> Vec<u8> {
    if !cfg!(windows) {
        return bytes;
    }
    let bytes = Arc::new(bytes);
    // Once `tiled` returns, its document is gone and `bytes` is the only owner left: no copy.
    tiled(bytes.clone()).unwrap_or_else(|| Arc::try_unwrap(bytes).unwrap_or_else(|shared| (*shared).clone()))
}

/// `bytes` with their large images drawn as tiles, or `None` when there is none, or the copy
/// can't be read or written.
pub fn tiled(bytes: Arc<Vec<u8>>) -> Option<Vec<u8>> {
    guard(|| {
        let mut doc = pdfcraft_cos::Document::open(bytes).ok()?;
        let report = pdfcraft_optimize::tile_large_images(&mut doc);
        (report.tiled > 0).then(|| write_full(&doc, &SaveOptions::default()).ok()).flatten()
    })
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use pdfcraft_cos::{Dict, Document, Object, Stream};

    use super::*;

    /// A one-page PDF drawing a `w` × `h` grey image over the page.
    fn page_with_image(w: i64, h: i64) -> Vec<u8> {
        let mut doc = Document::new_empty();
        let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
        let mut image = Dict::new();
        image.set("Type", Object::name("XObject"));
        image.set("Subtype", Object::name("Image"));
        image.set("Width", Object::Int(w));
        image.set("Height", Object::Int(h));
        image.set("ColorSpace", Object::name("DeviceGray"));
        image.set("BitsPerComponent", Object::Int(8));
        let samples: Vec<u8> = (0..w * h).map(|i| (i % 251) as u8).collect();
        let image = doc.add(Object::Stream(Stream::flate(image, &samples)));
        let mut xobjects = Dict::new();
        xobjects.set("Im", Object::Ref(image));
        let mut resources = Dict::new();
        resources.set("XObject", Object::Dict(xobjects));
        let content = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"q 612 0 0 792 0 0 cm /Im Do Q".to_vec())));
        let mut page = Dict::new();
        page.set("Type", Object::name("Page"));
        page.set("Parent", Object::Ref(pages));
        page.set("MediaBox", Object::Array([0, 0, 612, 792].map(Object::Int).to_vec()));
        page.set("Resources", Object::Dict(resources));
        page.set("Contents", Object::Ref(content));
        let page = doc.add(Object::Dict(page));
        doc.update_dict(pages, |d| {
            d.set("Count", Object::Int(1));
            d.set("Kids", Object::Array(vec![Object::Ref(page)]));
        })
        .unwrap();
        write_full(&doc, &SaveOptions::default()).unwrap()
    }

    #[test]
    fn a_large_image_goes_to_the_spooler_as_tiles() {
        let tiled = tiled(Arc::new(page_with_image(2100, 2100))).expect("tiled");
        let doc = Document::open(Arc::new(tiled)).unwrap();
        let largest = doc
            .scan_objects()
            .filter_map(|(_, o)| match &*o {
                Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") => Some(s.dict.int(b"Width")? * s.dict.int(b"Height")?),
                _ => None,
            })
            .max();
        // The page's image is a form now; its tiles are the images it draws.
        assert_eq!(largest, Some(2001 * 2001));
    }

    #[test]
    fn a_print_with_nothing_to_tile_or_unreadable_goes_as_it_is() {
        assert_eq!(tiled(Arc::new(page_with_image(100, 100))), None);
        assert_eq!(tiled(Arc::new(b"not a PDF".to_vec())), None);
        assert_eq!(spooler_copy(b"not a PDF".to_vec()), b"not a PDF");
        let small = page_with_image(100, 100);
        assert_eq!(spooler_copy(small.clone()), small);
    }
}
