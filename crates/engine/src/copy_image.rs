//! Copy Image: find the picture under a point and hand it to the clipboard, as pixels or as a
//! file in its original format. A picture drawn through grouped
//! Form artwork (as Word, InDesign and many scanners write them) is found inside the group.

use std::collections::{HashMap, HashSet};
use std::io::Write;

use pdfcraft_cos::{Dict, Document as Cos, ObjRef, Object};

use crate::Document;

/// Clipboards copy the whole bitmap; past this many pixels a pixel copy is refused.
const MAX_COPY_PIXELS: u64 = 64_000_000;
/// How deep Form artwork is searched for its picture.
const MAX_FORM_DEPTH: usize = 12;

/// A picture on a page, with its index in [`Document::page_images`].
pub type PagePicture = (usize, pdfcraft_edit::PageImage);

/// The topmost of `pictures` under user-space point `at` (`[x, y]`). `pictures` is bottom first,
/// as [`Document::page_pictures_job`] lists them, so the last one that holds `at` wins.
pub fn picture_at(pictures: &[PagePicture], at: [f64; 2]) -> Option<&PagePicture> {
    pictures.iter().rev().find(|(_, image)| image.contains(at))
}

/// The pictures on `page`, bottom first: raster images, and Form artwork that holds one.
fn pictures(cos: &Cos, page: usize) -> Vec<PagePicture> {
    let images = match pdfcraft_edit::page_images(cos, page) {
        Ok(images) => images,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "page {} picture selection unavailable: {e}", page.saturating_add(1));
            return Vec::new();
        }
    };
    if !images.iter().any(|image| image.is_form) {
        return images.into_iter().enumerate().collect();
    }
    let drawn = match pdfcraft_edit::reading_images(cos, page) {
        Ok(drawn) => drawn,
        Err(e) => {
            // Fail closed for groups whose bitmap bounds cannot be proved; direct images remain usable.
            let _ = writeln!(std::io::stderr(), "page {} grouped picture selection skipped: {e}", page.saturating_add(1));
            return images.into_iter().enumerate().filter(|(_, image)| !image.is_form).collect();
        }
    };
    // Index placements once: reused image objects and page-sized CAD groups are not picture bounds.
    let sources: HashMap<_, _> = images.iter().enumerate().map(|(index, image)| (image.draw_source(), index)).collect();
    drawn.into_iter().filter_map(|image| sources.get(&image.draw_source()).map(|&index| (index, image))).collect()
}

impl Document {
    /// The topmost picture on `page` under user-space point `at`, as its index in
    /// [`Document::page_images`]: a raster image, or Form artwork that holds one.
    pub fn page_image_at(&self, page: usize, at: [f64; 2]) -> Option<usize> {
        let editor = self.editor.as_ref()?;
        picture_at(&pictures(&editor.cos, page), at).map(|(index, _)| *index)
    }

    /// Work that lists the pictures on `page`, bottom first, for another thread: it reads every
    /// drawing operator on the page, seconds on a dense one, so it must not run on the window
    /// thread. Look a point up with [`picture_at`].
    ///
    /// `None` when the document can't be read. The work reads the document as it is now: later
    /// edits aren't seen (key its answer by [`Document::edit_generation`]). It returns no pictures
    /// for a page that doesn't exist or can't be read. If nested artwork cannot be read, only
    /// direct images are offered, with a logged warning; uncertain Form bounds never select a page.
    pub fn page_pictures_job(&self, page: usize) -> Option<impl FnOnce() -> Vec<PagePicture> + Send + 'static> {
        let cos = self.editor.as_ref()?.cos.clone();
        Some(move || match crate::guard(|| pictures(&cos, page)) {
            Ok(pictures) => pictures,
            Err(e) => {
                let _ = writeln!(std::io::stderr(), "page {} picture selection failed: {e}", page.saturating_add(1));
                Vec::new()
            }
        })
    }

    /// Copy Image in Original Format: picture `index` on `page` as a file (extension, bytes):
    /// a JPEG as stored in the PDF, otherwise a lossless PNG.
    pub fn page_image_copy_file(&self, page: usize, index: usize) -> Result<(&'static str, Vec<u8>), String> {
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        let img = self.page_images(page).into_iter().nth(index).ok_or_else(|| format!("page {} has no image {}", page + 1, index + 1))?;
        let object = img.object.ok_or("the image has no object")?;
        let picture = if img.is_form { picture_in_form(&editor.cos, object).ok_or("this artwork holds no picture")? } else { object };
        pdfcraft_create::image_file(&editor.cos, picture)
    }

    /// Copy Image: picture `index` on `page` at its own resolution, as straight (unpremultiplied)
    /// RGBA pixels (width, height, pixels) for the clipboard. Soft masks become alpha.
    pub fn page_image_rgba(&self, page: usize, index: usize) -> Result<(u32, u32, Vec<u8>), String> {
        let (_, bytes) = self.page_image_copy_file(page, index)?;
        let format = image::guess_format(&bytes).map_err(|e| e.to_string())?;
        let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes.as_slice()), format);
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_COPY_PIXELS * 4);
        reader.limits(limits);
        let decoder = reader.into_decoder().map_err(|e| e.to_string())?;
        let (w, h) = image::ImageDecoder::dimensions(&decoder);
        if w == 0 || h == 0 {
            return Err("the image is empty".into());
        }
        if u64::from(w) * u64::from(h) > MAX_COPY_PIXELS {
            return Err(format!("the image is too large to copy ({w} × {h} pixels); use Save Image As"));
        }
        let rgba = image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?.into_rgba8();
        Ok((rgba.width(), rgba.height(), rgba.into_raw()))
    }

    /// Edit ▸ Copy of an added image (`index` among `page`'s added items): its picture as a file
    /// (extension, bytes), a JPEG or JPEG 2000 file as stored, otherwise a lossless PNG, for
    /// [`Edit::PasteContent`](crate::Edit::PasteContent). Refused when the document doesn't allow
    /// copying.
    pub fn added_image_file(&self, page: usize, index: usize) -> Result<(&'static str, Vec<u8>), String> {
        self.check_copying().map_err(|e| e.to_string())?;
        let editor = self.editor.as_ref().ok_or("the document can't be read")?;
        let item = self.added().iter().filter(|a| a.page == page).nth(index);
        let Some(pdfcraft_edit::Content::Image(img)) = item.map(|a| &a.content) else {
            return Err(format!("item {} on page {} is not an added image", index + 1, page + 1));
        };
        picture_file(&editor.cos, img.image)
    }
}

/// An added image's picture as a file: [`pdfcraft_create::image_file`], except that a JPEG 2000
/// picture, which Add Image stores as the file it was given, is that file.
fn picture_file(cos: &Cos, image: ObjRef) -> Result<(&'static str, Vec<u8>), String> {
    match &*cos.get(image) {
        Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") && s.dict.name(b"Filter") == Some(b"JPXDecode") => Ok(("jp2", s.raw.to_vec())),
        _ => pdfcraft_create::image_file(cos, image),
    }
}

/// Whether image object `image` in `cos` is the picture `file` (as [`picture_file`] writes it).
/// The sizes are compared first, so an unrelated image is not re-encoded to find out.
pub(crate) fn is_picture(cos: &Cos, image: ObjRef, file: &[u8]) -> bool {
    let size = |s: &pdfcraft_cos::Stream| Some((u32::try_from(s.dict.int(b"Width")?).ok()?, u32::try_from(s.dict.int(b"Height")?).ok()?));
    // The object at that number now, not one of an earlier generation of it.
    if cos.generation(image.num) != image.generation {
        return false;
    }
    let stored = match &*cos.get(image) {
        // A JPEG 2000 picture is stored as its file.
        Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") && s.dict.name(b"Filter") == Some(b"JPXDecode") => return s.raw[..] == *file,
        Object::Stream(s) if s.dict.name(b"Subtype") == Some(b"Image") => size(s),
        _ => return false,
    };
    let given = image::ImageReader::new(std::io::Cursor::new(file)).with_guessed_format().ok().and_then(|r| r.into_dimensions().ok());
    stored.is_some() && stored == given && picture_file(cos, image).is_ok_and(|(_, data)| data == file)
}

/// The image object an added image in `cos` already draws picture `file` from, if any: where a
/// second paste from another document finds the first one's.
pub(crate) fn added_picture(cos: &Cos, file: &[u8]) -> Option<ObjRef> {
    let mut seen = std::collections::HashSet::new();
    pdfcraft_edit::list_added(cos).into_iter().find_map(|a| match a.content {
        pdfcraft_edit::Content::Image(img) if seen.insert(img.image) && is_picture(cos, img.image, file) => Some(img.image),
        _ => None,
    })
}

/// The largest raster image Form `form` draws through its resources (nested Forms included).
pub(crate) fn picture_in_form(cos: &Cos, form: ObjRef) -> Option<ObjRef> {
    let mut seen = HashSet::from([form]);
    let mut best: Option<(u64, ObjRef)> = None;
    let res = resources(cos, form)?;
    collect(cos, &res, 0, &mut seen, &mut best);
    best.map(|(_, r)| r)
}

fn resources(cos: &Cos, stream: ObjRef) -> Option<Dict> {
    match &*cos.get(stream) {
        Object::Stream(s) => s.dict.get(b"Resources").map(|r| cos.resolve(r)).and_then(|r| r.as_dict().cloned()),
        _ => None,
    }
}

fn collect(cos: &Cos, res: &Dict, depth: usize, seen: &mut HashSet<ObjRef>, best: &mut Option<(u64, ObjRef)>) {
    if depth > MAX_FORM_DEPTH {
        return;
    }
    let Some(xobjects) = res.get(b"XObject").map(|x| cos.resolve(x)).and_then(|x| x.as_dict().cloned()) else { return };
    for (_, v) in xobjects.iter() {
        let Some(r) = v.as_ref() else { continue };
        if !seen.insert(r) {
            continue;
        }
        let obj = cos.get(r);
        let Object::Stream(s) = &*obj else { continue };
        match s.dict.name(b"Subtype") {
            Some(b"Image") => {
                let px = |k: &[u8]| u64::try_from(s.dict.int(k).unwrap_or(0)).unwrap_or(0);
                let size = px(b"Width").saturating_mul(px(b"Height"));
                if size > 0 && best.is_none_or(|(b, _)| size > b) {
                    *best = Some((size, r));
                }
            }
            Some(b"Form") => {
                if let Some(inner) = resources(cos, r) {
                    collect(cos, &inner, depth + 1, seen, best);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_image_failed_nested_walk_keeps_direct_images_only() {
        let mut cos = pdfcraft_create::blank(200.0, 200.0, 1).unwrap();
        let dict = |entries: Vec<(&[u8], Object)>| entries.into_iter().map(|(key, value)| (key.to_vec(), value)).collect::<Dict>();
        let image = cos.add(Object::Stream(pdfcraft_cos::Stream::from_raw(
            dict(vec![(b"Subtype", Object::name("Image")), (b"Width", Object::Int(1)), (b"Height", Object::Int(1))]),
            vec![255, 0, 0],
        )));
        let inner = dict(vec![(b"XObject", Object::Dict(dict(vec![(b"Im", Object::Ref(image))])))]);
        let form = cos.add(Object::Stream(pdfcraft_cos::Stream::from_raw(
            dict(vec![
                (b"Subtype", Object::name("Form")),
                (b"BBox", Object::Array([0, 0, 200, 200].into_iter().map(Object::Int).collect())),
                (b"Resources", Object::Dict(inner)),
            ]),
            b"10000000000000 0 0 1 0 0 cm /Im Do".to_vec(),
        )));
        let resources = dict(vec![(b"XObject", Object::Dict(dict(vec![(b"Im", Object::Ref(image)), (b"Fm", Object::Ref(form))])))]);
        let content = cos.add(Object::Stream(pdfcraft_cos::Stream::from_raw(Dict::new(), b"q 10 0 0 10 0 0 cm /Im Do Q /Fm Do".to_vec())));
        let page = pdfcraft_model::pages(&cos)[0].obj;
        cos.update_dict(page, |d| {
            d.set(b"Resources".to_vec(), resources);
            d.set(b"Contents".to_vec(), content);
        })
        .unwrap();
        assert!(pdfcraft_edit::reading_images(&cos, 0).is_err());
        let selected = pictures(&cos, 0);
        assert_eq!(selected.len(), 1);
        assert_eq!(picture_at(&selected, [5.0, 5.0]).map(|p| p.0), Some(0));
        assert!(picture_at(&selected, [100.0, 100.0]).is_none());
    }
}
