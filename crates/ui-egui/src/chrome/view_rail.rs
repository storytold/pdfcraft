//! The view rail: the end slot's column of view controls (dock side, page display, rotation, fit, zoom, page
//! stepping, history and the page box) for the active document.

use craft::behavior::kurbo::Size;
use craft::behavior::{DockSide, IconId};
use craft::pages::PageDisplay;
use craft::theme::TypeRole;
use craft::ui::buttons::IconButton;
use craft::ui::contract::{Bind, PopupKind, Trigger};
use craft::ui::display::Text;
use craft::ui::menu::{Menu, MenuItem};
use craft::ui::text::{TextField, ZoomField, ZoomScale};
use egui::Ui;

use crate::PdfCraftApp;

/// Shows the rail's view cluster; nothing without an active document or a current page to view.
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut Ui) {
    let Some((index, id)) = app.active_ids() else { return };
    let Some(doc) = app.session.get(id) else { return };
    let page_count = doc.info.pages.len();
    let labels: Vec<String> = doc.info.pages.iter().map(|p| p.label.clone()).collect();
    let rail_on_right = app.rail_side == DockSide::End;
    let Some(view) = app.views.get_mut(index) else { return };
    let current = view.current();
    // Without a current page there is nothing to view or zoom.
    let Some(page) = doc.info.pages.get(current) else { return };
    let page_size = Size::new(f64::from(page.width.max(1.0)), f64::from(page.height.max(1.0)));

    IconButton::command(IconId::new(if rail_on_right { "panel_left" } else { "panel_right" }), "view.dock_side").show(ui);

    // The rows' marks come from the commands' checked state (the catalog), not from here.
    let display_icon = if view.view.display == PageDisplay::Single { "file-text" } else { "columns-2" };
    let display = IconButton::new(IconId::new(display_icon)).label("Page display").popup(PopupKind::Menu).show(ui);
    Menu::new("Page display").opened_by(&display.response, Trigger::Press).show(ui, |ui, _| {
        for command in [
            "view.layout.single",
            "view.layout.two_up",
            "view.continuous",
            "view.page_gaps",
            "view.layout.cover",
            "view.fit_width_scrolling",
            "view.fit_one_page",
            "view.zoom.actual_size",
            "view.zoom.page_level",
            "view.fit_visible",
            "view.read_mode",
            "view.full_screen",
        ] {
            MenuItem::command(command).show(ui);
        }
    });

    IconButton::command(IconId::new("rotate_cw"), "view.rotate.cw").show(ui);
    IconButton::command(IconId::new("maximize_2"), "view.fit_toggle").show(ui);
    IconButton::command(IconId::new("zoom_out"), "view.zoom.out").show(ui);
    IconButton::command(IconId::new("zoom_in"), "view.zoom.in").show(ui);

    let field = ZoomField::new("Zoom", view.canvas_view(), page_size).scale(ZoomScale::Points).show(ui);
    if let Some(edited) = field.view.committed() {
        view.apply_canvas_view(edited);
    }

    IconButton::command(IconId::new("chevron_down"), "view.next_page").show(ui);
    IconButton::command(IconId::new("chevron_up"), "view.previous_page").show(ui);
    IconButton::command(IconId::new("chevron_right"), "view.next_view").show(ui);
    IconButton::command(IconId::new("chevron_left"), "view.previous_view").show(ui);

    let label = labels.get(current).map_or("", String::as_str);
    // Out of range (an empty document) reads "Page 0 of 0", never past the end.
    let shown = if current < page_count { current + 1 } else { page_count };
    let of = crate::i18n::fmt(crate::i18n::t("Page {p} of {n}"), &[("p", &shown.to_string()), ("n", &page_count.to_string())]);
    Text::new(super::page_position(current, page_count, label)).role(TypeRole::Caption).tooltip(of).show(ui);

    let current_label = view.current_page_label(labels.get(current).map(String::as_str));
    // Taken once, in the pass that shows the field, also while the field is not focused.
    let reselect = std::mem::take(&mut view.page_input_reselect);
    let edit = TextField::new("Current page", Bind::bound(&mut view.page_input))
        .id(view.page_input_id())
        .select_on_focus()
        .select_all(reselect)
        .submits()
        .show(ui);
    if edit.submitted {
        let typed = view.page_input.clone();
        view.go_to_typed(&typed, &labels);
        view.page_input = current_label.clone();
    }
    // Navigation updates the idle box, never the text being edited.
    if !edit.response.has_focus() {
        view.page_input = view.current_page_label(labels.get(view.current()).map(String::as_str));
    }
}
