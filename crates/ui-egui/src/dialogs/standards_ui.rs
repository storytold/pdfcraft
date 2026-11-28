//! Standards ▸ PDF/A: what the document declares, verifying it against PDF/A-2b, 3b, 4, 4e or 4f,
//! and fixing what can be fixed for PDF/A-2b and 3b (Save as PDF/A).

use craft::ui::buttons::Button;
use craft::ui::contract::{Bind, Entry, Show as _, ShowCollection as _, ShowContainer as _, WordSetCx as _};
use craft::ui::dialog::{DialogId, DialogResult, DialogStack, StackDialog, dialog_id};
use craft::ui::display::{Shade, Text};
use craft::ui::form::Field;
use craft::ui::list::{Finding, Findings, FindingsList, FindingsSummary, FindingsWords, Severity};
use craft::ui::patterns::form_dialogs::FormDialog;
use craft::ui::pickers::Select;
use craft::ui::sections::{DialogRole, FooterButton};
use egui::Ui;
use pdfcraft_engine::pdfa::{Issue, Level};

use crate::PdfCraftApp;

/// The dialog's remembered choice: the level.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfaState {
    pub level: Level,
    /// The document `level` was chosen for ([`PdfCraftApp::pdfa_choose_level`]).
    pub chosen_for: Option<pdfcraft_engine::DocId>,
}

impl Default for PdfaState {
    fn default() -> Self {
        PdfaState { level: Level::A2b, chosen_for: None }
    }
}

/// The levels the dialog verifies against. Save as PDF/A makes only the PDF/A-2b and 3b ones.
const LEVELS: [Level; 5] = [Level::A2b, Level::A3b, Level::A4, Level::A4e, Level::A4f];

/// Whether Save as PDF/A makes files of `level`: PDF/A-2b and 3b, not the PDF/A-4 levels.
fn convertible(level: Level) -> bool {
    level.part() != 4
}

/// The level as the value the form binds, which a form needs a default for.
#[derive(Clone, Debug, PartialEq)]
struct LevelForm {
    level: Level,
}

impl Default for LevelForm {
    fn default() -> Self {
        LevelForm { level: PdfaState::default().level }
    }
}

/// What a footer or body button asked for; `pdfa_pass` runs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Verify,
    Convert,
    Remove,
}

/// What the dialog shows of the document, which only the app can read.
#[derive(Clone, Debug, Default)]
struct Shown {
    declared: String,
    /// The label of the button that removes the PDF/A identification, when one is declared.
    remove: Option<String>,
    intents: Vec<String>,
    /// The last verification; `None` until Verify runs and after the level changes.
    issues: Option<Vec<Issue>>,
}

/// Standards ▸ PDF/A, owned by the dialog host. Save as PDF/A and Verify keep it open.
pub(crate) struct PdfaDialog {
    id: DialogId,
    form: LevelForm,
    shown: Shown,
    action: Option<Action>,
}

fn removal() -> pdfcraft_engine::Edit {
    pdfcraft_engine::Edit::RemoveStandardId { standard: pdfcraft_engine::Standard::PdfA }
}

impl PdfCraftApp {
    /// What the active document declares: the text, the removal button's label and the output intents.
    fn pdfa_declared(&self) -> Shown {
        let declared = self.active_ids().and_then(|(_, id)| self.session.get(id)).map(|d| d.standards()).unwrap_or_default();
        let mut standards: Vec<String> = declared.pdfa.iter().map(|(p, c)| format!("PDF/A-{p}{}", c.to_lowercase())).collect();
        standards.extend(declared.pdfua.map(|p| format!("PDF/UA-{p}")));
        let text = if standards.is_empty() { tl!("none").to_string() } else { standards.join(", ") };
        // Worded as Edit ▸ Undo will name it.
        let remove = declared.pdfa.is_some().then(|| crate::i18n::edit_label(&removal().label()));
        Shown { declared: text, remove, intents: declared.output_intents.clone(), issues: None }
    }

    /// Chooses the level for the active document once, when the dialog opens or another document
    /// becomes active: the level the document declares when the dialog verifies it, PDF/A-2b
    /// otherwise. A level picked by hand stays while the same document is active. Returns whether
    /// it chose.
    fn pdfa_choose_level(&mut self) -> bool {
        let id = self.active_ids().map(|(_, id)| id);
        if self.pdfa.chosen_for == id {
            return false;
        }
        let declared = id.and_then(|id| self.session.get(id)).and_then(|d| d.standards().pdfa);
        let level = declared.and_then(|(part, conformance)| Level::from_declared(part, &conformance)).filter(|l| LEVELS.contains(l));
        self.pdfa = PdfaState { level: level.unwrap_or(PdfaState::default().level), chosen_for: id };
        true
    }

    /// Opens the PDF/A dialog on the level the active document declares, with nothing verified yet.
    pub(crate) fn open_pdfa(&mut self) {
        // Opening the dialog chooses the level again, from what the document declares.
        self.pdfa.chosen_for = None;
        self.pdfa_choose_level();
        let dialog = PdfaDialog { id: dialog_id("pdfa"), form: LevelForm { level: self.pdfa.level }, shown: self.pdfa_declared(), action: None };
        // PLAN: assumed `fn open<D: StackDialog>(app: &mut PdfCraftApp, dialog: D)` (pdf-hub-w2)
        crate::dialogs::host::open(self, dialog);
    }

    /// What PdfCraft's checks find in the active document at the chosen level.
    fn pdfa_verify(&self) -> Option<Vec<Issue>> {
        let (_, id) = self.active_ids()?;
        let level = self.pdfa.level;
        self.session.get(id).map(|d| d.pdfa_verify(level))
    }

    /// Converts, then what is left to fix; `None` when the conversion did not apply.
    fn pdfa_convert(&mut self, ctx: &egui::Context) -> Option<Vec<Issue>> {
        let level = self.pdfa.level;
        if !convertible(level) || !self.apply_edit(ctx, pdfcraft_engine::Edit::ConvertPdfA { level }) {
            return None;
        }
        let issues = self.pdfa_verify().unwrap_or_default();
        let left = issues.len();
        self.notify(if left == 0 {
            crate::i18n::fmt(
                tl!("PdfCraft's checks now find no problems for {level}. This is not a validator result. Save to keep the changes."),
                &[("level", level.label())],
            )
        } else if left == 1 {
            crate::i18n::fmt(tl!("Fixed what could be fixed; 1 problem remains"), &[])
        } else {
            crate::i18n::fmt(tl!("Fixed what could be fixed; {n} problems remain"), &[("n", &left.to_string())])
        });
        Some(issues)
    }

    /// Runs what the open PDF/A dialog asked for and refreshes what it shows of the document.
    pub(crate) fn pdfa_pass(&mut self, ctx: &egui::Context) {
        // PLAN: assumed `self.dialogs: DialogHost` with `fn get_mut<D: StackDialog>(&mut self) -> Option<&mut D>` (pdf-hub-w1 field, pdf-hub-w2 host)
        let Some((level, action)) = self.dialogs.get_mut::<PdfaDialog>().map(|d| (d.form.level, d.action.take())) else { return };
        let action = if self.pdfa_choose_level() {
            // Another document became active: the dialog takes its level and forgets the last result,
            // and what was asked of the last document is not done to this one.
            let chosen = self.pdfa.level;
            if let Some(d) = self.dialogs.get_mut::<PdfaDialog>() {
                d.form.level = chosen;
                d.shown.issues = None;
            }
            None
        } else {
            self.pdfa.level = level;
            action
        };
        // `Some(x)` replaces the shown issues with `x`.
        let issues = match action {
            Some(Action::Verify) => Some(self.pdfa_verify()),
            Some(Action::Convert) => self.pdfa_convert(ctx).map(Some),
            Some(Action::Remove) => self.apply_edit(ctx, removal()).then_some(None),
            None => None,
        };
        let current = self.pdfa_declared();
        if let Some(d) = self.dialogs.get_mut::<PdfaDialog>() {
            let kept = d.shown.issues.take();
            d.shown = Shown { issues: issues.unwrap_or(kept), ..current };
        }
    }
}

impl StackDialog for PdfaDialog {
    type Result = ();

    fn id(&self) -> DialogId {
        self.id
    }

    fn show(&mut self, ui: &mut Ui, _: &DialogStack) -> Option<DialogResult<()>> {
        let PdfaDialog { form, shown, action, .. } = self;
        let before = form.level;
        let out = FormDialog::new("PDF/A", Bind::bound(&mut *form))
            .ok_label(tl!("Save as PDF/A").to_string())
            .ok_enabled(convertible(before))
            .accept_stays_open(true)
            .cancel_label(tl!("Close").to_string())
            .alt_resets(false)
            .extra_buttons([FooterButton::new(tl!("Verify").to_string(), DialogRole::Other)])
            .show(ui, |ui, _, f| {
                Text::new(crate::i18n::fmt(tl!("Declared conformance: {shown}"), &[("shown", &shown.declared)])).show(ui);
                if let Some(label) = &shown.remove
                    && Button::new(label.clone()).show(ui).clicks > 0
                {
                    *action = Some(Action::Remove);
                }
                if !shown.intents.is_empty() {
                    let list = shown.intents.join(", ");
                    Text::new(crate::i18n::fmt(tl!("Output intent: {list}"), &[("list", &list)])).shade(Shade::Muted).show(ui);
                }
                let levels = LEVELS.map(|l| Entry::new(l, l.label().to_string()));
                let caption = tl!("Conformance level:").to_string();
                Field::new(caption.clone()).show(ui, |ui, _| {
                    if let Some(l) = Select::new(caption.clone(), Bind::given(f.level), &levels).show(ui).value().copied() {
                        f.level = l;
                    }
                });
                match &shown.issues {
                    None => {
                        Text::new(tl!("Verify to see what the document needs.").to_string()).shade(Shade::Muted).show(ui);
                    }
                    Some(v) if v.is_empty() => {
                        Text::new(crate::i18n::fmt(
                            tl!("PdfCraft's checks found no problems for {level}. This is not a validator result."),
                            &[("level", f.level.label())],
                        ))
                        .show(ui);
                    }
                    Some(v) => {
                        let findings: Vec<Finding> = v.iter().enumerate().map(|(n, i)| finding(n as u64, i)).collect();
                        FindingsSummary::new(&findings).show(ui);
                        // The dialog's body scrolls: the list takes its full height and owns no scroll of its own.
                        FindingsList::new(tl!("PDF/A problems").to_string()).natural_height(true).show_items(
                            ui,
                            &Findings::new(&findings),
                            &mut |key, icx| findings[*key as usize].row(&icx.words::<FindingsWords>()),
                        );
                    }
                }
                let can_convert = convertible(f.level);
                if !can_convert {
                    Text::new(tl!("PDF/A-4 levels are verified only: Save as PDF/A makes PDF/A-2b and 3b files.").to_string())
                        .shade(Shade::Muted)
                        .show(ui);
                }
                can_convert
            });
        if form.level != before {
            shown.issues = None;
        }
        match out.picked {
            Some(0) => *action = Some(Action::Convert),
            Some(2) => *action = Some(Action::Verify),
            _ => {}
        }
        if out.accepted.is_some() {
            *action = Some(Action::Convert);
            None
        } else {
            out.closed.map(|_| DialogResult::Cancel)
        }
    }
}

/// An issue as a finding. The verifier hands the wording over whole, with the document's own name apart from it, so
/// the problem reads in this language and the name as it is.
fn finding(id: u64, i: &Issue) -> Finding {
    let page = i.page.map(|p| crate::i18n::fmt(tl!(" (page {p})"), &[("p", &(p + 1).to_string())])).unwrap_or_default();
    let fix = if i.fixable { String::new() } else { tl!("  · not fixable here").to_string() };
    Finding::new(id, Severity::Error, format!("{}{page}", i.said(&crate::i18n::words))).detail(format!("ISO 19005 {}{fix}", i.clause))
}

#[cfg(test)]
mod wording_tests {
    /// The Standards panel reports what PdfCraft's own checks found; only a run of an external
    /// validator can state the stronger thing, and none is recorded here.
    #[test]
    fn no_standards_panel_wording_claims_conformance() {
        let source = include_str!("standards_ui.rs");
        // Built in pieces so that this file does not match itself.
        for phrase in [concat!("conform", "s to"), concat!("now ", "conforms")] {
            assert!(!source.contains(phrase), "a standards message makes a claim only a validator run can make: {phrase}");
        }
    }
}
