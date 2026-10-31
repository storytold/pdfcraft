//! Side panels: tools (annotation quick tools) and document lists with real page thumbnails.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use adw::{glib, gtk, prelude::*};
use libadwaita as adw;
use pdfcraft_automation::Automation;
use pdfcraft_engine::{AttachmentSource, DocId, DocInfo, Edit, FieldInfoKind, FieldValue, RenderConfig, RenderPool, RenderRequest, RequestKind};

use super::APP;
use crate::document::DocumentView;

const THUMB_HEIGHT: f32 = 128.0;
const THUMB_TAG: u64 = 7;

type Shared = Rc<RefCell<Automation>>;

/// Applies an undoable edit through the shared session, refreshes the view and
/// the window chrome. Returns whether the edit succeeded.
fn apply_edit(view: &Rc<DocumentView>, automation: &Shared, edit: Edit) -> bool {
    let result = automation.borrow_mut().session_mut().apply(view.id, edit);
    match result {
        Ok(()) => {
            view.refresh(automation);
            if let Some(app) = APP.with(|slot| slot.borrow().clone()) {
                app.update_chrome();
            }
            true
        }
        Err(error) => {
            eprintln!("[panels] edit rechazado: {error}");
            false
        }
    }
}

/// Shows a modal text prompt (single EntryRow) and calls `on_ok` with the text.
fn prompt_entry(_view: &Rc<DocumentView>, title: &str, row_title: &str, initial: &str, on_ok: impl Fn(String) + 'static) {
    let window = app_window();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);
    let entry = adw::EntryRow::new();
    entry.set_title(row_title);
    entry.set_text(initial);
    let list = gtk::ListBox::new();
    list.set_css_classes(&["boxed-list"]);
    list.set_selection_mode(gtk::SelectionMode::None);
    list.append(&entry);
    content.append(&list);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancelar");
    let apply = gtk::Button::with_label("Aceptar");
    apply.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&apply);
    content.append(&buttons);
    let dialog = adw::Window::builder().title(title).modal(true).build();
    if let Some(window) = &window {
        dialog.set_transient_for(Some(window));
    }
    dialog.set_content(Some(&content));
    dialog.set_default_size(440, 140);
    let dialog_clone = dialog.clone();
    cancel.connect_clicked(move |_| dialog_clone.close());
    let dialog_clone = dialog.clone();
    let entry_clone = entry.clone();
    let apply_clone = apply.clone();
    entry.connect_changed(move |entry| {
        apply_clone.set_sensitive(!entry.text().trim().is_empty());
    });
    apply.connect_clicked(move |_| {
        let text = entry_clone.text().trim().to_string();
        dialog_clone.close();
        if !text.is_empty() {
            on_ok(text);
        }
    });
    dialog.present();
}

fn app_window() -> Option<gtk::Window> {
    APP.with(|slot| slot.borrow().as_ref().map(|app| app.window.upcast_ref::<gtk::Window>().clone()))
}

pub struct DocumentPanel {
    automation: Shared,
    tools_root: gtk::Box,
    root: gtk::Box,
    stack: gtk::Stack,
    comments: gtk::ListBox,
    bookmarks: gtk::ListBox,
    add_bookmark_btn: gtk::Button,
    thumbs: gtk::FlowBox,
    fields: gtk::ListBox,
    layers: gtk::ListBox,
    reset_layers_btn: gtk::Button,
    attachments: gtk::ListBox,
    signatures: gtk::ListBox,
    tool_buttons: RefCell<Vec<(String, gtk::ToggleButton)>>,
    active: RefCell<Option<Rc<DocumentView>>>,
    current: Cell<Option<DocId>>,
    fingerprint: Cell<u64>,
    thumb_pool: RefCell<Option<RenderPool>>,
}

impl DocumentPanel {
    pub fn new(automation: &Shared) -> Rc<Self> {
        let comments = list_box();
        let bookmarks = list_box();
        let thumbs = gtk::FlowBox::new();
        thumbs.set_max_children_per_line(3);
        thumbs.set_min_children_per_line(2);
        thumbs.set_selection_mode(gtk::SelectionMode::None);
        thumbs.set_margin_top(12);
        thumbs.set_margin_bottom(12);
        thumbs.set_margin_start(12);
        thumbs.set_margin_end(12);
        thumbs.set_row_spacing(8);
        thumbs.set_column_spacing(8);
        thumbs.set_homogeneous(true);
        let fields = list_box();
        let layers = list_box();
        let attachments = list_box();
        let signatures = list_box();

        let add_bookmark_btn = gtk::Button::from_icon_name("list-add-symbolic");
        add_bookmark_btn.add_css_class("flat");
        add_bookmark_btn.set_tooltip_text(Some("Añadir marcador en la página actual"));
        let bookmarks_box = section("Marcadores", &add_bookmark_btn, &bookmarks);
        let reset_layers_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        reset_layers_btn.add_css_class("flat");
        reset_layers_btn.set_tooltip_text(Some("Restaurar visibilidad inicial de las capas"));
        let layers_box = section("Capas", &reset_layers_btn, &layers);

        let stack = gtk::Stack::new();
        let sections: [(&str, &gtk::Widget); 7] = [
            ("Comentarios", comments.upcast_ref()),
            ("Marcadores", bookmarks_box.upcast_ref()),
            ("Páginas", thumbs.upcast_ref()),
            ("Campos", fields.upcast_ref()),
            ("Capas", layers_box.upcast_ref()),
            ("Adjuntos", attachments.upcast_ref()),
            ("Firmas", signatures.upcast_ref()),
        ];
        for (title, child) in sections {
            let scroll = gtk::ScrolledWindow::new();
            scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
            scroll.set_vexpand(true);
            scroll.set_child(Some(child));
            stack.add_titled(&scroll, Some(title), title);
        }
        let switcher = gtk::StackSwitcher::new();
        switcher.set_stack(Some(&stack));
        let switcher_wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        switcher_wrap.set_margin_top(6);
        switcher_wrap.set_halign(gtk::Align::Center);
        switcher_wrap.append(&switcher);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&switcher_wrap);
        root.append(&stack);

        let tools_root = gtk::Box::new(gtk::Orientation::Vertical, 6);
        tools_root.set_margin_top(12);
        tools_root.set_margin_bottom(12);
        tools_root.set_margin_start(12);
        tools_root.set_margin_end(12);

        Rc::new(Self {
            automation: Rc::clone(automation),
            tools_root,
            root,
            stack,
            comments,
            bookmarks,
            add_bookmark_btn,
            thumbs,
            fields,
            layers,
            reset_layers_btn,
            attachments,
            signatures,
            tool_buttons: RefCell::new(Vec::new()),
            active: RefCell::new(None),
            current: Cell::new(None),
            fingerprint: Cell::new(0),
            thumb_pool: RefCell::new(None),
        })
    }

    pub fn container(&self) -> &gtk::Box {
        &self.root
    }

    pub fn select_section(&self, title: &str) {
        self.stack.set_visible_child_name(title);
    }

    pub fn tools_container(&self) -> &gtk::Box {
        &self.tools_root
    }

    pub fn build_tools(self: &Rc<Self>) {
        let tools = [
            ("Nota", "comment.note"),
            ("Texto", "comment.freetext"),
            ("Resaltar", "comment.highlight"),
            ("Subrayar", "comment.underline"),
            ("Tachar", "comment.strikeout"),
            ("Ondular", "comment.squiggly"),
            ("Tinta", "comment.ink"),
            ("Línea", "comment.line"),
            ("Flecha", "comment.arrow"),
            ("Rectángulo", "comment.square"),
            ("Óvalo", "comment.circle"),
            ("Polígono", "comment.polygon"),
            ("Nube", "comment.cloud"),
            ("Polilínea", "comment.polyline"),
            ("Llamada", "comment.callout"),
            ("Marca", "comment.caret"),
            ("Reemplazo", "comment.replace"),
            ("Redactar", "comment.redact"),
            ("Borrar", "comment.eraser"),
            ("Texto", "sign.fill.text"),
            ("Visto", "sign.fill.check"),
            ("Cruz", "sign.fill.cross"),
            ("Punto", "sign.fill.dot"),
            ("Guion", "sign.fill.line"),
            ("Fecha", "sign.fill.date"),
            ("Firma", "sign.fill.signature"),
            ("Iniciales", "sign.fill.initials"),
            ("Firmar", "sign.digital"),
            ("Certificar", "sign.certify"),
        ];
        let grid = gtk::FlowBox::new();
        grid.set_max_children_per_line(2);
        grid.set_min_children_per_line(2);
        grid.set_selection_mode(gtk::SelectionMode::None);
        grid.set_homogeneous(true);
        for (label, id) in tools {
            let button = gtk::ToggleButton::with_label(label);
            self.tool_buttons.borrow_mut().push((id.to_string(), button.clone()));
            grid.append(&button);
        }
        let none = gtk::ToggleButton::with_label("Ninguna herramienta");
        none.set_active(true);
        self.tool_buttons.borrow_mut().push((String::new(), none.clone()));
        grid.append(&none);
        self.tools_root.append(&grid);

        let panel = Rc::downgrade(self);
        for (_, button) in self.tool_buttons.borrow().iter() {
            let panel = panel.clone();
            button.connect_toggled(move |toggled| {
                if !toggled.is_active() {
                    return;
                }
                let Some(panel) = panel.upgrade() else { return };
                panel.tool_selected(toggled);
            });
        }

        let panel = Rc::downgrade(self);
        self.add_bookmark_btn.clone().connect_clicked(move |_| {
            let Some(panel) = panel.upgrade() else { return };
            if let Some(view) = panel.active.borrow().as_ref() {
                panel.add_bookmark(view);
            }
        });
        let panel = Rc::downgrade(self);
        self.reset_layers_btn.clone().connect_clicked(move |_| {
            let Some(panel) = panel.upgrade() else { return };
            if let Some(view) = panel.active.borrow().as_ref() {
                panel.reset_layers(view);
            }
        });
    }

    fn tool_selected(&self, active: &gtk::ToggleButton) {
        let mut chosen = String::new();
        for (id, button) in self.tool_buttons.borrow().iter() {
            if !std::ptr::eq(button.upcast_ref::<gtk::Widget>(), active.upcast_ref::<gtk::Widget>()) {
                button.set_active(false);
            } else if button.is_active() {
                chosen = id.clone();
            }
        }
        if let Some(view) = self.active.borrow().as_ref() {
            let tool = if chosen.is_empty() { None } else { Some(chosen) };
            view.set_annotation_tool(tool);
        }
    }

    pub fn update(self: &Rc<Self>, view: Option<&Rc<DocumentView>>) {
        let Some(view) = view else {
            self.current.set(None);
            return;
        };
        let id = view.id;
        let info = view.info.borrow().clone();
        let signature_rows = {
            let automation = self.automation.borrow();
            automation
                .session()
                .get(id)
                .map(|doc| doc.signatures.iter().map(|s| (s.field.clone(), s.signed, s.signer.clone(), s.page)).collect::<Vec<_>>())
                .unwrap_or_default()
        };
        let (assembly_allowed, annotate_allowed, fill_allowed, modify_allowed) = {
            let automation = self.automation.borrow();
            automation
                .session()
                .get(id)
                .map(|doc| (doc.allows_assembly(), doc.allows_annotation(), doc.allows_form_filling(), doc.allows_modification()))
                .unwrap_or((false, false, false, false))
        };
        let fingerprint = fingerprint(id, &info, signature_rows.len());
        if self.current.get() == Some(id) && self.fingerprint.get() == fingerprint {
            return;
        }
        self.current.set(Some(id));
        self.fingerprint.set(fingerprint);
        *self.thumb_pool.borrow_mut() = None;
        *self.active.borrow_mut() = None;
        for (_, button) in self.tool_buttons.borrow().iter() {
            button.set_active(false);
        }

        fill_comments(&self.comments, &info, view, &self.automation, self, annotate_allowed);
        fill_bookmarks(&self.bookmarks, &info, view, &self.automation, self, assembly_allowed);
        self.fill_thumbs(view, &info);
        fill_fields(&self.fields, &info, view, &self.automation, self, fill_allowed);
        fill_layers(&self.layers, &info, view, &self.automation, self);
        fill_attachments(&self.attachments, &info, view, &self.automation, self, modify_allowed);
        fill_signatures(&self.signatures, &signature_rows, view, &self.automation);
        *self.active.borrow_mut() = Some(Rc::clone(view));
    }

    fn fill_thumbs(&self, view: &Rc<DocumentView>, info: &DocInfo) {
        while let Some(child) = self.thumbs.first_child() {
            self.thumbs.remove(&child);
        }
        let automation = self.automation.borrow();
        let Some(doc) = automation.session().get(view.id) else { return };
        let count = doc.info.pages.len();
        let bytes = doc.display.clone();
        let mut config = RenderConfig::default().on_screen();
        config.password = doc.password.as_deref().map(Arc::from);
        drop(automation);
        let pool = RenderPool::new(bytes, 1, config);
        let mut requests = Vec::new();
        for page in 0..count {
            let child = gtk::FlowBoxChild::new();
            let picture = gtk::Picture::new();
            picture.set_css_classes(&["pdf-page"]);
            picture.set_size_request(-1, 128);
            child.set_child(Some(&picture));
            self.thumbs.append(&child);
            let height = info.pages.get(page).map_or(800.0, |page_info| page_info.height);
            let scale = (THUMB_HEIGHT / height.max(1.0)).clamp(0.05, 0.5);
            requests.push(RenderRequest { page, kind: RequestKind::Pixels, tile: None, scale, tag: THUMB_TAG, preview: true });
            let view = Rc::clone(view);
            let automation = Rc::clone(&self.automation);
            child.connect_activate(move |child| {
                let index = child.index().max(0) as usize;
                view.go_to(index, &automation);
            });
        }
        pool.set_queue(requests);
        *self.thumb_pool.borrow_mut() = Some(pool);
    }

    pub fn poll_thumbs(&self) {
        let pool_slot = self.thumb_pool.borrow_mut();
        let Some(pool) = pool_slot.as_ref() else { return };
        for _ in 0..32 {
            let Some(rendered) = pool.try_recv() else { break };
            if rendered.request.tag != THUMB_TAG {
                continue;
            }
            let Some(child) = self.thumbs.child_at_index(rendered.request.page as i32) else { continue };
            let Some(picture) = child.child().and_then(|widget| widget.downcast::<gtk::Picture>().ok()) else { continue };
            let Ok(width) = i32::try_from(rendered.width) else { continue };
            let Ok(height) = i32::try_from(rendered.height) else { continue };
            if width <= 0 || height <= 0 {
                continue;
            }
            let bytes = glib::Bytes::from(rendered.rgba.as_ref());
            let texture = gtk::gdk::MemoryTexture::new(width, height, gtk::gdk::MemoryFormat::R8g8b8a8, &bytes, rendered.width as usize * 4);
            picture.set_paintable(Some(&texture));
        }
    }

    pub(super) fn add_bookmark(&self, view: &Rc<DocumentView>) -> bool {
        let outline_len = view.info.borrow().outline.len();
        apply_edit(
            view,
            &self.automation,
            Edit::AddBookmark {
                parent: Vec::new(),
                index: outline_len,
                title: String::from("Sin título"),
                page: view.current(),
                left: None,
                top: None,
            },
        )
    }

    pub(super) fn rename_bookmark(&self, view: &Rc<DocumentView>, path: Vec<usize>, title: &str) -> bool {
        apply_edit(view, &self.automation, Edit::RenameBookmark { path, title: title.to_string() })
    }

    pub(super) fn delete_bookmark(&self, view: &Rc<DocumentView>, path: Vec<usize>) -> bool {
        apply_edit(view, &self.automation, Edit::DeleteBookmark { path })
    }

    pub(super) fn set_field_value(&self, view: &Rc<DocumentView>, name: &str, value: FieldValue) -> bool {
        apply_edit(view, &self.automation, Edit::SetFieldValue { name: name.to_string(), value })
    }

    pub(super) fn delete_comment(&self, view: &Rc<DocumentView>, page: usize, index: usize) -> bool {
        apply_edit(view, &self.automation, Edit::DeleteAnnotation { page, index })
    }

    pub(super) fn delete_attachment(&self, view: &Rc<DocumentView>, key: String) -> bool {
        apply_edit(view, &self.automation, Edit::DeleteAttachment { key })
    }

    pub(super) fn set_layer_visible(&self, view: &Rc<DocumentView>, index: usize, visible: bool) -> bool {
        let changed = self.automation.borrow_mut().session_mut().set_layer_visible(view.id, index, visible);
        if changed {
            view.refresh(&self.automation);
        }
        changed
    }

    pub(super) fn reset_layers(&self, view: &Rc<DocumentView>) -> bool {
        let changed = self.automation.borrow_mut().session_mut().reset_layers(view.id);
        if changed {
            view.refresh(&self.automation);
        }
        changed
    }
}

fn list_box() -> gtk::ListBox {
    let list = gtk::ListBox::new();
    list.set_css_classes(&["boxed-list"]);
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_margin_top(12);
    list.set_margin_bottom(12);
    list.set_margin_start(12);
    list.set_margin_end(12);
    list
}

/// A titled section: header row with title + action button over the list content.
fn section(title: &str, action: &gtk::Button, content: &impl IsA<gtk::Widget>) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.set_margin_top(12);
    header.set_margin_start(12);
    header.set_margin_end(12);
    let label = gtk::Label::new(Some(title));
    label.set_css_classes(&["heading"]);
    label.set_halign(gtk::Align::Start);
    label.set_hexpand(true);
    header.append(&label);
    header.append(action);
    root.append(&header);
    root.append(content);
    root
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.add_css_class("flat");
    button.set_tooltip_text(Some(tooltip));
    button.set_valign(gtk::Align::Center);
    button
}

fn row(title: &str, subtitle: Option<&str>) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(glib::markup_escape_text(title).as_ref());
    if let Some(subtitle) = subtitle {
        row.set_subtitle(glib::markup_escape_text(subtitle).as_ref());
    }
    row
}

fn fill_comments(list: &gtk::ListBox, info: &DocInfo, view: &Rc<DocumentView>, automation: &Shared, panel: &Rc<DocumentPanel>, editable: bool) {
    clear(list);
    for annotation in &info.annotations {
        let title = format!("Página {} · {}", annotation.page + 1, annotation.subtype);
        let subtitle = annotation.contents.as_deref().or(annotation.author.as_deref()).map(str::trim).filter(|text| !text.is_empty());
        let row = row(&title, subtitle);
        let page = annotation.page;
        let nav_view = Rc::clone(view);
        let nav_automation = Rc::clone(automation);
        row.connect_activated(move |_| {
            nav_view.go_to(page, &nav_automation);
        });
        if editable {
            let delete_btn = icon_button("user-trash-symbolic", "Eliminar comentario");
            let delete_panel = Rc::clone(panel);
            let delete_view = Rc::clone(view);
            let index = annotation.index;
            delete_btn.connect_clicked(move |_| {
                delete_panel.delete_comment(&delete_view, page, index);
            });
            row.add_suffix(&delete_btn);
        }
        list.append(&row);
    }
}

fn fill_bookmarks(list: &gtk::ListBox, info: &DocInfo, view: &Rc<DocumentView>, automation: &Shared, panel: &Rc<DocumentPanel>, editable: bool) {
    clear(list);
    let mut pending = vec![(info.outline.as_slice(), 0usize, Vec::<usize>::new())];
    while let Some((items, depth, prefix)) = pending.pop() {
        for (index, item) in items.iter().enumerate() {
            let mut path = prefix.clone();
            path.push(index);
            let indent = "  ".repeat(depth);
            let page_text = item.page.map_or_else(|| "sin página".to_string(), |page| format!("p. {}", page + 1));
            let row = row(&format!("{indent}{}", item.title), Some(&page_text));
            if let Some(page) = item.page {
                let nav_view = Rc::clone(view);
                let nav_automation = Rc::clone(automation);
                row.connect_activated(move |_| {
                    nav_view.go_to(page, &nav_automation);
                });
            }
            if editable {
                let rename_btn = icon_button("document-edit-symbolic", "Renombrar marcador");
                let rename_panel = Rc::clone(panel);
                let rename_view = Rc::clone(view);
                let title = item.title.clone();
                let rename_path = path.clone();
                rename_btn.connect_clicked(move |_| {
                    let prompt_path = rename_path.clone();
                    let prompt_view = Rc::clone(&rename_view);
                    let prompt_panel = Rc::clone(&rename_panel);
                    let entry_view = Rc::clone(&prompt_view);
                    prompt_entry(&entry_view, "Renombrar marcador", "Título", &title, move |text| {
                        prompt_panel.rename_bookmark(&prompt_view, prompt_path.clone(), &text);
                    });
                });
                row.add_suffix(&rename_btn);
                let delete_btn = icon_button("user-trash-symbolic", "Eliminar marcador");
                let delete_panel = Rc::clone(panel);
                let delete_view = Rc::clone(view);
                let delete_path = path.clone();
                delete_btn.connect_clicked(move |_| {
                    delete_panel.delete_bookmark(&delete_view, delete_path.clone());
                });
                row.add_suffix(&delete_btn);
            }
            list.append(&row);
            if !item.children.is_empty() {
                pending.push((item.children.as_slice(), depth + 1, path));
            }
        }
    }
}

fn fill_fields(list: &gtk::ListBox, info: &DocInfo, view: &Rc<DocumentView>, automation: &Shared, panel: &Rc<DocumentPanel>, editable: bool) {
    clear(list);
    for field in &info.fields {
        let title = format!("{} ({:?})", field.name, field.kind);
        let row = row(&title, field.value.as_deref());
        if let Some(page) = field.page {
            let nav_view = Rc::clone(view);
            let nav_automation = Rc::clone(automation);
            row.connect_activated(move |_| {
                nav_view.go_to(page, &nav_automation);
            });
        }
        if editable {
            match field.kind {
                FieldInfoKind::Text => {
                    let edit_btn = icon_button("document-edit-symbolic", "Editar valor");
                    let panel = Rc::clone(panel);
                    let view = Rc::clone(view);
                    let name = field.name.clone();
                    let initial = field.value.clone().unwrap_or_default();
                    edit_btn.connect_clicked(move |_| {
                        let apply_panel = Rc::clone(&panel);
                        let apply_view = Rc::clone(&view);
                        let apply_name = name.clone();
                        prompt_entry(&view, "Editar valor del campo", "Valor", &initial, move |text| {
                            apply_panel.set_field_value(&apply_view, &apply_name, FieldValue::Text(text));
                        });
                    });
                    row.add_suffix(&edit_btn);
                }
                FieldInfoKind::CheckBox => {
                    let toggle_btn = icon_button("object-toggle-symbolic", "Marcar o desmarcar");
                    let panel = Rc::clone(panel);
                    let view = Rc::clone(view);
                    let name = field.name.clone();
                    let checked = field.value.as_deref().is_some_and(|value| value == "Yes" || value == "On" || value == "1");
                    toggle_btn.connect_clicked(move |_| {
                        panel.set_field_value(&view, &name, FieldValue::Check(!checked));
                    });
                    row.add_suffix(&toggle_btn);
                }
                _ => {}
            }
        }
        list.append(&row);
    }
}

fn fill_layers(list: &gtk::ListBox, info: &DocInfo, view: &Rc<DocumentView>, _automation: &Shared, panel: &Rc<DocumentPanel>) {
    clear(list);
    for (index, layer) in info.layers.iter().enumerate() {
        let subtitle = if layer.visible { "visible" } else { "oculta" };
        let row = row(&layer.name, Some(subtitle));
        let switch = gtk::Switch::new();
        switch.set_active(layer.visible);
        switch.set_valign(gtk::Align::Center);
        let panel = Rc::clone(panel);
        let view = Rc::clone(view);
        let model_visible = layer.visible;
        switch.connect_active_notify(move |switch| {
            let active = switch.is_active();
            if active == model_visible {
                return;
            }
            if !panel.set_layer_visible(&view, index, active) {
                switch.set_active(model_visible);
            }
        });
        row.add_suffix(&switch);
        list.append(&row);
    }
}

fn fill_attachments(list: &gtk::ListBox, info: &DocInfo, view: &Rc<DocumentView>, _automation: &Shared, panel: &Rc<DocumentPanel>, editable: bool) {
    clear(list);
    for attachment in &info.attachments {
        let subtitle = attachment.size.map(|size| format!("{size} bytes"));
        let row = row(&attachment.name, subtitle.as_deref());
        if editable {
            let key = match &attachment.source {
                AttachmentSource::Document { key } => Some(key.clone()),
                AttachmentSource::Annotation { .. } => None,
            };
            if let Some(key) = key {
                let delete_btn = icon_button("user-trash-symbolic", "Eliminar adjunto");
                let panel = Rc::clone(panel);
                let view = Rc::clone(view);
                delete_btn.connect_clicked(move |_| {
                    panel.delete_attachment(&view, key.clone());
                });
                row.add_suffix(&delete_btn);
            }
        }
        list.append(&row);
    }
}

type SignatureRow = (String, bool, Option<String>, Option<usize>);

fn fill_signatures(list: &gtk::ListBox, signatures: &[SignatureRow], view: &Rc<DocumentView>, automation: &Shared) {
    clear(list);
    for (field, signed, signer, page) in signatures {
        let state = if *signed {
            format!("firmado{}", signer.as_deref().map_or_else(String::new, |name| format!(" por {name}")))
        } else {
            String::from("sin firmar")
        };
        let subtitle = page.map_or(state.clone(), |page| format!("{state} · p. {}", page + 1));
        let row = row(field, Some(&subtitle));
        if let Some(page) = page {
            let page = *page;
            let view = Rc::clone(view);
            let automation = Rc::clone(automation);
            row.connect_activated(move |_| {
                view.go_to(page, &automation);
            });
        }
        list.append(&row);
    }
}

fn clear(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn fingerprint(id: DocId, info: &DocInfo, signature_count: usize) -> u64 {
    let mut value = id.0;
    value = value.wrapping_mul(31).wrapping_add(info.annotations.len() as u64);
    value = value.wrapping_mul(31).wrapping_add(info.fields.len() as u64);
    value = value.wrapping_mul(31).wrapping_add(info.layers.len() as u64);
    value = value.wrapping_mul(31).wrapping_add(info.attachments.len() as u64);
    value = value.wrapping_mul(31).wrapping_add(info.pages.len() as u64);
    value = value.wrapping_mul(31).wrapping_add(signature_count as u64);
    for layer in &info.layers {
        value = value.wrapping_mul(31).wrapping_add(layer.visible as u64);
    }
    for field in &info.fields {
        let mut field_hash = field.kind as u64;
        if let Some(text) = field.value.as_deref() {
            for byte in text.bytes() {
                field_hash = field_hash.wrapping_mul(31).wrapping_add(byte as u64);
            }
        }
        value = value.wrapping_mul(31).wrapping_add(field_hash);
    }
    let mut outline_total = 0u64;
    let mut stack = vec![info.outline.as_slice()];
    while let Some(items) = stack.pop() {
        outline_total = outline_total.wrapping_add(items.len() as u64);
        for item in items {
            stack.push(item.children.as_slice());
        }
    }
    value = value.wrapping_mul(31).wrapping_add(outline_total);
    value
}
