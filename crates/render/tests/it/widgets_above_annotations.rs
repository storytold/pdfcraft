//! A contributor-original synthetic page: a check box widget listed before a filled square
//! comment that covers it. Form fields' widgets are drawn after the page's other annotations
//! (hayro-interpret patch (86)), so the check box shows; the comment covered it before.

use pdfcraft_render::{PageRenderer, RenderConfig, RenderRequest};
use std::sync::Arc;

/// A 200 × 200 pt page whose `/Annots` lists `order` of: `W`, a check box drawing a green box,
/// `V`, a second check box drawing a blue box at the same place, `S`, a square comment drawing a
/// black one over them, and `R`, a larger red square comment.
fn pdf(order: &str) -> Vec<u8> {
    let annot = |c: char| match c {
        'W' => "6 0 R",
        'S' => "7 0 R",
        'V' => "10 0 R",
        _ => "8 0 R",
    };
    let annots: Vec<&str> = order.chars().map(annot).collect();
    let form = |rgb: &str| {
        let content = format!("{rgb} rg 0 0 100 100 re f");
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] /Length {} >>\nstream\n{content}\nendstream", content.len())
    };
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R 10 0 R] >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [{}] >>", annots.join(" ")),
        form("0 1 0"),
        form("0 0 0"),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (box) /F 4 /Rect [50 50 150 150] /AS /Off /AP << /N << /Off 4 0 R /Yes 4 0 R >> >> >>"
            .to_string(),
        "<< /Type /Annot /Subtype /Square /F 4 /C [0 0 0] /Rect [40 40 160 160] /AP << /N 5 0 R >> >>".to_string(),
        "<< /Type /Annot /Subtype /Square /F 4 /C [1 0 0] /Rect [30 30 170 170] /AP << /N 9 0 R >> >>".to_string(),
        form("1 0 0"),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /T (other) /F 4 /Rect [50 50 150 150] /AS /Off /AP << /N << /Off 11 0 R /Yes 11 0 R >> >> >>"
            .to_string(),
        form("0 0 1"),
    ];
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

/// The colour at the page's centre.
fn centre(order: &str) -> [u8; 3] {
    at(order, 100, 100)
}

/// The colour at (`x`, `y`) px from the top left.
fn at(order: &str, x: usize, y: usize) -> [u8; 3] {
    let mut renderer = PageRenderer::new(Arc::new(pdf(order)), RenderConfig::default());
    let out = renderer.render(RenderRequest { page: 0, scale: 1.0, ..Default::default() });
    assert!(out.error.is_none(), "{:?}", out.error);
    let at = (y * out.width as usize + x) * 4;
    let px = out.rgba.get(at..at + 3).expect("the centre lies on the page");
    [px[0], px[1], px[2]]
}

#[test]
fn a_widget_is_drawn_above_the_comments_on_its_page() {
    assert_eq!(centre("WS"), [0, 255, 0], "a check box listed before a comment over it");
    assert_eq!(centre("SW"), [0, 255, 0], "a check box listed after it");
    // Comments keep their `/Annots` order among themselves.
    assert_eq!(centre("RS"), [0, 0, 0], "the black square listed last is on top");
    assert_eq!(centre("SR"), [255, 0, 0], "the red square listed last is on top");
    // And with a widget among them: (45, 45) is inside both squares, outside the check box.
    assert_eq!(at("WRS", 45, 45), [0, 0, 0], "a widget first: the black square listed last is on top");
    assert_eq!(at("WSR", 45, 45), [255, 0, 0], "a widget first: the red square listed last is on top");
    assert_eq!(centre("WSR"), [0, 255, 0], "and the check box above both");
}

/// Two overlapping widgets keep their `/Annots` order between themselves, the one listed later on
/// top, and both stay above the comments wherever those are listed.
#[test]
fn overlapping_widgets_keep_their_order_above_the_comments() {
    const GREEN: [u8; 3] = [0, 255, 0];
    const BLUE: [u8; 3] = [0, 0, 255];
    for (order, top) in [("WV", BLUE), ("VW", GREEN), ("WVS", BLUE), ("VWS", GREEN), ("SWV", BLUE), ("SVW", GREEN), ("WSRV", BLUE), ("VRSW", GREEN)] {
        assert_eq!(centre(order), top, "{order}: the check box listed later is on top");
    }
    // The comments still keep their own order below the widgets: (45, 45) is inside both squares
    // and outside the check boxes.
    assert_eq!(at("VWRS", 45, 45), [0, 0, 0], "the black square listed last is on top");
    assert_eq!(at("SVRW", 45, 45), [255, 0, 0], "the red square listed last is on top");
}
