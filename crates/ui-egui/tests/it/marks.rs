//! Header & footer, watermark and background dialogs in the real shell (egui_kittest).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::MarkKind;
use pdfcraft_ui_egui::{Dialog, PdfCraftApp};

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

fn harness() -> Harness<'static, PdfCraftApp> {
    harness_with(FIXTURE.to_vec())
}

fn harness_with(pdf: Vec<u8>) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, pdf.clone()).unwrap();
        app
    });
    h.run_steps(4);
    h
}

fn marks(h: &Harness<'static, PdfCraftApp>) -> Vec<MarkKind> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().marks().to_vec()
}

#[test]
fn header_and_footer_dialog_inserts_tokens_and_applies_to_a_subset() {
    let mut h = harness();
    assert!(!h.state_mut().execute("edit.header_footer.remove"), "nothing to remove yet");
    assert!(h.state_mut().execute("edit.header_footer"));
    h.run_steps(3);
    assert_eq!(h.state().dialog, Some(Dialog::Marks(MarkKind::HeaderFooter)));
    // The center footer box takes the page-number token.
    h.state_mut().marks_draft.focused_box = 4;
    h.get_by_label("Insert Page Number").click();
    h.run_steps(2);
    assert_eq!(h.state().marks_draft.hf.text[4], "<<1>>");
    h.state_mut().marks_draft.range.subset = pdfcraft_ui_egui::marks::Subset::Odd;
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    assert_eq!(marks(&h), [MarkKind::HeaderFooter]);
    // Update and Remove are now available.
    assert!(h.state_mut().execute("edit.header_footer.update"));
    h.run_steps(2);
    h.get_by_label("Update Header and Footer");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state_mut().execute("edit.header_footer.remove"));
    assert!(marks(&h).is_empty());
}

#[test]
fn header_and_footer_dialog_draws_in_the_chosen_face() {
    // #3754: the dialog offers Helvetica, Times and Courier (it only said "Font: Helvetica"),
    // and the face chosen is the one the page's header is drawn in.
    let mut h = harness();
    assert!(h.state_mut().execute("edit.header_footer"));
    h.run_steps(3);
    h.get_by_label("Font:");
    h.get_by_value("Helvetica").click();
    h.run_steps(2);
    h.get_by_label("Times").click();
    h.run_steps(2);
    assert_eq!(h.state().marks_draft.hf.font, pdfcraft_engine::FontFamily::Times);
    h.state_mut().marks_draft.hf.text[0] = "Serif".into();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(marks(&h), [MarkKind::HeaderFooter]);
    let s = h.state();
    let doc = pdfcraft_cos::Document::open(s.session.get(s.views[0].id).unwrap().bytes.clone()).unwrap();
    let page = &pdfcraft_model::pages(&doc)[0];
    let res = doc.resolve(page.dict.get(b"Resources").unwrap());
    let fonts = doc.resolve(res.as_dict().unwrap().get(b"Font").unwrap()).as_dict().cloned().unwrap();
    let face = doc.resolve(fonts.get(b"PCTimes").expect("Times is added")).as_dict().cloned().unwrap();
    assert_eq!(face.name(b"BaseFont"), Some(&b"Times-Roman"[..]));
}

#[test]
fn header_and_footer_dialog_can_shrink_the_page_content() {
    let mut h = harness();
    assert!(h.state_mut().execute("edit.bates"));
    h.run_steps(3);
    assert!(!h.state().marks_draft.hf.shrink, "off by default, as in Acrobat");
    h.get_by_label("Shrink the page content to make room for the header and footer").click();
    h.run_steps(2);
    assert!(h.state().marks_draft.hf.shrink);
    h.get_by_label("OK").click();
    h.run_steps(4);
    assert_eq!(h.state().dialog, None);
    assert_eq!(marks(&h), [MarkKind::HeaderFooter]);
}

#[test]
fn watermark_and_background_dialogs() {
    let mut h = harness();
    h.state_mut().execute("edit.watermark");
    h.run_steps(2);
    assert!(h.query_by(|n| n.label().as_deref() == Some("OK") && n.is_disabled()).is_some(), "OK waits for text");
    h.get_by_label("Offset from centre");
    h.state_mut().marks_draft.wm.text = "DRAFT".into();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    h.state_mut().execute("edit.background");
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(marks(&h).len(), 2);
}

#[test]
fn backgrounds_and_watermarks_from_a_file() {
    let mut h = harness();
    h.state_mut().execute("edit.background");
    h.run_steps(2);
    h.get_by_label("File").click();
    h.run_steps(2);
    h.get_by_label("No file chosen");
    assert!(h.query_by(|n| n.label().as_deref() == Some("OK") && n.is_disabled()).is_some(), "OK waits for a file");
    // (Browse… opens the system picker; the test supplies the file directly.)
    h.state_mut().marks_draft.file = Some(("letterhead.pdf".into(), std::sync::Arc::new(FIXTURE.to_vec())));
    h.run_steps(2);
    h.get_by_label("Page number");
    h.get_by_label("Scale relative to target page");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(marks(&h), vec![MarkKind::Background]);
    let s = h.state();
    assert_eq!(s.session.get(s.views[0].id).unwrap().can_undo(), Some("Add background"));
}

/// Three pages: a white backdrop of its own (as browsers write), a page-sized picture (a scan)
/// and bare paper.
fn backdrop_fixture() -> Vec<u8> {
    let stream = |data: &str| format!("<< /Length {} >> stream\n{data}\nendstream", data.len());
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] /Resources << /XObject << /Im0 9 0 R >> >> >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 8 0 R >>".into(),
        stream("q 1 1 1 rg 0 0 300 400 re f Q 0 g 20 20 50 50 re f"),
        stream("q 300 0 0 400 0 0 cm /Im0 Do Q"),
        stream("0 g 20 20 50 50 re f"),
        "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >> stream\n\u{80}\nendstream"
            .into(),
    ];
    let mut out = String::from("%PDF-1.7\n");
    for (i, o) in objs.iter().enumerate() {
        out.push_str(&format!("{} 0 obj {o} endobj\n", i + 1));
    }
    out.push_str("trailer << /Root 1 0 R >>\n%%EOF");
    out.into_bytes()
}

#[test]
fn the_background_dialog_offers_to_replace_a_white_backdrop_and_warns_about_scans() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, backdrop_fixture()).unwrap();
        app
    });
    h.run_steps(4);
    let backdrop = |h: &Harness<'static, PdfCraftApp>, page| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().page_backdrop(page)
    };
    assert_eq!(backdrop(&h, 0), Some(pdfcraft_engine::Backdrop::Fill));
    h.state_mut().execute("edit.background");
    h.run_steps(3);
    let replace = "Page 1 paints its own white backdrop, which would hide the background. Replace it";
    h.get_by_label("A picture covers page 2 (a scan?), so the background won't show there.");
    assert!(h.state().marks_draft.bg.replace_backdrop, "on by default");
    // Left unticked, the backdrop stays and still hides the background.
    h.get_by_label(replace).click();
    h.run_steps(2);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(backdrop(&h, 0), Some(pdfcraft_engine::Backdrop::Fill));
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(2);
    // Reopened, the choice starts on again; ticked, the backdrop is replaced and the scan is
    // reported, not touched.
    h.state_mut().execute("edit.background");
    h.run_steps(3);
    h.get_by_label(replace);
    assert!(h.state().marks_draft.bg.replace_backdrop, "on again when the dialog opens");
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(backdrop(&h, 0), None);
    assert_eq!(backdrop(&h, 1), Some(pdfcraft_engine::Backdrop::Image));
    // Nothing left to replace: the offer is gone. A choice left off from an earlier dialog
    // doesn't carry into Update, which restores the backdrop first and would otherwise keep it
    // with no checkbox shown.
    h.state_mut().marks_draft.bg.replace_backdrop = false;
    h.state_mut().execute("edit.background.update");
    h.run_steps(3);
    assert!(h.query_by_label(replace).is_none());
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert_eq!(backdrop(&h, 0), None, "Update keeps the backdrop replaced");
}

/// Remove background says which pages keep their own white backdrop off: here one whose
/// backdrop copy changed before the fill since the background replaced it.
#[test]
fn remove_background_says_which_pages_keep_their_own_backdrop_off() {
    let copy = "1 1 1 rg 0 0 300 400 re n 0 g 20 20 50 50 re f";
    let bg = "q 1 0 0 rg 0 0 300 400 re f Q";
    let fixture = format!(
        "%PDF-1.7\n\
         1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
         2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >> endobj\n\
         3 0 obj << /Type /Page /Parent 2 0 R /Contents [4 0 R 5 0 R] >> endobj\n\
         4 0 obj << /PCMark /Background /Length {} >> stream\n{bg}\nendstream endobj\n\
         5 0 obj << /PCBackdrop << /Index 2 /Op (f) /Before <{}> >> /Length {} >> stream\n{copy}\nendstream endobj\n\
         trailer << /Root 1 0 R >>\n%%EOF",
        bg.len(),
        "00".repeat(32),
        copy.len()
    );
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, fixture.into_bytes()).unwrap();
        app
    });
    h.run_steps(4);
    assert_eq!(marks(&h), [MarkKind::Background]);
    h.state_mut().execute("edit.background.remove");
    h.run_steps(2);
    assert!(marks(&h).is_empty());
    let toast = h.state().toast.as_ref().map(|(t, _)| t.clone()).unwrap_or_default();
    assert_eq!(toast, "Page 1 changed after the background was added, so its own white backdrop stays off. It looks the same on white paper.");
}

/// Two 300 × 400 pages with content (a dark bar), the second turned a quarter (/Rotate 90), so it
/// is shown landscape, 400 × 300.
fn content_fixture() -> Vec<u8> {
    let bar = "0 0 0 rg 40 300 220 60 re f";
    format!(
        "%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /Contents 5 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /Rotate 90 /Contents 5 0 R >> endobj
5 0 obj << /Length {} >>
stream
{bar}
endstream
endobj
trailer << /Root 1 0 R >>
%%EOF",
        bar.len()
    )
    .into_bytes()
}

/// The pictures of page `page` (its thumbnail) painted last frame: paint order, bounds, tint.
fn page_pictures(h: &Harness<'static, PdfCraftApp>, page: usize) -> Vec<(usize, egui::Rect, egui::Color32)> {
    let Some(tex) = h.state().views[0].thumb(page).map(|t| t.id()) else { return Vec::new() };
    h.output()
        .shapes
        .iter()
        .enumerate()
        .filter_map(|(k, s)| match &s.shape {
            egui::Shape::Mesh(m) if m.texture_id == tex => Some((k, m.calc_bounds(), m.vertices.first()?.color)),
            _ => None,
        })
        .collect()
}

/// The preview's picture of `page`, once its thumbnail has arrived (it renders off the UI thread).
fn wait_for_picture(h: &mut Harness<'static, PdfCraftApp>, page: usize) -> (usize, egui::Rect, egui::Color32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        h.step();
        if let [one] = page_pictures(h, page)[..] {
            return one;
        }
        assert!(std::time::Instant::now() < deadline, "the preview never showed page {}'s picture", page + 1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// A text painted on the page last frame (the dialog's text boxes show it too): its paint order
/// and its laid-out rectangle.
fn painted(h: &Harness<'static, PdfCraftApp>, text: &str, page: egui::Rect) -> (usize, egui::Rect) {
    h.output()
        .shapes
        .iter()
        .enumerate()
        .find_map(|(k, s)| match &s.shape {
            egui::Shape::Text(t) if t.galley.text() == text => {
                Some((k, t.galley.rect.translate(t.pos.to_vec2()))).filter(|(_, r)| page.contains(r.center()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("{text:?} is painted on the page"))
}

fn near(a: f32, b: f32, what: &str) {
    assert!((a - b).abs() < 1.0, "{what}: {a} against {b}");
}

#[test]
fn the_preview_shows_the_page_the_range_selects_under_the_marks() {
    use pdfcraft_ui_egui::marks::{PageRange, Subset};
    let mut h = harness_with(content_fixture());
    assert!(h.state_mut().execute("edit.header_footer"));
    h.run_steps(2);
    {
        let d = &mut h.state_mut().marks_draft;
        d.hf.text[1] = "TOP".into();
        d.hf.text[5] = "END".into();
        // Page 2 alone, not the current page: the preview follows the range (#2837).
        d.range = PageRange { all: false, from: 2, to: 2, subset: Subset::All };
    }
    let (at, page, tint) = wait_for_picture(&mut h, 1);
    h.get_by_label("Preview: page 2 of 2");
    assert_eq!(tint, egui::Color32::WHITE);
    assert!((page.width() / page.height() - 4.0 / 3.0).abs() < 0.01, "page 2 is shown turned, as displayed: {page:?}");
    // The marks are drawn over the page, where the engine puts them on its displayed box
    // (default margins: 36 pt top and bottom, 72 pt left and right).
    let s = page.width() / 400.0;
    let (top_at, top) = painted(&h, "TOP", page);
    let (end_at, end) = painted(&h, "END", page);
    assert!(at < top_at && at < end_at, "the page is under the marks");
    near(top.center().x, page.center().x, "centre header, across");
    near(top.center().y, page.top() + 36.0 * s, "centre header, down");
    near(end.right(), page.right() - 72.0 * s, "right footer, across");
    near(end.center().y, page.bottom() - 36.0 * s, "right footer, down");
    h.get_by_label("Cancel").click();
    h.run_steps(2);

    // The watermark sits on the page's centre.
    assert!(h.state_mut().execute("edit.watermark"));
    h.run_steps(2);
    {
        let d = &mut h.state_mut().marks_draft;
        d.wm.text = "DRAFT".into();
        d.wm.rotation = 0.0;
    }
    let (at, page, _) = wait_for_picture(&mut h, 1);
    let (mark_at, mark) = painted(&h, "DRAFT", page);
    assert!(at < mark_at, "the page is under the watermark");
    near(mark.center().x, page.center().x, "watermark, across");
    near(mark.center().y, page.center().y, "watermark, down");
    h.get_by_label("Cancel").click();
    h.run_steps(2);

    // A background colour shows where the page is white: it tints the page's picture.
    assert!(h.state_mut().execute("edit.background"));
    h.run_steps(2);
    let (_, _, tint) = wait_for_picture(&mut h, 1);
    let [red, green, blue] = pdfcraft_engine::Background::default().color.map(|c| (c * 255.0).round() as u8);
    assert_eq!(tint, egui::Color32::from_rgb(red, green, blue), "the default background colour, opaque");
}

/// The preview's page stepper, beside its "page N of M" label: the dialog's own button, not the
/// page navigation's of the same name.
fn preview_step(h: &mut Harness<'static, PdfCraftApp>, label: &str, now: &str) {
    let row = h.get_by_label(now).rect().center().y;
    h.query_all_by_label(label).find(|n| (n.rect().center().y - row).abs() < 15.0).unwrap_or_else(|| panic!("{label} beside {now:?}")).click();
    h.run_steps(2);
}

#[test]
fn the_preview_steps_through_the_pages_and_starts_at_the_current_one() {
    let mut h = harness();
    h.state_mut().views[0].go_to_page(1);
    h.run_steps(2);
    assert!(h.state_mut().execute("edit.watermark"));
    h.run_steps(2);
    h.get_by_label("Preview: page 2 of 3");
    preview_step(&mut h, "Next page", "Preview: page 2 of 3");
    h.get_by_label("Preview: page 3 of 3");
    preview_step(&mut h, "Previous page", "Preview: page 3 of 3");
    preview_step(&mut h, "Previous page", "Preview: page 2 of 3");
    h.get_by_label("Preview: page 1 of 3");
    // Opened again, the preview is back at the current page.
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    assert!(h.state_mut().execute("edit.watermark"));
    h.run_steps(2);
    h.get_by_label("Preview: page 2 of 3");
}

/// Pages from 1 to 1 stays one page: the dialog used to turn "to" back into the last page on
/// every frame while both were 1, so only page 1 could not be chosen alone. The field starts on
/// the last page.
#[test]
fn page_range_from_one_to_one_selects_only_the_first_page() {
    let mut h = harness();
    h.state_mut().execute("edit.background");
    h.run_steps(2);
    assert_eq!(h.state().marks_draft.range.pages(3), [0, 1, 2]);
    h.get_by_label("Pages from").click();
    h.run_steps(2);
    let range = h.state().marks_draft.range;
    assert_eq!((range.from, range.to), (1, 3), "from the first page to the last");
    h.state_mut().marks_draft.range.to = 1;
    h.run_steps(3);
    let range = h.state().marks_draft.range;
    assert_eq!((range.from, range.to), (1, 1), "the range stays as typed");
    h.get_by_label("Applies to 1 page");
    h.get_by_label("OK").click();
    h.run_steps(3);
    let s = h.state();
    let doc = s.session.get(s.views[0].id).unwrap();
    assert_eq!(doc.marks(), [MarkKind::Background]);
    // Rendered: page 1 has the default background colour, page 2 is still blank.
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), pdfcraft_render::RenderConfig::default());
    let corner = |r: &mut pdfcraft_render::PageRenderer, page: usize| {
        let p = r.render(pdfcraft_render::RenderRequest { page, scale: 1.0, ..Default::default() });
        assert!(p.error.is_none(), "{:?}", p.error);
        p.rgba[..4].to_vec()
    };
    let [red, green, blue] = pdfcraft_engine::Background::default().color.map(|c| (c * 255.0).round() as u8);
    assert_eq!(corner(&mut r, 0), vec![red, green, blue, 255], "page 1 has the background");
    assert_eq!(corner(&mut r, 1), vec![255, 255, 255, 255], "page 2 does not");
}
