//! Viewer preferences that change the window and its interface when a document opens
//! (ISO 32000-2 §12.2, Table 147): `HideToolbar`, `HideMenubar` and `HideWindowUI` hide parts of
//! the interface for that document's tab, and `FitWindow` and `CenterWindow` size and place the
//! window around its first page.
//!
//! The hidden interface is the tab's own and is never saved: another tab shows the usual
//! interface, and the command [`TOGGLE`] (F8) brings it back, so a document can't leave the user
//! without the toolbar, menu or panels.

use egui::{Pos2, Rect, Vec2, ViewportCommand, vec2};

/// The command that shows the interface a document hid, and hides it again.
pub const TOGGLE: &str = "view.toggle_interface";

/// Which parts of the interface are hidden in a document's tab.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HiddenInterface {
    /// The mode bar's tools: the mode tabs, Save, Print, Properties and the command search, and
    /// the floating quick-action bar (`HideToolbar`).
    pub toolbar: bool,
    /// The Menu button (`HideMenubar`).
    pub menubar: bool,
    /// The right rail with its page navigation, the side panels and the document's scroll bars,
    /// "leaving only the document's contents displayed" (`HideWindowUI`).
    pub window_ui: bool,
}

impl HiddenInterface {
    pub const NONE: Self = Self { toolbar: false, menubar: false, window_ui: false };
    pub const ALL: Self = Self { toolbar: true, menubar: true, window_ui: true };

    /// What a document's viewer preferences ask to hide.
    pub fn asked_by(v: &pdfcraft_engine::InitialView) -> Self {
        Self { toolbar: v.hide_toolbar, menubar: v.hide_menubar, window_ui: v.hide_window_ui }
    }

    pub fn any(self) -> bool {
        self.toolbar || self.menubar || self.window_ui
    }

    /// The mode bar has nothing left to show.
    pub fn mode_bar(self) -> bool {
        self.toolbar && self.menubar
    }

    /// For the control channel's `ui.state`.
    pub fn to_json(self) -> serde_json::Value {
        serde_json::json!({ "toolbar": self.toolbar, "menubar": self.menubar, "window_ui": self.window_ui })
    }
}

/// A tab's interface: what its document asked to hide, and what is hidden now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TabInterface {
    pub asked: HiddenInterface,
    pub hidden: HiddenInterface,
}

impl TabInterface {
    /// The interface a document opens with: what it asks to hide, unless the user's preference
    /// is to open every document their own way.
    pub fn opening(asked: HiddenInterface, ignore_document: bool) -> Self {
        if ignore_document { Self::default() } else { Self { asked, hidden: asked } }
    }

    /// [`TOGGLE`]: show everything that is hidden; with nothing hidden, hide what the document
    /// asked to hide, or everything when it asked for nothing. Returns whether it is now hidden.
    pub fn toggle(&mut self) -> bool {
        self.hidden = if self.hidden.any() {
            HiddenInterface::NONE
        } else if self.asked.any() {
            self.asked
        } else {
            HiddenInterface::ALL
        };
        self.hidden.any()
    }
}

/// `FitWindow` and `CenterWindow`, waiting for the document's first frame (the size of its page
/// on screen is known only once it is laid out).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingWindow {
    pub doc: pdfcraft_engine::DocId,
    pub fit: bool,
    pub center: bool,
    /// Frames left to wait for the document to be laid out before giving up.
    pub frames_left: u8,
}

/// What the window looks like now, as the windowing layer reports it (points).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowNow {
    /// The window's content area.
    pub inner: Vec2,
    /// The window with its frame, where the windowing layer knows it.
    pub outer: Option<Rect>,
    pub monitor: Option<Vec2>,
    /// The document area inside the window.
    pub document_area: Vec2,
    pub maximized: bool,
    pub fullscreen: bool,
}

/// The first displayed page (or spread) as the view lays it out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageNow {
    /// Its width and height on screen at 100%, in points.
    pub size: Vec2,
    /// The space the view keeps around it (gutters, margins, the gap in a spread), in points.
    pub margins: Vec2,
    /// The view's set zoom (1.0 = 100%); `None` when the view fits the page to the window.
    pub zoom: Option<f32>,
}

/// Smallest window the fit makes, so a tiny page doesn't leave a sliver of a window.
const MIN_INNER: Vec2 = vec2(480.0, 360.0);
/// Share of the monitor the fitted window may take, leaving room for the desktop's own bars.
const MONITOR_SHARE: f32 = 0.9;

/// The viewport commands for `FitWindow` / `CenterWindow`. None for a maximized or full-screen
/// window (resizing it would fight the user's choice), or when the sizes are unusable.
///
/// `FitWindow` keeps the document area's height and makes the window as wide as the page needs
/// at that height when the view fits the page to the window, and makes the document area the
/// page's size on screen when the view has a set zoom; either way within the monitor.
/// `CenterWindow` centres the window on the monitor when the window is on the monitor at the
/// desktop's origin (the windowing layer reports the monitor's size but not where it is).
pub fn window_commands(now: WindowNow, page: PageNow, fit: bool, center: bool) -> Vec<ViewportCommand> {
    let mut out = Vec::new();
    if now.maximized || now.fullscreen || !usable(now.inner) || !usable(now.document_area) {
        return out;
    }
    let chrome = (now.inner - now.document_area).max(Vec2::ZERO);
    let mut inner = now.inner;
    let zoom_ok = page.zoom.is_none_or(|z| z.is_finite() && z > 0.0);
    if fit && zoom_ok && usable(page.size) && page.margins.x.is_finite() && page.margins.y.is_finite() {
        let area = match page.zoom {
            // Fitted to the window: keep the document area's height, and make the area as wide
            // as the page is at the zoom that height gives it.
            None => {
                let zoom = ((now.document_area.y - page.margins.y) / page.size.y).max(0.0);
                vec2(page.size.x * zoom + page.margins.x, now.document_area.y)
            }
            Some(zoom) => page.size * zoom + page.margins,
        };
        let mut target = area + chrome;
        if let Some(monitor) = now.monitor.filter(|m| usable(*m)) {
            target = target.min(monitor * MONITOR_SHARE);
        }
        target = target.max(MIN_INNER).round();
        if usable(target) && (target - now.inner).length() >= 1.0 {
            out.push(ViewportCommand::InnerSize(target));
            inner = target;
        }
    }
    if center
        && let (Some(outer), Some(monitor)) = (now.outer, now.monitor.filter(|m| usable(*m)))
        && Rect::from_min_size(Pos2::ZERO, monitor).contains(outer.min)
    {
        // The frame around the content area stays the size it is.
        let frame = (outer.size() - now.inner).max(Vec2::ZERO);
        let size = inner + frame;
        let at = ((monitor - size) / 2.0).max(Vec2::ZERO).round();
        if at.x.is_finite() && at.y.is_finite() && (at - outer.min.to_vec2()).length() >= 1.0 {
            out.push(ViewportCommand::OuterPosition(at.to_pos2()));
        }
    }
    out
}

fn usable(v: Vec2) -> bool {
    v.x.is_finite() && v.y.is_finite() && v.x >= 1.0 && v.y >= 1.0
}

impl crate::PdfCraftApp {
    /// The window and interface preferences of the document opening in tab `index`
    /// ([`Self::apply_initial_view`] has applied its navigation, layout, zoom and page).
    ///
    /// - `HideToolbar`, `HideMenubar`, `HideWindowUI` hide those parts of the interface in its tab,
    ///   unless Always use my default zoom and page layout is on, with a notice of the key that
    ///   brings them back.
    /// - A document that shows one page or one spread at a time (`/PageLayout /SinglePage`,
    ///   `/TwoPageLeft`, `/TwoPageRight`) and names no zoom opens at fit page instead of the
    ///   default zoom. ISO 32000-2 doesn't say which zoom such a layout opens at.
    /// - `FitWindow`, `CenterWindow` wait for the page to be laid out
    ///   ([`Self::window_preferences_frame`]), and only for the window's only document: a
    ///   document opened in a tab beside others doesn't resize or move their window.
    pub(crate) fn apply_viewer_preferences(&mut self, index: usize, v: &pdfcraft_engine::InitialView, activate: bool) {
        use pdfcraft_engine::{InitialLayout as L, Magnification as M};
        let own_view = self.always_default_view;
        let only_document = self.views.len() == 1 && !self.combine_tab.open;
        let Some(view) = self.views.get_mut(index) else { return };
        view.interface = TabInterface::opening(HiddenInterface::asked_by(v), own_view);
        if !own_view && v.magnification == M::Default && matches!(v.layout, L::SinglePage | L::TwoUp | L::TwoUpCoverPage) {
            view.fit = crate::canvas::Fit::Page;
        }
        let (doc, hidden) = (view.id, view.interface.hidden.any());
        if activate && hidden {
            self.notify_interface_hidden();
        }
        // A browser page can't resize or move its window.
        if !cfg!(target_arch = "wasm32") && activate && only_document && !self.full_screen && (v.fit_window || v.center_window) {
            self.pending_window = Some(PendingWindow { doc, fit: v.fit_window, center: v.center_window, frames_left: 30 });
        }
    }

    /// The parts of the interface hidden in the active tab.
    pub fn hidden_interface(&self) -> HiddenInterface {
        self.active.and_then(|i| self.views.get(i)).map_or(HiddenInterface::NONE, |v| v.interface.hidden)
    }

    /// [`TOGGLE`] in the active tab.
    pub(crate) fn toggle_interface(&mut self) {
        let Some(view) = self.active.and_then(|i| self.views.get_mut(i)) else { return };
        if view.interface.toggle() {
            self.notify_interface_hidden();
        }
    }

    fn notify_interface_hidden(&mut self) {
        let key = pdfcraft_engine::commands::command(TOGGLE).and_then(|c| c.shortcut).map(|s| s.label(cfg!(target_os = "macos"))).unwrap_or_default();
        self.notify_fmt("This document hides the toolbars and panels. Press {key} to show them.", &[("key", &key)]);
    }

    /// `FitWindow` and `CenterWindow` for the document just opened, once its first page is laid
    /// out. Dropped when another tab shows first, or after a few frames without a layout.
    pub(crate) fn window_preferences_frame(&mut self, ctx: &egui::Context) {
        let Some(mut pending) = self.pending_window.take() else { return };
        let Some((index, id)) = self.active_ids() else { return };
        if id != pending.doc {
            return;
        }
        let Some(view) = self.views.get(index) else { return };
        let area = view.viewport_rect();
        let page = self.session.get(id).and_then(|d| view.first_page_now(&d.info));
        let Some(page) = page.filter(|_| area.is_positive()) else {
            if let Some(left) = pending.frames_left.checked_sub(1) {
                pending.frames_left = left;
                self.pending_window = Some(pending);
                ctx.request_repaint();
            }
            return;
        };
        let content = ctx.content_rect().size();
        let now = ctx.input(|i| {
            let vp = i.viewport();
            WindowNow {
                inner: vp.inner_rect.map_or(content, |r| r.size()),
                outer: vp.outer_rect,
                monitor: vp.monitor_size,
                document_area: area.size(),
                maximized: vp.maximized.unwrap_or(false),
                fullscreen: vp.fullscreen.unwrap_or(false),
            }
        });
        for command in window_commands(now, page, pending.fit, pending.center) {
            ctx.send_viewport_cmd(command);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> WindowNow {
        WindowNow {
            inner: vec2(1400.0, 900.0),
            outer: Some(Rect::from_min_size(Pos2::new(50.0, 40.0), vec2(1400.0, 930.0))),
            monitor: Some(vec2(1920.0, 1080.0)),
            document_area: vec2(1300.0, 800.0),
            maximized: false,
            fullscreen: false,
        }
    }

    const FITTED: PageNow = PageNow { size: vec2(200.0, 300.0), margins: vec2(140.0, 56.0), zoom: None };

    #[test]
    fn a_fitted_page_gets_a_window_of_its_shape() {
        // An 800-point-high area fits the 300-point page at 248%: 496 points wide plus margins,
        // and the 100 points of interface beside the document area.
        let c = window_commands(now(), FITTED, true, false);
        assert_eq!(c, vec![ViewportCommand::InnerSize(vec2(736.0, 900.0))]);
    }

    #[test]
    fn a_set_zoom_gets_the_page_size_within_the_monitor() {
        let page = PageNow { size: vec2(400.0, 500.0), margins: vec2(100.0, 100.0), zoom: Some(1.0) };
        assert_eq!(window_commands(now(), page, true, false), vec![ViewportCommand::InnerSize(vec2(600.0, 700.0))]);
        // Too tall for the monitor: the window stops at the share of the monitor it may take.
        let page = PageNow { size: vec2(1000.0, 2000.0), margins: Vec2::ZERO, zoom: Some(1.0) };
        assert_eq!(window_commands(now(), page, true, false), vec![ViewportCommand::InnerSize(vec2(1100.0, 972.0))]);
        // A tiny page doesn't make a sliver of a window.
        let page = PageNow { size: vec2(10.0, 10.0), margins: Vec2::ZERO, zoom: Some(1.0) };
        assert_eq!(window_commands(now(), page, true, false), vec![ViewportCommand::InnerSize(MIN_INNER)]);
    }

    #[test]
    fn a_maximized_or_full_screen_window_is_left_alone() {
        assert!(window_commands(WindowNow { maximized: true, ..now() }, FITTED, true, true).is_empty());
        assert!(window_commands(WindowNow { fullscreen: true, ..now() }, FITTED, true, true).is_empty());
        // Nothing asked, nothing done.
        assert!(window_commands(now(), FITTED, false, false).is_empty());
    }

    #[test]
    fn centring_uses_the_new_size_and_the_frame() {
        let c = window_commands(now(), FITTED, true, true);
        // 736 wide and 900 + 30 of frame high, on 1920 × 1080.
        assert_eq!(c.get(1), Some(&ViewportCommand::OuterPosition(Pos2::new(592.0, 75.0))));
        // Centre alone keeps the size.
        let c = window_commands(now(), FITTED, false, true);
        assert_eq!(c, vec![ViewportCommand::OuterPosition(Pos2::new(260.0, 75.0))]);
        // A window on another monitor (outside the one at the origin) isn't moved.
        let away = WindowNow { outer: Some(Rect::from_min_size(Pos2::new(2000.0, 40.0), vec2(1400.0, 930.0))), ..now() };
        assert!(window_commands(away, FITTED, false, true).is_empty());
        // Nor is one whose monitor or frame is unknown.
        assert!(window_commands(WindowNow { monitor: None, ..now() }, FITTED, false, true).is_empty());
        assert!(window_commands(WindowNow { outer: None, ..now() }, FITTED, false, true).is_empty());
    }

    #[test]
    fn unusable_sizes_do_nothing() {
        for bad in [Vec2::ZERO, vec2(f32::NAN, 10.0), vec2(f32::INFINITY, 10.0), vec2(-5.0, 10.0)] {
            let page = PageNow { size: bad, ..FITTED };
            assert!(window_commands(now(), page, true, false).is_empty(), "{bad:?}");
            assert!(window_commands(WindowNow { inner: bad, ..now() }, FITTED, true, true).is_empty(), "{bad:?}");
            assert!(window_commands(WindowNow { document_area: bad, ..now() }, FITTED, true, true).is_empty(), "{bad:?}");
            assert!(window_commands(WindowNow { monitor: Some(bad), ..now() }, FITTED, false, true).is_empty(), "{bad:?}");
        }
        for zoom in [f32::NAN, 0.0, -1.0, f32::INFINITY] {
            assert!(window_commands(now(), PageNow { zoom: Some(zoom), ..FITTED }, true, false).is_empty(), "{zoom}");
        }
        let page = PageNow { margins: vec2(f32::NAN, 0.0), ..FITTED };
        assert!(window_commands(now(), page, true, false).is_empty());
    }

    #[test]
    fn toggling_restores_and_rehides() {
        let asked = HiddenInterface { toolbar: true, ..HiddenInterface::NONE };
        let mut tab = TabInterface::opening(asked, false);
        assert_eq!(tab.hidden, asked);
        assert!(!tab.toggle());
        assert_eq!(tab.hidden, HiddenInterface::NONE);
        assert!(tab.toggle());
        assert_eq!(tab.hidden, asked);
        // The user's preference to open documents their own way hides nothing.
        let mut own = TabInterface::opening(asked, true);
        assert_eq!(own.hidden, HiddenInterface::NONE);
        // A tab whose document asked for nothing hides everything, and shows it again.
        assert!(own.toggle());
        assert_eq!(own.hidden, HiddenInterface::ALL);
        assert!(!own.toggle());
    }
}
