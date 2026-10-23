//! Encrypt with Certificate (Acrobat: Protect ▸ Encrypt with certificate): the document is
//! encrypted for the holders of certificates (ISO 32000-2 §7.6.5, `/Adobe.PubSec`), each of whom
//! opens it with their own private key. Recipients come from certificate files (.cer, .crt,
//! .pem, .der) or from the person's own digital ID files; and the prompt that opens such a
//! document with one of those digital IDs.

use egui::{Align, Color32, Layout};
use pdfcraft_engine::sign::{self, Certificate, DigitalId};
use pdfcraft_engine::{Algorithm, CertificateProtection, CertificateRecipient, Changes, Edit, Printing, Protection};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

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

impl PdfCraftApp {
    /// Add one of the person's digital ID files as a recipient with every permission; its
    /// identity also joins the session, so the saved document opens here.
    pub fn add_own_recipient(&mut self) -> Result<(), String> {
        let (path, password) = (self.certificate_draft.own_id.clone(), self.certificate_draft.own_password.clone());
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
        let d = &mut self.certificate_draft;
        d.add(Recipient::of(&cert, true));
        d.own_password.clear();
        Ok(())
    }

    /// Apply the dialog: the document is encrypted for its recipients when it is saved. With
    /// no randomness for the file key nothing changes, and the notice says why.
    pub(crate) fn apply_certificate_draft(&mut self) -> bool {
        let seed = match self.session.new_seed() {
            Ok(seed) => seed,
            Err(e) => {
                self.notify_fmt("Couldn't encrypt: {e}", &[("e", &e.to_string())]);
                return false;
            }
        };
        let edit = self.certificate_draft.edit(seed);
        if self.apply_edit(edit) {
            self.certificate_draft = Default::default();
            self.notify_tr("Certificate encryption will be applied when you save");
            true
        } else {
            false
        }
    }
}

/// Draw the dialog body; returns (apply, cancel).
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let files = id_files(app);
    let red = Color32::from_rgb(0xD1, 0x3B, 0x3B);
    ui.label(egui::RichText::new(tl!("Encrypt with Certificate")).font(theme::semibold(18.0)));
    ui.add_space(4.0);
    ui.separator();
    ui.add_space(6.0);
    ui.add(egui::Label::new(tl!("Only the recipients can open the document, each with the private key of their own certificate.")).wrap());
    ui.add_space(8.0);
    ui.label(egui::RichText::new(tl!("Recipients")).font(theme::semibold(14.0)));
    ui.add_space(4.0);
    let mut remove = None;
    {
        let d = &mut app.certificate_draft;
        if d.recipients.is_empty() {
            ui.label(egui::RichText::new(tl!("No recipients yet.")).color(t.text_muted));
        } else {
            egui::Grid::new("cert-recipients").num_columns(4).spacing([14.0, 6.0]).striped(true).show(ui, |ui| {
                for (i, r) in d.recipients.iter_mut().enumerate() {
                    ui.vertical(|ui| {
                        let name = if r.mine { crate::i18n::fmt(tl!("{name} (you)"), &[("name", &r.name)]) } else { r.name.clone() };
                        ui.label(egui::RichText::new(name).font(theme::medium(13.0)));
                        let sub = [r.email.as_str(), r.issuer.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
                        if !sub.is_empty() {
                            ui.label(egui::RichText::new(sub).color(t.text_muted).font(theme::regular(11.5)));
                        }
                    });
                    ui.label(
                        egui::RichText::new(crate::i18n::fmt(tl!("Expires {date}"), &[("date", &r.expires)]))
                            .color(t.text_muted)
                            .font(theme::regular(11.5)),
                    );
                    ui.add(widgets::checkbox(&mut r.everything, tl!("All permissions")));
                    if ui.button(tl!("Remove")).clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
        }
        if let Some(i) = remove {
            d.recipients.remove(i);
        }
    }
    ui.add_space(6.0);
    let mut browse = false;
    #[cfg(not(target_arch = "wasm32"))]
    if ui.button(tl!("Add certificate file…")).clicked() {
        browse = true;
    }
    let mut add_own = false;
    if !files.is_empty() {
        ui.add_space(6.0);
        let d = &mut app.certificate_draft;
        if d.own_id.is_empty() || !files.iter().any(|(p, _)| *p == d.own_id) {
            d.own_id = files[0].0.clone();
        }
        ui.horizontal(|ui| {
            ui.label(tl!("Add your digital ID"));
            egui::ComboBox::from_id_salt("cert-own-id").width(200.0).selected_text(id_label(&files, &d.own_id)).show_ui(ui, |ui| {
                for (p, n) in &files {
                    ui.selectable_value(&mut d.own_id, p.clone(), n);
                }
            });
            let r = ui.add(widgets::line(&mut d.own_password).password(true).hint_text(tl!("Password")).desired_width(150.0));
            if ui.button(tl!("Add")).clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                add_own = true;
            }
        });
    }
    if add_own {
        app.certificate_draft.error = app.add_own_recipient().err();
    }
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);
    {
        use pdfcraft_engine::Algorithm as A;
        let p = &mut app.certificate_draft.protection;
        egui::Grid::new("cert-permissions").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(tl!("Compatibility"));
            let algos = [(A::Aes256, tl!("Acrobat X and later (256-bit AES)")), (A::Aes128, tl!("Acrobat 7.0 and later (128-bit AES)"))];
            if !algos.iter().any(|(a, _)| *a == p.algorithm) {
                p.algorithm = Algorithm::Aes256;
            }
            let cur = algos.iter().find(|(a, _)| *a == p.algorithm).map_or(algos[0].1, |(_, l)| *l);
            egui::ComboBox::from_id_salt("cert-algo").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                for (a, l) in algos {
                    ui.selectable_value(&mut p.algorithm, a, l);
                }
            });
            ui.end_row();
            ui.label(tl!("Encrypt"));
            ui.vertical(|ui| {
                widgets::radio_value(ui, &mut p.encrypt_metadata, true, tl!("All document contents"));
                widgets::radio_value(ui, &mut p.encrypt_metadata, false, tl!("All document contents except metadata"));
            });
            ui.end_row();
            ui.label(tl!("Printing allowed"));
            let opts = [(Printing::None, tl!("None")), (Printing::Low, tl!("Low Resolution (150 dpi)")), (Printing::High, tl!("High Resolution"))];
            let cur = opts.iter().find(|(o, _)| *o == p.printing).map_or(tl!("High Resolution"), |(_, l)| *l);
            egui::ComboBox::from_id_salt("cert-print").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                for (o, l) in opts {
                    ui.selectable_value(&mut p.printing, o, l);
                }
            });
            ui.end_row();
            ui.label(tl!("Changes allowed"));
            let opts = [
                (Changes::None, tl!("None")),
                (Changes::Pages, tl!("Inserting, deleting, and rotating pages")),
                (Changes::FillSign, tl!("Filling in form fields and signing existing signature fields")),
                (Changes::CommentFillSign, tl!("Commenting, filling in form fields, and signing existing signature fields")),
                (Changes::AnyExceptExtract, tl!("Any except extracting pages")),
            ];
            let cur = opts.iter().find(|(o, _)| *o == p.changes).map_or(tl!("None"), |(_, l)| *l);
            egui::ComboBox::from_id_salt("cert-changes").width(300.0).selected_text(cur).show_ui(ui, |ui| {
                for (o, l) in opts {
                    ui.selectable_value(&mut p.changes, o, l);
                }
            });
            ui.end_row();
            ui.label("");
            ui.vertical(|ui| {
                ui.add(widgets::checkbox(&mut p.copy, tl!("Enable copying of text, images, and other content")));
                ui.add(widgets::checkbox(&mut p.accessibility, tl!("Enable text access for screen reader devices for the visually impaired")));
            });
            ui.end_row();
        });
    }
    ui.add_space(8.0);
    let d = &app.certificate_draft;
    if let Some(e) = &d.error {
        // Known error text is translated; anything else passes through.
        ui.add(egui::Label::new(egui::RichText::new(tl!(e)).color(red)).wrap());
    }
    if !d.recipients.is_empty() && !d.includes_me() {
        ui.add(
            egui::Label::new(
                egui::RichText::new(tl!("None of the recipients is one of your digital IDs: you won't be able to open the saved document."))
                    .color(Color32::from_rgb(0xE8, 0x8A, 0x1A)),
            )
            .wrap(),
        );
    }
    ui.label(
        egui::RichText::new(tl!("The restrictions apply to recipients without “All permissions”. It's applied when you save."))
            .color(t.text_faint)
            .font(theme::regular(11.5)),
    );
    ui.add_space(12.0);
    let (mut apply, mut cancel) = (false, false);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        let ok = d.problem().is_none();
        if ui.add_enabled_ui(ok, |ui| widgets::pill_button(ui, tl!("Apply"), true)).inner.clicked() {
            apply = true;
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            cancel = true;
        }
    });
    // The picker answers on a later frame, into the draft if the same dialog is still open.
    #[cfg(not(target_arch = "wasm32"))]
    if browse {
        let dialog = rfd::AsyncFileDialog::new().add_filter(tl!("Certificate"), &["cer", "crt", "pem", "der"]);
        let epoch = app.dialog_epoch();
        app.ask_one(crate::pickers::Ask::File(dialog), None, move |app, p| {
            if app.dialog_epoch() != epoch {
                return;
            }
            match recipients_from_file(&p) {
                Ok(list) => {
                    let d = &mut app.certificate_draft;
                    d.error = None;
                    for r in list {
                        d.add(r);
                    }
                }
                Err(e) => app.certificate_draft.error = Some(e),
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = browse;
    (apply, cancel)
}

/// What the digital ID prompt was answered with.
enum Answer {
    Submit,
    Cancel,
}

/// The prompt for a document encrypted with certificates: which digital ID file opens it, and
/// its password.
pub(crate) fn open_prompt(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let files = id_files(app);
    let t = Tokens::get(ctx);
    let mut browse = false;
    let Some(prompt) = app.password_prompt.as_mut() else { return };
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
    let mut answer = None;
    let modal = widgets::show_modal(ctx, "certificate-password", |ui| {
        ui.set_width(440.0);
        ui.horizontal(|ui| {
            ui.add(crate::icons::image("file-lock-2", 22.0, t.accent));
            ui.label(egui::RichText::new(tl!("Digital ID required")).font(theme::semibold(17.0)));
        });
        ui.add_space(6.0);
        ui.add(egui::Label::new(message).wrap());
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let shown = if chosen.is_empty() { tl!("Choose a digital ID file").to_string() } else { id_label(&files, chosen) };
            egui::ComboBox::from_id_salt("certificate-password-id").width(300.0).selected_text(shown).show_ui(ui, |ui| {
                for (p, n) in &files {
                    ui.selectable_value(chosen, p.clone(), n);
                }
            });
            #[cfg(not(target_arch = "wasm32"))]
            if ui.button(tl!("Browse…")).clicked() {
                browse = true;
            }
        });
        ui.add_space(6.0);
        let r = ui.add(widgets::line(&mut prompt.input).password(true).hint_text(tl!("Password")).desired_width(f32::INFINITY));
        if (r.has_focus() || r.lost_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            answer = Some(Answer::Submit);
        }
        if !browse {
            r.request_focus();
        }
        if let Some(e) = prompt.error.as_deref() {
            ui.add_space(4.0);
            ui.add(egui::Label::new(egui::RichText::new(tl!(e)).color(Color32::from_rgb(0xD1, 0x3B, 0x3B))).wrap());
        }
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled_ui(!chosen.is_empty(), |ui| widgets::pill_button(ui, tl!("Open"), true)).inner.clicked() {
                answer = Some(Answer::Submit);
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                answer = Some(Answer::Cancel);
            }
        });
    });
    let answer = answer.or(modal.should_close().then_some(Answer::Cancel));
    match answer {
        Some(Answer::Submit) if prompt.digital_id.as_deref().is_some_and(|p| !p.is_empty()) => {
            let pw = prompt.input.clone();
            app.submit_password(Some(pw));
        }
        Some(Answer::Cancel) => app.submit_password(None),
        _ => {}
    }
    #[cfg(not(target_arch = "wasm32"))]
    if browse {
        let dialog = rfd::AsyncFileDialog::new().add_filter(tl!("Digital ID"), &["p12", "pfx"]);
        app.ask_one(crate::pickers::Ask::File(dialog), None, move |app, p| {
            if let Some(chosen) = app.password_prompt.as_mut().and_then(|p| p.digital_id.as_mut()) {
                *chosen = p.to_string_lossy().into_owned();
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = browse;
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
