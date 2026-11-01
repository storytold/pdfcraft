//! Copy Image: find the picture under a point and hand it to the clipboard, as pixels or as a
//! file in its original format. A picture drawn through grouped
//! Form artwork (as Word, InDesign and many scanners write them) is found inside the group.

use std::collections::HashSet;

use pdfcraft_cos::{Dict, Document as Cos, ObjRef, Object};

use crate::Document;

/// Clipboards copy the whole bitmap; past this many pixels a pixel copy is refused.
const MAX_COPY_PIXELS: u64 = 64_000_000;
/// How deep Form artwork is searched for its picture.
const MAX_FORM_DEPTH: usize = 12;

impl Document {
    /// The topmost picture on `page` under user-space point `at`, as its index in
    /// [`Document::page_images`]: a raster image, or Form artwork that holds one.
    pub fn page_image_at(&self, page: usize, at: [f64; 2]) -> Option<usize> {
        let editor = self.editor.as_ref()?;
        let inside = |r: &[f64; 4]| (r[0]..=r[2]).contains(&at[0]) && (r[1]..=r[3]).contains(&at[1]);
        self.page_images(page).iter().rposition(|i| {
            inside(&i.rect)
                && match (i.is_form, i.object) {
                    (false, Some(_)) => true,
                    (true, Some(form)) => picture_in_form(&editor.cos, form).is_some(),
                    _ => false,
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
fn picture_in_form(cos: &Cos, form: ObjRef) -> Option<ObjRef> {
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
