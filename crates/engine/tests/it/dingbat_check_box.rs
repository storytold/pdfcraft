//! A check box the file leaves to the viewer draws its `/MK /CA` caption `4` in ZapfDingbats
//! (ISO 32000-2 Table 192 and §12.7.5.2.3), and the standard font's bundled stand-in draws that
//! check mark (a20, Annex D.6) at its standard advance (§9.6.2.2): centred in the box, at the
//! `/DA` size, neither squeezed nor spilling past its advance.

use std::sync::Arc;

use pdfcraft_cos::{Document, SaveOptions, write_incremental};
use pdfcraft_fonts::Std14;
use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};

/// The `/DA` size of the check, in points (and pixels, at scale 1).
const SIZE: f64 = 60.0;

fn pdf(objects: &[&str]) -> Vec<u8> {
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer << /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

#[test]
fn a_check_box_caption_check_is_drawn_at_its_standard_advance() {
    // A 100 pt box at (50, 50) on a 200 pt page, with no border, no background and no /AP.
    let bytes = pdf(&[
        "<< /Type /Catalog /Pages 2 0 R /AcroForm 4 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [5 0 R] >>",
        "<< /Fields [5 0 R] /NeedAppearances true >>",
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (c) /V /Yes /AS /Yes /DA (/ZaDb 60 Tf 0 g) /MK << /CA (4) >> /Rect [50 50 150 150] /F 4 /P 3 0 R >>",
    ]);
    let doc = Document::open(Arc::new(bytes)).expect("the fixture opens");
    let shown = pdfcraft_forms::with_field_appearances(&doc).expect("the check box gets an appearance");
    let bytes = write_incremental(&shown, &SaveOptions::default()).expect("the display copy saves");
    let mut renderer = PageRenderer::new(Arc::new(bytes), RenderConfig::default());
    let page = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(page.error.is_none(), "{:?}", page.error);

    // The bounding box of the dark pixels, in pixels from the top left.
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..page.height {
        for x in 0..page.width {
            let at = usize::try_from((y * page.width + x) * 4).unwrap_or(usize::MAX);
            if page.rgba.get(at).is_some_and(|r| *r < 128) {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    assert!(x0 <= x1, "the check is drawn");
    let (x0, y0, x1, y1) = (f64::from(x0), f64::from(y0), f64::from(x1 + 1), f64::from(y1 + 1));

    // The check's advance, centred across the box: 846 units at 60 pt is 50.76 pt.
    let advance = Std14::ZapfDingbats.advance(b'4', SIZE);
    assert_eq!(Std14::ZapfDingbats.width(b'4'), 846.0, "a20's standard width");
    let left = 50.0 + (100.0 - advance) / 2.0;
    assert!(x0 >= left - 1.0 && x1 <= left + advance + 1.0, "the check stays within its advance {left}..{}: {x0}..{x1}", left + advance);
    assert!(x1 - x0 >= 0.8 * advance, "the check is not squeezed: {} px of {advance}", x1 - x0);
    assert!(((x0 + x1) / 2.0 - 100.0).abs() <= 3.0, "the check is centred across the box: {x0}..{x1}");
    // Inside the box vertically, and about as tall as the font size makes it.
    assert!(y0 >= 50.0 && y1 <= 150.0, "the check stays inside the box: {y0}..{y1}");
    assert!((0.5 * SIZE..=0.85 * SIZE).contains(&(y1 - y0)), "the check is {} px tall at {SIZE} pt", y1 - y0);
}
