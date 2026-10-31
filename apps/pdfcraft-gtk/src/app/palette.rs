use super::App;
use adw::{gtk, prelude::*};
use libadwaita as adw;
use pdfcraft_engine::commands;
use std::{rc::Rc, time::Duration};

impl App {
    pub(super) fn show_palette(self: &Rc<Self>) {
        let dialog = adw::Window::builder()
            .title("Find tools and commands")
            .default_width(640)
            .default_height(720)
            .modal(true)
            .transient_for(&self.window)
            .build();

        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new("Find tools and commands", "Menus, tools and headless commands");
        header.set_title_widget(Some(&title));
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Search commands"));
        search.set_hexpand(true);
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let clamp = adw::Clamp::new();
        clamp.set_child(Some(&list));
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_child(Some(&clamp));
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.append(&search);
        content.append(&scroll);
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&content));
        dialog.set_content(Some(&toolbar));

        populate(self, &list, &dialog, "");
        let app = self.clone();
        let palette_list = list.clone();
        let changed_list = palette_list.clone();
        let palette_dialog = dialog.clone();
        search.connect_search_changed(move |entry| populate(&app, &changed_list, &palette_dialog, &entry.text()));
        let palette_dialog = dialog.clone();
        search.connect_activate(move |_| {
            if let Some(row) = palette_list.row_at_index(0) {
                row.activate();
            } else {
                palette_dialog.close();
            }
        });
        dialog.present();
        if std::env::var("PDFCRAFT_GTK_QA").as_deref() == Ok("palette") {
            capture_palette_when_ready(self.clone(), dialog, list, 0);
        }
    }
}

fn capture_palette_when_ready(app: Rc<App>, dialog: adw::Window, list: gtk::ListBox, attempt: u32) {
    let child = dialog.child();
    let ready = dialog.width() > 0
        && dialog.height() > 0
        && child.as_ref().is_some_and(|widget| widget.is_mapped() && widget.width() > 0 && widget.height() > 0);
    if !ready && attempt < 50 {
        adw::glib::timeout_add_local_once(Duration::from_millis(100), move || {
            capture_palette_when_ready(app, dialog, list, attempt + 1);
        });
        return;
    }
    let mut count = 0;
    let mut row = list.first_child();
    while let Some(widget) = row {
        count += 1;
        row = widget.next_sibling();
    }
    let image = crate::capture::path("palette");
    let captured = crate::capture::window_png(&dialog, &image);
    eprintln!(
        "palette rows={count} expected={} size={}x{} visible={} mapped={} child={} attempts={attempt} capture={captured:?}",
        commands::COMMANDS.len(),
        dialog.width(),
        dialog.height(),
        dialog.is_visible(),
        dialog.is_mapped(),
        child
            .as_ref()
            .map(|widget| format!("{}x{} visible={} mapped={}", widget.width(), widget.height(), widget.is_visible(), widget.is_mapped()))
            .unwrap_or_else(|| "none".to_string()),
    );
    let ok = ready && count == commands::COMMANDS.len() && captured.is_ok();
    crate::capture::mark(ok, "palette", &app.application);
}

fn populate(app: &Rc<App>, list: &gtk::ListBox, dialog: &adw::Window, query: &str) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let query = query.to_lowercase();
    let active_id = app.active_view().map(|view| view.id);
    let automation = app.automation.borrow();
    for command in
        commands::COMMANDS.iter().filter(|command| query.is_empty() || command.label.to_lowercase().contains(&query) || command.id.contains(&query))
    {
        let row = adw::ActionRow::new();
        row.set_title(&adw::glib::markup_escape_text(command.label));
        row.set_subtitle(command.id);
        row.set_activatable(true);
        row.set_sensitive(commands::is_enabled(command, automation.session(), active_id));
        let app = app.clone();
        let dialog = dialog.clone();
        let id = command.id;
        row.connect_activated(move |_| {
            dialog.close();
            app.run_registered(id);
        });
        list.append(&row);
    }
}
