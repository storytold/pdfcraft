use super::App;
use adw::{gio, glib, gtk, prelude::*};
use libadwaita as adw;
use pdfcraft_engine::Edit;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

impl App {
    pub(super) fn install_document_actions(self: &Rc<Self>) {
        self.action("new-blank", |app| app.new_blank());
        self.action("save", |app| app.save_active());
        self.action("save-as", |app| app.save_as_dialog());
        self.action("revert", |app| app.revert_active());
        self.action("undo", |app| app.undo_active());
        self.action("redo", |app| app.redo_active());
        self.action("rotate-left", |app| app.rotate_active(-90));
        self.action("rotate-right", |app| app.rotate_active(90));
        self.action("delete-page", |app| app.delete_active_page());
        self.action("duplicate-page", |app| app.duplicate_active_page());
        self.action("reverse-pages", |app| app.reverse_active_pages());
        self.action("insert-blank", |app| app.insert_blank_page());
    }

    fn new_blank(self: &Rc<Self>) {
        match self.automation.borrow().session().create_blank(595.0, 842.0, 1) {
            Ok(bytes) => self.open_created("Documento sin título.pdf".to_string(), bytes),
            Err(error) => self.show_error(&format!("No se pudo crear el documento: {error}")),
        }
    }

    fn save_active(self: &Rc<Self>) {
        let Some(view) = self.active_view() else { return };
        if let Some(path) = view.path() {
            self.save_to(view, path);
        } else {
            self.save_as_dialog();
        }
    }

    fn save_as_dialog(self: &Rc<Self>) {
        let Some(view) = self.active_view() else { return };
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Guardar PDF como");
        dialog.set_modal(true);
        dialog.set_initial_name(Some(&view.name()));
        let app = self.clone();
        dialog.save(Some(&self.window), None::<&gio::Cancellable>, move |result| match result {
            Ok(file) => {
                if let Some(path) = file.path() {
                    app.save_to(view, path);
                }
            }
            Err(error) if error.kind::<gtk::DialogError>() == Some(gtk::DialogError::Dismissed) => {}
            Err(error) => app.show_error(&format!("No se pudo abrir el diálogo de guardado: {error}")),
        });
    }

    fn save_to(self: &Rc<Self>, view: Rc<crate::document::DocumentView>, path: PathBuf) {
        let prepared = {
            let automation = self.automation.borrow();
            automation.session().save_bytes(view.id).and_then(|bytes| {
                automation.session().check_save(view.id, &bytes)?;
                Ok(bytes)
            })
        };
        let bytes = match prepared {
            Ok(bytes) => bytes,
            Err(error) => {
                self.show_error(&format!("No se pudo preparar el guardado: {error}"));
                return;
            }
        };
        let app = self.clone();
        let write_path = path.clone();
        let write_bytes = bytes.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || write_atomic(&write_path, &write_bytes)).await;
            match result {
                Ok(Ok(())) => {
                    let path_string = path.to_string_lossy().into_owned();
                    if let Err(error) = app.automation.borrow_mut().session_mut().mark_saved(view.id, bytes, Some(path_string)) {
                        app.show_error(&format!("El archivo se escribió, pero el motor no pudo adoptar el guardado: {error}"));
                        return;
                    }
                    let name = path.file_name().map_or_else(|| "Documento.pdf".into(), |name| name.to_string_lossy().into_owned());
                    view.set_location(name.clone(), Some(path.clone()));
                    let page = app.tabs.page(&view.root);
                    page.set_title(&name);
                    page.set_tooltip(&path.to_string_lossy());
                    app.update_chrome();
                    app.toast.add_toast(adw::Toast::new(&format!("Guardado en {}", path.display())));
                }
                Ok(Err(error)) => app.show_error(&format!("No se pudo guardar {}: {error}", path.display())),
                Err(_) => app.show_error("La tarea de guardado falló"),
            }
        });
    }

    fn revert_active(&self) {
        let Some(view) = self.active_view() else { return };
        let result = self.automation.borrow_mut().session_mut().revert(view.id);
        match result {
            Ok(()) => {
                view.refresh(&self.automation);
                self.update_chrome();
            }
            Err(error) => self.show_error(&format!("No se pudo revertir: {error}")),
        }
    }

    fn undo_active(&self) {
        let Some(view) = self.active_view() else { return };
        let result = self.automation.borrow_mut().session_mut().undo(view.id);
        match result {
            Ok(_) => {
                view.refresh(&self.automation);
                self.update_chrome();
            }
            Err(error) => self.show_error(&format!("No se pudo deshacer: {error}")),
        }
    }

    fn redo_active(&self) {
        let Some(view) = self.active_view() else { return };
        let result = self.automation.borrow_mut().session_mut().redo(view.id);
        match result {
            Ok(_) => {
                view.refresh(&self.automation);
                self.update_chrome();
            }
            Err(error) => self.show_error(&format!("No se pudo rehacer: {error}")),
        }
    }

    fn rotate_active(&self, degrees: i64) {
        self.with_active(|app, view| {
            let page = view.current();
            app.apply_edit(Edit::RotatePages { pages: vec![page], degrees });
        });
    }

    fn delete_active_page(&self) {
        self.with_active(|app, view| {
            if view.page_count() <= 1 {
                app.show_error("No se puede eliminar la única página del documento");
                return;
            }
            app.apply_edit(Edit::DeletePages { pages: vec![view.current()] });
        });
    }

    fn duplicate_active_page(&self) {
        self.with_active(|app, view| {
            app.apply_edit(Edit::DuplicatePages { pages: vec![view.current()], at: Some(view.current() + 1) });
        });
    }

    fn reverse_active_pages(&self) {
        self.with_active(|app, view| {
            let pages = (0..view.page_count()).collect();
            app.apply_edit(Edit::ReversePages { pages });
        });
    }

    fn insert_blank_page(&self) {
        self.with_active(|app, view| {
            let current = view.current();
            let dimensions = view.info.borrow().pages.get(current).map(|page| (page.width, page.height));
            let Some((width, height)) = dimensions else { return };
            app.apply_edit(Edit::InsertBlankPage { at: current + 1, width: f64::from(width), height: f64::from(height) });
        });
    }

    pub(super) fn apply_edit(&self, edit: Edit) {
        let Some(view) = self.active_view() else { return };
        let result = self.automation.borrow_mut().session_mut().apply(view.id, edit);
        match result {
            Ok(()) => {
                view.refresh(&self.automation);
                self.update_chrome();
            }
            Err(error) => {
                eprintln!("[DEBUG-apply_edit] error={error}");
                self.show_error(&format!("No se pudo editar el documento: {error}"));
            }
        }
    }

    pub(super) fn run_registered(self: &Rc<Self>, id: &str) {
        match id {
            "file.open" => self.open_dialog(),
            "create.blank" => self.new_blank(),
            "file.save" => self.save_active(),
            "file.save_as" => self.save_as_dialog(),
            "file.revert" => self.revert_active(),
            "file.close" => self.close_active(),
            "file.close_all" => self.close_all(),
            "edit.undo" => self.undo_active(),
            "edit.redo" => self.redo_active(),
            "page.rotate" => self.rotate_active(90),
            "page.rotate_ccw" => self.rotate_active(-90),
            "page.delete" => self.delete_active_page(),
            "page.insert_blank" => self.insert_blank_page(),
            "page.duplicate" => self.duplicate_active_page(),
            "page.reverse" => self.reverse_active_pages(),
            "view.palette" => self.show_palette(),
            "view.next_tab" => self.select_relative_tab(1),
            "view.previous_tab" => self.select_relative_tab(-1),
            "view.full_screen" => {
                if self.window.is_fullscreen() {
                    self.window.unfullscreen();
                } else {
                    self.window.fullscreen();
                }
            }
            "help.app_page" => self.open_uri("https://artcraft.software/pdfcraft"),
            "help.github" => self.open_uri("https://github.com/storytold/pdfcraft"),
            "help.about" => self.about(),
            _ if App::supports_native(id) => {
                self.run_native(id);
            }
            _ if App::supports_editing(id) => {
                self.run_editing(id);
            }
            _ if App::supports_signing(id) => {
                self.run_signing(id);
            }
            _ => self.run_automation_command(id),
        }
    }

    fn close_all(&self) {
        while self.tabs.n_pages() > 0 {
            self.close_active();
        }
    }

    fn select_relative_tab(&self, delta: i32) {
        let count = self.tabs.n_pages();
        if count == 0 {
            return;
        }
        let current = self.tabs.selected_page().map_or(0, |page| self.tabs.page_position(&page));
        let next = (current + delta).rem_euclid(count);
        let page = self.tabs.nth_page(next);
        self.tabs.set_selected_page(&page);
    }

    pub(super) fn open_uri(&self, uri: &str) {
        if let Err(error) = gio::AppInfo::launch_default_for_uri(uri, None::<&gio::AppLaunchContext>) {
            self.show_error(&format!("No se pudo abrir {uri}: {error}"));
        }
    }

    pub(super) fn schedule_document_qa(self: &Rc<Self>) {
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(2200), move || {
            let Some(view) = app.active_view() else {
                crate::capture::mark(false, "document", &app.application);
                return;
            };
            app.apply_edit(Edit::RotatePages { pages: vec![0], degrees: 90 });
            let rotated = view.info.borrow().pages.first().map(|page| page.rotation).unwrap_or(0);
            if rotated != 90 {
                crate::capture::mark(false, "document", &app.application);
                return;
            }
            app.undo_active();
            app.redo_active();
            app.duplicate_active_page();
            if view.page_count() != 3 {
                crate::capture::mark(false, "document", &app.application);
                return;
            }
            app.delete_active_page();
            app.insert_blank_page();
            app.reverse_active_pages();
            if view.page_count() != 3 {
                crate::capture::mark(false, "document", &app.application);
                return;
            }
            let path = crate::capture::artifact("document-output.pdf");
            app.save_to(view, path);
        });
        let app = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(5000), move || {
            let output = crate::capture::artifact("document-output.pdf");
            let image = crate::capture::path("document");
            let saved = fs::metadata(&output).map(|metadata| metadata.len() > 0).unwrap_or(false);
            let clean = app.active_view().is_some_and(|view| app.automation.borrow().session().get(view.id).is_some_and(|document| !document.dirty));
            crate::capture::window_png_when_ready(app.window.clone(), &image, 8, move |captured| {
                crate::capture::mark(saved && clean && captured.is_ok(), "document", &app.application);
            });
        });
    }
}

fn write_atomic(path: &Path, bytes: &Arc<Vec<u8>>) -> Result<(), String> {
    let name = path.file_name().ok_or_else(|| "la ruta no tiene nombre de archivo".to_string())?.to_string_lossy();
    let temporary = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&temporary).map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        fs::rename(&temporary, path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
