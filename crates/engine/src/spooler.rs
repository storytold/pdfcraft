//! The print-ready PDF as the system spooler gets it.

use std::sync::Arc;

use pdfcraft_cos::{SaveOptions, write_full};

use crate::guard;

/// The print-ready PDF `bytes` ([`crate::Session::print_pdf`]) as they go to the system spooler
/// with `job`. Two things happen on Windows only; elsewhere the bytes go as they are.
///
/// - A job in grayscale is made grey here first ([`greyed`]), and `job.grey_pages` says so.
///   Windows has no colour conversion of its own, and a driver that lists no grey option (Microsoft
///   Print to PDF, or one with only a vendor's own grey setting) refused the job outright
///   (#5806). The driver is still asked for grey where it has it.
/// - Large images are drawn as tiles ([`pdfcraft_optimize::tile_large_images`], #5131): Windows'
///   PDF renderer prints an image of more than 2²² pixels from a shrunk copy, so a scan comes out
///   at the printer's resolution.
///
/// When the copy can't be read or written, the bytes go as they are: neither step ever stops a
/// print, and a grayscale job whose pages couldn't be made grey keeps asking the driver for grey,
/// strictly, as before.
pub fn spooler_copy(bytes: Vec<u8>, job: &mut crate::print::spool::Job) -> Vec<u8> {
    if !cfg!(windows) {
        return bytes;
    }
    let mut bytes = Arc::new(bytes);
    if job.grayscale {
        match greyed(bytes.clone()) {
            Some(Some(grey)) => {
                bytes = Arc::new(grey);
                job.grey_pages = true;
            }
            Some(None) => job.grey_pages = true,
            None => {}
        }
    }
    // Once `tiled` returns, its document is gone and `bytes` is the only owner left: no copy.
    tiled(bytes.clone()).unwrap_or_else(|| Arc::try_unwrap(bytes).unwrap_or_else(|shared| (*shared).clone()))
}

/// `bytes` with every colour on every page, its comments and form fields included, turned grey by
/// Convert colours (black kept black, pictures re-encoded where they can be): `Some(Some(_))`.
/// `Some(None)` when nothing needed converting, and `None` when the copy can't be read or written.
///
/// Convert colours leaves shadings, patterns, inline images and pictures it can't re-encode
/// (indexed, CMYK, JPEG 2000 …) as they are; the driver is still asked for grey, so a driver with a
/// grey option makes those grey too, and one without prints them as they are.
pub fn greyed(bytes: Arc<Vec<u8>>) -> Option<Option<Vec<u8>>> {
    guard(|| {
        let mut doc = pdfcraft_cos::Document::open(bytes).ok()?;
        let pages: Vec<usize> = (0..pdfcraft_model::pages(&doc).len()).collect();
        let settings =
            pdfcraft_edit::ConvertColours { target: pdfcraft_edit::ColourTarget::Gray, preserve_black: true, images: true, annotations: true };
        let mut recolour = |doc: &pdfcraft_cos::Document, s: &pdfcraft_cos::Stream, _: pdfcraft_edit::ColourTarget| {
            pdfcraft_optimize::recolour_image(doc, s, pdfcraft_optimize::ImageTarget::Gray)
        };
        let done = pdfcraft_edit::convert_colours(&mut doc, &pages, &settings, &mut recolour).ok()?;
        if done.colours == 0 && done.images == 0 {
            return Some(None);
        }
        write_full(&doc, &SaveOptions::default()).ok().map(Some)
    })
    .ok()
    .flatten()
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
    use crate::print::spool::Job;

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
        let mut job = Job::default();
        assert_eq!(spooler_copy(b"not a PDF".to_vec(), &mut job), b"not a PDF");
        let small = page_with_image(100, 100);
        assert_eq!(spooler_copy(small.clone(), &mut job), small);
        assert!(!job.grey_pages);
        // A grayscale job whose copy can't be read asks the driver for grey as before.
        let mut grey = Job { grayscale: true, ..Job::default() };
        assert_eq!(spooler_copy(b"not a PDF".to_vec(), &mut grey), b"not a PDF");
        assert!(!grey.grey_pages);
    }

    /// A one-page PDF filling the page in red.
    fn red_page() -> Vec<u8> {
        let mut doc = Document::new_empty();
        let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
        let content = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"1 0 0 rg 0 0 612 792 re f".to_vec())));
        let mut page = Dict::new();
        page.set("Type", Object::name("Page"));
        page.set("Parent", Object::Ref(pages));
        page.set("MediaBox", Object::Array([0, 0, 612, 792].map(Object::Int).to_vec()));
        page.set("Contents", Object::Ref(content));
        let page = doc.add(Object::Dict(page));
        doc.update_dict(pages, |d| {
            d.set("Count", Object::Int(1));
            d.set("Kids", Object::Array(vec![Object::Ref(page)]));
        })
        .unwrap();
        write_full(&doc, &SaveOptions::default()).unwrap()
    }

    /// The operators of the first page's content.
    fn first_page_content(bytes: Vec<u8>) -> String {
        let doc = Document::open(Arc::new(bytes)).unwrap();
        let page = pdfcraft_model::pages(&doc)[0].obj;
        let contents = doc.get(page).as_dict().and_then(|d| d.reference(b"Contents")).unwrap();
        match &*doc.get(contents) {
            Object::Stream(s) => String::from_utf8_lossy(&s.decoded().unwrap()).into_owned(),
            other => panic!("{other:?}"),
        }
    }

    /// #5806: a page in colour is made grey for a grayscale job, which says so.
    #[test]
    fn a_colour_page_is_made_grey_for_a_grayscale_job() {
        let grey = greyed(Arc::new(red_page())).expect("readable").expect("converted");
        let content = first_page_content(grey.clone());
        assert!(!content.contains(" rg"), "{content}");
        assert!(content.contains(" g"), "{content}");
        // Already grey: nothing to convert, and the pages are still grey.
        assert_eq!(greyed(Arc::new(grey)), Some(None));
        assert_eq!(greyed(Arc::new(b"not a PDF".to_vec())), None);
    }

    /// On Windows a grayscale job's copy is grey and the job says its pages are; elsewhere the
    /// bytes go as they are and the driver is asked, as before. A colour job is never converted.
    #[test]
    fn a_grayscale_job_sends_grey_pages_on_windows_only() {
        let red = red_page();
        let mut job = Job { grayscale: true, ..Job::default() };
        let sent = spooler_copy(red.clone(), &mut job);
        if cfg!(windows) {
            assert!(job.grey_pages);
            assert!(!first_page_content(sent).contains(" rg"));
        } else {
            assert!(!job.grey_pages);
            assert_eq!(sent, red);
        }
        let mut colour = Job::default();
        assert_eq!(spooler_copy(red.clone(), &mut colour), red);
        assert!(!colour.grey_pages);
    }

    /// On Windows the spooler gets the tiled copy ([`tiled`]); elsewhere the bytes as they are.
    #[test]
    fn the_spooler_gets_tiles_on_windows_only() {
        let large = page_with_image(2100, 2100);
        let sent = spooler_copy(large.clone(), &mut Job::default());
        if cfg!(windows) {
            assert_eq!(Some(&sent), tiled(Arc::new(large.clone())).as_ref());
            assert_ne!(sent, large);
        } else {
            assert_eq!(sent, large);
        }
    }
}
