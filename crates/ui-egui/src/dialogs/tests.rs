use std::cell::Cell;
use std::rc::Rc;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

#[test]
fn preferences_scroll_preserves_disabled_animation() {
    for animated in [false, true] {
        let scroll = Rc::new(Cell::new(false));
        let requested = scroll.clone();
        let mut h = Harness::builder().with_size(egui::vec2(600.0, 400.0)).with_step_dt(1.0 / 60.0).build_ui(move |ui| {
            super::preferences_scroll(ui, 200.0, |ui| {
                ui.add_space(1000.0);
                let target = ui.label("Scroll target");
                if requested.replace(false) {
                    ui.scroll_to_rect(target.rect, Some(egui::Align::BOTTOM));
                }
                ui.add_space(300.0);
            });
        });
        h.ctx.all_styles_mut(|style| {
            style.scroll_animation = if animated { egui::style::ScrollAnimation::default() } else { egui::style::ScrollAnimation::none() };
        });
        h.run_steps(4);
        let before = h.get_by_label("Scroll target").rect().top();
        scroll.set(true);
        h.run_steps(4);
        let early = h.get_by_label("Scroll target").rect().top();
        h.run_steps(20);
        let settled = h.get_by_label("Scroll target").rect().top();
        assert!((before - settled).abs() > 50.0, "the target must scroll: {before} -> {settled}");
        assert_eq!((early - settled).abs() > 1.0, animated, "animated: {animated}, early: {early}, settled: {settled}");
    }
}
