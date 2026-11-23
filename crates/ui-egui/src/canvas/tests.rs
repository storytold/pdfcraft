use super::area::{MARK_SIDE_SHARE, mark_anchor, verdict_mark_rect};
use super::cache::grayscale;
use super::*;
use craft::pages::ui::{PageBody, PlacedPage};
use craft::ui::primitives::textures::{LoadFailure, Revision, TextureState, TextureStore, TextureStoreOptions};

#[test]
fn typed_zoom_percentages_parse() {
    for (typed, zoom) in [("150", 1.5), ("150%", 1.5), (" 62.5 % ", 0.625), ("6400%", 64.0)] {
        assert_eq!(DocView::parse_zoom_percent(typed), Some(zoom), "{typed}");
    }
    for typed in ["", "%", "abc", "0", "-50", "NaN", "inf", "1e40", "150%%x", "%150"] {
        assert_eq!(DocView::parse_zoom_percent(typed), None, "{typed}");
    }
}

fn signature(signed: bool, visible: bool, page: Option<usize>, rect: Option<[f64; 4]>) -> pdfcraft_engine::SignatureInfo {
    pdfcraft_engine::SignatureInfo {
        field: "Signature1".into(),
        signed,
        page,
        rect,
        visible,
        signer: None,
        certificate: None,
        chain: Vec::new(),
        date: None,
        signing_time: None,
        reason: None,
        location: None,
        contact: None,
        sub_filter: None,
        doc_timestamp: false,
        certify: None,
        revision: 1,
        signed_len: 0,
        digest: None,
        algorithm: None,
        timestamp: false,
        timestamp_time: None,
        status: pdfcraft_engine::SignatureStatus::Valid,
        modification: pdfcraft_engine::sign::Modification::None,
        verdict_shown: false,
        details: Vec::new(),
    }
}

#[test]
fn signature_states_change_with_a_verdict_or_what_draws_it() {
    let shown = signature(true, true, Some(0), Some([10.0, 10.0, 100.0, 50.0]));
    let before = vec![shown.clone()];
    assert!(!signature_states_changed(&before, &before), "identical lists report no change");
    let mut invalid = shown.clone();
    invalid.status = pdfcraft_engine::SignatureStatus::Invalid;
    let mut drawn = shown.clone();
    drawn.verdict_shown = true;
    let mut other = shown.clone();
    other.field = "Signature2".into();
    for (after, why) in [(vec![invalid], "verdict"), (vec![drawn], "drawn"), (vec![other], "field"), (Vec::new(), "count")] {
        assert!(signature_states_changed(&before, &after), "{why}");
    }
}

#[test]
fn signature_badges_keep_signed_visible_widgets_on_the_page() {
    let rect = [10.0, 10.0, 100.0, 50.0];
    let signatures =
        vec![signature(true, true, Some(0), Some(rect)), signature(true, true, Some(1), Some(rect)), signature(true, true, Some(0), Some(rect))];
    let badges = signature_badges(&signatures, 0);
    assert_eq!(badges.len(), 2);
    assert_eq!(badges[0].rect, rect);
}

#[test]
fn signature_badges_skip_unsigned_invisible_and_unplaced_fields() {
    let rect = [10.0, 10.0, 100.0, 50.0];
    let signatures = vec![
        signature(false, true, Some(0), Some(rect)),
        signature(true, false, Some(0), Some(rect)),
        signature(true, true, Some(0), None),
        signature(true, true, Some(0), Some(rect)),
    ];
    let badges = signature_badges(&signatures, 0);
    assert_eq!(badges.len(), 1);
    assert_eq!(badges[0].rect, rect);
}

#[test]
fn signature_badges_skip_rectangles_that_cannot_reach_the_screen() {
    let hostile = vec![
        [f64::NAN, 10.0, 100.0, 50.0],
        [10.0, f64::INFINITY, 100.0, 50.0],
        [10.0, 10.0, f64::NEG_INFINITY, 50.0],
        [10.0, 10.0, 1e300, 50.0],
        [10.0, 10.0, 100.0, 50.0],
    ];
    let signatures: Vec<_> = hostile.into_iter().map(|rect| signature(true, true, Some(0), Some(rect))).collect();
    let badges = signature_badges(&signatures, 0);
    assert_eq!(badges.len(), 1);
}

#[test]
fn signature_badges_keep_every_verdict_the_appearance_shows() {
    // Whether the redrawn layers are visible (not clipped or painted over) is not known, so a
    // valid verdict keeps its badge as well.
    use pdfcraft_engine::SignatureStatus::{Invalid, Unknown, Valid};
    for status in [Valid, Unknown, Invalid] {
        let mut shown = signature(true, true, Some(0), Some([10.0, 10.0, 100.0, 50.0]));
        (shown.verdict_shown, shown.status) = (true, status);
        assert_eq!(signature_badges(&[shown.clone()], 0).len(), 1, "{status:?}");
        shown.verdict_shown = false;
        assert_eq!(signature_badges(&[shown], 0).len(), 1, "{status:?}");
    }
}

#[test]
fn the_verdict_mark_takes_the_corner_nearest_its_anchor() {
    let widget = Rect::from_min_max(pos2(40.0, 60.0), pos2(540.0, 760.0));
    let centre = widget.center();
    for anchor in [widget.left_top(), widget.right_top(), widget.right_bottom(), widget.left_bottom()] {
        let mark = verdict_mark_rect(widget, Rect::EVERYTHING, anchor).expect("a widget this size carries a mark");
        assert!(widget.contains_rect(mark), "{mark:?} in {widget:?}");
        assert!(!mark.contains(centre), "the mark covers the artwork it annotates: {mark:?}");
        let (right, below) = (mark.center().x > centre.x, mark.center().y > centre.y);
        assert_eq!((right, below), (anchor.x > centre.x, anchor.y > centre.y), "{mark:?} for {anchor:?}");
    }
    // Without a usable anchor it sits bottom-right.
    let mark = verdict_mark_rect(widget, Rect::EVERYTHING, pos2(f32::NAN, f32::NAN)).unwrap();
    assert!(mark.min.x > centre.x && mark.min.y > centre.y, "{mark:?}");
}

#[test]
fn the_verdict_mark_turns_with_the_page_and_the_view() {
    // A named-layer caption runs across the widget's top on the page; the mark takes the
    // opposite corner however the page's /Rotate and the view turn the two.
    let rect = [100.0, 100.0, 300.0, 220.0];
    for page_turn in [0_u16, 90, 180, 270] {
        for view_turn in [0_u16, 90, 180, 270] {
            let page =
                pdfcraft_render::PageInfo { width: 612.0, height: 792.0, label: "1".into(), crop: [0.0, 0.0, 612.0, 792.0], rotation: page_turn };
            let info = DocInfo { pages: vec![page], ..Default::default() };
            let xf = PageXform { rect: Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 800.0)), rot: view_turn, pw: 612.0, ph: 792.0 };
            let widget = xf.user_rect(&info, 0, rect.map(|v: f64| v as f32));
            let mark = verdict_mark_rect(widget, xf.rect, mark_anchor(&xf, &info, 0, rect)).expect("a widget this size carries a mark");
            let (right, below) = (mark.center().x > widget.center().x, mark.center().y > widget.center().y);
            let expected = match (page_turn + view_turn) % 360 {
                0 => (true, true),
                90 => (false, true),
                180 => (false, false),
                _ => (true, false),
            };
            assert_eq!((right, below), expected, "page /Rotate {page_turn}, view {view_turn}");
        }
    }
}

#[test]
fn the_verdict_mark_stays_inside_its_widget_at_every_finite_size() {
    // 50 and 56 are where a mark always taking the 48pt cap would no longer fit.
    let sizes = [0.5, 3.0, 6.0, 12.0, 20.0, 32.0, 40.0, 50.0, 56.0, 90.0, 200.0, 500.0, 5_000.0];
    for &w in &sizes {
        for &h in &sizes {
            let widget = Rect::from_min_max(pos2(10.0, 10.0), pos2(10.0 + w, 10.0 + h));
            for anchor in [widget.left_top(), widget.right_bottom()] {
                let Some(mark) = verdict_mark_rect(widget, Rect::EVERYTHING, anchor) else { continue };
                assert!(widget.contains_rect(mark), "{mark:?} is not inside {widget:?} for a {w}x{h} widget");
            }
            let Some(mark) = verdict_mark_rect(widget, Rect::EVERYTHING, widget.right_bottom()) else { continue };
            // A quarter of the shorter side each way: at most 6.25% of a square widget.
            let share = mark.width() * mark.height() / (widget.width() * widget.height());
            assert!(share <= MARK_SIDE_SHARE * MARK_SIDE_SHARE * 1.0001, "the mark covers {:.2}% of a {w}x{h} widget", share * 100.0);
        }
    }
}

#[test]
fn the_verdict_mark_stays_on_the_page_when_its_widget_crosses_the_edge() {
    let page = Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 300.0));
    let widget = Rect::from_min_max(pos2(200.0, 150.0), pos2(600.0, 450.0));
    let mark = verdict_mark_rect(widget, page, widget.right_bottom()).expect("the part on the page holds a mark");
    assert!(page.contains_rect(mark), "{mark:?} is off {page:?}");
    assert!(mark.max.x > 350.0 && mark.max.y > 250.0, "the corner of the part on the page nearest the anchor: {mark:?}");
    // Wholly off the page, or a sliver on it too thin for a legible mark: none.
    let off = widget.translate(vec2(400.0, 0.0));
    assert_eq!(verdict_mark_rect(off, page, off.right_bottom()), None);
    let sliver = Rect::from_min_max(pos2(390.0, 0.0), pos2(600.0, 300.0));
    assert_eq!(verdict_mark_rect(sliver, page, sliver.right_bottom()), None);
}

#[test]
fn snapshot_tabs_read_like_their_source_plus_the_suffix() {
    let snap = Snapshot { kind: SnapshotKind::SignedVersion, revision: 1, from: "report.pdf".into() };
    assert_eq!(snapshot_tab_name("Report", "report.pdf", &snap), "Report (signed version)");
    let rev = Snapshot { kind: SnapshotKind::Revision, revision: 2, from: "report.pdf".into() };
    assert_eq!(snapshot_tab_name("Report", "report.pdf", &rev), "Report (revision 2)");
    assert_eq!(snapshot_tab_name("report.pdf", "report.pdf", &snap), "report.pdf");
    assert_eq!(snapshot_tab_name("report (revision 2).pdf", "report (revision 2).pdf", &rev), "report (revision 2).pdf");
}

#[test]
fn grayscale_converts_colours_to_their_luminance() {
    let img = egui::ColorImage::new([2, 1], vec![Color32::from_rgb(255, 0, 0), Color32::from_rgba_premultiplied(0, 51, 102, 128)]);
    let gray = grayscale(img);
    let opaque = gray.pixels[0];
    assert_eq!(opaque.to_array(), [76, 76, 76, 255]);
    // Unmultiply RGB first, so a translucent colour keeps its paper-relative luminance.
    let translucent = gray.pixels[1];
    assert_eq!(translucent.to_array(), [42, 42, 42, 128]);
}

#[test]
fn black_white_and_transparents_are_preserved_by_grayscale() {
    let img = egui::ColorImage::new([3, 1], vec![Color32::BLACK, Color32::WHITE, Color32::TRANSPARENT]);
    let gray = grayscale(img);
    assert_eq!(gray.pixels[0].to_array(), [0, 0, 0, 255]);
    assert_eq!(gray.pixels[1].to_array(), [255, 255, 255, 255]);
    assert_eq!(gray.pixels[2].to_array(), [0, 0, 0, 0]);
}

#[test]
fn markup_on_several_pages_is_one_undo_step_named_like_one() {
    let mark = |page| {
        Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
            page,
            shape: pdfcraft_engine::Shape::TextMarkup { kind: pdfcraft_engine::Markup::Highlight, quads: vec![[0.0; 8]] },
            style: Default::default(),
            contents: String::new(),
            author: String::new(),
        })
    };
    assert!(selection::one_undo_step(Vec::new()).is_none());
    assert!(matches!(selection::one_undo_step(vec![mark(0)]), Some(Edit::AddAnnotation(_))));
    let Some(Edit::Batch { label, edits }) = selection::one_undo_step(vec![mark(0), mark(1)]) else { panic!("a batch") };
    assert_eq!((label.as_str(), edits.len()), (mark(0).label().wording, 2));
    assert!(pdfcraft_engine::EDIT_LABELS.contains(&label.as_str()), "a wording the catalogues translate");
}

/// An edit while page input is blocked (the window losing focus, a dialog or the palette open):
/// the page's paragraphs are still measured for the latest edit and drawn, instead of vanishing
/// until the pointer comes back.
#[test]
fn edit_text_boxes_stay_drawn_while_page_input_is_blocked() {
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
        {
            let mut app = PdfCraftApp::for_unit_tests();
            let bytes = app.session.create_blank(300.0, 400.0, 1).unwrap();
            app.open_bytes(&egui::Context::default(), "blank.pdf", None, bytes.as_ref().clone()).unwrap();
            app.quick_tool = QuickTool::EditText;
            app
        }
    });
    h.run_steps(3);
    // Block page input the way the window losing focus does.
    h.state_mut().palette_open = true;
    h.run_steps(2);
    let ctx_h = h.ctx.clone();
    assert!(h.state_mut().apply_edit(&ctx_h, Edit::RotatePages { pages: vec![0], degrees: 90 }));
    h.run_steps(3);
    let app = h.state();
    let generation = app.session.get(app.views[0].id).unwrap().edit_generation();
    assert_eq!(app.views[0].session.edit_lines.get(&0).map(|(g, _)| *g), Some(generation), "the boxes follow the edit while input is blocked");
}

/// A cut page (#623) is named in a bar of its own: a static XFA form's notice, which comes
/// first among the form notices, still shows beside it instead of replacing it (review on #2113).
#[test]
fn a_cut_page_is_named_beside_an_xfa_form_notice() {
    use egui_kittest::kittest::Queryable;

    let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
        {
            let mut app = PdfCraftApp::for_unit_tests();
            app.set_option(&cc.egui_ctx, "language", "en").unwrap();
            let bytes = pdfcraft_xfa::fixtures::static_shell("<form1><page1><name>Ada</name></page1></form1>");
            app.open_bytes(&egui::Context::default(), "form.pdf", None, bytes).unwrap();
            app
        }
    });
    h.run_steps(4);
    h.get_by_label_contains("XFA data are kept in step");
    h.state_mut().views[0].reports.cut(0);
    h.run_steps(2);
    h.get_by_label_contains("holds more content");
    h.get_by_label_contains("XFA data are kept in step");
}

/// Dismissing another notice doesn't hide a page found cut later (#623), and dismissing the
/// cut-page notice lasts until another page is found cut (review on #2113).
#[test]
fn a_page_found_cut_after_a_dismissal_is_still_named() {
    use egui_kittest::kittest::Queryable;

    let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
        {
            let mut app = PdfCraftApp::for_unit_tests();
            app.set_option(&cc.egui_ctx, "language", "en").unwrap();
            let bytes = app.session.create_blank(300.0, 400.0, 2).unwrap();
            app.open_bytes(&egui::Context::default(), "blank.pdf", None, bytes.as_ref().clone()).unwrap();
            app
        }
    });
    h.run_steps(3);
    // The reader dismissed the document's notice before the render found the cut page.
    h.state_mut().views[0].notice_dismissed = true;
    h.state_mut().views[0].reports.cut(0);
    h.run_steps(2);
    h.get_by_label_contains("holds more content");
    h.get_by_label("Dismiss").click();
    h.run_steps(2);
    assert!(h.query_by_label_contains("holds more content").is_none(), "the cut-page notice can be dismissed");
    // Re-reading the same page (an undo, a layer toggle) doesn't bring it back.
    h.state_mut().views[0].invalidate_content();
    h.state_mut().views[0].reports.cut(0);
    h.run_steps(2);
    assert!(h.query_by_label_contains("holds more content").is_none(), "the dismissed page stays dismissed");
    h.state_mut().views[0].reports.cut(1);
    h.run_steps(2);
    h.get_by_label_contains("pages hold more content");
    assert_eq!(h.state().views[0].cut_pages(), vec![0, 1], "both pages are reported cut");
}

/// Full screen's presenter for one frame of `ctx`, at time `now`, over the pages `shown` on
/// screen: whether the frame asked to be drawn again at once.
fn presenter_frame(ctx: &egui::Context, v: &mut DocView, info: &DocInfo, shown: &[PlacedPage], now: f64) -> bool {
    presenter_frame_animating(ctx, v, info, shown, now, true)
}

/// [`presenter_frame`] with the Animation effects preference as given (`false` also when the
/// system asks for no animations).
fn presenter_frame_animating(ctx: &egui::Context, v: &mut DocView, info: &DocInfo, shown: &[PlacedPage], now: f64, animates: bool) -> bool {
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        let painter = ui.ctx().layer_painter(egui::LayerId::background());
        let at = presenter::PresentAt { current: v.view.current, display: v.view.display, continuous: v.view.continuous, rotation: v.view.rotation };
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(900.0, 400.0));
        let asked = presenter::frame(&mut v.session.presenter, presenter::Shown { pages: shown, rect }, at, info, &painter, now, animates, None);
        // The page view takes the turn on its next pass: one row on, a spread in two-page view.
        if let Some(forward) = asked.step {
            v.view.step(forward);
            let row = if v.view.display == PageDisplay::TwoUp { 2 } else { 1 };
            let target = if forward { v.view.current.checked_add(row) } else { v.view.current.checked_sub(row) };
            v.view.turn = None;
            if let Some(next) = target.filter(|n| *n < info.pages.len()) {
                v.view.current = next;
                ui.ctx().request_repaint();
            }
        }
    });
    // No renderer here to upload textures: drop the frame's texture changes.
    out.textures_delta.clear();
    out.viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|o| o.repaint_delay.is_zero())
}

/// The store the placed pages' pictures come from, one per context.
fn pictures(ctx: &egui::Context) -> TextureStore<usize> {
    TextureStore::in_context(ctx, "canvas-tests", TextureStoreOptions::pages())
}

/// A rendered page picture, a blank one, as the store reports it.
fn ready(ctx: &egui::Context) -> TextureState {
    let store = pictures(ctx);
    store.put(ctx, 0, Revision::new(1), egui::ColorImage::new([4, 4], vec![Color32::WHITE; 16]));
    store.state(ctx, &0, Revision::new(1))
}

/// A picture the renderer could not make, as the store reports it.
fn failed(ctx: &egui::Context) -> TextureState {
    let store = pictures(ctx);
    store.put_failure(ctx, 1, Revision::new(1), LoadFailure::Unreadable);
    store.state(ctx, &1, Revision::new(1))
}

/// Page `page` on screen at 300 × 400 with `picture`, `column` places wide from the left.
fn placed(page: usize, column: usize, picture: TextureState) -> PlacedPage {
    let rect = Rect::from_min_size(pos2(300.0 * column as f32, 0.0), vec2(300.0, 400.0));
    PlacedPage::new(page, rect, 1.0, (0.0, 400.0), picture)
}

fn page_info() -> pdfcraft_render::PageInfo {
    pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 }
}

/// `pages` 300×400 pt pages with `transitions`, in full screen showing `display`, one page or
/// spread at a time.
fn presenting_view(pages: usize, display: PageDisplay, transitions: Vec<pdfcraft_render::PageTransition>) -> (DocView, DocInfo) {
    let info = DocInfo { pages: vec![page_info(); pages], transitions, ..Default::default() };
    // PLAN: assumed `DocView::new(id, info, defaults, highlight_fields)` and `ViewDefaults { display, .. }` (pdf-canvas-w8)
    let mut v = DocView::new(DocId(1), &info, ViewDefaults { display, continuous: false, ..Default::default() }, true);
    v.enter_presentation();
    (v, info)
}

/// Turn to `page` as a key would: the view's current page moves and the page shows with its
/// picture in.
fn turn_to(ctx: &egui::Context, v: &mut DocView, page: usize) -> Vec<PlacedPage> {
    v.view.current = page;
    vec![placed(page, 0, ready(ctx))]
}

/// A page's `/Trans` (style and `/D`) and `/Dur`, the rest as the spec's defaults.
fn trans(page: usize, style: Option<pdfcraft_render::TransitionStyle>, duration: f32, display: Option<f32>) -> pdfcraft_render::PageTransition {
    pdfcraft_render::PageTransition { page, style, duration, horizontal: true, inward: true, direction: Some(0), scale: 1.0, opaque: false, display }
}

/// Full screen that keeps a continuous display (Fill screen off, #4273) scrolls from page to page:
/// a page's transition doesn't play over the scrolling pages.
#[test]
fn full_screen_that_keeps_scrolling_plays_no_transition() {
    use pdfcraft_render::TransitionStyle;
    let ctx = egui::Context::default();
    let info = DocInfo { pages: vec![page_info(); 3], transitions: vec![trans(1, Some(TransitionStyle::Wipe), 1.0, None)], ..Default::default() };
    let mut v = DocView::new(DocId(1), &info, ViewDefaults { display: PageDisplay::Single, continuous: true, ..Default::default() }, true);
    v.full_screen_fill = false;
    v.enter_presentation();
    assert!(v.continuous(), "Fill screen off keeps continuous scrolling");
    presenter_frame(&ctx, &mut v, &info, &[placed(0, 0, ready(&ctx))], 0.0);
    let shown = turn_to(&ctx, &mut v, 1);
    presenter_frame(&ctx, &mut v, &info, &shown, 0.1);
    assert!(!v.presenting_a_transition(), "no Wipe over a scrolling display");
}

/// A transition into a page the renderer could not draw ends at once: the page before is not held
/// on screen waiting for it, and the page's own time can start.
#[test]
fn a_transition_into_a_page_that_failed_to_render_ends_at_once() {
    use pdfcraft_render::TransitionStyle;
    let ctx = egui::Context::default();
    let (mut v, info) = presenting_view(2, PageDisplay::Single, vec![trans(1, Some(TransitionStyle::Wipe), 1.0, None)]);
    presenter_frame(&ctx, &mut v, &info, &[placed(0, 0, ready(&ctx))], 0.0);
    v.view.current = 1;
    presenter_frame(&ctx, &mut v, &info, &[placed(1, 0, failed(&ctx))], 0.1);
    assert!(!v.presenting_a_transition(), "no wait for a page that won't render");
}

/// While the incoming page is still rendering, the presenter holds the page before without asking
/// for a frame after frame: the picture's arrival wakes the window.
#[test]
fn a_transition_waiting_for_its_page_does_not_redraw_every_frame() {
    use pdfcraft_render::TransitionStyle;
    let ctx = egui::Context::default();
    let (mut v, info) = presenting_view(2, PageDisplay::Single, vec![trans(1, Some(TransitionStyle::Wipe), 1.0, None)]);
    presenter_frame(&ctx, &mut v, &info, &[placed(0, 0, ready(&ctx))], 0.0);
    v.view.current = 1;
    let waiting = [placed(1, 0, TextureState::Missing)];
    // egui asks for its first frames itself: past them, only the presenter asks.
    for now in [0.1, 0.2, 0.3] {
        presenter_frame(&ctx, &mut v, &info, &waiting, now);
    }
    let again = presenter_frame(&ctx, &mut v, &info, &waiting, 0.4);
    assert!(v.presenting_a_transition(), "still waiting for page 2");
    assert!(!again, "without asking for a frame after frame");
}

/// A display time on the first page of the last two-page spread can't advance (there is no spread
/// after it): the presenter then stays on it without asking for a frame after frame.
#[test]
fn a_display_time_on_the_last_spread_does_not_redraw_every_frame() {
    let ctx = egui::Context::default();
    let (mut v, info) = presenting_view(4, PageDisplay::TwoUp, vec![trans(2, None, 1.0, Some(1.0))]);
    v.view.current = 2;
    let spread = [placed(2, 0, ready(&ctx)), placed(3, 1, ready(&ctx))];
    // egui asks for its first frames itself: past them, only the presenter asks.
    for now in [0.0, 0.5, 0.6] {
        presenter_frame(&ctx, &mut v, &info, &spread, now);
    }
    let again = presenter_frame(&ctx, &mut v, &info, &spread, 5.0);
    assert_eq!(v.view.current, 2, "the last spread stays");
    assert!(!again, "and asks for no frame after frame");
}

/// A page's display time (ISO 32000-2 §12.4.4.1, `/Dur`) counts from when the page is on screen:
/// a slow render that still shows Rendering… does not use it up.
#[test]
fn a_display_time_starts_once_the_page_has_rendered() {
    let ctx = egui::Context::default();
    let (mut v, info) = presenting_view(2, PageDisplay::Single, vec![trans(0, None, 1.0, Some(1.0))]);
    let pending = [placed(0, 0, TextureState::Missing)];
    for now in [0.0, 1.0, 2.0, 5.0] {
        presenter_frame(&ctx, &mut v, &info, &pending, now);
    }
    assert_eq!(v.view.current, 0, "page 1 is still rendering: its second has not started");
    let rendered = [placed(0, 0, ready(&ctx))];
    presenter_frame(&ctx, &mut v, &info, &rendered, 5.5);
    presenter_frame(&ctx, &mut v, &info, &rendered, 6.0);
    assert_eq!(v.view.current, 0, "half of its second on screen");
    presenter_frame(&ctx, &mut v, &info, &rendered, 6.6);
    assert_eq!(v.view.current, 1, "its second on screen is up");
}

/// In two-page view a display time counts from when the whole spread is on screen: one page of
/// it still showing Rendering… holds the spread (#5316 review).
#[test]
fn a_display_time_in_two_page_view_waits_for_the_whole_spread() {
    let ctx = egui::Context::default();
    let (mut v, info) = presenting_view(4, PageDisplay::TwoUp, vec![trans(0, None, 1.0, Some(1.0))]);
    let half = [placed(0, 0, ready(&ctx)), placed(1, 1, TextureState::Missing)];
    for now in [0.0, 1.0, 2.0, 5.0] {
        presenter_frame(&ctx, &mut v, &info, &half, now);
    }
    assert_eq!(v.view.current, 0, "page 2 of the spread is still rendering: the spread's second has not started");
    let whole = [placed(0, 0, ready(&ctx)), placed(1, 1, ready(&ctx))];
    presenter_frame(&ctx, &mut v, &info, &whole, 5.5);
    presenter_frame(&ctx, &mut v, &info, &whole, 6.0);
    assert_eq!(v.view.current, 0, "half of its second on screen");
    presenter_frame(&ctx, &mut v, &info, &whole, 6.6);
    assert_eq!(v.view.current, 2, "its second on screen is up: the next spread");
}

/// A page with a transition but no display time (ISO 32000-2 Table 31, `Dur`: "By default, the
/// viewer shall not advance automatically") stays until the user turns it, however long it shows.
#[test]
fn a_page_without_a_display_time_never_advances_by_itself() {
    use pdfcraft_render::TransitionStyle;
    let ctx = egui::Context::default();
    let (mut v, info) = presenting_view(3, PageDisplay::Single, vec![trans(0, Some(TransitionStyle::Wipe), 1.0, None)]);
    let shown = [placed(0, 0, ready(&ctx))];
    for now in [0.0, 1.0, 60.0, 86_400.0, 1.0e7] {
        presenter_frame(&ctx, &mut v, &info, &shown, now);
    }
    assert_eq!(v.view.current, 0, "no /Dur: the page waits for the presenter");
}

/// A display time of 0 seconds advances as soon as the page is on screen: zero is a number the
/// spec allows, the time a page "shall be displayed" at most.
#[test]
fn a_display_time_of_zero_advances_once_the_page_is_on_screen() {
    let ctx = egui::Context::default();
    let (mut v, info) = presenting_view(3, PageDisplay::Single, vec![trans(0, None, 1.0, Some(0.0))]);
    presenter_frame(&ctx, &mut v, &info, &[placed(0, 0, ready(&ctx))], 0.0);
    assert_eq!(v.view.current, 1, "page 1's zero seconds are up at once");
    presenter_frame(&ctx, &mut v, &info, &[placed(1, 0, ready(&ctx))], 0.1);
    assert_eq!(v.view.current, 1, "page 2 has no display time");
}

/// A display time longer than a day is cut to a day; the largest number the file can hold would
/// otherwise be past what a timer can wait for.
#[test]
fn a_display_time_longer_than_a_day_is_cut_to_a_day() {
    for display in [1.0e9, f32::MAX] {
        let ctx = egui::Context::default();
        let (mut v, info) = presenting_view(3, PageDisplay::Single, vec![trans(0, None, 1.0, Some(display))]);
        let shown = [placed(0, 0, ready(&ctx))];
        presenter_frame(&ctx, &mut v, &info, &shown, 0.0);
        presenter_frame(&ctx, &mut v, &info, &shown, 86_399.0);
        assert_eq!(v.view.current, 0, "/Dur {display}: not yet a day");
        presenter_frame(&ctx, &mut v, &info, &shown, 86_400.5);
        assert_eq!(v.view.current, 1, "/Dur {display}: a day is up");
    }
}

/// A transition the file says lasts a day (or longer) ends within ten seconds: it can't hold the
/// screen.
#[test]
fn a_transition_of_a_day_ends_within_ten_seconds() {
    use pdfcraft_render::TransitionStyle;
    for duration in [86_400.0, f32::MAX] {
        let ctx = egui::Context::default();
        let (mut v, info) = presenting_view(3, PageDisplay::Single, vec![trans(1, Some(TransitionStyle::Wipe), duration, None)]);
        presenter_frame(&ctx, &mut v, &info, &[placed(0, 0, ready(&ctx))], 0.0);
        let shown = turn_to(&ctx, &mut v, 1);
        presenter_frame(&ctx, &mut v, &info, &shown, 0.1);
        presenter_frame(&ctx, &mut v, &info, &shown, 5.0);
        assert!(v.presenting_a_transition(), "/D {duration}: playing");
        presenter_frame(&ctx, &mut v, &info, &shown, 10.2);
        assert!(!v.presenting_a_transition(), "/D {duration}: over after ten seconds");
    }
}

/// With animations off (the preference, or the system asking for none), a transition is a cut and
/// the page's display time still counts from when it is on screen.
#[test]
fn without_animations_a_transition_cuts_and_the_display_time_counts() {
    use pdfcraft_render::TransitionStyle;
    let ctx = egui::Context::default();
    let (mut v, info) = presenting_view(3, PageDisplay::Single, vec![trans(1, Some(TransitionStyle::Wipe), 5.0, Some(1.0))]);
    presenter_frame_animating(&ctx, &mut v, &info, &[placed(0, 0, ready(&ctx))], 0.0, false);
    let shown = turn_to(&ctx, &mut v, 1);
    presenter_frame_animating(&ctx, &mut v, &info, &shown, 0.1, false);
    assert!(!v.presenting_a_transition(), "cut");
    presenter_frame_animating(&ctx, &mut v, &info, &shown, 1.2, false);
    assert_eq!(v.view.current, 2, "page 2's second is up");
}

/// A page turned by hand while a display time counts starts its own time afresh (ISO 32000-2
/// §12.4.4.1, NOTE 1: "The user can advance the page manually before the specified time has
/// expired"); the time left on the page before is not carried over.
#[test]
fn a_page_turned_by_hand_starts_its_own_display_time() {
    let ctx = egui::Context::default();
    let timed = (0..4).map(|p| trans(p, None, 1.0, Some(2.0))).collect();
    let (mut v, info) = presenting_view(4, PageDisplay::Single, timed);
    presenter_frame(&ctx, &mut v, &info, &[placed(0, 0, ready(&ctx))], 0.0);
    presenter_frame(&ctx, &mut v, &info, &[placed(0, 0, ready(&ctx))], 1.5);
    let shown = turn_to(&ctx, &mut v, 1);
    presenter_frame(&ctx, &mut v, &info, &shown, 1.5);
    presenter_frame(&ctx, &mut v, &info, &shown, 3.0);
    assert_eq!(v.view.current, 1, "page 2 has shown 1.5 of its 2 seconds");
    presenter_frame(&ctx, &mut v, &info, &shown, 3.6);
    assert_eq!(v.view.current, 2, "page 2's own 2 seconds are up");
}

/// Three pages: two hold "fox", the third has no text at all.
fn fox_pages_pdf() -> Vec<u8> {
    let body = "BT /F1 14 Tf 20 150 Td (fox) Tj ET";
    let content = format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len());
    let page = |contents: &str| format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] {contents}/Resources << /Font << /F1 7 0 R >> >> >>");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_string(),
        page("/Contents 6 0 R "),
        page("/Contents 6 0 R "),
        page(""),
        content,
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
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
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

/// Replace all rewrites the matches of every page in one undo step, and a page whose text was
/// read but holds no match (the third) neither takes part in the step nor stops it.
#[test]
fn replace_all_is_one_undo_step_and_counts_pages_whose_matches_had_no_paragraph() {
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|cc| {
        crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
        {
            let mut app = PdfCraftApp::for_unit_tests();
            app.set_option(&cc.egui_ctx, "language", "en").unwrap();
            app.open_bytes(&egui::Context::default(), "fox.pdf", None, fox_pages_pdf()).unwrap();
            app
        }
    });
    h.run_steps(3);
    let id = h.state().views[0].id;
    let before = h.state().session.get(id).unwrap().bytes.clone();
    let depth = h.state().session.get(id).unwrap().undo_depth();
    // PLAN: assumed `DocView::open_replace` and `Find::{query, case_query, replacement}` keep their names (pdf-canvas-w14)
    {
        let view = &mut h.state_mut().views[0];
        view.open_replace();
        let find = view.find.as_mut().unwrap();
        find.query = "fox".into();
        find.case_query = "fox".into();
        find.replacement = "owl".into();
        // A query set here, not typed into the field, starts its search as the field's edit does.
        view.rerun_find();
    }
    // The text layers arrive over a few frames; the matches of the two pages with text follow.
    for _ in 0..200 {
        h.run_steps(2);
        if h.state().views[0].find.as_ref().is_some_and(|f| f.matches.len() == 2) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.state().views[0].find.as_ref().map(|f| f.matches.len()), Some(2), "one match on each of the two pages with text");
    h.state_mut().views[0].pending_action = Some(ViewAction::Replace { all: true });
    h.run_steps(3);
    let doc = h.state().session.get(id).unwrap();
    assert_eq!(doc.undo_depth(), depth + 1, "both pages are one undo step");
    assert_ne!(doc.bytes, before, "the text changed");
    h.state_mut().undo(&egui::Context::default());
    let doc = h.state().session.get(id).unwrap();
    assert_eq!((doc.undo_depth(), &doc.bytes), (depth, &before), "one undo puts both pages back");
}
