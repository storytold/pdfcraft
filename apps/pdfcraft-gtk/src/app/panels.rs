//! Side panels: tools (annotation quick tools) and document lists with real page thumbnails.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use adw::{glib, gtk, prelude::*};
use libadwaita as adw;
use pdfcraft_automation::Automation;
use pdfcraft_engine::{
    AttachmentSource, DocId, DocInfo, Edit, FieldInfoKind, FieldProps, FieldValue, RenderConfig, RenderPool, RenderRequest, RequestKind,
};

use super::APP;
use crate::document::DocumentView;
use pdfcraft_engine::catalog::{self, Availability};

const THUMB_HEIGHT: f32 = 128.0;
const THUMB_TAG: u64 = 7;

/// (etiqueta, comando, icono lucide, color de la herramienta)
const ANNOT_TOOLS: &[(&str, &str, &str, &str)] = &[
    ("Nota", "comment.note", "message-square-text", "#FFD100"),
    ("Texto", "comment.freetext", "type", "#222226"),
    ("Resaltar", "comment.highlight", "highlighter", "#FFF000"),
    ("Subrayar", "comment.underline", "underline", "#0078D4"),
    ("Tachar", "comment.strikeout", "strikethrough", "#E32121"),
    ("Ondular", "comment.squiggly", "spline", "#2E9E5C"),
    ("Tinta", "comment.ink", "pen-line", "#0066DE"),
    ("Línea", "comment.line", "minus", "#E32121"),
    ("Flecha", "comment.arrow", "arrow-up-right", "#E32121"),
    ("Rectángulo", "comment.square", "square", "#E32121"),
    ("Óvalo", "comment.circle", "circle", "#E32121"),
    ("Polígono", "comment.polygon", "pencil", "#E32121"),
    ("Nube", "comment.cloud", "cloud", "#E32121"),
    ("Polilínea", "comment.polyline", "spline", "#E32121"),
    ("Llamada", "comment.callout", "message-square", "#222226"),
    ("Marca", "comment.caret", "superscript", "#0078D4"),
    ("Reemplazo", "comment.replace", "file-input", "#0078D4"),
    ("Redactar", "comment.redact", "rectangle-horizontal", "#E03E3E"),
    ("Borrar", "comment.eraser", "eraser", "#5E5E66"),
    ("Texto", "sign.fill.text", "text-cursor-input", "#8E4EE6"),
    ("Visto", "sign.fill.check", "square", "#8E4EE6"),
    ("Cruz", "sign.fill.cross", "circle", "#8E4EE6"),
    ("Punto", "sign.fill.dot", "circle", "#8E4EE6"),
    ("Guion", "sign.fill.line", "minus", "#8E4EE6"),
    ("Fecha", "sign.fill.date", "type", "#8E4EE6"),
    ("Firma", "sign.fill.signature", "pen-line", "#8E4EE6"),
    ("Iniciales", "sign.fill.initials", "signature", "#8E4EE6"),
    ("Firmar", "sign.digital", "signature", "#8E4EE6"),
    ("Certificar", "sign.certify", "badge-check", "#149CA8"),
];

fn tinted_icon(icon: &str, color: &str) -> gtk::Image {
    let image = gtk::Image::new();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/icons").join(format!("{icon}.svg"));
    if let Ok(bytes) = std::fs::read(&path) {
        let painted = String::from_utf8_lossy(&bytes).replace("currentColor", color);
        if let Ok(texture) = gtk::gdk::Texture::from_bytes(&glib::Bytes::from(painted.as_bytes())) {
            image.set_paintable(Some(&texture));
            return image;
        }
    }
    image.set_icon_name(Some("image-missing-symbolic"));
    image
}

fn availability_chip(availability: Availability) -> Option<String> {
    match availability {
        Availability::Ready => None,
        Availability::Planned(reason) => Some(reason.to_string()),
        Availability::Provider => Some("requiere motor externo".to_string()),
    }
}

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
    on_command: RefCell<Option<Rc<dyn Fn(&str)>>>,
    rail_groups: Cell<usize>,
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
            on_command: RefCell::new(None),
            rail_groups: Cell::new(0),
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

    pub fn set_command_runner(&self, runner: Rc<dyn Fn(&str)>) {
        *self.on_command.borrow_mut() = Some(runner);
    }

    pub fn rail_group_count(&self) -> usize {
        self.rail_groups.get()
    }

    pub fn tool_button_count(&self) -> usize {
        self.tool_buttons.borrow().len()
    }

    pub fn build_tools(self: &Rc<Self>) {
        let heading = gtk::Label::new(Some("Herramientas"));
        heading.set_css_classes(&["heading"]);
        heading.set_halign(gtk::Align::Start);
        heading.set_margin_top(6);
        let top = gtk::Box::new(gtk::Orientation::Vertical, 0);
        top.append(&heading);
        let grid = gtk::FlowBox::new();
        grid.set_max_children_per_line(3);
        grid.set_min_children_per_line(3);
        grid.set_selection_mode(gtk::SelectionMode::None);
        grid.set_homogeneous(true);
        for (label, id, icon, color) in ANNOT_TOOLS {
            let button = gtk::ToggleButton::new();
            let image = tinted_icon(icon, color);
            image.set_pixel_size(18);
            button.set_child(Some(&image));
            button.set_tooltip_text(Some(label));
            self.tool_buttons.borrow_mut().push(((*id).to_string(), button.clone()));
            grid.append(&button);
        }
        let none = gtk::ToggleButton::with_label("Ninguna");
        none.set_active(true);
        none.set_tooltip_text(Some("Ninguna herramienta"));
        self.tool_buttons.borrow_mut().push((String::new(), none.clone()));
        grid.append(&none);
        top.append(&grid);

        let commands = gtk::Label::new(Some("Comandos"));
        commands.set_css_classes(&["heading"]);
        commands.set_halign(gtk::Align::Start);
        commands.set_margin_top(10);
        top.append(&commands);
        let mut group_count = 0usize;
        for category in catalog::TOOL_CATEGORIES {
            let category_label = gtk::Label::new(Some(category.label));
            category_label.set_css_classes(&["dim-label"]);
            category_label.set_halign(gtk::Align::Start);
            category_label.set_margin_top(6);
            top.append(&category_label);
            for group_id in category.groups {
                let Some(group) = catalog::group(group_id) else { continue };
                group_count += 1;
                let hex = format!("#{0:02X}{1:02X}{2:02X}", group.hue[0], group.hue[1], group.hue[2]);
                let items = gtk::FlowBox::new();
                items.set_max_children_per_line(3);
                items.set_min_children_per_line(2);
                items.set_selection_mode(gtk::SelectionMode::None);
                items.set_homogeneous(true);
                items.set_margin_start(28);
                for section in group.sections {
                    for item in section.items {
                        let button = gtk::Button::new();
                        button.add_css_class("flat");
                        let image = tinted_icon(item.icon, &hex);
                        image.set_pixel_size(18);
                        button.set_child(Some(&image));
                        button.set_tooltip_text(Some(item.label));
                        if !matches!(item.availability, Availability::Ready) {
                            button.set_sensitive(false);
                        }
                        let runner = self.on_command.borrow().clone();
                        let command = item.command.to_string();
                        button.connect_clicked(move |_| {
                            if let Some(runner) = runner.as_ref() {
                                runner(&command);
                            }
                        });
                        items.append(&button);
                    }
                }
                let revealer = gtk::Revealer::new();
                revealer.set_child(Some(&items));
                revealer.set_reveal_child(false);
                let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let group_icon = tinted_icon(group.icon, &hex);
                group_icon.set_pixel_size(20);
                row_box.append(&group_icon);
                let group_label = gtk::Label::new(Some(group.label));
                group_label.set_halign(gtk::Align::Start);
                row_box.append(&group_label);
                let chip = availability_chip(group.availability);
                if let Some(chip) = chip.as_deref() {
                    let chip_label = gtk::Label::new(Some(chip));
                    chip_label.set_css_classes(&["dim-label"]);
                    chip_label.set_halign(gtk::Align::Start);
                    row_box.append(&chip_label);
                }
                let row = gtk::Button::new();
                row.add_css_class("flat");
                row.set_child(Some(&row_box));
                row.set_halign(gtk::Align::Fill);
                let revealer_toggle = revealer.clone();
                row.connect_clicked(move |_| {
                    revealer_toggle.set_reveal_child(!revealer_toggle.reveals_child());
                });
                top.append(&row);
                top.append(&revealer);
            }
        }
        self.rail_groups.set(group_count);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_vexpand(true);
        scroll.set_child(Some(&top));
        self.tools_root.append(&scroll);

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

    pub(super) fn move_bookmark(&self, view: &Rc<DocumentView>, from: Vec<usize>, to_parent: Vec<usize>, index: usize) -> bool {
        apply_edit(view, &self.automation, Edit::MoveBookmark { from, to_parent, index })
    }

    pub(super) fn set_bookmark_page(&self, view: &Rc<DocumentView>, path: Vec<usize>, page: usize) -> bool {
        apply_edit(view, &self.automation, Edit::SetBookmarkPage { path, page })
    }

    pub(super) fn set_field_props(&self, view: &Rc<DocumentView>, name: &str, tooltip: String, read_only: bool, required: bool) -> bool {
        apply_edit(
            view,
            &self.automation,
            Edit::SetFieldProps {
                name: name.to_string(),
                props: Box::new(FieldProps { tooltip: Some(tooltip), read_only: Some(read_only), required: Some(required), ..FieldProps::default() }),
            },
        )
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
                row.add_suffix(&bookmark_menu_button(panel, view, prefix.clone(), index, depth, path.clone()));
            }
            list.append(&row);
            if !item.children.is_empty() {
                pending.push((item.children.as_slice(), depth + 1, path));
            }
        }
    }
}

/// Overflow menu for one bookmark row: reorder, indent, retarget, besides the inline rename/delete.
fn bookmark_menu_button(
    panel: &Rc<DocumentPanel>,
    view: &Rc<DocumentView>,
    prefix: Vec<usize>,
    index: usize,
    depth: usize,
    path: Vec<usize>,
) -> gtk::MenuButton {
    let menu = gtk::MenuButton::new();
    menu.set_icon_name("view-more-symbolic");
    menu.set_tooltip_text(Some("Más acciones de marcador"));
    menu.add_css_class("flat");
    let list = gtk::ListBox::new();
    list.set_css_classes(&["boxed-list"]);
    list.set_selection_mode(gtk::SelectionMode::None);
    let popover = gtk::Popover::new();
    popover.set_child(Some(&list));
    menu.set_popover(Some(&popover));
    let entries: [(&str, bool, Box<dyn Fn()>); 5] = [
        (
            "Subir",
            index > 0,
            Box::new({
                let panel = Rc::clone(panel);
                let view = Rc::clone(view);
                let from = path.clone();
                let to_parent = prefix.clone();
                move || {
                    panel.move_bookmark(&view, from.clone(), to_parent.clone(), index - 1);
                }
            }),
        ),
        (
            "Bajar",
            true,
            Box::new({
                let panel = Rc::clone(panel);
                let view = Rc::clone(view);
                let from = path.clone();
                let to_parent = prefix.clone();
                move || {
                    panel.move_bookmark(&view, from.clone(), to_parent.clone(), index + 1);
                }
            }),
        ),
        (
            "Indentar",
            index > 0,
            Box::new({
                let panel = Rc::clone(panel);
                let view = Rc::clone(view);
                let from = path.clone();
                let mut to_parent = prefix.clone();
                to_parent.push(index.saturating_sub(1));
                move || {
                    panel.move_bookmark(&view, from.clone(), to_parent.clone(), usize::MAX);
                }
            }),
        ),
        (
            "Quitar indentación",
            depth > 0,
            Box::new({
                let panel = Rc::clone(panel);
                let view = Rc::clone(view);
                let from = path.clone();
                let parent_last = prefix.last().copied().unwrap_or(0);
                let to_parent = prefix[..prefix.len().saturating_sub(1)].to_vec();
                move || {
                    panel.move_bookmark(&view, from.clone(), to_parent.clone(), parent_last + 1);
                }
            }),
        ),
        (
            "Fijar a la página actual",
            true,
            Box::new({
                let panel = Rc::clone(panel);
                let view = Rc::clone(view);
                let target = path.clone();
                move || {
                    panel.set_bookmark_page(&view, target.clone(), view.current());
                }
            }),
        ),
    ];
    for (label, enabled, action) in entries {
        let button = gtk::Button::with_label(label);
        button.add_css_class("flat");
        button.set_halign(gtk::Align::Fill);
        button.set_sensitive(enabled);
        let popover = popover.clone();
        button.connect_clicked(move |_| {
            popover.popdown();
            action();
        });
        list.append(&button);
    }
    menu
}

/// Modal editor for the basic field properties (tooltip, read only, required).
fn open_field_props(view: &Rc<DocumentView>, name: &str, tooltip: &str, panel: &Rc<DocumentPanel>) {
    let window = app_window();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);
    let tooltip_row = adw::EntryRow::new();
    tooltip_row.set_title("Descripción emergente");
    tooltip_row.set_text(tooltip);
    let read_only = gtk::Switch::new();
    read_only.set_valign(gtk::Align::Center);
    let read_only_row = adw::ActionRow::new();
    read_only_row.set_title("Solo lectura");
    read_only_row.add_suffix(&read_only);
    read_only_row.set_activatable_widget(Some(&read_only));
    let required = gtk::Switch::new();
    required.set_valign(gtk::Align::Center);
    let required_row = adw::ActionRow::new();
    required_row.set_title("Obligatorio");
    required_row.add_suffix(&required);
    required_row.set_activatable_widget(Some(&required));
    let list = gtk::ListBox::new();
    list.set_css_classes(&["boxed-list"]);
    list.set_selection_mode(gtk::SelectionMode::None);
    list.append(&tooltip_row);
    list.append(&read_only_row);
    list.append(&required_row);
    content.append(&list);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancelar");
    let save = gtk::Button::with_label("Guardar");
    save.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&save);
    content.append(&buttons);
    let dialog = adw::Window::builder().title(format!("Propiedades de {name}")).modal(true).build();
    if let Some(window) = &window {
        dialog.set_transient_for(Some(window));
    }
    dialog.set_content(Some(&content));
    dialog.set_default_size(440, 220);
    let dialog_cancel = dialog.clone();
    cancel.connect_clicked(move |_| dialog_cancel.close());
    let dialog_save = dialog.clone();
    let tooltip_save = tooltip_row.clone();
    let read_only_save = read_only.clone();
    let required_save = required.clone();
    let panel_save = Rc::clone(panel);
    let view_save = Rc::clone(view);
    let name_save = name.to_string();
    save.connect_clicked(move |_| {
        dialog_save.close();
        panel_save.set_field_props(
            &view_save,
            &name_save,
            tooltip_save.text().trim().to_string(),
            read_only_save.is_active(),
            required_save.is_active(),
        );
    });
    dialog.present();
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
            let props_btn = icon_button("document-properties-symbolic", "Propiedades del campo");
            let props_panel = Rc::clone(panel);
            let props_view = Rc::clone(view);
            let props_name = field.name.clone();
            let props_tooltip = field.tooltip.clone().unwrap_or_default();
            props_btn.connect_clicked(move |_| {
                open_field_props(&props_view, &props_name, &props_tooltip, &props_panel);
            });
            row.add_suffix(&props_btn);
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
