//! The picture of a watermark or background taken from a file (Source ▸ File), for the dialogs'
//! preview: brought in the way OK brings it in, drawn alone on a page of its shape, and rendered
//! small, transparent where it draws nothing.

use std::sync::Arc;

use pdfcraft_cos::Object;
use pdfcraft_render::{PageRenderer, Pixels, RenderConfig, RenderRequest, RenderedPage, effective_scale};

use crate::{MarkFile, guard, mark_source};

/// The longest side [`mark_picture`] renders, in pixels, whatever it is asked for.
pub const MAX_MARK_PICTURE_SIDE: u32 = 2048;

/// The longer side of the page the picture is drawn on, in points. Only its shape matters: the
/// picture is fitted to it, as a mark is fitted to the page it goes on.
const SHEET: f64 = 1000.0;

/// A mark's picture, as [`mark_picture`] renders it.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkPicture {
    /// The picture's size in points, which a watermark or background fits to the page at its
    /// scale (`picture` in `pdfcraft-edit`).
    pub size: (f64, f64),
    /// The raster's size in pixels.
    pub width: u32,
    pub height: u32,
    /// Premultiplied RGBA8, row-major: transparent where the picture draws nothing.
    pub rgba: Pixels,
}

/// Render the picture `f` adds as a watermark or background, its longer side `max_side` pixels
/// (at most [`MAX_MARK_PICTURE_SIDE`]). It is brought in as OK brings it in (an image turned
/// upright by its EXIF orientation, a page of a PDF as a form, upright), so a file OK refuses is
/// refused here too. Never panics.
pub fn mark_picture(f: &MarkFile, max_side: u32) -> Result<MarkPicture, String> {
    guard(|| render(f, max_side))?
}

fn render(f: &MarkFile, max_side: u32) -> Result<MarkPicture, String> {
    // Two pages of the picture's shape: the picture alone, on the renderer's white paper, and
    // the picture over black. Between them they give its transparency.
    let mut doc = pdfcraft_create::blank(SHEET, SHEET, 2).map_err(|e| e.to_string())?;
    let src = mark_source(&mut doc, f).map_err(|e| e.to_string())?;
    let (w, h) = src.size;
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return Err(format!("{}: the picture has no size", f.name));
    }
    let k = SHEET / w.max(h);
    let (pw, ph) = ((w * k).max(1.0), (h * k).max(1.0));
    let media = Object::Array(vec![0.into(), 0.into(), Object::Real(pw), Object::Real(ph)]);
    for page in pdfcraft_model::pages(&doc) {
        doc.update_dict(page.obj, |d| d.set(b"MediaBox".to_vec(), media.clone())).map_err(|e| e.to_string())?;
    }
    let black = pdfcraft_edit::Background { color: [0.0; 3], opacity: 1.0, source: None, scale: 1.0 };
    pdfcraft_edit::add_background(&mut doc, &[1], &black, false).map_err(|e| e.to_string())?;
    let whole = pdfcraft_edit::Watermark { source: Some(src), scale: 1.0, opacity: 1.0, rotation: 0.0, ..Default::default() };
    pdfcraft_edit::add_watermark(&mut doc, &[0, 1], &whole, false).map_err(|e| e.to_string())?;
    let bytes = Arc::new(pdfcraft_cos::write_full(&doc, &pdfcraft_cos::SaveOptions::default()).map_err(|e| e.to_string())?);

    let side = max_side.clamp(1, MAX_MARK_PICTURE_SIDE) as f32;
    let scale = effective_scale(pw as f32, ph as f32, side / pw.max(ph) as f32);
    let mut renderer = PageRenderer::new(bytes, RenderConfig::default());
    let mut page = |n: usize| -> Result<RenderedPage, String> {
        let out = renderer.render(RenderRequest { page: n, scale, ..Default::default() });
        match out.error {
            Some(e) => Err(e),
            None => Ok(out),
        }
    };
    let (on_white, on_black) = (page(0)?, page(1)?);
    if (on_white.width, on_white.height) != (on_black.width, on_black.height) || on_white.rgba.len() != on_black.rgba.len() {
        return Err("the picture rendered at two sizes".into());
    }
    Ok(MarkPicture { size: src.size, width: on_black.width, height: on_black.height, rgba: unpaper(&on_white.rgba, &on_black.rgba) })
}

/// The picture with its transparency, from its renders on white and over black. Where it lets
/// `1 − a` of what is behind it through, it is that much lighter on white than over black, and
/// over black it is itself, premultiplied.
fn unpaper(on_white: &[u8], on_black: &[u8]) -> Pixels {
    let (white, _) = on_white.as_chunks::<4>();
    let (black, _) = on_black.as_chunks::<4>();
    let words = white
        .iter()
        .zip(black)
        .map(|(&[wr, wg, wb, _], &[br, bg, bb, _])| {
            let a = 255 - wr.saturating_sub(br).max(wg.saturating_sub(bg)).max(wb.saturating_sub(bb));
            u32::from_ne_bytes([br.min(a), bg.min(a), bb.min(a), a])
        })
        .collect::<Vec<u32>>();
    Pixels::from(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, px: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let data: Vec<u8> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).flat_map(|(x, y)| px(x, y)).collect();
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.write_header().unwrap().write_image_data(&data).unwrap();
        }
        out
    }

    fn file(name: &str, bytes: Vec<u8>, page: usize) -> MarkFile {
        MarkFile { name: name.into(), bytes: Arc::new(bytes), page }
    }

    /// The pixel at (`x`, `y`) of the picture, premultiplied RGBA.
    fn at(p: &MarkPicture, x: u32, y: u32) -> [u8; 4] {
        let i = (y * p.width + x) as usize * 4;
        p.rgba[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn an_image_keeps_its_size_colours_and_transparency() {
        // 40 × 20 pixels: opaque red on the left, nothing on the right.
        let bytes = png(40, 20, |x, _| if x < 20 { [255, 0, 0, 255] } else { [0, 0, 0, 0] });
        let p = mark_picture(&file("half.png", bytes, 0), 80).unwrap();
        assert_eq!(p.size, (40.0, 20.0), "40 × 20 px at 72 dpi, as OK lays it out");
        assert_eq!((p.width, p.height), (80, 40));
        assert_eq!(at(&p, 10, 20), [255, 0, 0, 255], "the red half is opaque red");
        assert_eq!(at(&p, 70, 20), [0, 0, 0, 0], "the empty half lets the page through");
    }

    #[test]
    fn a_pdf_page_is_transparent_where_it_draws_nothing_and_upright() {
        // 200 × 100, turned a quarter (/Rotate 90): shown 100 wide and 200 tall. A white box
        // on the left half of the unturned page, nothing on the right.
        let content = "1 1 1 rg 0 0 100 100 re f";
        let pdf = format!(
            "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Rotate 90 /Contents 4 0 R >> endobj
4 0 obj << /Length {} >>
stream
{content}
endstream
endobj
trailer << /Root 1 0 R >>
%%EOF",
            content.len()
        );
        let p = mark_picture(&file("stamp.pdf", pdf.clone().into_bytes(), 0), 100).unwrap();
        assert_eq!(p.size, (100.0, 200.0), "the page as displayed");
        assert_eq!((p.width, p.height), (50, 100));
        // Turned clockwise, the unturned left half is the top half.
        assert_eq!(at(&p, 25, 25), [255, 255, 255, 255], "painted white stays opaque white");
        assert_eq!(at(&p, 25, 75), [0, 0, 0, 0], "what the page leaves unpainted is transparent");
        assert!(mark_picture(&file("stamp.pdf", pdf.into_bytes(), 3), 100).is_err(), "a page the file doesn't have");
    }

    #[test]
    fn what_ok_refuses_is_refused_and_sizes_are_capped() {
        assert!(mark_picture(&file("x.png", b"not an image".to_vec(), 0), 100).is_err());
        assert!(mark_picture(&file("x.png", Vec::new(), 0), 100).is_err());
        assert!(mark_picture(&file("x.pdf", b"%PDF-1.7 junk".to_vec(), 0), 100).is_err());
        let bytes = png(4, 2, |_, _| [0, 0, 255, 255]);
        let p = mark_picture(&file("blue.png", bytes.clone(), 0), u32::MAX).unwrap();
        assert_eq!(p.width, MAX_MARK_PICTURE_SIDE);
        let p = mark_picture(&file("blue.png", bytes, 0), 0).unwrap();
        assert_eq!((p.width, p.height), (1, 1));
    }

    #[test]
    fn transparency_comes_from_the_two_backdrops() {
        // Opaque white, nothing, half-transparent black, half-transparent red: (128, 0, 0, 128)
        // premultiplied lets 127 of white through.
        let white = [255, 255, 255, 255, 255, 255, 255, 255, 127, 127, 127, 255, 255, 127, 127, 255];
        let black = [255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 128, 0, 0, 255];
        let px = unpaper(&white, &black);
        assert_eq!(&px[..], &[255, 255, 255, 255, 0, 0, 0, 0, 0, 0, 0, 128, 128, 0, 0, 128]);
    }
}
