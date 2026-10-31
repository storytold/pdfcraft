use adw::{gtk, prelude::*};
use libadwaita as adw;
use pdfcraft_automation::ToolDef;
use serde_json::{Map, Value};

pub fn present(parent: &adw::ApplicationWindow, definition: &ToolDef, defaults: Value, on_submit: impl Fn(Value) + 'static) {
    let dialog = adw::Window::builder().title(definition.title).default_width(680).default_height(760).transient_for(parent).modal(true).build();
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let cancel = gtk::Button::with_label("Cancel");
    let submit = gtk::Button::with_label("Run");
    submit.add_css_class("suggested-action");
    if definition.destructive {
        submit.remove_css_class("suggested-action");
        submit.add_css_class("destructive-action");
    }
    header.pack_start(&cancel);
    header.pack_end(&submit);
    toolbar.add_top_bar(&header);

    let group = adw::PreferencesGroup::new();
    group.set_title(definition.title);
    group.set_description(Some(definition.description));
    let mut fields = Vec::new();
    let empty = Map::new();
    let properties = definition.input_schema.get("properties").and_then(Value::as_object).unwrap_or(&empty);
    let required = definition.input_schema.get("required").and_then(Value::as_array).cloned().unwrap_or_default();
    for (name, schema) in properties {
        let is_required = required.iter().any(|value| value.as_str() == Some(name));
        let default = defaults.get(name).cloned().or_else(|| schema.get("default").cloned());
        let field = Field::new(name, schema, is_required, default);
        group.add(field.widget());
        fields.push(field);
    }
    let error = gtk::Label::new(None);
    if std::env::var("PDFCRAFT_GTK_QA").as_deref() == Ok("form") && definition.command == Some("export.text") {
        for field in &fields {
            if let Field::Text { name, row, .. } = field
                && name == "path"
            {
                row.set_text(&crate::capture::artifact("exported.txt").to_string_lossy());
            }
        }
    }
    let picker_parent = dialog.clone();
    for field in &fields {
        let Field::Text { name, row, kind: FieldKind::String, .. } = field else {
            continue;
        };
        let row = row.clone();
        match name.as_str() {
            "folder" => {
                let button = gtk::Button::from_icon_name("folder-open-symbolic");
                button.set_tooltip_text(Some("Elegir carpeta"));
                button.add_css_class("flat");
                let parent = picker_parent.clone();
                let row_cb = row.clone();
                button.connect_clicked(move |_| {
                    let row = row_cb.clone();
                    gtk::FileDialog::new().select_folder(Some(&parent), gtk::gio::Cancellable::NONE, move |file| {
                        if let Ok(file) = file
                            && let Some(path) = file.path()
                        {
                            row.set_text(&path.to_string_lossy());
                        }
                    });
                });
                row.add_suffix(&button);
            }
            "path" | "out" => {
                let button = gtk::Button::from_icon_name("document-save-symbolic");
                button.set_tooltip_text(Some("Elegir fichero"));
                button.add_css_class("flat");
                let parent = picker_parent.clone();
                let row_cb = row.clone();
                button.connect_clicked(move |_| {
                    let row = row_cb.clone();
                    let dialog = gtk::FileDialog::new();
                    let text = row.text();
                    if !text.is_empty() {
                        let initial = gtk::gio::File::for_path(text);
                        dialog.set_initial_file(Some(&initial));
                    }
                    dialog.save(Some(&parent), gtk::gio::Cancellable::NONE, move |file| {
                        if let Ok(file) = file
                            && let Some(path) = file.path()
                        {
                            row.set_text(&path.to_string_lossy());
                        }
                    });
                });
                row.add_suffix(&button);
            }
            _ => {}
        }
    }
    error.add_css_class("error");
    error.set_wrap(true);
    error.set_visible(false);
    let clamp = adw::Clamp::builder().maximum_size(720).build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(12);
    content.set_margin_bottom(18);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.append(&group);
    content.append(&error);
    clamp.set_child(Some(&content));
    let scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&clamp).build();
    toolbar.set_content(Some(&scroll));
    dialog.set_content(Some(&toolbar));

    cancel.connect_clicked({
        let dialog = dialog.clone();
        move |_| {
            dialog.close();
        }
    });
    let submit_dialog = dialog.clone();
    submit.connect_clicked(move |_| {
        let mut values = Map::new();
        for field in &fields {
            match field.value() {
                Ok(Some(value)) => {
                    values.insert(field.name().to_string(), value);
                }
                Ok(None) => {}
                Err(message) => {
                    error.set_label(&message);
                    error.set_visible(true);
                    return;
                }
            }
        }
        error.set_visible(false);
        submit_dialog.close();
        on_submit(Value::Object(values));
    });
    dialog.present();
    if std::env::var("PDFCRAFT_GTK_QA").as_deref() == Ok("form") && definition.command == Some("export.text") {
        adw::glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || {
            let image = crate::capture::path("form");
            if let Err(error) = crate::capture::window_png(&dialog, &image) {
                eprintln!("form capture error: {error}");
                std::process::exit(1);
            }
            submit.emit_clicked();
        });
    }
}

enum Field {
    Text { name: String, kind: FieldKind, required: bool, row: adw::EntryRow },
    Boolean { name: String, required: bool, row: adw::ActionRow, choices: gtk::DropDown },
    Enum { name: String, required: bool, row: adw::ActionRow, choices: gtk::DropDown, values: Vec<Value> },
}

#[derive(Clone, Copy)]
enum FieldKind {
    String,
    Integer,
    Number,
    Json,
}

impl Field {
    fn new(name: &str, schema: &Value, required: bool, default: Option<Value>) -> Self {
        let description = schema.get("description").and_then(Value::as_str).unwrap_or(name);
        if let Some(values) = schema.get("enum").and_then(Value::as_array) {
            let mut labels = Vec::new();
            let mut stored = Vec::new();
            if !required {
                labels.push("(not set)".to_string());
            }
            for value in values {
                labels.push(display_value(value));
                stored.push(value.clone());
            }
            let choices = gtk::DropDown::from_strings(&labels.iter().map(String::as_str).collect::<Vec<_>>());
            if let Some(default) = default
                && let Some(index) = stored.iter().position(|value| *value == default)
            {
                choices.set_selected((index + usize::from(!required)) as u32);
            }
            let row = choice_row(name, description, &choices);
            return Self::Enum { name: name.to_string(), required, row, choices, values: stored };
        }
        if schema.get("type").and_then(Value::as_str) == Some("boolean") {
            let labels = if required { &["No", "Yes"][..] } else { &["(not set)", "No", "Yes"][..] };
            let choices = gtk::DropDown::from_strings(labels);
            if let Some(value) = default.and_then(|value| value.as_bool()) {
                choices.set_selected(if required { u32::from(value) } else { 1 + u32::from(value) });
            }
            let row = choice_row(name, description, &choices);
            return Self::Boolean { name: name.to_string(), required, row, choices };
        }
        let kind = match schema.get("type").and_then(Value::as_str) {
            Some("integer") => FieldKind::Integer,
            Some("number") => FieldKind::Number,
            Some("array" | "object") => FieldKind::Json,
            _ => FieldKind::String,
        };
        let row = adw::EntryRow::new();
        row.set_title(name);
        if matches!(kind, FieldKind::Json) {
            row.set_tooltip_text(Some(description));
        }
        if let Some(default) = default {
            row.set_text(&match kind {
                FieldKind::String => default.as_str().map(ToString::to_string).unwrap_or_else(|| display_value(&default)),
                _ => display_value(&default),
            });
        }
        Self::Text { name: name.to_string(), kind, required, row }
    }

    fn widget(&self) -> &gtk::Widget {
        match self {
            Self::Text { row, .. } => row.upcast_ref(),
            Self::Boolean { row, .. } | Self::Enum { row, .. } => row.upcast_ref(),
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Text { name, .. } | Self::Boolean { name, .. } | Self::Enum { name, .. } => name,
        }
    }

    fn value(&self) -> Result<Option<Value>, String> {
        match self {
            Self::Text { name, kind, required, row } => {
                let text = row.text();
                if text.is_empty() {
                    return if *required { Err(format!("{name} is required")) } else { Ok(None) };
                }
                let value = match kind {
                    FieldKind::String => Value::String(text.to_string()),
                    FieldKind::Integer => text.parse::<i64>().map(Value::from).map_err(|_| format!("{name} must be an integer"))?,
                    FieldKind::Number => text.parse::<f64>().map(Value::from).map_err(|_| format!("{name} must be a number"))?,
                    FieldKind::Json => serde_json::from_str(&text).map_err(|error| format!("{name}: invalid JSON: {error}"))?,
                };
                Ok(Some(value))
            }
            Self::Boolean { required, choices, .. } => {
                let selected = choices.selected() as usize;
                if !*required && selected == 0 {
                    Ok(None)
                } else {
                    let index = if *required { selected } else { selected - 1 };
                    Ok(Some(Value::Bool(index == 1)))
                }
            }
            Self::Enum { required, choices, values, .. } => {
                let selected = choices.selected() as usize;
                if !*required && selected == 0 {
                    return Ok(None);
                }
                let index = if *required { selected } else { selected - 1 };
                Ok(values.get(index).cloned())
            }
        }
    }
}

fn choice_row(name: &str, description: &str, choices: &gtk::DropDown) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(&adw::glib::markup_escape_text(name));
    row.set_subtitle(&adw::glib::markup_escape_text(description));
    row.add_suffix(choices);
    row.set_activatable_widget(Some(choices));
    row
}

fn display_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}
