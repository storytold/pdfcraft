use super::App;
use crate::document::DocumentView;
use adw::{glib, gtk, prelude::*};
use libadwaita as adw;
use pdfcraft_automation::{Content, ToolDef, tools};
use serde_json::{Value, json};
use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};

thread_local! {
    /// Última ventana de resultados creada, para QA (text-tools) y depuración.
    pub(super) static LAST_RESULTS: RefCell<Option<adw::Window>> = const { RefCell::new(None) };
}

#[derive(Clone)]
enum Route {
    Command(String),
    Tool(String),
}

pub(super) fn supports_command(id: &str) -> bool {
    let definitions = tools();
    definitions.iter().any(|definition| definition.command == Some(id))
        || super::command_aliases::alias(id).is_some_and(|(name, _)| definitions.iter().any(|definition| definition.name == name))
}

impl App {
    pub(super) fn run_automation_command(self: &Rc<Self>, id: &str) {
        let definitions = tools();
        let resolved = definitions
            .iter()
            .find(|definition| definition.command == Some(id))
            .cloned()
            .map(|definition| (definition, serde_json::json!({}), Route::Command(id.to_string())));
        let resolved = resolved.or_else(|| {
            let (name, defaults) = super::command_aliases::alias(id)?;
            let definition = definitions.iter().find(|definition| definition.name == name)?.clone();
            Some((definition, defaults, Route::Tool(name.to_string())))
        });
        let Some((definition, mut defaults, route)) = resolved else {
            self.show_error(&format!("No hay una herramienta headless registrada para {id}"));
            return;
        };
        if accepts_document(&definition)
            && let Some(view) = self.active_view()
        {
            defaults["doc"] = json!(view.id.0);
        }
        if needs_form(&definition) {
            let app = self.clone();
            crate::tool_form::present(&self.window, &definition, defaults, move |params| {
                app.execute_automation_route(route.clone(), definition.title, params);
            });
        } else {
            self.execute_automation_route(route, definition.title, defaults);
        }
    }

    fn execute_automation_route(self: &Rc<Self>, route: Route, title: &str, params: Value) {
        let result = match &route {
            Route::Command(id) => self.automation.borrow_mut().call("command_run", &json!({"id": id, "params": params})),
            Route::Tool(name) => self.automation.borrow_mut().call(name, &params),
        };
        match result {
            Ok(contents) => {
                self.reconcile_documents();
                self.show_automation_results(title, contents);
            }
            Err(error) => self.show_error(&format!("No se pudo ejecutar {title}: {error}")),
        }
    }

    pub(super) fn reconcile_documents(self: &Rc<Self>) {
        let documents = {
            let automation = self.automation.borrow();
            automation
                .session()
                .docs()
                .iter()
                .map(|document| (document.id, document.name.clone(), document.path.clone(), document.info.clone()))
                .collect::<Vec<_>>()
        };
        let existing = self.views.borrow().keys().copied().collect::<Vec<_>>();
        for id in existing {
            if documents.iter().any(|(document_id, _, _, _)| *document_id == id) {
                continue;
            }
            let root = self.views.borrow().get(&id).map(|view| view.root.clone());
            if let Some(root) = root {
                let page = self.tabs.page(&root);
                self.tabs.close_page(&page);
            }
        }
        let mut added = None;
        for (id, name, path, info) in documents {
            if let Some(view) = self.views.borrow().get(&id).cloned() {
                view.set_location(name, path.as_ref().map(PathBuf::from));
                view.refresh(&self.automation);
                continue;
            }
            let view = DocumentView::new(id, name, path.as_ref().map(PathBuf::from), info, self.automation.clone());
            let page = self.tabs.append(&view.root);
            page.set_title(&view.name());
            if let Some(path) = path {
                page.set_tooltip(&path);
            }
            self.views.borrow_mut().insert(id, view.clone());
            self.connect_document(&view);
            view.go_to(0, &self.automation);
            added = Some(page);
        }
        if let Some(page) = added {
            self.tabs.set_selected_page(&page);
        }
        self.update_chrome();
    }

    pub(super) fn show_automation_results(self: &Rc<Self>, title: &str, contents: Vec<Content>) {
        if contents.is_empty() {
            self.toast.add_toast(adw::Toast::new(&format!("{title}: completado")));
            return;
        }
        let dialog = adw::Window::builder()
            .title(title)
            .default_width(760)
            .default_height(720)
            .modal(true)
            .transient_for(&self.window)
            .application(&self.application)
            .build();
        let toolbar = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        let close = gtk::Button::with_label("Close");
        header.pack_start(&close);
        toolbar.add_top_bar(&header);
        let scroll = gtk::ScrolledWindow::builder().hexpand(true).vexpand(true).build();
        let clamp = adw::Clamp::builder().maximum_size(900).margin_top(18).margin_bottom(18).margin_start(18).margin_end(18).build();
        let list = gtk::Box::new(gtk::Orientation::Vertical, 18);
        let mut json_text = String::new();
        for content in contents {
            match content {
                Content::Json(value) => {
                    if !json_text.is_empty() {
                        json_text.push_str("\n\n");
                    }
                    match serde_json::to_string_pretty(&value) {
                        Ok(text) => json_text.push_str(&text),
                        Err(error) => json_text.push_str(&format!("Could not format JSON: {error}")),
                    }
                }
                Content::Png { data, .. } => match gtk::gdk::Texture::from_bytes(&glib::Bytes::from(&data)) {
                    Ok(texture) => {
                        let picture = gtk::Picture::for_paintable(&texture);
                        picture.set_content_fit(gtk::ContentFit::Contain);
                        list.append(&picture);
                    }
                    Err(error) => self.show_error(&format!("No se pudo decodificar una imagen de resultado: {error}")),
                },
            }
        }
        if !json_text.is_empty() {
            let text = gtk::TextView::new();
            text.set_editable(false);
            text.set_cursor_visible(false);
            text.set_monospace(true);
            text.set_wrap_mode(gtk::WrapMode::WordChar);
            text.buffer().set_text(&json_text);
            list.append(&text);
        }
        clamp.set_child(Some(&list));
        scroll.set_child(Some(&clamp));
        toolbar.set_content(Some(&scroll));
        dialog.set_content(Some(&toolbar));
        let close_dialog = dialog.clone();
        close.connect_clicked(move |_| {
            close_dialog.close();
        });
        dialog.present();
        LAST_RESULTS.with_borrow_mut(|slot| *slot = Some(dialog.clone()));
        match std::env::var("PDFCRAFT_GTK_QA").as_deref() {
            Ok("automation") => {
                let app = self.clone();
                let dialog = dialog.clone();
                glib::timeout_add_local_once(Duration::from_millis(800), move || {
                    let image = crate::capture::path("automation");
                    let captured = crate::capture::window_png(&dialog, &image);
                    eprintln!(
                        "automation result_bytes={} size={}x{} visible={} mapped={} capture={captured:?}",
                        json_text.len(),
                        dialog.width(),
                        dialog.height(),
                        dialog.is_visible(),
                        dialog.is_mapped()
                    );
                    crate::capture::mark(!json_text.is_empty() && captured.is_ok(), "automation", &app.application);
                });
            }
            Ok("form") => {
                let app = self.clone();
                glib::timeout_add_local_once(Duration::from_millis(800), move || {
                    let output = crate::capture::artifact("exported.txt");
                    let image = crate::capture::path("form-result");
                    let captured = crate::capture::window_png(&dialog, &image);
                    let written = output.metadata().is_ok_and(|metadata| metadata.len() > 0);
                    eprintln!("form output={} written={written} capture={captured:?}", output.display());
                    crate::capture::mark(written && captured.is_ok(), "form", &app.application);
                });
            }
            _ => {}
        }
    }
}

fn accepts_document(definition: &ToolDef) -> bool {
    definition.input_schema.get("properties").and_then(Value::as_object).is_some_and(|properties| properties.contains_key("doc"))
}

fn needs_form(definition: &ToolDef) -> bool {
    definition.input_schema.get("properties").and_then(Value::as_object).is_some_and(|properties| properties.keys().any(|key| key != "doc"))
}
