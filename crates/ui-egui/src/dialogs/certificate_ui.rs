//! Encrypt with Certificate (Acrobat: Protect ▸ Encrypt with certificate): the document is
//! encrypted for the holders of certificates (ISO 32000-2 §7.6.5, `/Adobe.PubSec`), each of whom
//! opens it with their own private key. Recipients come from certificate files (.cer, .crt,
//! .pem, .der) or from the person's own digital ID files; and the prompt that opens such a
//! document with one of those digital IDs.

use craft::theme::variant::Tone;
use craft::ui::buttons::Button;
use craft::ui::choice::{Checkbox, Radio, RadioGroup};
use craft::ui::contract::{Bind, Entry, Props as _, Show as _, ShowContainer as _};
use craft::ui::dialog::{DialogId, DialogResult, DialogStack, StackDialog, dialog_id};
use craft::ui::display::{Shade, Text};
use craft::ui::form::Field;
use craft::ui::layout_components::Row;
use craft::ui::patterns::form_dialogs::FormDialog;
use craft::ui::pickers::Select;
use craft::ui::style::TypeRole;
use craft::ui::text::TextField;
use egui::Ui;
use pdfcraft_engine::sign::{self, Certificate, DigitalId};
use pdfcraft_engine::{Algorithm, CertificateProtection, CertificateRecipient, Changes, Edit, Printing, Protection};

use crate::PdfCraftApp;

fn text(s: &str) -> String {
    tl!(s).to_string()
}

/// One recipient in the dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct Recipient {
    pub name: String,
    pub email: String,
    pub issuer: String,
    pub expires: String,
    /// The certificate, DER.
    pub der: Vec<u8>,
    /// Everything allowed; otherwise the dialog's restrictions apply.
    pub everything: bool,
    /// One of the person's own digital IDs: the saved document opens here with it.
    pub mine: bool,
}

impl Recipient {
    pub fn of(c: &Certificate, mine: bool) -> Self {
        let e = crate::sign_ui::entry_for("", c);
        Recipient { name: e.name, email: e.email, issuer: e.issuer, expires: e.expires, der: c.raw.clone(), everything: mine, mine }
    }
}

/// The dialog's state while it is open.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CertificateDraft {
    pub recipients: Vec<Recipient>,
    /// What recipients without "All permissions" may do, the compatibility and what is encrypted.
    pub protection: Protection,
    /// Adding one of the person's digital ID files: its path and password.
    pub own_id: String,
    pub own_password: String,
    pub error: Option<String>,
    /// The person's digital ID files that can be recipients: `(path, name)`, read when the dialog opens.
    pub own_files: Vec<(String, String)>,
    /// What the dialog asks the app to do on its next pass (a file to choose, a digital ID to open).
    pub want: Option<Want>,
}

/// What the Encrypt with Certificate dialog asks the app for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// Choose certificate files to add as recipients.
    CertificateFile,
    /// Open the chosen digital ID with its password and add it as a recipient.
    AddOwn,
}

impl CertificateDraft {
    /// Add a recipient; one already listed (the same certificate) stays once, keeping the more
    /// generous of the two.
    pub fn add(&mut self, r: Recipient) {
        match self.recipients.iter_mut().find(|x| x.der == r.der) {
            Some(x) => {
                x.mine |= r.mine;
                x.everything |= r.everything;
            }
            None => self.recipients.push(r),
        }
    }

    /// Whether one of the recipients is one of the person's digital IDs.
    pub fn includes_me(&self) -> bool {
        self.recipients.iter().any(|r| r.mine)
    }

    /// Why Apply is disabled, if it is.
    pub fn problem(&self) -> Option<&'static str> {
        self.recipients.is_empty().then_some("Add at least one recipient.")
    }

    /// The `/P` bits a recipient without "All permissions" gets.
    pub fn restricted_bits(&self) -> i32 {
        Protection { permissions_password: Some(String::new()), ..self.protection.clone() }.permission_bits()
    }

    /// The edit Apply makes, with a fresh `seed` (`Session::new_seed`).
    pub fn edit(&self, seed: [u8; 20]) -> Edit {
        let restricted = self.restricted_bits();
        Edit::ProtectWithCertificates(CertificateProtection {
            recipients: self
                .recipients
                .iter()
                .map(|r| CertificateRecipient { certificate: r.der.clone(), permissions: if r.everything { -1 } else { restricted } })
                .collect(),
            algorithm: self.protection.algorithm,
            encrypt_metadata: self.protection.encrypt_metadata,
            seed,
        })
    }
}

/// Why a certificate can't be a recipient: the envelopes are RSA key transport only.
fn unusable(c: &Certificate) -> Option<&'static str> {
    (!matches!(c.public_key, sign::PublicKey::Rsa { .. })).then_some("Only certificates with RSA keys can be recipients.")
}

/// Recipients from a certificate file (DER, or PEM with one or more certificates).
#[cfg(not(target_arch = "wasm32"))]
pub fn recipients_from_file(path: &std::path::Path) -> Result<Vec<Recipient>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let certs = sign::x509::load_certificates(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(problem) = certs.iter().find_map(unusable) {
        return Err(problem.to_string());
    }
    Ok(certs.iter().map(|c| Recipient::of(c, false)).collect())
}

/// A digital ID file opened with its password.
pub fn open_digital_id(path: &str, password: &str) -> Result<DigitalId, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    sign::pkcs12::open(&bytes, password).map_err(|e| match e {
        sign::SignError::WrongPassword => "The password is incorrect.".to_string(),
        e => e.to_string(),
    })
}

/// The person's digital IDs that can open an encrypted document: the files (a Keychain or
/// Windows store key never leaves its store, and decrypting through it is not supported yet).
fn id_files(app: &PdfCraftApp) -> Vec<(String, String)> {
    app.digital_ids
        .iter()
        .filter(|e| e.unusable.is_none() && !e.path.starts_with("keychain:") && !e.path.starts_with("windows:"))
        .map(|e| (e.path.clone(), e.name.clone()))
        .collect()
}

/// What a digital ID path is called in a list: its name if known, else the file's name.
fn id_label(files: &[(String, String)], path: &str) -> String {
    files
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, n)| n.clone())
        .unwrap_or_else(|| std::path::Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned()))
}

/// Encrypt with Certificate, owned by the dialog host.
pub(crate) struct CertificateDialog {
    id: DialogId,
    pub(crate) draft: CertificateDraft,
}

impl StackDialog for CertificateDialog {
    type Result = CertificateDraft;

    fn id(&self) -> DialogId {
        self.id
    }

    fn show(&mut self, ui: &mut Ui, _: &DialogStack) -> Option<DialogResult<CertificateDraft>> {
        let problem = self.draft.problem();
        let mut form = FormDialog::new(text("Encrypt with Certificate"), Bind::bound(&mut self.draft)).ok_label(text("Apply"));
        if let Some(problem) = problem {
            form = form.ok_reason(text(problem));
        }
        let shown = form.show(ui, |ui, _, draft| body(ui, draft));
        match (shown.accepted, shown.closed) {
            (Some(draft), _) => Some(DialogResult::Ok(draft)),
            (None, Some(_)) => Some(DialogResult::Cancel),
            (None, None) => None,
        }
    }
}

/// The dialog's rows; returns whether Apply can be pressed.
fn body(ui: &mut Ui, d: &mut CertificateDraft) -> bool {
    Text::new(text("Only the recipients can open the document, each with the private key of their own certificate.")).show(ui);
    Text::new(text("Recipients")).role(TypeRole::Heading).show(ui);
    if d.recipients.is_empty() {
        Text::new(text("No recipients yet.")).shade(Shade::Muted).show(ui);
    }
    let mut remove = None;
    for (i, r) in d.recipients.iter_mut().enumerate() {
        let name = if r.mine { crate::i18n::fmt(tl!("{name} (you)"), &[("name", &r.name)]) } else { r.name.clone() };
        Text::new(name).show(ui);
        let mut sub: Vec<String> = [r.email.clone(), r.issuer.clone()].into_iter().filter(|s| !s.is_empty()).collect();
        sub.push(crate::i18n::fmt(tl!("Expires {date}"), &[("date", &r.expires)]));
        Text::new(sub.join(" · ")).role(TypeRole::Caption).shade(Shade::Muted).show(ui);
        Row::new().show(ui, |ui, _| {
            Checkbox::new(Bind::bound(&mut r.everything)).label(text("All permissions")).show(ui);
            if Button::new(text("Remove")).show(ui).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        d.recipients.remove(i);
    }
    #[cfg(not(target_arch = "wasm32"))]
    if Button::new(text("Add certificate file…")).show(ui).clicked() {
        d.want = Some(Want::CertificateFile);
    }
    if !d.own_files.is_empty() {
        if d.own_id.is_empty() || !d.own_files.iter().any(|(p, _)| *p == d.own_id) {
            d.own_id = d.own_files[0].0.clone();
        }
        let ids: Vec<Entry<String>> = d.own_files.iter().map(|(p, n)| Entry::new(p.clone(), n.clone())).collect();
        let label = text("Add your digital ID");
        Field::new(label.clone()).show(ui, |ui, _| {
            if let Some(path) = Select::new(label.clone(), Bind::given(d.own_id.clone()), &ids).show(ui).value().cloned() {
                d.own_id = path;
            }
        });
        Row::new().show(ui, |ui, _| {
            let typed = TextField::new(text("Password"), Bind::bound(&mut d.own_password)).masked().submits().show(ui);
            if Button::new(text("Add")).show(ui).clicked() || typed.submitted {
                d.want = Some(Want::AddOwn);
            }
        });
    }
    advanced_rows(ui, &mut d.protection);
    if let Some(e) = &d.error {
        // Known error text is translated; anything else passes through.
        Text::new(tl!(e).to_string()).tone(Tone::Bad).show(ui);
    }
    if !d.recipients.is_empty() && !d.includes_me() {
        Text::new(text("None of the recipients is one of your digital IDs: you won't be able to open the saved document.")).tone(Tone::Warn).show(ui);
    }
    Text::new(text("The restrictions apply to recipients without “All permissions”. It's applied when you save."))
        .role(TypeRole::Caption)
        .shade(Shade::Muted)
        .show(ui);
    d.problem().is_none()
}

/// What recipients without "All permissions" may do, the compatibility and what is encrypted.
fn advanced_rows(ui: &mut Ui, p: &mut Protection) {
    use Algorithm as A;
    let algos =
        [Entry::new(A::Aes256, text("Acrobat X and later (256-bit AES)")), Entry::new(A::Aes128, text("Acrobat 7.0 and later (128-bit AES)"))];
    if !matches!(p.algorithm, A::Aes256 | A::Aes128) {
        p.algorithm = A::Aes256;
    }
    let label = text("Compatibility");
    Field::new(label.clone()).show(ui, |ui, _| {
        if let Some(a) = Select::new(label.clone(), Bind::given(p.algorithm), &algos).show(ui).value().copied() {
            p.algorithm = a;
        }
    });
    let label = text("Encrypt");
    Field::new(label.clone()).show(ui, |ui, _| {
        RadioGroup::new(label.clone(), Bind::bound(&mut p.encrypt_metadata)).show(ui, |ui, _| {
            Radio::new(true).label(text("All document contents")).show(ui);
            Radio::new(false).label(text("All document contents except metadata")).show(ui);
        });
    });
    let printing = [
        Entry::new(Printing::None, text("None")),
        Entry::new(Printing::Low, text("Low Resolution (150 dpi)")),
        Entry::new(Printing::High, text("High Resolution")),
    ];
    let label = text("Printing allowed");
    Field::new(label.clone()).show(ui, |ui, _| {
        if let Some(v) = Select::new(label.clone(), Bind::given(p.printing), &printing).show(ui).value().copied() {
            p.printing = v;
        }
    });
    let changes = [
        Entry::new(Changes::None, text("None")),
        Entry::new(Changes::Pages, text("Inserting, deleting, and rotating pages")),
        Entry::new(Changes::FillSign, text("Filling in form fields and signing existing signature fields")),
        Entry::new(Changes::CommentFillSign, text("Commenting, filling in form fields, and signing existing signature fields")),
        Entry::new(Changes::AnyExceptExtract, text("Any except extracting pages")),
    ];
    let label = text("Changes allowed");
    Field::new(label.clone()).show(ui, |ui, _| {
        if let Some(v) = Select::new(label.clone(), Bind::given(p.changes), &changes).show(ui).value().copied() {
            p.changes = v;
        }
    });
    Checkbox::new(Bind::bound(&mut p.copy)).label(text("Enable copying of text, images, and other content")).show(ui);
    Checkbox::new(Bind::bound(&mut p.accessibility)).label(text("Enable text access for screen reader devices for the visually impaired")).show(ui);
}

/// The digital ID prompt's values while it is shown.
#[derive(Clone, Debug, Default, PartialEq)]
struct IdChoice {
    path: String,
    password: String,
    files: Vec<(String, String)>,
    message: String,
    error: Option<String>,
    browse: bool,
}

impl PdfCraftApp {
    /// Opens Encrypt with Certificate with no recipients.
    pub(crate) fn open_certificate(&mut self) {
        let draft = CertificateDraft { own_files: id_files(self), ..Default::default() };
        crate::dialogs::host::open(self, CertificateDialog { id: dialog_id("protect.certificate"), draft });
    }

    /// Add the draft's chosen digital ID file as a recipient with every permission; its identity
    /// also joins the session, so the saved document opens here.
    pub fn add_own_recipient(&mut self, d: &mut CertificateDraft) -> Result<(), String> {
        let (path, password) = (d.own_id.clone(), d.own_password.clone());
        if path.is_empty() {
            return Err("Choose a digital ID.".into());
        }
        let id = open_digital_id(&path, &password)?;
        if let Some(problem) = unusable(&id.certificate) {
            return Err(problem.to_string());
        }
        let cert = id.certificate.clone();
        self.session.add_identity(id);
        self.add_digital_id(&path, &cert);
        d.add(Recipient::of(&cert, true));
        d.own_password.clear();
        Ok(())
    }

    /// What the open dialog asked for, and its answer: Apply encrypts the document for its
    /// recipients when it is saved.
    pub(crate) fn certificate_pass(&mut self, ctx: &egui::Context) {
        match self.dialogs.get_mut::<CertificateDialog>().and_then(|d| d.draft.want.take()) {
            Some(Want::AddOwn) => {
                if let Some(mut draft) = self.dialogs.get::<CertificateDialog>().map(|d| d.draft.clone()) {
                    draft.error = self.add_own_recipient(&mut draft).err();
                    if let Some(open) = self.dialogs.get_mut::<CertificateDialog>() {
                        open.draft = draft;
                    }
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            Some(Want::CertificateFile) => {
                let request = craft::host::PickRequest::open("protect.certificate.file").filter(tl!("Certificate"), &["cer", "crt", "pem", "der"]);
                // The picker answers on a later pass, into the dialog if it is still open.
                self.pick_file(request, None, |app, _, picked| {
                    let Some(path) = picked.first_path() else { return };
                    let Some(open) = app.dialogs.get_mut::<CertificateDialog>() else { return };
                    match recipients_from_file(&path) {
                        Ok(list) => {
                            open.draft.error = None;
                            for r in list {
                                open.draft.add(r);
                            }
                        }
                        Err(e) => open.draft.error = Some(e),
                    }
                });
            }
            _ => {}
        }
        if let Some(DialogResult::Ok(draft)) = self.dialogs.take::<CertificateDialog>() {
            self.apply_certificate_draft(ctx, &draft);
        }
    }

    /// Apply the dialog: the document is encrypted for its recipients when it is saved. With
    /// no randomness for the file key nothing changes, and the notice says why.
    pub(crate) fn apply_certificate_draft(&mut self, ctx: &egui::Context, draft: &CertificateDraft) -> bool {
        let seed = match self.session.new_seed() {
            Ok(seed) => seed,
            Err(e) => {
                self.notify_fmt("Couldn't encrypt: {e}", &[("e", &e.to_string())]);
                return false;
            }
        };
        if self.apply_edit(ctx, draft.edit(seed)) {
            self.notify_tr("Certificate encryption will be applied when you save");
            true
        } else {
            false
        }
    }

    /// The prompt for a document encrypted with certificates, in place of the password dialog:
    /// which digital ID file opens it, and its password. Called by the password pass every pass
    /// while such a prompt waits.
    pub(crate) fn digital_id_pass(&mut self, ui: &mut Ui) {
        let files = id_files(self);
        let Some(prompt) = self.password_prompt.as_mut() else { return };
        let Some(chosen) = prompt.digital_id.as_mut() else { return };
        if chosen.is_empty()
            && let Some((p, _)) = files.first()
        {
            *chosen = p.clone();
        }
        let message = crate::i18n::fmt(
            tl!("“{name}” is encrypted for specific people. Choose your digital ID and enter its password to open it."),
            &[("name", &prompt.name)],
        );
        let mut choice = IdChoice {
            path: chosen.clone(),
            password: prompt.digital_id_password.clone(),
            files,
            message,
            error: prompt.error.clone(),
            browse: false,
        };
        let mut open = true;
        let ready = !choice.path.is_empty();
        let shown = FormDialog::new(text("Digital ID required"), Bind::bound(&mut choice))
            .opened(Bind::bound(&mut open))
            .ok_label(text("Open"))
            .ok_enabled(ready)
            .show(ui, |ui, _, c| id_body(ui, c));
        let (accepted, closed) = (shown.accepted, shown.closed);
        if let Some(prompt) = self.password_prompt.as_mut() {
            prompt.digital_id = Some(choice.path.clone());
            prompt.digital_id_password = choice.password.clone();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if choice.browse {
            let request = craft::host::PickRequest::open("protect.digital_id").filter(tl!("Digital ID"), &["p12", "pfx"]);
            self.pick_file(request, None, |app, _, picked| {
                if let (Some(path), Some(chosen)) = (picked.first_path(), app.password_prompt.as_mut().and_then(|p| p.digital_id.as_mut())) {
                    *chosen = path.to_string_lossy().into_owned();
                }
            });
        }
        match (accepted, closed) {
            (Some(c), _) if !c.path.is_empty() => self.submit_password(ui.ctx(), Some(c.password)),
            (None, Some(_)) => self.submit_password(ui.ctx(), None),
            _ => {}
        }
    }
}

/// The digital ID prompt's rows; returns whether Open can be pressed.
fn id_body(ui: &mut Ui, c: &mut IdChoice) -> bool {
    Text::new(c.message.clone()).show(ui);
    let label = text("Digital ID");
    let mut ids: Vec<Entry<String>> = c.files.iter().map(|(p, n)| Entry::new(p.clone(), n.clone())).collect();
    if !c.path.is_empty() && !c.files.iter().any(|(p, _)| *p == c.path) {
        ids.push(Entry::new(c.path.clone(), id_label(&c.files, &c.path)));
    }
    Field::new(label.clone()).show(ui, |ui, _| {
        Row::new().show(ui, |ui, _| {
            if !ids.is_empty()
                && let Some(path) = Select::new(label.clone(), Bind::given(c.path.clone()), &ids).show(ui).value().cloned()
            {
                c.path = path;
            }
            #[cfg(not(target_arch = "wasm32"))]
            if Button::new(text("Browse…")).show(ui).clicked() {
                c.browse = true;
            }
        });
    });
    TextField::new(text("Password"), Bind::bound(&mut c.password)).masked().show(ui);
    if let Some(e) = &c.error {
        Text::new(tl!(e).to_string()).tone(Tone::Bad).show(ui);
    }
    !c.path.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recipient(der: &[u8], mine: bool) -> Recipient {
        Recipient {
            name: "Ada".into(),
            email: String::new(),
            issuer: String::new(),
            expires: String::new(),
            der: der.to_vec(),
            everything: mine,
            mine,
        }
    }

    #[test]
    fn drafts_need_a_recipient_and_give_each_their_permissions() {
        let mut d = CertificateDraft::default();
        assert_eq!(d.problem(), Some("Add at least one recipient."));
        d.add(recipient(b"other", false));
        d.add(recipient(b"me", true));
        // The same certificate twice stays once, with the more generous permissions.
        d.add(recipient(b"other", false));
        assert_eq!(d.recipients.len(), 2);
        assert!(d.includes_me() && d.problem().is_none());
        d.protection.printing = Printing::None;
        d.protection.copy = false;
        let Edit::ProtectWithCertificates(p) = d.edit([7; 20]) else { panic!() };
        assert_eq!(p.seed, [7; 20]);
        assert_eq!(p.recipients[0].permissions, d.restricted_bits());
        assert_ne!(p.recipients[0].permissions, -1);
        assert_eq!(p.recipients[0].permissions & (1 << 2), 0, "no printing");
        assert_eq!(p.recipients[1].permissions, -1, "your own ID keeps everything");
    }
}
