//! The print-ready PDF as the system spooler gets it.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Object, SaveOptions, Stream, write_full};
use pdfcraft_render::{PRINT_TAG, PageRenderer, RenderConfig, RenderRequest, Tile};

use crate::guard;

/// The resolution [`spooler_copy`] flattens a sheet at, unless the sheet is too large for it
/// ([`MAX_FLATTENED_PIXELS`], [`MAX_FLATTENED_SIDE`]).
pub const FLATTEN_DPI: f64 = 300.0;
/// The side of the largest tile a flattened sheet is drawn with, in pixels: 2000 × 2000 stays
/// under the 2²² pixels past which Windows' PDF renderer shrinks an image (#5131), so [`tiled`]
/// leaves the tiles alone, and under the renderer's own tile limit.
pub const FLATTEN_TILE: u32 = 2000;
/// The most pixels one flattened sheet has. A 24 × 36 in sheet at 300 dpi (78 MP) keeps its
/// resolution; a larger one is drawn at a lower one. The sheet is rendered and encoded a tile at
/// a time, so this bounds the encoded picture (240 MB as raw RGB) and the time it takes, not the
/// memory one render needs.
pub const MAX_FLATTENED_PIXELS: f64 = 80.0e6;
/// The most pixels along a flattened sheet's longer side: a long banner gets a lower resolution
/// rather than hundreds of tiles, each of which interprets the whole sheet's content again.
pub const MAX_FLATTENED_SIDE: f64 = 16_000.0;
/// The most bytes of encoded pictures one copy holds. A sheet whose picture would pass it keeps
/// its vectors and prints as it would have without flattening.
const MAX_FLATTENED_BYTES: usize = 512 << 20;

/// The print-ready PDF `bytes` ([`crate::Session::print_pdf`]) as they go to the system spooler
/// with `job`. Three things happen on Windows only, in this order; elsewhere the bytes go as they
/// are.
///
/// - A job in grayscale is made grey here first ([`greyed`]), and `job.grey_pages` says so.
///   Windows has no colour conversion of its own, and a driver that lists no grey option (Microsoft
///   Print to PDF, or one with only a vendor's own grey setting) refused the job outright
///   (#5806). The driver is still asked for grey where it has it.
/// - With `flatten_transparency`, each sheet that uses transparency becomes an opaque picture of
///   itself at [`FLATTEN_DPI`] ([`flattened`]). It is meant for GDI (v3) drivers, which get the
///   job through Windows' XPS-to-GDI conversion: that conversion fails on soft masks (#6155),
///   prints halos around them (#5838) or prints transparency as black boxes (#5940). It comes
///   after greying, so the picture is of the grey sheet.
/// - Large images are drawn as tiles ([`pdfcraft_optimize::tile_large_images`], #5131): Windows'
///   PDF renderer prints an image of more than 2²² pixels from a shrunk copy, so a scan comes out
///   at the printer's resolution. A flattened sheet's tiles are already under that.
///
/// When the copy can't be read or written, the bytes go as they are: no step ever stops a print,
/// and a grayscale job whose pages couldn't be made grey keeps asking the driver for grey,
/// strictly, as before.
pub fn spooler_copy(bytes: Vec<u8>, job: &mut crate::print::spool::Job, flatten_transparency: bool) -> Vec<u8> {
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
    if flatten_transparency && let Some(flat) = flattened(bytes.clone(), FLATTEN_DPI) {
        bytes = Arc::new(flat);
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

/// `bytes` with each page that uses transparency ([`pdfcraft_preflight::page_uses_transparency`])
/// replaced by an opaque picture of itself, or `None` when no page needs it, none could be
/// rendered, or the copy can't be read or written. Pages without transparency stay as they are.
///
/// The picture is at `dpi` (72 to 600; [`FLATTEN_DPI`] if not a number), lower on a sheet past
/// [`MAX_FLATTENED_PIXELS`] or [`MAX_FLATTENED_SIDE`], in tiles of at most [`FLATTEN_TILE`]
/// pixels a side, each one image (RGB over white, Flate) rendered and encoded on its own. The
/// page keeps its boxes, `/Rotate` and `/UserUnit`, so the sheet keeps its size and orientation.
/// Its transparency group goes, since nothing on it is transparent any more, and so do its
/// annotations, which the picture already shows. A print-ready PDF has none:
/// [`pdfcraft_print::impose`] draws them into the sheet.
///
/// A page whose picture can't be rendered keeps its vectors, as does one whose picture would take
/// the pictures past 512 MB encoded.
pub fn flattened(bytes: Arc<Vec<u8>>, dpi: f64) -> Option<Vec<u8>> {
    guard(|| {
        let mut doc = pdfcraft_cos::Document::open(bytes.clone()).ok()?;
        let pages = pdfcraft_model::pages(&doc);
        let transparent: Vec<usize> =
            pages.iter().enumerate().filter(|(_, p)| pdfcraft_preflight::page_uses_transparency(&doc, &p.dict)).map(|(i, _)| i).collect();
        if transparent.is_empty() {
            return None;
        }
        // As Print as image renders (`print_raster`): printing is not the View event, comments
        // are drawn, and the sheet clears to white; `PRINT_TAG` turns reading colours off. The
        // print-ready PDF already holds the layers as they print.
        let config = RenderConfig { for_print: true, ..RenderConfig::default() };
        let mut renderer = PageRenderer::new(bytes, config);
        // The renderer must list the same pages, or a sheet could get another one's picture.
        if renderer.page_count() != pages.len() {
            return None;
        }
        let dpi = if dpi.is_finite() { dpi.clamp(72.0, 600.0) } else { FLATTEN_DPI };
        let mut budget = MAX_FLATTENED_BYTES;
        let mut done = 0usize;
        for i in transparent {
            let Some(page) = pages.get(i) else { continue };
            let Some(picture) = sheet_picture(&doc, &mut renderer, page, i, dpi, &mut budget) else { continue };
            let mut xobjects = Dict::new();
            for (name, image) in picture.tiles {
                xobjects.set(name, Object::Ref(doc.add(Object::Stream(image))));
            }
            let mut resources = Dict::new();
            resources.set("XObject", Object::Dict(xobjects));
            let contents = doc.add(Object::Stream(Stream::flate(Dict::new(), &picture.content)));
            let replaced = doc.update_dict(page.obj, |d| {
                d.set("Contents", Object::Ref(contents));
                d.set("Resources", Object::Dict(resources));
                d.remove(b"Annots");
                d.remove(b"Group");
            });
            if replaced.is_ok() {
                done += 1;
            }
        }
        (done > 0).then(|| write_full(&doc, &SaveOptions::default()).ok()).flatten()
    })
    .ok()
    .flatten()
}

/// A page drawn as tiles: the content that draws them, and each tile's resource name and image.
struct Picture {
    content: Vec<u8>,
    tiles: Vec<(Vec<u8>, Stream)>,
}

/// The scale (pixels per point) a `w` × `h` pt sheet is flattened at for `dpi`: lowered so the
/// sheet stays within [`MAX_FLATTENED_PIXELS`] and [`MAX_FLATTENED_SIDE`].
fn flatten_scale(w: f64, h: f64, dpi: f64) -> f64 {
    let (w, h) = (w.max(1.0), h.max(1.0));
    (dpi / 72.0).min((MAX_FLATTENED_PIXELS / (w * h)).sqrt()).min(MAX_FLATTENED_SIDE / w.max(h))
}

/// Page `index` of `doc` rendered by `renderer` as tiles at `dpi` (see [`flattened`]), with the
/// content that draws them where the page shows. `None` when a tile doesn't render, or the
/// tiles' encoded size would pass what is left of `budget`, which a page that succeeds is
/// charged.
fn sheet_picture(
    doc: &pdfcraft_cos::Document,
    renderer: &mut PageRenderer,
    page: &pdfcraft_model::Page,
    index: usize,
    dpi: f64,
    budget: &mut usize,
) -> Option<Picture> {
    // The renderer draws the page as displayed (crop box, `/UserUnit`, `/Rotate`), y down; the
    // view matrix takes that display space (y up) back to the page's user space.
    let (w, h) = page.display_size(doc);
    let scale = flatten_scale(w, h, dpi) as f32;
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    let (width, height) = (pdfcraft_render::device_pixels(w as f32, scale), pdfcraft_render::device_pixels(h as f32, scale));
    let s = f64::from(scale);
    let mut content = format!("q {} cm\n", matrix(page.view_matrix(doc)));
    let mut tiles = Vec::new();
    let mut used = 0usize;
    for y in (0..height).step_by(FLATTEN_TILE as usize) {
        for x in (0..width).step_by(FLATTEN_TILE as usize) {
            let (tw, th) = (FLATTEN_TILE.min(width - x), FLATTEN_TILE.min(height - y));
            let r =
                renderer.render(RenderRequest { page: index, scale, tile: Some(Tile { x, y, w: tw, h: th }), tag: PRINT_TAG, ..Default::default() });
            let pixels = (tw as usize) * (th as usize);
            if r.error.is_some() || r.width != tw || r.height != th || r.rgba.len() != pixels * 4 {
                return None;
            }
            let mut rgba = r.rgba.to_vec();
            crate::print_raster::over_white(&mut rgba, false);
            let rgb: Vec<u8> = rgba.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
            let mut image = Dict::new();
            image.set("Type", Object::name("XObject"));
            image.set("Subtype", Object::name("Image"));
            image.set("Width", Object::Int(i64::from(tw)));
            image.set("Height", Object::Int(i64::from(th)));
            image.set("ColorSpace", Object::name("DeviceRGB"));
            image.set("BitsPerComponent", Object::Int(8));
            let image = Stream::flate(image, &rgb);
            used = used.saturating_add(image.raw.len());
            if used > *budget {
                return None;
            }
            // The tile's place in display space, y up: its top edge is `y` pixels below the top.
            // The last row and column may reach past the page by under a pixel, which the crop
            // box clips.
            let name = format!("PCFlat{}", tiles.len());
            let place = [f64::from(tw) / s, 0.0, 0.0, f64::from(th) / s, f64::from(x) / s, h - f64::from(y + th) / s];
            content.push_str(&format!("q {} cm /{name} Do Q\n", matrix(place)));
            tiles.push((name.into_bytes(), image));
        }
    }
    content.push_str("Q\n");
    *budget -= used;
    Some(Picture { content: content.into_bytes(), tiles })
}

/// `m` as the six operands of `cm`, at full precision.
fn matrix(m: [f64; 6]) -> String {
    let mut out = Vec::new();
    for (i, v) in m.into_iter().enumerate() {
        if i > 0 {
            out.push(b' ');
        }
        pdfcraft_cos::serialize(&Object::Real(v), &mut out);
    }
    String::from_utf8_lossy(&out).into_owned()
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
        assert_eq!(spooler_copy(b"not a PDF".to_vec(), &mut job, false), b"not a PDF");
        let small = page_with_image(100, 100);
        assert_eq!(spooler_copy(small.clone(), &mut job, false), small);
        assert!(!job.grey_pages);
        // A grayscale job whose copy can't be read asks the driver for grey as before.
        let mut grey = Job { grayscale: true, ..Job::default() };
        assert_eq!(spooler_copy(b"not a PDF".to_vec(), &mut grey, false), b"not a PDF");
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
        let sent = spooler_copy(red.clone(), &mut job, false);
        if cfg!(windows) {
            assert!(job.grey_pages);
            assert!(!first_page_content(sent).contains(" rg"));
        } else {
            assert!(!job.grey_pages);
            assert_eq!(sent, red);
        }
        let mut colour = Job::default();
        assert_eq!(spooler_copy(red.clone(), &mut colour, false), red);
        assert!(!colour.grey_pages);
    }

    /// On Windows the spooler gets the tiled copy ([`tiled`]); elsewhere the bytes as they are.
    #[test]
    fn the_spooler_gets_tiles_on_windows_only() {
        let large = page_with_image(2100, 2100);
        let sent = spooler_copy(large.clone(), &mut Job::default(), false);
        if cfg!(windows) {
            assert_eq!(Some(&sent), tiled(Arc::new(large.clone())).as_ref());
            assert_ne!(sent, large);
        } else {
            assert_eq!(sent, large);
        }
    }

    /// A PDF with the pages `make` returns, each given its type, its parent and, when it has
    /// none, a US Letter MediaBox. `make` adds what the pages refer to.
    fn pdf_of(make: impl FnOnce(&mut Document) -> Vec<Dict>) -> Vec<u8> {
        let mut doc = Document::new_empty();
        let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages"))).unwrap();
        let made = make(&mut doc);
        let count = made.len() as i64;
        let kids: Vec<Object> = made
            .into_iter()
            .map(|mut page| {
                page.set("Type", Object::name("Page"));
                page.set("Parent", Object::Ref(pages));
                if !page.contains(b"MediaBox") {
                    page.set("MediaBox", Object::Array([0, 0, 612, 792].map(Object::Int).to_vec()));
                }
                Object::Ref(doc.add(Object::Dict(page)))
            })
            .collect();
        doc.update_dict(pages, |d| {
            d.set("Count", Object::Int(count));
            d.set("Kids", Object::Array(kids));
        })
        .unwrap();
        write_full(&doc, &SaveOptions::default()).unwrap()
    }

    /// A page drawing `content` with `resources`.
    fn page(doc: &mut Document, resources: Dict, content: &[u8]) -> Dict {
        let content = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.to_vec())));
        let mut page = Dict::new();
        page.set("Resources", Object::Dict(resources));
        page.set("Contents", Object::Ref(content));
        page
    }

    /// An image XObject dictionary of `w` × `h` 8-bit samples in `space`.
    fn image_dict(w: i64, h: i64, space: &str) -> Dict {
        let mut image = Dict::new();
        image.set("Type", Object::name("XObject"));
        image.set("Subtype", Object::name("Image"));
        image.set("Width", Object::Int(w));
        image.set("Height", Object::Int(h));
        image.set("ColorSpace", Object::name(space));
        image.set("BitsPerComponent", Object::Int(8));
        image
    }

    /// A blue square, the content of a page without transparency.
    const BLUE_SQUARE: &[u8] = b"0 0 1 rg 100 100 200 200 re f";

    /// A page without transparency: a blue square.
    fn vector_page(doc: &mut Document) -> Dict {
        page(doc, Dict::new(), BLUE_SQUARE)
    }

    /// A page drawing a red image over a blue square, with a soft mask that fades the image in
    /// from left to right.
    fn soft_masked_page(doc: &mut Document) -> Dict {
        let alpha: Vec<u8> = (0..64 * 64).map(|i| (i % 64 * 4) as u8).collect();
        let mask = doc.add(Object::Stream(Stream::flate(image_dict(64, 64, "DeviceGray"), &alpha)));
        let mut image = image_dict(64, 64, "DeviceRGB");
        image.set("SMask", Object::Ref(mask));
        let red: Vec<u8> = (0..64 * 64).flat_map(|_| [255, 0, 0]).collect();
        let image = doc.add(Object::Stream(Stream::flate(image, &red)));
        let mut xobjects = Dict::new();
        xobjects.set("Im", Object::Ref(image));
        let mut resources = Dict::new();
        resources.set("XObject", Object::Dict(xobjects));
        page(doc, resources, b"0 0 1 rg 100 100 200 200 re f q 300 0 0 300 150 150 cm /Im Do Q")
    }

    /// Page `n`'s dictionary, inherited entries filled in, and the operators of its content.
    fn page_of(doc: &Document, n: usize) -> (Dict, String) {
        let page = pdfcraft_model::pages(doc).swap_remove(n);
        let contents = page.dict.reference(b"Contents").unwrap();
        let content = match &*doc.get(contents) {
            Object::Stream(s) => String::from_utf8_lossy(&s.decoded().unwrap()).into_owned(),
            other => panic!("{other:?}"),
        };
        (page.dict, content)
    }

    /// The images among page `n`'s XObjects, as (width, height, has a soft mask), sorted.
    fn page_images(doc: &Document, n: usize) -> Vec<(i64, i64, bool)> {
        let (page, _) = page_of(doc, n);
        let resources = doc.resolve(page.get(b"Resources").unwrap());
        let xobjects = doc.resolve(resources.as_dict().unwrap().get(b"XObject").unwrap());
        let mut images: Vec<(i64, i64, bool)> = xobjects
            .as_dict()
            .unwrap()
            .iter()
            .filter_map(|(_, x)| match &*doc.resolve(x) {
                Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") => {
                    Some((s.dict.int(b"Width")?, s.dict.int(b"Height")?, s.dict.contains(b"SMask")))
                }
                _ => None,
            })
            .collect();
        images.sort();
        images
    }

    /// #6155, #5838, #5940: a page with a soft-masked image becomes an opaque picture in tiles
    /// under Windows' image cap, on a sheet of the same size, and tiling leaves them alone.
    #[test]
    fn a_soft_masked_page_is_flattened_into_tiles() {
        let flat = flattened(Arc::new(pdf_of(|doc| vec![soft_masked_page(doc)])), FLATTEN_DPI).expect("flattened");
        let doc = Document::open(Arc::new(flat.clone())).unwrap();
        // US Letter at 300 dpi is 2550 × 3300 pixels: two tiles across, two down.
        assert_eq!(page_images(&doc, 0), [(550, 1300, false), (550, 2000, false), (2000, 1300, false), (2000, 2000, false)]);
        assert!(doc.scan_objects().all(|(_, o)| !matches!(&*o, Object::Stream(s) if s.dict.contains(b"SMask"))));
        let (page, content) = page_of(&doc, 0);
        assert_eq!(page.get(b"MediaBox"), Some(&Object::Array([0, 0, 612, 792].map(Object::Int).to_vec())));
        assert_eq!(content.matches(" Do").count(), 4, "{content}");
        assert!(!pdfcraft_preflight::page_uses_transparency(&doc, &page));
        assert_eq!(tiled(Arc::new(flat)), None);
    }

    /// A page without transparency, or with a transparency group around opaque content only (as
    /// a print-ready sheet draws a page that Word gave one), keeps its vectors; so does a copy
    /// that can't be read.
    #[test]
    fn a_page_without_transparency_or_unreadable_is_not_flattened() {
        assert_eq!(flattened(Arc::new(pdf_of(|doc| vec![vector_page(doc)])), FLATTEN_DPI), None);
        assert_eq!(flattened(Arc::new(page_with_image(100, 100)), FLATTEN_DPI), None);
        let grouped = pdf_of(|doc| {
            let mut group = Dict::new();
            group.set("S", Object::name("Transparency"));
            let mut form = Dict::new();
            form.set("Type", Object::name("XObject"));
            form.set("Subtype", Object::name("Form"));
            form.set("BBox", Object::Array([0, 0, 612, 792].map(Object::Int).to_vec()));
            form.set("Group", Object::Dict(group));
            let form = doc.add(Object::Stream(Stream::from_raw(form, BLUE_SQUARE.to_vec())));
            let mut xobjects = Dict::new();
            xobjects.set("Fm", Object::Ref(form));
            let mut resources = Dict::new();
            resources.set("XObject", Object::Dict(xobjects));
            vec![page(doc, resources, b"/Fm Do")]
        });
        assert_eq!(flattened(Arc::new(grouped), FLATTEN_DPI), None);
        assert_eq!(flattened(Arc::new(b"not a PDF".to_vec()), FLATTEN_DPI), None);
    }

    /// Of two pages, only the one with transparency becomes a picture; the other keeps its
    /// content as it was.
    #[test]
    fn only_the_transparent_page_of_two_is_flattened() {
        let source = pdf_of(|doc| vec![vector_page(doc), soft_masked_page(doc)]);
        let flat = flattened(Arc::new(source), 150.0).expect("flattened");
        let doc = Document::open(Arc::new(flat)).unwrap();
        assert_eq!(page_of(&doc, 0).1, String::from_utf8_lossy(BLUE_SQUARE));
        // US Letter at 150 dpi is 1275 × 1650 pixels: one tile.
        assert_eq!(page_images(&doc, 1), [(1275, 1650, false)]);
        assert_eq!(page_of(&doc, 1).1.matches(" Do").count(), 1);
    }

    /// The picture lands where the page showed what it pictures, on a page turned a quarter
    /// with a crop box away from the origin.
    #[test]
    fn a_flattened_turned_page_looks_as_it_did() {
        let source = pdf_of(|doc| {
            let mut half = Dict::new();
            half.set("ca", Object::Real(0.5));
            let mut states = Dict::new();
            states.set("Half", Object::Dict(half));
            let mut resources = Dict::new();
            resources.set("ExtGState", Object::Dict(states));
            // Half-transparent red over the crop box's lower left 100 × 100 pt.
            let mut page = page(doc, resources, b"/Half gs 1 0 0 rg 50 0 100 100 re f");
            page.set("MediaBox", Object::Array([0, 0, 300, 200].map(Object::Int).to_vec()));
            page.set("CropBox", Object::Array([50, 0, 300, 200].map(Object::Int).to_vec()));
            page.set("Rotate", Object::Int(90));
            vec![page]
        });
        let flat = flattened(Arc::new(source.clone()), 72.0).expect("flattened");
        let (page, _) = page_of(&Document::open(Arc::new(flat.clone())).unwrap(), 0);
        assert_eq!(page.get(b"Rotate"), Some(&Object::Int(90)));
        assert_eq!(page.get(b"CropBox"), Some(&Object::Array([50, 0, 300, 200].map(Object::Int).to_vec())));
        let render = |bytes: Vec<u8>| {
            let r = PageRenderer::new(Arc::new(bytes), RenderConfig::default()).render(RenderRequest {
                page: 0,
                scale: 1.0,
                tag: PRINT_TAG,
                ..Default::default()
            });
            assert!(r.error.is_none(), "{:?}", r.error);
            r
        };
        let (before, after) = (render(source), render(flat));
        // Turned a quarter clockwise, the page shows 200 × 250 pt with the red at its top left.
        assert_eq!((before.width, before.height), (200, 250));
        assert_eq!((after.width, after.height), (200, 250));
        let px = |r: &pdfcraft_render::RenderedPage, x: u32, y: u32| {
            let i = ((y * r.width + x) * 4) as usize;
            [r.rgba[i], r.rgba[i + 1], r.rgba[i + 2]]
        };
        let pink = px(&before, 50, 50);
        assert!(pink[0] > 240 && (100..160).contains(&pink[1]), "{pink:?}");
        assert_eq!(px(&before, 150, 200), [255, 255, 255]);
        // Sampled away from the red's edges (at 100 px), where pixels may blend.
        for y in (5..250).step_by(10) {
            for x in (5..200).step_by(10) {
                let (b, a) = (px(&before, x, y), px(&after, x, y));
                assert!(b.iter().zip(a).all(|(b, a)| b.abs_diff(a) <= 3), "({x}, {y}): {b:?} became {a:?}");
            }
        }
    }

    /// A sheet keeps the full resolution up to 24 × 36 in; a larger one gets a lower one within
    /// the pixel bound, and a long banner one within the side bound.
    #[test]
    fn a_large_sheet_is_flattened_at_a_lower_resolution() {
        let dpi = |w: f64, h: f64| flatten_scale(w * 72.0, h * 72.0, FLATTEN_DPI) * 72.0;
        assert!((dpi(8.5, 11.0) - 300.0).abs() < 1e-9);
        assert!((dpi(24.0, 36.0) - 300.0).abs() < 1e-9);
        let large = dpi(36.0, 48.0);
        assert!(large < 300.0 && 36.0 * 48.0 * large * large <= MAX_FLATTENED_PIXELS * 1.000_001, "{large}");
        assert!((dpi(36.0, 1200.0) * 1200.0 - MAX_FLATTENED_SIDE).abs() < 1e-6);
    }

    /// With the flag off a transparent page goes to the spooler as it is; on, Windows gets the
    /// flattened copy.
    #[test]
    fn the_spooler_flattens_transparency_only_when_asked() {
        let source = pdf_of(|doc| vec![soft_masked_page(doc)]);
        assert_eq!(spooler_copy(source.clone(), &mut Job::default(), false), source);
        let sent = spooler_copy(source.clone(), &mut Job::default(), true);
        if cfg!(windows) {
            assert_eq!(Some(&sent), flattened(Arc::new(source), FLATTEN_DPI).as_ref());
        } else {
            assert_eq!(sent, source);
        }
    }
}
