//! Find goes to where the match is (#3530): after zooming in or out, every step to a match (⌘G,
//! ⇧⌘G, a click in the Search panel's results) leaves the highlighted match on screen, near the
//! middle of the window, on pages larger and smaller than the window, and also when a zoom and a
//! step land in the same frame.

use egui_kittest::Harness;
use pdfcraft_ui_egui::PdfCraftApp;

/// The word searched for, placed (x, y in PDF user space) on each page.
const NEEDLE: &str = "needle";

/// A page size, or a point on a page, in points.
type Size = (u32, u32);
type Point = (u32, u32);

/// Page sizes (points) and where on each page the needle is: the corners, the middle and the
/// edges of large pages, as well as a small page that fits the window.
fn pages() -> Vec<(Size, Vec<Point>)> {
    vec![
        ((612, 792), vec![(60, 720), (480, 60)]),
        ((1700, 2400), vec![(40, 2340), (1560, 2340), (820, 1200), (40, 40), (1560, 40)]),
        ((2400, 1200), vec![(2300, 1140), (60, 600), (1200, 60)]),
        ((612, 792), vec![(300, 400)]),
    ]
}

/// A PDF of [`pages`]: lines of filler text down every page, and the needles.
fn fixture() -> Vec<u8> {
    let pages = pages();
    let n = pages.len();
    let font = 3 + 2 * n;
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 3 + 2 * i)).collect();
    let mut objs = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string(), format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" "))];
    for (i, ((w, h), needles)) in pages.iter().enumerate() {
        let mut body = String::from("BT /F1 9 Tf 11 TL");
        // Filler text a needle search must step over; it never contains the needle.
        body.push_str(&format!(" 20 {} Td", h.saturating_sub(100)));
        for line in 0..(h.saturating_sub(200) / 11) {
            body.push_str(&format!(" (filler line {line} of the quick brown fox jumps over the lazy dog) '"));
        }
        body.push_str(" ET");
        for (x, y) in needles {
            body.push_str(&format!(" BT /F1 14 Tf {x} {y} Td ({NEEDLE}) Tj ET"));
        }
        objs.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents {} 0 R /Resources << /Font << /F1 {font} 0 R >> >> >>",
            4 + 2 * i
        ));
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string());
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

fn needles() -> usize {
    pages().iter().map(|(_, n)| n.len()).sum()
}

/// The app with the fixture open and a search for the needle that has found every match.
fn harness() -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("search.pdf", None, fixture()).expect("opens");
        app.set_option("left", "closed").unwrap();
        app
    });
    h.run_steps(4);
    settle(&mut h);
    h.state_mut().views[0].open_find();
    h.state_mut().views[0].find.as_mut().unwrap().query = NEEDLE.into();
    h.state_mut().views[0].rerun_find();
    // Find runs in the background: wait by a deadline, not a frame count.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline && h.state().views[0].find.as_ref().unwrap().matches.len() < needles() {
        h.run_steps(2);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(h.state().views[0].find.as_ref().unwrap().matches.len(), needles(), "every needle is found");
    settle(&mut h);
    h
}

/// Run frames until rendering has finished and the layout holds still.
fn settle(h: &mut Harness<'static, PdfCraftApp>) {
    for _ in 0..200 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Fail unless the current match is drawn inside the window, and, where the page lets the view
/// scroll that far, near its middle (#3530 asks for the middle, as Acrobat does).
fn assert_match_shown(h: &Harness<'static, PdfCraftApp>, what: &str) {
    let v = &h.state().views[0];
    let f = v.find.as_ref().unwrap();
    let i = f.current.unwrap_or_else(|| panic!("{what}: no current match"));
    let (page, range) = f.matches[i].clone();
    let window = v.viewport_rect();
    let at = |g: usize| v.glyph_screen_pos(page, g).unwrap_or_else(|| panic!("{what}: match {i} (page {}) is not on screen", page + 1));
    let (first, last) = (at(range.start), at(range.end - 1));
    // At the highest zooms one word is wider than the window: then its middle is what shows.
    let ends = if last.x - first.x < window.width() * 0.8 { vec![first, last] } else { vec![first.lerp(last, 0.5)] };
    for p in ends {
        assert!(window.contains(p), "{what}: match {i} (page {}) at {p:?} is outside the window {window:?} (zoom {})", page + 1, v.zoom);
    }
    let Some(page_rect) = v.page_screen_rect(page) else { return };
    let centre = first.lerp(last, 0.5);
    // Glyph centres, not the match's box: their middle is off the box's by up to half a glyph.
    let slack = 4.0 + 0.25 * (last.x - first.x).abs();
    // Off the middle only as far as the content's ends stop the scroll.
    let room_x = (page_rect.left() < window.left() - 1.0 || centre.x > window.center().x)
        && (page_rect.right() > window.right() + 1.0 || centre.x < window.center().x);
    let room_y = (page_rect.top() < window.top() - 1.0 || centre.y > window.center().y)
        && (page_rect.bottom() > window.bottom() + 1.0 || centre.y < window.center().y);
    if room_x {
        assert!((centre.x - window.center().x).abs() < slack, "{what}: match {i} at x {} is not centred in {window:?} (zoom {})", centre.x, v.zoom);
    }
    if room_y {
        assert!((centre.y - window.center().y).abs() < slack, "{what}: match {i} at y {} is not centred in {window:?} (zoom {})", centre.y, v.zoom);
    }
}

/// Step through every match forward and back at each zoom, as ⌘G and ⇧⌘G do.
#[test]
fn every_match_is_shown_after_zooming() {
    let mut h = harness();
    let n = needles();
    for zoom in [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0] {
        h.state_mut().views[0].set_zoom(zoom);
        settle(&mut h);
        for forward in [true, false] {
            for _ in 0..n {
                h.state_mut().views[0].find_step(forward);
                settle(&mut h);
                assert_match_shown(&h, &format!("zoom {zoom}, step {}", if forward { "forward" } else { "back" }));
            }
        }
    }
}

/// A click on a result in the Search panel goes to it at any zoom.
#[test]
fn search_panel_results_are_shown_after_zooming() {
    let mut h = harness();
    for zoom in [0.33, 3.0, 12.0] {
        h.state_mut().views[0].set_zoom(zoom);
        settle(&mut h);
        for i in (0..needles()).rev() {
            h.state_mut().views[0].go_to_match(i);
            settle(&mut h);
            assert_match_shown(&h, &format!("zoom {zoom}, result {i}"));
        }
    }
}

/// Zooms and steps mixed in a fixed pseudo-random order, as a reader zooms in and out between
/// searches; some steps land in the same frame as a zoom, before the zoom has been laid out.
#[test]
fn zooming_and_stepping_in_any_order_shows_the_match() {
    let mut h = harness();
    let mut seed: u32 = 0x5eed_3530;
    let mut next = |m: u32| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) % m
    };
    for round in 0..80 {
        let same_frame = next(3) == 0;
        match next(4) {
            0 => h.state_mut().views[0].zoom_step(true),
            1 => h.state_mut().views[0].zoom_step(false),
            2 => {
                let z = [0.2, 0.75, 1.5, 6.0, 10.0][next(5) as usize];
                h.state_mut().views[0].set_zoom(z);
            }
            _ => {}
        }
        if !same_frame {
            settle(&mut h);
        }
        if next(2) == 0 {
            let i = next(needles() as u32) as usize;
            h.state_mut().views[0].go_to_match(i);
        } else {
            let forward = next(2) == 0;
            h.state_mut().views[0].find_step(forward);
        }
        settle(&mut h);
        assert_match_shown(&h, &format!("round {round}{}", if same_frame { " (zoom in the same frame)" } else { "" }));
    }
}

/// ⌘/Ctrl with the wheel zooms over several frames (egui eases a notch over about a quarter of a
/// second). A step to a match while that zoom is still easing goes to the match, and the rest of
/// the zoom keeps it where it went instead of holding the point under the pointer still.
#[test]
fn a_step_during_a_wheel_zoom_keeps_the_match_on_screen() {
    let mut h = harness();
    h.state_mut().set_option("animation_effects", "on").unwrap();
    h.state_mut().views[0].set_zoom(3.0);
    settle(&mut h);
    let n = needles();
    for (i, dy) in (0..n).zip([60.0, -60.0].into_iter().cycle()) {
        // The pointer well away from the middle, where the zoom would otherwise be anchored.
        let w = h.state().views[0].viewport_rect();
        h.hover_at(w.min + egui::vec2(80.0, 80.0));
        h.run_steps(1);
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(0.0, dy),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::COMMAND,
        });
        h.run_steps(2);
        let before = h.state().views[0].zoom;
        h.state_mut().views[0].find_step(true);
        // Let the zoom finish easing, then the rendering.
        h.run_steps(60);
        settle(&mut h);
        assert!((h.state().views[0].zoom - before).abs() > 1e-3, "step {i}: the zoom was still easing when the step came");
        assert_match_shown(&h, &format!("step {i} during a wheel zoom ({dy})"));
    }
}

/// Single page and two-up, scrolling or a page (spread) at a time: each step shows its match.
#[test]
fn every_page_display_shows_the_match_after_zooming() {
    use pdfcraft_ui_egui::canvas::PageLayout;
    let mut h = harness();
    for (layout, continuous) in [(PageLayout::Single, false), (PageLayout::TwoUp, true), (PageLayout::TwoUp, false)] {
        h.state_mut().views[0].set_display(layout, continuous);
        for zoom in [0.5, 4.0] {
            h.state_mut().views[0].set_zoom(zoom);
            settle(&mut h);
            for _ in 0..needles() {
                h.state_mut().views[0].find_step(true);
                settle(&mut h);
                assert_match_shown(&h, &format!("{layout:?}, continuous {continuous}, zoom {zoom}"));
            }
        }
    }
}
