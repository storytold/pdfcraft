//! Header & footer, watermark and background dialogs in the real shell (egui_kittest).

use crate::support;
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
    support::outside_menus(&h, "File").click();
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
    assert_eq!(tint, egui::Color32::from_rgb(255, 255, 217), "the default pale yellow, opaque");
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

/// Keeps every texture the app uploads, so a test can read what a picture it paints shows.
#[derive(Clone, Default)]
struct Uploads(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<egui::TextureId, std::sync::Arc<egui::ColorImage>>>>);

impl egui_kittest::TestRenderer for Uploads {
    fn handle_delta(&mut self, delta: &mut egui::TexturesDelta) {
        let mut seen = self.0.lock().unwrap();
        for (id, images) in delta.set.drain() {
            for d in images.into_iter().filter(|d| d.pos.is_none()) {
                let egui::ImageData::Color(image) = d.image;
                seen.insert(id, image);
            }
        }
        // Freed textures stay readable: ids are never reused.
        delta.free.clear();
    }

    fn render(&mut self, _: &egui::Context, _: &egui::FullOutput) -> Result<image::RgbaImage, String> {
        Err("this renderer only keeps the uploads".into())
    }
}

/// The page picture the dialog's preview painted last frame, left of its "Preview: page …" label,
/// once its texture is in: its tint and its image.
fn preview_picture(h: &Harness<'static, PdfCraftApp>, uploads: &Uploads) -> Option<(egui::Color32, std::sync::Arc<egui::ColorImage>)> {
    let label = h.query_by_label_contains("Preview: page")?.rect();
    h.output().shapes.iter().find_map(|s| {
        let egui::Shape::Mesh(m) = &s.shape else { return None };
        let r = m.calc_bounds();
        // The size of the 210-point box left of the label (which is centred down its side), and
        // not the stepper's arrow beside the label. A landscape page is shorter than the box, so
        // it can sit below the label: within half the box of it.
        let beside = r.width() > 50.0
            && r.height() <= 215.0
            && r.right() <= label.left()
            && label.left() - r.right() < 260.0
            && (r.center().y - label.center().y).abs() <= 105.0;
        if !beside {
            return None;
        }
        let image = uploads.0.lock().unwrap().get(&m.texture_id).cloned()?;
        Some((m.vertices.first()?.color, image))
    })
}

/// [`preview_picture`], waiting until `ready` accepts it (the page renders off the UI thread).
fn wait_for_preview(
    h: &mut Harness<'static, PdfCraftApp>,
    uploads: &Uploads,
    what: &str,
    ready: impl Fn(&egui::ColorImage) -> bool,
) -> (egui::Color32, std::sync::Arc<egui::ColorImage>) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        h.step();
        let shown = preview_picture(h, uploads);
        if let Some((tint, image)) = shown.clone().filter(|(_, image)| ready(image)) {
            return (tint, image);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the preview never showed {what}; last (tint, size, middle of the lower half): {:?}",
            shown.map(|(tint, i)| (tint, i.size, at(&i, 0.5, 0.75)))
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// The pixel of `image` at (u, v), from 0 to 1 across and down.
fn at(image: &egui::ColorImage, u: f32, v: f32) -> egui::Color32 {
    let [w, h] = image.size;
    let (x, y) = (((u * w as f32) as usize).min(w - 1), ((v * h as f32) as usize).min(h - 1));
    image.pixels[y * w + x]
}

fn count(image: &egui::ColorImage, f: impl Fn(egui::Color32) -> bool) -> usize {
    image.pixels.iter().filter(|&&c| f(c)).count()
}

/// Update draws its preview on the page without the marks it replaces, which the page's thumbnail
/// still has: a pale yellow background updated to light blue previews light blue, not blue
/// multiplied onto the yellow, and a new watermark or header isn't drawn over the old one. The
/// page's content and its other kinds of mark stay.
#[test]
fn update_previews_the_page_without_the_marks_it_replaces() {
    let uploads = Uploads::default();
    let pdf = content_fixture();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).renderer(uploads.clone()).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("doc.pdf", None, pdf.clone()).unwrap();
        app
    });
    h.run_steps(4);
    let yellow = egui::Color32::from_rgb(255, 255, 153);
    let red = |c: egui::Color32| c.r() > 200 && c.g() < 80 && c.b() < 80;
    let blue = |c: egui::Color32| c.b() > 150 && c.r() < 100 && c.g() < 100;
    let dark = |c: egui::Color32| c.r() < 60 && c.g() < 60 && c.b() < 60;
    let apply = |h: &mut Harness<'static, PdfCraftApp>, command: &str, draft: &dyn Fn(&mut pdfcraft_ui_egui::marks::MarksDraft)| {
        assert!(h.state_mut().execute(command));
        h.run_steps(2);
        draft(&mut h.state_mut().marks_draft);
        h.run_steps(2);
        h.get_by_label("OK").click();
        h.run_steps(3);
    };
    // A pale yellow background, a red watermark and a blue header.
    apply(&mut h, "edit.background", &|d| d.bg.color = [1.0, 1.0, 0.6]);
    apply(&mut h, "edit.watermark", &|d| {
        d.wm.text = "OLD".into();
        d.wm.color = [1.0, 0.0, 0.0];
        d.wm.opacity = 1.0;
        d.wm.rotation = 0.0;
    });
    apply(&mut h, "edit.header_footer", &|d| {
        d.hf.text[1] = "OLD".into();
        d.hf.color = [0.0, 0.0, 1.0];
        d.hf.font_size = 40.0;
    });
    // Add keeps the marks there, and previews on the thumbnail, which has all three.
    assert!(h.state_mut().execute("edit.watermark"));
    h.run_steps(2);
    let all = |i: &egui::ColorImage| at(i, 0.5, 0.75) == yellow && count(i, red) > 200 && count(i, blue) > 40;
    wait_for_preview(&mut h, &uploads, "the page with its marks", all);
    h.get_by_label("Cancel").click();
    h.run_steps(2);

    assert!(h.state_mut().execute("edit.background.update"));
    h.run_steps(2);
    h.state_mut().marks_draft.bg.color = [0.6, 0.8, 1.0];
    h.step();
    // Update shows the page without the marks from the frame it opens: neither a blank sheet nor
    // the old marks while it renders.
    let shown = |h: &Harness<'static, PdfCraftApp>| preview_picture(h, &uploads).expect("a page is previewed");
    let (tint, page) = shown(&h);
    assert_eq!(tint, egui::Color32::from_rgb(153, 204, 255), "the new colour tints the page");
    assert_eq!(at(&page, 0.5, 0.75), egui::Color32::WHITE, "the page is previewed without the yellow background");
    assert!(count(&page, red) > 200 && count(&page, blue) > 40 && count(&page, dark) > 1000, "the watermark, header and page content stay");
    // So does a step to page 2: the pages are prepared once, and each is drawn as it is shown.
    h.state_mut().marks_draft.preview = Some(1);
    h.step();
    assert_eq!(at(&shown(&h).1, 0.5, 0.75), egui::Color32::WHITE, "page 2 is previewed without its background at once");
    h.get_by_label("Cancel").click();
    h.run_steps(2);

    assert!(h.state_mut().execute("edit.watermark.update"));
    h.run_steps(2);
    h.state_mut().marks_draft.wm.text = "NEW".into();
    h.step();
    let (_, page) = shown(&h);
    assert_eq!(count(&page, red), 0, "the new watermark isn't previewed over the old one");
    assert!(at(&page, 0.5, 0.75) == yellow && count(&page, blue) > 40, "the background and header stay");
    h.get_by_label("Cancel").click();
    h.run_steps(2);

    assert!(h.state_mut().execute("edit.header_footer.update"));
    h.run_steps(2);
    let (_, page) = shown(&h);
    assert_eq!(count(&page, blue), 0, "the new header isn't previewed over the old one");
    assert!(at(&page, 0.5, 0.75) == yellow && count(&page, red) > 200, "the background and watermark stay");
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
    // Rendered: page 1 has the (default cream) background, page 2 is still blank.
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), pdfcraft_render::RenderConfig::default());
    let corner = |r: &mut pdfcraft_render::PageRenderer, page: usize| {
        let p = r.render(pdfcraft_render::RenderRequest { page, scale: 1.0, ..Default::default() });
        assert!(p.error.is_none(), "{:?}", p.error);
        p.rgba[..4].to_vec()
    };
    assert_eq!(corner(&mut r, 0), vec![255, 255, 217, 255], "page 1 has the background");
    assert_eq!(corner(&mut r, 1), vec![255, 255, 255, 255], "page 2 does not");
}
