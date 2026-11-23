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

<<<<<<< HEAD
/// Full screen's presenter for one frame of `ctx`, at time `now`, over the pages `shown` on
/// screen: whether the frame asked to be drawn again at once.
fn presenter_frame(ctx: &egui::Context, v: &mut DocView, info: &DocInfo, shown: &[PlacedPage], now: f64) -> bool {
    presenter_frame_animating(ctx, v, info, shown, now, true)
=======
/// Ctrl+wheel zooms a visible window that another application has focus over, as plain
/// wheel scrolling does; the palette still blocks it.
#[test]
fn ctrl_wheel_zooms_an_unfocused_window() {
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(300.0, 400.0, 1).unwrap();
        app.open_bytes("blank.pdf", None, bytes.as_ref().clone()).unwrap();
        app
    });
    h.run_steps(3);
    let center = h.state().views[0].viewport_rect().center();
    h.hover_at(center);
    h.input_mut().focused = false;
    h.run_steps(2);
    let ctrl_wheel = egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Line,
        delta: vec2(0.0, 1.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::COMMAND,
        source: egui::MouseWheelSource::Unknown,
    };
    h.state_mut().palette_open = true;
    let before = h.state().views[0].zoom;
    h.event(ctrl_wheel.clone());
    h.run_steps(10);
    assert_eq!(h.state().views[0].zoom, before, "the palette still blocks zoom");
    h.state_mut().palette_open = false;
    h.run_steps(2);
    h.event(ctrl_wheel);
    h.run_steps(10);
    assert!(h.state().views[0].zoom > before, "Ctrl+wheel zooms without window focus");
}

#[test]
fn virtualized_organizer_keeps_insert_gaps_at_document_positions() {
    use egui_kittest::kittest::Queryable;

    let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(300.0, 400.0, 100).unwrap();
        app.open_bytes("organize-gaps.pdf", None, bytes.as_ref().clone()).unwrap();
        app.views[0].organize = true;
        app
    });
    h.run_steps(4);
    assert!(h.state().views[0].thumb_demand.len() < 50);
    assert_eq!(h.query_all_by_label("Insert a file at the end").count(), 0, "the last virtualized cell is not the final page");
    let center = h.state().views[0].viewport_rect().center();
    h.hover_at(center);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: vec2(0.0, -100_000.0),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
        source: egui::MouseWheelSource::Unknown,
    });
    // Well past egui's longest scroll animation (about 0.3 s).
    h.run_steps(40);
    h.get_by_label("Insert a file at the end");
    let before_last = h.get_by_label("Insert a file before page 100").rect().center();
    h.hover_at(before_last);
    h.run_steps(2);
    assert_eq!(h.state().views[0].grid_gap, Some(99), "external drops use the document index after virtualized scrolling");
    assert!(h.state().views[0].thumb_demand.len() < 50);
}

#[test]
fn real_shell_pages_organizer_and_print_only_collect_current_thumbnail_consumers() {
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(612.0, 792.0, 10_000).unwrap();
        app.open_bytes("synthetic-ten-thousand.pdf", None, bytes.as_ref().clone()).unwrap();
        app.right = Some(RightPanel::Pages);
        app
    });
    h.run_steps(4);
    assert!((1..20).contains(&h.state().views[0].thumb_demand.len()));
    h.state_mut().right = None;
    h.state_mut().views[0].organize = true;
    h.run_steps(2);
    assert!((1..50).contains(&h.state().views[0].thumb_demand.len()));
    h.state_mut().views[0].organize = false;
    h.state_mut().dialog = Some(crate::Dialog::Print);
    h.state_mut().print_draft.sheet = 9999;
    h.run_steps(2);
    assert_eq!(h.state().views[0].thumb_demand.keys().copied().collect::<Vec<_>>(), vec![9999]);
    h.state_mut().print_draft.handling = crate::PrintHandling::Booklet;
    h.state_mut().print_draft.sheet = 0;
    h.run_steps(2);
    assert_eq!(h.state().views[0].thumb_demand.len(), 2, "booklet only needs its two placed pages");
    h.state_mut().dialog = None;
    h.run_steps(2);
    assert!(h.state().views[0].thumb_demand.is_empty());
    assert!(h.state().views[0].thumbs.is_empty());
}

/// One mouse-wheel notch (a line) at `at` seconds.
fn notch(v: &mut DocView, dy: f32, at: f64) -> bool {
    v.single_page_wheel(egui::MouseWheelUnit::Line, dy, egui::TouchPhase::Move, at, true)
}

#[test]
fn thumbnails_cover_the_panel_on_a_high_dpi_screen() {
    // Pages panel slot is at most 150 logical points. At 2 px/pt a letter page's thumbnail
    // must be at least that wide, so the panel samples it down instead of stretching it.
    let info = thumbnail_info(1);
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    v.need_thumbnail(0, true);
    let queue = v.prepare_render_queue(&info, 2.0);
    let scale = queue.first().expect("one thumbnail").scale;
    assert!(612.0 * scale >= 150.0 * 2.0, "{scale}");
}

#[test]
fn notches_turn_pages_and_stop_at_the_ends() {
    let mut v = view(3, PageLayout::Single, false);
    assert!(notch(&mut v, -1.0, 1.0) && notch(&mut v, -1.0, 1.05));
    assert_eq!(v.current, 2);
    assert!(!notch(&mut v, -1.0, 1.1), "nothing turns past the last page");
    assert!(notch(&mut v, 1.0, 1.2));
    assert_eq!(v.current, 1);
}

#[test]
fn notches_turn_two_page_spreads() {
    // Two-page view, shown one spread at a time (not continuous): a notch turns a whole
    // spread at once, not a single page (#752).
    let mut v = view(4, PageLayout::TwoUp, false);
    assert!(notch(&mut v, -1.0, 1.0));
    assert_eq!(v.current, 2, "a notch turns from the first spread to the second");
    assert!(notch(&mut v, 1.0, 1.1));
    assert_eq!(v.current, 0, "a notch back turns to the previous spread");
}

#[test]
fn continuous_views_ignore_the_wheel_for_paging() {
    // Scrolling views let the ScrollArea handle the wheel; it never turns pages.
    for layout in [PageLayout::Single, PageLayout::TwoUp] {
        let mut v = view(3, layout, true);
        assert!(!notch(&mut v, -1.0, 1.0), "{layout:?} continuous ignores the wheel");
        assert_eq!(v.current, 0);
    }
}

#[test]
fn navigation_drops_partial_wheel_motion() {
    use egui::{MouseWheelUnit::Point, TouchPhase::Move, TouchPhase::Start};
    let mut v = view(3, PageLayout::Single, false);
    assert!(!v.single_page_wheel(Point, 0.0, Start, 1.0, true));
    assert!(!v.single_page_wheel(Point, -30.0, Move, 1.02, true));
    v.go_to_page(1);
    // The same touch has to move a full notch's worth again before it turns.
    assert!(!v.single_page_wheel(Point, -30.0, Move, 1.04, true));
    assert!(v.single_page_wheel(Point, -30.0, Move, 1.06, true));
    assert_eq!(v.current, 2);
}

#[test]
fn reselecting_the_layout_does_not_scroll_to_top() {
    let mut v = view(3, PageLayout::Single, true);
    v.goto = None;
    v.set_layout(PageLayout::Single);
    assert!(v.goto.is_none(), "re-selecting the current layout is a no-op");
    v.set_continuous(true);
    assert!(v.goto.is_none(), "re-selecting the current continuity is a no-op");
    v.set_layout(PageLayout::TwoUp);
    assert_eq!(v.goto, Some((0, 0.0)), "switching the base layout aligns the current page");
    v.goto = None;
    v.set_continuous(false);
    assert_eq!(v.goto, Some((0, 0.0)), "toggling continuous scrolling aligns the current page");
}

#[test]
fn fit_page_and_height_hold_still_while_mixed_page_sizes_scroll_past() {
    let page =
        |width: f32, height: f32| pdfcraft_render::PageInfo { width, height, label: String::new(), crop: [0.0, 0.0, width, height], rotation: 0 };
    let info = DocInfo { pages: vec![page(300.0, 400.0), page(600.0, 800.0)], ..Default::default() };
    for fit in [Fit::Page, Fit::Height] {
        let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
        (v.fit, v.viewport_w, v.viewport_h) = (fit, 1000.0, 800.0);
        v.fit_zoom(&info);
        let zoom = v.zoom;
        v.current = 1;
        v.fit_zoom(&info);
        assert_eq!(v.zoom, zoom, "{fit:?}: scrolling onto a bigger page keeps the zoom");
        // A single page shown on its own (not scrolling) fits each one.
        (v.layout, v.continuous) = (PageLayout::Single, false);
        v.fit_zoom(&info);
        let big = v.zoom;
        v.current = 0;
        v.fit_zoom(&info);
        assert!(v.zoom > big, "{fit:?}: single-page view fits the page it shows");
    }
}

/// Review on #2070: full screen in two-page view fits the spread it shows, the cover page
/// on its own included; two-page view in the window keeps one zoom for the document.
#[test]
fn full_screen_two_page_view_fits_the_spread_it_shows() {
    let page =
        |width: f32, height: f32| pdfcraft_render::PageInfo { width, height, label: String::new(), crop: [0.0, 0.0, width, height], rotation: 0 };
    // Spreads [0, 1] small, [2, 3] large; with a cover page [0], [1, 2] mixed, [3].
    let info = DocInfo { pages: vec![page(300.0, 400.0), page(300.0, 400.0), page(600.0, 800.0), page(600.0, 800.0)], ..Default::default() };
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    (v.layout, v.continuous, v.viewport_w, v.viewport_h) = (PageLayout::TwoUp, false, 1000.0, 800.0);
    v.fit = Fit::Page;
    v.fit_zoom(&info);
    let windowed = v.zoom;
    v.enter_presentation();
    v.fit_zoom(&info);
    let small = v.zoom;
    assert!(small > windowed * 1.9, "the small spread fills the screen: {small} vs {windowed}");
    v.current = 3;
    v.fit_zoom(&info);
    // The large spread fits the screen edge to edge, without the window's margins (#576).
    let edge_to_edge = (v.viewport_w - v.page_gap()) / (2.0 * 600.0 * PT);
    assert!((v.zoom - edge_to_edge).abs() < 1e-4 && v.zoom > windowed, "the large spread fills the screen: {} vs {edge_to_edge}", v.zoom);
    v.current = 1;
    v.fit_zoom(&info);
    assert_eq!(v.zoom, small, "page 2 is in the first spread");
    // With a cover page: [0] alone, then [1, 2], then [3].
    v.cover = true;
    v.current = 0;
    v.fit_zoom(&info);
    assert_eq!(v.zoom, small, "the cover page alone");
    // The mixed spread [1, 2] fits its two pages' own widths side by side, 300 + 600 points,
    // and not twice the wider one: here the height decides.
    v.current = 1;
    v.fit_zoom(&info);
    let mixed_fit = ((v.viewport_w - v.page_gap()) / (900.0 * PT)).min(v.viewport_h / (800.0 * PT));
    assert!((v.zoom - mixed_fit).abs() < 1e-4 && v.zoom > edge_to_edge, "page 2 shares a spread with the large page 3: {} vs {mixed_fit}", v.zoom);
    // And the content is as wide as that spread, so the spread is centred on the screen: room for
    // twice the larger page would centre it past the screen's right edge.
    let content_w = v.content_width(&info, v.viewport_w);
    let rects = v.layout(&info, content_w, 1.0);
    let (left, right) = (rects[1].min.x, rects[2].max.x);
    assert!(content_w <= v.viewport_w + 0.5, "the spread needs no scrolling: {content_w}");
    assert!(left >= -0.5 && right <= v.viewport_w + 0.5, "the spread is on the screen: {left}..{right}");
    assert!((left - (v.viewport_w - right)).abs() < 1.0, "and centred: {left}..{right}");
    // The cover alone keeps its spread's blank slot, and is on the screen too.
    v.current = 0;
    v.fit_zoom(&info);
    let content_w = v.content_width(&info, v.viewport_w);
    let cover = v.layout(&info, content_w, 1.0)[0];
    assert!(cover.min.x >= v.viewport_w / 2.0 && cover.max.x <= v.viewport_w + 0.5, "the cover sits on the right: {cover:?}");
    // Scrolling while in full screen fits the document, as scrolling does in the window.
    (v.continuous, v.current) = (true, 0);
    v.fit_zoom(&info);
    assert!((v.zoom - edge_to_edge).abs() < 1e-4, "continuous full screen keeps one zoom");
    v.continuous = false;
    // The cover page is a spread of its own even when the next page is larger.
    let mixed = DocInfo { pages: vec![page(300.0, 400.0), page(600.0, 800.0), page(600.0, 800.0)], ..Default::default() };
    v.current = 0;
    v.fit_zoom(&mixed);
    assert_eq!(v.zoom, small, "the small cover alone, not with page 2");
    // Leaving full screen brings the document-wide fit back.
    v.leave_presentation();
    (v.fit, v.current) = (Fit::Page, 0);
    (v.layout, v.continuous) = (PageLayout::TwoUp, false);
    v.fit_zoom(&info);
    assert!((v.zoom - windowed).abs() < 1e-4, "in the window, one zoom for the document");
    // With Fill off, full screen keeps the window's display (#4273): two-page view keeps one zoom
    // for the document, and its width, as the spreads turn.
    let content_w = v.content_width(&info, v.viewport_w);
    v.full_screen_fill = false;
    v.enter_presentation();
    for current in [0, 1, 3] {
        v.current = current;
        v.fit_zoom(&info);
        assert!((v.zoom - windowed).abs() < 1e-4, "fill off, page {current}: the window's zoom, {} vs {windowed}", v.zoom);
        assert_eq!(v.content_width(&info, v.viewport_w), content_w, "fill off, page {current}: the window's width");
    }
}

#[test]
fn fit_width_fits_the_shown_page_in_single_page_view_only() {
    let page =
        |width: f32, height: f32| pdfcraft_render::PageInfo { width, height, label: String::new(), crop: [0.0, 0.0, width, height], rotation: 0 };
    let info = DocInfo { pages: vec![page(300.0, 400.0), page(600.0, 400.0)], ..Default::default() };
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    (v.fit, v.viewport_w, v.viewport_h) = (Fit::Width, 1000.0, 800.0);
    v.layout = PageLayout::Single;
    v.continuous = false;
    v.fit_zoom(&info);
    let narrow = v.zoom;
    // The shown page fills the width between the side gutters, not centred for the wide page.
    let shown = v.layout(&info, v.content_width(&info, 1000.0), 1.0)[0];
    assert!((shown.left() - SIDE).abs() < 1e-2 && (shown.width() - 860.0).abs() < 1e-2, "{shown:?}");
    v.current = 1;
    v.fit_zoom(&info);
    assert!((narrow / v.zoom - 2.0).abs() < 1e-3, "the narrow page gets twice the zoom of the wide one");
    // Scrolling views keep one zoom for the whole document.
    v.continuous = true;
    v.fit_zoom(&info);
    let wide = v.zoom;
    v.current = 0;
    v.fit_zoom(&info);
    assert_eq!(v.zoom, wide, "continuous fit width holds still across page sizes");
}

#[test]
fn layouts_match_their_commands_and_names() {
    for l in PageLayout::ORDER {
        let spec = pdfcraft_engine::commands::command(l.command()).expect("registered");
        assert_eq!((l.label(), l.icon()), (spec.label, spec.icon), "{l:?}");
        assert_eq!(PageLayout::from_command(l.command()), Some(l));
        assert_eq!(PageLayout::try_parse(l.as_str()), Some(l));
    }
    assert_eq!(PageLayout::try_parse(" Two-Up "), Some(PageLayout::TwoUp));
    assert_eq!(PageLayout::try_parse("facing"), None);
}

#[test]
fn a_null_xyz_coordinate_keeps_the_readers_place_on_the_new_page() {
    // CAD exports link each cross-reference as `/XYZ null null 0`. Acrobat keeps the
    // point at the window's top-left (and the zoom) when following one to another sheet.
    let info = pdfcraft_render::DocInfo {
        pages: (0..3)
            .map(|_| pdfcraft_render::PageInfo { width: 842.0, height: 595.0, label: String::new(), crop: [0.0, 0.0, 842.0, 595.0], rotation: 0 })
            .collect(),
        ..Default::default()
    };
    // Page 1 at 400 %, scrolled so its point 200 pt across and 125 pt down is at the corner
    // where /XYZ puts its point.
    let zoomed = |rot: u16, size: Vec2| {
        let mut v = view(3, PageLayout::Single, true);
        v.rotation = rot;
        v.viewport_screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 600.0));
        v.screen_xforms = vec![(0, PageXform { rect: Rect::from_min_size(pos2(SIDE - 800.0, -500.0), size), rot, pw: 842.0, ph: 595.0 })];
        v
    };
    let near = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 1e-4);
    let mut v = zoomed(0, vec2(4.0 * 842.0, 4.0 * 595.0));
    v.go_to_dest(1, DestView::Xyz { left: None, top: None, zoom: None }, &info);
    let (page, fx, fy, rel) = v.goto_point.expect("the place is kept");
    assert_eq!((v.current, page, rel), (1, 1, vec2(SIDE, 0.0)));
    assert!(near(fx, 200.0 / 842.0) && near(fy, 125.0 / 595.0), "{fx:?} {fy:?}");
    // A coordinate the destination gives wins; only the null one is kept.
    let mut v = zoomed(0, vec2(4.0 * 842.0, 4.0 * 595.0));
    v.go_to_dest(2, DestView::Xyz { left: Some(421.0), top: None, zoom: Some(4.0) }, &info);
    let (_, fx, fy, _) = v.goto_point.expect("the place is kept");
    assert!(near(fx, 0.5) && near(fy, 125.0 / 595.0), "{fx:?} {fy:?}");
    // With the view turned a quarter, the point under the corner is still the one kept.
    let mut v = zoomed(90, vec2(4.0 * 595.0, 4.0 * 842.0));
    v.go_to_dest(1, DestView::Xyz { left: None, top: None, zoom: None }, &info);
    let (_, fx, fy, _) = v.goto_point.expect("the place is kept");
    assert!(near(fx, 800.0 / (4.0 * 595.0)) && near(fy, 500.0 / (4.0 * 842.0)), "{fx:?} {fy:?}");
    // A page that fits the window keeps the layout's placement: no point, as before.
    let mut v = zoomed(0, vec2(800.0, 565.0));
    v.go_to_dest(1, DestView::Xyz { left: None, top: None, zoom: None }, &info);
    assert_eq!((v.current, v.goto, v.goto_point), (1, Some((1, 0.0)), None));
}

#[test]
fn destinations_set_the_zoom_mode_and_the_point_to_show() {
    let info = pdfcraft_render::DocInfo {
        pages: (0..3)
            .map(|_| pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 })
            .collect(),
        ..Default::default()
    };
    let mut v = view(3, PageLayout::Single, true);
    v.fit = Fit::Width;
    // No position (or a malformed one): the top of the page, zoom as it was.
    v.go_to_dest(1, DestView::Top, &info);
    assert_eq!((v.current, v.goto, v.goto_point, v.fit), (1, Some((1, 0.0)), None, Fit::Width));
    // /XYZ with a null zoom keeps the zoom mode; its point is a quarter down page 3.
    v.go_to_dest(2, DestView::Xyz { left: None, top: Some(300.0), zoom: None }, &info);
    assert_eq!((v.current, v.fit, v.goto_point), (2, Fit::Width, Some((2, None, Some(0.25), vec2(SIDE, 0.0)))));
    v.go_to_dest(0, DestView::Xyz { left: Some(150.0), top: None, zoom: Some(2.5) }, &info);
    assert_eq!((v.fit, v.zoom, v.goto_point), (Fit::None, 2.5, Some((0, Some(0.5), None, vec2(SIDE, 0.0)))));
    // A zoom beyond the view's range is clamped to it.
    v.go_to_dest(0, DestView::Xyz { left: None, top: None, zoom: Some(1e30) }, &info);
    assert_eq!((v.zoom, v.goto_point), (64.0, None));
    v.go_to_dest(0, DestView::Fit, &info);
    assert_eq!((v.fit, v.goto_point), (Fit::Page, None));
    v.go_to_dest(0, DestView::FitH { top: Some(100.0) }, &info);
    assert_eq!((v.fit, v.goto_point), (Fit::Width, Some((0, None, Some(0.75), vec2(SIDE, 0.0)))));
    v.go_to_dest(0, DestView::FitV { left: Some(75.0) }, &info);
    assert_eq!((v.fit, v.goto_point), (Fit::Height, Some((0, Some(0.25), None, vec2(SIDE, 0.0)))));
    // /FitR: the zoom that fits a 100×50 pt rectangle in an 800×600 window, centred.
    (v.viewport_w, v.viewport_h) = (800.0, 600.0);
    v.go_to_dest(0, DestView::FitR { rect: [0.0, 300.0, 100.0, 350.0] }, &info);
    assert_eq!(v.fit, Fit::None);
    assert!((v.zoom - 8.0 / PT).abs() < 1e-4, "{}", v.zoom);
    let (page, fx, fy, rel) = v.goto_point.expect("a point to show");
    assert_eq!((page, rel), (0, vec2(400.0, 300.0)));
    assert!((fx.unwrap_or(-1.0) - 50.0 / 300.0).abs() < 1e-4 && (fy.unwrap_or(-1.0) - 75.0 / 400.0).abs() < 1e-4, "{fx:?} {fy:?}");
    // A page past the end goes to the last page; a page the document info lacks, to its top.
    v.go_to_dest(usize::MAX, DestView::FitH { top: Some(100.0) }, &info);
    assert_eq!((v.current, v.goto_point.map(|g| g.0)), (2, Some(2)));
    let empty = pdfcraft_render::DocInfo::default();
    v.go_to_dest(1, DestView::FitH { top: Some(100.0) }, &empty);
    assert_eq!((v.current, v.goto_point), (1, None));
}

#[test]
fn page_text_failure_preserves_valid_search_match_without_jitter() {
    let mut v = view(3, PageLayout::Single, true);
    v.current = 1;
    v.find =
        Some(Find { query: "test".into(), case_query: "test".into(), matches: vec![(0, 0..4), (1, 5..9)], current: Some(0), ..Default::default() });
    let req = RenderRequest { page: 0, kind: RequestKind::Text, ..Default::default() };
    v.last_queue = vec![req];
    let ctx = egui::Context::default();
    v.receive_result(
        &ctx,
        pdfcraft_render::RenderedPage {
            request: req,
            width: 0,
            height: 0,
            rgba: Default::default(),
            error: Some("extraction failed".into()),
            text: None,
            warnings: Vec::new(),
            millis: 0,
        },
    );
    // Active match was on page 0 which failed: current should fall back to match on page 1 without jitter
    assert_eq!(v.find.as_ref().unwrap().current, Some(0));
    assert_eq!(v.find.as_ref().unwrap().matches, vec![(1, 5..9)]);
    assert!(v.goto.is_none());
}

/// A page whose picture moved can get its whole-page raster back before its last stale tile.
/// From then on the raster has the picture where it went: it is drawn from there, not from
/// where it was (which the raster now shows empty), and another drag starts from there too.
#[test]
fn a_moved_picture_is_drawn_from_the_fresh_raster_while_a_tile_is_still_stale() {
    let content = b"q 60 0 0 60 40 260 cm /Im0 Do Q";
    let objs: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 400] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>".to_vec(),
        [format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(), content.as_slice(), b"\nendstream"].concat(),
        [
            b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceRGB /Length 3 >>\nstream\n".as_slice(),
            &[200, 30, 30],
            b"\nendstream",
        ]
        .concat(),
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(o);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        bytes.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 1100.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("picture.pdf", None, bytes.clone()).unwrap();
        app.set_option("zoom", "100").unwrap();
        app.quick_tool = QuickTool::EditText;
        app
    });
    let settle = |h: &mut egui_kittest::Harness<'static, PdfCraftApp>| {
        for _ in 0..400 {
            h.step();
            if !h.state().render_pending() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    };
    h.run_steps(4);
    settle(&mut h);
    let page = h.state().views[0].page_screen_rect(0).unwrap();
    let at = |x: f32, y: f32| egui::pos2(page.left() + x / 300.0 * page.width(), page.top() + (400.0 - y) / 400.0 * page.height());
    let red = |pixels: &image::RgbaImage, r: Rect| {
        let (w, hh) = (pixels.width(), pixels.height());
        (r.top().max(0.0) as u32..(r.bottom() as u32).min(hh))
            .flat_map(|y| (r.left().max(0.0) as u32..(r.right() as u32).min(w)).map(move |x| (x, y)))
            .filter(|(x, y)| matches!(pixels.get_pixel(*x, *y).0, [r, g, b, _] if r > 150 && g < 90 && b < 90))
            .count()
    };
    let (old, new) = (Rect::from_two_pos(at(42.0, 262.0), at(98.0, 318.0)), Rect::from_two_pos(at(182.0, 82.0), at(238.0, 138.0)));
    // Select the picture and drag it, without overlap, from (40, 260) to (180, 80).
    let drag = |h: &mut egui_kittest::Harness<'static, PdfCraftApp>, from: Pos2, to: Pos2, release: bool| {
        h.hover_at(from);
        h.run_steps(1);
        h.drag_at(from);
        h.run_steps(1);
        for k in 1..=4 {
            h.hover_at(from + (to - from) * (k as f32 / 4.0));
            h.run_steps(1);
        }
        if release {
            h.drop_at(to);
            h.step();
        }
    };
    let (grab, put) = (at(70.0, 290.0), at(210.0, 110.0));
    h.hover_at(grab);
    h.run_steps(1);
    h.drag_at(grab);
    h.run_steps(1);
    h.drop_at(grab);
    h.run_steps(3);
    assert!(h.state().views[0].image_selection.is_some(), "the picture is selected");
    h.state_mut().views[0].hold_rasters = true;
    drag(&mut h, grab, put, true);
    // One tile stays stale, as a tile result arriving after the whole-page raster would; then
    // the whole-page raster is let through.
    let tile = h.ctx.load_texture("stale-tile", egui::ColorImage::new([1, 1], vec![Color32::TRANSPARENT]), TextureOptions::LINEAR);
    h.state_mut().views[0].tiles.insert((0, 1, 0, 0), tile);
    h.state_mut().views[0].stale_tiles.insert((0, 1, 0, 0));
    h.state_mut().views[0].hold_rasters = false;
    for _ in 0..400 {
        h.step();
        if !h.state().views[0].page_raster_out_of_date(0) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!h.state().views[0].page_raster_out_of_date(0), "the whole-page raster is fresh");
    assert!(h.state().views[0].page_out_of_date(0), "a tile is still stale");
    h.run_steps(2);
    let pixels = h.render().unwrap();
    assert!(red(&pixels, new) > 100, "the picture shows where it went");
    assert_eq!(red(&pixels, old), 0, "and not where it was");
    // Another drag in that interval takes its pixels from where the raster has them.
    let further = at(210.0, 200.0);
    drag(&mut h, put, further, false);
    let pixels = h.render().unwrap();
    let moving = Rect::from_two_pos(at(182.0, 172.0), at(238.0, 228.0));
    assert!(red(&pixels, moving) > 100, "the dragged picture is drawn from the fresh raster");
    assert_eq!(red(&pixels, old), 0, "nothing shows where it first was");
    h.state_mut().views[0].stale_tiles.clear();
    h.state_mut().views[0].tiles.clear();
}

/// Review on #5325: one Esc closes one thing. With a form field being filled (a check box that
/// has the focus) and text selected, the page leaves the selection for the next Esc: the field's
/// editor takes this one.
#[test]
fn esc_with_a_field_being_filled_leaves_the_selection() {
    let mut v = view(3, PageLayout::Single, true);
    v.all_text = true;
    v.forms.focus = Some(crate::forms_ui::Focus {
        name: "agree".into(),
        obj: None,
        widget: 0,
        text: String::new(),
        picked: Vec::new(),
        request_focus: false,
        select_all: false,
        press: None,
        calendar: None,
        calendar_rect: None,
        calendar_text: None,
        list: Default::default(),
        had_focus: true,
    });
    let ctx = egui::Context::default();
    ctx.input_mut(|i| {
        i.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
    });
    shortcuts(&mut v, &ctx, Default::default(), true, Default::default());
    assert!(v.all_text, "the selection stays while the field takes Esc");
    v.forms.focus = None;
    shortcuts(&mut v, &ctx, Default::default(), true, Default::default());
    assert!(!v.all_text, "with no field being filled, Esc clears the selection");
}

/// Review on #4378: `FitWindow` measures the spread the view shows, its pages side by side
/// and as tall as the taller one, with one page in a spread that has only one.
#[test]
fn first_page_now_measures_the_pages_in_the_spread() {
    let page =
        |width: f32, height: f32| pdfcraft_render::PageInfo { width, height, label: String::new(), crop: [0.0, 0.0, width, height], rotation: 0 };
    let info = DocInfo { pages: vec![page(300.0, 400.0), page(600.0, 800.0), page(200.0, 500.0)], ..Default::default() };
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    let one = v.first_page_now(&info).expect("a page");
    assert_eq!((one.size, one.margins.x), (vec2(300.0 * PT, 400.0 * PT), 2.0 * SIDE));
    v.set_display(PageLayout::TwoUp, false);
    let gap = v.page_gap();
    // [0, 1]: 300 + 600 wide, 800 high, one gap between them.
    let spread = v.first_page_now(&info).expect("a spread");
    assert_eq!((spread.size, spread.margins.x), (vec2(900.0 * PT, 800.0 * PT), 2.0 * SIDE + gap));
    // [2] on its own at the end: one page, no gap.
    v.current = 2;
    let last = v.first_page_now(&info).expect("the last page");
    assert_eq!((last.size, last.margins.x), (vec2(200.0 * PT, 500.0 * PT), 2.0 * SIDE));
    // With a cover page: [0] alone, then [1, 2].
    v.set_cover(true);
    assert_eq!(v.first_page_now(&info).expect("a spread").size, vec2(800.0 * PT, 800.0 * PT));
    v.current = 0;
    let cover = v.first_page_now(&info).expect("the cover");
    assert_eq!((cover.size, cover.margins.x), (vec2(300.0 * PT, 400.0 * PT), 2.0 * SIDE));
    v.current = 9;
    assert!(v.first_page_now(&info).is_none());
}

/// Full screen's presenter for one frame of `ctx`, at time `now`: whether the frame asked to be
/// drawn again at once.
fn presenter_frame(ctx: &egui::Context, v: &mut DocView, info: &DocInfo, now: f64) -> bool {
    presenter_frame_animating(ctx, v, info, now, true)
>>>>>>> upstream/main
}

/// [`presenter_frame`] with the Animation effects preference as given (`false` also when the
/// system asks for no animations).
fn presenter_frame_animating(ctx: &egui::Context, v: &mut DocView, info: &DocInfo, shown: &[PlacedPage], now: f64, animates: bool) -> bool {
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        let painter = ui.ctx().layer_painter(egui::LayerId::background());
        let at = presenter::PresentAt { current: v.view.current, display: v.view.display, rotation: v.view.rotation };
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

<<<<<<< HEAD
=======
/// A transition of no length, a negative one or one that is not a number changes the page at once
/// (a negative one would otherwise never end).
#[test]
fn a_transition_of_zero_or_negative_length_cuts() {
    use pdfcraft_render::TransitionStyle;
    for duration in [0.0, -1.0, f32::NAN, f32::NEG_INFINITY] {
        let ctx = egui::Context::default();
        let (mut v, info) = presenting_view(3, vec![trans(1, Some(TransitionStyle::Wipe), duration, None)], &ctx);
        presenter_frame(&ctx, &mut v, &info, 0.0);
        turn_to(&ctx, &mut v, 1);
        presenter_frame(&ctx, &mut v, &info, 0.1);
        assert!(!v.presenting_a_transition(), "/D {duration}: cut");
        assert_eq!(v.current, 1);
    }
}

/// Full screen that keeps a continuous display (Fill screen off, #4273) scrolls from page to page:
/// a page's transition doesn't play over the scrolling pages.
#[test]
fn full_screen_that_keeps_scrolling_plays_no_transition() {
    use pdfcraft_render::TransitionStyle;
    let ctx = egui::Context::default();
    let page = pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 };
    let info = DocInfo { pages: vec![page; 3], transitions: vec![trans(1, Some(TransitionStyle::Wipe), 1.0, None)], ..Default::default() };
    let mut v = DocView::new(DocId(1), &info, ViewDefaults { layout: PageLayout::Single, continuous: true, ..Default::default() });
    v.full_screen_fill = false;
    v.enter_presentation();
    assert!(v.continuous(), "Fill screen off keeps continuous scrolling");
    give_raster(&ctx, &mut v, 0);
    presenter_frame(&ctx, &mut v, &info, 0.0);
    turn_to(&ctx, &mut v, 1);
    presenter_frame(&ctx, &mut v, &info, 0.1);
    assert!(!v.presenting_a_transition(), "no Wipe over a scrolling display");
}

/// A style the spec doesn't name is read as its default R, Replace (ISO 32000-2 Table 164: "The
/// new page simply replaces the old one with no special transition effect"): the page changes at
/// once, and its display time still counts.
#[test]
fn an_unknown_transition_style_cuts_and_keeps_the_display_time() {
    use pdfcraft_render::TransitionStyle;
    let ctx = egui::Context::default();
    let unknown = TransitionStyle::from_name("Spiral");
    let (mut v, info) = presenting_view(3, vec![trans(1, Some(unknown), 5.0, Some(1.0))], &ctx);
    presenter_frame(&ctx, &mut v, &info, 0.0);
    turn_to(&ctx, &mut v, 1);
    presenter_frame(&ctx, &mut v, &info, 0.1);
    assert!(!v.presenting_a_transition(), "no effect for an unknown style");
    presenter_frame(&ctx, &mut v, &info, 0.9);
    assert_eq!(v.current, 1, "its display time is not up");
    presenter_frame(&ctx, &mut v, &info, 1.2);
    assert_eq!(v.current, 2, "its display time is up");
}

>>>>>>> upstream/main
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
<<<<<<< HEAD
    // The text layers arrive over a few frames; the matches of the two pages with text follow.
    for _ in 0..200 {
        h.run_steps(2);
        if h.state().views[0].find.as_ref().is_some_and(|f| f.matches.len() == 2) {
=======
}

/// Cancelling the pages' pointer gestures (Escape, automation's cancel) lets go of a
/// camera corner being dragged: the quadrilateral stays, and no corner follows the pointer.
#[test]
fn cancelling_pointer_gestures_lets_go_of_a_camera_corner() {
    let mut v = view(1, PageLayout::Single, true);
    v.camera = Some(crate::camera_ui::CameraQuad { grab: Some(2), ..crate::camera_ui::CameraQuad::inset(0, 300.0, 400.0) });
    v.cancel_pointer_gestures();
    assert_eq!(v.camera.map(|q| q.grab), Some(None));
}

/// A camera corner at the page's edge is hit on the page only: a press off the page is no page's,
/// so its grab cursor does not show there.
#[test]
fn a_camera_corner_at_the_page_edge_is_hit_on_the_page_only() {
    let mut v = view(1, PageLayout::Single, true);
    v.camera = Some(crate::camera_ui::CameraQuad { page: 0, corners: [[0.0, 0.0], [300.0, 0.0], [300.0, 400.0], [0.0, 400.0]], grab: None });
    let xf = PageXform { rect: Rect::from_min_size(pos2(10.0, 20.0), egui::vec2(300.0, 400.0)), rot: 0, pw: 300.0, ph: 400.0 };
    assert!(crate::camera_ui::hit(&xf, 0, &v, pos2(14.0, 24.0)).is_some(), "on the page");
    assert!(crate::camera_ui::hit(&xf, 0, &v, pos2(6.0, 16.0)).is_none(), "just off it");
}

/// #5125: after Enhance scanned file, the page shown is re-rendered, not the raster of the
/// scan before it was cleaned (same scale, so nothing else would replace it).
#[test]
fn enhancing_a_scan_re_renders_the_page_shown() {
    let (w, h) = (64usize, 64usize);
    let px: Vec<u8> = (0..w * h).map(|i| if i % w < 4 { 30 } else { 198 }).collect();
    let mut pdf = format!(
        "%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>\nendobj\n4 0 obj\n<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 8 /Length {} >>\nstream\n",
        px.len()
    )
    .into_bytes();
    pdf.extend_from_slice(&px);
    let content = format!("q {w} 0 0 {h} 0 0 cm /Im0 Do Q");
    pdf.extend_from_slice(
        format!(
            "\nendstream\nendobj\n5 0 obj\n<< /Length {} >>\nstream\n{content}\nendstream\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n",
            content.len()
        )
        .as_bytes(),
    );
    let mut app = PdfCraftApp::new();
    app.open_bytes("scan.pdf", None, pdf).unwrap();
    let (i, doc) = app.active_ids().unwrap();
    let ctx = egui::Context::default();
    let tex = ctx.load_texture("before", egui::ColorImage::filled([1, 1], Color32::WHITE), TextureOptions::LINEAR);
    app.views[i].pages.insert(0, PageTex { tag: 1000, tex });
    let job = app.session.scan_clean_job(doc, &[], pdfcraft_engine::scan_clean::ScanCleanSettings::default()).unwrap();
    let cleaned = job.run(|_, _| true).unwrap();
    assert_eq!(cleaned.report.pages, vec![0], "the page is a scan and gets cleaned");
    let progress =
        std::sync::Arc::new(std::sync::Mutex::new(crate::scan_clean_ui::ScanCleanProgress { result: Some(Ok(cleaned)), ..Default::default() }));
    app.scan_clean_run = Some(crate::scan_clean_ui::ScanCleanRun { doc, progress });
    app.poll_scan_clean();
    assert!(app.scan_clean_run.is_none());
    assert_eq!(app.views[i].pages.get(&0).unwrap().tag, STALE_TAG, "the old raster is re-rendered");
}

/// #5708 review: a page that fits is centred with nothing to scroll, so only pixel snapping may
/// pass for fitting. A page half a pixel or more too tall scrolls, or its ends would be cut off.
#[test]
fn only_pixel_snapping_passes_for_fitting_the_view() {
    let page = |height: f32| [Rect::from_min_size(pos2(0.0, MARGIN), vec2(300.0, height))];
    for ppp in [1.0, 2.0] {
        let slack = 0.5 / ppp;
        let (_, content) = vertical_placement(false, &page(600.0 + slack), &[0], 600.0, MARGIN, ppp);
        assert_eq!(content, 600.0, "at {ppp}x, snapping's half pixel still fits");
        for over in [slack + 0.1, 1.0] {
            let (shift, content) = vertical_placement(false, &page(600.0 + over), &[0], 600.0, MARGIN, ppp);
            assert_eq!((shift, content), (0.0, 600.0 + over + 2.0 * MARGIN), "at {ppp}x, {over} pt too tall scrolls");
        }
    }
    // A scale that isn't one has no slack and never panics.
    for ppp in [0.0, f32::NAN, f32::INFINITY] {
        assert!(!fits_height(Rect::from_min_size(pos2(0.0, 0.0), vec2(1.0, 600.5)), 600.0, ppp));
    }
}

/// Review of #5017: a selection across pages copied before every page it covers is read is not
/// dropped. Copy stays on and reads those pages off the UI thread, taking only the part each
/// page has selected.
#[test]
fn a_selection_across_pages_still_to_be_read_is_copied_once_read() {
    let mut app = PdfCraftApp::new();
    app.open_bytes("three.pdf", None, text_pages_pdf(&["Alpha one", "Bravo two", "Charlie three"])).unwrap();
    // From the second glyph of page 1 to the first of page 3, no page read yet.
    app.views[0].selection = Some(Selection { page: 0, span: Span::Run { anchor: 1, head: 0, head_page: 2, head_x: None } });
    let span = app.views[0].selection_waits_for_text().expect("waits for its pages");
    assert_eq!(span, TextSpan { start: (0, 1), end: (2, 0) });
    assert!(app.views[0].selected_text().is_none(), "nothing to copy on this thread yet");
    assert!(pdfcraft_engine::commands::command("edit.copy").is_some_and(|c| app.command_enabled(c)), "Copy stays on");
    // The job reads only the selection's part of each page, and no page outside it.
    let doc = app.session.get(app.views[0].id).unwrap();
    let text = crate::DocumentCopyJob::of_selection(&app.views[0], doc, span).text(&std::sync::atomic::AtomicBool::new(false));
    assert_eq!(text.as_deref(), Some("lpha one\nBravo two\nC"));
    let later = crate::DocumentCopyJob::of_selection(&app.views[0], doc, TextSpan { start: (1, 0), end: (1, 4) });
    assert!(matches!(later.pages[0], crate::CopyPage::Skip) && matches!(later.pages[2], crate::CopyPage::Skip));
    assert_eq!(later.text(&std::sync::atomic::AtomicBool::new(false)).as_deref(), Some("Bravo"));
    // Copy starts that job, and the clipboard gets the text when it is done.
    let ctx = egui::Context::default();
    assert!(app.copy_selected_text(0, &ctx), "the copy starts");
    let mut copied = None;
    for _ in 0..500 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.finish_document_copy(ui.ctx()));
        out.textures_delta.clear();
        copied = out.platform_output.commands.iter().find_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        });
        if copied.is_some() {
>>>>>>> upstream/main
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
