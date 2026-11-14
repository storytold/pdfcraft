use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// `n` blank A4 pages.
fn blank_pages(n: usize) -> Vec<u8> {
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 595 842] >>", kids.join(" ")));
    for _ in 0..n {
        objs.push("<< /Type /Page /Parent 2 0 R >>".to_string());
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    for (i, o) in objs.iter().enumerate() {
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    out.extend_from_slice(b"trailer << /Root 1 0 R >>\n%%EOF\n");
    out
}

/// #7016: in a 400-page document the scroll bar's thumb shrank to egui's 12 point minimum, 8
/// points wide, and the pointer dragging it covered it whole, so it seemed to vanish. Its drawn
/// length is measured in the rendered window, in the bar's middle column, with the pointer off it.
#[test]
fn a_long_documents_scroll_thumb_stays_long_enough_to_see_under_the_pointer() {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("long.pdf", None, blank_pages(400)).unwrap();
        app
    });
    h.run_steps(6);
    let viewport = h.state().views[0].viewport_rect();
    let x = viewport.right() - 4.0;
    // Drag the thumb a third of the way down, then hold the pointer over the bar's top end, well
    // above the thumb, so the bar stays shown and the pointer hides none of it.
    let mut pos = egui::pos2(x, viewport.top() + 4.0);
    h.input_mut().events.push(egui::Event::PointerMoved(pos));
    h.run_steps(2);
    h.input_mut().events.push(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(2);
    for _ in 0..4 {
        pos.y += 50.0;
        h.input_mut().events.push(egui::Event::PointerMoved(pos));
        h.run_steps(2);
    }
    assert!(h.state().views[0].current > 50, "the drag scrolled, to page {}", h.state().views[0].current + 1);
    h.input_mut().events.push(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    // A frame apart: moved in the release's frame, the drag would still follow the pointer up.
    h.run_steps(2);
    h.input_mut().events.push(egui::Event::PointerMoved(egui::pos2(x, viewport.top() + 2.0)));
    h.run_steps(20);
    let _gpu = crate::gpu();
    let image = h.render().unwrap();
    // The longest run of thumb-dark pixels in the column, below where the pointer rests.
    let (mut run, mut longest) = (0u32, 0u32);
    for y in (viewport.top() + 30.0) as u32..viewport.bottom() as u32 - 4 {
        let [r, g, b, _] = image.get_pixel(x as u32, y).0;
        if u32::from(r) + u32::from(g) + u32::from(b) < 3 * 200 {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    assert!(longest >= 32, "the thumb is drawn {longest} px long");
}
