#[test]
fn zoom_anchor_scroll_stays_within_the_content() {
    let (content, viewport) = (vec2(1000.0, 2000.0), vec2(800.0, 600.0));
    // Past the end: clamped to the maximum (200, 1400), not left for egui to snap back.
    assert_eq!(anchored_scroll(pos2(900.0, 1900.0), vec2(0.0, 0.0), content, viewport), vec2(200.0, 1400.0));
    // Before the start.
    assert_eq!(anchored_scroll(pos2(10.0, 10.0), vec2(300.0, 300.0), content, viewport), Vec2::ZERO);
    // Inside: unchanged.
    assert_eq!(anchored_scroll(pos2(150.0, 800.0), vec2(100.0, 100.0), content, viewport), vec2(50.0, 700.0));
    // Content smaller than the viewport: no scrolling.
    assert_eq!(anchored_scroll(pos2(400.0, 400.0), Vec2::ZERO, vec2(500.0, 500.0), viewport), vec2(0.0, 0.0));
}

#[test]
fn page_size_hint_zone_is_the_bottom_left_quadrant() {
    let area = Rect::from_min_max(pos2(100.0, 50.0), pos2(500.0, 450.0));
    assert!(in_size_hint_zone(area, pos2(110.0, 440.0)));
    assert!(!in_size_hint_zone(area, pos2(110.0, 100.0)), "top-left");
    assert!(!in_size_hint_zone(area, pos2(400.0, 440.0)), "bottom-right");
    assert!(!in_size_hint_zone(area, pos2(50.0, 440.0)), "outside");
    assert!(!in_size_hint_zone(area, pos2(f32::NAN, 440.0)));
}

use super::*;

#[test]
fn typed_zoom_percentages_parse() {
    for (typed, zoom) in [("150", 1.5), ("150%", 1.5), (" 62.5 % ", 0.625), ("6400%", 64.0)] {
        assert_eq!(DocView::parse_zoom_percent(typed), Some(zoom), "{typed}");
    }
    for typed in ["", "%", "abc", "0", "-50", "NaN", "inf", "1e40", "150%%x", "%150"] {
        assert_eq!(DocView::parse_zoom_percent(typed), None, "{typed}");
    }
}

/// A rotated (landscape) page is drawn as wide as the cell allows and centred in the picture
/// area, as Acrobat draws it, not as a small strip pinned to the top; a portrait page still
/// fills the area's height; neither covers the checkbox corner or the label below.
#[test]
fn grid_pages_are_centred_and_as_large_as_the_cell_allows() {
    let cell = Rect::from_min_size(pos2(100.0, 50.0), vec2(190.0, 282.0));
    let area = Rect::from_min_max(pos2(cell.left(), cell.top() + 16.0), pos2(cell.right(), cell.bottom() - 72.0));
    let checkbox = Rect::from_min_size(cell.min + vec2(8.0, 8.0), Vec2::splat(20.0));
    let landscape = grid_page_rect(cell, vec2(842.0, 595.0));
    assert!((landscape.center().y - area.center().y).abs() < 0.01, "centred vertically: {landscape:?}");
    assert!((landscape.center().x - cell.center().x).abs() < 0.01);
    assert!((landscape.width() - (cell.width() - 32.0)).abs() < 0.01, "as wide as the cell allows: {landscape:?}");
    assert!(!landscape.intersects(checkbox));
    let portrait = grid_page_rect(cell, vec2(595.0, 842.0));
    assert!(area.contains_rect(portrait), "inside the picture area: {portrait:?}");
    assert!((portrait.center().y - area.center().y).abs() < 0.01);
    assert!(portrait.left() >= checkbox.right(), "clear of the checkbox: {portrait:?}");
    // Degenerate sizes still give a finite rectangle inside the cell.
    for page in [vec2(0.0, 0.0), vec2(1e9, 1.0), vec2(1.0, 1e9), vec2(f32::NAN, 10.0), vec2(f32::INFINITY, 10.0)] {
        let r = grid_page_rect(cell, page);
        assert!(r.is_finite() && cell.contains_rect(r), "{page:?} -> {r:?}");
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
        details: Vec::new(),
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
fn page_zoom_reads_the_side_the_page_width_is_on() {
    // A 200 x 100 pt page at 2 pixels per point: 400 x 200 on screen, or 200 x 400 turned.
    let xf = |rot: u16, size: Vec2| PageXform { rect: Rect::from_min_size(Pos2::ZERO, size), rot, pw: 200.0, ph: 100.0 };
    for rot in [0, 180] {
        assert_eq!(xf(rot, vec2(400.0, 200.0)).zoom(), 2.0, "{rot}");
    }
    for rot in [90, 270] {
        assert_eq!(xf(rot, vec2(200.0, 400.0)).zoom(), 2.0, "{rot}: the width runs down the screen");
    }
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
fn rendered_rasters_become_texture_images_without_a_copy() {
    let pdf = b"%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 100 50] /Contents 4 0 R >> endobj
4 0 obj << /Length 35 >> stream
0 0 1 rg 10 10 30 20 re f
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";
    let mut r = pdfcraft_render::PageRenderer::new(Arc::new(pdf.to_vec()), Default::default());
    let out = r.render(RenderRequest { page: 0, kind: RequestKind::Pixels, tile: None, scale: 1.5, tag: 0, preview: false });
    assert!(out.error.is_none(), "{:?}", out.error);
    let size = [out.width as usize, out.height as usize];
    let copied = egui::ColorImage::from_rgba_premultiplied(size, &out.rgba);
    let at = out.rgba.as_ptr();
    let img = texture_image(size, out.rgba);
    assert_eq!(img, copied);
    assert_eq!(img.pixels.as_ptr().cast::<u8>(), at, "the renderer's own buffer");
}

#[test]
fn tiles_stay_granular_up_close_and_cover_giant_sheets_in_a_handful() {
    // Ordinary zooms keep 1024 px tiles; degenerate scales fall back to it, never zero.
    assert_eq!(tile_side(1.0, 2000), TILE);
    assert_eq!(tile_side(4.0, 2000), TILE);
    assert_eq!(tile_side(f32::NAN, 2000), TILE);
    assert_eq!(tile_side(f32::INFINITY, 2000), TILE);
    assert_eq!(tile_side(0.0, 2000), TILE);
    // Zoomed viewports need a few large tiles instead of many small ones.
    assert_eq!(tile_side(12.0, 2000), 2048);
    assert_eq!(tile_side(20.0, 2000), 3413);
    assert_eq!(tile_side(100.0, 2000), 4096);
    // A giant sheet at fit width (6740 px long side) is covered four tiles a side.
    assert_eq!(tile_side(96.0 / 72.0, 6740), 1685);
    // The backend clamps tile sides at 4096.
    assert_eq!(tile_side(20.0, 100_000), 4096);
}

#[test]
fn grid_zoom_is_clamped_and_sharp_renders_tolerate_a_pinch() {
    assert!(sharp_enough(900, 876.0) && sharp_enough(800, 876.0) && sharp_enough(1300, 876.0));
    assert!(!sharp_enough(264, 876.0), "a thumbnail stretched over a zoomed page");
    assert!(!sharp_enough(876, 200.0), "far larger than drawn: wasteful");
    assert!(!sharp_enough(100, 0.0) && !sharp_enough(100, f32::NAN));
    let mut v = view(3, PageLayout::Single, true);
    for odd in [f32::NAN, f32::INFINITY] {
        v.set_grid_zoom(odd);
        assert_eq!(v.grid_zoom(), 1.0);
    }
    v.set_grid_zoom(99.0);
    assert_eq!(v.grid_zoom(), 3.0);
    v.set_grid_zoom(-4.0);
    assert_eq!(v.grid_zoom(), 0.5);
}

fn view(pages: usize, layout: PageLayout, continuous: bool) -> DocView {
    let info = pdfcraft_render::DocInfo {
        pages: (0..pages)
            .map(|_| pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 })
            .collect(),
        ..Default::default()
    };
    DocView::new(DocId(1), &info, ViewDefaults { layout, continuous, ..Default::default() })
}

fn history_search_view() -> DocView {
    let mut v = view(5, PageLayout::Single, true);
    v.texts.insert(
        3,
        pdfcraft_render::PageText {
            glyphs: vec![pdfcraft_render::TextGlyph {
                text: "match".into(),
                rect: [20.0, 260.0, 60.0, 280.0],
                quad: [[20.0, 260.0], [60.0, 260.0], [60.0, 280.0], [20.0, 280.0]],
                direction: [1.0, 0.0],
            }],
            line_of: vec![0],
            space_before: vec![false],
        }
        .into(),
    );
    v.find = Some(Find { matches: vec![(3, 0..1), (3, 0..1), (1, 0..1)], current: Some(2), ..Default::default() });
    v
}

#[test]
fn explicit_search_jumps_record_history_and_keep_match_destination() {
    for step in [None, Some(true), Some(false)] {
        let mut v = history_search_view();
        v.page_input = "draft in progress".into();
        v.goto_point = Some((3, Some(0.8), Some(0.9), Vec2::ZERO));
        if let Some(forward) = step {
            v.find_step(forward);
        } else {
            v.go_to_match(0);
        }
        assert_eq!(v.back, vec![0], "explicit search jump must remember its origin");
        assert_eq!(v.goto, Some((3, 0.5)), "keep the match offset instead of the page top");
        assert_eq!(v.current, 3);
        assert_eq!(v.current_page_label(None), "4");
        // Chrome refreshes the idle page box; navigation must not overwrite a draft
        // that may still be focused. DocView does not own the editor's focus state.
        assert_eq!(v.page_input, "draft in progress");
        assert!(v.goto_point.is_none(), "an earlier link destination cannot override the search offset");
        v.go_to_match(1);
        assert_eq!(v.back, vec![0], "same-page matches do not duplicate history");
        assert!(v.view_history(false));
        assert_eq!(v.current, 0);
        assert!(v.view_history(true));
        assert_eq!(v.current, 3);
        assert!(v.view_history(false));
        v.go_to_match(2);
        assert!(v.forward.is_empty(), "a new explicit jump discards Forward");
        assert_eq!(v.back, vec![0]);
    }
}

#[test]
fn automatic_search_updates_do_not_record_history_or_discard_forward() {
    let mut v = history_search_view();
    v.forward = vec![4];
    v.find = Some(Find { query: "match".into(), ..Default::default() });
    v.rerun_find();
    assert_eq!(v.current, 3);
    assert_eq!(v.goto, Some((3, 0.5)));
    assert!(v.back.is_empty());
    assert_eq!(v.forward, vec![4]);
    v.find.as_mut().unwrap().current = None;
    v.texts.insert(1, Arc::new(find_test_text("match")));
    v.refresh_find_page(1);
    assert_eq!(v.current, 1, "an automatic late match still follows its anchor");
    assert!(v.back.is_empty(), "automatic search updates do not record history");
    assert_eq!(v.forward, vec![4]);
}

/// One glyph per character on one line, for the search-order tests below.
fn find_test_text(s: &str) -> PageText {
    PageText {
        glyphs: s
            .chars()
            .map(|c| pdfcraft_render::TextGlyph { text: c.into(), rect: [0.0, 0.0, 10.0, 10.0], quad: [[0.0, 0.0]; 4], direction: [1.0, 0.0] })
            .collect(),
        line_of: s.chars().map(|_| 0).collect(),
        space_before: s.chars().map(|_| false).collect(),
    }
}

/// A text-layer result for `page`, for the search tests below.
fn find_text_result(page: usize, text: &str) -> pdfcraft_render::RenderedPage {
    pdfcraft_render::RenderedPage {
        request: RenderRequest { page, kind: RequestKind::Text, tag: TEXT_TAG, ..Default::default() },
        width: 0,
        height: 0,
        rgba: Default::default(),
        error: None,
        text: Some(Arc::new(find_test_text(text))),
        warnings: Vec::new(),
        millis: 0,
    }
}

/// Text layers arrive in whatever order the workers finish, not in page order. A search
/// started on page 1 must settle on page 1's match even when page 2's text arrives first,
/// instead of keeping the later match picked only because page 1 had not been read yet.
#[test]
fn automatic_search_match_follows_the_viewed_page_when_text_arrives_out_of_order() {
    let mut v = view(2, PageLayout::Single, true);
    v.current = 0;
    v.find = Some(Find { query: "sample".into(), ..Default::default() });
    v.rerun_find();
    assert_eq!(v.find.as_ref().unwrap().current, None, "no text yet, so no match");

    let ctx = egui::Context::default();
    let request = |page| RenderRequest { page, kind: RequestKind::Text, tag: TEXT_TAG, ..Default::default() };
    let result = |page, text: &str| pdfcraft_render::RenderedPage {
        request: request(page),
        width: 0,
        height: 0,
        rgba: Default::default(),
        error: None,
        text: Some(Arc::new(find_test_text(text))),
        warnings: Vec::new(),
        millis: 0,
    };
    v.last_queue = vec![request(1), request(0)];
    // Page 2's text finishes first, as it can when the workers race.
    v.receive_result(&ctx, result(1, "Second page sample text"));
    assert_eq!(v.find.as_ref().unwrap().current, Some(0), "the only known match is picked while page 1 is pending");

    v.receive_result(&ctx, result(0, "Panel sample draft notes"));
    let f = v.find.as_ref().unwrap();
    assert_eq!(f.matches.iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(f.current, Some(0), "page 1's match takes over once its text arrives");
    assert_eq!(v.current, 0, "the view follows the first match at or after the page being viewed");
}

/// A match the user picked (a click in the Search panel, or ⌘G) is not moved by a text
/// layer that arrives later: only automatic choices follow the viewed page.
#[test]
fn a_user_picked_search_match_stays_put_when_earlier_text_arrives() {
    let mut v = view(2, PageLayout::Single, true);
    v.current = 0;
    v.find = Some(Find { query: "sample".into(), ..Default::default() });
    v.rerun_find();
    v.texts.insert(1, Arc::new(find_test_text("Second page sample text")));
    v.refresh_find_page(1);
    v.go_to_match(0);
    assert_eq!(v.find.as_ref().unwrap().auto_from, None, "an explicit choice is not automatic");

    v.texts.insert(0, Arc::new(find_test_text("Panel sample draft notes")));
    v.refresh_find_page(0);
    let f = v.find.as_ref().unwrap();
    assert_eq!(f.matches.iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(f.current, Some(1), "the picked match keeps its identity after page 1 is inserted");
    assert_eq!(v.current, 1, "the user's page does not change under them");
}

/// The automatic choice may move the view only while the reader has left it where that
/// choice put it. Navigating by the page box/Pages panel/outline or by view history is the
/// reader's decision: a text layer arriving afterwards must not pull them back to an
/// anchor-derived match. (The harness covers same-page wheel and pointer scrolling.)
#[test]
fn a_late_automatic_match_does_not_override_the_readers_navigation() {
    enum Nav {
        Page,
        History,
    }
    for nav in [Nav::Page, Nav::History] {
        let mut v = view(3, PageLayout::Single, true);
        v.current = 0;
        v.texts.insert(2, Arc::new(find_test_text("Second page sample")));
        v.find = Some(Find { query: "sample".into(), ..Default::default() });
        v.rerun_find();
        assert_eq!((v.current, v.find.as_ref().unwrap().auto_page), (2, Some(2)), "the automatic match was shown");
        match nav {
            // The page box, the Pages panel and outline destinations all end here.
            Nav::Page => v.go_to_page(1),
            // View ▸ Page navigation ▸ Previous view writes `current` directly.
            Nav::History => {
                v.back = vec![1];
                assert!(v.view_history(false));
            }
        }
        assert_eq!(v.current, 1);
        v.texts.insert(0, Arc::new(find_test_text("First page sample")));
        v.refresh_find_page(0);
        let f = v.find.as_ref().unwrap();
        assert_eq!(f.auto_from, None, "the reader's navigation ends automatic following");
        assert_eq!(f.auto_page, None);
        assert_eq!(v.current, 1, "a late match cannot pull the reader back");
    }
}

/// Typing on while the results arrive must not re-anchor the search on a page the automatic
/// flash picked. The anchor stays the page the reader was on when they started searching.
#[test]
fn refining_the_query_keeps_the_page_the_search_started_from() {
    let mut v = view(3, PageLayout::Single, true);
    v.current = 0;
    v.texts.insert(2, Arc::new(find_test_text("Second page sample text")));
    v.find = Some(Find { query: "sample".into(), ..Default::default() });
    v.rerun_find();
    assert_eq!((v.current, v.find.as_ref().unwrap().auto_from), (2, Some(0)), "the first query's match scrolled the view");

    // Page 1 has a match for the refined query, and so has page 3. Anchored on the page the
    // automatic flash reached, the search would pick page 3; the starting page picks page 1.
    v.texts.insert(0, Arc::new(find_test_text("Panel draft text")));
    v.find.as_mut().unwrap().query = "text".into();
    v.rerun_find();
    let f = v.find.as_ref().unwrap();
    assert_eq!(f.auto_from, Some(0), "refinement keeps the page the reader searched from");
    assert_eq!(f.matches.first().map(|(p, _)| *p), Some(0));
    assert_eq!(f.current, Some(0));
    assert_eq!(v.current, 0);
}

/// ⌘G / Ctrl+G goes through `find_step`, an explicit choice like a Search-panel click: a
/// text layer arriving later must not undo it.
#[test]
fn keyboard_find_step_is_explicit_and_a_late_text_layer_cannot_undo_it() {
    let mut v = view(3, PageLayout::Single, true);
    v.current = 0;
    v.texts.insert(1, Arc::new(find_test_text("first sample")));
    v.texts.insert(2, Arc::new(find_test_text("second sample")));
    v.find = Some(Find { query: "sample".into(), ..Default::default() });
    v.rerun_find();
    assert_eq!((v.current, v.find.as_ref().unwrap().auto_from), (1, Some(0)));

    let ctx = egui::Context::default();
    ctx.input_mut(|i| {
        i.events.push(egui::Event::Key { key: egui::Key::G, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND });
    });
    shortcuts(&mut v, &ctx, Default::default());

    assert_eq!(v.current, 2, "the keyboard step chose the next match");
    assert_eq!(v.find.as_ref().unwrap().auto_from, None, "the keyboard step is the user's choice");
    assert_eq!(v.back, vec![1], "an explicit search jump records its origin");

    v.texts.insert(0, Arc::new(find_test_text("intro sample")));
    v.refresh_find_page(0);
    let f = v.find.as_ref().unwrap();
    assert_eq!(v.current, 2, "the chosen match keeps the view");
    assert_eq!(f.current.and_then(|i| f.matches.get(i)).map(|(p, _)| *p), Some(2));
}

/// A page whose text extraction failed keeps the automatic anchor: the failure selects the
/// next match without moving the view, and a later text layer can still follow the anchor.
#[test]
fn a_failed_text_layer_keeps_the_automatic_anchor_without_jitter() {
    let mut v = view(3, PageLayout::Single, true);
    v.current = 0;
    v.texts.insert(1, Arc::new(find_test_text("first sample")));
    v.texts.insert(2, Arc::new(find_test_text("second sample")));
    v.find = Some(Find { query: "sample".into(), ..Default::default() });
    v.rerun_find();
    assert_eq!((v.current, v.find.as_ref().unwrap().auto_page), (1, Some(1)));

    let goto = v.goto;
    let ctx = egui::Context::default();
    let request = RenderRequest { page: 1, kind: RequestKind::Text, ..Default::default() };
    v.last_queue = vec![request];
    v.receive_result(
        &ctx,
        pdfcraft_render::RenderedPage {
            request,
            width: 0,
            height: 0,
            rgba: Default::default(),
            error: Some("extraction failed".into()),
            text: None,
            warnings: Vec::new(),
            millis: 0,
        },
    );
    let f = v.find.as_ref().unwrap();
    assert_eq!(f.matches.iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec![2]);
    assert_eq!(f.current, Some(0), "the remaining match is selected");
    assert_eq!((f.auto_from, f.auto_page), (Some(0), Some(1)), "a failure does not end automatic following");
    assert_eq!(v.current, 1, "a failure never moves the view");
    assert_eq!(v.goto, goto, "a failure never flashes the match");

    // The search still follows its original page when that page's text arrives.
    v.last_queue = vec![RenderRequest { page: 0, kind: RequestKind::Text, tag: TEXT_TAG, ..Default::default() }];
    v.receive_result(&ctx, find_text_result(0, "intro sample"));
    assert_eq!(v.current, 0, "the anchor's earlier match takes over");
    let f = v.find.as_ref().unwrap();
    assert_eq!(f.current.and_then(|i| f.matches.get(i)).map(|(p, _)| *p), Some(0));
}

/// A reader who navigated before a page's text failed is not pulled back by a later
/// matching layer: the failure path must preserve the same `Ended` state as a successful
/// one, and the result list may select the late match without revealing it.
#[test]
fn a_failed_text_layer_after_navigation_does_not_reveal_a_late_match() {
    let mut v = view(3, PageLayout::Single, true);
    v.current = 0;
    v.find = Some(Find { query: "sample".into(), ..Default::default() });
    v.rerun_find();
    v.go_to_page(2);
    let moved = v.goto;
    assert!(v.find.as_ref().unwrap().follow_ended, "the reader navigated before any match arrived");

    let ctx = egui::Context::default();
    let failed = RenderRequest { page: 1, kind: RequestKind::Text, tag: TEXT_TAG, ..Default::default() };
    v.last_queue = vec![failed];
    v.receive_result(
        &ctx,
        pdfcraft_render::RenderedPage {
            request: failed,
            width: 0,
            height: 0,
            rgba: Default::default(),
            error: Some("extraction failed".into()),
            text: None,
            warnings: Vec::new(),
            millis: 0,
        },
    );
    let f = v.find.as_ref().unwrap();
    assert!(f.follow_ended, "a failure after navigation does not restart following");
    assert_eq!((f.auto_from, f.auto_page), (None, None));
    assert_eq!(v.current, 2, "a failure never moves the view");
    assert_eq!(v.goto, moved);

    let arrived = RenderRequest { page: 0, kind: RequestKind::Text, tag: TEXT_TAG, ..Default::default() };
    v.last_queue = vec![arrived];
    v.receive_result(&ctx, find_text_result(0, "intro sample"));
    let f = v.find.as_ref().unwrap();
    assert!(f.follow_ended);
    assert_eq!((f.auto_from, f.auto_page), (None, None));
    assert_eq!(v.current, 2, "the late matching layer does not pull the reader back");
    assert_eq!(v.goto, moved, "no flash was issued");
    assert_eq!(f.current.and_then(|i| f.matches.get(i)).map(|(p, _)| *p), Some(0), "the result list still selects the late match");
}

/// The reader can navigate before any text layer has supplied a match. That navigation
/// still ends automatic following, so the first late match is listed but not revealed by
/// a flash that would pull the reader back.
#[test]
fn navigation_before_the_first_match_is_not_overridden_by_a_late_text_layer() {
    let mut v = view(3, PageLayout::Single, true);
    v.current = 0;
    v.find = Some(Find { query: "sample".into(), ..Default::default() });
    v.rerun_find();
    let f = v.find.as_ref().unwrap();
    assert_eq!(f.current, None, "no text layer has arrived yet");
    assert_eq!((f.auto_from, f.auto_page), (Some(0), Some(0)));

    v.go_to_page(2);
    assert_eq!(v.goto, Some((2, 0.0)), "the reader moved to page 3");
    assert!(v.find.as_ref().unwrap().follow_ended, "moving the view ends automatic following");

    let ctx = egui::Context::default();
    let request = RenderRequest { page: 1, kind: RequestKind::Text, tag: TEXT_TAG, ..Default::default() };
    v.last_queue = vec![request];
    v.receive_result(&ctx, find_text_result(1, "late sample"));
    let f = v.find.as_ref().unwrap();
    assert_eq!(v.current, 2, "the late first match does not pull the reader back");
    assert_eq!(v.goto, Some((2, 0.0)), "no flash was issued");
    assert_eq!((f.auto_from, f.auto_page), (None, None));
    assert_eq!(f.current.and_then(|i| f.matches.get(i)).map(|(p, _)| *p), Some(1), "the result list still selects the match");
}

/// A page-box jump to the page already shown is still the reader's navigation: automatic
/// following must end, so a late earlier match is listed without moving the view. (The
/// harness covers wheel, scrollbar and pointer scrolling through the production paint path.)
#[test]
fn a_same_page_page_box_jump_ends_automatic_following() {
    let mut v = view(3, PageLayout::Single, true);
    v.current = 0;
    v.texts.insert(1, Arc::new(find_test_text("first sample")));
    v.find = Some(Find { query: "sample".into(), ..Default::default() });
    v.rerun_find();
    assert_eq!((v.current, v.find.as_ref().unwrap().auto_page), (1, Some(1)), "the automatic choice is on page 2");

    // Choosing the page already shown still ends following; it stays on page 2 at its top.
    v.go_to_page(1);
    let shown = v.goto;
    assert_eq!(shown, Some((1, 0.0)));

    v.texts.insert(0, Arc::new(find_test_text("intro sample")));
    v.refresh_find_page(0);
    let f = v.find.as_ref().unwrap();
    assert!(f.follow_ended);
    assert_eq!((f.auto_from, f.auto_page), (None, None));
    assert_eq!(v.current, 1, "the late earlier match does not move the reader within the page");
    assert_eq!(v.goto, shown, "no flash was issued");
    assert_eq!(f.current.and_then(|i| f.matches.get(i)).map(|(p, _)| *p), Some(1), "the page-2 match stays selected");
}

/// A document-wide invalidation (a layer toggle, an edit, undo/redo) clears results for
/// re-extraction but is not the reader navigating. Automatic following keeps its anchor so
/// the re-extracted layers still reveal the automatic choice; a reader who already took
/// over stays in control. If `document_changed` clamps the view away from that anchor, the
/// next layer ends following instead of revealing a match from the stale page.
#[test]
fn content_invalidation_keeps_the_following_state() {
    let mut v = view(3, PageLayout::Single, true);
    v.current = 0;
    v.texts.insert(1, Arc::new(find_test_text("first sample")));
    v.find = Some(Find { query: "sample".into(), ..Default::default() });
    v.rerun_find();
    assert_eq!((v.current, v.find.as_ref().unwrap().auto_page), (1, Some(1)), "the automatic choice is on page 2");

    v.invalidate_content();
    let f = v.find.as_ref().unwrap();
    assert!(f.matches.is_empty() && f.current.is_none(), "results wait for the re-extracted text");
    assert!(!f.follow_ended, "a content change is not reader navigation");
    assert_eq!((f.auto_from, f.auto_page), (Some(0), Some(1)));

    // Re-extracted text still follows the page the search started from.
    let ctx = egui::Context::default();
    let request = RenderRequest { page: 0, kind: RequestKind::Text, tag: TEXT_TAG, ..Default::default() };
    v.last_queue = vec![request];
    v.receive_result(&ctx, find_text_result(0, "intro sample"));
    assert_eq!(v.current, 0, "the re-extracted match is revealed by the still-active search");
    let f = v.find.as_ref().unwrap();
    assert!(!f.follow_ended);
    assert_eq!((f.auto_from, f.auto_page), (Some(0), Some(0)));

    // A reader who navigated first keeps the view they chose across the invalidation.
    v.go_to_page(2);
    let moved = v.goto;
    assert!(v.find.as_ref().unwrap().follow_ended);
    v.invalidate_content();
    let f = v.find.as_ref().unwrap();
    assert!(f.follow_ended);
    assert_eq!((f.auto_from, f.auto_page), (None, None));
    v.last_queue = vec![request];
    v.receive_result(&ctx, find_text_result(0, "late sample"));
    assert_eq!(v.current, 2, "no delayed reveal after the reader navigated");
    assert_eq!(v.goto, moved, "no flash was issued");
    let f = v.find.as_ref().unwrap();
    assert_eq!(f.current.and_then(|i| f.matches.get(i)).map(|(p, _)| *p), Some(0), "the result list still selects the late match");
}

/// A document edit can shrink the page count. `document_changed` clamps `current` but leaves
/// the automatic anchor's page index alone; a re-extracted layer must then end following
/// rather than reveal a match by scrolling from a page that no longer exists.
#[test]
fn a_page_count_shrink_ends_following_before_a_late_reveal() {
    let mut v = view(5, PageLayout::Single, true);
    v.current = 0;
    v.texts.insert(4, Arc::new(find_test_text("last sample")));
    v.find = Some(Find { query: "sample".into(), ..Default::default() });
    v.rerun_find();
    assert_eq!((v.current, v.find.as_ref().unwrap().auto_page), (4, Some(4)), "the automatic choice is on the last page");

    let info = pdfcraft_render::DocInfo {
        pages: (0..2)
            .map(|_| pdfcraft_render::PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 })
            .collect(),
        ..Default::default()
    };
    v.document_changed(&info);
    assert_eq!(v.current, 1, "the view clamps to the last remaining page");
    let f = v.find.as_ref().unwrap();
    assert!(!f.follow_ended, "the content change itself keeps the automatic state");
    assert_eq!((f.auto_from, f.auto_page), (Some(0), Some(4)), "the anchor still names the removed page");
    let goto = v.goto;

    let ctx = egui::Context::default();
    let request = RenderRequest { page: 0, kind: RequestKind::Text, tag: TEXT_TAG, ..Default::default() };
    v.last_queue = vec![request];
    v.receive_result(&ctx, find_text_result(0, "intro sample"));
    let f = v.find.as_ref().unwrap();
    assert!(f.follow_ended, "a layer for the remaining pages ends the stale follow");
    assert_eq!((f.auto_from, f.auto_page), (None, None));
    assert_eq!(v.current, 1, "the late layer cannot pull the view back");
    assert_eq!(v.goto, goto, "no flash was issued");
    assert_eq!(f.current.and_then(|i| f.matches.get(i)).map(|(p, _)| *p), Some(0), "the result list still selects the match");
}

/// Typing refinements while text layers keep arriving must keep the page the search began
/// on, not re-anchor on whichever page an earlier automatic flash happened to show.
#[test]
fn rapid_query_edits_interleaved_with_text_results_keep_the_start_anchor() {
    let mut v = view(3, PageLayout::Single, true);
    v.current = 0;
    v.find = Some(Find { query: "sam".into(), ..Default::default() });
    v.rerun_find();
    assert!(v.find.as_ref().unwrap().current.is_none());

    let edits = [("samp", 2, "Page three sample", 2), ("sampl", 1, "Page two sample", 1), ("sample", 0, "Page one sample", 0)];
    for (query, page, text, shown) in edits {
        v.find.as_mut().unwrap().query = query.into();
        v.rerun_find();
        let f = v.find.as_ref().unwrap();
        assert_eq!(f.auto_from, Some(0), "refinement keeps the page the reader started from");
        assert!(!f.follow_ended);
        v.texts.insert(page, Arc::new(find_test_text(text)));
        v.refresh_find_page(page);
        let f = v.find.as_ref().unwrap();
        assert_eq!(v.current, shown, "the late page follows the automatic anchor {shown}");
        assert_eq!((f.auto_from, f.auto_page), (Some(0), Some(shown)));
        assert!(!f.follow_ended, "the automatic flash itself does not end following");
    }
    assert!(v.back.is_empty(), "automatic search updates do not record history");
}

#[test]
fn text_layers_evict_far_pages_but_keep_visible_and_matches() {
    let info = thumbnail_info(100);
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    v.current = 50;
    v.frame_visible = HashSet::from([50, 51]);
    for p in 0..100 {
        v.texts.insert(p, Arc::new(PageText::default()));
    }
    v.text_failed.insert(0);
    v.text_failed.insert(99);
    v.find = Some(Find { matches: vec![(0, 0..1)], ..Default::default() });
    let _ = v.prepare_render_queue(&info, 1.0);
    assert!(v.texts.contains_key(&50) && v.texts.contains_key(&51), "visible pages stay");
    assert!(v.texts.contains_key(&0), "matched pages stay");
    assert!(v.texts.contains_key(&49), "nearest prefetch window stays");
    assert!(!v.texts.contains_key(&99), "far pages go");
    assert_eq!(v.texts.len(), 2 + 1 + MAX_CACHED_TEXT_PAGES, "visible + matched + prefetch window");
    assert!(v.text_failed.contains(&0) && v.text_failed.contains(&99), "failures are bytes-free: never evict");
}

/// A PDF of one line of text on each page, `lines[i]` on page `i`.
fn text_pages_pdf(lines: &[&str]) -> Vec<u8> {
    let n = lines.len();
    // 1 catalog, 2 pages, 3 font, then a page and its content stream for each line.
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Count {n} /Kids [{}] >>", (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect::<Vec<_>>().join(" ")),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    for (i, line) in lines.iter().enumerate() {
        let content = format!("BT /F1 18 Tf 72 700 Td ({line}) Tj ET");
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + 2 * i
        ));
        objects.push(format!("<< /Length {} >>\nstream\n{content}\nendstream", content.len()));
    }
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
    out
}

/// #2639: scrolling continuously, Select All takes every page's text, and Copy gets all of it
/// in page order, reading the pages whose text wasn't read for display.
#[test]
fn select_all_copies_the_whole_document_while_scrolling_continuously() {
    let mut app = PdfCraftApp::new();
    app.open_bytes("three.pdf", None, text_pages_pdf(&["Alpha one", "Bravo two", "Charlie three"])).unwrap();
    let v = &mut app.views[0];
    assert!(v.continuous, "continuous scrolling is the default");
    assert!(v.texts.is_empty(), "no page's text read yet");
    // Nothing shown is selected yet (false, as for a page whose text isn't read), but the whole
    // document is: Copy reads every page.
    assert!(!v.select_all());
    assert!(v.all_text && v.selection.is_none());
    assert_eq!(v.pages_without_text(), vec![0, 1, 2]);
    // Copy reads the pages off the UI thread; the clipboard gets the text once they're read.
    let ctx = egui::Context::default();
    assert!(app.copy_selected_text(0, &ctx), "the copy starts");
    assert!(app.document_copy.is_some(), "reading the pages, not on this thread");
    let mut text = None;
    for _ in 0..500 {
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.finish_document_copy(ui.ctx()));
        out.textures_delta.clear();
        text = out.platform_output.commands.iter().find_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        });
        if text.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let text = text.expect("the copied text arrives");
    assert!(app.document_copy.is_none(), "done");
    let at = |w: &str| text.find(w).unwrap_or_else(|| panic!("{w:?} missing from {text:?}"));
    assert!(at("Alpha") < at("Bravo") && at("Bravo") < at("Charlie"), "page order: {text:?}");
    assert_eq!(text.lines().count(), 3, "a line per page: {text:?}");
    // A new scope ends it: one page at a time, Select All is the page's again.
    app.views[0].set_continuous(false);
    assert!(!app.views[0].all_text, "not after switching to one page at a time");
    // So do a page selection and a change of the document's content (an edit, undo).
    app.views[0].set_continuous(true);
    app.views[0].select_all();
    app.views[0].select_text(0, 0, 0);
    assert!(!app.views[0].all_text, "a page selection replaces it");
    app.views[0].select_all();
    app.views[0].invalidate_content();
    assert!(!app.views[0].all_text, "not past an edit of the document");
}

/// A whole-document copy reads the pages without text, keeps those already read and skips
/// those that failed before.
#[test]
fn a_document_copy_reads_only_the_pages_it_needs() {
    let mut app = PdfCraftApp::new();
    app.open_bytes("three.pdf", None, text_pages_pdf(&["Alpha one", "Bravo two", "Charlie three"])).unwrap();
    app.views[0].text_failed.insert(2);
    let job = crate::DocumentCopyJob::of(&app.views[0], app.session.get(app.views[0].id).unwrap());
    let kinds: Vec<_> = job.pages.iter().map(|p| matches!(p, crate::CopyPage::ToRead)).collect();
    assert_eq!(kinds, vec![true, true, false], "pages 1 and 2 to read, page 3 failed before");
    assert!(matches!(job.pages[2], crate::CopyPage::Skip));
}

/// A later copy wins: a whole-document copy still reading its pages never replaces what was
/// copied after it started, from the page or anywhere else (a text field, a comment).
#[test]
fn a_later_copy_cancels_a_document_copy_still_reading() {
    let mut app = PdfCraftApp::new();
    app.open_bytes("three.pdf", None, text_pages_pdf(&["Alpha one", "Bravo two", "Charlie three"])).unwrap();
    let ctx = egui::Context::default();
    // Another copy in the same frame (here a text field's) drops it.
    app.views[0].select_all();
    assert!(app.copy_selected_text(0, &ctx) && app.document_copy.is_some());
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
        ui.ctx().copy_text("a word".to_owned());
        app.finish_document_copy(ui.ctx());
    });
    out.textures_delta.clear();
    assert!(app.document_copy.is_none(), "cancelled");
    let copies: Vec<_> = out.platform_output.commands.iter().filter(|c| matches!(c, egui::OutputCommand::CopyText(_))).collect();
    assert_eq!(copies, vec![&egui::OutputCommand::CopyText("a word".to_owned())], "the later copy stays");
    // So does copying a page selection.
    app.views[0].select_all();
    assert!(app.copy_selected_text(0, &ctx) && app.document_copy.is_some());
    app.views[0].select_text(0, 0, 5);
    app.copy_selected_text(0, &ctx);
    assert!(app.document_copy.is_none(), "a page selection's copy cancels it");
}

/// A press anywhere on a page ends Select All of the document, whatever the tool (here the
/// Hand, which selects no text) and whether the page's text is read yet.
#[test]
fn a_press_on_a_page_ends_select_all_of_the_document() {
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("three.pdf", None, text_pages_pdf(&["Alpha one", "Bravo two", "Charlie three"])).unwrap();
        app
    });
    let mut on_screen = None;
    for _ in 0..600 {
        h.run_steps(2);
        on_screen = h.state().views[0].page_screen_rect(0);
        if on_screen.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let r = on_screen.expect("the first page is on screen");
    h.state_mut().quick_tool = crate::QuickTool::Hand;
    h.state_mut().views[0].select_all();
    assert!(h.state().views[0].all_text);
    let at = r.center();
    h.event(egui::Event::PointerMoved(at));
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(2);
    assert!(!h.state().views[0].all_text, "a press on the page ends it");

    // A press while a dialog is up lands on the dialog, not the page: the selection stays.
    h.state_mut().views[0].select_all();
    h.state_mut().dialog = Some(crate::Dialog::JsConsole);
    h.run_steps(2);
    h.event(egui::Event::PointerMoved(at));
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE });
    h.run_steps(1);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
    h.run_steps(2);
    assert!(h.state().views[0].all_text, "a press beneath a dialog doesn't end it");
}

/// The pages of a whole-document copy come out in order whatever was read before, and a
/// cancelled copy gives nothing (a new copy or a closed document cancels the old one).
#[test]
fn a_document_copy_reads_in_page_order_and_stops_when_cancelled() {
    let mut app = PdfCraftApp::new();
    app.open_bytes("three.pdf", None, text_pages_pdf(&["Alpha one", "Bravo two", "Charlie three"])).unwrap();
    // The middle page's text read for display already; the others read by the copy.
    let doc = app.session.get(app.views[0].id).unwrap();
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), Default::default());
    let middle = r.render(RenderRequest { page: 1, kind: RequestKind::Text, ..Default::default() }).text.unwrap();
    app.views[0].texts.insert(1, middle);
    let job = || crate::DocumentCopyJob::of(&app.views[0], app.session.get(app.views[0].id).unwrap());
    let text = job().text(&std::sync::atomic::AtomicBool::new(false)).unwrap();
    assert!(text.find("Alpha").unwrap() < text.find("Bravo").unwrap() && text.find("Bravo").unwrap() < text.find("Charlie").unwrap(), "{text:?}");
    assert_eq!(job().text(&std::sync::atomic::AtomicBool::new(true)), None, "cancelled");
}

/// Select All keeps to the page shown one page at a time, and in a document of one page; of
/// the document, it reads the text already read and doesn't ask again for failed pages.
#[test]
fn select_all_of_the_document_only_while_scrolling_through_several_pages() {
    let info = thumbnail_info(3);
    let one_line = |s: &str| PageText {
        glyphs: s
            .chars()
            .map(|c| pdfcraft_render::TextGlyph { text: c.into(), rect: [0.0, 0.0, 10.0, 10.0], quad: [[0.0, 0.0]; 4], direction: [1.0, 0.0] })
            .collect(),
        line_of: s.chars().map(|_| 0).collect(),
        space_before: s.chars().map(|_| false).collect(),
    };
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    for p in 0..3 {
        v.texts.insert(p, Arc::new(one_line("abc")));
    }
    v.continuous = false;
    v.current = 1;
    assert!(v.select_all());
    assert!(!v.all_text && v.selection.is_some_and(|s| s.page == 1), "one page at a time: the page shown");
    let single = thumbnail_info(1);
    let mut w = DocView::new(DocId(2), &single, ViewDefaults::default());
    w.texts.insert(0, Arc::new(one_line("abc")));
    assert!(w.select_all());
    assert!(!w.all_text && w.selection.is_some(), "a one-page document: that page");
    // Text read for display is used; pages that failed aren't asked for again.
    let mut d = DocView::new(DocId(3), &info, ViewDefaults::default());
    d.texts.insert(1, Arc::new(one_line("abc")));
    d.text_failed.insert(2);
    assert!(d.select_all() && d.all_text);
    assert_eq!(d.pages_without_text(), vec![0]);
}

#[test]
fn text_layers_keep_the_selection_page() {
    let info = thumbnail_info(100);
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    let text = PageText {
        glyphs: vec![pdfcraft_render::TextGlyph { text: "x".into(), rect: [0.0, 0.0, 10.0, 10.0], quad: [[0.0, 0.0]; 4], direction: [1.0, 0.0] }],
        line_of: vec![0],
        space_before: vec![false],
    };
    for p in 0..100 {
        v.texts.insert(p, Arc::new(text.clone()));
    }
    v.select_text(0, 0, 0);
    v.current = 90;
    v.frame_visible = HashSet::from([90, 91]);
    let _ = v.prepare_render_queue(&info, 1.0);
    assert_eq!(v.selected_text().as_deref(), Some("x"), "stale selections still copy");
}

#[test]
fn text_layers_stay_while_a_search_is_active() {
    let info = thumbnail_info(100);
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    v.current = 50;
    v.frame_visible = HashSet::from([50]);
    for p in 0..100 {
        v.texts.insert(p, Arc::new(PageText::default()));
    }
    v.find = Some(Find { query: "dup".into(), case_query: "dup".into(), matches: vec![(0, 0..1)], ..Default::default() });
    let _ = v.prepare_render_queue(&info, 1.0);
    assert_eq!(v.texts.len(), 100, "evicting mid-search would churn the workers");
}

fn thumbnail_info(count: usize) -> DocInfo {
    DocInfo {
        pages: (0..count)
            .map(|i| {
                let (width, height) = match i % 3 {
                    0 => (612.0, 792.0),
                    1 => (200.0, 4000.0),
                    _ => (4000.0, 200.0),
                };
                pdfcraft_render::PageInfo { width, height, label: (i + 1).to_string(), crop: [0.0, 0.0, width, height], rotation: 0 }
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn ten_thousand_pages_only_demand_viewport_and_prefetch_at_both_densities() {
    let info = thumbnail_info(10_000);
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    let cols = 4;
    let rows = info.pages.len().div_ceil(cols);
    for ppp in [1.0, 2.0] {
        for row in [0, 1234, rows - 1] {
            v.begin_render_frame();
            let range = thumbnail_rows(row as f32 * 250.0, row as f32 * 250.0 + 750.0, 250.0, rows);
            for p in range.start * cols..(range.end * cols).min(info.pages.len()) {
                v.need_thumbnail(p, p / cols == row);
            }
            let queue = v.prepare_render_queue(&info, ppp);
            assert!(queue.len() <= 5 * cols, "only three visible rows and two prefetch rows");
            assert_eq!(queue.first().unwrap().page / cols, row, "visible cells precede prefetch");
            let bytes: usize = queue
                .iter()
                .map(|r| {
                    let p = &info.pages[r.page];
                    let (w, h) = (device_pixels(p.width, r.scale), device_pixels(p.height, r.scale));
                    assert!(w <= (THUMB_W * ppp).ceil() as u32);
                    assert!(h <= (THUMB_H * ppp).ceil() as u32);
                    rgba_bytes(w as usize, h as usize)
                })
                .sum();
            assert!(bytes <= THUMB_BYTES);
            assert_eq!(queue, v.prepare_render_queue(&info, ppp), "unchanged demand reconstructs the same work");
        }
    }
}

#[test]
fn oversized_thumbnail_viewport_scales_all_admitted_cells_to_one_budget() {
    let info = thumbnail_info(10_000);
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    for page in 0..MAX_THUMB_DEMAND {
        v.need_thumbnail(page, true);
    }
    let queue = v.prepare_render_queue(&info, 2.0);
    assert_eq!(queue.len(), MAX_THUMB_DEMAND);
    let bytes: usize = queue
        .iter()
        .map(|r| {
            let p = &info.pages[r.page];
            rgba_bytes(device_pixels(p.width, r.scale) as usize, device_pixels(p.height, r.scale) as usize)
        })
        .sum();
    assert!(bytes <= THUMB_BYTES);
}

fn solid_result(request: RenderRequest, w: u32, h: u32) -> pdfcraft_render::RenderedPage {
    pdfcraft_render::RenderedPage {
        request,
        width: w,
        height: h,
        rgba: pdfcraft_render::Pixels::from(vec![u32::MAX; rgba_bytes(w as usize, h as usize) / 4]),
        text: None,
        error: None,
        warnings: Vec::new(),
        millis: 0,
    }
}

/// #3149: the replacement dialog commits after the canvas collected this frame's demand.
#[test]
fn replacing_a_page_after_layout_never_uploads_an_oversized_texture() {
    let ctx = egui::Context::default();
    ctx.input_mut(|i| i.max_texture_side = 2048);
    let mut app = PdfCraftApp::new();
    let original = app.session.create_blank(200.0, 300.0, 1).unwrap();
    let replacement = app.session.create_blank(400.0, 500.0, 1).unwrap();
    app.open_bytes("original.pdf", None, original.as_ref().clone()).unwrap();
    let request = RenderRequest { scale: 4.7, tag: scale_tag(4.7), ..Default::default() };
    app.views[0].frame_queue = vec![request];
    app.views[0].frame_visible.insert(0);
    assert!(app.apply_edit(Edit::ReplacePages { pages: vec![0], name: "replacement.pdf".into(), bytes: replacement, src_pages: vec![0] }));
    let doc = app.session.get(app.views[0].id).unwrap();
    assert_eq!((doc.info.pages[0].width, doc.info.pages[0].height), (400.0, 500.0));
    let edited_bytes = doc.bytes.clone();
    let pool = RenderPool::new_inline(doc.display.clone(), Default::default());
    app.views[0].finish_render_frame(&ctx, &doc.info, &pool);
    assert!(app.views[0].last_queue.is_empty(), "old geometry must not reach the replacement renderer");

    // The next layout can render a bounded backdrop and sharp tiles at the same zoom.
    let backdrop = RenderRequest { scale: 2048.0 / 500.0, tag: scale_tag(2048.0 / 500.0), ..request };
    let tile = RenderRequest { tile: Some(Tile { x: 0, y: 0, w: 1024, h: 1024 }), ..request };
    for _ in 0..2 {
        let view = &mut app.views[0];
        view.begin_render_frame();
        view.frame_visible.insert(0);
        view.frame_queue = vec![backdrop, tile];
        view.finish_render_frame(&ctx, &doc.info, &pool);
    }
    let view = &app.views[0];
    assert_eq!(view.pages.get(&0).unwrap().tex.size(), [1639, 2048]);
    assert_eq!(view.tiles.get(&(0, tile.tag, 0, 0)).unwrap().size(), [1024, 1024]);
    assert!(view.errors.is_empty());
    assert_eq!(doc.bytes, edited_bytes, "rendering preserves the unsaved edit");
    assert!(doc.dirty);
    assert_eq!(doc.can_undo(), Some("Replace page"));
}

#[test]
fn oversized_raster_results_do_not_replace_cached_textures() {
    let ctx = egui::Context::default();
    ctx.input_mut(|i| i.max_texture_side = 64);
    for request in [
        RenderRequest::default(),
        RenderRequest { tag: THUMB_TAG, ..Default::default() },
        RenderRequest { tag: GRID_TAG, ..Default::default() },
        RenderRequest { tag: PRINT_TAG, ..Default::default() },
        RenderRequest { tile: Some(Tile { x: 0, y: 0, w: 64, h: 32 }), ..Default::default() },
    ] {
        let mut view = DocView::new(DocId(1), &thumbnail_info(1), ViewDefaults::default());
        view.last_queue = vec![request];
        let texture = |v: &DocView| {
            v.pages
                .values()
                .map(|p| &p.tex)
                .chain(v.thumbs.values().map(|p| &p.tex))
                .chain(v.grid_pages.values().map(|(_, t)| t))
                .chain(v.print_pages.values().map(|(_, t)| t))
                .chain(v.tiles.values())
                .map(|t| (t.id(), t.size()))
                .collect::<Vec<_>>()
        };
        assert_eq!(view.receive_result(&ctx, solid_result(request, 64, 32)), 64 * 32 * 4);
        let cached = texture(&view);
        assert_eq!(cached.len(), 1);
        for (w, h) in [(65, 32), (32, 65), (65, 65), (0, 32), (32, 0)] {
            assert_eq!(view.receive_result(&ctx, solid_result(request, w, h)), 0, "{request:?}: {w}x{h}");
            assert_eq!(texture(&view), cached, "an invalid result must keep the previous raster");
        }
        assert_eq!(view.receive_result(&ctx, solid_result(request, 32, 64)), 32 * 64 * 4);
        assert_eq!(texture(&view)[0].1, [32, 64], "valid results still make progress");
    }
}

/// A print preview carries the renderer's print bit; the bit that asks for luminance is
/// its own, so a colour preview is not converted to monochrome and the two modes are told
/// apart by their tags.
#[test]
fn print_preview_tag_distinguishes_grayscale_from_colour() {
    let mut v = view(1, PageLayout::Single, true);
    v.set_print_grayscale(false);
    v.queue_print_previews(&[(0, 1.0)]);
    let colour = v.frame_queue.last().expect("a print preview was queued").tag;
    assert_ne!(colour & PRINT_TAG, 0, "a print preview carries the renderer's print bit");
    assert_eq!(colour & GRAYSCALE_TAG, 0, "a colour print preview must not ask for luminance");

    v.set_print_grayscale(true);
    v.queue_print_previews(&[(0, 1.0)]);
    let gray = v.frame_queue.last().expect("a grayscale print preview was queued").tag;
    assert_ne!(gray & GRAYSCALE_TAG, 0, "a grayscale print preview asks for luminance");
    assert_ne!(gray, colour, "the two print preview modes must not share a tag");
}

#[test]
fn thumbnails_release_on_scroll_panel_close_and_request_again_after_eviction() {
    let ctx = egui::Context::default();
    let info = thumbnail_info(10_000);
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    v.need_thumbnail(0, true);
    let first = v.prepare_render_queue(&info, 1.0);
    v.last_queue = first.clone();
    assert_eq!(v.receive_result(&ctx, solid_result(first[0], 132, 171)), 132 * 171 * 4);
    assert!(v.prepare_render_queue(&info, 1.0).is_empty());
    v.begin_render_frame();
    v.need_thumbnail(9999, true);
    v.prepare_render_queue(&info, 1.0);
    assert!(v.thumbs.is_empty(), "jump releases old handles");
    v.begin_render_frame();
    v.need_thumbnail(0, true);
    assert_eq!(v.prepare_render_queue(&info, 1.0), first, "an evicted result must be requested again");
    v.begin_render_frame();
    assert!(v.prepare_render_queue(&info, 1.0).is_empty(), "closing consumers cancels thumbnail work");
}

#[test]
fn stale_results_are_rejected_before_conversion_after_dpi_invalidation_and_suspend() {
    let ctx = egui::Context::default();
    let info = thumbnail_info(1);
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    v.need_thumbnail(0, true);
    let first = v.prepare_render_queue(&info, 1.0)[0];
    v.last_queue = v.prepare_render_queue(&info, 2.0);
    // Valid pixels: only the request gate can keep these out (a size check can't).
    let obsolete = || solid_result(first, 132, 171);
    assert_eq!(v.receive_result(&ctx, obsolete()), 0);
    assert!(v.thumbs.is_empty(), "a result for the old DPI is not uploaded");
    // The same result is uploaded while its request is current.
    v.last_queue = vec![first];
    assert_eq!(v.receive_result(&ctx, obsolete()), 132 * 171 * 4);
    v.thumbs.clear();
    v.invalidate_content();
    assert_eq!(v.receive_result(&ctx, obsolete()), 0);
    assert!(v.thumbs.is_empty(), "nor after the content changed");
    let pool = RenderPool::new_inline(Arc::new(Vec::new()), Default::default());
    v.last_queue = vec![first];
    v.suspend_rendering(&pool);
    assert_eq!(v.receive_result(&ctx, obsolete()), 0);
    assert!(v.thumbs.is_empty(), "nor after the tab was suspended");
}

#[test]
fn editing_after_layout_preserves_stale_page_until_replacement() {
    let ctx = egui::Context::default();
    let info = thumbnail_info(1);
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    let req = RenderRequest { scale: 1.0, tag: 1000, ..Default::default() };
    v.last_queue = vec![req];
    v.receive_result(&ctx, solid_result(req, 10, 10));
    v.frame_queue = vec![req];
    v.page_changed(0);
    assert_eq!(v.prepare_render_queue(&info, 1.0), vec![req]);
    assert_eq!(v.pages.get(&0).unwrap().tag, STALE_TAG);
}

/// #1858: a released drawing stays painted from its points until a raster that has it
/// arrives. A page raster that lands before the drawing's edit is applied (in full screen the
/// edit waits for the next frame, and a render already under way can finish first) is of the
/// document without it, and must not end the preview.
#[test]
fn a_raster_from_before_a_drawing_is_committed_keeps_it_drawn() {
    let ctx = egui::Context::default();
    let info = thumbnail_info(1);
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    let req = RenderRequest { scale: 1.0, tag: 1000, ..Default::default() };
    v.comments.settle(0.0, 0, crate::comments::CommentTool::Ink, &[[10.0, 10.0], [40.0, 30.0]]);
    v.last_queue = vec![req];
    assert!(v.receive_result(&ctx, solid_result(req, 10, 10)) > 0, "the raster is taken");
    assert_eq!(v.comments.settling_len(), 1, "a raster from before the commit doesn't have the stroke");
    v.comments.drawings_committed();
    v.last_queue = vec![req];
    assert!(v.receive_result(&ctx, solid_result(req, 10, 10)) > 0);
    assert_eq!(v.comments.settling_len(), 0, "a raster after the commit draws it");
    // An edit that fails leaves nothing to wait for.
    v.comments.settle(0.0, 0, crate::comments::CommentTool::Ink, &[[10.0, 10.0], [40.0, 30.0]]);
    v.comments.drawings_refused();
    assert_eq!(v.comments.settling_len(), 0, "a refused drawing isn't left painted");
}

#[test]
fn page_and_tile_admission_accounts_for_bytes_before_count_limits() {
    let mut info = thumbnail_info(40);
    for p in &mut info.pages {
        p.width = 4000.0;
        p.height = 4000.0;
    }
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    v.frame_queue = (0..40).map(|page| RenderRequest { page, scale: 1.0, tag: 1000, ..Default::default() }).collect();
    let queue = v.prepare_render_queue(&info, 1.0);
    assert_eq!(queue.len(), 2, "128 MiB admits two 4000-square pages, not 24");
    v.frame_queue = (0..40)
        .map(|page| RenderRequest { page, scale: 1.0, tag: 1000, tile: Some(Tile { x: 0, y: 0, w: 1024, h: 1024 }), ..Default::default() })
        .collect();
    assert_eq!(v.prepare_render_queue(&info, 1.0).len(), 32);
    assert!(!v.admit_texture(RenderRequest::default(), PAGE_BYTES + 1));
}

#[test]
fn on_screen_pages_and_tiles_are_admitted_beyond_the_prefetch_allowance() {
    let ctx = egui::Context::default();
    let mut info = thumbnail_info(40);
    for p in &mut info.pages {
        p.width = 2000.0;
        p.height = 2000.0;
    }
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    // Ten 16 MB pages on screen (a zoomed-out HiDPI view) exceed the 128 MiB allowance.
    v.frame_visible = (0..10).collect();
    v.frame_queue = (0..40).map(|page| RenderRequest { page, scale: 1.0, tag: 1000, ..Default::default() }).collect();
    let queue = v.prepare_render_queue(&info, 1.0);
    let pages: Vec<usize> = queue.iter().map(|r| r.page).collect();
    assert_eq!(pages, (0..10).collect::<Vec<_>>(), "every on-screen page, and no prefetch past the allowance");
    v.last_queue = queue.clone();
    for req in &queue {
        assert!(v.receive_result(&ctx, solid_result(*req, 2000, 2000)) > 0, "on-screen page {} uploaded", req.page);
    }
    assert_eq!(v.pages.len(), 10, "none evicted another on-screen page");
    // A 5K viewport needs more 1024-pixel tiles than the tile allowance holds.
    v.frame_queue = (0..40)
        .map(|x| RenderRequest { page: 0, scale: 1.0, tag: 1000, tile: Some(Tile { x: x * TILE, y: 0, w: 1024, h: 1024 }), ..Default::default() })
        .collect();
    assert_eq!(v.prepare_render_queue(&info, 1.0).len(), 40);
}

#[test]
fn a_page_turned_back_to_is_still_cached() {
    let ctx = egui::Context::default();
    let info = thumbnail_info(10);
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    let req = |page| RenderRequest { page, scale: 1.0, tag: 1000, ..Default::default() };
    v.frame_visible = [0].into();
    v.frame_queue = vec![req(0)];
    v.last_queue = v.prepare_render_queue(&info, 1.0);
    v.receive_result(&ctx, solid_result(req(0), 100, 130));
    // Single-page view moves on to page 1: page 0 is no longer demanded.
    v.begin_render_frame();
    v.current = 1;
    v.frame_visible = [1].into();
    v.frame_queue = vec![req(1)];
    assert_eq!(v.prepare_render_queue(&info, 1.0), vec![req(1)]);
    assert!(v.pages.contains_key(&0), "kept while the allowance has room");
    // And back: nothing to render.
    v.begin_render_frame();
    v.current = 0;
    v.frame_visible = [0].into();
    v.frame_queue = vec![req(0)];
    assert!(v.prepare_render_queue(&info, 1.0).is_empty());
}

#[test]
fn visible_pages_render_before_thumbnails_and_text() {
    let ctx = egui::Context::default();
    let info = thumbnail_info(4);
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    let main = RenderRequest { page: 0, scale: 1.0, tag: 1000, ..Default::default() };
    let text = RenderRequest { page: 0, kind: RequestKind::Text, scale: 1.0, tag: TEXT_TAG, ..Default::default() };
    // Opening page 0 queues its pixels and text alongside Pages-panel thumbnails.
    v.begin_render_frame();
    v.frame_visible = [0].into();
    v.frame_queue = vec![main, text];
    for p in 1..4 {
        v.need_thumbnail(p, true);
    }
    let queue = v.prepare_render_queue(&info, 1.0);
    assert_eq!(queue, vec![main], "visible pixels first: no thumbnails or text yet");
    // Once the visible raster arrives, background work is admitted.
    v.last_queue = queue;
    assert!(v.receive_result(&ctx, solid_result(main, 100, 130)) > 0);
    v.begin_render_frame();
    v.frame_visible = [0].into();
    v.frame_queue = vec![text];
    for p in 1..4 {
        v.need_thumbnail(p, true);
    }
    let queue = v.prepare_render_queue(&info, 1.0);
    assert!(queue.iter().any(|r| r.kind == RequestKind::Text), "text admitted after pixels");
    assert!(queue.iter().any(|r| r.tag & THUMB_TAG != 0), "thumbnails admitted after pixels");
    assert!(!queue.iter().any(|r| r.kind == RequestKind::Pixels && r.tag & THUMB_TAG == 0), "no main pixels re-requested");
    // An errored visible page never holds background work back.
    v.errors.insert(0, "gone".to_string());
    v.begin_render_frame();
    v.frame_visible = [0].into();
    v.frame_queue = vec![main, text];
    for p in 1..4 {
        v.need_thumbnail(p, true);
    }
    let queue = v.prepare_render_queue(&info, 1.0);
    assert!(queue.iter().any(|r| r.kind == RequestKind::Text), "text admitted despite the error");
    assert!(queue.iter().any(|r| r.tag & THUMB_TAG != 0), "thumbnails still admitted");
}

#[test]
fn thumbnail_tags_do_not_change_as_a_row_scrolls_in_or_out() {
    let info = thumbnail_info(10_000);
    let mut v = DocView::new(DocId(1), &info, ViewDefaults::default());
    let tags = |v: &mut DocView, cells: usize| {
        v.begin_render_frame();
        for page in 0..cells {
            v.need_thumbnail(page, true);
        }
        v.thumbnail_requests(&info, 2.0).into_iter().map(|r| (r.page, r.tag)).collect::<HashMap<_, _>>()
    };
    // 2x on a large screen: about 70 cells, give or take a row of six.
    let (a, b) = (tags(&mut v, 70), tags(&mut v, 76));
    assert!((0..70).all(|p| a[&p] == b[&p]), "a row more or less re-renders nothing");
}

#[test]
fn many_open_documents_share_one_raster_allowance_and_close_releases_handles() {
    let ctx = egui::Context::default();
    let mut app = PdfCraftApp::new();
    let bytes = app.session.create_blank(612.0, 792.0, 1).unwrap();
    let mut expected = Vec::new();
    for index in 0..6 {
        app.open_bytes(&format!("synthetic-{index}.pdf"), None, bytes.as_ref().clone()).unwrap();
        let info = app.session.get(app.views.last().unwrap().id).unwrap().info.clone();
        let view = app.views.last_mut().unwrap();
        view.begin_render_frame();
        view.need_thumbnail(0, true);
        let request = view.thumbnail_requests(&info, ctx.pixels_per_point()).into_iter().next().unwrap();
        let page = info.pages.get(request.page).unwrap();
        let dimensions = (device_pixels(page.width, request.scale), device_pixels(page.height, request.scale));
        view.last_queue = vec![request];
        assert_eq!(
            view.receive_result(&ctx, solid_result(request, dimensions.0, dimensions.1)),
            rgba_bytes(dimensions.0 as usize, dimensions.1 as usize)
        );
        assert_eq!(view.thumbs.get(&0).unwrap().tag, request.tag);
        assert_eq!(view.thumbs.get(&0).unwrap().tex.size(), [dimensions.0 as usize, dimensions.1 as usize]);

        view.begin_render_frame();
        view.need_thumbnail(0, true);
        let queue = view.prepare_render_queue(&info, ctx.pixels_per_point());
        assert!(queue.is_empty(), "the synthetic thumbnail satisfies current demand");
        view.last_queue = queue;
        expected.push((request, dimensions));
    }
    assert_eq!(app.views.len(), 6, "all six documents were opened");
    assert_eq!(app.active_ids().map(|(index, _)| index), Some(5));
    assert!(app.views.iter().zip(&expected).all(|(view, (request, dimensions))| {
        view.thumbs.len() == 1
            && view.thumbs.get(&0).is_some_and(|thumb| thumb.tag == request.tag && thumb.tex.size() == [dimensions.0 as usize, dimensions.1 as usize])
    }));

    app.finish_render_frame(&ctx);
    let total: usize = app.views.iter().map(|v| v.thumbs.values().map(|p| texture_bytes(&p.tex)).sum::<usize>()).sum();
    let (active_request, active_dimensions) = expected.last().unwrap();
    let active_bytes = rgba_bytes(active_dimensions.0 as usize, active_dimensions.1 as usize);
    assert_eq!(total, active_bytes, "only the active view retains a thumbnail");
    assert!(total <= THUMB_BYTES, "retained thumbnails stay within the existing allowance");
    assert!(app.views.iter().take(5).all(|v| v.thumbs.is_empty() && v.last_queue.is_empty()));
    let active = app.views.last().unwrap();
    assert_eq!(active.thumbs.len(), 1);
    assert_eq!(active.thumbs.get(&0).unwrap().tag, active_request.tag);
    assert!(active.last_queue.is_empty(), "the current thumbnail is already cached");
    for view in &app.views {
        let doc = app.session.get(view.id).unwrap();
        assert_eq!(doc.renderer.stats().render_requests, 0, "the fixture must not launch a native render");
    }

    app.close_tab(5);
    assert_eq!(app.views.len(), 5, "closing the active tab drops its owning view");
    assert!(app.views.iter().all(|v| v.thumbs.is_empty()));
}

#[test]
fn suspended_signature_preview_releases_layers_and_rebuilds_without_editing() {
    let ctx = egui::Context::default();
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_pixel(12, 4, image::Rgba([20, 30, 40, 255])).write_to(&mut png, image::ImageFormat::Png).unwrap();
    let image = pdfcraft_engine::SignatureImage::from_bytes(png.get_ref()).unwrap();
    let mut session = pdfcraft_engine::Session::new();
    let blank = session.create_blank(300.0, 400.0, 1).unwrap();
    let id = session.open_new("signature-preview.pdf", blank).unwrap();
    let info = session.get(id).unwrap().info.pages[0].clone();
    session.apply(id, image.edit(0, &info, [20.0, 100.0], false, "Test").unwrap()).unwrap();
    let doc = session.get(id).unwrap();
    let bytes = doc.bytes.clone();
    let generation = doc.edit_generation();
    let mut v = DocView::new(id, &doc.info, ViewDefaults::default());
    v.comments.selected = Some((0, 0));
    v.signature_drag.prepare(&ctx, doc, v.comments.selected, 1.0);
    assert!(v.signature_drag.contains(0, 0));

    v.suspend_rendering(&doc.renderer);
    assert!(!v.signature_drag.contains(0, 0), "inactive tabs release the preview pool and textures");
    assert_eq!(v.comments.selected, Some((0, 0)), "the user's selection survives suspension");
    v.signature_drag.prepare(&ctx, doc, v.comments.selected, 1.0);
    assert!(v.signature_drag.contains(0, 0), "reactivating the view rebuilds the selected image's preview");
    assert!(Arc::ptr_eq(&bytes, &doc.bytes));
    assert_eq!(doc.edit_generation(), generation);
    assert_eq!(doc.can_undo(), Some("Add signature"));
    assert!(doc.dirty);
}

#[test]
fn suspending_a_view_releases_owned_raster_textures_and_queue_state() {
    let ctx = egui::Context::default();
    let mut view = view(2, PageLayout::Single, true);
    let texture = |name| ctx.load_texture(name, egui::ColorImage::filled([1, 1], Color32::WHITE), TextureOptions::LINEAR);
    let request = RenderRequest { page: 0, scale: 1.0, tag: 1, ..Default::default() };
    view.pages.insert(0, PageTex { tag: request.tag, tex: texture("suspended-page") });
    view.thumbs.insert(1, PageTex { tag: THUMB_TAG, tex: texture("suspended-thumbnail") });
    view.tiles.insert((0, request.tag, 0, 0), texture("suspended-tile"));
    view.grid_pages.insert(1, (1, texture("suspended-grid-page")));
    view.print_pages.insert(1, (PRINT_TAG, texture("suspended-print-page")));
    view.stale_thumbs.insert(1);
    view.waiting_since.insert(0, 1.0);
    view.last_queue = vec![request];
    view.frame_queue = vec![request];
    view.frame_visible.insert(0);
    view.need_thumbnail(1, true);

    let pool = RenderPool::new_inline(Arc::new(Vec::new()), Default::default());
    view.suspend_rendering(&pool);

    assert!(view.pages.is_empty(), "page handles are released");
    assert!(view.thumbs.is_empty(), "thumbnail handles are released");
    assert!(view.tiles.is_empty(), "tile handles are released");
    assert!(view.grid_pages.is_empty(), "organize-grid handles are released");
    assert!(view.print_pages.is_empty(), "print-preview handles are released");
    assert!(view.stale_thumbs.is_empty(), "stale thumbnail bookkeeping is cleared");
    assert!(view.waiting_since.is_empty(), "waiting bookkeeping is cleared");
    assert!(view.last_queue.is_empty() && !view.render_pending(), "queued demand is retired");
    assert!(view.frame_queue.is_empty() && view.frame_visible.is_empty() && view.thumb_demand.is_empty());
}

/// Edit text & images keeps its boxes while page input is blocked (the window unfocused, a
/// dialog or the palette open): the page's paragraphs are still measured for the latest edit
/// and drawn, instead of vanishing until the pointer comes back.
#[test]
fn edit_text_boxes_stay_drawn_while_page_input_is_blocked() {
    let mut h = egui_kittest::Harness::builder().with_size(vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        let bytes = app.session.create_blank(300.0, 400.0, 1).unwrap();
        app.open_bytes("blank.pdf", None, bytes.as_ref().clone()).unwrap();
        app.quick_tool = QuickTool::EditText;
        app
    });
    h.run_steps(3);
    // Block page input the way the window losing focus does.
    h.state_mut().palette_open = true;
    h.run_steps(2);
    assert!(h.state_mut().apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 }));
    h.run_steps(3);
    let app = h.state();
    let generation = app.session.get(app.views[0].id).unwrap().edit_generation();
    assert_eq!(app.views[0].edit_lines.get(&0).map(|(g, _)| *g), Some(generation), "the boxes follow the edit while input is blocked");
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
    assert!((v.zoom - windowed).abs() < 1e-4, "the large spread fits as before");
    v.current = 1;
    v.fit_zoom(&info);
    assert_eq!(v.zoom, small, "page 2 is in the first spread");
    // With a cover page: [0] alone, then [1, 2] (the large page decides), then [3].
    v.cover = true;
    v.current = 0;
    v.fit_zoom(&info);
    assert_eq!(v.zoom, small, "the cover page alone");
    v.current = 1;
    v.fit_zoom(&info);
    assert!((v.zoom - windowed).abs() < 1e-4, "page 2 shares a spread with the large page 3");
    // Scrolling while in full screen fits the document, as scrolling does in the window.
    (v.continuous, v.current) = (true, 0);
    v.fit_zoom(&info);
    assert!((v.zoom - windowed).abs() < 1e-4, "continuous full screen keeps one zoom");
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
