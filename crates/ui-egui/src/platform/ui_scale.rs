//! Preferences ▸ Interface size: how large the interface is drawn. `Auto` keeps the display's own
//! scale and adds the desktop's text scaling (GNOME's "Large Text" or text scaling factor, which
//! the desktop app reads at launch), so PdfCraft's labels match the other apps' instead of staying
//! small and showing their pixels on a 1× display. A chosen size is absolute, as PhotoCraft's
//! Interface ▸ UI Scale is: "100%" is one screen pixel per point whatever scale the display
//! reports, so a system that reports 200% on a 1080p screen doesn't make "100%" draw at 200%.
//!
//! The scale is egui's zoom factor: `pixels_per_point` grows with it, so page rasters, which are
//! rendered at `pixels_per_point` (`canvas::render_scale`), stay one texel per screen pixel.

/// The smallest and largest interface sizes, as factors.
pub const MIN: f32 = 0.5;
pub const MAX: f32 = 3.0;

/// The sizes the Preferences menu offers, in percent.
pub const PRESETS: [u16; 12] = [75, 80, 85, 90, 95, 100, 125, 150, 175, 200, 250, 300];

/// A factor from settings, the control channel or the system: finite and within [`MIN`]..=[`MAX`],
/// or `None`.
pub fn valid(factor: f64) -> Option<f32> {
    let factor = factor as f32;
    (factor.is_finite() && (MIN..=MAX).contains(&factor)).then_some(factor)
}

/// egui's zoom factor on a display whose own scale is `native`: the system's text scale for
/// `Auto` (`None`), or the chosen size with the display's scale divided back out.
pub fn zoom(preference: Option<f32>, system: f32, native: f32) -> f32 {
    match preference {
        None => valid(f64::from(system)).unwrap_or(1.0),
        Some(size) => size / usable(native),
    }
}

/// The size the interface is drawn at (screen pixels per point) for [`zoom`]'s arguments.
pub fn drawn(preference: Option<f32>, system: f32, native: f32) -> f32 {
    usable(native) * zoom(preference, system, native)
}

/// The display's scale, or 1.0 when egui doesn't know it (or reports nonsense).
fn usable(native: f32) -> f32 {
    if native.is_finite() && native > 0.0 { native } else { 1.0 }
}

/// Parse a control-channel value: `auto`, or a percentage such as `125` or `125%`.
pub fn parse(value: &str) -> Result<Option<f32>, String> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    let percent: f64 = value.trim_end_matches('%').trim().parse().map_err(|_| format!("ui-scale must be auto or a percentage, not {value}"))?;
    valid(percent / 100.0).map(Some).ok_or_else(|| format!("ui-scale must be between {}% and {}%", (MIN * 100.0).round(), (MAX * 100.0).round()))
}

/// The percentage a factor shows as.
pub fn percent(factor: f32) -> u32 {
    // In MIN..=MAX (every factor here went through `valid`, or is 1.0), so the cast can't overflow.
    (factor * 100.0).round().clamp(0.0, 1000.0) as u32
}

/// Draw at `factor` from the next frame on, when it differs from the factor last applied
/// (`applied`), so a zoom set elsewhere (a test harness's `pixels_per_point`) isn't undone every
/// frame. egui's own ⌘+/⌘−/⌘0 interface zoom stays off: those keys zoom the page, and the
/// preference alone decides the interface size.
pub fn sync(ctx: &egui::Context, factor: f32, applied: &mut Option<f32>) {
    if applied.is_none() {
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
    }
    if *applied != Some(factor) {
        ctx.set_zoom_factor(factor);
        *applied = Some(factor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_follows_a_usable_system_scale_only() {
        assert_eq!(zoom(None, 1.25, 1.0), 1.25);
        assert_eq!(drawn(None, 1.25, 2.0), 2.5, "Auto keeps the display's scale");
        for system in [f32::NAN, f32::INFINITY, -1.0, 0.0, 0.1, 40.0] {
            assert_eq!(zoom(None, system, 1.0), 1.0, "{system}");
        }
    }

    #[test]
    fn a_chosen_size_is_absolute_whatever_the_display_reports() {
        for native in [1.0, 1.25, 2.0, 3.0] {
            assert_eq!(drawn(Some(1.0), 1.25, native), 1.0, "100% on a {native}× display");
            assert_eq!(drawn(Some(1.5), 1.0, native), 1.5, "150% on a {native}× display");
        }
        assert_eq!(zoom(Some(1.0), 1.0, 2.0), 0.5);
        for native in [f32::NAN, f32::INFINITY, 0.0, -2.0] {
            assert_eq!(drawn(Some(1.25), 1.0, native), 1.25, "{native}");
        }
    }

    #[test]
    fn control_values_parse_or_say_what_is_allowed() {
        assert_eq!(parse("auto"), Ok(None));
        assert_eq!(parse(" AUTO "), Ok(None));
        assert_eq!(parse("125"), Ok(Some(1.25)));
        assert_eq!(parse("150%"), Ok(Some(1.5)));
        for bad in ["", "big", "10", "1000", "NaN", "inf", "-125"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        for p in PRESETS {
            assert_eq!(parse(&p.to_string()).map(|f| f.map(percent)), Ok(Some(u32::from(p))));
        }
    }

    #[test]
    fn sync_sets_the_zoom_and_turns_off_keyboard_zoom() {
        let ctx = egui::Context::default();
        let mut applied = None;
        sync(&ctx, 1.25, &mut applied);
        assert_eq!(applied, Some(1.25));
        // A bare context's first frame uploads the font atlas; nothing paints it here.
        ctx.run_ui(egui::RawInput::default(), |_| {}).textures_delta.clear();
        assert_eq!(ctx.zoom_factor(), 1.25);
        assert!(!ctx.options(|o| o.zoom_with_keyboard));
    }

    #[test]
    fn the_app_draws_at_the_system_scale_until_a_size_is_chosen() {
        // A display that reports 200% (#456: a 1080p screen whose system reports 2×).
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1000.0, 700.0)).with_pixels_per_point(2.0).build_eframe(|_cc| {
            let mut app = crate::PdfCraftApp::new();
            app.system_text_scale = 1.25;
            app
        });
        h.run_steps(3);
        assert_eq!(h.ctx.pixels_per_point(), 2.5, "Auto follows the display's scale and the system's text scale");
        h.state_mut().set_option("ui-scale", "100%").expect("a valid size");
        h.run_steps(3);
        assert_eq!(h.ctx.pixels_per_point(), 1.0, "100% is one screen pixel per point");
        h.state_mut().set_option("ui-scale", "150%").expect("a valid size");
        h.run_steps(3);
        assert_eq!(h.ctx.pixels_per_point(), 1.5);
        assert!(h.state_mut().set_option("ui-scale", "5000").is_err());
        // The chosen size survives a restart; a hostile one in the settings follows the system.
        let saved = h.state().persist();
        let mut restored = crate::PdfCraftApp::new();
        restored.restore(&saved);
        assert_eq!(restored.ui_scale, Some(1.5));
        restored.restore(&saved.replace("\"ui_scale\":1.5", "\"ui_scale\":1e30"));
        assert_eq!(restored.ui_scale, None);
        h.state_mut().set_option("ui-scale", "auto").expect("auto");
        h.run_steps(3);
        assert_eq!(h.ctx.pixels_per_point(), 2.5);
    }
}
