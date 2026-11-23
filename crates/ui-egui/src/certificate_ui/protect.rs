use super::{Cards, parse_certificate, read_certificate};
#[cfg(not(target_arch = "wasm32"))]
use crate::Dialog;
use crate::{PdfCraftApp, widgets};
use pdfcraft_engine::{
    Algorithm, DocId, Edit,
    certificate_security::{CertificateProtection, CertificateRecipient},
};

#[derive(Clone, Debug)]
pub struct Recipient {
    pub name: String,
    pub certificate: CertificateRecipient,
}
pub struct ProtectDraft {
    pub doc: Option<DocId>,
    pub recipients: Vec<Recipient>,
    pub file: String,
    pub cards: Cards,
    pub error: Option<String>,
    pub algorithm: Algorithm,
    pub encrypt_metadata: bool,
}
impl Default for ProtectDraft {
    fn default() -> Self {
        Self {
            doc: None,
            recipients: Vec::new(),
            file: String::new(),
            cards: Default::default(),
            error: None,
            algorithm: Algorithm::Aes256,
            encrypt_metadata: true,
        }
    }
}
impl ProtectDraft {
    pub fn add_certificate(&mut self, bytes: Vec<u8>, now: i64) -> Result<(), String> {
        if self.recipients.len() >= 64 {
            return Err("A document supports at most 64 recipients.".into());
        }
        if bytes.len() > 65536 {
            return Err("Recipient certificates must not exceed 64 KiB.".into());
        }
        let cert = parse_certificate(&bytes, now)?;
        for entry in &self.recipients {
            let other = pdfcraft_engine::sign::Certificate::parse(&entry.certificate.certificate).map_err(|e| e.to_string())?;
            if other.issuer.raw == cert.issuer.raw && other.serial == cert.serial {
                return Err("This recipient or its issuer/serial identifier is already listed.".into());
            }
        }
        self.recipients.push(Recipient { name: cert.display_name(), certificate: CertificateRecipient { certificate: bytes, permissions: -1 } });
        self.error = None;
        Ok(())
    }
    pub fn edit(&self) -> Edit {
        Edit::ProtectCertificates(CertificateProtection {
            recipients: self.recipients.iter().map(|r| r.certificate.clone()).collect(),
            algorithm: self.algorithm,
            encrypt_metadata: self.encrypt_metadata,
        })
    }
}

pub(crate) fn protect_body(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> bool {
    if app.certificate_protect.doc != app.active_ids().map(|(_, id)| id) {
        ui.label(tl!("The document changed. Close this dialog and choose the document again."));
        return ui.button(tl!("Close")).clicked();
    }
    let now = app.session.now_secs();
    let d = &mut app.certificate_protect;
    ui.heading(tl!("Encrypt with certificates"));
    ui.label(tl!("Add each recipient's public encryption certificate. No PIN is needed to encrypt."));
    let mut browse = false;
    let l = ui.label(tl!("Certificate file (.der, .cer)"));
    ui.horizontal(|ui| {
        ui.add(widgets::line(&mut d.file).desired_width(220.0)).labelled_by(l.id);
        if ui.button(tl!("Browse…")).clicked() {
            browse = true;
        }
        if ui.button(tl!("Add file")).clicked()
            && let Err(e) = read_certificate(std::path::Path::new(&d.file)).and_then(|bytes| d.add_certificate(bytes, now))
        {
            d.error = Some(e);
        }
    });
    ui.collapsing(tl!("Add from smartcard"), |ui| {
        d.cards.body(ui);
        if ui.add_enabled(d.cards.selected_entry().is_some(), egui::Button::new(tl!("Add selected certificate"))).clicked()
            && let Some(entry) = d.cards.selected_entry()
            && let Err(e) = d.add_certificate(entry.certificate.clone(), now)
        {
            d.error = Some(e);
        }
    });
    ui.separator();
    let mut remove = None;
    egui::ScrollArea::vertical().max_height(230.0).show(ui, |ui| {
        for (i, recipient) in d.recipients.iter_mut().enumerate() {
            ui.push_id(i, |ui| {
                ui.horizontal(|ui| {
                    ui.strong(recipient.name.chars().take(100).collect::<String>());
                    if ui.button(tl!("Remove")).clicked() {
                        remove = Some(i);
                    }
                });
                let bits = &mut recipient.certificate.permissions;
                let mut full = *bits & 2 != 0;
                if ui.add(widgets::checkbox(&mut full, tl!("Full control (including security changes)"))).changed() {
                    if full {
                        *bits |= 2;
                    } else {
                        *bits &= !2;
                    }
                }
                if !full {
                    ui.horizontal_wrapped(|ui| {
                        for (bit, label) in [
                            (3, "Print"),
                            (4, "Modify"),
                            (5, "Copy"),
                            (6, "Comment"),
                            (9, "Fill forms"),
                            (10, "Accessibility"),
                            (11, "Assemble pages"),
                            (12, "High-quality printing"),
                        ] {
                            let mask = 1 << (bit - 1);
                            let mut on = *bits & mask != 0;
                            if ui.add(widgets::checkbox(&mut on, tl!(label))).changed() {
                                if on {
                                    *bits |= mask;
                                } else {
                                    *bits &= !mask;
                                }
                            }
                        }
                    });
                }
                ui.separator();
            });
        }
    });
    if let Some(i) = remove.filter(|i| *i < d.recipients.len()) {
        d.recipients.remove(i);
    }
    ui.horizontal(|ui| {
        ui.label(tl!("Encryption"));
        widgets::radio_value(ui, &mut d.algorithm, Algorithm::Aes256, "AES-256");
        widgets::radio_value(ui, &mut d.algorithm, Algorithm::Aes128, "AES-128");
    });
    ui.add(widgets::checkbox(&mut d.encrypt_metadata, tl!("Encrypt document metadata")));
    let ready = d.recipients.iter().any(|r| r.certificate.permissions & 2 != 0);
    if !ready {
        ui.label(tl!("Add at least one recipient with full control."));
    }
    if let Some(e) = &d.error {
        ui.colored_label(egui::Color32::RED, e);
    }
    ui.label(tl!("Save the document to apply encryption. Changing security rewrites the file."));
    let (mut apply, mut close) = (false, false);
    ui.horizontal(|ui| {
        apply = ui.add_enabled(ready, egui::Button::new(tl!("Apply"))).clicked();
        close = ui.button(tl!("Cancel")).clicked();
    });
    let edit = apply.then(|| d.edit());
    #[cfg(not(target_arch = "wasm32"))]
    if browse {
        let epoch = app.dialog_epoch();
        let doc = app.certificate_protect.doc;
        app.ask_one(crate::pickers::Ask::File(rfd::AsyncFileDialog::new().add_filter("DER certificate", &["der", "cer"])), None, move |app, path| {
            if app.dialog_epoch() == epoch && app.dialog == Some(Dialog::ProtectCertificates) && app.certificate_protect.doc == doc {
                app.certificate_protect.file = path.to_string_lossy().into_owned();
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    let _ = browse;
    if let Some(edit) = edit
        && app.apply_edit(edit)
    {
        close = true;
        app.notify_tr("Certificate protection will be applied when you save");
    }
    close
}
