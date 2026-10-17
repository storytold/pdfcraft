//! Export a PDF ▸ Comic book archive (`.cbz`): one image per page in a zip archive, named so
//! they sort in page order (`001.jpg`, `002.jpg`…), with a `ComicInfo.xml` carrying the title,
//! author, subject, language, reading direction and bookmarks (as page bookmarks).
//!
//! Comics in PDF are mostly scans: each page one JPEG filling it. Such a page's JPEG goes into
//! the archive unchanged (passthrough), with no loss and no re-encoding, when nothing on the page
//! would look different from it: the content stream only places that one image over the page's
//! visible area, upright and unstretched, the page isn't rotated, the image has no mask,
//! transparency, decode array, optional-content membership or CMYK samples, and the page has no
//! comments or form fields. Every other page is rendered at the chosen resolution, as Export ▸
//! Image does.

use std::io::Write;

use pdfcraft_content::Matrix;
use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_export::{ComicInfo, ComicPage, ZipWriter};

use super::{ExportSource, Exporter, ImageFormat};

/// How pages are written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CbzOptions {
    /// The resolution rendered pages are drawn at.
    pub dpi: f64,
    /// PNG or JPEG; comic readers don't open TIFF.
    pub format: ImageFormat,
    /// Put a page's own JPEG into the archive unchanged when the page is just that image.
    pub passthrough: bool,
}

impl Default for CbzOptions {
    fn default() -> Self {
        Self { dpi: 200.0, format: ImageFormat::Jpeg { quality: 90 }, passthrough: true }
    }
}

/// What an export did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CbzReport {
    /// Page images written.
    pub pages: usize,
    /// The 0-based pages whose own JPEG went in unchanged.
    pub passed_through: Vec<usize>,
    /// Rendered pages too large for the renderer at the asked resolution, drawn at the most it
    /// allows: (0-based page, dpi).
    pub lower_dpi: Vec<(usize, f64)>,
}

/// Write `pages` (0-based, in this order) of `src` as a comic book archive to `out`, calling
/// `progress` with the number of pages done before each page. Returns the writer, so a caller
/// streaming to a staging file can flush and rename it.
pub fn write_cbz<W: Write>(
    src: &ExportSource,
    pages: &[usize],
    opts: CbzOptions,
    out: W,
    mut progress: impl FnMut(usize),
) -> Result<(W, CbzReport), String> {
    if opts.format == ImageFormat::Tiff {
        return Err("comic readers don't open TIFF images: choose PNG or JPEG".into());
    }
    if let Some(p) = pages.iter().find(|p| **p >= src.pages) {
        return Err(format!("page {} does not exist", p.saturating_add(1)));
    }
    let doc = Document::open_with_password(src.bytes.clone(), src.config.password.as_deref()).map_err(|e| e.to_string())?;
    let all = pdfcraft_model::pages(&doc);
    let mut info = metadata(&doc);
    // The first bookmark (in listing order) to each page names it.
    let mut marks: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    for b in pdfcraft_organize::bookmarks(&doc) {
        if let Some(p) = b.page {
            marks.entry(p).or_insert(b.title);
        }
    }
    let digits = pages.len().to_string().len().max(3);
    let mut zip = ZipWriter::new(out);
    let mut ex = Exporter::from_source(src.clone());
    let mut report = CbzReport::default();
    let io = |e: std::io::Error| e.to_string();
    for (k, &p) in pages.iter().enumerate() {
        progress(k);
        let own = if opts.passthrough { all.get(p).and_then(|page| passthrough(&doc, page)) } else { None };
        let (data, width, height, ext) = match own {
            Some((data, w, h)) => {
                report.passed_through.push(p);
                (data, w, h, "jpg")
            }
            None => {
                let (data, w, h) = ex.image_sized(p, opts.dpi, opts.format)?;
                let used = ex.dpi_used(p, opts.dpi);
                if used < opts.dpi.clamp(18.0, 1200.0) - 0.5 {
                    report.lower_dpi.push((p, used.floor()));
                }
                (data, w, h, opts.format.extension())
            }
        };
        zip.add(&format!("{:0digits$}.{ext}", k + 1), &data, false).map_err(io)?;
        info.pages.push(ComicPage { width, height, size: data.len() as u64, bookmark: marks.remove(&p) });
        report.pages += 1;
    }
    zip.add("ComicInfo.xml", pdfcraft_export::comic_info(&info).as_bytes(), true).map_err(io)?;
    Ok((zip.finish().map_err(io)?, report))
}

/// The document's title, author, subject, language and reading direction.
fn metadata(doc: &Document) -> ComicInfo {
    let view = pdfcraft_organize::initial_view(doc);
    ComicInfo {
        title: pdfcraft_organize::info(doc, "Title"),
        summary: pdfcraft_organize::info(doc, "Subject"),
        writer: pdfcraft_organize::info(doc, "Author"),
        language: view.language,
        right_to_left: view.right_to_left,
        pages: Vec::new(),
    }
}

/// A content stream that only places an image is a few dozen bytes; anything much longer draws
/// more than that, and isn't worth decoding to find out.
const MAX_CONTENT: usize = 64 * 1024;
/// Nested `q` deeper than this isn't a page that only places an image.
const MAX_SAVES: usize = 32;

/// The page's own JPEG, with its width and height, when the page shows exactly that image and
/// nothing else (see the module documentation); `None` when it has to be rendered.
fn passthrough(doc: &Document, page: &pdfcraft_model::Page) -> Option<(Vec<u8>, u32, u32)> {
    if page.rotation(doc) != 0 || has_annotations(doc, &page.dict) {
        return None;
    }
    let content = content(doc, page.dict.get(b"Contents")?)?;
    let parsed = pdfcraft_content::parse(&content);
    if parsed.skipped > 0 {
        return None;
    }
    let mut ctm = Matrix::IDENTITY;
    let mut saved = Vec::new();
    let mut placed: Option<(Vec<u8>, Matrix)> = None;
    for op in &parsed.ops {
        match op.op.as_slice() {
            b"q" if saved.len() < MAX_SAVES => saved.push(ctm),
            b"Q" => ctm = saved.pop()?,
            b"cm" => ctm = Matrix::from_operands(&op.operands)?.then(&ctm),
            b"Do" if placed.is_none() => placed = Some((op.name(0)?.to_vec(), ctm)),
            // Marked content changes nothing drawn, unless it is optional content.
            b"BMC" | b"EMC" => {}
            b"BDC" if op.name(0) != Some(b"OC") => {}
            _ => return None,
        }
    }
    let (name, ctm) = placed?;
    let resources = doc.resolve(page.dict.get(b"Resources")?);
    let xobjects = doc.resolve(resources.as_dict()?.get(b"XObject")?);
    let image = doc.get(xobjects.as_dict()?.get(&name)?.as_ref()?);
    let Object::Stream(s) = &*image else { return None };
    let (w, h) = plain_jpeg(doc, s)?;
    covers(ctm, page.crop(doc), w, h).then(|| (s.raw.to_vec(), w, h))
}

/// Comments and form fields draw over the page (links and pop-ups don't).
fn has_annotations(doc: &Document, page: &Dict) -> bool {
    let Some(annots) = page.get(b"Annots").map(|a| doc.resolve(a)) else { return false };
    let Some(annots) = annots.as_array() else { return false };
    annots.iter().any(|a| {
        let a = doc.resolve(a);
        !matches!(a.as_dict().and_then(|d| d.get(b"Subtype")).map(|s| doc.resolve(s)).as_deref().and_then(Object::as_name), Some(b"Link" | b"Popup"))
    })
}

/// The page's content streams, decoded and joined; `None` when one doesn't decode or they are
/// longer than [`MAX_CONTENT`].
fn content(doc: &Document, contents: &Object) -> Option<Vec<u8>> {
    let decode = |o: &Object, room: usize| match &*doc.resolve(o) {
        Object::Stream(s) => s.decoded_within(room).ok(),
        _ => None,
    };
    let contents_obj = doc.resolve(contents);
    let Some(parts) = contents_obj.as_array() else { return decode(contents, MAX_CONTENT) };
    let mut out = Vec::new();
    for part in parts {
        let mut d = decode(part, MAX_CONTENT.saturating_sub(out.len()))?;
        out.append(&mut d);
        out.push(b'\n');
        if out.len() > MAX_CONTENT {
            return None;
        }
    }
    Some(out)
}

/// An image XObject's width and height when its data is a JPEG file a comic reader shows as the
/// PDF does: 8-bit grey or RGB, with nothing (mask, soft mask, decode array, optional content,
/// decode parameters) changing how it looks.
fn plain_jpeg(doc: &Document, s: &Stream) -> Option<(u32, u32)> {
    let d = &s.dict;
    if d.name(b"Subtype") != Some(b"Image") {
        return None;
    }
    let filter = d.get(b"Filter").map(|f| doc.resolve(f))?;
    let dct = |o: &Object| matches!(o.as_name(), Some(b"DCTDecode" | b"DCT"));
    let jpeg = match &*filter {
        Object::Array(a) => a.len() == 1 && a.first().is_some_and(dct),
        o => dct(o),
    };
    let changed = [&b"ImageMask"[..], b"Mask", b"SMask", b"Decode", b"DecodeParms", b"OC", b"SMaskInData"]
        .iter()
        .any(|k| d.get(k).is_some_and(|v| !matches!(&*doc.resolve(v), Object::Null | Object::Bool(false))));
    if !jpeg || changed || d.get(b"BitsPerComponent").and_then(|b| doc.resolve(b).as_int()) != Some(8) || !grey_or_rgb(doc, d.get(b"ColorSpace")?) {
        return None;
    }
    if !s.raw.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let size = |k: &[u8]| d.get(k).and_then(|v| doc.resolve(v).as_int()).and_then(|v| u32::try_from(v).ok()).filter(|v| *v > 0);
    Some((size(b"Width")?, size(b"Height")?))
}

/// A device, calibrated or ICC-based colour space with one or three components.
fn grey_or_rgb(doc: &Document, space: &Object) -> bool {
    let space = doc.resolve(space);
    match &*space {
        Object::Name(n) => matches!(n.as_slice(), b"DeviceGray" | b"DeviceRGB" | b"G" | b"RGB"),
        Object::Array(a) => match a.first().and_then(Object::as_name) {
            Some(b"CalGray" | b"CalRGB") => true,
            Some(b"ICCBased") => {
                let n = a
                    .get(1)
                    .map(|p| doc.resolve(p))
                    .and_then(|p| p.as_dict().and_then(|d| d.get(b"N")).map(|n| doc.resolve(n)).and_then(|n| n.as_int()));
                matches!(n, Some(1 | 3))
            }
            _ => false,
        },
        _ => false,
    }
}

/// Does `ctm` (the image's unit square to user space) lay a `w` × `h` image upright over the
/// whole of `crop`, within a point (or half a percent of the page) at each edge, and without
/// stretching it by more than a percent?
fn covers(ctm: Matrix, crop: [f64; 4], w: u32, h: u32) -> bool {
    let [a, b, c, d, e, f] = ctm.0;
    if !ctm.0.iter().all(|v| v.is_finite()) || a <= 0.0 || d <= 0.0 || b.abs() > a * 1e-3 || c.abs() > d * 1e-3 {
        return false;
    }
    let (cw, ch) = (crop[2] - crop[0], crop[3] - crop[1]);
    let (tx, ty) = ((cw * 0.005).max(1.0), (ch * 0.005).max(1.0));
    let edges = (e - crop[0]).abs() <= tx && (e + a - crop[2]).abs() <= tx && (f - crop[1]).abs() <= ty && (f + d - crop[3]).abs() <= ty;
    let stretch = (f64::from(w) / f64::from(h)) / (a / d);
    edges && (stretch - 1.0).abs() <= 0.01
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny RGB JPEG, `w` × `h`.
    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        use image::ImageEncoder;
        let rgb = vec![200u8; (w * h * 3) as usize];
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80).write_image(&rgb, w, h, image::ExtendedColorType::Rgb8).unwrap();
        out
    }

    /// A PDF from page specs: each a content stream, extra page-dictionary entries (a /MediaBox
    /// there replaces the 300 × 400 one) and an image dictionary's extra entries (a /ColorSpace
    /// there replaces /DeviceRGB). The image is a 30 × 40 JPEG named /Im0.
    fn pdf(pages: &[(&str, &str, &str)], trailer_extra: &str, catalog_extra: &str) -> Vec<u8> {
        let img = jpeg(30, 40);
        let mut objs: Vec<Vec<u8>> = Vec::new();
        objs.push(format!("<< /Type /Catalog /Pages 2 0 R {catalog_extra} >>").into_bytes());
        let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", 3 + 3 * i)).collect();
        objs.push(format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len()).into_bytes());
        for (i, (content, page_extra, image_extra)) in pages.iter().enumerate() {
            let n = 3 + 3 * i;
            let media = if page_extra.contains("/MediaBox") { "" } else { "/MediaBox [0 0 300 400]" };
            objs.push(format!("<< /Type /Page /Parent 2 0 R {media} /Contents {} 0 R /Resources << /XObject << /Im0 {} 0 R >> /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> {page_extra} >>", n + 1, n + 2).into_bytes());
            objs.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()).into_bytes());
            let space = if image_extra.contains("/ColorSpace") { "" } else { "/ColorSpace /DeviceRGB" };
            let mut s = format!("<< /Type /XObject /Subtype /Image /Width 30 /Height 40 {space} /BitsPerComponent 8 /Filter /DCTDecode /Length {} {image_extra} >>\nstream\n", img.len()).into_bytes();
            s.extend_from_slice(&img);
            s.extend_from_slice(b"\nendstream");
            objs.push(s);
        }
        objs.push(b"<< /Title (Space Comics) /Author (A. Artist) >>".to_vec());
        let info = objs.len();
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {} /Root 1 0 R /Info {info} 0 R {trailer_extra} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes(),
        );
        out
    }

    fn source(bytes: Vec<u8>) -> ExportSource {
        let doc = Document::open_with_password(std::sync::Arc::new(bytes.clone()), None).unwrap();
        let pages = pdfcraft_model::pages(&doc);
        ExportSource {
            sizes: pages.iter().map(|p| p.display_size(&doc)).map(|(w, h)| (w as f32, h as f32)).collect(),
            pages: pages.len(),
            bytes: std::sync::Arc::new(bytes),
            config: Default::default(),
        }
    }

    /// The entries of a classic archive, read through its local headers: (name, data).
    fn entries(zip: &[u8]) -> Vec<(String, Vec<u8>)> {
        let u16_at = |at: usize| u16::from_le_bytes([zip[at], zip[at + 1]]) as usize;
        let u32_at = |at: usize| u32::from_le_bytes(zip[at..at + 4].try_into().unwrap()) as usize;
        let mut at = 0;
        let mut out = Vec::new();
        while zip[at..].starts_with(b"PK\x03\x04") {
            let (method, packed, n, x) = (u16_at(at + 8), u32_at(at + 18), u16_at(at + 26), u16_at(at + 28));
            let name = String::from_utf8(zip[at + 30..at + 30 + n].to_vec()).unwrap();
            let raw = &zip[at + 30 + n + x..at + 30 + n + x + packed];
            let data = if method == 8 {
                let mut d = Vec::new();
                std::io::Read::read_to_end(&mut flate2::read::DeflateDecoder::new(raw), &mut d).unwrap();
                d
            } else {
                raw.to_vec()
            };
            out.push((name, data));
            at += 30 + n + x + packed;
        }
        out
    }

    const PLACED: &str = "q 300 0 0 400 0 0 cm /Im0 Do Q";

    #[test]
    fn a_page_that_is_one_jpeg_goes_in_unchanged_and_others_are_rendered() {
        let bytes = pdf(
            &[(PLACED, "", ""), ("BT /F1 24 Tf 20 200 Td (Hello) Tj ET", "", ""), ("/Artifact BMC q 300 0 0 400 0 0 cm /Im0 Do Q EMC", "", "")],
            "",
            "/Lang (en) /ViewerPreferences << /Direction /R2L >>",
        );
        let src = source(bytes);
        let mut seen = Vec::new();
        let (zip, report) = write_cbz(&src, &[0, 1, 2], CbzOptions { dpi: 36.0, ..Default::default() }, Vec::new(), |k| seen.push(k)).unwrap();
        assert_eq!(seen, [0, 1, 2]);
        assert_eq!(report.pages, 3);
        assert_eq!(report.passed_through, [0, 2]);
        let e = entries(&zip);
        let names: Vec<&str> = e.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["001.jpg", "002.jpg", "003.jpg", "ComicInfo.xml"]);
        assert_eq!(e[0].1, jpeg(30, 40), "the page's own JPEG, byte for byte");
        assert_ne!(e[1].1, jpeg(30, 40));
        assert!(e[1].1.starts_with(&[0xFF, 0xD8]));
        let xml = String::from_utf8(e[3].1.clone()).unwrap();
        assert!(xml.contains("<Title>Space Comics</Title>") && xml.contains("<Writer>A. Artist</Writer>"), "{xml}");
        assert!(xml.contains("<LanguageISO>en</LanguageISO>") && xml.contains("<Manga>YesAndRightToLeft</Manga>"), "{xml}");
        assert!(xml.contains("<Page Image=\"0\" Type=\"FrontCover\" ImageSize=\"") && xml.contains("ImageWidth=\"30\" ImageHeight=\"40\""), "{xml}");
        // The rendered page is 300 × 400 pt at 36 dpi.
        assert!(xml.contains("ImageWidth=\"150\" ImageHeight=\"200\""), "{xml}");
    }

    /// Each of these makes the page look different from its JPEG alone, so it is rendered.
    #[test]
    fn pages_that_are_more_than_their_jpeg_are_rendered() {
        let cases: &[(&str, &str, &str, &str)] = &[
            ("rotated page", PLACED, "/Rotate 90", ""),
            ("a comment", PLACED, "/Annots [<< /Type /Annot /Subtype /Square /Rect [10 10 50 50] >>]", ""),
            ("text over the image", "q 300 0 0 400 0 0 cm /Im0 Do Q BT /F1 12 Tf 10 10 Td (x) Tj ET", "", ""),
            ("image twice", "q 300 0 0 400 0 0 cm /Im0 Do Q q 300 0 0 400 0 0 cm /Im0 Do Q", "", ""),
            ("not covering the page", "q 150 0 0 200 0 0 cm /Im0 Do Q", "", ""),
            ("flipped", "q 300 0 0 -400 0 400 cm /Im0 Do Q", "", ""),
            ("stretched", "q 300 0 0 400 0 0 cm /Im0 Do Q", "/MediaBox [0 0 400 400]", ""),
            ("cropped by the crop box", PLACED, "/CropBox [10 10 290 390]", ""),
            ("a soft mask", PLACED, "", "/SMask 1 0 R"),
            ("inverted", PLACED, "", "/Decode [1 0 1 0 1 0]"),
            ("optional content", PLACED, "", "/OC << /Type /OCG /Name (L) >>"),
            ("optional marked content", "/OC /L BDC q 300 0 0 400 0 0 cm /Im0 Do Q EMC", "", ""),
            ("a clip", "q 0 0 100 100 re W n 300 0 0 400 0 0 cm /Im0 Do Q", "", ""),
            ("CMYK", PLACED, "", "/ColorSpace /DeviceCMYK"),
        ];
        for (why, content, page_extra, image_extra) in cases {
            let src = source(pdf(&[(content, page_extra, image_extra)], "", ""));
            let (_, report) = write_cbz(&src, &[0], CbzOptions { dpi: 18.0, ..Default::default() }, Vec::new(), |_| {}).unwrap();
            assert!(report.passed_through.is_empty(), "{why}: passed through");
            assert_eq!(report.pages, 1, "{why}");
        }
    }

    #[test]
    fn passthrough_can_be_turned_off_and_png_chosen() {
        let src = source(pdf(&[(PLACED, "", "")], "", ""));
        let opts = CbzOptions { dpi: 18.0, format: ImageFormat::Png, passthrough: false };
        let (zip, report) = write_cbz(&src, &[0], opts, Vec::new(), |_| {}).unwrap();
        assert!(report.passed_through.is_empty());
        let e = entries(&zip);
        assert_eq!(e[0].0, "001.png");
        assert!(e[0].1.starts_with(b"\x89PNG"));
    }

    #[test]
    fn bookmarks_name_their_pages_and_order_follows_the_list() {
        let bytes = pdf(&[(PLACED, "", ""), (PLACED, "", "")], "", "/Outlines 100 0 R");
        // Append an outline as an incremental update: one bookmark to the second page.
        let doc = Document::open_with_password(std::sync::Arc::new(bytes.clone()), None).unwrap();
        let second = pdfcraft_model::pages(&doc)[1].obj;
        let mut out = bytes.clone();
        let o100 = out.len();
        out.extend_from_slice(b"100 0 obj\n<< /Type /Outlines /First 101 0 R /Last 101 0 R /Count 1 >>\nendobj\n");
        let o101 = out.len();
        out.extend_from_slice(format!("101 0 obj\n<< /Title (Chapter Two) /Parent 100 0 R /Dest [{} 0 R /Fit] >>\nendobj\n", second.num).as_bytes());
        let prev = String::from_utf8_lossy(&bytes)
            .rfind("startxref")
            .map(|i| String::from_utf8_lossy(&bytes)[i + 10..].lines().next().unwrap().trim().to_string())
            .unwrap();
        let xref = out.len();
        out.extend_from_slice(
            format!(
                "xref\n100 2\n{o100:010} 00000 n \n{o101:010} 00000 n \ntrailer\n<< /Size 102 /Root 1 0 R /Prev {prev} >>\nstartxref\n{xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        let src = source(out);
        let (zip, _) = write_cbz(&src, &[1, 0], CbzOptions::default(), Vec::new(), |_| {}).unwrap();
        let e = entries(&zip);
        let xml = String::from_utf8(e.last().unwrap().1.clone()).unwrap();
        // Page 2 comes first in the archive, so it is image 0 and carries the bookmark.
        assert!(xml.contains("<Page Image=\"0\" Type=\"FrontCover\" ImageSize=\"") && xml.contains("Bookmark=\"Chapter Two\""), "{xml}");
        assert_eq!(xml.matches("Bookmark=").count(), 1);
    }

    #[test]
    fn tiff_and_missing_pages_are_refused() {
        let src = source(pdf(&[(PLACED, "", "")], "", ""));
        let tiff = CbzOptions { format: ImageFormat::Tiff, ..Default::default() };
        assert!(write_cbz(&src, &[0], tiff, Vec::new(), |_| {}).unwrap_err().contains("TIFF"));
        assert!(write_cbz(&src, &[1], CbzOptions::default(), Vec::new(), |_| {}).unwrap_err().contains("page 2"));
    }

    #[test]
    fn many_pages_get_wider_numbers() {
        let specs: Vec<(&str, &str, &str)> = (0..3).map(|_| (PLACED, "", "")).collect();
        let src = source(pdf(&specs, "", ""));
        let order: Vec<usize> = (0..1000).map(|i| i % 3).collect();
        let (zip, report) = write_cbz(&src, &order, CbzOptions::default(), Vec::new(), |_| {}).unwrap();
        assert_eq!(report.pages, 1000);
        let e = entries(&zip);
        assert_eq!(e[0].0, "0001.jpg");
        assert_eq!(e[999].0, "1000.jpg");
    }
}
