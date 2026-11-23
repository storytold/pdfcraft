//! The JavaScript console and Document JavaScripts dialogs: one open at a time, held in `PdfCraftApp::js_dialog`.

use craft::behavior::{DisplayOrder, ItemSource, Selection};
use craft::ui::contract::{Bind, CollectionEvent, Scroll, SelectMode, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::dialog::Dialog;
use craft::ui::display::{Shade, Text};
use craft::ui::layout_components::Stack;
use craft::ui::list::{List, ListRow};
use craft::ui::sections::{DialogRole, FooterButton};
use craft::ui::text::{SelectableText, TextField};
use pdfcraft_engine::Edit;

use crate::PdfCraftApp;

/// Document JavaScripts: the script being edited.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DocJsDraft {
    pub name: String,
    pub script: String,
}

/// Which JavaScript dialog is open.
#[derive(Clone, Debug, PartialEq)]
pub enum JsDialog {
    /// The console, over `PdfCraftApp::js_console`.
    Console,
    /// Document JavaScripts with the script being edited.
    DocumentScripts(DocJsDraft),
}

/// Opens the JavaScript console.
pub(crate) fn open_console(app: &mut PdfCraftApp) {
    app.js_dialog = Some(JsDialog::Console);
}

/// Opens Document JavaScripts with an empty draft.
pub(crate) fn open_document_scripts(app: &mut PdfCraftApp) {
    app.js_dialog = Some(JsDialog::DocumentScripts(DocJsDraft::default()));
}

/// Shows the open JavaScript dialog, if any, and carries out the button it was closed or answered with.
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    // Taken out so the body may borrow the app; put back below unless the dialog closed.
    let Some(dialog) = app.js_dialog.take() else { return };
    let mut open = true;
    let dialog = match dialog {
        JsDialog::Console => {
            console(app, ui, &mut open);
            JsDialog::Console
        }
        JsDialog::DocumentScripts(mut draft) => {
            document_scripts(app, ui, &mut open, &mut draft);
            JsDialog::DocumentScripts(draft)
        }
    };
    // A run may have opened a dialog of its own; that one wins.
    if open && app.js_dialog.is_none() {
        app.js_dialog = Some(dialog);
    }
}

#[derive(Clone, Copy)]
enum Action {
    Run,
    Clear,
    Save,
    Delete,
    Close,
}

fn console(app: &mut PdfCraftApp, ui: &mut egui::Ui, open: &mut bool) {
    let actions = [
        (Action::Run, FooterButton::new(tl!("Run"), DialogRole::Accept).stays_open(true)),
        (Action::Clear, FooterButton::new(tl!("Clear"), DialogRole::Other).stays_open(true)),
        (Action::Close, FooterButton::new(tl!("Close"), DialogRole::Cancel)),
    ];
    let buttons: Vec<FooterButton> = actions.iter().map(|(_, b)| b.clone()).collect();
    let javascript = app.session.javascript();
    let mut submitted = false;
    let console = &mut app.js_console;
    let shown = Dialog::new(tl!("JavaScript Console")).opened(Bind::bound(&mut *open)).buttons(buttons).show(ui, |ui, _cx| {
        if !javascript {
            Text::new(tl!("JavaScript is turned off (Preferences ▸ JavaScript).")).shade(Shade::Muted).show(ui);
        }
        Stack::new().scroll(Scroll::vertical().anchored_end()).show(ui, |ui, _| {
            if console.log.is_empty() {
                Text::new(tl!("Output appears here.")).shade(Shade::Muted).show(ui);
            } else {
                SelectableText::new(tl!("Output"), &console.log.join("\n")).multiline().mono().show(ui);
            }
        });
        let input = TextField::new(tl!("JavaScript"), Bind::bound(&mut console.input))
            .lines(4)
            .mono()
            .tab_inserts(true)
            .submits()
            .placeholder(tl!("JavaScript, e.g. getField(\"total\").value"))
            .show(ui);
        submitted = input.submitted;
    });
    let picked = shown.output.picked.and_then(|at| actions.get(at)).map(|(a, _)| *a);
    let ctx = ui.ctx().clone();
    match picked {
        Some(Action::Run) => app.run_console(&ctx),
        Some(Action::Clear) => app.js_console.log.clear(),
        _ if submitted => app.run_console(&ctx),
        _ => {}
    }
}

/// The scripts as the list's item source, keyed by position.
struct ScriptRows(usize);

impl DisplayOrder<usize> for ScriptRows {
    fn len(&self) -> usize {
        self.0
    }

    fn key(&self, at: usize) -> usize {
        at
    }
}

impl ItemSource<usize> for ScriptRows {}

fn document_scripts(app: &mut PdfCraftApp, ui: &mut egui::Ui, open: &mut bool, draft: &mut DocJsDraft) {
    let scripts = app.active_ids().and_then(|(_, id)| app.session.get(id)).map(|d| d.document_scripts()).unwrap_or_default();
    let actions = [
        (Action::Save, FooterButton::new(tl!("Save"), DialogRole::Accept).stays_open(true)),
        (Action::Delete, FooterButton::new(tl!("Delete"), DialogRole::Destructive).stays_open(true)),
        (Action::Close, FooterButton::new(tl!("Close"), DialogRole::Cancel)),
    ];
    let buttons: Vec<FooterButton> = actions.iter().map(|(_, b)| b.clone()).collect();
    let shown = Dialog::new(tl!("Document JavaScripts")).opened(Bind::bound(&mut *open)).buttons(buttons).show(ui, |ui, _cx| {
        TextField::new(tl!("Script Name:"), Bind::bound(&mut draft.name)).show(ui);
        if scripts.is_empty() {
            Text::new(tl!("This document has no document-level scripts.")).shade(Shade::Muted).show(ui);
        } else {
            let before = scripts.iter().position(|(n, _)| *n == draft.name);
            let mut selection = before.map_or_else(Selection::default, Selection::single);
            let source = ScriptRows(scripts.len());
            // A short list that scrolls its own rows, above the script.
            let out = List::new(tl!("Document JavaScripts"))
                .select(SelectMode::Single)
                .selection(Bind::bound(&mut selection))
                .max_rows(5)
                .show_items(ui, &source, &mut |&at, icx| {
                    let name = scripts.get(at).map(|(n, _)| n.clone()).unwrap_or_default();
                    ListRow::new(name).striped(icx.at % 2 == 1)
                });
            // Only the pass's own pick copies; a drained event list is empty on a second pass.
            if out.events.iter().any(|e| !matches!(e, CollectionEvent::Column(_)))
                && selection.lead != before
                && let Some((name, script)) = selection.lead.and_then(|at| scripts.get(at))
            {
                *draft = DocJsDraft { name: name.clone(), script: script.clone() };
            }
        }
        TextField::new(tl!("Script"), Bind::bound(&mut draft.script)).lines(8).mono().tab_inserts(true).show(ui);
    });
    let name = draft.name.trim().to_string();
    let picked = shown.output.picked.and_then(|at| actions.get(at)).map(|(a, _)| *a);
    match picked {
        Some(Action::Save) if !name.is_empty() => {
            app.apply_edit(ui.ctx(), Edit::SetDocumentScript { name, script: Some(draft.script.clone()) });
        }
        Some(Action::Delete) if scripts.iter().any(|(n, _)| *n == name) => {
            app.apply_edit(ui.ctx(), Edit::SetDocumentScript { name, script: None });
            *draft = DocJsDraft::default();
        }
        _ => {}
    }
}
