//! Desktop certificate security. Drafts hold public certificates; PINs live only while opening.
use pdfcraft_engine::sign::Certificate;
use std::sync::mpsc::{Receiver, TryRecvError};

mod open;
mod protect;
pub use open::OpenPrompt;
pub(crate) use open::show_open;
pub(crate) use protect::protect_body;
pub use protect::{ProtectDraft, Recipient};

#[derive(Clone, Debug)]
pub struct CardEntry {
    pub reference: String,
    pub name: String,
    pub token_label: String,
    pub token_serial: String,
    pub certificate: Vec<u8>,
}
#[derive(Default)]
pub struct Cards {
    pub entries: Vec<CardEntry>,
    pub selected: Option<String>,
    pub error: Option<String>,
    receiver: Option<Receiver<Result<Vec<CardEntry>, String>>>,
    #[cfg(target_os = "linux")]
    cancellation: Option<pdfcraft_engine::sign::pkcs11::CancelOnDrop>,
}
impl Cards {
    pub fn selected_entry(&self) -> Option<&CardEntry> {
        self.entries.iter().find(|e| Some(&e.reference) == self.selected.as_ref())
    }
    #[cfg(not(target_arch = "wasm32"))]
    fn refresh(&mut self, ctx: &egui::Context) {
        if self.receiver.is_some() {
            return;
        }
        #[cfg(target_os = "linux")]
        let cancellation = pdfcraft_engine::sign::pkcs11::Cancellation::default();
        #[cfg(target_os = "linux")]
        {
            self.cancellation = Some(cancellation.cancel_on_drop());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        match std::thread::Builder::new().name("pdfcraft-card-list".into()).spawn(move || {
            #[cfg(target_os = "linux")]
            let result = cancellation.run(|| pdfcraft_engine::guard(discover).unwrap_or_else(Err));
            #[cfg(not(target_os = "linux"))]
            let result = pdfcraft_engine::guard(discover).unwrap_or_else(Err);
            let _ = tx.send(result);
            ctx.request_repaint();
        }) {
            Ok(_) => {
                self.receiver = Some(rx);
                self.error = None;
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }
    #[cfg(target_arch = "wasm32")]
    fn refresh(&mut self, _: &egui::Context) {
        self.error = Some("Smartcard opening requires the desktop app.".into());
    }
    pub(crate) fn body(&mut self, ui: &mut egui::Ui) {
        if let Some(rx) = &self.receiver {
            match rx.try_recv() {
                Ok(result) => {
                    self.receiver = None;
                    match result {
                        Ok(entries) => {
                            self.entries = entries;
                            if self.selected_entry().is_none() {
                                self.selected = None;
                            }
                            self.error = None;
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.error = Some("The card discovery worker stopped.".into());
                }
                Err(TryRecvError::Empty) => {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                }
            }
        }
        if ui.add_enabled(self.receiver.is_none(), egui::Button::new(tl!("Refresh cards"))).clicked() {
            self.selected = None;
            self.refresh(ui.ctx());
        }
        if self.receiver.is_some() {
            ui.label(tl!("Looking for smartcards…"));
        }
        egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
            for entry in &self.entries {
                let label =
                    format!("{} · {}", entry.name.chars().take(100).collect::<String>(), entry.token_label.chars().take(40).collect::<String>());
                if ui
                    .add_enabled(self.receiver.is_none(), crate::widgets::radio(self.selected.as_ref() == Some(&entry.reference), &label))
                    .on_hover_text(&entry.reference)
                    .clicked()
                {
                    self.selected = Some(entry.reference.clone());
                }
            }
        });
        if self.entries.is_empty() {
            ui.label(tl!("Connect your smartcard, then choose Refresh cards."));
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::RED, error);
        }
    }
}
#[cfg(target_os = "linux")]
fn discover() -> Result<Vec<CardEntry>, String> {
    pdfcraft_engine::sign::pkcs11::decryption_identities().map_err(|e| e.to_string()).map(|ids| {
        ids.into_iter()
            .map(|id| CardEntry {
                reference: pdfcraft_engine::sign::pkcs11::reference(id.certificate()),
                name: id.certificate().display_name(),
                token_label: id.token_label().into(),
                token_serial: id.token_serial().into(),
                certificate: id.certificate().raw.clone(),
            })
            .collect()
    })
}
#[cfg(all(not(target_os = "linux"), not(target_arch = "wasm32")))]
fn discover() -> Result<Vec<CardEntry>, String> {
    Err("PKCS #11 smartcard opening requires Linux.".into())
}

pub(crate) fn read_certificate(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path).map_err(|e| e.to_string())?.take(65537).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("Recipient certificates must not exceed 64 KiB.".into());
    }
    Ok(bytes)
}
fn parse_certificate(bytes: &[u8], now: i64) -> Result<Certificate, String> {
    let certificate = Certificate::parse(bytes).map_err(|e| e.to_string())?;
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(problem) = pdfcraft_engine::sign::envelope::recipient_problem(&certificate) {
        return Err(problem.into());
    }
    if !certificate.valid_at(pdfcraft_engine::sign::Time::from_unix(now)) {
        return Err("The recipient certificate is not currently valid.".into());
    }
    Ok(certificate)
}
