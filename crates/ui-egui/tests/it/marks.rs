//! Header & footer, watermark and background dialogs in the real shell (egui_kittest).

use crate::support;
use crate::support::AppHarness as _;
use craft::host::ManualJobs;
use craft::pages::ui::PageRangeOptions;
use craft::pages::{PageRange, PageScope, Subset};
use craft::ui::dialog::{DialogId, dialog_id};
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use pdfcraft_engine::MarkKind;
use pdfcraft_ui_egui::PdfCraftApp;
use serde_json::json;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

const BOX_LABELS: [&str; 6] =
    ["Left Header Text", "Center Header Text", "Right Header Text", "Left Footer Text", "Center Footer Text", "Right Footer Text"];
const WATERMARK_TEXT: &str = "Text";
const REPLACE: &str = "Page 1 paints its own white backdrop, which would hide the background. Replace it";

const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >> endobj
3 0 obj << /Type /Page /Parent 2 0 R >> endobj
4 0 obj << /Type /Page /Parent 2 0 R >> endobj
5 0 obj << /Type /Page /Parent 2 0 R >> endobj
trailer << /Root 1 0 R >>
%%EOF";

/// Keeps every texture the app uploads, so a test can read what a picture it paints shows.
#[derive(Clone, Default)]
struct Uploads(Arc<std::sync::Mutex<std::collections::HashMap<egui::TextureId, Arc<egui::ColorImage>>>>);

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

/// The app in a harness whose background jobs wait until the test releases them.
struct Rig {
    h: Harness<'static, PdfCraftApp>,
    jobs: Arc<ManualJobs>,
    uploads: Uploads,
}

impl Deref for Rig {
    type Target = Harness<'static, PdfCraftApp>;
    fn deref(&self) -> &Self::Target {
        &self.h
    }
}

impl DerefMut for Rig {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.h
    }
}

impl Rig {
    /// Runs `n` frames, releasing the queued jobs before each.
    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.jobs.run_all();
            self.h.step();
        }
    }
}

/// The app on a host built over `ManualJobs`.
fn harness() -> Rig {
    harness_with(FIXTURE.to_vec())
}

fn harness_with(pdf: Vec<u8>) -> Rig {
    harness_with_files(pdf, Arc::new(craft::host::ScriptedFiles::new()))
}

fn harness_with_files(pdf: Vec<u8>, files: Arc<craft::host::ScriptedFiles>) -> Rig {
    let jobs = Arc::new(ManualJobs::new());
    let uploads = Uploads::default();
    let on = jobs.clone();
    let h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).renderer(uploads.clone()).build_eframe(move |cc| {
        crate::support::install_runtime(&cc.egui_ctx);
        {
            let mut app = support::test_app(support::host_with(on.clone(), files.clone()));
            app.set_option(&egui::Context::default(), "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "doc.pdf", None, pdf.clone()).unwrap();
            app
        }
    });
    let mut rig = Rig { h, jobs, uploads };
    rig.steps(4);
    rig
}

fn marks(h: &Rig) -> Vec<MarkKind> {
    let s = h.state();
    s.session.get(s.views[0].id).unwrap().marks().to_vec()
}

fn dialog_of(kind: MarkKind) -> DialogId {
    let name = match kind {
        MarkKind::HeaderFooter => "header_footer",
        MarkKind::Watermark => "watermark",
        MarkKind::Background => "background",
        _ => "printer_marks",
    };
    dialog_id(format!("marks.{name}"))
}

fn is_open(h: &Rig) -> bool {
    !h.state().dialogs.is_empty()
}

/// A copy of the open marks dialog's fields.
fn fields(h: &Rig, kind: MarkKind) -> DialogFields {
    h.state().dialogs.get_by_id::<HostedParamDialog>(dialog_of(kind)).expect("the dialog is open").fields().clone()
}

/// Sets a field as `ui.dialog.set` does, for the rows with no control path a click or typing reaches.
fn set_field(h: &mut Rig, kind: MarkKind, field: &str, value: serde_json::Value) {
    let ctx = h.ctx.clone();
    h.state_mut().dialogs.set_json_field(&ctx, dialog_of(kind), field, value).unwrap();
    h.steps(1);
}

fn set_range(h: &mut Rig, kind: MarkKind, range: PageRange) {
    // PLAN: assumed `FieldValue: Serialize`, the JSON form `set_json_field` takes for the range row.
    let value = serde_json::to_value(PageRangeOptions::field_value(&range)).unwrap();
    set_field(h, kind, "range", value);
}

fn type_into(h: &mut Rig, label: &str, text: &str) {
    h.get_by_role_and_label(egui::accesskit::Role::MultilineTextInput, label).focus();
    h.steps(1);
    h.get_by_role_and_label(egui::accesskit::Role::MultilineTextInput, label).type_text(text);
    h.steps(2);
}

fn ok_disabled(h: &Rig) -> bool {
    h.query_by(|n| n.label().as_deref() == Some("OK") && n.is_disabled()).is_some()
}

#[test]
fn header_and_footer_dialog_inserts_tokens_and_applies_to_a_subset() {
    let mut h = harness();
    assert!(!h.execute("edit.header_footer.remove"), "nothing to remove yet");
    assert!(h.execute("edit.header_footer"));
    h.steps(3);
    assert!(is_open(&h));
    // The right footer box takes the page-number token.
    h.get_by_label(BOX_LABELS[5]).click();
    h.steps(2);
    h.get_by_label("Insert Page Number").click();
    h.steps(2);
    assert_eq!(fields(&h, MarkKind::HeaderFooter).text("hf.text.5"), Some("<<1>>"));
    set_range(&mut h, MarkKind::HeaderFooter, PageRange { subset: Subset::Odd, ..PageRange::default() });
    h.get_by_label("OK").click();
    h.steps(4);
    assert!(!is_open(&h));
    assert_eq!(marks(&h), [MarkKind::HeaderFooter]);
    // Update and Remove are now available.
    assert!(h.execute("edit.header_footer.update"));
    h.steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Update Header and Footer");
    h.get_by_label("Cancel").click();
    h.steps(2);
    assert!(h.execute("edit.header_footer.remove"));
    assert!(marks(&h).is_empty());
}

#[test]
fn header_and_footer_dialog_draws_in_the_chosen_face() {
    // #3754: the dialog offers Helvetica, Times and Courier (it only said "Font: Helvetica"),
    // and the face chosen is the one the page's header is drawn in.
    let mut h = harness();
    assert!(h.execute("edit.header_footer"));
    h.steps(3);
    let font = h.get_by_role_and_label(egui::accesskit::Role::ComboBox, "Font");
    assert_eq!(font.value().as_deref(), Some("Helvetica"));
    font.click();
    h.steps(2);
    h.get_by_label("Times").click();
    h.steps(2);
    type_into(&mut h, BOX_LABELS[0], "Serif");
    h.get_by_label("OK").click();
    h.steps(4);
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
    assert!(h.execute("edit.bates"));
    h.steps(3);
    assert_eq!(fields(&h, MarkKind::HeaderFooter).bool("shrink"), Some(false), "off by default, as in Acrobat");
    h.get_by_label("Shrink the page content to make room for the header and footer").click();
    h.steps(2);
    assert_eq!(fields(&h, MarkKind::HeaderFooter).bool("shrink"), Some(true));
    h.get_by_label("OK").click();
    h.steps(4);
    assert!(!is_open(&h));
    assert_eq!(marks(&h), [MarkKind::HeaderFooter]);
}

#[test]
fn watermark_and_background_dialogs() {
    let mut h = harness();
    h.execute("edit.watermark");
    h.steps(2);
    assert!(ok_disabled(&h), "OK waits for text");
    h.get_by_label("Horizontal offset from centre");
    type_into(&mut h, WATERMARK_TEXT, "DRAFT");
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Add Watermark").get_by_label("OK").click();
    h.steps(3);
    h.execute("edit.background");
    h.steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Add Background").get_by_label("OK").click();
    h.steps(3);
    assert_eq!(marks(&h).len(), 2);
}

#[test]
fn backgrounds_and_watermarks_from_a_file() {
    let files = Arc::new(craft::host::ScriptedFiles::answering([craft::host::Picked::Contents(vec![("letterhead.pdf".into(), FIXTURE.to_vec())])]));
    let mut h = harness_with_files(FIXTURE.to_vec(), files);
    h.execute("edit.background");
    h.steps(2);
    h.get_by_role_and_label(egui::accesskit::Role::RadioButton, "File").click();
    h.steps(2);
    assert!(h.get_by_role_and_label(egui::accesskit::Role::TextInput, "File").value().unwrap_or_default().is_empty());
    assert!(ok_disabled(&h), "OK waits for a file");
    h.get_by_label("Browse…").click();
    h.steps(3);
    h.get_by_role_and_label(egui::accesskit::Role::Dialog, "Add Background").get_by_role_and_label(egui::accesskit::Role::SpinButton, "Page number");
    h.get_by_role_and_label(egui::accesskit::Role::Slider, "Scale relative to target page");
    h.get_by_label("OK").click();
    h.steps(3);
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
    let mut h = harness_with(backdrop_fixture());
    let backdrop = |h: &Rig, page| {
        let s = h.state();
        s.session.get(s.views[0].id).unwrap().page_backdrop(page)
    };
    assert_eq!(backdrop(&h, 0), Some(pdfcraft_engine::Backdrop::Fill));
    h.execute("edit.background");
    h.steps(3);
    h.get_by_label("A picture covers page 2 (a scan?), so the background won't show there.");
    assert_eq!(fields(&h, MarkKind::Background).bool("replace_backdrop"), Some(true), "on by default");
    // Left unticked, the backdrop stays and still hides the background.
    h.get_by_label(REPLACE).click();
    h.steps(2);
    assert_eq!(fields(&h, MarkKind::Background).bool("replace_backdrop"), Some(false));
    h.get_by_label("OK").click();
    h.steps(3);
    assert_eq!(backdrop(&h, 0), Some(pdfcraft_engine::Backdrop::Fill));
    assert!(h.execute("edit.undo"));
    h.steps(2);
    // Reopened, the choice starts on again; ticked, the backdrop is replaced and the scan is
    // reported, not touched.
    h.execute("edit.background");
    h.steps(3);
    h.get_by_label(REPLACE);
    assert_eq!(fields(&h, MarkKind::Background).bool("replace_backdrop"), Some(true), "on again when the dialog opens");
    h.get_by_label("OK").click();
    h.steps(3);
    assert_eq!(backdrop(&h, 0), None);
    assert_eq!(backdrop(&h, 1), Some(pdfcraft_engine::Backdrop::Image));
    // Nothing left to replace: the offer is gone, and Update keeps the backdrop replaced.
    h.execute("edit.background.update");
    h.steps(3);
    assert!(h.query_by_label(REPLACE).is_none());
    h.get_by_label("OK").click();
    h.steps(3);
    assert_eq!(backdrop(&h, 0), None, "Update keeps the backdrop replaced");
}

/// The preview shows what OK will do to each page: the colour on bare paper and on a page whose
/// white backdrop is replaced, none on a scan, and none on that page with Replace unticked.
#[test]
fn the_background_preview_follows_the_replace_checkbox() {
    let mut h = harness_with(backdrop_fixture());
    assert!(h.execute("edit.background"));
    h.steps(3);
    h.get_by_label(REPLACE);
    let [red, green, blue] = fields(&h, MarkKind::Background).color("marks.bg.colour").unwrap().map(|c| (f64::from(c) * 255.0).round() as u8);
    let colour = egui::Color32::from_rgb(red, green, blue);
    let tint = |h: &mut Rig, page: usize| {
        show_page(h, page);
        wait_for_preview(h, "the page's picture", |_| true).tint
    };
    assert_eq!(tint(&mut h, 0), colour, "page 1's backdrop is replaced, so the colour shows");
    assert_eq!(tint(&mut h, 1), egui::Color32::WHITE, "a picture covers page 2");
    assert_eq!(tint(&mut h, 2), colour, "page 3 is bare paper");
    h.get_by_label(REPLACE).click();
    h.steps(2);
    assert_eq!(fields(&h, MarkKind::Background).bool("replace_backdrop"), Some(false));
    assert_eq!(tint(&mut h, 0), egui::Color32::WHITE, "unticked, page 1 keeps its backdrop, which hides the colour");
    assert_eq!(tint(&mut h, 2), colour, "page 3 has no backdrop to keep");
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
    let mut h = harness_with(fixture.into_bytes());
    assert_eq!(marks(&h), [MarkKind::Background]);
    h.execute("edit.background.remove");
    h.steps(2);
    assert!(marks(&h).is_empty());
    assert_eq!(
        support::shown_notice(&mut h.h).as_deref(),
        Some("Page 1 changed after the background was added, so its own white backdrop stays off. It looks the same on white paper.")
    );
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

/// The preview pager's position, read from its caption's accessible value: the shown page and the count.
fn is_pager(ak: &egui_kittest::kittest::AccessKitNode<'_>) -> bool {
    ak.role() == egui::accesskit::Role::Label && ak.numeric_value().is_some() && ak.label().is_some_and(|label| label.starts_with("Preview: page "))
}

fn pager(h: &Rig) -> (usize, usize) {
    let node = h.query_by(is_pager).expect("the preview has a pager");
    let ak = node.accesskit_node();
    (ak.numeric_value().unwrap_or_default() as usize, ak.label().unwrap().split_whitespace().last().unwrap().parse().unwrap())
}

/// Presses the pager's step button: the dialog's own, not the page navigation's of the same name.
fn press_step(h: &mut Rig, label: &str) {
    let node = h.query_by(is_pager).expect("the preview has a pager");
    // The caption and its step buttons are the children of one row.
    node.parent().expect("the pager sits in a row").get_by_label(label).click();
    h.steps(4);
}

/// Steps the preview to `page` (0-based) with the pager's buttons.
fn show_page(h: &mut Rig, page: usize) {
    for _ in 0..16 {
        let (shown, _) = pager(h);
        if shown == page + 1 {
            return;
        }
        press_step(h, if shown < page + 1 { "Next" } else { "Previous" });
    }
    panic!("the pager never reached page {}", page + 1);
}

/// The preview pane's picture as painted last frame.
struct Picture {
    /// Paint order of the mesh.
    at: usize,
    rect: egui::Rect,
    tint: egui::Color32,
    image: Arc<egui::ColorImage>,
}

/// Paint groups preserve their children's order in the frame.
fn frame_shapes(h: &Rig) -> Vec<&egui::Shape> {
    fn append<'a>(shape: &'a egui::Shape, out: &mut Vec<&'a egui::Shape>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| append(shape, out)),
            _ => out.push(shape),
        }
    }
    let mut shapes = Vec::new();
    h.output().shapes.iter().for_each(|s| append(&s.shape, &mut shapes));
    shapes
}

/// The picture of the preview pane: the largest uploaded texture painted beside the pager (the small page picture
/// stands at its left, as in production).
fn preview_picture(h: &Rig) -> Option<Picture> {
    let caption = h.query_by(is_pager)?;
    let row = caption.parent()?;
    let pager_rect = caption.rect().union(row.query_by_label("Previous")?.rect()).union(row.query_by_label("Next")?.rect());
    let uploads = h.uploads.0.lock().unwrap();
    frame_shapes(h)
        .into_iter()
        .enumerate()
        .filter_map(|(at, s)| {
            let egui::Shape::Mesh(m) = s else { return None };
            let rect = m.calc_bounds();
            let over = rect.width() > 30.0
                && rect.right() <= pager_rect.left() + 4.0
                && rect.top() < pager_rect.bottom()
                && rect.bottom() > pager_rect.top();
            if !over {
                return None;
            }
            // The pane clips the mesh at its frame. Its UVs identify the full picture's bounds.
            let uv = egui::Rect::from_min_max(
                egui::pos2(
                    m.vertices.iter().map(|v| v.uv.x).fold(f32::INFINITY, f32::min),
                    m.vertices.iter().map(|v| v.uv.y).fold(f32::INFINITY, f32::min),
                ),
                egui::pos2(
                    m.vertices.iter().map(|v| v.uv.x).fold(f32::NEG_INFINITY, f32::max),
                    m.vertices.iter().map(|v| v.uv.y).fold(f32::NEG_INFINITY, f32::max),
                ),
            );
            if uv.width() <= 0.0 || uv.height() <= 0.0 {
                return None;
            }
            let size = egui::vec2(rect.width() / uv.width(), rect.height() / uv.height());
            let min = rect.min - egui::vec2(uv.min.x * size.x, uv.min.y * size.y);
            Some(Picture {
                at,
                rect: egui::Rect::from_min_size(min, size),
                tint: m.vertices.first()?.color,
                image: uploads.get(&m.texture_id).cloned()?,
            })
        })
        .max_by(|a, b| (a.rect.area()).total_cmp(&b.rect.area()))
}

/// [`preview_picture`], waiting until `ready` accepts its image: the jobs are released each frame.
fn wait_for_preview(h: &mut Rig, what: &str, ready: impl Fn(&egui::ColorImage) -> bool) -> Picture {
    let mut last = None;
    for _ in 0..200 {
        h.steps(1);
        if let Some(p) = preview_picture(h) {
            if ready(&p.image) {
                return p;
            }
            last = Some((p.tint, p.image.size, at(&p.image, 0.5, 0.75)));
        }
    }
    panic!("the preview never showed {what}; last (tint, size, middle of the lower half): {last:?}");
}

/// A text painted on the page last frame (the dialog's text boxes show it too): its paint order
/// and its laid-out rectangle.
fn painted(h: &Rig, text: &str, page: egui::Rect) -> (usize, egui::Rect) {
    frame_shapes(h)
        .into_iter()
        .enumerate()
        .find_map(|(k, s)| match s {
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

#[ignore = "known gap of the egui-to-craft port: see docs/port-known-gaps.md"]
#[test]
fn the_preview_shows_the_page_the_range_selects_under_the_marks() {
    let mut h = harness_with(content_fixture());
    assert!(h.execute("edit.header_footer"));
    h.steps(2);
    type_into(&mut h, BOX_LABELS[1], "TOP");
    type_into(&mut h, BOX_LABELS[5], "END");
    // Page 2 alone, not the current page: the preview follows the range (#2837).
    set_range(&mut h, MarkKind::HeaderFooter, PageRange { scope: PageScope::Range, from: 2, to: Some(2), subset: Subset::All });
    let Picture { at, rect: page, tint, .. } = wait_for_preview(&mut h, "page 2", |_| true);
    assert_eq!(pager(&h), (2, 2));
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
    h.steps(2);

    // The watermark sits on the page's centre.
    assert!(h.execute("edit.watermark"));
    h.steps(2);
    type_into(&mut h, WATERMARK_TEXT, "DRAFT");
    set_field(&mut h, MarkKind::Watermark, "wm.rotation", json!(0.0));
    show_page(&mut h, 1);
    let Picture { at, rect: page, .. } = wait_for_preview(&mut h, "page 2 under the watermark", |_| true);
    let (mark_at, mark) = painted(&h, "DRAFT", page);
    assert!(at < mark_at, "the page is under the watermark");
    near(mark.center().x, page.center().x, "watermark, across");
    near(mark.center().y, page.center().y, "watermark, down");
    h.get_by_label("Cancel").click();
    h.steps(2);

    // A background colour shows where the page is white: it tints the page's picture.
    assert!(h.execute("edit.background"));
    h.steps(2);
    show_page(&mut h, 1);
    let tint = wait_for_preview(&mut h, "page 2 under the background", |_| true).tint;
    let [red, green, blue] = fields(&h, MarkKind::Background).color("marks.bg.colour").unwrap().map(|c| (f64::from(c) * 255.0).round() as u8);
    assert_eq!(tint, egui::Color32::from_rgb(red, green, blue), "the default background colour, opaque");
}

#[test]
fn the_preview_steps_through_the_pages_and_starts_at_the_current_one() {
    let mut h = harness();
    h.state_mut().views[0].go_to_page(1);
    h.steps(2);
    assert!(h.execute("edit.watermark"));
    h.steps(2);
    assert_eq!(pager(&h), (2, 3));
    press_step(&mut h, "Next");
    assert_eq!(pager(&h), (3, 3));
    press_step(&mut h, "Next");
    assert_eq!(pager(&h), (3, 3), "Next does nothing at the last page");
    press_step(&mut h, "Previous");
    press_step(&mut h, "Previous");
    assert_eq!(pager(&h), (1, 3));
    press_step(&mut h, "Previous");
    assert_eq!(pager(&h), (1, 3), "Back does nothing at the first page");
    // Opened again, the preview is back at the current page.
    h.get_by_label("Cancel").click();
    h.steps(2);
    assert!(h.execute("edit.watermark"));
    h.steps(2);
    assert_eq!(pager(&h), (2, 3));
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
    let mut h = harness_with(content_fixture());
    let yellow = egui::Color32::from_rgb(255, 255, 153);
    let red = |c: egui::Color32| c.r() > 200 && c.g() < 80 && c.b() < 80;
    let blue = |c: egui::Color32| c.b() > 150 && c.r() < 100 && c.g() < 100;
    let dark = |c: egui::Color32| c.r() < 60 && c.g() < 60 && c.b() < 60;
    let apply = |h: &mut Rig, command: &str, kind: MarkKind, values: &[(&str, serde_json::Value)]| {
        assert!(h.execute(command));
        h.steps(2);
        for (field, value) in values {
            set_field(h, kind, field, value.clone());
        }
        h.steps(2);
        h.get_by_label("OK").click();
        h.steps(3);
    };
    // A pale yellow background, a red watermark and a blue header.
    apply(&mut h, "edit.background", MarkKind::Background, &[("marks.bg.colour", json!([1.0, 1.0, 0.6]))]);
    apply(
        &mut h,
        "edit.watermark",
        MarkKind::Watermark,
        &[("wm.text", json!("OLD")), ("marks.wm.colour", json!([1.0, 0.0, 0.0])), ("wm.opacity", json!(100.0)), ("wm.rotation", json!(0.0))],
    );
    apply(
        &mut h,
        "edit.header_footer",
        MarkKind::HeaderFooter,
        &[("hf.text.1", json!("OLD")), ("marks.hf.colour", json!([0.0, 0.0, 1.0])), ("hf.size", json!(40.0))],
    );
    // Add keeps the marks there, and previews on the thumbnail, which has all three.
    assert!(h.execute("edit.watermark"));
    h.steps(2);
    let all = |i: &egui::ColorImage| at(i, 0.5, 0.75) == yellow && count(i, red) > 200 && count(i, blue) > 40;
    wait_for_preview(&mut h, "the page with its marks", all);
    h.get_by_label("Cancel").click();
    h.steps(2);

    assert!(h.execute("edit.background.update"));
    h.steps(2);
    set_field(&mut h, MarkKind::Background, "marks.bg.colour", json!([0.6, 0.8, 1.0]));
    // Update shows the page without the marks: neither a blank sheet nor the old marks while it renders.
    let unmarked = |i: &egui::ColorImage| at(i, 0.5, 0.75) == egui::Color32::WHITE;
    let Picture { tint, image: page, .. } = wait_for_preview(&mut h, "the page without its background", unmarked);
    assert_eq!(tint, egui::Color32::from_rgb(153, 204, 255), "the new colour tints the page");
    assert!(count(&page, red) > 200 && count(&page, blue) > 40 && count(&page, dark) > 1000, "the watermark, header and page content stay");
    // So does a step to page 2: the pages are prepared once, and each is drawn as it is shown.
    show_page(&mut h, 1);
    wait_for_preview(&mut h, "page 2 without its background", unmarked);
    h.get_by_label("Cancel").click();
    h.steps(2);

    assert!(h.execute("edit.watermark.update"));
    h.steps(2);
    set_field(&mut h, MarkKind::Watermark, "wm.text", json!("NEW"));
    let page = wait_for_preview(&mut h, "the page without the old watermark", |i| count(i, red) == 0).image;
    assert!(at(&page, 0.5, 0.75) == yellow && count(&page, blue) > 40, "the background and header stay");
    h.get_by_label("Cancel").click();
    h.steps(2);

    assert!(h.execute("edit.header_footer.update"));
    h.steps(2);
    let page = wait_for_preview(&mut h, "the page without the old header", |i| count(i, blue) == 0).image;
    assert!(at(&page, 0.5, 0.75) == yellow && count(&page, red) > 200, "the background and watermark stay");
}
