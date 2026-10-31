use super::DocumentView;
use adw::{glib, gtk, prelude::*};
use libadwaita as adw;
use pdfcraft_engine::{
    AttachIcon, Edit, FillMark, LineEnding, Markup, NewAnnotation, NoteIcon, OverlayLook, Shape, Style, displayed_offset, rect_quad, text_box_at,
    typed_signature_shape,
};

impl DocumentView {
    pub fn set_annotation_tool(&self, tool: Option<String>) {
        *self.annotation_tool.borrow_mut() = tool;
        self.page_box.set_cursor_from_name(if self.annotation_tool.borrow().is_some() { Some("crosshair") } else { None });
    }

    pub fn annotation_tool_is(&self, tool: &str) -> bool {
        self.annotation_tool.borrow().as_deref() == Some(tool)
    }

    fn page_rotation(&self, page: usize) -> i64 {
        self.info.borrow().pages.get(page).map_or(0, |info| i64::from(info.rotation))
    }

    /// Coloca una marca de Fill & Sign (visto, cruz, punto, guion) centrada en `at`.
    pub fn place_fill_mark(&self, page: usize, mark: FillMark, at: [f64; 2]) {
        let rotation = self.page_rotation(page);
        let rect = mark.rect_at(at, rotation);
        self.commit_shape(page, Shape::Mark { rect, mark }, "");
    }

    /// Coloca texto mecanografiado con su borde superior mostrado en `at` (el mismo encuadre que
    /// el original: displayed_offset con la sexta parte del tamaño por encima del clic).
    pub fn place_typed_text(&self, page: usize, at: [f64; 2], text: &str, font_size: f64) {
        let rotation = self.page_rotation(page);
        let top = displayed_offset(rotation, at, 0.0, font_size * 0.6);
        let width = (text.chars().count().max(4) as f64) * font_size * 0.6;
        let rect = text_box_at(rotation, top, width, font_size).unwrap_or([top[0], top[1] - font_size, top[0] + width, top[1]]);
        self.commit_shape(page, Shape::Typewriter { rect, font_size }, text);
    }

    /// Coloca la fecha de hoy como texto mecanografiado.
    pub fn place_date(&self, page: usize, at: [f64; 2]) {
        let Some(today) = glib::DateTime::now_local().ok().and_then(|date| date.format("%d/%m/%Y").ok()) else { return };
        self.place_typed_text(page, at, today.as_str(), 10.0);
    }

    /// Coloca la firma o iniciales guardadas (texto tipografiado en fuente script); si no hay
    /// ninguna guardada abre el diálogo de creación y no coloca nada.
    pub fn place_saved_signature(&self, page: usize, at: [f64; 2], initials: bool) -> bool {
        let Some(saved) = crate::app::native::saved_signature(initials) else {
            self.open_create_signature_prompt(initials);
            return false;
        };
        let rotation = self.page_rotation(page);
        let height = if initials { 24.0 } else { 32.0 };
        match typed_signature_shape(at, &saved, height, rotation) {
            Some(shape) => {
                self.commit_shape(page, shape, "");
                true
            }
            None => false,
        }
    }

    /// Diálogo de texto libre para la herramienta sign.fill.text.
    pub fn open_fill_text_prompt(&self, page: usize, at: [f64; 2]) {
        let widget = self.root.upcast_ref::<gtk::Widget>();
        let Some(window) = widget.root().and_downcast::<gtk::Window>() else { return };
        let entry = adw::EntryRow::new();
        entry.set_title("Texto a colocar");
        entry.set_activates_default(true);
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.append(&entry);
        let cancel = gtk::Button::with_label("Cancelar");
        let apply = gtk::Button::with_label("Colocar");
        apply.add_css_class("suggested-action");
        apply.set_sensitive(false);
        let apply_hint = apply.clone();
        entry.connect_changed(move |entry| apply_hint.set_sensitive(!entry.text().trim().is_empty()));
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&apply);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.append(&list);
        content.append(&buttons);
        let dialog = adw::Window::builder().title("Añadir texto").modal(true).transient_for(&window).build();
        dialog.set_default_size(440, 180);
        if let Some(application) = window.application() {
            dialog.set_application(Some(&application));
        }
        dialog.set_content(Some(&content));
        let view_weak = self.weak_self.borrow().clone();
        let dialog_apply = dialog.clone();
        let entry_apply = entry.clone();
        apply.connect_clicked(move |_| {
            let text = entry_apply.text().trim().to_string();
            dialog_apply.close();
            if let Some(view) = view_weak.upgrade()
                && !text.is_empty()
            {
                view.place_typed_text(page, at, &text, 10.0);
            }
        });
        let dialog_cancel = dialog.clone();
        cancel.connect_clicked(move |_| dialog_cancel.close());
        dialog.present();
    }

    /// Diálogo de firma digital con certificado .p12.
    pub fn open_sign_dialog(&self, page: usize, rect: Option<[f64; 4]>, certify: Option<u8>) {
        let widget = self.root.upcast_ref::<gtk::Widget>();
        let Some(window) = widget.root().and_downcast::<gtk::Window>() else { return };
        let heading = match certify {
            Some(_) => "Certificar documento",
            None if rect.is_none() => "Firma invisible",
            None => "Firmar digitalmente",
        };
        let id_entry = adw::EntryRow::new();
        id_entry.set_title("Certificado (.p12)");
        let saved_id = crate::app::native::saved_sign_id();
        if !saved_id.is_empty() {
            id_entry.set_text(&saved_id);
        }
        let password = adw::PasswordEntryRow::new();
        password.set_title("Contraseña del certificado");
        let reason = adw::EntryRow::new();
        reason.set_title("Motivo (opcional)");
        let location = adw::EntryRow::new();
        location.set_title("Ubicación (opcional)");
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.append(&id_entry);
        list.append(&password);
        list.append(&reason);
        list.append(&location);
        let cancel = gtk::Button::with_label("Cancelar");
        let apply = gtk::Button::with_label("Firmar");
        apply.add_css_class("suggested-action");
        apply.set_sensitive(false);
        let apply_a = apply.clone();
        let apply_b = apply.clone();
        let id_a = id_entry.clone();
        let id_b = id_entry.clone();
        let password_a = password.clone();
        let password_b = password.clone();
        let update_sensitivity = move || {
            let ready = !id_a.text().trim().is_empty() && !password_a.text().is_empty();
            apply_a.set_sensitive(ready);
        };
        id_entry.connect_changed(move |_| update_sensitivity());
        password.connect_changed(move |_| {
            let ready = !id_b.text().trim().is_empty() && !password_b.text().is_empty();
            apply_b.set_sensitive(ready);
        });
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&apply);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.append(&list);
        content.append(&buttons);
        let dialog = adw::Window::builder().title(heading).modal(true).transient_for(&window).build();
        dialog.set_default_size(460, 300);
        if let Some(application) = window.application() {
            dialog.set_application(Some(&application));
        }
        dialog.set_content(Some(&content));
        let view_weak = self.weak_self.borrow().clone();
        let dialog_apply = dialog.clone();
        apply.connect_clicked(move |_| {
            let id_path = id_entry.text().trim().to_string();
            let password_text = password.text().to_string();
            let reason_text = reason.text().trim().to_string();
            let location_text = location.text().trim().to_string();
            dialog_apply.close();
            if let Some(view) = view_weak.upgrade()
                && !id_path.is_empty()
            {
                crate::app::signing::request_sign_output(
                    &view,
                    crate::app::signing::SignRequest {
                        page,
                        rect,
                        certify,
                        id_path,
                        password: password_text,
                        reason: reason_text,
                        location: location_text,
                    },
                );
            }
        });
        let dialog_cancel = dialog.clone();
        cancel.connect_clicked(move |_| dialog_cancel.close());
        dialog.present();
        crate::app::signing::note_dialog(&dialog);
    }

    /// Diálogo para crear la firma o iniciales guardadas (texto tipografiado; dibujo e imagen
    /// quedan pendientes de paridad).
    pub fn open_create_signature_prompt(&self, initials: bool) {
        let widget = self.root.upcast_ref::<gtk::Widget>();
        let Some(window) = widget.root().and_downcast::<gtk::Window>() else { return };
        let what = if initials { "iniciales" } else { "firma" };
        let entry = adw::EntryRow::new();
        entry.set_title(if initials { "Tus iniciales" } else { "Tu nombre" });
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.append(&entry);
        let hint = gtk::Label::new(Some(&format!("Escribe tu {what}. Se guarda para colocarla con un clic.")));
        hint.set_wrap(true);
        hint.add_css_class("dim-label");
        let cancel = gtk::Button::with_label("Cancelar");
        let apply = gtk::Button::with_label("Guardar");
        apply.add_css_class("suggested-action");
        apply.set_sensitive(false);
        let apply_hint = apply.clone();
        entry.connect_changed(move |entry| apply_hint.set_sensitive(!entry.text().trim().is_empty()));
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        buttons.append(&cancel);
        buttons.append(&apply);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.append(&hint);
        content.append(&list);
        content.append(&buttons);
        let dialog =
            adw::Window::builder().title(if initials { "Crear iniciales" } else { "Crear firma" }).modal(true).transient_for(&window).build();
        dialog.set_default_size(440, 210);
        if let Some(application) = window.application() {
            dialog.set_application(Some(&application));
        }
        dialog.set_content(Some(&content));
        let dialog_apply = dialog.clone();
        let entry_apply = entry.clone();
        apply.connect_clicked(move |_| {
            let text = entry_apply.text().trim().to_string();
            crate::app::native::save_saved_signature(initials, &text);
            dialog_apply.close();
        });
        let dialog_cancel = dialog.clone();
        cancel.connect_clicked(move |_| dialog_cancel.close());
        dialog.present();
    }

    pub fn set_attachment(&self, file: String, data: Vec<u8>) {
        *self.attachment.borrow_mut() = Some((file, data));
    }

    pub(super) fn install_annotation_gestures(&self, picture: &gtk::Picture, page: usize) {
        let click = gtk::GestureClick::new();
        let weak = self.weak_self.borrow().clone();
        let picture_click = picture.clone();
        click.connect_pressed(move |_, _, x, y| {
            let Some(view) = weak.upgrade() else { return };
            let Some(tool) = view.annotation_tool.borrow().clone() else { return };
            let start = view.user_point(&picture_click, page, x, y);
            match tool.as_str() {
                "comment.note" => view.commit_shape(page, Shape::Note { at: start, icon: NoteIcon::Comment }, ""),
                "comment.caret" => view.commit_shape(page, Shape::Caret { rect: [start[0], start[1] - 18.0, start[0] + 8.0, start[1]] }, ""),
                "comment.attach" => {
                    if let Some((file, data)) = view.attachment.borrow().clone() {
                        view.commit_shape(page, Shape::Attachment { at: start, icon: AttachIcon::PushPin, file, data }, "");
                    }
                }
                "comment.eraser" => view.erase_at(page, start),
                "sign.fill.check" => view.place_fill_mark(page, FillMark::Check, start),
                "sign.fill.cross" => view.place_fill_mark(page, FillMark::Cross, start),
                "sign.fill.dot" => view.place_fill_mark(page, FillMark::Dot, start),
                "sign.fill.line" => view.place_fill_mark(page, FillMark::Line, start),
                "sign.fill.date" => view.place_date(page, start),
                "sign.fill.text" => view.open_fill_text_prompt(page, start),
                "sign.fill.signature" => {
                    view.place_saved_signature(page, start, false);
                }
                "sign.fill.initials" => {
                    view.place_saved_signature(page, start, true);
                }
                _ => {}
            }
        });
        picture.add_controller(click);

        let drag = gtk::GestureDrag::new();
        let weak = self.weak_self.borrow().clone();
        drag.connect_drag_begin(move |_, x, y| {
            let Some(view) = weak.upgrade() else { return };
            if view.annotation_tool.borrow().is_none() {
                return;
            }
            *view.drag.borrow_mut() = Some((x, y, 0.0, 0.0));
            view.annotation_stroke.borrow_mut().clear();
        });
        let weak = self.weak_self.borrow().clone();
        let picture_update = picture.clone();
        drag.connect_drag_update(move |_, x, y| {
            let Some(view) = weak.upgrade() else { return };
            let Some((start_x, start_y, _, _)) = *view.drag.borrow() else { return };
            *view.drag.borrow_mut() = Some((start_x, start_y, x, y));
            if view.annotation_tool.borrow().as_deref() == Some("comment.ink") {
                let point = view.user_point(&picture_update, page, start_x + x, start_y + y);
                view.annotation_stroke.borrow_mut().push(point);
            }
        });
        let weak = self.weak_self.borrow().clone();
        let picture_end = picture.clone();
        drag.connect_drag_end(move |_, x, y| {
            let Some(view) = weak.upgrade() else { return };
            let Some(tool) = view.annotation_tool.borrow().clone() else { return };
            let Some((start_x, start_y, _, _)) = view.drag.borrow_mut().take() else { return };
            let start = view.user_point(&picture_end, page, start_x, start_y);
            let end = view.user_point(&picture_end, page, start_x + x, start_y + y);
            if tool == "comment.ink" {
                let strokes = vec![view.annotation_stroke.borrow().clone()];
                if strokes[0].len() >= 2 {
                    view.commit_shape(page, Shape::Ink { strokes }, "");
                }
                return;
            }
            if tool == "comment.redact" {
                let rect = [start[0].min(end[0]), start[1].min(end[1]), start[0].max(end[0]), start[1].max(end[1])];
                let (overlay, use_overlay) = crate::app::native::redact_defaults();
                let shape = Shape::Redact {
                    quads: vec![rect_quad(rect)],
                    overlay: if use_overlay { overlay } else { String::new() },
                    look: OverlayLook::default(),
                };
                view.commit_shape(page, shape, "");
                return;
            }
            if tool == "sign.digital" || tool == "sign.certify" {
                let rect = [start[0].min(end[0]), start[1].min(end[1]), start[0].max(end[0]), start[1].max(end[1])];
                let certify = (tool == "sign.certify").then_some(1u8);
                view.open_sign_dialog(page, Some(rect), certify);
                return;
            }
            if let Some(shape) = drag_shape(&tool, start, end) {
                let contents = match tool.as_str() {
                    "comment.freetext" | "comment.callout" => "Texto",
                    "comment.replace" => "Reemplazo",
                    _ => "",
                };
                view.commit_shape(page, shape, contents);
            }
        });
        picture.add_controller(drag);
    }

    fn user_point(&self, picture: &gtk::Picture, page: usize, x: f64, y: f64) -> [f64; 2] {
        let info = self.info.borrow();
        let Some(page_info) = info.pages.get(page) else { return [0.0, 0.0] };
        let width = f64::from(picture.width()).max(1.0);
        let height = f64::from(picture.height()).max(1.0);
        let view_x = x / width * f64::from(page_info.width);
        let view_y = y / height * f64::from(page_info.height);
        let point = page_info.view_to_user(view_x as f32, view_y as f32);
        [f64::from(point[0]), f64::from(point[1])]
    }

    fn commit_shape(&self, page: usize, shape: Shape, contents: &str) {
        let style = Style::default_for(&shape);
        let annotation = NewAnnotation { page, shape, style, contents: contents.to_string(), author: "PdfCraft".to_string() };
        let result = self.automation.borrow_mut().session_mut().apply(self.id, Edit::AddAnnotation(annotation));
        if result.is_ok() {
            self.refresh(&self.automation);
        }
    }

    fn erase_at(&self, page: usize, point: [f64; 2]) {
        let index = self
            .info
            .borrow()
            .annotations
            .iter()
            .find(|annotation| {
                let rect = annotation.rect.map(f64::from);
                annotation.page == page && point[0] >= rect[0] && point[0] <= rect[2] && point[1] >= rect[1] && point[1] <= rect[3]
            })
            .map(|annotation| annotation.index);
        if let Some(index) = index {
            let result = self.automation.borrow_mut().session_mut().apply(self.id, Edit::DeleteAnnotation { page, index });
            if result.is_ok() {
                self.refresh(&self.automation);
            }
        }
    }
}

fn drag_shape(tool: &str, start: [f64; 2], end: [f64; 2]) -> Option<Shape> {
    let rect = [start[0].min(end[0]), start[1].min(end[1]), start[0].max(end[0]), start[1].max(end[1])];
    let quad = [rect[0], rect[3], rect[2], rect[3], rect[0], rect[1], rect[2], rect[1]];
    let markup = |kind| Shape::TextMarkup { kind, quads: vec![quad] };
    let vertices = vec![start, [end[0], start[1]], end, [start[0], end[1]]];
    Some(match tool {
        "comment.highlight" => markup(Markup::Highlight),
        "comment.underline" => markup(Markup::Underline),
        "comment.strikeout" => markup(Markup::StrikeOut),
        "comment.squiggly" => markup(Markup::Squiggly),
        "comment.square" => Shape::Rectangle { rect },
        "comment.circle" => Shape::Oval { rect },
        "comment.line" => Shape::Line { from: start, to: end, start: LineEnding::None, end: LineEnding::None },
        "comment.arrow" => Shape::Line { from: start, to: end, start: LineEnding::None, end: LineEnding::OpenArrow },
        "comment.polygon" => Shape::Polygon { vertices, cloud: false },
        "comment.cloud" => Shape::Polygon { vertices, cloud: true },
        "comment.polyline" => Shape::PolyLine { vertices, start: LineEnding::None, end: LineEnding::None },
        "comment.freetext" => Shape::TextBox { rect, font_size: 12.0 },
        "comment.callout" => Shape::Callout { rect, knee: [rect[0], rect[1]], point: start, font_size: 12.0, ending: LineEnding::OpenArrow },
        "comment.replace" => markup(Markup::StrikeOut),
        _ => return None,
    })
}
