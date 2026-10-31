use super::App;
use crate::document::DocumentView;
use adw::{gio, gtk, prelude::*};
use libadwaita as adw;
use pdfcraft_engine::{DocId, Edit, FieldConflicts};
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc};

#[derive(Clone)]
struct PageClip {
    from: DocId,
    name: String,
    bytes: Arc<Vec<u8>>,
    pages: Vec<usize>,
}

thread_local! {
    static PAGE_CLIP: RefCell<Option<PageClip>> = const { RefCell::new(None) };
}

impl App {
    pub(super) fn supports_editing(id: &str) -> bool {
        matches!(
            id,
            "file.open_recent"
                | "page.copy"
                | "page.cut"
                | "page.paste"
                | "create.clipboard"
                | "edit.copy"
                | "edit.snapshot"
                | "redact.properties"
                | "page.organize"
        )
    }

    pub(super) fn run_editing(self: &Rc<Self>, id: &str) {
        match id {
            "file.open_recent" => self.open_recent(),
            "page.copy" => self.copy_pages(false),
            "page.cut" => self.copy_pages(true),
            "page.paste" => self.paste_pages(),
            "create.clipboard" => self.create_from_clipboard(),
            "edit.copy" => self.copy_text(),
            "edit.snapshot" => self.copy_snapshot(),
            "redact.properties" => self.redact_properties(),
            "page.organize" => self.organize_pages(),
            _ => {}
        }
    }

    fn open_recent(self: &Rc<Self>) {
        let recents = super::native::recent_paths();
        let existing: Vec<PathBuf> = recents.into_iter().filter(|path| path.exists()).collect();
        if existing.is_empty() {
            self.toast.add_toast(adw::Toast::new("No hay documentos recientes"));
            return;
        }
        let list = gtk::ListBox::builder().css_classes(["boxed-list"]).margin_top(18).margin_bottom(18).margin_start(18).margin_end(18).build();
        let app = self.clone();
        for path in existing {
            let row = adw::ActionRow::builder().title(path.display().to_string()).activatable(true).name(path.display().to_string()).build();
            let open = gtk::Button::from_icon_name("document-open-symbolic");
            open.set_valign(gtk::Align::Center);
            open.set_tooltip_text(Some("Abrir"));
            let app_row = app.clone();
            let path_row = path.clone();
            open.connect_clicked(move |_| {
                let path = path_row.clone();
                app_row.open_recent_path(&path);
            });
            row.add_suffix(&open);
            list.append(&row);
        }
        let app_list = app.clone();
        list.connect_row_activated(move |_, row| {
            let name = row.widget_name().to_string();
            if !name.is_empty() {
                app_list.open_recent_path(&PathBuf::from(name));
            }
        });
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&list));
        scroll.set_min_content_height(520);
        let window = recent_window(self, "Documentos recientes", &scroll);
        window.present();
    }

    pub(super) fn open_recent_path(self: &Rc<Self>, path: &std::path::Path) {
        self.open_paths(vec![path.to_path_buf()]);
    }

    fn copy_pages(self: &Rc<Self>, cut: bool) {
        self.with_active(|app, view| {
            let page = view.current();
            let clip = {
                let automation = app.automation.borrow();
                let Some(document) = automation.session().get(view.id) else { return };
                PageClip { from: view.id, name: view.name.borrow().clone(), bytes: document.bytes.clone(), pages: vec![page] }
            };
            if cut && view.page_count() <= 1 {
                app.toast.add_toast(adw::Toast::new("No se puede cortar la última página"));
                return;
            }
            PAGE_CLIP.with_borrow_mut(|slot| *slot = Some(clip));
            if cut {
                app.apply_edit(Edit::DeletePages { pages: vec![page] });
            } else {
                app.toast.add_toast(adw::Toast::new(&format!("Página {} copiada", page + 1)));
            }
        });
    }

    pub(super) fn page_clip() -> Option<(DocId, usize)> {
        PAGE_CLIP.with_borrow(|slot| slot.as_ref().map(|clip| (clip.from, clip.pages.len())))
    }

    fn paste_pages(self: &Rc<Self>) {
        let Some(clip) = PAGE_CLIP.with_borrow(Clone::clone) else {
            self.toast.add_toast(adw::Toast::new("El portapapeles de páginas está vacío"));
            return;
        };
        self.with_active(|app, view| {
            let at = view.current() + 1;
            let conflicts = if clip.from == view.id { FieldConflicts::Merge } else { FieldConflicts::Rename };
            eprintln!("[DEBUG-paste] at={at} pages={:?} name={}", clip.pages, clip.name);
            app.apply_edit(Edit::InsertPagesFrom {
                name: clip.name.clone(),
                bytes: clip.bytes.clone(),
                pages: Some(clip.pages.clone()),
                at,
                field_conflicts: conflicts,
            });
            eprintln!("[DEBUG-paste] after count={}", view.page_count());
        });
    }

    fn create_from_clipboard(self: &Rc<Self>) {
        let app = self.clone();
        self.window.clipboard().read_text_async(gio::Cancellable::NONE, move |result| {
            let Some(text) = result.ok().flatten().map(|text| text.to_string()) else {
                app.toast.add_toast(adw::Toast::new("El portapapeles no contiene texto"));
                return;
            };
            if text.trim().is_empty() {
                app.toast.add_toast(adw::Toast::new("El portapapeles está vacío"));
                return;
            }
            let created = {
                let mut automation = app.automation.borrow_mut();
                automation.session_mut().create_from_text("Desde el portapapeles", &text)
            };
            match created {
                Ok(bytes) => {
                    app.open_created("Desde el portapapeles.pdf".to_string(), bytes);
                }
                Err(error) => app.toast.add_toast(adw::Toast::new(&format!("No se pudo crear el PDF: {error}"))),
            }
        });
    }

    fn copy_text(self: &Rc<Self>) {
        self.with_active(|app, view| {
            let page = view.current();
            let text = {
                let automation = app.automation.borrow();
                automation
                    .session()
                    .get(view.id)
                    .map(|document| document.text_lines(page).iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n"))
            };
            match text {
                Some(text) if !text.is_empty() => {
                    app.window.clipboard().set_text(&text);
                    app.toast.add_toast(adw::Toast::new("Texto de la página copiado"));
                }
                Some(_) => app.toast.add_toast(adw::Toast::new("La página no tiene texto seleccionable")),
                None => app.toast.add_toast(adw::Toast::new("El documento ya no está abierto")),
            }
        });
    }

    fn copy_snapshot(self: &Rc<Self>) {
        self.with_active(|app, view| match view.current_texture() {
            Some(texture) => {
                app.window.clipboard().set_texture(&texture);
                app.toast.add_toast(adw::Toast::new("Captura de página copiada"));
            }
            None => app.toast.add_toast(adw::Toast::new("La página aún no está renderizada")),
        });
    }

    fn redact_properties(self: &Rc<Self>) {
        let (text, overlay) = super::native::redact_defaults();
        let group =
            adw::PreferencesGroup::builder().title("Preferencias de redacción").description("Valores iniciales al marcar redacciones").build();
        let text_row = adw::EntryRow::builder().title("Texto superpuesto").text(&text).build();
        let overlay_row = adw::SwitchRow::builder().title("Mostrar superposición").active(overlay).build();
        group.add(&text_row);
        group.add(&overlay_row);
        let apply = gtk::Button::with_label("Guardar");
        apply.add_css_class("suggested-action");
        apply.set_margin_top(18);
        apply.set_margin_bottom(18);
        apply.set_margin_start(18);
        apply.set_margin_end(18);
        apply.connect_clicked(move |_| {
            super::native::save_redact_defaults(text_row.text().as_str(), overlay_row.is_active());
        });
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&group);
        content.append(&apply);
        let window = recent_window(self, "Propiedades de redacción", &content);
        window.present();
    }

    fn organize_pages(self: &Rc<Self>) {
        let Some(view) = self.active_view() else {
            self.toast.add_toast(adw::Toast::new("Abre un documento para organizar páginas"));
            return;
        };
        let window = adw::Window::builder()
            .title("Organizar páginas")
            .default_width(560)
            .default_height(720)
            .modal(true)
            .transient_for(&self.window)
            .application(&self.application)
            .build();
        let list = gtk::ListBox::builder().css_classes(["boxed-list"]).margin_top(18).margin_bottom(18).margin_start(18).margin_end(18).build();
        let app = self.clone();
        let weak_view = Rc::downgrade(&view);
        rebuild_organizer(&list, &app, &weak_view);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_child(Some(&list));
        scroll.set_vexpand(true);
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&scroll));
        window.set_content(Some(&toolbar));
        window.present();
    }
}

fn rebuild_organizer(list: &gtk::ListBox, app: &Rc<App>, weak_view: &std::rc::Weak<DocumentView>) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    let Some(view) = weak_view.upgrade() else { return };
    let total = view.page_count();
    let make_button = |icon: &str, tooltip: &str| {
        let button = gtk::Button::from_icon_name(icon);
        button.set_valign(gtk::Align::Center);
        button.set_tooltip_text(Some(tooltip));
        button
    };
    for page in 0..total {
        let row = adw::ActionRow::builder().title(format!("Página {}", page + 1)).build();
        let app_up = app.clone();
        let weak_up = weak_view.clone();
        let list_up = list.clone();
        let up = make_button("go-up-symbolic", "Subir");
        if page == 0 {
            up.set_sensitive(false);
        }
        up.connect_clicked(move |_| {
            app_up.apply_edit(Edit::MovePages { pages: vec![page], to: page - 1 });
            rebuild_organizer(&list_up, &app_up, &weak_up);
        });
        let app_down = app.clone();
        let weak_down = weak_view.clone();
        let list_down = list.clone();
        let down = make_button("go-down-symbolic", "Bajar");
        if page + 1 >= total {
            down.set_sensitive(false);
        }
        down.connect_clicked(move |_| {
            app_down.apply_edit(Edit::MovePages { pages: vec![page], to: page + 2 });
            rebuild_organizer(&list_down, &app_down, &weak_down);
        });
        let app_rotate = app.clone();
        let weak_rotate = weak_view.clone();
        let list_rotate = list.clone();
        let rotate = make_button("object-rotate-left-symbolic", "Rotar 90°");
        rotate.connect_clicked(move |_| {
            app_rotate.apply_edit(Edit::RotatePages { pages: vec![page], degrees: 90 });
            rebuild_organizer(&list_rotate, &app_rotate, &weak_rotate);
        });
        let app_delete = app.clone();
        let weak_delete = weak_view.clone();
        let list_delete = list.clone();
        let delete = make_button("user-trash-symbolic", "Eliminar");
        if total <= 1 {
            delete.set_sensitive(false);
        }
        delete.connect_clicked(move |_| {
            app_delete.apply_edit(Edit::DeletePages { pages: vec![page] });
            rebuild_organizer(&list_delete, &app_delete, &weak_delete);
        });
        for button in [&up, &down, &rotate, &delete] {
            row.add_suffix(button);
        }
        list.append(&row);
    }
}

pub(super) fn recent_window(app: &App, title: &str, content: &impl IsA<gtk::Widget>) -> adw::Window {
    let window = adw::Window::builder()
        .title(title)
        .default_width(680)
        .default_height(680)
        .modal(true)
        .transient_for(&app.window)
        .application(&app.application)
        .build();
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(content));
    window.set_content(Some(&toolbar));
    window
}
