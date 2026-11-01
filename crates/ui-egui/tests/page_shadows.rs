//! Decorative page shadows are app preferences, never PDF edits.

use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_ui_egui::PdfCraftApp;

#[test]
fn viewer_shadows_follow_the_preference_without_moving_pages() {
    for theme in ["light", "dark"] {
        for layout in ["single", "two-page", "continuous", "two-up"] {
            let mut h = harness(theme);
            h.state_mut().set_option("layout", layout).unwrap();
            h.run_steps(3);
            let rectangles = h.state().views[0].visible_page_rects().to_vec();
            let original = shadows(&h, false);
            assert!(!original.is_empty(), "{theme} {layout}");
            h.state_mut().set_option("show_page_edge_shadows", "false").unwrap();
            h.run_steps(2);
            assert!(shadows(&h, false).is_empty(), "{theme} {layout}");
            assert_eq!(h.state().views[0].visible_page_rects(), rectangles);
            h.state_mut().set_option("zoom", "125").unwrap();
            h.state_mut().set_option("page", "2").unwrap();
            h.run_steps(3);
            assert!(shadows(&h, false).is_empty());
            h.state_mut().set_option("show_page_edge_shadows", "true").unwrap();
            h.run_steps(2);
            assert!(!shadows(&h, false).is_empty());
        }
    }
}

#[test]
fn viewport_fades_follow_the_preference_and_keep_other_scroll_areas_styled() {
    for theme in ["light", "dark"] {
        for organize in [false, true] {
            let mut h = harness(theme);
            if organize {
                h.state_mut().set_option("organize", "on").unwrap();
                // Make the three-page grid overflow vertically.
                h.state_mut().set_option("grid-zoom", "300").unwrap();
            }
            h.run_steps(4);
            let original_style = h.ctx.global_style().spacing.scroll.fade;
            assert!(original_style.strength > 0.0);
            assert!(!viewport_fades(&h).is_empty(), "bottom fade at start: {theme}, organize={organize}");
            scroll_down(&mut h, 100.0);
            let original = viewport_fades(&h);
            let viewport = h.state().views[0].viewport_rect();
            assert!(original.iter().any(|r| (r.top() - viewport.top()).abs() < 2.0), "top fade: {original:?}");
            assert!(original.iter().any(|r| (r.bottom() - viewport.bottom()).abs() < 2.0), "bottom fade: {original:?}");
            let pages = h.state().views[0].visible_page_rects().to_vec();
            h.state_mut().set_option("show_page_edge_shadows", "false").unwrap();
            h.run_steps(2);
            assert!(viewport_fades(&h).is_empty(), "viewport fades remain: {theme}, organize={organize}");
            assert_eq!(h.state().views[0].visible_page_rects(), pages);
            assert_eq!(h.ctx.global_style().spacing.scroll.fade, original_style);
            h.state_mut().set_option("show_page_edge_shadows", "true").unwrap();
            h.run_steps(2);
            assert_eq!(viewport_fades(&h), original);
            scroll_down(&mut h, 100_000.0);
            let at_end = viewport_fades(&h);
            assert!(at_end.iter().any(|r| (r.top() - viewport.top()).abs() < 2.0));
            assert!(!at_end.iter().any(|r| (r.bottom() - viewport.bottom()).abs() < 2.0), "no bottom fade at the end");
        }
    }
}

#[test]
fn horizontal_viewport_fades_also_follow_the_preference() {
    let mut h = harness("dark");
    h.state_mut().set_option("zoom", "400").unwrap();
    h.run_steps(4);
    let viewport = h.state().views[0].viewport_rect();
    let horizontal: Vec<_> = viewport_fades(&h).into_iter().filter(|r| r.width() <= 21.0).collect();
    assert!(horizontal.iter().any(|r| (r.left() - viewport.left()).abs() < 2.0), "left fade: {horizontal:?}");
    assert!(horizontal.iter().any(|r| (r.right() - viewport.right()).abs() < 2.0), "right fade: {horizontal:?}");
    let pages = h.state().views[0].visible_page_rects().to_vec();
    h.state_mut().set_option("show_page_edge_shadows", "false").unwrap();
    h.run_steps(2);
    assert!(viewport_fades(&h).is_empty());
    assert_eq!(h.state().views[0].visible_page_rects(), pages);
}

fn scroll_down(h: &mut Harness<'static, PdfCraftApp>, points: f32) {
    h.hover_at(h.state().views[0].viewport_rect().center());
    h.run_steps(1);
    h.event(egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -points),
        phase: egui::TouchPhase::Move,
        modifiers: egui::Modifiers::NONE,
        source: egui::MouseWheelSource::Unknown,
    });
    h.run_steps(12);
}

/// Inspect the actual egui painter output: untextured four-vertex gradients
/// with a transparent edge, spanning one dimension of the document viewport.
fn viewport_fades(h: &Harness<'static, PdfCraftApp>) -> Vec<egui::Rect> {
    let viewport = h.state().views[0].viewport_rect();
    h.output()
        .shapes
        .iter()
        .filter_map(|s| {
            let egui::Shape::Mesh(mesh) = &s.shape else { return None };
            if mesh.texture_id != egui::TextureId::default() || mesh.vertices.len() != 4 || mesh.indices.len() != 6 {
                return None;
            }
            let rect = mesh.calc_bounds();
            let transparent = mesh.vertices.iter().filter(|v| v.color == egui::Color32::TRANSPARENT).count();
            (transparent == 2
                && (((rect.width() - viewport.width()).abs() < 2.0 && rect.height() <= 21.0)
                    || ((rect.height() - viewport.height()).abs() < 2.0 && rect.width() <= 21.0))
                && rect.intersects(viewport))
            .then_some(rect)
        })
        .collect()
}

#[test]
fn organize_shadows_follow_the_preference() {
    for theme in ["light", "dark"] {
        let mut h = harness(theme);
        h.state_mut().set_option("organize", "on").unwrap();
        h.run_steps(3);
        let original = shadows(&h, true);
        assert_eq!(original.len(), 3);
        h.state_mut().set_option("show_page_edge_shadows", "false").unwrap();
        h.run_steps(2);
        assert!(shadows(&h, true).is_empty());
        h.state_mut().set_option("show_page_edge_shadows", "true").unwrap();
        h.run_steps(2);
        assert_eq!(shadows(&h, true), original);
    }
}

#[test]
fn checkbox_and_multiple_documents_do_not_edit_pdf_state() {
    let mut h = harness("light");
    h.state_mut().open_bytes("second.pdf", None, pdf()).unwrap();
    let ids: Vec<_> = h.state().views.iter().map(|v| v.id).collect();
    h.state_mut().session.apply(ids[0], pdfcraft_engine::Edit::SetInfo { key: "Title".into(), value: "Keep existing edits".into() }).unwrap();
    assert!(h.state().session.get(ids[0]).unwrap().dirty);
    assert!(h.state().session.get(ids[0]).unwrap().can_undo().is_some());
    let saved: Vec<_> = ids.iter().map(|id| h.state_mut().session.save_bytes(*id).unwrap()).collect();
    let before: Vec<_> = h
        .state()
        .views
        .iter()
        .map(|v| {
            let d = h.state().session.get(v.id).unwrap();
            (
                d.bytes.clone(),
                d.dirty,
                d.can_undo().map(str::to_owned),
                d.can_redo().map(str::to_owned),
                d.info.pages.iter().map(|p| (p.width, p.height, p.rotation)).collect::<Vec<_>>(),
            )
        })
        .collect();
    h.state_mut().set_option("dialog", "preferences").unwrap();
    h.run_steps(3);
    let checkbox = h.get_by_label("Show Page Edge Shadows");
    checkbox.click();
    h.run_steps(2);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&h.state().persist()).unwrap()["show_page_edge_shadows"], false);
    h.get_by_label("Show Page Edge Shadows").focus();
    h.key_press(egui::Key::Space);
    h.run_steps(2);
    assert_eq!(serde_json::from_str::<serde_json::Value>(&h.state().persist()).unwrap()["show_page_edge_shadows"], true);
    let dialog_shadow = h.ctx.global_style().visuals.window_shadow;
    assert_ne!(dialog_shadow, egui::Shadow::NONE);
    h.state_mut().set_option("show_page_edge_shadows", "false").unwrap();
    h.run_steps(2);
    assert_eq!(h.ctx.global_style().visuals.window_shadow, dialog_shadow);
    h.state_mut().set_option("dialog", "none").unwrap();
    for tab in [0, 1] {
        h.state_mut().active = Some(tab);
        h.run_steps(2);
        assert!(shadows(&h, false).is_empty());
        let d = h.state().session.get(h.state().views[tab].id).unwrap();
        assert_eq!(
            (&d.bytes, d.dirty, d.can_undo(), d.can_redo()),
            (&before[tab].0, before[tab].1, before[tab].2.as_deref(), before[tab].3.as_deref())
        );
        assert_eq!(d.info.pages.iter().map(|p| (p.width, p.height, p.rotation)).collect::<Vec<_>>(), before[tab].4);
    }
    for (id, bytes) in ids.iter().zip(&saved) {
        assert_eq!(h.state_mut().session.save_bytes(*id).unwrap(), *bytes);
    }
    h.state_mut().open_bytes("new.pdf", None, pdf()).unwrap();
    h.run_steps(2);
    assert!(shadows(&h, false).is_empty());
}

#[test]
fn rendered_page_content_is_identical_with_and_without_decoration() {
    let mut h = harness("dark");
    h.state_mut().set_option("panel", "pages").unwrap();
    h.state_mut().set_option("zoom", "175").unwrap();
    h.run_steps(3);
    scroll_down(&mut h, 20.0);
    for _ in 0..400 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!h.state().render_pending(), "the actual PDF raster must be ready");
    let rect = h.state().views[0].page_screen_rect(0).unwrap();
    let fades = viewport_fades(&h);
    assert_eq!(fades.iter().filter(|r| r.height() <= 21.0).count(), 2, "the screenshot comparison must contain both vertical fades");
    let enabled = h.render().unwrap();
    h.state_mut().set_option("show_page_edge_shadows", "false").unwrap();
    h.run_steps(2);
    let disabled = h.render().unwrap();
    assert_eq!(h.state().views[0].page_screen_rect(0), Some(rect));
    let viewport = h.state().views[0].viewport_rect();
    assert!(viewport_fades(&h).is_empty());
    // Only the painter's actual decorative fade strips may change over the page.
    // All remaining artwork, annotation and form pixels must be identical.
    for (_, page) in h.state().views[0].visible_page_rects() {
        let visible = page.intersect(viewport).shrink(2.0);
        if !visible.is_positive() {
            continue;
        }
        for y in visible.top().ceil() as u32..visible.bottom().floor() as u32 {
            for x in visible.left().ceil() as u32..visible.right().floor() as u32 {
                if !fades.iter().any(|fade| fade.expand(1.0).contains(egui::pos2(x as f32, y as f32))) {
                    assert_eq!(enabled.get_pixel(x, y), disabled.get_pixel(x, y), "page content at {x},{y}");
                }
            }
        }
    }
    // The top overlaps the first page's blue artwork; the bottom overlaps the
    // blank second page. Sample clear of the unchanged scrollbar at the edge.
    let x = rect.intersect(viewport).center().x as u32;
    for (y, expected) in [(viewport.top() + 8.0, [204, 230, 255, 255]), (viewport.bottom() - 10.0, [255, 255, 255, 255])] {
        assert_eq!(disabled.get_pixel(x, y as u32).0, expected, "unobscured PDF at y={y}");
        assert_ne!(enabled.get_pixel(x, y as u32).0, expected, "fade overlays the PDF at y={y}");
    }
    // The Pages sidebar, including its thumbnails and selection borders, is unaffected.
    for y in 0..enabled.height() {
        for x in (0..viewport.left().floor() as u32).chain(viewport.right().ceil() as u32..enabled.width()) {
            assert_eq!(enabled.get_pixel(x, y), disabled.get_pixel(x, y), "sidebar at {x},{y}");
        }
    }
    assert_ne!(enabled.as_raw(), disabled.as_raw(), "the decoration really changes the image");
    if let Ok(directory) = std::env::var("PDFCRAFT_SHADOW_SCREENSHOTS") {
        enabled.save(format!("{directory}/page-shadows-enabled.png")).unwrap();
        disabled.save(format!("{directory}/page-shadows-disabled.png")).unwrap();
    }
}

fn harness(theme: &str) -> Harness<'static, PdfCraftApp> {
    let theme = theme.to_string();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_| {
        let mut app = PdfCraftApp::new();
        // Save comparisons must not depend on crossing a /ModDate second boundary.
        app.session = app.session.with_clock(|| 1_700_000_000);
        app.set_option("language", "en").unwrap();
        app.set_option("theme", &theme).unwrap();
        app.open_bytes("pages.pdf", None, pdf()).unwrap();
        app.set_option("zoom", "100").unwrap();
        app
    });
    h.run_steps(3);
    h
}

fn shadows(h: &Harness<'static, PdfCraftApp>, grid: bool) -> Vec<egui::Rect> {
    let color = pdfcraft_ui_egui::theme::Tokens::for_kind(h.state().theme).page_shadow;
    fn collect(shape: &egui::Shape, color: egui::Color32, grid: bool, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    collect(s, color, grid, out);
                }
            }
            egui::Shape::Rect(r)
                if r.fill == color
                    && r.rect.width() > 20.0
                    && r.rect.height() > 20.0
                    && if grid {
                        r.blur_width == 0.0 && r.corner_radius == egui::CornerRadius::same(1)
                    } else {
                        r.blur_width == 14.0 && r.corner_radius == egui::CornerRadius::ZERO
                    } =>
            {
                out.push(r.rect)
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for s in &h.output().shapes {
        collect(&s.shape, color, grid, &mut out);
    }
    out
}

/// Synthetic pages include colored artwork, an annotation and a form field.
fn pdf() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [7 0 R] >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 300 400] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Contents 8 0 R /Annots [6 0 R 7 0 R] >>".to_string(),
        "<< /Type /Page /Parent 2 0 R >>".to_string(),
        "<< /Type /Page /Parent 2 0 R >>".to_string(),
        "<< /Type /Annot /Subtype /Text /Rect [20 20 40 40] /Contents (Note) >>".to_string(),
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T (field) /V (value) /Rect [50 50 100 70] /P 3 0 R >>".to_string(),
        {
            let body = "0.8 0.9 1 rg 0 0 300 400 re f 0.3 g 10 10 280 8 re f";
            format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len())
        },
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

#[test]
fn page_shadow_preference_round_trips_and_ignores_invalid_settings() {
    let mut app = PdfCraftApp::new();
    let enabled = |app: &PdfCraftApp| serde_json::from_str::<serde_json::Value>(&app.persist()).unwrap()["show_page_edge_shadows"].clone();
    assert_eq!(enabled(&app), true);
    app.restore("{}");
    assert_eq!(enabled(&app), true);
    for value in [false, true] {
        app.set_option("show_page_edge_shadows", if value { "true" } else { "false" }).unwrap();
        let mut restored = PdfCraftApp::new();
        restored.restore(&app.persist());
        assert_eq!(enabled(&restored), value);
    }
    for invalid in ["null", "0", "\"false\"", "[]", "{}"] {
        app.restore(&format!("{{\"show_page_edge_shadows\":{invalid}}}"));
        assert_eq!(enabled(&app), true);
    }
    app.restore("not json");
    assert_eq!(enabled(&app), true);
    assert!(app.set_option("show_page_edge_shadows", "maybe").is_err());
    assert_eq!(enabled(&app), true);
}
