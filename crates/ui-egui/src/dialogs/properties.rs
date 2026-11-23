//! Document Properties: one hosted tabbed dialog over seven tabs built from the document at each open. The values
//! live in the dialog's fields; `properties_session` binds them to the revision they were opened on.

use craft::behavior::{DisplayOrder, ItemSource, Selection};
use craft::ui::buttons::Button;
use craft::ui::contract::{Bind, Props as _, SelectMode, Show as _, ShowCollection as _, ShowContainer as _, clicked};
use craft::ui::dialog::{DialogId, dialog_id};
use craft::ui::display::Text;
use craft::ui::form::Field;
use craft::ui::layout_components::{Row, Stack};
use craft::ui::list::{List, ListRow};
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, Param, ParamKind, PropertiesDialog, PropertiesTab, PropertyRow};
use craft::ui::runtime::env::Label;
use craft::ui::text::TextField;
use edit_session::EditSession;
use pdfcraft_engine::Document;
use serde_json::Value;

use crate::PdfCraftApp;
use crate::properties_session::{
    Context, LAYOUTS, MAGNIFICATIONS, NAVIGATIONS, PERCENTS, Values, custom_json, custom_pairs, description_key, percent_id,
};

/// The host key of the Properties dialog.
pub(crate) const PROPERTIES_KEY: &str = "file.properties";

/// The most custom properties the Custom tab lists, which the engine's own listing stops at.
const CUSTOM_LIMIT: usize = 256;

/// The tabs of Document Properties, in the order of the tab strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropsTab {
    Description,
    InitialView,
    Security,
    Fonts,
    Custom,
    Advanced,
    SpaceAudit,
}

impl PropsTab {
    /// The position of the tab in the strip.
    pub fn index(self) -> usize {
        match self {
            PropsTab::Description => 0,
            PropsTab::InitialView => 1,
            PropsTab::Security => 2,
            PropsTab::Fonts => 3,
            PropsTab::Custom => 4,
            PropsTab::Advanced => 5,
            PropsTab::SpaceAudit => 6,
        }
    }

    /// The tab at position `index` of the strip.
    pub fn from_index(index: usize) -> Option<Self> {
        Some(match index {
            0 => PropsTab::Description,
            1 => PropsTab::InitialView,
            2 => PropsTab::Security,
            3 => PropsTab::Fonts,
            4 => PropsTab::Custom,
            5 => PropsTab::Advanced,
            6 => PropsTab::SpaceAudit,
            _ => return None,
        })
    }
}

/// The id the host knows the Properties dialog by.
pub(crate) fn properties_id() -> DialogId {
    dialog_id(PROPERTIES_KEY)
}

fn yes(on: bool) -> String {
    tl!(if on { "Yes" } else { "No" }).to_string()
}

/// A read-only row; document values and chrome labels both show "—" when empty.
fn value(label: &str, text: String) -> PropertyRow {
    PropertyRow::value(tl!(label).to_string(), if text.is_empty() { "—".to_owned() } else { text })
}

fn text_row(key: &str, label: &str, lines: Option<usize>) -> PropertyRow {
    let param = Param::new(key, tl!(label).to_string(), ParamKind::text(""));
    PropertyRow::field(match lines {
        Some(lines) => param.multiline(lines),
        None => param,
    })
}

fn bool_row(key: &str, label: &str) -> PropertyRow {
    PropertyRow::field(Param::new(key, tl!(label).to_string(), ParamKind::bool(false)))
}

fn choice_row(key: &str, label: &str, options: Vec<(String, Label)>) -> PropertyRow {
    PropertyRow::field(Param::new(key, tl!(label).to_string(), ParamKind::choice(options, None)))
}

fn radio_row(key: &str, label: &str, options: [(&str, &str); 2]) -> PropertyRow {
    let options = options.map(|(id, name)| (id.to_owned(), Label::from(tl!(name).to_string()))).to_vec();
    PropertyRow::field(Param::new(key, tl!(label).to_string(), ParamKind::radio(options, None)))
}

/// A row of buttons that ask the host for `actions` (an id and its label); `lead` is text before them.
fn action_row(key: &str, label: &str, lead: Option<String>, actions: Vec<(String, String)>) -> PropertyRow {
    PropertyRow::field(Param::custom(key, tl!(label).to_string(), move |ui, _, cx| {
        Row::new()
            .show(ui, |ui, _| {
                if let Some(lead) = &lead {
                    Text::new(lead.clone()).show(ui);
                }
                for (id, text) in &actions {
                    if clicked(&Button::new(text.clone()).show(ui)) {
                        cx.request_action(id.clone());
                    }
                }
            })
            .output
    }))
}

/// The Custom tab's listed properties as a collection source: position is the key.
struct PropRows(usize);

impl DisplayOrder<usize> for PropRows {
    fn len(&self) -> usize {
        self.0
    }

    fn key(&self, at: usize) -> usize {
        at
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        (*key < self.0).then_some(*key)
    }
}

impl ItemSource<usize> for PropRows {}

/// The Custom tab's row: the listed properties, the Name and Value fields and Add, Update and Delete. The rules for
/// the buttons are written here once; `file_listed` is how many properties the file lists, `editable` whether the
/// document accepts changes. `custom.unlisted` is the fact that the typed name is an entry the list leaves out.
fn custom_row(file_listed: usize, editable: bool, listed: Vec<(String, String)>) -> PropertyRow {
    let row = Param::custom("custom", tl!("Custom properties").to_string(), move |ui, fields, _| {
        let mut name = fields.text("custom.name").unwrap_or_default().to_owned();
        let mut text = fields.text("custom.value").unwrap_or_default().to_owned();
        let mut props = custom_pairs(fields).unwrap_or_default();
        let unlisted = fields.bool("custom.unlisted").unwrap_or(false);
        let shown = Stack::new().show(ui, |ui, _| {
            Field::new(tl!("Name:").to_string()).show(ui, |ui, _| {
                TextField::new(tl!("Name:").to_string(), Bind::bound(&mut name)).enabled(editable).show(ui);
            });
            Field::new(tl!("Value:").to_string()).show(ui, |ui, _| {
                TextField::new(tl!("Value:").to_string(), Bind::bound(&mut text)).enabled(editable).show(ui);
            });
            // Untrimmed: a file's `/Project#20` is not `/Project`, and a typed space is refused.
            let value = text.trim().to_owned();
            let at = props.iter().position(|(key, _)| *key == name);
            let refused = !name.is_empty() && pdfcraft_engine::check_custom_key(&name).is_err();
            // A name that isn't UTF-8 is listed with U+FFFD and can't be written back as it is.
            let open = editable && !refused && !name.contains(char::REPLACEMENT_CHARACTER);
            // A new name goes after the file's entries, so Add is off once the draft or the file's own list holds the most.
            let room = props.len() < CUSTOM_LIMIT && file_listed < CUSTOM_LIMIT;
            let add = open && room && at.is_none() && !unlisted && !name.is_empty() && !value.is_empty();
            let update = open && !value.is_empty() && at.and_then(|n| props.get(n)).is_some_and(|(_, v)| *v != value);
            let delete = open && at.is_some();
            Row::new().show(ui, |ui, _| {
                if clicked(&Button::new(tl!("Add").to_string()).enabled(add).show(ui)) {
                    props.push((name.clone(), value.clone()));
                    name.clear();
                    text.clear();
                } else if clicked(&Button::new(tl!("Update").to_string()).enabled(update).show(ui)) {
                    if let Some(entry) = at.and_then(|n| props.get_mut(n)) {
                        entry.1 = value.clone();
                    }
                } else if clicked(&Button::new(tl!("Delete").to_string()).enabled(delete).show(ui)) {
                    if let Some(n) = at.filter(|n| *n < props.len()) {
                        props.remove(n);
                    }
                    name.clear();
                    text.clear();
                }
            });
            if props.is_empty() {
                Text::new(tl!("None").to_string()).show(ui);
            }
            // A pick on a property fills the fields with it; values are document data. The selection is derived
            // from the typed name each pass, so it differs from it only in the pass of the pick.
            let before = props.iter().position(|(key, _)| *key == name);
            let mut selection = before.map_or_else(Selection::default, Selection::single);
            List::new(tl!("Custom properties").to_string()).select(SelectMode::Single).selection(Bind::bound(&mut selection)).show_items(
                ui,
                &PropRows(props.len()),
                &mut |&at, icx| {
                    let (key, entry) = props.get(at).cloned().unwrap_or_default();
                    ListRow::new(key).detail(entry).striped(icx.at % 2 == 1)
                },
            );
            if selection.lead != before
                && let Some((key, entry)) = selection.lead.and_then(|at| props.get(at))
            {
                name = key.clone();
                text = entry.clone();
            }
        });
        if fields.text("custom.name") != Some(name.as_str()) {
            fields.set("custom.name", name);
        }
        if fields.text("custom.value") != Some(text.as_str()) {
            fields.set("custom.value", text);
        }
        if custom_pairs(fields).as_ref() != Some(&props) {
            fields.set("custom", custom_json(&props));
        }
        shown.output
    })
    .companions(["custom.name", "custom.value", "custom.unlisted"])
    .opens_with(custom_json(&listed));
    PropertyRow::field(row)
}

impl PdfCraftApp {
    /// Opens Document Properties on `tab`, or shows `tab` when it is already open.
    pub(crate) fn open_properties(&mut self, ctx: &egui::Context, tab: PropsTab) {
        match self.dialogs.get_mut_by_id::<HostedParamDialog>(properties_id()) {
            Some(dialog) => {
                dialog.set_tab(tab.index());
                if tab == PropsTab::SpaceAudit {
                    self.measure_space_audit();
                }
            }
            None => self.show_properties(ctx, tab),
        }
    }

    /// Builds the dialog and its session from the live document and opens it, replacing an open one.
    fn show_properties(&mut self, ctx: &egui::Context, tab: PropsTab) {
        let Some((_, id)) = self.active_ids() else { return };
        self.session.refresh_fonts(id);
        if tab == PropsTab::SpaceAudit {
            self.measure_space_audit();
        }
        // Starts the measure, so it is built before the document is borrowed.
        let space = crate::optimize_ui::space_audit_param(self);
        let Some(doc) = self.session.get(id) else { return };
        let (values, context, tabs) = (Values::read(doc), Context::read(doc), self.properties_tabs(doc, space));
        let fields = values.to_fields();
        self.props_session = Some(EditSession::new(values, context));
        let dialog = HostedParamDialog::new(ctx, PROPERTIES_KEY, fields, move |fields| {
            PropertiesDialog::new(PROPERTIES_KEY, tl!("Document Properties").to_string(), tabs.clone(), Bind::bound(fields)).into_param_dialog()
        })
        .tab(tab.index())
        .held_ok();
        // PLAN: assumed `dialogs::host::open(app: &mut PdfCraftApp, dialog: impl StackDialog) -> DialogId` (pdf-hub-w2)
        crate::dialogs::host::open(self, dialog);
    }

    /// Rebuilds the open dialog on the tab it shows, from the live document.
    pub(crate) fn rebuild_properties(&mut self, ctx: &egui::Context, tab: PropsTab) {
        self.props_session = None;
        self.show_properties(ctx, tab);
    }

    /// Closes the dialog without a result for the app.
    pub(crate) fn close_properties(&mut self, ctx: &egui::Context) {
        self.dialogs.cancel_by_id(ctx, properties_id());
    }

    /// The open dialog's values and the tab it shows.
    pub(crate) fn properties_fields(&self) -> Option<(DialogFields, PropsTab)> {
        let dialog = self.dialogs.get_by_id::<HostedParamDialog>(properties_id())?;
        Some((dialog.fields().clone(), PropsTab::from_index(dialog.shown_tab())?))
    }

    /// One pass of the dialog: answers its actions and OK, and keeps the session bound to the live document.
    pub(crate) fn properties_pass(&mut self, ctx: &egui::Context) {
        let id = properties_id();
        // Only a cancel is left here: OK is held and answered below.
        let _ = self.dialogs.take_by_id::<HostedParamDialog>(id);
        let Some(dialog) = self.dialogs.get_mut_by_id::<HostedParamDialog>(id) else {
            self.discard_properties();
            return;
        };
        let (ok, action) = (dialog.take_ok().is_some(), dialog.take_action());
        if let Some(action) = action {
            // The action runs its own dialog or opens another document, so Properties closes first.
            self.close_properties(ctx);
            self.discard_properties();
            if let Some(n) = action.strip_prefix("revision:").and_then(|n| n.parse().ok()) {
                self.open_revision(n);
            } else if matches!(action.as_str(), "protect.password" | "protect.unlock" | "protect.remove") {
                self.execute(ctx, &action);
            }
            return;
        }
        if ok {
            let answer = if self.finish_properties(ctx) {
                Ok(())
            } else {
                Err(Label::from(tl!("The document refused these properties. Correct them and try again.").to_string()))
            };
            if let Some(dialog) = self.dialogs.get_mut_by_id::<HostedParamDialog>(id) {
                dialog.answer_ok(answer);
            }
            return;
        }
        // The Fonts tab shows the fonts as they are after any content edit.
        if let (Some((_, id)), Some((_, PropsTab::Fonts))) = (self.active_ids(), self.properties_fields()) {
            self.session.refresh_fonts(id);
        }
        // The Space Audit tab measures the live document when opened and again after an edit.
        if let (Some((_, id)), Some((_, PropsTab::SpaceAudit))) = (self.active_ids(), self.properties_fields())
            && let Some(doc) = self.session.get(id)
        {
            let key = (id, doc.edit_generation());
            if self.space_audit_of != Some(key) && !self.space_audit_jobs.contains(&key) {
                self.measure_space_audit();
            }
        }
        if self.prepare_properties(ctx) {
            self.sync_custom_unlisted(ctx);
        }
    }

    /// Records in `custom.unlisted` whether the typed Custom name is an entry of the file's information the list
    /// leaves out (not text, only padding, or past its 256), which Add must not overwrite.
    fn sync_custom_unlisted(&mut self, ctx: &egui::Context) {
        let Some((fields, _)) = self.properties_fields() else { return };
        let name = fields.text("custom.name").unwrap_or_default();
        let unlisted = !name.is_empty()
            && self
                .active_ids()
                .and_then(|(_, id)| self.session.get(id))
                .is_some_and(|doc| doc.has_info(name) && !doc.custom_info().iter().any(|(key, _)| key == name));
        if fields.bool("custom.unlisted") != Some(unlisted) {
            let _ = self.dialogs.set_json_field(ctx, properties_id(), "custom.unlisted", Value::Bool(unlisted));
        }
    }

    /// The seven tabs, in strip order, from `doc`.
    fn properties_tabs(&self, doc: &Document, space: Param) -> Vec<PropertiesTab> {
        let editable = doc.allows_modification();
        let tab = |title: &str, rows| PropertiesTab::new(tl!(title).to_string(), rows);
        let own = |tab: PropertiesTab| if editable { tab } else { tab.read_only(None) };
        vec![
            own(tab("Description", self.description_rows(doc))),
            own(tab("Initial View", initial_view_rows(doc))),
            tab("Security", security_rows(doc)),
            tab("Fonts", font_rows(doc)),
            tab("Custom", vec![custom_row(doc.custom_info().len(), editable, doc.custom_info())]),
            tab("Advanced", self.advanced_rows(doc, editable)),
            tab("Space Audit", vec![PropertyRow::field(space)]),
        ]
    }

    fn description_rows(&self, doc: &Document) -> Vec<PropertyRow> {
        let i = &doc.info;
        let mut rows = vec![value("File", doc.name.clone())];
        for key in crate::properties_session::INFO_KEYS {
            let lines = matches!(key, "Subject" | "Keywords").then_some(1);
            rows.push(text_row(&description_key(key), key, lines));
        }
        rows.push(value("Application", i.creator.clone().unwrap_or_default()));
        rows.push(value("PDF producer", i.producer.clone().unwrap_or_default()));
        rows
    }

    fn advanced_rows(&self, doc: &Document, editable: bool) -> Vec<PropertyRow> {
        let i = &doc.info;
        let mut rows = vec![
            value("PDF version", i.pdf_version.clone()),
            value("Location", doc.path.clone().unwrap_or_default()),
            value("File size", format!("{} ({})", crate::panels::human_size(i.file_size), crate::panels::byte_count(i.file_size))),
        ];
        if let Some(page) = i.pages.first() {
            rows.push(value("Page size", self.units.size(f64::from(page.width), f64::from(page.height))));
        }
        rows.extend([
            value("Number of pages", i.pages.len().to_string()),
            value("Tagged PDF", yes(i.tagged)),
            value("Form fields", i.fields.len().to_string()),
            value("Comments", i.annotations.len().to_string()),
            value("Layers", i.layers.len().to_string()),
            value("Attachments", i.attachments.len().to_string()),
            value("JavaScript", yes(i.has_javascript)),
        ]);
        // Each incremental update is a revision; earlier ones open as their own document.
        let ends = doc.revision_ends();
        if ends.len() > 1 {
            let buttons = (1..ends.len())
                .rev()
                .take(12)
                .map(|n| (format!("revision:{n}"), crate::i18n::fmt(tl!("View revision {n}"), &[("n", &n.to_string())]).to_string()))
                .collect();
            rows.push(action_row("revisions", "Revisions", Some(ends.len().to_string()), buttons));
        }
        let reading = |row: PropertyRow| match row {
            PropertyRow::Field(param) if !editable => PropertyRow::field(param.disabled()),
            row => row,
        };
        rows.push(reading(radio_row("binding", "Binding", [("left", "Left Edge"), ("right", "Right Edge")])));
        rows.push(reading(PropertyRow::field(
            Param::new("language", tl!("Language").to_string(), ParamKind::text("")).hint(tl!("e.g. en-US").to_string()),
        )));
        // What was repaired while reading a damaged file (fidelity: never silent).
        let repairs = doc.repair_log();
        rows.push(value("Repairs", if repairs.is_empty() { tl!("None").to_string() } else { repairs.len().to_string() }));
        if !repairs.is_empty() {
            rows.push(PropertyRow::value(tl!("Repair log").to_string(), repairs.join("\n")));
        }
        rows
    }
}

fn initial_view_rows(doc: &Document) -> Vec<PropertyRow> {
    let pages = doc.info.pages.len();
    let named = |table: &[(&str, &str)]| table.iter().map(|(id, name)| ((*id).to_owned(), Label::from(tl!(name).to_string()))).collect::<Vec<_>>();
    let navigation = named(&NAVIGATIONS.map(|(id, _, name)| (id, name)));
    let layout = named(&LAYOUTS.map(|(id, _, name)| (id, name)));
    let mut magnification = named(&MAGNIFICATIONS.map(|(id, _, name)| (id, name)));
    // 100% is saved as Actual Size is (/XYZ zoom 1), so it reads back as Actual Size.
    let current = doc.initial_view().magnification;
    let mut percents = PERCENTS.to_vec();
    if let pdfcraft_engine::Magnification::Percent(p) = current
        && !percents.contains(&p)
    {
        percents.push(p);
    }
    magnification.extend(percents.into_iter().map(|p| (percent_id(p), Label::from(format!("{p:.0}%")))));
    vec![
        PropertyRow::heading(tl!("Layout and Magnification").to_string()),
        choice_row("navigation", "Navigation tab", navigation),
        choice_row("layout", "Page layout", layout),
        choice_row("magnification", "Magnification", magnification),
        // The engine refuses a page past the end when OK is pressed, and the dialog says so.
        PropertyRow::field(
            Param::new("page", tl!("Open to page").to_string(), ParamKind::int(Some(1), None, 1))
                .hint(crate::i18n::fmt(tl!("of {pages}"), &[("pages", &pages.to_string())]).to_string()),
        ),
        PropertyRow::heading(tl!("Window Options").to_string()),
        bool_row("fit_window", "Resize window to initial page"),
        bool_row("center_window", "Center window on screen"),
        bool_row("full_screen", "Open in Full Screen mode"),
        radio_row("display_title", "Show:", [("file", "File Name"), ("title", "Document Title")]),
        PropertyRow::heading(tl!("User Interface Options").to_string()),
        bool_row("hide_menubar", "Hide menu bar"),
        bool_row("hide_toolbar", "Hide toolbars"),
        bool_row("hide_window_ui", "Hide window controls"),
    ]
}

fn security_rows(doc: &Document) -> Vec<PropertyRow> {
    let button = |id: &str, text: &str| (id.to_owned(), tl!(text).to_string());
    let Some(sec) = doc.security_summary() else {
        let mut rows =
            vec![value("Security method", tl!("No security").to_string()), value("Restrictions", tl!("None — everything is allowed").to_string())];
        if doc.allows_security_change() {
            rows.push(action_row("security.actions", "Security", None, vec![button("protect.password", "Protect using password…")]));
        }
        return rows;
    };
    let opened = if sec.pending {
        tl!("— (protection is applied when you save)")
    } else if sec.owner {
        tl!("Owner password (no restrictions apply)")
    } else {
        tl!("User password")
    };
    let mut rows = vec![
        value("Security method", tl!("Password security").to_string()),
        value("Encryption", sec.method.clone()),
        value("Opened with", opened.to_string()),
    ];
    // A restricted document asks for its permissions password first.
    if doc.allows_security_change() || doc.can_unlock() {
        let mut actions = Vec::new();
        if doc.can_unlock() {
            actions.push(button("protect.unlock", "Unlock…"));
        }
        actions.push(button("protect.password", "Change settings…"));
        actions.push(button("protect.remove", "Remove security"));
        rows.push(action_row("security.actions", "Security", None, actions));
    }
    let p = sec.permissions;
    let allowed = |on: bool| tl!(if on { "Allowed" } else { "Not allowed" }).to_string();
    let printing = if !p.print() {
        tl!("Not allowed")
    } else if p.print_high_quality() {
        tl!("High resolution")
    } else {
        tl!("Low resolution")
    };
    rows.extend([
        value("Printing", printing.to_string()),
        value("Changing the document", allowed(p.modify())),
        value("Document assembly", allowed(p.assemble())),
        value("Content copying", allowed(p.copy())),
        value("Content copying for accessibility", allowed(p.extract_for_accessibility())),
        value("Commenting", allowed(p.annotate())),
        value("Filling of form fields", allowed(p.fill_forms())),
    ]);
    rows
}

fn font_rows(doc: &Document) -> Vec<PropertyRow> {
    let fonts = &doc.info.fonts;
    if fonts.is_empty() {
        return vec![value("Fonts", tl!("No fonts are referenced by the pages.").to_string())];
    }
    fonts
        .iter()
        .map(|f| {
            let mut detail = f.kind.clone();
            if let Some(e) = &f.encoding {
                detail.push_str(&format!(" · {e}"));
            }
            detail.push_str(if f.subset {
                " · Embedded subset"
            } else if f.embedded {
                " · Embedded"
            } else {
                " · Not embedded (substituted)"
            });
            PropertyRow::value(f.name.clone(), detail)
        })
        .collect()
}
