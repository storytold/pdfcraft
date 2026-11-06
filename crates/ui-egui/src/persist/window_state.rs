//! The window's saved size and place across starts. eframe stores them just before
//! `App::save`, from the window as it is at that moment. A minimized window has neither: Windows
//! reports it as 0 × 0 at (-32000, -32000), and the next start then opened at egui's 64-point
//! floor in the screen's corner. So while the window is minimized the last settings it was shown
//! with are put back, and a window that opens at that floor is given a usable size.
//!
//! A window that opens larger than its monitor (the 1440 × 920 first-start size on a 1366 × 768
//! laptop, or 1536 × 864 points at 125 % scaling, or a size saved on a bigger monitor) runs off
//! the screen, and with it the foot of every dialog and drop-down list fitted to the window. So
//! such a window is made to fit and, on the main monitor, centred. A window that starts maximized
//! keeps that size to return to, so it is checked again when it comes back from maximized or full
//! screen.

/// eframe's storage key for the window settings.
const WINDOW_KEY: &str = "window";
/// The size egui opens a stored size below (its floor for "ridiculously small" windows), in points.
const EGUI_FLOOR: f32 = 64.0;
/// The size a window that opened at [`EGUI_FLOOR`] is given (apps/pdfcraft opens a first start at
/// this size too), at most nine tenths of the monitor.
const USABLE: egui::Vec2 = egui::vec2(1440.0, 920.0);
/// The share of the monitor a window that does not fit is given, frame included. egui does not
/// report a taskbar or dock, so this only makes room for one: the window is centred, so each edge
/// gets half of what is left, and a 48-point taskbar on a 768-point monitor still covers the last
/// few points of the window.
const FIT: f32 = 0.9;
/// How far past its monitor a window may reach before it is fitted, in points: a window snapped to
/// the full height of a screen with a hidden taskbar reaches past by its invisible resize borders.
const SLACK: f32 = 16.0;

/// The inner size, and where it can be told, the outer position, that fits a window whose outer
/// rectangle is `outer` and inner `inner` on a monitor of `monitor` points; `None` when it fits.
/// egui knows the monitor's size but not where it is on the desktop, so the window is only moved
/// when its corner is within that size of the desktop's origin, taken to mean the main monitor. A
/// wider monitor beside the main one, or a taller one below it, can pass that test, and a window
/// too big for it is then moved to the main monitor; egui's data cannot tell the two apart.
fn fit_to_monitor(outer: egui::Rect, inner: egui::Rect, monitor: egui::Vec2) -> Option<(egui::Vec2, Option<egui::Pos2>)> {
    let finite = |v: egui::Vec2| v.x.is_finite() && v.y.is_finite();
    if !finite(monitor) || !finite(outer.size()) || !finite(inner.size()) || !finite(outer.min.to_vec2()) {
        return None;
    }
    if monitor.x <= 0.0 || monitor.y <= 0.0 {
        return None;
    }
    if outer.width() <= monitor.x + SLACK && outer.height() <= monitor.y + SLACK {
        return None;
    }
    let frame = (outer.size() - inner.size()).max(egui::Vec2::ZERO);
    let room = monitor * FIT - frame;
    // Never grow it, and never ask for less than a point (the app's minimum size still applies).
    let size = inner.size().min(room).max(egui::Vec2::splat(1.0));
    // Windows' outer rectangle includes its invisible resize borders, so a window against the
    // monitor's left or top edge has its corner a few points past the desktop's origin.
    let on_main = outer.min.x >= -SLACK && outer.min.y >= -SLACK && outer.min.x < monitor.x && outer.min.y < monitor.y;
    let place = on_main.then(|| ((monitor - (size + frame)) * 0.5).max(egui::Vec2::ZERO).to_pos2());
    Some((size, place))
}

#[derive(Debug, Default)]
pub struct WindowState {
    /// The settings eframe last stored for a shown (not minimized) window.
    shown: Option<String>,
    /// The size has been checked ([`Self::repair`]): once at start, and again on coming back from
    /// maximized or full screen.
    checked: bool,
    /// The window was maximized or full screen when last shown.
    was_filling: bool,
}

impl WindowState {
    /// The settings the window started with: what to put back if it is minimized before eframe
    /// stores a shown one.
    pub fn started_with(storage: Option<&dyn eframe::Storage>) -> Self {
        Self { shown: storage.and_then(|s| s.get_string(WINDOW_KEY)), ..Self::default() }
    }

    /// After eframe has stored the window: remember it while the window is shown, and while it
    /// is minimized put the last shown settings back (or none, so the next start opens at the
    /// default size).
    pub fn keep(&mut self, storage: &mut dyn eframe::Storage, minimized: bool) {
        if !minimized {
            self.shown = storage.get_string(WINDOW_KEY);
        } else if let Some(shown) = &self.shown {
            storage.set_string(WINDOW_KEY, shown.clone());
        } else {
            storage.remove_string(WINDOW_KEY);
        }
    }

    /// Once the window's size is known: a shown window at egui's floor in both directions came
    /// from settings stored while it was minimized (before this fix), so give it a usable size. A
    /// shown window larger than its monitor (not maximized or full screen, which fit by
    /// definition) is fitted to it ([`fit_to_monitor`]), at start and on coming back from either.
    ///
    /// Call this from a drawn pass (`App::ui`), not `App::logic`. On Wayland a resize the app asks
    /// for brings no resize event, and eframe only tells the painter about one after a drawn pass.
    /// From a logic-only pass (a compositor holding back frame callbacks) the window would shrink
    /// and keep drawing at its old size. A logic-only pass also leaves the ui input alone, so the
    /// size it could read is the one from the last drawn pass.
    pub fn repair(&mut self, ctx: &egui::Context) {
        // Wayland gives a window no position, and egui has no inner rectangle without one. The size
        // the window reports this frame stands in for it; egui's own rectangle would not do, as it
        // keeps a default size when a frame reports none.
        let (rect, outer, minimized, maximized, fullscreen, monitor) = ctx.input(|i| {
            let v = i.viewport();
            (v.inner_rect.or(i.raw.screen_rect), v.outer_rect, v.minimized, v.maximized, v.fullscreen, v.monitor_size)
        });
        let Some(rect) = rect else { return };
        if minimized == Some(true) {
            return;
        }
        let filling = maximized == Some(true) || fullscreen == Some(true);
        if self.was_filling && !filling {
            self.checked = false;
        }
        self.was_filling = filling;
        if self.checked {
            return;
        }
        self.checked = true;
        if rect.width() <= EGUI_FLOOR + 0.5 && rect.height() <= EGUI_FLOOR + 0.5 {
            // The stored settings are the broken ones: never put them back.
            self.shown = None;
            let size = monitor.map_or(USABLE, |m| USABLE.min(m * 0.9));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        } else if !filling {
            // The monitor can be unknown for a while (Wayland, a surface on no output yet): a
            // window that cannot be measured yet is checked again on the next frame.
            let Some(monitor) = monitor else {
                self.checked = false;
                return;
            };
            // Without the outer rectangle (Wayland) the size is checked and the window not moved.
            if let Some((size, place)) = fit_to_monitor(outer.unwrap_or(rect), rect, monitor) {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                if let Some(place) = place.filter(|_| outer.is_some()) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(place));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::Storage as _;

    #[derive(Default)]
    struct Store(std::collections::HashMap<String, String>);

    impl eframe::Storage for Store {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    const SHOWN: &str = "(outer_position_pixels:Some((x:200.0,y:200.0)),inner_size_points:Some((x:1320.0,y:940.0)))";
    const MINIMIZED: &str = "(outer_position_pixels:Some((x:-32000.0,y:-32000.0)),inner_size_points:Some((x:0.0,y:0.0)))";

    /// Closing PdfCraft while it was minimized stored a 0 × 0 window off screen.
    #[test]
    fn a_minimized_window_keeps_the_last_shown_settings() {
        let mut store = Store::default();
        store.set_string(WINDOW_KEY, SHOWN.into());
        let mut state = WindowState::started_with(Some(&store));
        // eframe stores the minimized window, then the app saves.
        store.set_string(WINDOW_KEY, MINIMIZED.into());
        state.keep(&mut store, true);
        assert_eq!(store.get_string(WINDOW_KEY).as_deref(), Some(SHOWN));
        // Shown again and resized: that is what is kept from then on.
        let moved = SHOWN.replace("200.0", "300.0");
        store.set_string(WINDOW_KEY, moved.clone());
        state.keep(&mut store, false);
        store.set_string(WINDOW_KEY, MINIMIZED.into());
        state.keep(&mut store, true);
        assert_eq!(store.get_string(WINDOW_KEY), Some(moved));
    }

    #[test]
    fn with_nothing_shown_yet_a_minimized_save_stores_no_window() {
        let mut store = Store::default();
        let mut state = WindowState::started_with(Some(&store));
        store.set_string(WINDOW_KEY, MINIMIZED.into());
        state.keep(&mut store, true);
        assert_eq!(store.get_string(WINDOW_KEY), None, "the next start opens at the default size");
    }

    fn repaired(size: egui::Vec2, minimized: Option<bool>) -> Vec<egui::ViewportCommand> {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        let info = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        info.inner_rect = Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size));
        info.minimized = minimized;
        info.monitor_size = Some(egui::vec2(2674.0, 1504.0));
        let mut state = WindowState { shown: Some(MINIMIZED.into()), ..WindowState::default() };
        let out = ctx.run_logic(&input, |ctx| state.repair(ctx));
        let commands = out.viewport_commands.get(&egui::ViewportId::ROOT).cloned().unwrap_or_default();
        // Repaired: the settings it started with are not put back by a minimized close.
        assert_eq!(state.shown.is_none(), !commands.is_empty());
        commands
    }

    #[test]
    fn a_window_at_eguis_floor_gets_a_usable_size() {
        assert_eq!(repaired(egui::vec2(64.0, 64.0), Some(false)), [egui::ViewportCommand::InnerSize(USABLE)]);
        assert!(repaired(egui::vec2(820.0, 520.0), Some(false)).is_empty(), "a small window someone chose stays");
        assert!(repaired(egui::vec2(64.0, 64.0), Some(true)).is_empty(), "not while minimized");
    }

    /// A 1536 × 864-point monitor: 1920 × 1080 at 125 % scaling.
    const LAPTOP: egui::Vec2 = egui::vec2(1536.0, 864.0);
    /// A frame of 8-point side borders and a 32-point title bar over an 8-point foot.
    const FRAME: egui::Vec2 = egui::vec2(16.0, 48.0);

    /// The outer and inner rectangles of a window whose outer corner is at `at`.
    fn window(at: egui::Pos2, inner: egui::Vec2) -> (egui::Rect, egui::Rect) {
        let outer = egui::Rect::from_min_size(at, inner + FRAME);
        (outer, egui::Rect::from_min_size(at + egui::vec2(8.0, 40.0), inner))
    }

    /// The first-start 1440 × 920 window opened at (128, 128) on a 125 % laptop screen ran off
    /// its foot, and the Scan & OCR language list with it.
    #[test]
    fn a_window_taller_than_its_monitor_is_fitted_and_centred() {
        let (outer, inner) = window(egui::pos2(128.0, 128.0), USABLE);
        let (size, place) = fit_to_monitor(outer, inner, LAPTOP).expect("it does not fit");
        let fitted = size + FRAME;
        assert!(fitted.x <= LAPTOP.x * FIT + 0.01 && fitted.y <= LAPTOP.y * FIT + 0.01, "{fitted:?}");
        assert_eq!(size.x, USABLE.x.min(LAPTOP.x * FIT - FRAME.x), "never wider than it was");
        let place = place.expect("on the main monitor");
        let end = place + fitted;
        assert!(place.x >= 0.0 && place.y >= 0.0 && end.x <= LAPTOP.x && end.y <= LAPTOP.y, "{place:?} to {end:?}");
        assert!((place.y - (LAPTOP.y - end.y)).abs() < 0.01, "centred");
    }

    #[test]
    fn a_window_that_fits_is_left_alone() {
        let (outer, inner) = window(egui::pos2(128.0, 128.0), USABLE);
        assert_eq!(fit_to_monitor(outer, inner, egui::vec2(2674.0, 1504.0)), None);
        // Snapped to a full-height screen, its invisible borders reach past the monitor.
        let snapped = egui::Rect::from_min_size(egui::pos2(-8.0, -8.0), LAPTOP + egui::vec2(16.0, 16.0));
        assert_eq!(fit_to_monitor(snapped, snapped.shrink(8.0), LAPTOP), None);
    }

    /// Where a second monitor sits is unknown, so a window with its corner past the monitor's size
    /// is shrunk where it is.
    #[test]
    fn a_window_off_the_main_monitor_is_only_shrunk() {
        let (outer, inner) = window(egui::pos2(1700.0, 100.0), USABLE);
        let (size, place) = fit_to_monitor(outer, inner, LAPTOP).expect("it does not fit");
        assert!(size.y + FRAME.y <= LAPTOP.y * FIT + 0.01);
        assert_eq!(place, None);
        let (outer, inner) = window(egui::pos2(-1600.0, 0.0), USABLE);
        assert_eq!(fit_to_monitor(outer, inner, LAPTOP).map(|f| f.1), Some(None));
    }

    /// Windows' outer rectangle includes its invisible resize borders: a window against the left
    /// edge of the main monitor has its corner at x = -7, and was taken for one on another monitor.
    #[test]
    fn a_window_against_the_monitors_edge_is_on_it() {
        for at in [egui::pos2(-7.0, 300.0), egui::pos2(300.0, -7.0), egui::pos2(-7.0, -7.0), egui::pos2(-SLACK, -SLACK)] {
            let (outer, inner) = window(at, USABLE);
            let (_, place) = fit_to_monitor(outer, inner, LAPTOP).expect("it does not fit");
            assert!(place.is_some(), "{at:?}");
        }
        let (outer, inner) = window(egui::pos2(-SLACK - 1.0, 300.0), USABLE);
        assert_eq!(fit_to_monitor(outer, inner, LAPTOP).map(|f| f.1), Some(None), "further past it is another monitor");
    }

    #[test]
    fn a_monitor_size_that_makes_no_sense_changes_nothing() {
        let (outer, inner) = window(egui::pos2(0.0, 0.0), USABLE);
        for monitor in [egui::Vec2::ZERO, egui::vec2(-1.0, 800.0), egui::vec2(f32::NAN, 800.0), egui::vec2(f32::INFINITY, f32::INFINITY)] {
            assert_eq!(fit_to_monitor(outer, inner, monitor), None, "{monitor:?}");
        }
        let nan = egui::Rect::from_min_size(egui::pos2(f32::NAN, 0.0), USABLE);
        assert_eq!(fit_to_monitor(nan, inner, LAPTOP), None);
    }

    fn started(outer: egui::Rect, inner: egui::Rect, maximized: bool) -> Vec<egui::ViewportCommand> {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        let info = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        info.inner_rect = Some(inner);
        info.outer_rect = Some(outer);
        info.minimized = Some(false);
        info.maximized = Some(maximized);
        info.monitor_size = Some(LAPTOP);
        let mut state = WindowState::default();
        let out = ctx.run_logic(&input, |ctx| state.repair(ctx));
        out.viewport_commands.get(&egui::ViewportId::ROOT).cloned().unwrap_or_default()
    }

    #[test]
    fn the_first_frame_fits_a_window_too_big_for_its_monitor() {
        let (outer, inner) = window(egui::pos2(128.0, 128.0), USABLE);
        let commands = started(outer, inner, false);
        let fit = fit_to_monitor(outer, inner, LAPTOP).expect("it does not fit");
        let place = fit.1.expect("on the main monitor");
        assert_eq!(commands, [egui::ViewportCommand::InnerSize(fit.0), egui::ViewportCommand::OuterPosition(place)]);
        // A maximized window reaches past the monitor by its borders, and is left alone.
        let maxed = egui::Rect::from_min_size(egui::pos2(-8.0, -8.0), LAPTOP + egui::vec2(16.0, 60.0));
        assert!(started(maxed, maxed.shrink(8.0), true).is_empty());
    }

    /// One frame of a window with these rectangles, keeping `state` between frames.
    fn frame(state: &mut WindowState, rects: (egui::Rect, egui::Rect), maximized: bool, minimized: bool) -> Vec<egui::ViewportCommand> {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        let info = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        info.outer_rect = Some(rects.0);
        info.inner_rect = Some(rects.1);
        info.minimized = Some(minimized);
        info.maximized = Some(maximized);
        info.fullscreen = Some(false);
        info.monitor_size = Some(LAPTOP);
        let out = ctx.run_logic(&input, |ctx| state.repair(ctx));
        out.viewport_commands.get(&egui::ViewportId::ROOT).cloned().unwrap_or_default()
    }

    /// With no outer rectangle (Wayland) a window cannot be placed, wherever its inner corner is.
    #[test]
    fn a_window_with_no_outer_rectangle_is_sized_and_not_moved() {
        let inner = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), USABLE);
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        let info = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        info.outer_rect = None;
        info.inner_rect = Some(inner);
        info.minimized = Some(false);
        info.maximized = Some(false);
        info.monitor_size = Some(LAPTOP);
        let mut state = WindowState::default();
        let out = ctx.run_logic(&input, |ctx| state.repair(ctx));
        let commands = out.viewport_commands.get(&egui::ViewportId::ROOT).cloned().unwrap_or_default();
        let (size, place) = fit_to_monitor(inner, inner, LAPTOP).expect("it does not fit");
        assert!(place.is_some(), "the corner alone would have it moved");
        assert_eq!(commands, [egui::ViewportCommand::InnerSize(size)]);
    }

    /// The commands from one frame of a window with no position, as on Wayland, and the given size.
    fn without_a_position(size: Option<egui::Vec2>, minimized: bool) -> Vec<egui::ViewportCommand> {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput { screen_rect: size.map(|s| egui::Rect::from_min_size(egui::Pos2::ZERO, s)), ..Default::default() };
        let info = input.viewports.entry(egui::ViewportId::ROOT).or_default();
        info.outer_rect = None;
        info.inner_rect = None;
        info.minimized = Some(minimized);
        info.maximized = Some(false);
        info.monitor_size = Some(LAPTOP);
        let mut state = WindowState::default();
        // A drawn pass, as in the app: a logic-only pass does not get the reported size.
        let mut out = ctx.run_ui(input, |ui| state.repair(ui.ctx()));
        out.textures_delta.clear();
        let is_fit = |c: &egui::ViewportCommand| matches!(c, egui::ViewportCommand::InnerSize(_) | egui::ViewportCommand::OuterPosition(_));
        out.viewport_output.get(&egui::ViewportId::ROOT).map_or_else(Vec::new, |o| o.commands.iter().filter(|c| is_fit(c)).cloned().collect())
    }

    /// Wayland gives a window no position, so egui has no inner rectangle: the window is fitted
    /// from its reported size, and not moved.
    #[test]
    fn a_window_with_no_position_is_fitted_and_not_moved() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, USABLE);
        let (size, _) = fit_to_monitor(rect, rect, LAPTOP).expect("it does not fit");
        assert_eq!(without_a_position(Some(USABLE), false), [egui::ViewportCommand::InnerSize(size)]);
        assert!(without_a_position(Some(LAPTOP), false).is_empty(), "it fits");
    }

    /// egui keeps a default-sized rectangle for a frame that reports no size, which must not be
    /// taken for the window; and a minimized window is left alone whatever it reports.
    #[test]
    fn a_window_with_no_size_or_minimized_is_left_alone() {
        assert!(without_a_position(None, false).is_empty());
        assert!(without_a_position(Some(USABLE), true).is_empty());
    }

    /// A window whose monitor is not known yet cannot be fitted, and must not lose its only check.
    #[test]
    fn a_window_is_fitted_once_its_monitor_is_known() {
        let (outer, inner) = window(egui::pos2(128.0, 128.0), USABLE);
        let run = |state: &mut WindowState, monitor: Option<egui::Vec2>| {
            let ctx = egui::Context::default();
            let mut input = egui::RawInput::default();
            let info = input.viewports.entry(egui::ViewportId::ROOT).or_default();
            info.outer_rect = Some(outer);
            info.inner_rect = Some(inner);
            info.minimized = Some(false);
            info.maximized = Some(false);
            info.monitor_size = monitor;
            let out = ctx.run_logic(&input, |ctx| state.repair(ctx));
            out.viewport_commands.get(&egui::ViewportId::ROOT).cloned().unwrap_or_default()
        };
        let mut state = WindowState::default();
        assert!(run(&mut state, None).is_empty(), "nothing to fit it to yet");
        let fit = fit_to_monitor(outer, inner, LAPTOP).expect("it does not fit");
        let place = fit.1.expect("on the main monitor");
        assert_eq!(run(&mut state, Some(LAPTOP)), [egui::ViewportCommand::InnerSize(fit.0), egui::ViewportCommand::OuterPosition(place)]);
        assert!(run(&mut state, Some(LAPTOP)).is_empty(), "and once only");
    }

    /// A window that starts maximized keeps a too-big size to return to: restored down, it ran
    /// off the screen with the start-up check long past.
    #[test]
    fn a_window_restored_down_from_maximized_is_fitted() {
        let filling = egui::Rect::from_min_size(egui::pos2(-8.0, -8.0), LAPTOP + egui::vec2(16.0, 16.0));
        let filling = (filling, filling.shrink(8.0));
        let big = window(egui::pos2(96.0, 0.0), egui::vec2(1440.0, 864.0));
        let small = window(egui::pos2(96.0, 40.0), egui::vec2(1000.0, 600.0));
        let mut state = WindowState::default();
        assert!(frame(&mut state, filling, true, false).is_empty(), "maximized at start");
        let commands = frame(&mut state, big, false, false);
        let (size, place) = fit_to_monitor(big.0, big.1, LAPTOP).expect("it does not fit");
        assert_eq!(commands, [egui::ViewportCommand::InnerSize(size), egui::ViewportCommand::OuterPosition(place.expect("main monitor"))]);
        assert!(frame(&mut state, big, false, false).is_empty(), "once for each return");
        // Maximized and back again, to a size that fits: nothing to do.
        let mut state = WindowState::default();
        assert!(frame(&mut state, small, false, false).is_empty());
        assert!(frame(&mut state, filling, true, false).is_empty());
        assert!(frame(&mut state, small, false, false).is_empty());
    }

    /// Windows reports a minimized window as not maximized: back from the taskbar, a maximized
    /// window has not come back to its normal size, and is not checked as if it had.
    #[test]
    fn minimizing_a_maximized_window_is_not_a_return_from_maximized() {
        let filling = egui::Rect::from_min_size(egui::pos2(-8.0, -8.0), LAPTOP + egui::vec2(16.0, 16.0));
        let filling = (filling, filling.shrink(8.0));
        let mut state = WindowState::default();
        assert!(frame(&mut state, filling, true, false).is_empty());
        let gone = egui::Rect::from_min_size(egui::pos2(-32000.0, -32000.0), egui::Vec2::ZERO);
        assert!(frame(&mut state, (gone, gone), false, true).is_empty());
        assert!(frame(&mut state, filling, true, false).is_empty());
        assert!(state.was_filling);
    }
}
