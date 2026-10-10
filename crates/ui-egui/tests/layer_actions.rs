//! Set-layer-visibility actions (`SetOCGState`) on links and push buttons show and hide layers,
//! as the Layers panel does.

use egui::Pos2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_ui_egui::PdfCraftApp;

/// One page with layers Red (on) and Green (off), a radio-button group, each drawing a band;
/// a "show-red" push button turns Red on and a link toggles Green.
fn fixture() -> Vec<u8> {
    let content = "/OC /R BDC 1 0 0 rg 20 300 260 40 re f EMC /OC /G BDC 0 1 0 rg 20 240 260 40 re f EMC";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [7 0 R] >> /OCProperties << /OCGs [5 0 R 6 0 R] /D << /OFF [6 0 R] /RBGroups [[5 0 R 6 0 R]] >> >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R /Resources << /Properties << /R 5 0 R /G 6 0 R >> >> /Annots [7 0 R 8 0 R] >>".into(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        "<< /Type /OCG /Name (Red) >>".into(),
        "<< /Type /OCG /Name (Green) >>".into(),
        "<< /Type /Annot /Subtype /Widget /FT /Btn /Ff 65536 /T (show-red) /Rect [20 20 120 50] /P 3 0 R /A << /S /SetOCGState /State [/ON 5 0 R] >> >>".into(),
        "<< /Type /Annot /Subtype /Link /Rect [180 20 280 50] /Border [0 0 0] /A << /S /SetOCGState /State [/Toggle 6 0 R] >> >>".into(),
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn harness(setup: impl FnOnce(&mut PdfCraftApp) + 'static) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("layers.pdf", None, fixture()).unwrap();
        app.set_option("zoom", "150").unwrap();
        setup(&mut app);
        app
    });
    h.run_steps(6);
    h
}

/// The screen position of a point given in PDF user space (y up) on the 300 × 400 page.
fn at(h: &Harness<'static, PdfCraftApp>, x: f32, y: f32) -> Pos2 {
    let r = h.state().views[0].page_screen_rect(0).unwrap();
    let k = r.width() / 300.0;
    r.min + egui::vec2(x * k, (400.0 - y) * k)
}

fn click(h: &mut Harness<'static, PdfCraftApp>, p: Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(3);
}

/// Whether Red and Green are shown.
fn shown(h: &Harness<'static, PdfCraftApp>) -> Vec<(String, bool)> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().info.layers.iter().map(|l| (l.name.clone(), l.visible)).collect()
}

fn layers(red: bool, green: bool) -> Vec<(String, bool)> {
    vec![("Red".into(), red), ("Green".into(), green)]
}

#[test]
fn links_set_layer_visibility() {
    let mut h = harness(|_| {});
    assert_eq!(shown(&h), layers(true, false));
    let link = at(&h, 230.0, 35.0);
    h.hover_at(link);
    h.run_steps(2);
    h.get_by_label("Set layer visibility");
    // Turning Green on turns Red, its radio-button partner, off.
    click(&mut h, link);
    assert_eq!(shown(&h), layers(false, true));
    // Toggling Green off turns nothing back on.
    click(&mut h, link);
    assert_eq!(shown(&h), layers(false, false));
    let s = h.state();
    assert!(!s.session.get(s.views[0].id).unwrap().dirty, "showing and hiding layers doesn't change the document");
}

#[test]
fn push_buttons_set_layer_visibility() {
    // Both on, as the Layers panel can leave them.
    let mut h = harness(|app| app.set_option("layer", "Green=on").unwrap());
    assert_eq!(shown(&h), layers(true, true));
    // Turning Red on (it already is) turns Green, its radio-button partner, off.
    let button = at(&h, 70.0, 35.0);
    click(&mut h, button);
    assert_eq!(shown(&h), layers(true, false));
    let s = h.state();
    assert!(!s.session.get(s.views[0].id).unwrap().dirty, "showing and hiding layers doesn't change the document");
}
