//! Window composition: the shell's title bar, mode bar, view rail, dock and page area.

use craft::behavior::IconId;
use craft::keymap::CommandRef;
use craft::menu::ThemeChoice;
use craft::theme::ModeChoice;
use craft::ui::buttons::{Button, IconButton};
use craft::ui::contract::{Bind, Outcome as _, Props as _, Show as _, ShowContainer as _};
use craft::ui::menu::{Menu, MenuItem};
use craft::ui::popover::DropVeil;
use craft::ui::runtime::env::WindowKey;
use craft::ui::shell::{AppShell, Slot, TitleBar};

use crate::panes::Place;
use crate::{PdfCraftApp, side_panels};

<<<<<<< HEAD
pub(crate) mod doc_tabs;
mod mode_bar;
pub(crate) mod view_rail;

pub(crate) const HOME_ICON: &str = "home";

/// Compose the window: title bar, mode bar, view rail and the dock, whose document panel holds the
/// pages.
///
/// The shell carries out every screen-mode change; the mode it ends the pass with goes back to the
/// app, which does the presentation work in one place.
pub(crate) fn shell(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    // Production's dialogs end with their buttons: no close x in the title row.
    craft::ui::dialog::set_close_button(ui.ctx(), false);
=======
/// Height of the tab strip when it doubles as the macOS title bar (#789). AppKit draws the traffic
/// lights itself, centred in the standard 28 pt title-bar band at the top of the window, and moving
/// them takes AppKit calls the workspace forbids (`unsafe`). So on macOS the strip is that band:
/// the lights sit vertically centred in it, and the controls beside them shrink to fit, as in
/// WordCraft (storytold/wordcraft#255).
pub const MAC_TITLE_BAR: f32 = 28.0;
/// The traffic lights as AppKit lays them out: button diameter, centre-to-centre pitch and the
/// first button's left edge (8–10 pt depending on the macOS version; the larger value keeps the
/// gap after them from shrinking).
const MAC_LIGHT: f32 = 12.0;
const MAC_LIGHT_PITCH: f32 = 20.0;
const MAC_LIGHTS_LEFT: f32 = 10.0;
/// Where the strip's content starts to the right of the traffic lights: the end of the three
/// lights plus the same gap the lights keep from the top of the window. Mirrored, it is where the
/// content ends when the lights are at the right.
pub const MAC_CONTENT_LEFT: f32 = MAC_LIGHTS_LEFT + 2.0 * MAC_LIGHT_PITCH + MAC_LIGHT + (MAC_TITLE_BAR - MAC_LIGHT) / 2.0;

/// The strip and its controls everywhere else.
const TAB_STRIP: f32 = 38.0;
const TAB_HEIGHT: f32 = 30.0;
const STRIP_BUTTON: f32 = 28.0;

/// Whether AppKit lays the traffic lights out from the right end of the title bar, or `None` when
/// no window can say yet. It does when the system language is right-to-left, whatever PdfCraft's
/// own interface language is, so the window is asked rather than the interface language
/// (Apple: `NSWindow.windowTitlebarLayoutDirection`, for apps with controls under the title bar).
#[cfg(target_os = "macos")]
fn mac_lights_on_right() -> Option<bool> {
    use objc2_app_kit::{NSApplication, NSUserInterfaceLayoutDirection};
    // AppKit's window list is only for the main thread (where egui runs).
    let mtm = objc2::MainThreadMarker::new()?;
    let window = NSApplication::sharedApplication(mtm).windows().firstObject()?;
    Some(window.windowTitlebarLayoutDirection() == NSUserInterfaceLayoutDirection::RightToLeft)
}

#[cfg(not(target_os = "macos"))]
fn mac_lights_on_right() -> Option<bool> {
    None
}

/// Draw the window's tab strip: home and open controls, shrinkable and scrollable document tabs with drag
/// reordering, and platform window controls.
pub fn tab_strip(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    // macOS: the strip is the 28 pt title bar, so the traffic lights are centred in it; tabs and
    // buttons shrink to keep a margin above and below.
    let mac = cfg!(target_os = "macos") && app.integrated_titlebar;
    if mac && app.titlebar_lights_right.is_none() {
        app.titlebar_lights_right = mac_lights_on_right();
    }
    // With the lights at the right (a right-to-left system language), the strip is the mirror
    // image of the usual one: Home beside the lights, the tabs and Open after it, and the
    // Discord, shortcuts and theme buttons at the far end.
    let mirrored = mac && app.titlebar_lights_right == Some(true);
    let (left, right) = match (mac, mirrored) {
        (true, false) => (MAC_CONTENT_LEFT as i8, 10),
        (true, true) => (10, MAC_CONTENT_LEFT as i8),
        (false, _) => (8, 10),
    };
    let (row, far_end) = if mirrored {
        (Layout::right_to_left(Align::Center), Layout::left_to_right(Align::Center))
    } else {
        (Layout::left_to_right(Align::Center), Layout::right_to_left(Align::Center))
    };
    let (tab_h, button) = if mac { (MAC_TITLE_BAR - 2.0, 24.0) } else { (TAB_HEIGHT, STRIP_BUTTON) };
    egui::Panel::top("tab_strip")
        .exact_size(if mac { MAC_TITLE_BAR } else { TAB_STRIP })
        .frame(egui::Frame::NONE.fill(t.titlebar).inner_margin(egui::Margin { left, right, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            let full = ui.max_rect();
            app.panes.main_strip = Some(full);
            let drag = ui.interact(full, ui.scope_id().with("titledrag"), Sense::click_and_drag());
            if drag.drag_started() {
                window_controls::desktop_grab(ui.ctx(), egui::ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
            }
            // GNOME on Wayland: the strip is the header bar, with the window buttons at its ends.
            let fullscreen = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
            let controls = app
                .window_controls
                .clone()
                .filter(|_| app.draws_window_frame() && !fullscreen)
                .unwrap_or_else(|| window_controls::Layout { left: Vec::new(), right: Vec::new() });
            ui.allocate_ui_with_layout(ui.available_size_before_wrap(), row, |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if !controls.left.is_empty() {
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        window_controls::buttons(ui, &controls.left);
                    });
                    ui.add_space(window_controls::SIDE_GAP - 4.0);
                }
                if icons::button(ui, "house", button, app.active.is_none() && !app.combine_showing(), tl!("Home")).clicked() {
                    app.active = None;
                    app.combine_tab.focused = false;
                }
                // Reserve the three icon buttons, Discord and their gaps outside the scrolling tabs.
                let controls_width = ui.fonts_mut(|f| f.layout_no_wrap("Discord".into(), theme::medium(13.0), t.text).size().x)
                    + 3.0 * button
                    + 54.0
                    + window_controls::width(&controls.right);
                // Open stays in sight after the tabs: its icon, text and padding, and the gaps around it.
                let open_width = ui.fonts_mut(|f| f.layout_no_wrap(tl!("Open").into(), theme::medium(13.0), t.text).size().x) + 38.0 + 12.0;
                let mut tabs_width = (ui.available_width() - controls_width - open_width).max(0.0);
                // Tabs shrink to share the strip (names end in "…") and scroll only once they reach
                // their narrowest; then arrows either side say so and step through them.
                let mut natural: Vec<f32> = Vec::with_capacity(app.views.len() + 1);
                // (The main pane's tabs: split view's second pane has a strip of its own.)
                for i in app.tabs_in(Place::Main) {
                    if let Some(name) = crate::canvas::tab_label(app, i) {
                        natural.push(tab_natural_width(ui, &t, &name));
                    }
                }
                if app.combine_tab.open {
                    natural.push(tab_natural_width(ui, &t, tl!("Combine files")));
                }
                let gap = ui.spacing().item_spacing.x;
                let overflow = tabs_overflow(&natural, tabs_width, gap, TAB_MIN_WIDTH);
                if overflow {
                    tabs_width = (tabs_width - 2.0 * (TAB_ARROW + gap)).max(0.0);
                }
                let cap = tab_cap(&natural, tabs_width, gap, TAB_MIN_WIDTH);
                // Where the tabs were scrolled to last frame, and how far they can go.
                let scroll_id = ui.scope_id().with("tab_scroll");
                let (offset, max_offset) = ui.data(|d| d.get_temp::<(f32, f32)>(scroll_id)).unwrap_or((0.0, 0.0));
                let scroll_left = |ui: &mut egui::Ui| overflow && arrow(ui, "chevron-left", offset > 0.5, tl!("Scroll tabs left"));
                let scroll_right = |ui: &mut egui::Ui| overflow && arrow(ui, "chevron-right", offset < max_offset - 0.5, tl!("Scroll tabs right"));
                // The arrows keep their sides: the one laid out first is at the row's start.
                let (mut left_clicked, mut right_clicked) = (false, false);
                if mirrored {
                    right_clicked = scroll_right(ui);
                } else {
                    left_clicked = scroll_left(ui);
                }
                // Keyed by document, not index: a tab dragged past the active one moves it without
                // the strip jumping to it.
                let state = (app.active_ids().map(|(_, id)| id), app.views.len(), app.combine_showing());
                let changed = ui.data_mut(|data| {
                    let id = ui.scope_id().with("active_tab");
                    let changed = data.get_temp::<(Option<pdfcraft_engine::DocId>, usize, bool)>(id) != Some(state);
                    data.insert_temp(id, state);
                    changed
                });
                // Mirrored, the tabs' reading start is the scroll area's far end: the strip goes
                // there once when it starts to scroll, and the active tab is brought back in view.
                let start_id = scroll_id.with("reading_start");
                let (start, started) = mirrored_start(mirrored, max_offset, ui.data(|d| d.get_temp::<bool>(start_id)).unwrap_or(false));
                ui.data_mut(|d| d.insert_temp(start_id, started));
                let changed = changed || start.is_some();
                let mut action = None;
                let mut close = None;
                // The pointer's x while a tab is dragged, for scrolling at the strip's edges.
                let mut drag_x = None;
                let show_tabs = |ui: &mut egui::Ui| {
                    ui.style_mut().always_scroll_the_only_direction = true;
                    let mut area = egui::ScrollArea::horizontal().id_salt("document_tabs").max_width(tabs_width).auto_shrink([true, true]);
                    if let Some(x) = ui.data_mut(|d| d.remove_temp::<f32>(scroll_id.with("to"))).or(start) {
                        area = area.horizontal_scroll_offset(x);
                    }
                    let out = area.show(ui, |ui| {
                        ui.horizontal_centered(|ui| {
                            // Each document tab's index and rect, and the one being dragged
                            // (with whether it was just released).
                            let mut slots = Vec::with_capacity(app.views.len());
                            let mut dragged = None;
                            // Mirrored, the tabs read from the right: the first is beside Home.
                            let mut order = app.tabs_in(Place::Main);
                            if mirrored {
                                order.reverse();
                                combine_tab(app, ui, &t, cap, tab_h, changed);
                            }
                            for i in order {
                                let Some(doc) = app.session.get(app.views[i].id) else { continue };
                                let Some(name) = crate::canvas::tab_label(app, i) else { continue };
                                let dirty = doc.dirty;
                                // A tab showing the document's title names its file on hover.
                                let file = (name != doc.name).then_some(doc.name.as_str());
                                let response = tab(
                                    ui,
                                    TabSpec {
                                        id: ui.scope_id().with(("document-tab", app.views[i].id)),
                                        theme: &t,
                                        icon: "file-text",
                                        name: &name,
                                        file,
                                        dirty,
                                        active: app.pane_front(Place::Main) == Some(i),
                                        index: i,
                                        cap,
                                        height: tab_h,
                                        draggable: true,
                                    },
                                    &mut close,
                                );
                                slots.push((i, response.rect));
                                if response.dragged() || response.drag_stopped() {
                                    dragged = Some((i, response.drag_stopped()));
                                }
                                if changed && app.active == Some(i) && !app.combine_showing() {
                                    response.scroll_to_me(Some(Align::Center));
                                }
                                response.context_menu(|ui| tab_menu(app, ui, i, &mut action));
                                // Double-clicking the tab's name renames its file, as in Finder and File Explorer.
                                if response.double_clicked() && cfg!(not(target_arch = "wasm32")) && doc.path.is_some() {
                                    action = Some(crate::tab_context::Action::Rename(app.views[i].id));
                                }
                                if response.clicked() {
                                    app.active = Some(i);
                                }
                                // A middle click closes the tab, as in Acrobat and browsers (#1853);
                                // unsaved changes ask first, as the close button does.
                                if response.middle_clicked() {
                                    close = Some(i);
                                }
                            }
                            drag_x = tab_drag(app, ui, &slots, dragged, Place::Main, close.is_some() || action.is_some(), mirrored);
                            if let Some(i) = close
                                && let Some(view) = app.views.get(i)
                            {
                                action.get_or_insert(Action::Close(view.view_key()));
                            }
                            if let Some(action) = action {
                                app.tab_action(action);
                            }
                            if !mirrored {
                                combine_tab(app, ui, &t, cap, tab_h, changed);
                            }
                        });
                    });
                    let max = (out.content_size.x - out.inner_rect.width()).max(0.0);
                    ui.data_mut(|d| d.insert_temp(scroll_id, (out.state.clamped_offset().x, max)));
                    // The strip just started to scroll: the next frame takes it to its reading start.
                    if mirrored_start(mirrored, max, started).0.is_some() {
                        ui.ctx().request_repaint();
                    }
                    // A tab dragged to either edge of a strip that scrolls brings the hidden tabs in.
                    if let Some(x) = drag_x
                        && max > 0.0
                    {
                        let step = if x < out.inner_rect.left() + TAB_DRAG_EDGE {
                            -TAB_DRAG_STEP
                        } else if x > out.inner_rect.right() - TAB_DRAG_EDGE {
                            TAB_DRAG_STEP
                        } else {
                            0.0
                        };
                        if step != 0.0 {
                            ui.data_mut(|d| d.insert_temp(scroll_id.with("to"), (out.state.clamped_offset().x + step).clamp(0.0, max)));
                            ui.ctx().request_repaint();
                        }
                    }
                };
                if mirrored {
                    // egui scrolls only content that runs left to right, so mirrored the tabs sit in
                    // a box that runs that way, exactly as wide as they are, beside Home.
                    let width = tabs_shown_width(&natural, cap, gap).min(tabs_width);
                    ui.allocate_ui_with_layout(vec2(width, ui.available_height()), Layout::left_to_right(Align::Center), show_tabs);
                } else {
                    ui.scope(show_tabs);
                }
                if mirrored {
                    left_clicked = scroll_left(ui);
                } else {
                    right_clicked = scroll_right(ui);
                }
                let mut scroll_to = None;
                if left_clicked {
                    scroll_to = Some((offset - (TAB_MIN_WIDTH + gap)).max(0.0));
                }
                if right_clicked {
                    scroll_to = Some((offset + TAB_MIN_WIDTH + gap).min(max_offset));
                }
                if let Some(x) = scroll_to {
                    ui.data_mut(|d| d.insert_temp(scroll_id.with("to"), x));
                    ui.ctx().request_repaint();
                }
                ui.add_space(4.0);
                let tip = crate::commands::command_tip(ui.ctx(), tl!("Open a PDF ({key})"), "file.open");
                if widgets::ghost_button_sized(ui, "plus", tl!("Open"), tab_h).on_hover_text(tip).clicked() {
                    app.open_dialog();
                }
                ui.with_layout(far_end, |ui| {
                    if !controls.right.is_empty() {
                        // Laid out left to right inside the right-to-left run, so they read in
                        // the desktop's order.
                        let size = egui::vec2(window_controls::width(&controls.right) - window_controls::SIDE_GAP, window_controls::BUTTON);
                        ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            window_controls::buttons(ui, &controls.right);
                        });
                        ui.add_space(window_controls::SIDE_GAP - 4.0);
                    }
                    if icons::button(ui, "settings", button, false, tl!("Preferences")).clicked() {
                        app.execute("app.preferences");
                    }
                    let (icon, label) = match app.theme_preference {
                        ThemePreference::System => ("monitor", tl!("Use system setting")),
                        ThemePreference::Light => ("sun", tl!("Light gray")),
                        ThemePreference::Dark => ("moon", tl!("Dark gray")),
                    };
                    let tip = format!("{}: {label}", tl!("Display theme"));
                    let response = icons::button(ui, icon, button, false, &tip);
                    // A click cycles System, Light, Dark; a right-click opens the menu.
                    if response.clicked() {
                        app.execute(match app.theme_preference {
                            ThemePreference::System => "view.theme.light",
                            ThemePreference::Light => "view.theme.dark",
                            ThemePreference::Dark => "view.theme.system",
                        });
                    }
                    egui::Popup::context_menu(&response).id(response.id.with("theme_menu")).show(|ui| appearance_menu(app, ui));
                    if icons::button(ui, "circle-help", button, false, tl!("Keyboard shortcuts")).clicked() {
                        app.dialog = Some(Dialog::Shortcuts);
                    }
                    // One click to the community, from anywhere in the app.
                    if widgets::ghost_button_sized(ui, "messages-square", "Discord", tab_h).on_hover_text(pdfcraft_engine::links::DISCORD).clicked() {
                        app.execute("help.discord");
                    }
                });
            });
        });
}

/// The Combine files tab, when it is open: after the document tabs in reading order (drawn first
/// when they read from the right). Its index can't clash with theirs.
fn combine_tab(app: &mut PdfCraftApp, ui: &mut egui::Ui, t: &Tokens, cap: Option<f32>, height: f32, changed: bool) {
    if !app.combine_tab.open {
        return;
    }
    let mut close = None;
    let response = tab(
        ui,
        TabSpec {
            id: ui.scope_id().with("combine-tab"),
            theme: t,
            icon: "files",
            name: tl!("Combine files"),
            file: None,
            dirty: false,
            active: app.combine_showing(),
            index: usize::MAX,
            cap,
            height,
            draggable: false,
        },
        &mut close,
    );
    if changed && app.combine_showing() {
        response.scroll_to_me(Some(Align::Center));
    }
    if response.clicked() {
        app.open_combine_tab();
    }
    // A middle click closes it too, as it does a document tab (#1853).
    if close.is_some() || response.middle_clicked() {
        app.close_combine_tab();
    }
}

/// The right-click menu on the title bar's theme button.
fn appearance_menu(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    if ui.button(tl!("Appearance settings…")).clicked() {
        app.dialog = Some(Dialog::Preferences);
        ui.close();
    }
}

/// A document tab's right-click menu, in any strip: the choice is left in `action`.
fn tab_menu(app: &PdfCraftApp, ui: &mut egui::Ui, index: usize, action: &mut Option<Action>) {
    let Some(view) = app.views.get(index) else { return };
    let (tab, id) = (view.view_key(), view.id);
    if ui.button(tl!("Close Tab")).clicked() {
        *action = Some(Action::Close(tab));
        ui.close();
    }
    if ui.add_enabled(!app.closed_tabs.is_empty(), egui::Button::new(tl!("Reopen Closed Tab"))).clicked() {
        *action = Some(Action::Reopen);
        ui.close();
    }
    ui.separator();
    // Split view (view.split-window).
    if ui.button(tl!("Split right")).clicked() {
        *action = Some(Action::Split { tab, down: false });
        ui.close();
    }
    if ui.button(tl!("Split down")).clicked() {
        *action = Some(Action::Split { tab, down: true });
        ui.close();
    }
    ui.separator();
    let reveal = cfg!(not(target_arch = "wasm32")) && app.session.get(id).is_some_and(|d| d.path.is_some());
    let label = if cfg!(target_os = "macos") { tl!("Reveal in Finder") } else { tl!("Reveal in File Explorer") };
    if ui.add_enabled(reveal, egui::Button::new(label)).clicked() {
        *action = Some(Action::Reveal(id));
        ui.close();
    }
    if ui.add_enabled(reveal, egui::Button::new(tl!("Rename…"))).clicked() {
        *action = Some(Action::Rename(id));
        ui.close();
    }
}

/// A tab of `place`'s strip (its tabs `slots`) being dragged (`dragged`: its index, and whether it
/// was let go this frame). It moves the tab along the strip (#1537): a line marks the gap it will
/// land in, and letting go drops it there. Over the right half of a page area that isn't split, or
/// over split view's other pane, a hint shows where it would go instead, and letting go puts it
/// there (`crate::panes`). `busy` is a close or menu choice this frame, which wins. Returns the
/// pointer's x while the tab moves along the strip, for scrolling at the strip's edges.
fn tab_drag(
    app: &mut PdfCraftApp,
    ui: &egui::Ui,
    slots: &[(usize, Rect)],
    dragged: Option<(usize, bool)>,
    place: Place,
    busy: bool,
    mirrored: bool,
) -> Option<f32> {
    let (from, released) = dragged?;
    let pos = ui.ctx().pointer_latest_pos()?;
    match app.drop_target(pos, place) {
        TabDrop::Strip => {
            let x = pos.x;
            // Reading order, with x flipped when the tabs read from the right.
            let slots = reading_order(slots, mirrored);
            let flip = |x: f32| if mirrored { -x } else { x };
            let to = drop_index(&slots, from, flip(x));
            if released {
                if !busy {
                    app.move_tab_in_strip(from, &slots, to);
                }
                return None;
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let Some(marker) = drop_marker(&slots, from, to, ui.spacing().item_spacing.x).map(flip) {
                let t = Tokens::get(ui.ctx());
                let span = slots.first().map_or(ui.max_rect().y_range(), |(_, r)| r.y_range());
                // In front of the first tab the gap lies outside the scroll area's clip rect
                // (#2225): keep the whole line inside it.
                let clip = ui.clip_rect();
                let marker = marker.clamp(clip.left() + 1.0, (clip.right() - 1.0).max(clip.left() + 1.0));
                ui.painter().vline(marker, span.shrink(4.0), Stroke::new(2.0, t.accent));
            }
            Some(x)
        }
        target => {
            if !released {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                app.panes.drop_hint = app.drop_hint_rect(target);
            } else if !busy {
                app.drop_tab(from, target);
            }
            None
        }
    }
}

/// The tab strip of split view's second pane, in `strip`: its tabs share the width (no scrolling),
/// and are clicked, closed, moved along and dragged to the other pane as in the main strip.
pub(crate) fn pane_tab_strip(app: &mut PdfCraftApp, ui: &mut egui::Ui, strip: Rect, place: Place) {
    let t = Tokens::get(ui.ctx());
    ui.painter().rect_filled(strip, 0.0, t.titlebar);
    ui.painter().hline(strip.x_range(), strip.bottom() - 0.5, Stroke::new(1.0, t.divider));
    let tabs = app.tabs_in(place);
    let front = app.pane_front(place);
    let inner = Rect::from_min_max(strip.min + vec2(6.0, 2.0), strip.max - vec2(6.0, 0.0));
    let mut row = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Max)).id_salt(("tab-row", place)));
    row.spacing_mut().item_spacing.x = 4.0;
    let natural: Vec<f32> = tabs.iter().filter_map(|&i| crate::canvas::tab_label(app, i)).map(|name| tab_natural_width(&row, &t, &name)).collect();
    let cap = tab_cap(&natural, row.available_width(), 4.0, TAB_MIN_WIDTH);
    let mut close = None;
    let mut action = None;
    let mut slots = Vec::with_capacity(tabs.len());
    let mut dragged = None;
    for &i in &tabs {
        let Some(view) = app.views.get(i) else { continue };
        let Some(doc) = app.session.get(view.id) else { continue };
        let Some(name) = crate::canvas::tab_label(app, i) else { continue };
        let dirty = doc.dirty;
        let file = (name != doc.name).then(|| doc.name.clone());
        let id = row.scope_id().with(("pane-tab", view.view_key()));
        let spec = TabSpec {
            id,
            theme: &t,
            icon: "file-text",
            name: &name,
            file: file.as_deref(),
            dirty,
            active: front == Some(i),
            index: i,
            cap,
            height: inner.height(),
            draggable: true,
        };
        let response = tab(&mut row, spec, &mut close);
        slots.push((i, response.rect));
        if response.dragged() || response.drag_stopped() {
            dragged = Some((i, response.drag_stopped()));
        }
        response.context_menu(|ui| tab_menu(app, ui, i, &mut action));
        if response.clicked() {
            app.active = Some(i);
            app.combine_tab.focused = false;
        }
        // A middle click closes the tab, as in the main strip (#1853).
        if response.middle_clicked() {
            close = Some(i);
        }
    }
    tab_drag(app, &row, &slots, dragged, place, close.is_some() || action.is_some(), false);
    if let Some(i) = close
        && let Some(view) = app.views.get(i)
    {
        action.get_or_insert(Action::Close(view.view_key()));
    }
    if let Some(action) = action {
        app.tab_action(action);
    }
}

/// A tab's chrome around its name: the icon on the left, the close button on the right.
const TAB_CHROME: f32 = 64.0;
/// The narrowest a tab shrinks to before the strip scrolls: room for most of a name.
const TAB_MIN_WIDTH: f32 = 180.0;
/// The arrows either side of tabs that don't fit.
const TAB_ARROW: f32 = 24.0;

/// Whether tabs of `natural` widths, `gap` apart, are wider than `budget` even at `min` each.
fn tabs_overflow(natural: &[f32], budget: f32, gap: f32, min: f32) -> bool {
    let narrowest: f32 = natural.iter().map(|w| w.min(min)).sum();
    narrowest + gap * natural.len().saturating_sub(1) as f32 > budget
}

/// How close to either end of a scrolling strip a dragged tab scrolls it, and how far per frame.
const TAB_DRAG_EDGE: f32 = 24.0;
const TAB_DRAG_STEP: f32 = 8.0;

/// Where the tab at `from` lands when dropped with the pointer at `x`: past every other tab
/// whose centre is left of the pointer. `slots` are the tabs' indices and rects, in order.
fn drop_index(slots: &[(usize, Rect)], from: usize, x: f32) -> usize {
    slots.iter().filter(|(i, r)| *i != from && r.center().x < x).count()
}

/// The x of the line marking where the tab at `from` drops at `to`: in the gap before the other
/// tab it lands in front of, or after the last. `None` when it would stay where it is.
fn drop_marker(slots: &[(usize, Rect)], from: usize, to: usize, gap: f32) -> Option<f32> {
    if slots.iter().position(|(i, _)| *i == from) == Some(to) {
        return None;
    }
    let mut others = slots.iter().filter(|(i, _)| *i != from).map(|(_, r)| r);
    match to.checked_sub(1) {
        None => others.next().map(|r| r.left() - gap / 2.0),
        Some(before) => others.nth(before).map(|r| r.right() + gap / 2.0),
    }
}

/// The tabs' slots in reading order, in a space where reading runs left to right: when the tabs
/// read from the right (`mirrored`), their order is reversed and x flipped, so `drop_index` and
/// `drop_marker` serve both directions.
fn reading_order(slots: &[(usize, Rect)], mirrored: bool) -> Vec<(usize, Rect)> {
    if !mirrored {
        return slots.to_vec();
    }
    slots.iter().rev().map(|(i, r)| (*i, Rect::from_x_y_ranges(-r.right()..=-r.left(), r.y_range()))).collect()
}

/// Where a mirrored strip that has just started to scroll goes (`max_offset`, the far end of a
/// scroll area that runs left to right, is where its tabs start to read), and whether it has gone
/// there. Once only, so the user's own scrolling stands; a strip that stops scrolling starts again.
fn mirrored_start(mirrored: bool, max_offset: f32, started: bool) -> (Option<f32>, bool) {
    if !mirrored || max_offset.is_nan() || max_offset <= 0.5 {
        return (None, false);
    }
    if started { (None, true) } else { (Some(max_offset), true) }
}

/// How wide tabs of `natural` widths, each at most `cap`, are laid out `gap` apart.
fn tabs_shown_width(natural: &[f32], cap: Option<f32>, gap: f32) -> f32 {
    let tabs: f32 = natural.iter().map(|w| cap.map_or(*w, |cap| w.min(cap))).sum();
    tabs + gap * natural.len().saturating_sub(1) as f32
}

/// A scroll arrow beside the tabs; dimmed (and inert) at its end. True when clicked.
fn arrow(ui: &mut egui::Ui, icon: &str, enabled: bool, tip: &str) -> bool {
    ui.add_enabled_ui(enabled, |ui| icons::button(ui, icon, TAB_ARROW, false, tip)).inner.clicked()
}

/// A tab name, at most 28 characters (longer names end in "…").
fn tab_name(name: &str) -> String {
    if name.chars().count() > 28 { format!("{}…", name.chars().take(27).collect::<String>()) } else { name.to_string() }
}

/// The width a tab takes when nothing has to shrink.
fn tab_natural_width(ui: &egui::Ui, t: &Tokens, name: &str) -> f32 {
    let label = tab_name(name);
    ui.fonts_mut(|f| f.layout_no_wrap(label, theme::regular(13.0), t.text).size().x) + TAB_CHROME
}

/// The widest any tab may be so that tabs of `natural` widths, `gap` apart, fit in `budget`:
/// narrow tabs keep their width and the rest share what is left equally (as browsers do).
/// `None` when every tab fits as it is; never below `min` (then the strip scrolls).
fn tab_cap(natural: &[f32], budget: f32, gap: f32, min: f32) -> Option<f32> {
    let n = natural.len();
    if n == 0 {
        return None;
    }
    let mut left = budget - gap * (n - 1) as f32;
    if natural.iter().sum::<f32>() <= left {
        return None;
    }
    let mut sorted = natural.to_vec();
    sorted.sort_by(f32::total_cmp);
    for (k, w) in sorted.iter().enumerate() {
        let share = left / (n - k) as f32;
        if *w > share {
            return Some(share.max(min));
        }
        left -= w;
    }
    None
}

/// Draw one tab with its icon, elided label, close button and unsaved dot; records a close request and
/// returns the tab response for drag handling.
struct TabSpec<'a> {
    id: egui::Id,
    theme: &'a Tokens,
    icon: &'a str,
    name: &'a str,
    file: Option<&'a str>,
    dirty: bool,
    active: bool,
    index: usize,
    cap: Option<f32>,
    height: f32,
    draggable: bool,
}

fn tab(ui: &mut egui::Ui, spec: TabSpec<'_>, close: &mut Option<usize>) -> egui::Response {
    let TabSpec { id, theme: t, icon, name, file, dirty, active, index, cap, height, draggable } = spec;
    let font = theme::regular(13.0);
    let mut label = tab_name(name);
    // A shrunk tab shows as much of its name as fits (the tooltip has all of it).
    if let Some(cap) = cap {
        let room = (cap - TAB_CHROME).max(0.0);
        label = crate::panels::ellipsized_prefix(&label, |s| crate::bidi::layout_ltr(ui, s.to_owned(), font.clone(), t.text).size().x <= room);
    }
    // Logical text, laid out explicitly left to right: the fork shapes and reorders it once,
    // and an Arabic file name keeps its extension at the visual end. AccessKit names stay logical.
    let label_color = if active { t.text } else { t.text_muted };
    let label_galley = crate::bidi::layout_ltr(ui, label.clone(), font.clone(), label_color);
    let text_w = label_galley.size().x;
    let width = cap.map_or(text_w + TAB_CHROME, |cap| (text_w + TAB_CHROME).min(cap));
    // Full strip height so the active tab sits on the mode bar instead of floating in the title bar.
    let height = ui.available_height().max(height);
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let a11y = if dirty { format!("{name} (edited)") } else { name.to_string() };
    let sense = if draggable { Sense::click_and_drag() } else { Sense::click() };
    let resp = craft_ui::tabs::Tab::new(id, &a11y, active).sense(sense).focus_stroke(Stroke::new(1.0, t.accent)).show_at(ui, rect, |ui, resp| {
        let press = theme::Press::track(ui, resp);
        let bg = if active {
            press.fill(t.chrome, t.chrome.lerp_to_gamma(t.hover, 0.65), t.hover)
        } else {
            press.fill(Color32::TRANSPARENT, t.hover, t.pressed)
        };
        if bg.a() > 0 {
            ui.painter().rect_filled(rect, CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 }, bg);
        }
        icons::paint(
            ui,
            Rect::from_min_size(rect.min + vec2(6.0, (height - 16.0) / 2.0), vec2(16.0, 16.0)),
            icon,
            15.0,
            if active { t.accent } else { t.text_muted },
        );
        ui.painter().galley(rect.min + vec2(28.0, (rect.height() - label_galley.size().y) / 2.0), label_galley.clone(), label_color);
    });
    let x_rect = Rect::from_center_size(rect.right_center() - vec2(16.0, 0.0), vec2(20.0, 20.0));
    let x = ui.interact(x_rect, id.with("close"), Sense::click());
    let x_press = theme::Press::track(ui, &x);
    x_press.wash(ui, x_rect, 4, false);
    // Unsaved changes: a dot where the close button sits, until the tab is hovered.
    if dirty && !resp.hovered() && !x.hovered() {
        ui.painter().circle_filled(x_rect.center(), 4.0, if active { t.text } else { t.text_muted });
    } else if active || resp.hovered() || x.hovered() {
        icons::paint(ui, x_rect.translate(x_press.offset()), "x", 13.0, t.text_muted);
    }
    if theme::hand(x).clicked() {
        *close = Some(index);
    }
    let shown = match file {
        Some(file) => format!("{name}\n{file}"),
        None => name.to_string(),
    };
    theme::hand(resp).on_hover_text(if dirty { crate::i18n::fmt(tl!("{name} — unsaved changes"), &[("name", &shown)]) } else { shown })
}

/// Width the print, save and info buttons and their gaps take in the mode bar: three buttons, the
/// 4 pt `item_spacing` after each of them (three gaps, not two), and the 8 pt spacer.
const MODE_BAR_RIGHT: f32 = 3.0 * 32.0 + 3.0 * 4.0 + 8.0;
/// Narrowest search field the mode bar shows; below it the field becomes an icon button.
const MODE_BAR_SEARCH_MIN: f32 = 140.0;

pub fn mode_bar(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    // The document in the active tab may hide the menu or the tools (`crate::viewer_prefs`).
>>>>>>> upstream/main
    let hidden = app.hidden_interface();
    let modes_visible = !hidden.mode_bar(app.native_menu.is_some());
    let window_ui = !hidden.window_ui;
    let fallback_rail = window_ui && side_panels::rail_anchor(app).is_none();
    let out = AppShell::new(WindowKey::main())
        .toast_alignment(egui::Align2::CENTER_BOTTOM)
        .screen_mode(Bind::given(app.screen_mode))
        // A presentation shows the page and nothing else, so no reveal brings the bars back.
        .allow_reveal(false)
        .title_bar(|ui, _, app: &mut PdfCraftApp| title_bar(app, ui))
        .top(Slot::new("modes").visible(modes_visible), |ui, _, app: &mut PdfCraftApp| mode_bar::show(app, ui))
        .start(
            Slot::new("view-rail-start").visible(fallback_rail && app.rail_side == craft::behavior::DockSide::Start),
            |ui, _, app: &mut PdfCraftApp| view_rail::show(app, ui),
        )
        .end(Slot::new("view-rail").visible(fallback_rail && app.rail_side == craft::behavior::DockSide::End), |ui, _, app: &mut PdfCraftApp| {
            view_rail::show(app, ui)
        })
        .body(|ui, _, app: &mut PdfCraftApp| window_body(app, ui))
        .show(ui, app);
    // PLAN: assumed `set_screen_mode(&mut self, ScreenMode)` (pdf-shell-w8)
    app.set_screen_mode(out.screen_mode);
}

/// The dock, under the veil that shows files held over the window. Files dropped on the window open as
/// documents; the Combine files tab takes its own through its veil.
fn window_body(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let mut dropped = None;
    let organize = app.active_ids().is_some_and(|(index, id)| {
        app.session.get(id).is_some_and(|doc| doc.allows_assembly()) && app.views.get(index).is_some_and(|view| view.organize)
    });
    let veil = DropVeil::new(tl!("Drop to open the files"));
    let veil = if organize { veil.positional() } else { veil };
    let _ = veil.show(ui, |ui, files| {
        if let Some(files) = files {
            dropped = Some(files.files.clone());
        }
        side_panels::dock(app, ui);
    });
    if let Some(files) = dropped
        && !app.combine_showing()
    {
        // While the page grid shows, it takes files dropped anywhere in the window: one let go off the grid goes
        // in after the last page, as there is no gap under the pointer to say where.
        let grid = app.active_ids().filter(|(_, id)| app.session.get(*id).is_some_and(|doc| doc.allows_assembly())).map(|(index, _)| index);
        match grid.and_then(|index| app.views.get_mut(index)).filter(|view| view.organize) {
            Some(view) => view.dropped_files = Some((usize::MAX, files)),
            None => {
                let ctx = ui.ctx().clone();
                app.open_dropped(&ctx, files);
            }
        }
    }
}

/// The title bar: Home, the document tabs and Open, then the community, shortcuts, theme and
/// Preferences buttons. The window's caption buttons are the bar's own.
fn title_bar(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let (theme_icon, theme_label) = match app.theme_preference {
        ModeChoice::System => ("monitor", tl!("Use system setting")),
        ModeChoice::Light => ("sun", tl!("Light gray")),
        ModeChoice::Dark => ("moon", tl!("Dark gray")),
    };
    // The button names the choice it shows; a click steps to the next one.
    let theme_label = format!("{}: {theme_label}", tl!("Display theme"));
    let out = TitleBar::new()
        .trailing(|ui, _| {
            Button::command("help.discord").label("Discord").icon(IconId::new("messages-square")).tooltip(tl!("Join the ArtCraft Discord")).show(ui);
            IconButton::command(IconId::new("circle-help"), "help.shortcuts").show(ui);
            let theme_button = IconButton::command(IconId::new(theme_icon), ThemeChoice::NextMode.command()).label(theme_label.clone()).show(ui);
            let opener = theme_button.response().clone();
            Menu::context(tl!("Appearance"), &opener).show(ui, |ui, _| {
                let appearance = CommandRef::with_params("app.preferences", serde_json::json!({ "tab": "appearance" }));
                MenuItem::command(appearance).label(tl!("Appearance settings…")).show(ui);
            });
            IconButton::command(IconId::new("settings"), "app.preferences").show(ui);
        })
        .show(ui, |ui, _| {
            let home = app.active.is_none() && !app.combine_showing();
            if IconButton::new(IconId::new(HOME_ICON)).label(tl!("Home")).selected(home).show(ui).clicked() {
                app.active = None;
                app.combine_tab.focused = false;
            }
            doc_tabs::strip(app, ui, Place::Main);
        })
        .output;
    if out.close_requested {
        app.request_quit(&ctx);
    }
}

/// What the rail shows under the page box: the page count, or, when the current page's label is
/// not its physical number (front matter numbered i, ii, …), its position as "3/180" (#2834).
fn page_position(current: usize, count: usize, label: &str) -> String {
    let number = current.saturating_add(1).to_string();
    if label.is_empty() || label == number || current >= count { count.to_string() } else { format!("{number}/{count}") }
}

#[cfg(test)]
mod page_position_tests {
    use super::page_position;

    #[test]
    fn position_shows_only_when_the_label_is_not_the_page_number() {
        assert_eq!(page_position(0, 180, "i"), "1/180");
        assert_eq!(page_position(2, 180, "1"), "3/180");
        assert_eq!(page_position(2, 180, "3"), "180");
        assert_eq!(page_position(2, 180, ""), "180");
        assert_eq!(page_position(0, 0, "i"), "0");
        assert_eq!(page_position(usize::MAX, 3, "x"), "3");
    }
}
<<<<<<< HEAD
=======

#[cfg(test)]
mod tab_widths {
    use super::{MAC_CONTENT_LEFT, MAC_LIGHT, MAC_LIGHT_PITCH, MAC_LIGHTS_LEFT, MAC_TITLE_BAR, tab_cap};

    /// Tabs that fit keep their own widths.
    #[test]
    fn tabs_that_fit_do_not_shrink() {
        assert_eq!(tab_cap(&[200.0, 150.0], 400.0, 4.0, 110.0), None);
        assert_eq!(tab_cap(&[], 0.0, 4.0, 110.0), None);
    }

    /// Too wide: narrow tabs keep their width, the wide ones share the rest equally, and
    /// together they fill the strip exactly.
    #[test]
    fn wide_tabs_share_what_narrow_ones_leave() {
        let natural = [120.0, 300.0, 300.0];
        let cap = tab_cap(&natural, 500.0, 4.0, 110.0).expect("must shrink");
        let used: f32 = natural.iter().map(|w| w.min(cap)).sum::<f32>() + 2.0 * 4.0;
        assert!((used - 500.0).abs() < 0.01, "{cap} uses {used}");
        assert!((cap - 186.0).abs() < 0.01, "{cap}");
    }

    /// Arrows appear only when the tabs can't fit even at their narrowest.
    #[test]
    fn arrows_only_when_the_narrowest_tabs_overflow() {
        use super::tabs_overflow;
        assert!(!tabs_overflow(&[250.0; 3], 600.0, 4.0, 180.0), "three shrunk tabs fit");
        assert!(tabs_overflow(&[250.0; 4], 600.0, 4.0, 180.0), "four don't, even at 180");
        assert!(!tabs_overflow(&[100.0; 5], 600.0, 4.0, 180.0), "narrow tabs fit as they are");
        assert!(!tabs_overflow(&[], 0.0, 4.0, 180.0));
    }

    /// Tabs never get narrower than the minimum: past it, the strip scrolls instead.
    #[test]
    fn tabs_stop_at_their_minimum() {
        assert_eq!(tab_cap(&[250.0; 12], 600.0, 4.0, 110.0), Some(110.0));
        assert_eq!(tab_cap(&[250.0; 3], 0.0, 4.0, 110.0), Some(110.0));
    }

    /// Three 100-wide tabs, 4 apart, starting at x = 0.
    fn three_slots() -> Vec<(usize, egui::Rect)> {
        (0..3).map(|i| (i, egui::Rect::from_min_size(egui::pos2(i as f32 * 104.0, 0.0), egui::vec2(100.0, 30.0)))).collect()
    }

    /// A dragged tab lands past every other tab whose centre the pointer has crossed.
    #[test]
    fn a_dragged_tab_drops_past_the_centres_it_crossed() {
        use super::drop_index;
        let slots = three_slots();
        // Centres at 50, 154 and 258.
        assert_eq!(drop_index(&slots, 0, 10.0), 0, "still over itself");
        assert_eq!(drop_index(&slots, 0, 160.0), 1, "past the middle tab's centre");
        assert_eq!(drop_index(&slots, 0, 900.0), 2, "past the end");
        assert_eq!(drop_index(&slots, 2, -50.0), 0, "before the start");
        assert_eq!(drop_index(&slots, 2, 100.0), 1, "between the first and middle centres");
        assert_eq!(drop_index(&[], 0, 100.0), 0);
    }

    /// The drop line sits in the gap the tab will land in, and not at all where it already is.
    #[test]
    fn the_drop_line_marks_the_gap_and_hides_when_nothing_moves() {
        use super::drop_marker;
        let slots = three_slots();
        assert_eq!(drop_marker(&slots, 0, 0, 4.0), None);
        assert_eq!(drop_marker(&slots, 1, 1, 4.0), None);
        // The first tab dropped at the end: after the last tab.
        assert_eq!(drop_marker(&slots, 0, 2, 4.0), Some(310.0));
        // The first tab dropped between the middle and the last: in the gap between them.
        assert_eq!(drop_marker(&slots, 0, 1, 4.0), Some(206.0));
        // The last tab dropped at the start: before the first tab.
        assert_eq!(drop_marker(&slots, 2, 0, 4.0), Some(-2.0));
        assert_eq!(drop_marker(&[], 0, 1, 4.0), None);
    }

    /// Mirrored, the slots come back in reading order with x flipped, so the drop helpers read
    /// the tabs from the right.
    #[test]
    fn mirrored_tabs_drop_in_their_reading_order() {
        use super::{drop_index, drop_marker, reading_order};
        // Drawn right to left: tab 0 at the right, tab 2 at the left.
        let drawn: Vec<_> = three_slots().into_iter().map(|(k, r)| (2 - k, r)).collect();
        let slots = reading_order(&drawn, true);
        assert_eq!(slots.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1, 2]);
        // Tab 0 (drawn at 208..308, beside Home) dragged to the far left lands at the end.
        assert_eq!(drop_index(&slots, 0, -10.0), 2);
        // Its drop line, flipped back, sits left of the leftmost tab.
        assert_eq!(drop_marker(&slots, 0, 2, 4.0).map(|x| -x), Some(-2.0));
        // Unmirrored, nothing changes.
        assert_eq!(reading_order(&three_slots(), false), three_slots());
        assert!(reading_order(&[], true).is_empty());
    }

    /// A mirrored strip goes to its reading start (the far end) once when it starts to scroll,
    /// then leaves the offset to the user until it no longer scrolls.
    #[test]
    fn a_mirrored_strip_starts_at_its_reading_start_once() {
        use super::mirrored_start;
        assert_eq!(mirrored_start(true, 0.0, false), (None, false), "nothing to scroll");
        assert_eq!(mirrored_start(true, 240.0, false), (Some(240.0), true), "it just started to scroll");
        assert_eq!(mirrored_start(true, 240.0, true), (None, true), "once only");
        assert_eq!(mirrored_start(true, 0.0, true), (None, false), "it stopped scrolling: next time starts again");
        assert_eq!(mirrored_start(false, 240.0, false), (None, false), "unmirrored, the start is offset 0");
        assert_eq!(mirrored_start(true, f32::NAN, false), (None, false));
    }

    #[test]
    fn shown_tabs_take_their_capped_widths_and_the_gaps() {
        use super::tabs_shown_width;
        assert_eq!(tabs_shown_width(&[100.0, 200.0], None, 4.0), 304.0);
        assert_eq!(tabs_shown_width(&[100.0, 200.0], Some(150.0), 4.0), 254.0);
        assert_eq!(tabs_shown_width(&[], None, 4.0), 0.0);
    }

    #[test]
    fn the_mac_title_bar_centres_and_clears_the_traffic_lights() {
        // The lights are centred in the bar, and content keeps the same gap after them (#789).
        let margin = (MAC_TITLE_BAR - MAC_LIGHT) / 2.0;
        let lights_end = MAC_LIGHTS_LEFT + 2.0 * MAC_LIGHT_PITCH + MAC_LIGHT;
        assert_eq!(MAC_CONTENT_LEFT - lights_end, margin);
        // `egui::Margin` is in `i8`.
        assert!(MAC_CONTENT_LEFT <= f32::from(i8::MAX));
    }
}
>>>>>>> upstream/main
