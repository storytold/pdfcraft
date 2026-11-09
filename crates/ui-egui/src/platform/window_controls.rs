//! Window buttons and resize edges for desktops that leave the window frame to the app.
//!
//! GNOME on Wayland has no server-side decorations (the compositor offers no
//! `zxdg_decoration_manager_v1`), so winit drew its own imitation frame. Instead the app turns the
//! frame off and the tab strip becomes the header bar, as in GNOME's own apps: it carries the
//! minimize, maximize and close buttons in the desktop's order (GNOME's `button-layout`), and
//! thin zones along the window's edges resize it. Desktops with server-side decorations (KDE,
//! Sway, Hyprland, X11) keep their own frame and never get here.

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

/// The round button's diameter, as in Adwaita.
pub const BUTTON: f32 = 24.0;
/// Between two window buttons.
pub const GAP: f32 = 6.0;
/// Between the window buttons and the rest of the strip.
pub const SIDE_GAP: f32 = 10.0;

/// The width `buttons` take in the strip, including the gap to the rest of it.
pub fn width(buttons: &[Button]) -> f32 {
    match buttons.len() {
        0 => 0.0,
        n => n as f32 * BUTTON + (n - 1) as f32 * GAP + SIDE_GAP,
    }
}

/// Draw the window buttons for one end of the strip, in reading order, inside a horizontal
/// layout running left to right; the caller adds [`SIDE_GAP`] where the strip continues.
pub fn buttons(ui: &mut egui::Ui, buttons: &[Button]) {
    let t = Tokens::get(ui.ctx());
    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
    for (n, &button) in buttons.iter().enumerate() {
        if n > 0 {
            ui.add_space(GAP);
        }
        let label = match button {
            Button::Minimize => tl!("Minimize"),
            Button::Maximize if maximized => tl!("Restore"),
            Button::Maximize => tl!("Maximize"),
            Button::Close => tl!("Close"),
        };
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(BUTTON), Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
        let response = response.on_hover_text(label);
        if ui.is_rect_visible(rect) {
            paint(ui.painter(), rect, button, maximized, &t, &response);
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
    egui::Area::new(egui::Id::unique("window_resize_edges"))
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
                let response = ui.interact(rect, ui.scope_id().with(("edge", n)), Sense::drag()).on_hover_cursor(cursor(direction));
                // Sensing only drags, a drag starts on the press itself.
                if response.drag_started() {
                    desktop_grab(ui.ctx(), egui::ViewportCommand::BeginResize(direction));
                }
            }
            // Without the desktop's frame or shadow, a hairline keeps the window apart from
            // what's behind it.
            ui.painter().rect_stroke(
                screen,
                0.0,
                Stroke::new(1.0, Color32::from_black_alpha(if t.dark() { 160 } else { 60 })),
                egui::StrokeKind::Inside,
            );
        });
}

/// Return the stable egui id for desktop window-grab state.
fn grab_id() -> egui::Id {
    egui::Id::unique("window_controls_desktop_grab")
}

/// Hand the pointer to the desktop to move or resize the window. On Wayland the compositor keeps
/// the button's release and winit reports only that the pointer left, which egui doesn't count as
/// a release; egui would think the button still held, and the next press on an edge or the title
/// bar wouldn't start a drag until a click inside the window. The release comes back on the next
/// input (`release_after_desktop_grab`).
pub fn desktop_grab(ctx: &egui::Context, command: egui::ViewportCommand) {
    ctx.send_viewport_cmd(command);
    ctx.data_mut(|d| d.insert_temp(grab_id(), ()));
    ctx.request_repaint();
}

/// After `desktop_grab`, add the primary button's release to the next input, unless it brings one.
/// The release goes before the input's first press (the next resize or move, or a click), which
/// egui would otherwise end straight away; a release after that press is the press's own. It also
/// goes before the pointer leaving, as a button event would bring the gone pointer back; a release
/// the input brings after the pointer left is the grab's own, and moves there instead.
pub fn release_after_desktop_grab(ctx: &egui::Context, raw: &mut egui::RawInput) {
    if ctx.data_mut(|d| d.remove_temp::<()>(grab_id())).is_none() {
        return;
    }
    let mut at = raw.events.len();
    let mut gone = None;
    let mut brought = None;
    for (n, e) in raw.events.iter().enumerate() {
        match e {
            egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed: false, .. } => {
                brought = Some(n);
                break;
            }
            egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed: true, .. } => {
                at = n;
                break;
            }
            egui::Event::PointerGone => {
                gone.get_or_insert(n);
            }
            _ => {}
        }
    }
    if let Some(n) = brought {
        // `gone` was found before `n`, so both are in range.
        if let Some(g) = gone {
            let release = raw.events.remove(n);
            raw.events.insert(g, release);
        }
        return;
    }
    let at = gone.unwrap_or(at);
    let (pos, modifiers) = ctx.input(|i| (i.pointer.interact_pos().unwrap_or_default(), i.modifiers));
    raw.events.insert(at, egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers });
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
    fn widths_cover_the_buttons_and_their_gaps() {
        assert_eq!(super::width(&[]), 0.0);
        assert_eq!(super::width(&[Button::Close]), super::BUTTON + super::SIDE_GAP);
        assert_eq!(super::width(&Layout::default().right), 3.0 * super::BUTTON + 2.0 * super::GAP + super::SIDE_GAP);
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

    #[test]
    fn a_desktop_grab_gives_the_button_release_back() {
        let ctx = egui::Context::default();
        let releases = |raw: &egui::RawInput| {
            raw.events.iter().filter(|e| matches!(e, egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed: false, .. })).count()
        };
        let mut raw = egui::RawInput::default();
        super::release_after_desktop_grab(&ctx, &mut raw);
        assert_eq!(releases(&raw), 0, "no grab, no release");

        super::desktop_grab(&ctx, egui::ViewportCommand::StartDrag);
        super::release_after_desktop_grab(&ctx, &mut raw);
        assert_eq!(releases(&raw), 1, "the release the desktop kept");

        let mut raw = egui::RawInput::default();
        super::release_after_desktop_grab(&ctx, &mut raw);
        assert_eq!(releases(&raw), 0, "only once");

        super::desktop_grab(&ctx, egui::ViewportCommand::BeginResize(ResizeDirection::East));
        let mut raw = egui::RawInput::default();
        raw.events.push(egui::Event::PointerButton {
            pos: Pos2::ZERO,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        super::release_after_desktop_grab(&ctx, &mut raw);
        assert_eq!(releases(&raw), 1, "not doubled when the input brings it");
    }

    #[test]
    fn the_release_goes_before_the_press_of_the_next_grab() {
        let button = |pressed| egui::Event::PointerButton {
            pos: Pos2::new(5.0, 5.0),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let primary = |raw: &egui::RawInput| {
            raw.events
                .iter()
                .filter_map(|e| if let egui::Event::PointerButton { pressed, .. } = e { Some(*pressed) } else { None })
                .collect::<Vec<_>>()
        };
        let ctx = egui::Context::default();
        let frame = |events: Vec<egui::Event>, grab: Option<egui::ViewportCommand>| {
            let mut raw = egui::RawInput { events, ..Default::default() };
            super::release_after_desktop_grab(&ctx, &mut raw);
            let order = primary(&raw);
            ctx.run_ui(raw, |ui| {
                if let Some(command) = grab.clone() {
                    super::desktop_grab(ui.ctx(), command);
                }
            })
            .textures_delta
            .clear();
            order
        };
        // The press of the first resize starts the grab; the desktop keeps its release.
        frame(vec![egui::Event::PointerMoved(Pos2::new(5.0, 5.0)), button(true)], Some(egui::ViewportCommand::BeginResize(ResizeDirection::East)));
        // The next input brings the press of the second resize.
        assert_eq!(frame(vec![button(true)], None), [false, true], "the kept release, then the new press");
        assert!(ctx.input(|i| i.pointer.primary_down()), "the second resize holds the button");

        // A quick click's release belongs to its own press, not to the grab before it.
        frame(vec![], Some(egui::ViewportCommand::StartDrag));
        assert_eq!(frame(vec![button(true), button(false)], None), [false, true, false]);
    }

    #[test]
    fn the_release_goes_before_the_pointer_leaving() {
        let ctx = egui::Context::default();
        let frame = |events: Vec<egui::Event>, grab: bool| {
            let mut raw = egui::RawInput { events, ..Default::default() };
            super::release_after_desktop_grab(&ctx, &mut raw);
            let order = raw.events.clone();
            ctx.run_ui(raw, |ui| {
                if grab {
                    super::desktop_grab(ui.ctx(), egui::ViewportCommand::StartDrag);
                }
            })
            .textures_delta
            .clear();
            order
        };
        let release = |e: &egui::Event| matches!(e, egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed: false, .. });
        let press = egui::Event::PointerButton {
            pos: Pos2::new(5.0, 5.0),
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        };
        // On Wayland the input after a move brings only that the pointer left.
        frame(vec![egui::Event::PointerMoved(Pos2::new(5.0, 5.0)), press], true);
        let order = frame(vec![egui::Event::PointerGone], false);
        assert!(matches!(order.as_slice(), [r, egui::Event::PointerGone] if release(r)), "{order:?}");
        assert_eq!(ctx.input(|i| i.pointer.latest_pos()), None, "the pointer stays gone");
        assert!(!ctx.input(|i| i.pointer.primary_down()));

        // A release after the pointer left is the one the desktop kept, not to be doubled.
        frame(vec![], true);
        let order = frame(
            vec![
                egui::Event::PointerGone,
                egui::Event::PointerButton {
                    pos: Pos2::new(5.0, 5.0),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            false,
        );
        assert!(matches!(order.as_slice(), [r, egui::Event::PointerGone] if release(r)), "moved, not doubled: {order:?}");
        assert_eq!(ctx.input(|i| i.pointer.latest_pos()), None, "the pointer stays gone");
    }
}
