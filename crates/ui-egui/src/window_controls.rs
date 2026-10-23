//! Window buttons and resize edges for windows whose frame the app draws.
//!
//! GNOME on Wayland has no server-side decorations (the compositor offers no
//! `zxdg_decoration_manager_v1`), so winit drew its own imitation frame. Instead the app turns the
//! frame off and the tab strip becomes the header bar, as in GNOME's own apps: it carries the
//! minimize, maximize and close buttons in the desktop's order (GNOME's `button-layout`), and
//! thin zones along the window's edges resize it. Desktops with server-side decorations (KDE,
//! Sway, Hyprland, X11) keep their own frame and never get here.
//!
//! On Windows the same strip is offered as a choice (Preferences ▸ Use system title bar, on by
//! default there): the title bar's row goes to the tabs, as in the browsers and Windows Terminal.
//! The buttons then take the caption buttons' shape, flat and the strip's full height, with the
//! close button's red hover. winit keeps the styles Windows needs for Aero Snap, the drop shadow
//! and edge resizing on an undecorated window; only the Snap Layouts flyout, which Windows 11
//! shows over its own maximize button, needs the system title bar.

use crate::theme::Tokens;
use egui::viewport::ResizeDirection;
use egui::{Align2, Color32, CursorIcon, Pos2, Rect, Sense, Stroke, Vec2};

/// A window button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Maximize,
    Close,
}

/// Which buttons go on which end of the header bar, outermost last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub left: Vec<Button>,
    pub right: Vec<Button>,
}

/// Use system title bar when nothing is saved: on where Windows' own title bar is the norm, off
/// on GNOME, whose frame winit only imitates.
pub const SYSTEM_TITLE_BAR_DEFAULT: bool = cfg!(target_os = "windows");

/// The saved Use system title bar preference in PdfCraft's settings JSON: `title_bar`, `"system"`
/// or `"app"`. Settings from before Windows offered the preference hold the boolean
/// `system_title_bar` instead, which every platform wrote as `false` while only GNOME showed the
/// checkbox; it still counts there, but on Windows it was never a choice, so it is left alone and
/// the default stands rather than every upgrade losing the title bar.
pub fn saved_system_title_bar(settings: &serde_json::Value) -> Option<bool> {
    match settings.get("title_bar").and_then(serde_json::Value::as_str) {
        Some("system") => Some(true),
        Some("app") => Some(false),
        _ if cfg!(target_os = "windows") => None,
        _ => settings.get("system_title_bar")?.as_bool(),
    }
}

impl Default for Layout {
    /// What a desktop that names no layout gets: all three, on the right.
    fn default() -> Self {
        Self { left: Vec::new(), right: vec![Button::Minimize, Button::Maximize, Button::Close] }
    }
}

impl Layout {
    /// GNOME's `org.gnome.desktop.wm.preferences button-layout`: `left:right`, each a comma list
    /// such as `appmenu:minimize,maximize,close` (Fedora's default is `appmenu:close`). Without a
    /// colon everything is on the left, as in GTK. Names this app has no button for (`appmenu`,
    /// `icon`, `menu`, `spacer`) are skipped, and a button named twice appears once. `gsettings`
    /// prints the value quoted; the quotes are dropped.
    pub fn parse(value: &str) -> Self {
        let value = value.trim().trim_matches(|c| c == '\'' || c == '"');
        let (left, right) = value.split_once(':').unwrap_or((value, ""));
        let mut seen = Vec::new();
        let mut side = |list: &str| {
            let mut out = Vec::new();
            for name in list.split(',') {
                let button = match name.trim() {
                    "minimize" => Button::Minimize,
                    "maximize" => Button::Maximize,
                    "close" => Button::Close,
                    _ => continue,
                };
                if !seen.contains(&button) {
                    seen.push(button);
                    out.push(button);
                }
            }
            out
        };
        let left = side(left);
        let right = side(right);
        Self { left, right }
    }
}

/// How the buttons look: the desktop's own caption buttons, so the strip reads as a title bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// Round washes of the text colour with symbolic glyphs, as in GNOME's header bars.
    Adwaita,
    /// Flat rectangles the strip's full height, as Windows draws its caption buttons; the close
    /// button turns red on hover.
    Windows,
}

impl Style {
    /// The style for the platform this build runs on.
    pub fn current() -> Self {
        if cfg!(target_os = "windows") { Self::Windows } else { Self::Adwaita }
    }

    /// Each button's width.
    pub fn button_width(self) -> f32 {
        match self {
            Self::Adwaita => BUTTON,
            Self::Windows => WINDOWS_BUTTON_WIDTH,
        }
    }

    /// Between two window buttons.
    pub fn gap(self) -> f32 {
        match self {
            Self::Adwaita => GAP,
            Self::Windows => 0.0,
        }
    }

    /// Between the window buttons and the rest of the strip.
    pub fn side_gap(self) -> f32 {
        match self {
            Self::Adwaita => SIDE_GAP,
            Self::Windows => WINDOWS_SIDE_GAP,
        }
    }

    /// Whether the buttons sit flush against the window's edge, with no strip margin outside them.
    pub fn flush_with_edge(self) -> bool {
        matches!(self, Self::Windows)
    }

    /// The width `buttons` take in the strip, including the gap to the rest of it.
    pub fn width(self, buttons: &[Button]) -> f32 {
        match buttons.len() {
            0 => 0.0,
            n => n as f32 * self.button_width() + (n - 1) as f32 * self.gap() + self.side_gap(),
        }
    }
}

/// The round button's diameter, as in Adwaita.
pub const BUTTON: f32 = 24.0;
/// Between two window buttons.
pub const GAP: f32 = 6.0;
/// Between the window buttons and the rest of the strip.
pub const SIDE_GAP: f32 = 10.0;
/// A Windows caption button's width (46 px at 100 %, from the system's own title bars).
const WINDOWS_BUTTON_WIDTH: f32 = 46.0;
/// Between the Windows buttons and the rest of the strip.
const WINDOWS_SIDE_GAP: f32 = 8.0;
/// Windows' close button on hover and press.
const WINDOWS_CLOSE_HOVER: Color32 = Color32::from_rgb(0xC4, 0x2B, 0x1C);
const WINDOWS_CLOSE_PRESSED: Color32 = Color32::from_rgb(0xB1, 0x2A, 0x1E);

/// The width `buttons` take in the strip, including the gap to the rest of it.
pub fn width(buttons: &[Button]) -> f32 {
    Style::current().width(buttons)
}

/// Draw the window buttons for one end of the strip, in reading order, inside a horizontal
/// layout running left to right; the caller adds [`SIDE_GAP`] where the strip continues.
pub fn buttons(ui: &mut egui::Ui, buttons: &[Button]) {
    let t = Tokens::get(ui.ctx());
    let style = Style::current();
    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
    // Windows' buttons stand the strip's full height, so a hover fills it top to bottom.
    let size = match style {
        Style::Adwaita => Vec2::splat(BUTTON),
        Style::Windows => Vec2::new(WINDOWS_BUTTON_WIDTH, ui.available_height().max(BUTTON)),
    };
    for (n, &button) in buttons.iter().enumerate() {
        if n > 0 {
            ui.add_space(style.gap());
        }
        let label = match button {
            Button::Minimize => tl!("Minimize"),
            Button::Maximize if maximized => tl!("Restore"),
            Button::Maximize => tl!("Maximize"),
            Button::Close => tl!("Close"),
        };
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
        let response = response.on_hover_text(label);
        if ui.is_rect_visible(rect) {
            match style {
                Style::Adwaita => paint(ui.painter(), rect, button, maximized, &t, &response),
                Style::Windows => paint_windows(ui.painter(), rect, button, maximized, &t, &response),
            }
        }
        if response.clicked() {
            let command = match button {
                Button::Minimize => egui::ViewportCommand::Minimized(true),
                Button::Maximize => egui::ViewportCommand::Maximized(!maximized),
                // The same path as the desktop's close: unsaved documents still ask first.
                Button::Close => egui::ViewportCommand::Close,
            };
            ui.ctx().send_viewport_cmd(command);
        }
    }
}

/// Adwaita's look: a round wash of the text colour, stronger on hover and press, and a 16-point
/// symbolic glyph drawn with lines (no icon files).
fn paint(painter: &egui::Painter, rect: Rect, button: Button, maximized: bool, t: &Tokens, response: &egui::Response) {
    let alpha = if response.is_pointer_button_down_on() {
        0.28
    } else if response.hovered() {
        0.16
    } else {
        0.10
    };
    painter.circle_filled(rect.center(), BUTTON / 2.0, t.text.gamma_multiply(alpha));
    if response.has_focus() {
        painter.circle_stroke(rect.center(), BUTTON / 2.0 - 0.5, Stroke::new(1.0, t.accent));
    }
    let stroke = Stroke::new(1.25, t.text);
    // The glyph's 16-point square, its corners on pixel centres so lines stay crisp.
    let c = rect.center();
    let at = |x: f32, y: f32| Pos2::new((c.x - 8.0 + x).round() + 0.5, (c.y - 8.0 + y).round() + 0.5);
    match button {
        Button::Minimize => {
            painter.line_segment([at(4.0, 11.0), at(11.0, 11.0)], stroke);
        }
        Button::Maximize if maximized => {
            // Restore: a smaller window in front of another.
            painter.rect_stroke(Rect::from_two_pos(at(4.0, 6.0), at(10.0, 12.0)), 1.0, stroke, egui::StrokeKind::Middle);
            painter.line_segment([at(6.0, 6.0), at(6.0, 4.0)], stroke);
            painter.line_segment([at(6.0, 4.0), at(12.0, 4.0)], stroke);
            painter.line_segment([at(12.0, 4.0), at(12.0, 10.0)], stroke);
            painter.line_segment([at(12.0, 10.0), at(10.0, 10.0)], stroke);
        }
        Button::Maximize => {
            painter.rect_stroke(Rect::from_two_pos(at(4.0, 4.0), at(11.0, 11.0)), 1.0, stroke, egui::StrokeKind::Middle);
        }
        Button::Close => {
            painter.line_segment([at(4.0, 4.0), at(11.0, 11.0)], stroke);
            painter.line_segment([at(11.0, 4.0), at(4.0, 11.0)], stroke);
        }
    }
}

/// Windows' look: a flat wash over the whole button on hover and press (the close button's is
/// red, with a white glyph), and a 10-pixel glyph drawn with 1-pixel lines on pixel centres.
fn paint_windows(painter: &egui::Painter, rect: Rect, button: Button, maximized: bool, t: &Tokens, response: &egui::Response) {
    let pressed = response.is_pointer_button_down_on();
    let hovered = response.hovered();
    let close = button == Button::Close;
    let fill = match (close, pressed, hovered) {
        (true, true, _) => WINDOWS_CLOSE_PRESSED,
        (true, false, true) => WINDOWS_CLOSE_HOVER,
        (false, true, _) => t.text.gamma_multiply(0.06),
        (false, false, true) => t.text.gamma_multiply(0.10),
        _ => Color32::TRANSPARENT,
    };
    if fill != Color32::TRANSPARENT {
        painter.rect_filled(rect, 0.0, fill);
    }
    if response.has_focus() {
        painter.rect_stroke(rect.shrink(1.0), 0.0, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
    }
    let colour = if close && (hovered || pressed) { Color32::WHITE } else { t.text };
    let stroke = Stroke::new(1.0, colour);
    // The glyph's 10-pixel square, its corners on pixel centres so lines stay crisp.
    let c = rect.center();
    let at = |x: f32, y: f32| Pos2::new((c.x - 5.0 + x).round() + 0.5, (c.y - 5.0 + y).round() + 0.5);
    match button {
        Button::Minimize => {
            painter.line_segment([at(0.0, 5.0), at(10.0, 5.0)], stroke);
        }
        Button::Maximize if maximized => {
            // Restore: the window in front of the one it was restored from.
            painter.rect_stroke(Rect::from_two_pos(at(0.0, 2.0), at(8.0, 10.0)), 1.0, stroke, egui::StrokeKind::Middle);
            painter.line_segment([at(2.0, 2.0), at(2.0, 0.0)], stroke);
            painter.line_segment([at(2.0, 0.0), at(10.0, 0.0)], stroke);
            painter.line_segment([at(10.0, 0.0), at(10.0, 8.0)], stroke);
            painter.line_segment([at(10.0, 8.0), at(8.0, 8.0)], stroke);
        }
        Button::Maximize => {
            painter.rect_stroke(Rect::from_two_pos(at(0.0, 0.0), at(10.0, 10.0)), 1.0, stroke, egui::StrokeKind::Middle);
        }
        Button::Close => {
            painter.line_segment([at(0.0, 0.0), at(10.0, 10.0)], stroke);
            painter.line_segment([at(10.0, 0.0), at(0.0, 10.0)], stroke);
        }
    }
}

/// How far into the window an edge resizes it.
const EDGE: f32 = 5.0;
/// How far along an edge a corner reaches, so corners are easy to hit.
const CORNER: f32 = 16.0;

/// The resize zones of a window filling `rect`: edges `edge` deep, and each corner an L of two
/// strips reaching `corner` along both edges (a square would reach into the content).
fn zones(rect: Rect, edge: f32, corner: f32) -> Vec<(ResizeDirection, Rect)> {
    let (l, r, t, b) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    // Never inverted, however small the window.
    let zone = |x0: f32, x1: f32, y0: f32, y1: f32| Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1.max(x0), y1.max(y0)));
    vec![
        (ResizeDirection::NorthWest, zone(l, l + corner, t, t + edge)),
        (ResizeDirection::NorthWest, zone(l, l + edge, t + edge, t + corner)),
        (ResizeDirection::NorthEast, zone(r - corner, r, t, t + edge)),
        (ResizeDirection::NorthEast, zone(r - edge, r, t + edge, t + corner)),
        (ResizeDirection::SouthWest, zone(l, l + corner, b - edge, b)),
        (ResizeDirection::SouthWest, zone(l, l + edge, b - corner, b - edge)),
        (ResizeDirection::SouthEast, zone(r - corner, r, b - edge, b)),
        (ResizeDirection::SouthEast, zone(r - edge, r, b - corner, b - edge)),
        (ResizeDirection::North, zone(l + corner, r - corner, t, t + edge)),
        (ResizeDirection::South, zone(l + corner, r - corner, b - edge, b)),
        (ResizeDirection::West, zone(l, l + edge, t + corner, b - corner)),
        (ResizeDirection::East, zone(r - edge, r, t + corner, b - corner)),
    ]
}

fn cursor(direction: ResizeDirection) -> CursorIcon {
    match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    }
}

/// Resize zones along the window's edges and a hairline border, while the window is neither
/// maximized nor full screen. The zones sit above everything else (in the foreground order), so a
/// press there resizes instead of reaching the scrollbar or panel under it.
pub fn resize_edges(ctx: &egui::Context) {
    let (maximized, fullscreen) = ctx.input(|i| (i.viewport().maximized.unwrap_or(false), i.viewport().fullscreen.unwrap_or(false)));
    if maximized || fullscreen {
        return;
    }
    let screen = ctx.content_rect();
    let t = Tokens::get(ctx);
    egui::Area::new(egui::Id::new("window_resize_edges"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .pivot(Align2::LEFT_TOP)
        .interactable(true)
        .show(ctx, |ui| {
            ui.set_min_size(Vec2::ZERO);
            for (n, (direction, rect)) in zones(screen, EDGE, CORNER).into_iter().enumerate() {
                if rect.width() <= 0.0 || rect.height() <= 0.0 {
                    continue;
                }
                let response = ui.interact(rect, ui.id().with(("edge", n)), Sense::drag()).on_hover_cursor(cursor(direction));
                // Sensing only drags, a drag starts on the press itself.
                if response.drag_started() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
                }
            }
            // Without the desktop's frame or shadow, a hairline keeps the window apart from
            // what's behind it. Windows keeps the drop shadow on an undecorated window (winit's
            // undecorated-shadow style), so the hairline would only double its own border.
            if Style::current() == Style::Windows {
                return;
            }
            ui.painter().rect_stroke(
                screen,
                0.0,
                Stroke::new(1.0, Color32::from_black_alpha(if t.dark() { 160 } else { 60 })),
                egui::StrokeKind::Inside,
            );
        });
}

#[cfg(test)]
mod tests {
    use super::{Button, Layout, ResizeDirection, zones};
    use egui::{Pos2, Rect};

    #[test]
    fn gnome_button_layouts_parse() {
        let close = Layout { left: vec![], right: vec![Button::Close] };
        assert_eq!(Layout::parse("'appmenu:close'"), close);
        assert_eq!(Layout::parse("appmenu:close\n"), close);
        assert_eq!(Layout::parse(":minimize,maximize,close"), Layout::default());
        assert_eq!(
            Layout::parse("close,minimize,maximize:appmenu"),
            Layout { left: vec![Button::Close, Button::Minimize, Button::Maximize], right: vec![] }
        );
        // No colon: the left side, as GTK reads it.
        assert_eq!(Layout::parse("close"), Layout { left: vec![Button::Close], right: vec![] });
        // Unknown names, spaces and repeats.
        assert_eq!(Layout::parse("icon,spacer: close , close,bogus"), close);
        assert_eq!(Layout::parse(""), Layout { left: vec![], right: vec![] });
    }

    #[test]
    fn the_saved_preference_is_read_with_its_history() {
        use super::saved_system_title_bar as saved;
        use serde_json::json;
        assert_eq!(saved(&json!({"title_bar": "system"})), Some(true));
        assert_eq!(saved(&json!({"title_bar": "app", "system_title_bar": true})), Some(false));
        assert_eq!(saved(&json!({"title_bar": "sideways"})), None);
        assert_eq!(saved(&json!({"theme": "Dark"})), None);
        assert_eq!(saved(&json!("not an object")), None);
        // The boolean of older builds: a choice on GNOME, where the checkbox was, and on Windows
        // only the default every save wrote.
        let legacy = if cfg!(target_os = "windows") { None } else { Some(false) };
        assert_eq!(saved(&json!({"system_title_bar": false})), legacy);
        assert_eq!(saved(&json!({"system_title_bar": true})), legacy.map(|_| true));
        assert_eq!(saved(&json!({"system_title_bar": "true"})), None);
    }

    #[test]
    fn widths_cover_the_buttons_and_their_gaps() {
        use super::Style::{Adwaita, Windows};
        assert_eq!(Adwaita.width(&[]), 0.0);
        assert_eq!(Adwaita.width(&[Button::Close]), super::BUTTON + super::SIDE_GAP);
        assert_eq!(Adwaita.width(&Layout::default().right), 3.0 * super::BUTTON + 2.0 * super::GAP + super::SIDE_GAP);
        // Windows' caption buttons touch each other and the window's edge.
        assert_eq!(Windows.width(&[]), 0.0);
        assert_eq!(Windows.width(&[Button::Close]), super::WINDOWS_BUTTON_WIDTH + super::WINDOWS_SIDE_GAP);
        assert_eq!(Windows.width(&Layout::default().right), 3.0 * super::WINDOWS_BUTTON_WIDTH + super::WINDOWS_SIDE_GAP);
        assert!(Windows.flush_with_edge() && !Adwaita.flush_with_edge());
        assert_eq!(super::width(&[Button::Close]), super::Style::current().width(&[Button::Close]));
    }

    #[test]
    fn resize_zones_line_the_edges_without_overlapping() {
        let window = Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0));
        let zones = zones(window, 5.0, 16.0);
        let at = |x: f32, y: f32| zones.iter().filter(|(_, r)| r.contains(Pos2::new(x, y))).map(|(d, _)| *d).collect::<Vec<_>>();
        assert_eq!(at(1.0, 1.0), [ResizeDirection::NorthWest]);
        assert_eq!(at(2.0, 12.0), [ResizeDirection::NorthWest]);
        assert_eq!(at(400.0, 2.0), [ResizeDirection::North]);
        assert_eq!(at(799.0, 300.0), [ResizeDirection::East]);
        assert_eq!(at(400.0, 599.0), [ResizeDirection::South]);
        assert_eq!(at(1.0, 300.0), [ResizeDirection::West]);
        assert_eq!(at(799.0, 599.0), [ResizeDirection::SouthEast]);
        assert!(at(400.0, 300.0).is_empty(), "the middle of the window is left alone");
        assert!(at(12.0, 12.0).is_empty(), "corners stay along the edges");
        // Strips meet without overlapping.
        for y in [0.5, 4.5, 5.5, 15.5, 16.5, 595.5] {
            for x in [0.5, 4.5, 5.5, 15.5, 16.5, 795.5, 799.5] {
                assert!(at(x, y).len() <= 1, "({x}, {y}): {:?}", at(x, y));
            }
        }
        // A window smaller than its corners gets empty (not inverted) edge zones.
        for (_, r) in super::zones(Rect::from_min_size(Pos2::ZERO, egui::vec2(20.0, 20.0)), 5.0, 16.0) {
            assert!(r.width() >= 0.0 && r.height() >= 0.0);
        }
    }
}
