use craft::ui::choice::{Radio, RadioGroup};
use craft::ui::contract::{Bind, Entry, Props as _, Show as _, ShowContainer as _};
use craft::ui::dialog::{DialogId, DialogResult, DialogStack, StackDialog, dialog_id};
use craft::ui::patterns::form_dialogs::{FormButtons, FormDialog};
use craft::ui::pickers::Select;
use pdfcraft_engine::{PurviewAccount, PurviewDocument, PurviewLabel, PurviewStatus};

use crate::PdfCraftApp;

#[derive(Clone)]
struct PurviewView {
    status: PurviewStatus,
    account: Option<PurviewAccount>,
    accounts: Vec<PurviewAccount>,
    document: Option<PurviewDocument>,
    has_document: bool,
    pending_name: Option<String>,
    labels: Vec<PurviewLabel>,
    labels_error: Option<String>,
    labels_loading: bool,
    account_change_reason: Option<String>,
    error: Option<String>,
}

impl PurviewView {
    fn read(app: &PdfCraftApp) -> Self {
        let pending_name = app.purview_signin_prompt.as_ref().map(|prompt| prompt.name.clone());
        let document = if pending_name.is_none() { app.active_ids().and_then(|(_, id)| app.session.get(id)) } else { None };
        Self {
            status: app.purview_status(),
            account: app.session.purview_account(),
            accounts: app.session.purview_accounts(),
            document: document.and_then(|doc| doc.purview().cloned()),
            has_document: document.is_some(),
            pending_name,
            labels: app.purview_labels.clone(),
            labels_error: app.purview_labels_error.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            labels_loading: app.purview_labels_job.is_some(),
            #[cfg(target_arch = "wasm32")]
            labels_loading: false,
            account_change_reason: app.purview_account_change_reason().map(str::to_owned),
            error: app.purview_signin_prompt.as_ref().and_then(|prompt| prompt.error.clone()),
        }
    }
}

enum Action {
    SignIn,
    SignOut,
    Switch(String),
    Refresh,
    RetryOpen,
    Apply(String, String),
    Remove(String),
}

pub(crate) struct PurviewDialog {
    id: DialogId,
    view: PurviewView,
    label_id: String,
    justification: String,
    action: Option<Action>,
}

impl PurviewDialog {
    fn new(view: PurviewView, label_id: String, justification: String) -> Self {
        Self { id: dialog_id("purview"), view, label_id, justification, action: None }
    }
}

fn account_section(ui: &mut egui::Ui, view: &PurviewView, action: &mut Option<Action>) {
    if let Some(account) = &view.account {
        ui.heading(if account.display_name.trim().is_empty() { "Microsoft account" } else { account.display_name.trim() });
        let address = if account.email.trim().is_empty() { account.principal_name.trim() } else { account.email.trim() };
        if !address.is_empty() {
            ui.label(address);
        }
        if view.accounts.len() > 1 && view.account_change_reason.is_none() {
            let entries: Vec<_> = view.accounts.iter().map(|account| Entry::new(account.id.clone(), account.label())).collect();
            if let Some(active) = Select::new(tl!("Microsoft account"), Bind::given(account.id.clone()), &entries).show(ui).value().cloned()
                && active != account.id
            {
                *action = Some(Action::Switch(active));
            }
        }
    }
    if let Some(reason) = &view.account_change_reason {
        ui.label(tl!(reason));
    } else {
        ui.horizontal(|ui| {
            if ui.button(tl!("Add account")).clicked() {
                *action = Some(Action::SignIn);
            }
            if ui.button(tl!("Sign out")).clicked() {
                *action = Some(Action::SignOut);
            }
        });
    }
}

fn document_section(ui: &mut egui::Ui, document: &PurviewDocument) {
    ui.separator();
    ui.heading(tl!(if document.protected {
        "Protected"
    } else if document.label.is_some() {
        "Sensitivity label"
    } else {
        "Unlabeled"
    }));
    if let Some(label) = &document.label {
        ui.label(&label.name);
        let detail = if label.description.is_empty() { &label.tooltip } else { &label.description };
        if !detail.is_empty() {
            ui.label(detail);
        }
        if label.sensitivity != 0 {
            ui.label(format!("{}: {}", tl!("Sensitivity"), label.sensitivity));
        }
        if label.protection {
            ui.label(tl!("Protection policy: enabled"));
        }
    } else {
        ui.label(tl!("No sensitivity label is attached to this document"));
    }
    if let Some(owner) = document.owner.as_ref().filter(|owner| !owner.trim().is_empty()) {
        ui.horizontal(|ui| {
            ui.label(tl!("Owner"));
            ui.label(owner);
        });
    }
    if let Some(content_id) = document.content_id.as_ref().filter(|id| !id.trim().is_empty()) {
        ui.horizontal(|ui| {
            ui.label(tl!("Policy"));
            ui.label(content_id);
        });
    }
    if document.labeling_required {
        ui.label(tl!("A sensitivity label is required before saving"));
    }
    if document.default_label_id.as_ref().is_some_and(|id| document.label.as_ref().is_none_or(|label| &label.id != id)) {
        ui.label(tl!("A default label is configured but is not applied automatically."));
    }
    if document.downgrade_justification_required {
        ui.label(tl!("Downgrade justification is required"));
    }
    if document.protected {
        ui.heading(tl!("Document capabilities"));
        for (name, allowed) in [
            ("View", document.rights.can_view()),
            ("Edit", document.rights.can_edit()),
            ("Save", document.rights.can_save()),
            ("Print", document.rights.can_print()),
            ("Copy / extract", document.rights.can_copy()),
            ("Comment", document.rights.can_comment()),
            ("Export", document.rights.can_export()),
            ("Change security", document.rights.can_change_security()),
        ] {
            ui.label(format!("{}: {}", tl!(name), tl!(if allowed { "On" } else { "Off" })));
        }
        if !document.rights.can_export() {
            ui.label(tl!("Export permission is required for unprotected files and page or object copies."));
        }
        ui.label(tl!("Changing labels or protection on protected documents is not available yet."));
        ui.label(tl!("Digital signing is not available for protected documents yet."));
    }
}

fn labels_section(ui: &mut egui::Ui, view: &PurviewView, selected: &mut String, justification: &mut String, action: &mut Option<Action>) {
    ui.separator();
    ui.heading(tl!("Sensitivity labels"));
    if view.labels_loading {
        ui.label(tl!("Loading sensitivity labels…"));
    } else if let Some(error) = &view.labels_error {
        ui.colored_label(egui::Color32::RED, error);
    } else if view.labels.is_empty() {
        ui.label(tl!("No sensitivity labels are available to this account or tenant policy."));
    } else {
        RadioGroup::new(tl!("Sensitivity labels"), Bind::bound(&mut *selected)).show(ui, |ui, _| {
            for label in view.labels.iter().filter(|label| label.active) {
                let detail = if label.description.is_empty() { &label.tooltip } else { &label.description };
                let policy = tl!(if label.protection { "Protection policy" } else { "Label only" });
                let text = if detail.is_empty() { format!("{} - {policy}", label.name) } else { format!("{} - {policy} - {detail}", label.name) };
                Radio::new(label.id.clone()).label(text).show(ui);
            }
        });
    }
    if ui.add_enabled(!view.labels_loading, egui::Button::new(tl!("Refresh labels"))).clicked() {
        *action = Some(Action::Refresh);
    }
    let protected = view.document.as_ref().is_some_and(|document| document.protected);
    if view.has_document && !protected {
        let label = ui.label(tl!("Justification (required for policy downgrades or removals)"));
        ui.text_edit_multiline(justification).labelled_by(label.id);
        ui.horizontal(|ui| {
            if !selected.is_empty() && ui.button(tl!("Apply label")).clicked() {
                *action = Some(Action::Apply(selected.clone(), justification.clone()));
            }
            if view.document.as_ref().is_some_and(|doc| doc.label.is_some()) && ui.button(tl!("Remove label")).clicked() {
                *action = Some(Action::Remove(justification.clone()));
            }
        });
    }
}

fn dialog_body(ui: &mut egui::Ui, view: &PurviewView, selected: &mut String, justification: &mut String, action: &mut Option<Action>) {
    match view.status {
        PurviewStatus::SignedOut => {
            if ui.button(tl!("Sign in")).clicked() {
                *action = Some(Action::SignIn);
            }
        }
        PurviewStatus::SigningIn => {
            ui.label(tl!("Opening Microsoft sign-in"));
            ui.label(tl!("Microsoft sign-in and policy loading continue if you close this window."));
        }
        PurviewStatus::NotConfigured => {
            ui.strong(tl!("Storytold's approved Microsoft Entra application client ID is missing"));
            ui.label(tl!("This POC intentionally does not reuse Azure CLI credentials or a sample Microsoft client ID."));
        }
        PurviewStatus::SdkUnavailable => {
            ui.label(tl!("Protection SDK unavailable"));
        }
        PurviewStatus::Ready => {
            account_section(ui, view, action);
            if view.document.as_ref().is_some_and(|document| document.protected)
                && view.pending_name.is_none()
                && ui.button(tl!("Renew Microsoft sign-in")).clicked()
            {
                *action = Some(Action::SignIn);
            }
            if let Some(name) = &view.pending_name {
                ui.label(tl!("Waiting to open"));
                ui.label(name);
                if ui.button(tl!("Retry opening")).clicked() {
                    *action = Some(Action::RetryOpen);
                }
            }
            if let Some(document) = &view.document {
                document_section(ui, document);
            }
            if view.pending_name.is_none() {
                labels_section(ui, view, selected, justification, action);
            }
        }
    }
    if let Some(error) = &view.error {
        ui.colored_label(egui::Color32::RED, error);
    }
}

impl StackDialog for PurviewDialog {
    type Result = ();

    fn id(&self) -> DialogId {
        self.id
    }

    fn show(&mut self, ui: &mut egui::Ui, _: &DialogStack) -> Option<DialogResult<()>> {
        let Self { view, label_id, justification, action, .. } = self;
        let title = if view.status == PurviewStatus::Ready {
            if view.pending_name.is_some() { "Open protected document" } else { "Sensitivity label" }
        } else {
            "Microsoft account"
        };
        let shown = FormDialog::new(tl!(title).to_string(), Bind::bound(label_id)).buttons(FormButtons::Close).enter_confirms(false).show(
            ui,
            |ui, _, selected| {
                dialog_body(ui, view, selected, justification, action);
                true
            },
        );
        shown.closed.map(|_| DialogResult::Cancel)
    }
}

impl PdfCraftApp {
    pub(crate) fn open_purview(&mut self) {
        if self.dialogs.is_open::<PurviewDialog>() {
            return;
        }
        self.purview_label_id.clear();
        self.purview_justification.clear();
        let view = PurviewView::read(self);
        let dialog = PurviewDialog::new(view, self.purview_label_id.clone(), self.purview_justification.clone());
        crate::dialogs::host::open(self, dialog);
    }

    pub(crate) fn purview_pass(&mut self, ctx: &egui::Context) {
        if self.dialogs.take::<PurviewDialog>().is_some() {
            if let Some(prompt) = self.purview_signin_prompt.take()
                && let Some(meta) = prompt.recovered
                && !self.recoverable.iter().any(|item| item.key == meta.key)
            {
                self.recoverable.push(meta);
            }
            return;
        }
        let view = PurviewView::read(self);
        let Some(dialog) = self.dialogs.get_mut::<PurviewDialog>() else { return };
        dialog.view = view;
        self.purview_label_id = dialog.label_id.clone();
        self.purview_justification = dialog.justification.clone();
        let action = dialog.action.take();
        match action {
            Some(Action::SignIn) => self.start_purview_sign_in(ctx),
            Some(Action::SignOut) => match self.session.purview_sign_out() {
                Ok(()) => {
                    self.clear_purview_labels();
                    self.clear_purview_justification();
                    if self.session.purview_status() == PurviewStatus::Ready {
                        self.refresh_purview_labels(ctx);
                    }
                    self.notify_tr("Signed out");
                }
                Err(error) => self.notify_fmt("Sign-out failed: {e}", &[("e", &error.to_string())]),
            },
            Some(Action::Switch(id)) => match self.session.purview_switch_account(&id) {
                Ok(()) => {
                    self.clear_purview_justification();
                    self.refresh_purview_labels(ctx);
                }
                Err(error) => self.notify_fmt("Couldn't switch Microsoft account: {e}", &[("e", &error.to_string())]),
            },
            Some(Action::Refresh) => self.refresh_purview_labels(ctx),
            Some(Action::RetryOpen) => self.retry_purview_signin(ctx),
            Some(Action::Apply(label, justification)) => {
                if let Some((_, id)) = self.active_ids() {
                    let reason = (!justification.trim().is_empty()).then_some(justification.as_str());
                    match self.session.purview_apply_label(id, &label, reason) {
                        Ok(()) => self.notify_tr("Sensitivity label applied"),
                        Err(error) => self.notify_fmt("Couldn't apply the sensitivity label: {e}", &[("e", &error.to_string())]),
                    }
                }
            }
            Some(Action::Remove(justification)) => {
                if let Some((_, id)) = self.active_ids() {
                    let reason = (!justification.trim().is_empty()).then_some(justification.as_str());
                    match self.session.purview_remove_label(id, reason) {
                        Ok(()) => self.notify_tr("Sensitivity label removed"),
                        Err(error) => self.notify_fmt("Couldn't remove the sensitivity label: {e}", &[("e", &error.to_string())]),
                    }
                }
            }
            None => {}
        }
    }

    pub(crate) fn refresh_purview_labels(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        if self.purview_labels_job.is_some() {
            return;
        }
        self.clear_purview_labels();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let job = match self.session.purview_labels_job() {
                Ok(job) => job,
                Err(error) => {
                    self.purview_labels_failed(&error);
                    return;
                }
            };
            let account = self.session.purview_account().map(|account| account.id);
            let (sender, receiver) = std::sync::mpsc::channel();
            let wake = self.wake.clone();
            match std::thread::Builder::new().name("purview-labels".into()).spawn(move || {
                if sender.send((account, job())).is_err() {
                    log::debug!("Microsoft Purview label loading completed after PdfCraft exited");
                }
                wake.wake();
            }) {
                Ok(_) => {
                    self.purview_labels_job = Some(receiver);
                    ctx.request_repaint();
                }
                Err(error) => self.purview_labels_failed(&pdfcraft_engine::PurviewError::Native(format!("couldn't start label loading: {error}"))),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = ctx;
            match self.session.purview_labels() {
                Ok(labels) => self.purview_labels = labels,
                Err(error) => self.purview_labels_failed(&error),
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn poll_purview_labels(&mut self) {
        let Some(receiver) = self.purview_labels_job.as_ref() else { return };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.purview_labels_job = None;
                self.purview_labels_failed(&pdfcraft_engine::PurviewError::Native("the sensitivity-label worker stopped".into()));
                return;
            }
        };
        self.purview_labels_job = None;
        if result.0 != self.session.purview_account().map(|account| account.id) {
            self.purview_labels_failed(&pdfcraft_engine::PurviewError::Native(
                "the Microsoft account changed while loading sensitivity labels".into(),
            ));
            return;
        }
        match result.1 {
            Ok(labels) => self.purview_labels = labels,
            Err(error) => self.purview_labels_failed(&error),
        }
    }

    pub(crate) fn clear_purview_labels(&mut self) {
        self.purview_labels.clear();
        self.purview_labels_error = None;
        self.purview_label_id.clear();
        if let Some(dialog) = self.dialogs.get_mut::<PurviewDialog>() {
            dialog.label_id.clear();
        }
    }

    pub(crate) fn clear_purview_justification(&mut self) {
        self.purview_justification.clear();
        if let Some(dialog) = self.dialogs.get_mut::<PurviewDialog>() {
            dialog.justification.clear();
        }
    }

    pub(crate) fn purview_labels_failed(&mut self, error: &pdfcraft_engine::PurviewError) {
        self.purview_labels_error = Some(error.to_string());
        self.notify_fmt("Couldn't load sensitivity labels: {e}", &[("e", &error.to_string())]);
    }
}
