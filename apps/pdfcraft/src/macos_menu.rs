//! Native macOS system menu bar (File, Edit, Pages, View, Tools, Window, Help) using `muda 0.21`.
//!
//! On macOS, desktop applications display a global native menu bar at the top of the screen.
//! This module builds the full hierarchy using muda and routes menu activations to
//! `PdfCraftApp::execute` via `OsEvent::Command`.
//!
//! `Quit PdfCraft` retains unsaved-changes protection by reusing `craft_quit::QUIT_MENU_ID`,
//! which routes through `craft-quit` rather than winit's default terminating quit.

use std::cell::RefCell;

use egui::Context;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use pdfcraft_engine::commands::Shortcut;

use crate::apple_events::AppleEvents;

thread_local! {
    static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
}

fn to_accelerator(s: Shortcut) -> Option<Accelerator> {
    let mut mods = Modifiers::empty();
    if s.command {
        mods |= Modifiers::META;
    }
    if s.shift {
        mods |= Modifiers::SHIFT;
    }
    if s.mac_ctrl {
        mods |= Modifiers::CONTROL;
    }
    let code = match s.key {
        "A" => Code::KeyA,
        "B" => Code::KeyB,
        "C" => Code::KeyC,
        "D" => Code::KeyD,
        "E" => Code::KeyE,
        "F" => Code::KeyF,
        "G" => Code::KeyG,
        "H" => Code::KeyH,
        "I" => Code::KeyI,
        "J" => Code::KeyJ,
        "K" => Code::KeyK,
        "L" => Code::KeyL,
        "M" => Code::KeyM,
        "N" => Code::KeyN,
        "O" => Code::KeyO,
        "P" => Code::KeyP,
        "Q" => Code::KeyQ,
        "R" => Code::KeyR,
        "S" => Code::KeyS,
        "T" => Code::KeyT,
        "U" => Code::KeyU,
        "V" => Code::KeyV,
        "W" => Code::KeyW,
        "X" => Code::KeyX,
        "Y" => Code::KeyY,
        "Z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        "," => Code::Comma,
        "." => Code::Period,
        "[" => Code::BracketLeft,
        "]" => Code::BracketRight,
        "+" | "=" => Code::Equal,
        "-" | "−" => Code::Minus,
        "Tab" => Code::Tab,
        "Delete" => Code::Delete,
        _ => return None,
    };
    Some(Accelerator::new(mods, code))
}

fn cmd_item(id: &'static str) -> MenuItem {
    if let Some(spec) = pdfcraft_engine::commands::command(id) {
        let label = pdfcraft_ui_egui::i18n::menu_label(id, spec.label);
        let shortcut = spec.shortcut.and_then(to_accelerator);
        MenuItem::with_id(id, label, true, shortcut)
    } else {
        log::error!("command id {id:?} not registered in COMMANDS");
        MenuItem::with_id(id, id, false, None)
    }
}

fn sub_menu(title: &'static str) -> Submenu {
    Submenu::new(pdfcraft_ui_egui::i18n::t(title), true)
}

pub fn install(ctx: &Context, apple_events: &AppleEvents, app_name: &str) -> muda::Result<()> {
    if MENU.with_borrow(Option::is_some) {
        return Ok(());
    }

    let menu = Menu::new();

    // 1. App menu (PdfCraft)
    let app_menu = Submenu::new(app_name, true);
    let quit_label = pdfcraft_ui_egui::i18n::fmt(pdfcraft_ui_egui::i18n::t("Quit {app}"), &[("app", app_name)]);
    let about_label = pdfcraft_ui_egui::i18n::fmt(pdfcraft_ui_egui::i18n::t("About {app}"), &[("app", app_name)]);
    let hide_label = pdfcraft_ui_egui::i18n::fmt(pdfcraft_ui_egui::i18n::t("Hide {app}"), &[("app", app_name)]);
    let hide_others_label = pdfcraft_ui_egui::i18n::t("Hide Others");
    let show_all_label = pdfcraft_ui_egui::i18n::t("Show All");

    let quit = MenuItem::with_id(craft_quit::QUIT_MENU_ID, quit_label, true, Some(Accelerator::new(Modifiers::META, Code::KeyQ)));
    app_menu.append_items(&[
        &PredefinedMenuItem::about(Some(&about_label), None),
        &PredefinedMenuItem::separator(),
        &cmd_item("app.preferences"),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::services(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::hide(Some(&hide_label)),
        &PredefinedMenuItem::hide_others(Some(hide_others_label)),
        &PredefinedMenuItem::show_all(Some(show_all_label)),
        &PredefinedMenuItem::separator(),
        &quit,
    ])?;

    // 2. File menu
    let file_menu = sub_menu("File");

    let open_recent_menu = sub_menu("Open Recent");
    open_recent_menu.append_items(&[&cmd_item("file.open_recent"), &PredefinedMenuItem::separator(), &cmd_item("file.clear_recent")])?;

    let create_menu = sub_menu("Create PDF");
    create_menu.append_items(&[
        &cmd_item("create.blank"),
        &PredefinedMenuItem::separator(),
        &cmd_item("create.file"),
        &cmd_item("create.multiple"),
        &cmd_item("create.multiple_separate"),
        &cmd_item("create.images"),
        &cmd_item("create.clipboard"),
    ])?;

    let export_menu = sub_menu("Export & Convert");
    export_menu.append_items(&[
        &cmd_item("export.image"),
        &cmd_item("export.all_images"),
        &PredefinedMenuItem::separator(),
        &cmd_item("export.docx"),
        &cmd_item("export.pptx"),
        &cmd_item("export.text"),
        &cmd_item("export.rtf"),
        &cmd_item("export.html"),
    ])?;

    let optimize_menu = sub_menu("Reduce & Optimize");
    optimize_menu.append_items(&[&cmd_item("optimize.reduce"), &cmd_item("optimize.advanced")])?;

    let security_menu = sub_menu("Password & Security");
    security_menu.append_items(&[&cmd_item("protect.password"), &cmd_item("protect.remove"), &cmd_item("protect.properties")])?;

    file_menu.append_items(&[
        &open_recent_menu,
        &PredefinedMenuItem::separator(),
        &create_menu,
        &cmd_item("page.combine"),
        &cmd_item("file.open"),
        &export_menu,
        &optimize_menu,
        &security_menu,
        &PredefinedMenuItem::separator(),
        &cmd_item("file.save"),
        &cmd_item("file.save_as"),
        &cmd_item("file.revert"),
        &PredefinedMenuItem::separator(),
        &cmd_item("print.dialog"),
        &cmd_item("file.properties"),
        &cmd_item("file.word_count"),
        &PredefinedMenuItem::separator(),
        &cmd_item("file.close"),
        &cmd_item("file.close_all"),
    ])?;

    // 3. Edit menu
    let edit_menu = sub_menu("Edit");
    edit_menu.append_items(&[
        &cmd_item("edit.undo"),
        &cmd_item("edit.redo"),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::cut(None),
        &PredefinedMenuItem::copy(None),
        &PredefinedMenuItem::paste(None),
        &PredefinedMenuItem::select_all(None),
        &PredefinedMenuItem::separator(),
        &cmd_item("edit.find"),
        &cmd_item("edit.advanced_search"),
    ])?;

    // 4. Pages menu
    let pages_menu = sub_menu("Pages");
    pages_menu.append_items(&[
        &cmd_item("page.organize"),
        &PredefinedMenuItem::separator(),
        &cmd_item("page.rotate"),
        &cmd_item("page.rotate_ccw"),
        &cmd_item("page.rotate_dialog"),
        &PredefinedMenuItem::separator(),
        &cmd_item("page.delete"),
        &cmd_item("page.insert_blank"),
        &cmd_item("page.insert"),
        &cmd_item("page.duplicate"),
        &cmd_item("page.replace"),
        &cmd_item("page.extract"),
        &cmd_item("page.split"),
        &cmd_item("page.reverse"),
        &cmd_item("page.crop"),
        &cmd_item("page.boxes"),
        &cmd_item("page.number"),
        &PredefinedMenuItem::separator(),
        &cmd_item("bookmark.add"),
        &cmd_item("bookmark.from_structure"),
    ])?;

    // 5. View menu
    let view_menu = sub_menu("View");

    let zoom_menu = sub_menu("Zoom");
    zoom_menu.append_items(&[
        &cmd_item("view.zoom.in"),
        &cmd_item("view.zoom.out"),
        &PredefinedMenuItem::separator(),
        &cmd_item("view.actual_size"),
        &cmd_item("view.page_level"),
        &cmd_item("view.fit_width"),
        &cmd_item("view.fit_height"),
        &cmd_item("view.fit_visible"),
        &cmd_item("view.marquee_zoom"),
        &PredefinedMenuItem::separator(),
        &cmd_item("view.rotate_cw"),
        &cmd_item("view.rotate_ccw"),
    ])?;

    let page_nav_menu = sub_menu("Page navigation");
    page_nav_menu.append_items(&[
        &cmd_item("view.focus_page_input"),
        &PredefinedMenuItem::separator(),
        &cmd_item("view.previous_view"),
        &cmd_item("view.next_view"),
    ])?;

    let page_display_menu = sub_menu("Page display");
    page_display_menu.append_items(&[
        &cmd_item("view.layout.single"),
        &cmd_item("view.layout.two_up"),
        &cmd_item("view.continuous"),
        &cmd_item("view.page_gaps"),
        &cmd_item("view.layout.cover"),
        &PredefinedMenuItem::separator(),
        &cmd_item("view.fit_width_scrolling"),
        &cmd_item("view.fit_one_page"),
    ])?;

    let side_panels_menu = sub_menu("Side panels");
    side_panels_menu.append_items(&[
        &cmd_item("comment.list"),
        &cmd_item("panel.bookmarks"),
        &cmd_item("panel.pages"),
        &cmd_item("form.fields"),
        &cmd_item("panel.layers"),
        &cmd_item("panel.attachments"),
        &cmd_item("sign.panel"),
        &cmd_item("panel.accessibility"),
        &cmd_item("a11y.tags"),
        &cmd_item("panel.search"),
        &cmd_item("panel.compare"),
    ])?;

    let theme_menu = sub_menu("Display theme");
    theme_menu.append_items(&[&cmd_item("view.theme.system"), &cmd_item("view.theme.light"), &cmd_item("view.theme.dark")])?;

    view_menu.append_items(&[
        &zoom_menu,
        &page_nav_menu,
        &page_display_menu,
        &side_panels_menu,
        &theme_menu,
        &PredefinedMenuItem::separator(),
        &cmd_item("view.full_screen"),
        &cmd_item("view.read_mode"),
        &PredefinedMenuItem::separator(),
        &cmd_item("view.next_tab"),
        &cmd_item("view.previous_tab"),
        &cmd_item("ui.dock.reset"),
    ])?;

    // 6. Tools menu
    let tools_menu = sub_menu("Tools");

    let modes_menu = sub_menu("Workspace modes");
    modes_menu.append_items(&[
        &cmd_item("mode.all_tools"),
        &cmd_item("view.read_mode"),
        &cmd_item("mode.edit"),
        &cmd_item("mode.convert"),
        &cmd_item("mode.sign"),
    ])?;

    let comments_menu = sub_menu("Comments & Markup");
    comments_menu.append_items(&[
        &cmd_item("comment.note"),
        &cmd_item("comment.highlight"),
        &cmd_item("comment.underline"),
        &cmd_item("comment.strikeout"),
        &cmd_item("comment.freetext"),
        &cmd_item("comment.line"),
        &cmd_item("comment.square"),
        &cmd_item("comment.circle"),
        &cmd_item("comment.stamp"),
        &PredefinedMenuItem::separator(),
        &cmd_item("comment.flatten"),
        &cmd_item("comment.import"),
        &cmd_item("comment.export"),
    ])?;

    let content_menu = sub_menu("Edit Content");
    content_menu.append_items(&[&cmd_item("edit.header_footer"), &cmd_item("edit.watermark"), &cmd_item("edit.background")])?;

    let forms_menu = sub_menu("Forms");
    forms_menu.append_items(&[
        &cmd_item("form.add.text"),
        &cmd_item("form.add.checkbox"),
        &cmd_item("form.add.radio"),
        &cmd_item("form.add.combo"),
        &cmd_item("form.add.list"),
        &cmd_item("form.add.button"),
        &cmd_item("form.add.signature"),
        &PredefinedMenuItem::separator(),
        &cmd_item("form.clear"),
        &cmd_item("form.flatten"),
    ])?;

    let sign_menu = sub_menu("Sign & Certify");
    sign_menu.append_items(&[
        &cmd_item("sign.fill.signature"),
        &cmd_item("sign.fill.initials"),
        &cmd_item("sign.digital"),
        &cmd_item("sign.certify"),
    ])?;

    let redact_menu = sub_menu("Redact");
    redact_menu.append_items(&[&cmd_item("redact.mark"), &cmd_item("redact.pages"), &cmd_item("redact.apply"), &cmd_item("redact.sanitize")])?;

    let ocr_menu = sub_menu("Scan & OCR");
    ocr_menu.append_items(&[&cmd_item("ocr.recognize"), &cmd_item("ocr.enhance_camera")])?;

    let a11y_menu = sub_menu("Accessibility");
    a11y_menu.append_items(&[&cmd_item("a11y.check"), &cmd_item("a11y.autotag"), &cmd_item("a11y.report")])?;

    tools_menu.append_items(&[
        &modes_menu,
        &PredefinedMenuItem::separator(),
        &comments_menu,
        &content_menu,
        &forms_menu,
        &sign_menu,
        &redact_menu,
        &ocr_menu,
        &a11y_menu,
        &PredefinedMenuItem::separator(),
        &cmd_item("doc.compare"),
    ])?;

    // 7. Window menu
    let window_menu = sub_menu("Window");
    window_menu.append_items(&[
        &PredefinedMenuItem::minimize(None),
        &PredefinedMenuItem::zoom(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::bring_all_to_front(None),
    ])?;

    // 8. Help menu
    let help_menu = sub_menu("Help");
    help_menu.append_items(&[
        &cmd_item("help.shortcuts"),
        &PredefinedMenuItem::separator(),
        &cmd_item("help.check_updates"),
        &cmd_item("help.website"),
        &cmd_item("help.github"),
        &cmd_item("help.discord"),
        &PredefinedMenuItem::separator(),
        &cmd_item("help.about"),
    ])?;

    menu.append_items(&[&app_menu, &file_menu, &edit_menu, &pages_menu, &view_menu, &tools_menu, &window_menu, &help_menu])?;

    menu.init_for_nsapp();

    let menu_inbox = apple_events.menu_inbox();
    let egui_ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(Box::new(move |event: MenuEvent| {
        let id_str = event.id.0.as_str();
        if id_str == craft_quit::QUIT_MENU_ID {
            craft_quit::on_menu_event(&egui_ctx, id_str);
        } else {
            if let Ok(mut inbox) = menu_inbox.lock() {
                inbox.push(id_str.to_string());
            }
            egui_ctx.request_repaint();
        }
    })));

    MENU.with_borrow_mut(|m| *m = Some(menu));
    Ok(())
}
