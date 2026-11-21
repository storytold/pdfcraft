//! A page that paints the same content over and over runs out of rasterizer coverage: what is
//! past it is left out and said to be, rather than panicking once a strip's alpha index no longer
//! fits (PDFBOX-5415's map page drew one form 2^22 times and panicked at 7 GB). Every scene is
//! kept to a coverage budget well below that index's limit, and drawing stops once it is spent
//! (the same page then held 5.4 GB for 76 s; it now holds about 240 MB for 4 s).

use super::*;
use std::sync::Mutex;

/// A blue circle, filled.
const FILLED_CIRCLE: &str = "0 0 1 rg 100 160 m 133 160 160 133 160 100 c 160 67 133 40 100 40 c 67 40 40 67 40 100 c 40 133 67 160 100 160 c f";

/// A page whose form `F0` draws `F1` twice, `F1` draws `F2` twice, and so on `levels` deep; the
/// last form fills a circle. The page draws `F0` once, so the circle is filled 2^`levels` times.
fn doubling_forms(levels: usize) -> Vec<u8> {
    doubling_forms_then(levels, FILLED_CIRCLE, "")
}

/// [`doubling_forms`] with the last form painting `leaf` instead, and the page painting `after`
/// once the forms are done.
fn doubling_forms_then(levels: usize, leaf: &str, after: &str) -> Vec<u8> {
    doubling_forms_page(levels, leaf, &format!("/F0 Do {after}"), "/XObject << /F0 5 0 R >>")
}

/// A page with `content` and `resources` (inside its resource dictionary), and the forms of
/// [`doubling_forms`] from object 5 (`F0`) on, the last one painting `leaf`.
fn doubling_forms_page(levels: usize, leaf: &str, content: &str, resources: &str) -> Vec<u8> {
    let mut objs: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),
        // Object 4 is the page's content; forms start at 5.
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << {resources} >> >>"),
        String::new(),
    ];
    objs[3] = format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len());
    for level in 0..=levels {
        let (body, resources) = if level == levels {
            (leaf.to_string(), String::new())
        } else {
            ("/F Do /F Do".to_string(), format!("/Resources << /XObject << /F {} 0 R >> >>", 6 + level))
        };
        objs.push(format!("<< /Type /XObject /Subtype /Form /BBox [0 0 200 200] {resources} /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    pdf_of(&objs)
}

/// A PDF of the objects `objs`, numbered from 1, with a cross-reference table.
fn pdf_of(objs: &[String]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// Renders page 1 with the rasterizer kept to `max_coverage` values, returning its pixels and
/// whether it said content was left out.
fn render_kept_to(pdf: Vec<u8>, max_coverage: Option<usize>) -> (Vec<u8>, bool) {
    let parsed = Pdf::new(Arc::new(pdf)).unwrap();
    let truncated = Arc::new(Mutex::new(false));
    let seen = truncated.clone();
    let settings = InterpreterSettings {
        warning_sink: Arc::new(move |w| {
            if matches!(w, InterpreterWarning::ContentTruncated) {
                *seen.lock().unwrap() = true;
            }
        }),
        ..Default::default()
    };
    let pixmap =
        hayro::render(&parsed.pages()[0], &RenderCache::new(), &settings, &RenderSettings { bg_color: WHITE, max_coverage, ..Default::default() });
    let cut = *truncated.lock().unwrap();
    (pixmap.data_as_u8_slice().to_vec(), cut)
}

#[test]
fn a_page_past_its_coverage_leaves_the_rest_out_and_says_so() {
    // 2^10 circles, each storing its edge tiles' coverage.
    let pdf = doubling_forms(10);
    let (whole, whole_cut) = render_kept_to(pdf.clone(), None);
    assert!(!whole_cut, "within the limit nothing is left out");
    let blue = |px: &[u8]| px.as_chunks::<4>().0.iter().filter(|p| p[2] > 200 && p[0] < 50).count();
    assert!(blue(&whole) > 10_000, "the circle is drawn");

    // Room for a few circles only: the rest are left out, the page still renders, and says so.
    let (kept, cut) = render_kept_to(pdf, Some(200_000));
    assert!(cut, "the page says content was left out");
    assert_eq!(kept.len(), whole.len());
    assert!(blue(&kept) > 10_000, "the circles drawn before the limit stay: {}", blue(&kept));
}

/// Pixels of `px` that are mostly red.
fn red(px: &[u8]) -> usize {
    px.as_chunks::<4>().0.iter().filter(|p| p[0] > 200 && p[2] < 50).count()
}

/// The page paints `leaf` 2^10 times through forms and then `marker` (red, away from the leaf's
/// drawing) once, both through the same rasterizer path. Kept to 200,000 coverage values, the
/// marker is past the limit: it is left out and the page says so. Unlimited, it is drawn.
fn leaves_out_past_the_limit(leaf: &str, marker: &str) {
    let pdf = doubling_forms_then(10, leaf, marker);
    let (whole, whole_cut) = render_kept_to(pdf.clone(), None);
    assert!(!whole_cut, "within the limit nothing is left out");
    assert!(red(&whole) > 50, "unlimited, the marker is drawn: {}", red(&whole));

    let (kept, cut) = render_kept_to(pdf, Some(200_000));
    assert!(cut, "the page says content was left out");
    assert_eq!(kept.len(), whole.len());
    assert_eq!(red(&kept), 0, "the marker, past the limit, is left out");
}

#[test]
fn strokes_past_the_coverage_are_left_out() {
    // A stroked circle and a stroked curve: neither is a rectangle, so both go through
    // `stroke_path`, and the page's only coverage is strokes.
    leaves_out_past_the_limit(
        "0 0 1 RG 4 w 100 160 m 133 160 160 133 160 100 c 160 67 133 40 100 40 c 67 40 40 67 40 100 c 40 133 67 160 100 160 c S",
        "1 0 0 RG 6 w 170 10 m 180 30 190 10 195 30 c S",
    );
}

#[test]
fn fast_rectangles_past_the_coverage_are_left_out() {
    // Axis-aligned rectangles fill through `fill_rect_fast`, and are the page's only coverage.
    leaves_out_past_the_limit("0 0 1 rg 40.5 40.5 119 119 re f", "1 0 0 rg 170.5 10.5 20 20 re f");
}

#[test]
fn layer_clips_past_the_coverage_are_left_out() {
    // hayro pushes no clipped layers (its clips go through `push_clip_path`), so the layer clip
    // is driven through the vello_cpu context directly, with clips as the scene's only coverage.
    use hayro::vello_cpu::color::palette::css::RED;
    use hayro::vello_cpu::kurbo::{Circle, Rect, Shape};
    use hayro::vello_cpu::{Pixmap, RenderContext, Resources};

    let clip = Circle::new((50.0, 50.0), 40.0).to_path(0.1);
    let mut ctx = RenderContext::new(100, 100);
    // Room while fewer than 30,000 - 2 x 100 x 100 = 10,000 values are stored.
    ctx.set_max_coverage(30_000);
    assert!(!ctx.coverage_full(), "an empty scene has room");

    // Below the limit the clip is kept: a fill inside it shows within the circle only.
    ctx.push_clip_layer(&clip);
    assert!(!ctx.coverage_full(), "the first clip fits");
    ctx.set_paint(RED);
    ctx.fill_rect(&Rect::new(0.0, 0.0, 100.0, 100.0));
    ctx.pop_layer();
    assert!(!ctx.coverage_full(), "the fill inside it fits too");
    let mut pixmap = Pixmap::new(100, 100);
    ctx.flush();
    ctx.render_to_pixmap(&mut Resources::new(), &mut pixmap);
    let px = |x: usize, y: usize| pixmap.data_as_u8_slice().get((y * 100 + x) * 4..(y * 100 + x) * 4 + 4).map(<[u8]>::to_vec);
    assert_eq!(px(50, 50), Some(vec![255, 0, 0, 255]), "inside the clip");
    assert_eq!(px(2, 2), Some(vec![0, 0, 0, 0]), "outside the clip");

    ctx.reset();
    for _ in 0..1_000 {
        ctx.push_clip_layer(&clip);
        ctx.pop_layer();
    }
    assert!(ctx.coverage_full(), "the clips past the limit were left out, and that is said");
    // The scene still rasterizes: the layers left unclipped have nothing drawn into them.
    ctx.flush();
    ctx.render_to_pixmap(&mut Resources::new(), &mut pixmap);

    ctx.reset();
    assert!(!ctx.coverage_full(), "a reset scene starts with room");
}

/// Past the limit the clip itself is refused, not only what is drawn into its layer: with room
/// for exactly one path, a fill takes it and the clip pushed next is what finds the scene full.
#[test]
fn a_layer_clip_past_the_coverage_is_itself_left_out() {
    use hayro::vello_cpu::RenderContext;
    use hayro::vello_cpu::color::palette::css::RED;
    use hayro::vello_cpu::kurbo::{Circle, Rect, Shape};

    let mut ctx = RenderContext::new(100, 100);
    // Room while fewer than 20,001 - 2 x 100 x 100 = 1 value is stored: for one path.
    ctx.set_max_coverage(20_001);
    ctx.set_paint(RED);
    ctx.fill_path(&Rect::new(10.5, 10.5, 30.5, 30.5).to_path(0.1));
    assert!(!ctx.coverage_full(), "the fill fits");
    ctx.push_clip_layer(&Circle::new((50.0, 50.0), 40.0).to_path(0.1));
    assert!(ctx.coverage_full(), "the clip, past the limit, is left out and that is said");
    ctx.pop_layer();
}

/// A fill of a hundred stripes one unit wide across the 200 x 200 page, two units apart. Every
/// tile of the canvas holds an edge of one, so one fill stores coverage for the whole canvas
/// (40,000 values at 72 dpi).
fn stripes() -> String {
    let stripes: Vec<String> = (0..100).map(|i| format!("{}.5 0 1 200 re", i * 2)).collect();
    format!("0 g {} f", stripes.join(" "))
}

/// Renders page 1 of `pdf` at 72 dpi with the default settings, returning the paths the
/// renderer was asked to draw (`hayro::paths_drawn`), whether the page said content was left
/// out, and the share of its pixels that are not white.
fn render_counted(pdf: Vec<u8>) -> (u64, bool, f64) {
    let parsed = Pdf::new(Arc::new(pdf)).unwrap();
    let truncated = Arc::new(Mutex::new(false));
    let seen = truncated.clone();
    let settings = InterpreterSettings {
        warning_sink: Arc::new(move |w| {
            if matches!(w, InterpreterWarning::ContentTruncated) {
                *seen.lock().unwrap() = true;
            }
        }),
        ..Default::default()
    };
    let before = hayro::paths_drawn();
    let pixmap = hayro::render(&parsed.pages()[0], &RenderCache::new(), &settings, &RenderSettings { bg_color: WHITE, ..Default::default() });
    let drawn = hayro::paths_drawn() - before;
    let cut = *truncated.lock().unwrap();
    let px = pixmap.data_as_u8_slice().as_chunks::<4>().0;
    let inked = px.iter().filter(|p| p[0] < 250).count() as f64 / px.len() as f64;
    (drawn, cut, inked)
}

/// The leaves a page of forms drawing the next form twice, thirteen deep, paints: 2^13 fills of
/// [`stripes`], each storing coverage for the whole canvas.
const BOMB_LEAVES: u64 = 1 << 13;

/// About how many fills of [`stripes`] the default coverage budget of a 200 x 200 canvas holds,
/// at the 40,000 values each stores (vello_cpu patch 1 also keeps room for twice the canvas, so
/// a couple fewer fit).
fn stripes_within_the_budget() -> u64 {
    let budget = hayro::coverage_budget(200, 200) as u64;
    budget / 40_000
}

/// Forms that draw the next form twice (PDFBOX-5415 drew one twenty deep, under soft masks) fill
/// the rasterizer's coverage long before the page's nested-paint budget runs out. Each path's
/// coverage stays in memory until the page is rasterized, so the page grew to the 2^31 values a
/// strip can index (5.4 GB, 76 s) before what came after was left out. The page now stops at its
/// default coverage budget (hayro patch 34), and the interpreter stops drawing once the renderer
/// has no room (hayro-interpret patch 112): the fills drawn are those the budget holds, not
/// every leaf, what fitted is drawn, and the page says the rest was left out.
#[test]
fn forms_drawing_the_next_twice_stop_at_the_default_coverage_budget() {
    assert_eq!(hayro::coverage_budget(200, 200), 1 << 26, "a small canvas gets the floor");
    let held = stripes_within_the_budget();
    assert!(held < BOMB_LEAVES / 4, "the page is past its budget: {held} of {BOMB_LEAVES} leaves fit");

    let (drawn, cut, inked) = render_counted(doubling_forms_then(13, &stripes(), ""));
    assert!(cut, "the page says content was left out");
    assert!(drawn <= held + 1, "drawing stops at the budget: {drawn} paths drawn, {held} fit, {BOMB_LEAVES} leaves");
    assert!(drawn + 2 >= held, "what the budget holds is drawn: {drawn} paths drawn, {held} fit");
    assert!(inked > 0.4, "the stripes that fitted show: {inked}");
}

/// The same forms as a page's soft mask: the mask is drawn as a scene of its own, which has the
/// same budget, and drawing it stops there too, keeping what fitted. Its stripes are white, so
/// the mask lets the page's blue fill through where they are drawn.
#[test]
fn forms_drawing_the_next_twice_in_a_soft_mask_stop_at_its_coverage_budget() {
    let white_stripes = stripes().replacen("0 g", "1 g", 1);
    let pdf =
        doubling_forms_page(13, &white_stripes, "/M gs 0 0 1 rg 0 0 200 200 re f", "/ExtGState << /M << /SMask << /S /Luminosity /G 5 0 R >> >> >>");
    let held = stripes_within_the_budget();
    let (drawn, cut, inked) = render_counted(pdf);
    assert!(cut, "the page says content was left out");
    // The mask's fills, and the page's own fill under it.
    assert!(drawn <= held + 2, "drawing the mask stops at its budget: {drawn} paths drawn, {held} fit, {BOMB_LEAVES} leaves");
    assert!(drawn + 2 >= held, "what the mask's budget holds is drawn: {drawn} paths drawn, {held} fit");
    assert!(inked > 0.4, "the fill shows through the stripes that fitted: {inked}");
}

/// Nested forms within the budget draw every leaf and say nothing was left out.
#[test]
fn nested_forms_within_the_budget_draw_every_leaf() {
    let (drawn, cut, inked) = render_counted(doubling_forms(6));
    assert!(!cut, "nothing is left out");
    assert_eq!(drawn, 64, "every leaf is drawn");
    assert!(inked > 0.1, "the circle shows: {inked}");
    let (drawn, cut, _) = render_counted(doubling_forms_then(8, &stripes(), ""));
    assert!(!cut, "256 canvas-wide fills fit");
    assert_eq!(drawn, 256, "every leaf is drawn");
}

/// The coverage budget is the floor for a small canvas and grows with a large one.
#[test]
fn the_coverage_budget_grows_with_the_canvas() {
    assert_eq!(hayro::coverage_budget(1, 1), 1 << 26);
    assert_eq!(hayro::coverage_budget(2048, 2048), 1 << 26, "16 values a pixel is the floor here");
    assert_eq!(hayro::coverage_budget(4096, 4096), 16 * 4096 * 4096);
    assert!(hayro::coverage_budget(u16::MAX, u16::MAX) > 1 << 31, "vello_cpu keeps it to 2^31");
}

/// A device with room for `room` paths, counting those it is handed.
struct Room {
    room: usize,
    paths: usize,
}

impl<'a> hayro::hayro_interpret::Device<'a> for Room {
    fn set_soft_mask(&mut self, _: Option<hayro::hayro_interpret::SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: hayro::hayro_interpret::BlendMode) {}
    fn draw_path(&mut self, _: &kurbo::BezPath, _: kurbo::Affine, _: &hayro::hayro_interpret::Paint<'a>, _: &hayro::hayro_interpret::PathDrawMode) {
        self.paths += 1;
    }
    fn push_clip_path(&mut self, _: &hayro::hayro_interpret::ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<hayro::hayro_interpret::SoftMask<'a>>, _: hayro::hayro_interpret::BlendMode) {}
    fn draw_glyph(
        &mut self,
        _: &hayro::hayro_interpret::font::Glyph<'a>,
        _: kurbo::Affine,
        _: kurbo::Affine,
        _: &hayro::hayro_interpret::Paint<'a>,
        _: &hayro::hayro_interpret::GlyphDrawMode,
    ) {
    }
    fn draw_image(&mut self, _: hayro::hayro_interpret::Image<'a, '_>, _: kurbo::Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
    fn out_of_room(&self) -> bool {
        self.paths >= self.room
    }
}

/// The interpreter stops drawing once its device has no room left (hayro-interpret patch 112),
/// as it does when cancelled, and says content was left out: 2^12 leaves, room for 100.
#[test]
fn the_interpreter_stops_when_its_device_has_no_room() {
    use hayro::hayro_interpret::{Context, InterpreterCache, interpret_page};
    let pdf = Pdf::new(Arc::new(doubling_forms(12))).unwrap();
    let page = &pdf.pages()[0];
    let truncated = Arc::new(Mutex::new(false));
    let seen = truncated.clone();
    let settings = InterpreterSettings {
        warning_sink: Arc::new(move |w| {
            if matches!(w, InterpreterWarning::ContentTruncated) {
                *seen.lock().unwrap() = true;
            }
        }),
        ..Default::default()
    };
    let cache = InterpreterCache::new();
    let mut ctx = Context::new(kurbo::Affine::IDENTITY, kurbo::Rect::new(0.0, 0.0, 200.0, 200.0), &cache, page.xref(), settings);
    let mut device = Room { room: 100, paths: 0 };
    interpret_page(page, &mut ctx, &mut device);
    assert_eq!(device.paths, 100, "nothing is drawn past the device's room");
    assert!(*truncated.lock().unwrap(), "the page says content was left out");

    let mut ctx =
        Context::new(kurbo::Affine::IDENTITY, kurbo::Rect::new(0.0, 0.0, 200.0, 200.0), &cache, page.xref(), InterpreterSettings::default());
    let mut device = Room { room: usize::MAX, paths: 0 };
    interpret_page(page, &mut ctx, &mut device);
    assert_eq!(device.paths, 1 << 12, "with room, every leaf is drawn");
}

/// A device with no room from the start stops before reading the page and says content was left
/// out: the page's first window holds no operator yet, so only the check after it saw the device
/// full, and it stopped without marking the page truncated.
#[test]
fn a_device_with_no_room_from_the_start_says_the_page_was_left_out() {
    use hayro::hayro_interpret::{Context, InterpreterCache, interpret_page};
    let pdf = Pdf::new(Arc::new(doubling_forms(2))).unwrap();
    let page = &pdf.pages()[0];
    let truncated = Arc::new(Mutex::new(false));
    let seen = truncated.clone();
    let settings = InterpreterSettings {
        warning_sink: Arc::new(move |w| {
            if matches!(w, InterpreterWarning::ContentTruncated) {
                *seen.lock().unwrap() = true;
            }
        }),
        ..Default::default()
    };
    let cache = InterpreterCache::new();
    let mut ctx = Context::new(kurbo::Affine::IDENTITY, kurbo::Rect::new(0.0, 0.0, 200.0, 200.0), &cache, page.xref(), settings);
    let mut device = Room { room: 0, paths: 0 };
    interpret_page(page, &mut ctx, &mut device);
    assert_eq!(device.paths, 0, "nothing is drawn");
    assert!(*truncated.lock().unwrap(), "the page says content was left out");
}

/// A page filled with a 200 x 200 tiling pattern whose cell paints `fills - 1` black fills of
/// [`stripes`] and then fills the whole cell red (the rasterizer asks the same room of every
/// path, a canvas' worth): its pixels and whether it said content was left out.
fn stripes_cell(fills: usize) -> (Vec<u8>, bool) {
    let black = stripes();
    let mut cell: Vec<&str> = vec![black.as_str(); fills.saturating_sub(1)];
    cell.push("1 0 0 rg 0 0 200 200 re f");
    let cell = cell.join("\n");
    let content = "/Pattern cs /P0 scn 0 0 200 200 re f";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /Pattern << /P0 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()),
        format!(
            "<< /Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 200 200] /XStep 200 /YStep 200 /Resources << >> /Length {} >>\nstream\n{cell}\nendstream",
            cell.len()
        ),
    ];
    render_kept_to(pdf_of(&objs), None)
}

/// A tiling cell whose last fill is the first its coverage budget leaves out says the page was
/// left out. Every other fill past the budget is caught by the check before the next operator;
/// after the last one there was none, so the cell was drawn without it and nothing was said.
/// The cell's fills are counted to the one the budget first refuses, by its colour: the red
/// last fill covers the cell until the cell has one fill too many.
#[test]
fn a_tiling_cell_whose_last_fill_is_left_out_says_so() {
    let held = stripes_within_the_budget() as usize;
    let mut first_refused = None;
    for fills in held.saturating_sub(4)..held + 4 {
        let (pixels, cut) = stripes_cell(fills);
        if red(&pixels) == 0 {
            first_refused = Some((fills, cut));
            break;
        }
        assert!(!cut, "{fills} fills fit and nothing is said to be left out");
    }
    let (fills, cut) = first_refused.expect("the budget refuses a fill near the estimate");
    assert!(fills > held.saturating_sub(4), "the cell with the fewest fills, {fills}, draws its last one");
    assert!(cut, "the cell's last fill, the {fills}th, is left out and the page says so");
}

#[test]
fn shared_raster_options_enforces_max_coverage() {
    let pdf = doubling_forms(10);
    let parsed = Pdf::new(Arc::new(pdf)).unwrap();
    let render_with = |max_coverage: Option<usize>| {
        let truncated = Arc::new(Mutex::new(false));
        let seen = truncated.clone();
        let settings = InterpreterSettings {
            warning_sink: Arc::new(move |w| {
                if matches!(w, InterpreterWarning::ContentTruncated) {
                    *seen.lock().unwrap() = true;
                }
            }),
            ..Default::default()
        };
        let options = RasterOptions { max_coverage, ..Default::default() };
        let [w, h] = shared_raster::dimensions(&parsed.pages()[0], &options).unwrap();
        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        shared_raster::render_into(&parsed.pages()[0], &RenderCache::new(), &settings, &options, &mut buf).unwrap();
        let cut = *truncated.lock().unwrap();
        (buf, cut)
    };

    let (whole, whole_cut) = render_with(None);
    assert!(!whole_cut, "unlimited render leaves nothing out");
    let (kept, kept_cut) = render_with(Some(200_000));
    assert!(kept_cut, "shared RasterOptions::max_coverage truncates and warns");
    assert_eq!(kept.len(), whole.len());
    let blue = |px: &[u8]| px.as_chunks::<4>().0.iter().filter(|p| p[2] > 200 && p[0] < 50).count();
    assert!(blue(&kept) > 10_000, "circles drawn before the limit stay: {}", blue(&kept));
}
