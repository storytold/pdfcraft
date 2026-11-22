//! The Field Properties dialog: one field's properties as a hosted form dialog whose OK is held until
//! the app has applied the edit, so a refused edit keeps what was typed.

use craft::theme::Srgba;
use craft::ui::contract::Bind;
use craft::ui::dialog::{DialogId, DialogResult, DialogStack, HeldAnswer, HeldOk, StackDialog, dialog_id};
use craft::ui::patterns::form_dialogs::FormDialog;
use craft::ui::runtime::env::Label;
use craft::ui::runtime::overlay::CloseReason;
use craft::ui::runtime::requests::ColorRequest;
use serde_json::{Value, json};

use super::draft::{FieldDraft, FieldTab};
use super::properties::{Shared, body, check};
use super::{Edit, FieldProps};
use crate::PdfCraftApp;
use crate::comments::colours::RecentColours;
use crate::dialogs::host;

/// The index `FormOutput::picked` gives the OK button; Cancel is 1.
const OK_BUTTON: usize = 0;
/// The index `FormOutput::picked` gives the Cancel button.
const CANCEL_BUTTON: usize = 1;

/// Field Properties for one field.
///
/// Its OK files the changed properties; [`finish`] applies them once per pass and answers the held OK, so the
/// dialog closes when the edit was made and stays, with what was typed, when it was refused.
pub struct FieldPropsDialog {
    id: DialogId,
    draft: FieldDraft,
    held: HeldAnswer<FieldProps>,
    recent: RecentColours,
    pending_colour: Option<ColorRequest>,
}

impl FieldPropsDialog {
    fn new(draft: FieldDraft, recent: RecentColours) -> Self {
        FieldPropsDialog { id: dialog_id("field-properties"), draft, held: HeldAnswer::new(), recent, pending_colour: None }
    }

    /// Writes the colour the picker answered for the colour row `key`, and ends any refusal shown for the old one.
    pub(crate) fn set_colour(&mut self, key: &str, colour: [f64; 3]) {
        if let Some(look) = self.draft.look.as_mut() {
            match key {
                "field.props.border" => look.border = Some(colour),
                "field.props.fill" => look.fill = Some(colour),
                "field.props.text" => look.text = colour,
                _ => {}
            }
        }
        self.held.clear_refusal();
    }
}

/// Open Field Properties for a field of the active document, on `tab`. When several fields are selected and
/// this one is among them, the dialog edits them together.
pub(crate) fn open(app: &mut PdfCraftApp, name: &str, widget: usize, tab: FieldTab) {
    let Some((index, id)) = app.active_ids() else { return };
    let names = app.views.get(index).map(|view| view.session.prepare.names()).unwrap_or_default();
    if names.len() > 1 && names.iter().any(|n| n == name) {
        let made = match app.session.get(id) {
            Some(doc) => crate::bulk_fields::SharedFieldPropsDialog::new(doc, names),
            None => Err("The document is no longer open."),
        };
        match made {
            Ok(dialog) => {
                host::open(app, dialog);
            }
            Err(why) => app.notify_tr(why),
        }
        return;
    }
    let Some(doc) = app.session.get(id) else { return };
    let Some(f) = doc.form.iter().find(|f| f.name == name).cloned() else { return };
    let mut d = FieldDraft::new(&f, widget);
    d.tab = tab;
    d.look = doc.field_look(name);
    d.check_style = doc.field_check_style(name);
    let others: Vec<String> = doc.form.iter().filter(|x| x.name != name).map(|x| x.name.clone()).collect();
    d.others = others.clone();
    d.actions = doc.field_actions(name);
    d.chained = doc.field_action_chains(name);
    if let Some(o) = d.original.as_mut() {
        o.look = d.look;
        o.check_style = d.check_style;
        o.others = others;
        o.actions = d.actions.clone();
    }
    let dialog = FieldPropsDialog::new(d, app.recent_colours.clone());
    host::open(app, dialog);
}

/// Runs once per pass for the open Field Properties dialog: opens the colour picker a colour row asked for,
/// applies the properties OK filed, and answers the held OK with what the app did.
pub(crate) fn finish(ctx: &egui::Context, app: &mut PdfCraftApp) {
    let recent = app.recent_colours.clone();
    let Some(dialog) = app.dialogs.get_mut::<FieldPropsDialog>() else { return };
    dialog.recent = recent;
    let request = dialog.pending_colour.take();
    let filed = dialog.held.take();
    let (field, widget, typed) = (dialog.draft.field.clone(), dialog.draft.widget, dialog.draft.name.trim().to_string());
    if let Some(request) = request {
        let key = request.key.clone();
        app.open_colour_picker(crate::colour_picker::PickerHost::App, request, move |app, _: &egui::Context, colour: Srgba| {
            if let Some(dialog) = app.dialogs.get_mut::<FieldPropsDialog>() {
                dialog.set_colour(&key, crate::comments::colours::to_rgb(colour));
            }
        });
    }
    let Some(props) = filed else { return };

    let form = |app: &PdfCraftApp| app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|doc| doc.form.clone());
    // A rename also renames the field in the formulas that read it.
    let before = props.name.as_ref().and_then(|_| form(app));
    // The widget being edited, by object: a radio button that joins a group by its new name
    // is appended after the group's own buttons, so its index changes.
    let object = before.as_ref().and_then(|all| all.iter().find(|f| f.name == field)?.widgets.get(widget).map(|w| w.obj));
    let applied = app.apply_edit(ctx, Edit::SetFieldProps { name: field.clone(), props: Box::new(props) });
    // A refused edit (its reason is in the notice) keeps the dialog open with what was typed, as the
    // several-fields dialog does, so nothing is lost meanwhile.
    let refused = !applied && app.active_ids().is_some();
    if applied && let Some(i) = app.active {
        // Keep the (possibly renamed) field selected.
        // A dotted name is the full name; a plain one renames the field within its parent.
        let prefix = field.rsplit_once('.').filter(|_| !typed.contains('.')).map(|(p, _)| format!("{p}.")).unwrap_or_default();
        let name = format!("{prefix}{typed}");
        let after = form(app);
        let index =
            object.and_then(|obj| after.as_ref()?.iter().find(|f| f.name == name)?.widgets.iter().position(|w| w.obj == obj)).unwrap_or(widget);
        if let (Some(before), Some(after)) = (before, after) {
            let n = pdfcraft_engine::form_calculations_changed(&before, &after).iter().filter(|f| **f != name).count();
            if n > 0 {
                app.notify(crate::i18n::trn(
                    crate::i18n::current(),
                    n as u64,
                    "{n} calculation now uses the new name.",
                    "{n} calculations now use the new name.",
                ));
            }
        }
        if let Some(view) = app.views.get_mut(i) {
            view.session.prepare.selected = Some((name, index));
        }
    }
    let answer = if refused { Err(Label::from(tl!("The field properties could not be changed."))) } else { Ok(()) };
    if let Some(dialog) = app.dialogs.get_mut::<FieldPropsDialog>() {
        dialog.held.answer(answer);
    }
}

impl StackDialog for FieldPropsDialog {
    type Result = FieldProps;

    fn id(&self) -> DialogId {
        self.id
    }

    /// Shows the dialog. OK is enabled while no answer waits for the app; Enter is OK.
    /// A pass that finds the app accepted the held OK closes with its properties.
    fn show(&mut self, ui: &mut egui::Ui, _: &DialogStack) -> Option<DialogResult<FieldProps>> {
        let finish_held = self.held.is_accepted();
        let waiting = self.held.is_waiting();
        let props = self.draft.props();
        // The refusal stays until what it refused changes.
        if self.held.refusal().is_some_and(|(_, refused)| props.as_ref() != Some(refused)) {
            self.held.clear_refusal();
        }
        let refusal = self.held.refusal().map(|(reason, _)| reason.clone());
        let ready = !waiting;
        let title = tl!(self.draft.title());
        let FieldPropsDialog { draft, recent, pending_colour, .. } = self;
        // The footer's Locked box edits a copy: the form is bound over the draft itself.
        let mut locked = draft.locked;
        let out = FormDialog::new(title, Bind::bound(&mut *draft))
            .accept_stays_open(true)
            .footer_extra(|ui| {
                check(ui, tl!("Locked"), &mut locked, true);
            })
            .ok_enabled(ready)
            .show(ui, |ui, form, d| {
                // A measuring run closes nothing: the real run of the same pass makes the close, as in
                // `HostedParamDialog`.
                if finish_held && !craft::ui::runtime::measure::active(ui.ctx()) {
                    form.request_close_as(CloseReason::Activated);
                }
                body(ui, d, Shared { recent: &*recent, pending_colour: &mut *pending_colour, refusal: refusal.as_ref(), waiting })
            });
        draft.locked = locked;
        // OK was pressed, by its button or by Enter: file the answer for the app; the dialog stays open.
        let pressed = out.picked == Some(OK_BUTTON) || out.accepted.is_some();
        if pressed && !finish_held && !waiting {
            if let Some(props) = props {
                self.held.file(props);
            } else {
                return Some(DialogResult::Cancel);
            }
            return None;
        }
        match out.closed {
            Some(CloseReason::Activated) if finish_held && out.picked != Some(CANCEL_BUTTON) => {
                Some(self.held.finish().map_or(DialogResult::Cancel, DialogResult::Ok))
            }
            Some(_) => Some(DialogResult::Cancel),
            None => None,
        }
    }

    fn held_state(&self) -> Option<HeldOk> {
        self.held.state()
    }

    /// Answers OK as its button does: files the changed properties for the app, and answers them once the app
    /// accepted them.
    fn confirm(&mut self, _: &egui::Context) -> Option<FieldProps> {
        if self.held.is_accepted() {
            return self.held.finish();
        }
        if self.held.is_waiting() {
            return None;
        }
        let props = self.draft.props()?;
        self.held.file(props);
        None
    }

    /// The draft's values, keyed as the draft names them.
    fn json_fields(&self) -> Option<Value> {
        let d = &self.draft;
        let look = d.look.as_ref();
        Some(json!({
            "tab": format!("{:?}", d.tab),
            "name": d.name,
            "tooltip": d.tooltip,
            "caption": d.caption,
            "read_only": d.read_only,
            "required": d.required,
            "multiline": d.multiline,
            "limit": d.limit,
            "max_len": d.max_len,
            "options": d.options,
            "flags": d.flags,
            "locked": d.locked,
            "quadding": d.quadding,
            "default": d.default,
            "export_value": d.export_value,
            "font_size": d.font_size,
            "rect": d.position,
            "rotation": d.rotation,
            "font": look.map(|l| l.font.label()),
            "text_colour": look.map(|l| l.text),
            "border": look.and_then(|l| l.border),
            "fill": look.and_then(|l| l.fill),
            "line_width": look.map(|l| l.width),
            "line_style": look.map(|l| l.style.label()),
            "check_style": d.check_style.map(|s| s.label()),
            "format": format!("{:?}", d.format),
            "validate": format!("{:?}", d.validate),
            "calculate": format!("{:?}", d.calculate),
            "script_refused": d.script_refused,
            "actions": d.actions.iter().map(|(t, a)| json!({ "trigger": t.label(), "action": a.describe() })).collect::<Vec<_>>(),
        }))
    }
}
