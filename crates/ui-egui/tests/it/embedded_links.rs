//! A link's embedded Go-To action (GoToE, ISO 32000-2 §12.6.4.4) opens the PDF attached to the
//! document in a new tab, as the Attachments panel's Open does, and goes to its destination.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// A PDF of `objs` (object 1 the catalog), with a cross-reference table.
fn write_pdf(objs: &[Vec<u8>]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut s = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    s.extend_from_slice(data);
    s.extend_from_slice(b"\nendstream");
    s
}

/// A three-page PDF to embed.
fn inner() -> Vec<u8> {
    write_pdf(&[
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 200 300] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R >>".to_vec(),
    ])
}

/// Link rows, by the y (PDF user space, 300 × 400 page) of their middle.
const TO_PAGE_3: f32 = 375.0;
const PARENT: f32 = 325.0;
const MISSING: f32 = 275.0;
const NOT_A_PDF: f32 = 225.0;
const BY_ANNOTATION: f32 = 175.0;

/// One page with `inner.pdf` and `notes.txt` in the EmbeddedFiles name tree, a file attachment
/// annotation (index 0, `/NM (att-1)`) holding `inner.pdf` too, and a GoToE link per case.
fn fixture() -> Vec<u8> {
    let links = [
        (TO_PAGE_3, "<< /S /GoToE /T << /R /C /N (inner.pdf) >> /D [2 /Fit] /NewWindow true >>"),
        (PARENT, "<< /S /GoToE /T << /R /P >> /D [0 /Fit] >>"),
        (MISSING, "<< /S /GoToE /T << /R /C /N (missing.pdf) >> /D [0 /Fit] >>"),
        (NOT_A_PDF, "<< /S /GoToE /T << /R /C /N (notes.txt) >> /D [0 /Fit] >>"),
        (BY_ANNOTATION, "<< /S /GoToE /T << /R /C /P 0 /A (att-1) >> /D [1 /Fit] >>"),
    ];
    let first_link = 9;
    let annots: Vec<String> = std::iter::once("8 0 R".to_string()).chain((0..links.len()).map(|k| format!("{} 0 R", first_link + k))).collect();
    let mut objs = vec![
        b"<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(inner.pdf) 6 0 R (notes.txt) 7 0 R] >> >> >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".to_vec(),
        format!("<< /Type /Page /Parent 2 0 R /Annots [{}] >>", annots.join(" ")).into_bytes(),
        stream("/Type /EmbeddedFile /Subtype /application#2Fpdf", &inner()),
        stream("/Type /EmbeddedFile /Subtype /text#2Fplain", b"just some notes"),
        b"<< /Type /Filespec /F (inner.pdf) /UF (inner.pdf) /EF << /F 4 0 R >> >>".to_vec(),
        b"<< /Type /Filespec /F (notes.txt) /UF (notes.txt) /EF << /F 5 0 R >> >>".to_vec(),
        b"<< /Type /Annot /Subtype /FileAttachment /Rect [270 10 290 30] /NM (att-1) /FS << /Type /Filespec /F (inner.pdf) /UF (inner.pdf) /EF << /F 4 0 R >> >> >>".to_vec(),
    ];
    objs.extend(
        links
            .iter()
            .map(|(y, a)| format!("<< /Type /Annot /Subtype /Link /Rect [20 {} 280 {}] /Border [0 0 0] /A {a} >>", y - 10.0, y + 10.0).into_bytes()),
    );
    write_pdf(&objs)
}

fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("outer.pdf", None, fixture()).unwrap();
        app.set_option("left", "closed").unwrap();
        app.set_option("panel", "none").unwrap();
        app.set_option("zoom", "100").unwrap();
        app
    });
    h.run_steps(8);
    h
}

/// Click the link at height `y` on page 1 of the first tab.
fn click(h: &mut Harness<'static, PdfCraftApp>, y: f32) {
    let r = h.state().views[0].page_screen_rect(0).expect("page 1 on screen");
    let k = r.width() / 300.0;
    let p = r.min + egui::vec2(150.0 * k, (400.0 - y) * k);
    h.hover_at(p);
    h.run_steps(1);
    h.drag_at(p);
    h.run_steps(1);
    h.drop_at(p);
    h.run_steps(4);
}

fn toast(h: &Harness<'static, PdfCraftApp>) -> Option<String> {
    h.state().toast.clone().map(|t| t.0)
}

fn tab_name(h: &Harness<'static, PdfCraftApp>, i: usize) -> Option<String> {
    let s = h.state();
    s.views.get(i).and_then(|v| s.session.get(v.id)).map(|d| d.name.clone())
}

/// `/R /C /N`: the file in the EmbeddedFiles name tree opens in a new tab at `/D`.
#[test]
fn an_embedded_go_to_link_opens_the_attached_pdf_at_its_destination() {
    let mut h = harness();
    click(&mut h, TO_PAGE_3);
    assert_eq!(h.state().views.len(), 2, "{:?}", toast(&h));
    assert_eq!(h.state().active, Some(1));
    assert_eq!(tab_name(&h, 1).as_deref(), Some("inner.pdf"));
    assert_eq!(h.state().session.get(h.state().views[1].id).unwrap().info.pages.len(), 3);
    assert_eq!(h.state().views[1].current, 2, "the destination's page");
    assert_eq!(h.state().session.get(h.state().views[1].id).unwrap().path, None, "opened from the document, not from a file");
}

/// `/R /C /P /A`: the file of a file attachment annotation, found by its `/NM`.
#[test]
fn an_embedded_go_to_link_opens_a_file_attachment_annotations_pdf() {
    let mut h = harness();
    click(&mut h, BY_ANNOTATION);
    assert_eq!(h.state().views.len(), 2, "{:?}", toast(&h));
    assert_eq!(h.state().views[1].current, 1);
}

/// The parent, a file that isn't attached, and a file that isn't a PDF open nothing and say so.
#[test]
fn embedded_go_to_links_that_cannot_be_followed_open_nothing() {
    for (y, says) in [
        (PARENT, "This PDF uses the unsupported GoToE action."),
        (MISSING, "This PDF uses the unsupported GoToE action."),
        (NOT_A_PDF, "notes.txt isn't a PDF, so the link can't open it."),
    ] {
        let mut h = harness();
        click(&mut h, y);
        assert_eq!(h.state().views.len(), 1, "nothing opened for the link at {y}");
        assert_eq!(toast(&h).as_deref(), Some(says));
    }
}
