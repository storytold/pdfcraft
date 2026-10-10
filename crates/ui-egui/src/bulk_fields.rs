//! Shared Field Properties for Prepare a form. Each property is opt-in, and appearance
//! changes are merged per field so a mixed selection keeps everything the user did not edit.

use pdfcraft_engine::{BorderStyle, CheckStyle, DocId, Edit, FieldFont, FieldLookPatch, FieldProps, FormFieldKind, field_flags};

use crate::{prepare, theme, widgets};

/// One property shown by the dialog. `apply` is explicit: merely opening the dialog, or
/// visiting a tab, must never turn mixed values into the first field's value.
#[derive(Clone, Debug)]
pub struct Change<T> {
    pub value: T,
    pub mixed: bool,
    pub apply: bool,
}

impl<T: Clone + PartialEq> Change<T> {
    fn from_values(values: impl IntoIterator<Item = T>) -> Option<Self> {
        let mut values = values.into_iter();
        let value = values.next()?;
        let mixed = values.any(|other| other != value);
        Some(Self { value, mixed, apply: false })
    }

    fn changed(&self, current: &T) -> Option<T> {
        (self.apply && current != &self.value).then(|| self.value.clone())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    General,
    Appearance,
    Options,
}

/// A dialog is bound to its source document, not whichever tab happens to be active later.
#[derive(Clone, Debug)]
pub struct Draft {
    pub doc: DocId,
    pub names: Vec<String>,
    pub tab: Tab,
    pub tooltip: Change<String>,
    pub read_only: Change<bool>,
    pub required: Change<bool>,
    pub locked: Change<bool>,
    pub font_size: Change<f64>,
    pub font: Change<FieldFont>,
    /// The original resource can be a custom font outside the three built-in choices.
    pub font_label: String,
    pub text: Change<[f64; 3]>,
    pub border: Change<Option<[f64; 3]>>,
    pub fill: Change<Option<[f64; 3]>>,
    pub width: Change<f64>,
    pub style: Change<BorderStyle>,
    pub align: Change<i64>,
    pub multiline: Change<bool>,
    /// Zero means no limit, as in the headless form tool.
    pub max_length: Change<usize>,
    pub check_style: Change<CheckStyle>,
    /// A flag, its label, whether the UI wording is inverted, and its proposed value.
    pub flags: Vec<(u32, &'static str, bool, Change<bool>)>,
    pub error: Option<String>,
    text_fields: bool,
    choice_fields: bool,
    check_fields: bool,
    text_appearance: bool,
}

impl Draft {
    /// The draft for editing `names` together, or why they can't be (a message for `tl!`).
    pub fn new(doc: &pdfcraft_engine::Document, names: Vec<String>) -> Result<Self, &'static str> {
        const MISSING: &str = "The selected fields could not be found. Select them again.";
        let fields = names.iter().map(|name| doc.form.iter().find(|f| &f.name == name)).collect::<Option<Vec<_>>>().ok_or(MISSING)?;
        let looks = names
            .iter()
            .map(|name| doc.field_look(name))
            .collect::<Option<Vec<_>>>()
            .ok_or("A selected field has no widget on any page, so the fields can't be edited together.")?;
        if fields.len() < 2 {
            return Err(MISSING);
        }
        Self::build(doc, names, &fields, &looks).ok_or(MISSING)
    }

    fn build(
        doc: &pdfcraft_engine::Document,
        names: Vec<String>,
        fields: &[&pdfcraft_engine::FormField],
        looks: &[pdfcraft_engine::FieldLook],
    ) -> Option<Self> {
        let text_fields = fields.iter().all(|f| f.kind == FormFieldKind::Text);
        let choice_fields = fields.iter().all(|f| matches!(f.kind, FormFieldKind::Combo | FormFieldKind::List));
        let check_fields = fields.iter().all(|f| matches!(f.kind, FormFieldKind::CheckBox | FormFieldKind::Radio));
        let text_appearance =
            fields.iter().all(|f| matches!(f.kind, FormFieldKind::Text | FormFieldKind::Combo | FormFieldKind::List | FormFieldKind::PushButton));
        let mut flag_defs = Vec::new();
        if text_fields {
            flag_defs.extend([
                (field_flags::DO_NOT_SCROLL, "Scroll long text", true),
                (field_flags::DO_NOT_SPELL_CHECK, "Check spelling", true),
                (field_flags::PASSWORD, "Password", false),
                (field_flags::COMB, "Comb of characters", false),
                (field_flags::FILE_SELECT, "Field is used for file selection", false),
            ]);
        } else if choice_fields {
            flag_defs
                .extend([(field_flags::SORT, "Sort items", false), (field_flags::COMMIT_ON_SEL_CHANGE, "Commit selected value immediately", false)]);
            if fields.iter().all(|f| f.kind == FormFieldKind::Combo) {
                flag_defs.push((field_flags::EDIT, "Allow user to enter custom text", false));
            } else if fields.iter().all(|f| f.kind == FormFieldKind::List) {
                flag_defs.push((field_flags::MULTI_SELECT, "Multiple selection", false));
            }
        }
        let flags = flag_defs
            .into_iter()
            .map(|(bit, label, inverted)| Change::from_values(fields.iter().map(|f| f.has(bit) != inverted)).map(|c| (bit, label, inverted, c)))
            .collect::<Option<Vec<_>>>()?;
        let mut font = Change::from_values(looks.iter().map(|l| l.font))?;
        let resource = prepare::da_font(&fields.first()?.da);
        font.mixed |= fields.iter().any(|f| prepare::da_font(&f.da) != resource);
        let font_label = if font.mixed {
            "Mixed".to_string()
        } else if ["Helv", "Helvetica", "TiRo", "Times-Roman", "Cour", "Courier"].contains(&resource) {
            font.value.label().to_string()
        } else {
            "Custom font".to_string()
        };
        Some(Self {
            doc: doc.id,
            names,
            tab: Tab::General,
            tooltip: Change::from_values(fields.iter().map(|f| f.tooltip.clone().unwrap_or_default()))?,
            read_only: Change::from_values(fields.iter().map(|f| f.read_only()))?,
            required: Change::from_values(fields.iter().map(|f| f.has(field_flags::REQUIRED)))?,
            locked: Change::from_values(fields.iter().map(|f| f.locked()))?,
            font_size: Change::from_values(fields.iter().map(|f| prepare::da_size(&f.da)))?,
            font,
            font_label,
            text: Change::from_values(looks.iter().map(|l| l.text))?,
            border: Change::from_values(looks.iter().map(|l| l.border))?,
            fill: Change::from_values(looks.iter().map(|l| l.fill))?,
            width: Change::from_values(looks.iter().map(|l| l.width))?,
            style: Change::from_values(looks.iter().map(|l| l.style))?,
            align: Change::from_values(fields.iter().map(|f| f.quadding))?,
            multiline: Change::from_values(fields.iter().map(|f| f.has(field_flags::MULTILINE)))?,
            max_length: Change::from_values(fields.iter().map(|f| f.max_len.unwrap_or(0)))?,
            check_style: Change::from_values(fields.iter().map(|f| doc.field_check_style(&f.name).unwrap_or(CheckStyle::Check)))?,
            flags,
            error: None,
            text_fields,
            choice_fields,
            check_fields,
            text_appearance,
        })
    }

    /// Resolve only the chosen changes against the current document. This also preserves
    /// unrelated appearance changes made since the dialog opened (e.g. through UI control).
    pub fn edits(&self, doc: &pdfcraft_engine::Document) -> Result<Vec<Edit>, String> {
        if doc.id != self.doc {
            return Err("The document changed while you were editing field properties.".into());
        }
        let mut edits = Vec::new();
        for name in &self.names {
            let f = doc
                .form
                .iter()
                .find(|f| &f.name == name)
                .ok_or_else(|| crate::i18n::fmt(crate::i18n::t("The field {name} no longer exists."), &[("name", name)]))?;
            let patch = FieldLookPatch {
                border: self.border.apply.then_some(self.border.value),
                fill: self.fill.apply.then_some(self.fill.value),
                width: self.width.apply.then_some(self.width.value),
                style: self.style.apply.then_some(self.style.value),
                text: (self.text_appearance && self.text.apply).then_some(self.text.value),
                font: (self.text_appearance && self.font.apply).then_some(self.font.value),
            };
            let props = FieldProps {
                tooltip: self.tooltip.changed(&f.tooltip.clone().unwrap_or_default()),
                read_only: self.read_only.changed(&f.read_only()),
                required: self.required.changed(&f.has(field_flags::REQUIRED)),
                locked: self.locked.changed(&f.locked()),
                appearance: (patch != FieldLookPatch::default()).then_some(patch),
                font_size: (self.text_appearance && self.font_size.apply).then_some(self.font_size.value),
                multiline: self.text_fields.then(|| self.multiline.changed(&f.has(field_flags::MULTILINE))).flatten(),
                max_len: self.text_fields.then(|| self.max_length.changed(&f.max_len.unwrap_or(0))).flatten().map(|v| (v > 0).then_some(v)),
                quadding: (self.text_fields || self.choice_fields).then(|| self.align.changed(&f.quadding)).flatten(),
                check_style: self.check_fields.then(|| self.check_style.changed(&doc.field_check_style(name).unwrap_or(CheckStyle::Check))).flatten(),
                flags: self
                    .flags
                    .iter()
                    .filter_map(|(bit, _, inverted, c)| c.changed(&(f.has(*bit) != *inverted)).map(|v| (*bit, v != *inverted)))
                    .collect(),
                ..Default::default()
            };
            if props != FieldProps::default() {
                edits.push(Edit::SetFieldProps { name: name.clone(), props: Box::new(props) });
            }
        }
        Ok(edits)
    }
}

fn row<T>(ui: &mut egui::Ui, label: &str, c: &mut Change<T>, edit: impl FnOnce(&mut egui::Ui, &mut T)) {
    ui.checkbox(&mut c.apply, tl!(label));
    // Colour swatches contain their own grid; give every property's editor a separate
    // namespace so border and fill controls do not share widget identities.
    ui.push_id(label, |ui| ui.add_enabled_ui(c.apply, |ui| edit(ui, &mut c.value)));
    ui.label(if c.mixed { tl!("Mixed") } else { "" });
    ui.end_row();
}

impl crate::PdfCraftApp {
    pub(crate) fn apply_bulk_field_props(&mut self) -> bool {
        let result = self.bulk_field_props.as_ref().ok_or("The field properties dialog is no longer open.".to_string()).and_then(|d| {
            if self.active_ids().map(|(_, id)| id) != Some(d.doc) {
                return Err("The document changed while you were editing field properties.".into());
            }
            self.session.get(d.doc).ok_or("The document is no longer open.".to_string()).and_then(|doc| d.edits(doc))
        });
        let error = match result {
            Ok(edits) if edits.is_empty() => return true,
            Ok(edits) => {
                if self.apply_edit(Edit::Batch { label: "Change field properties".into(), edits }) {
                    return true;
                }
                self.toast.as_ref().map(|(message, _)| message.clone()).unwrap_or_else(|| "The field properties could not be changed.".into())
            }
            Err(error) => error,
        };
        if let Some(draft) = self.bulk_field_props.as_mut() {
            draft.error = Some(error);
        }
        false
    }
}

fn bool_row(ui: &mut egui::Ui, label: &str, c: &mut Change<bool>) {
    row(ui, label, c, |ui, value| {
        egui::ComboBox::from_id_salt(label).selected_text(tl!(if *value { "Yes" } else { "No" })).show_ui(ui, |ui| {
            ui.selectable_value(value, true, tl!("Yes"));
            ui.selectable_value(value, false, tl!("No"));
        });
    });
}

fn color(ui: &mut egui::Ui, value: &mut Option<[f64; 3]>) {
    ui.horizontal(|ui| {
        let mut none = value.is_none();
        if ui.checkbox(&mut none, tl!("No color")).changed() {
            *value = if none { None } else { Some([0.0; 3]) };
        }
        if let Some(c) = crate::comments::swatch_grid(ui, *value) {
            *value = Some(c);
        }
    });
}

/// Check a property to apply its value to every selected field. No unchecked value is used.
pub(crate) fn body(ui: &mut egui::Ui, d: &mut Draft, t: &theme::Tokens) -> (bool, bool) {
    ui.set_width(600.0);
    ui.label(egui::RichText::new(tl!("Shared Field Properties")).font(theme::semibold(18.0)));
    ui.label(crate::i18n::fmt(tl!("{n} fields selected"), &[("n", &d.names.len().to_string())]));
    ui.label(
        egui::RichText::new(tl!("Only checked properties will change. Mixed means the selected fields have different values."))
            .small()
            .color(t.text_muted),
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        for (tab, label) in [(Tab::General, "General"), (Tab::Appearance, "Appearance"), (Tab::Options, "Options")] {
            if tab == Tab::Options && !(d.text_fields || d.choice_fields || d.check_fields) {
                continue;
            }
            if widgets::mode_tab(ui, tl!(label), d.tab == tab).clicked() {
                d.tab = tab;
            }
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().max_height(430.0).auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("bulk-field-properties").num_columns(3).spacing([16.0, 10.0]).show(ui, |ui| match d.tab {
            Tab::General => {
                row(ui, "Tooltip:", &mut d.tooltip, |ui, v| {
                    ui.add(egui::TextEdit::singleline(v).desired_width(280.0));
                });
                bool_row(ui, "Read Only", &mut d.read_only);
                bool_row(ui, "Required", &mut d.required);
                bool_row(ui, "Locked", &mut d.locked);
            }
            Tab::Appearance => {
                if d.text_appearance {
                    row(ui, "Font Size:", &mut d.font_size, |ui, v| {
                        ui.add(egui::DragValue::new(v).range(0.0..=100.0).speed(0.25).suffix(" pt"));
                    });
                    let font_label = if d.font.apply { d.font.value.label().to_string() } else { d.font_label.clone() };
                    row(ui, "Font:", &mut d.font, |ui, v| {
                        egui::ComboBox::from_id_salt("bulk-font").selected_text(tl!(&font_label)).show_ui(ui, |ui| {
                            for font in FieldFont::ALL {
                                ui.selectable_value(v, font, font.label());
                            }
                        });
                    });
                    row(ui, "Text Color:", &mut d.text, |ui, v| {
                        if let Some(c) = crate::comments::swatch_grid(ui, Some(*v)) {
                            *v = c;
                        }
                    });
                }
                row(ui, "Border Color:", &mut d.border, color);
                row(ui, "Fill Color:", &mut d.fill, color);
                row(ui, "Line Thickness:", &mut d.width, |ui, v| {
                    ui.add(egui::DragValue::new(v).range(0.0..=12.0).speed(0.25).suffix(" pt"));
                });
                row(ui, "Line Style:", &mut d.style, |ui, v| {
                    egui::ComboBox::from_id_salt("bulk-style").selected_text(tl!(v.label())).show_ui(ui, |ui| {
                        for style in BorderStyle::ALL {
                            ui.selectable_value(v, style, tl!(style.label()));
                        }
                    });
                });
            }
            Tab::Options => {
                if d.text_fields || d.choice_fields {
                    row(ui, "Alignment:", &mut d.align, |ui, v| {
                        let label = match *v {
                            1 => "Center",
                            2 => "Right",
                            _ => "Left",
                        };
                        egui::ComboBox::from_id_salt("bulk-align").selected_text(tl!(label)).show_ui(ui, |ui| {
                            for (value, label) in [(0, "Left"), (1, "Center"), (2, "Right")] {
                                ui.selectable_value(v, value, tl!(label));
                            }
                        });
                    });
                }
                if d.text_fields {
                    bool_row(ui, "Multi-line", &mut d.multiline);
                    row(ui, "Character limit:", &mut d.max_length, |ui, v| {
                        ui.add(egui::DragValue::new(v).range(0..=1_000_000));
                    });
                }
                if d.check_fields {
                    row(ui, "Button Style:", &mut d.check_style, |ui, v| {
                        egui::ComboBox::from_id_salt("bulk-check-style").selected_text(tl!(v.label())).show_ui(ui, |ui| {
                            for style in CheckStyle::ALL {
                                ui.selectable_value(v, style, tl!(style.label()));
                            }
                        });
                    });
                }
                for (_, label, _, change) in &mut d.flags {
                    bool_row(ui, label, change);
                }
            }
        });
        ui.add_space(8.0);
        if d.tab == Tab::Appearance && d.text_appearance {
            ui.label(egui::RichText::new(tl!("Font size 0 uses automatic sizing.")).small().color(t.text_muted));
        } else if d.tab == Tab::Options && d.text_fields {
            ui.label(egui::RichText::new(tl!("Character limit 0 means no limit. A comb of characters needs a limit.")).small().color(t.text_muted));
        } else if d.tab == Tab::General {
            ui.label(egui::RichText::new(tl!("To change locked fields, also set Locked to No.")).small().color(t.text_muted));
        }
    });
    if let Some(error) = &d.error {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(tl!(error)).color(t.text));
    }
    ui.separator();
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        apply = widgets::pill_button(ui, tl!("OK"), true).clicked();
        cancel = widgets::pill_button(ui, tl!("Cancel"), false).clicked();
    });
    (apply, cancel)
}
