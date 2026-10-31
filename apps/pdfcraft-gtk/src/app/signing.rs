use std::{cell::RefCell, path::Path, rc::Rc};

use adw::{gtk, prelude::*};
use libadwaita as adw;
use pdfcraft_engine::{SignOptions, SignatureStatus};

use super::App;
use crate::document::DocumentView;

type Shared = Rc<RefCell<pdfcraft_automation::Automation>>;

thread_local! {
    static SIGN_DIALOG: RefCell<Option<adw::Window>> = const { RefCell::new(None) };
}

pub(crate) fn supports_signing(id: &str) -> bool {
    matches!(id, "sign.digital" | "sign.certify" | "sign.certify_invisible" | "sign.validate" | "sign.panel")
}

pub(crate) fn note_dialog(dialog: &adw::Window) {
    SIGN_DIALOG.with(|slot| *slot.borrow_mut() = Some(dialog.clone()));
}

pub(crate) fn take_dialog() -> Option<adw::Window> {
    SIGN_DIALOG.with(|slot| slot.borrow().clone())
}

impl App {
    pub(super) fn supports_signing(id: &str) -> bool {
        supports_signing(id)
    }

    pub(super) fn run_signing(self: &Rc<Self>, id: &str) {
        match id {
            "sign.digital" | "sign.certify" => {
                let tool = id.to_string();
                self.tools_panel.set_visible(true);
                self.with_active(move |_, view| view.set_annotation_tool(Some(tool)));
            }
            "sign.certify_invisible" => {
                self.with_active(|_, view| {
                    let page = view.current();
                    view.open_sign_dialog(page, None, Some(2));
                });
            }
            "sign.validate" => {
                self.panels.select_section("Firmas");
                self.with_active(|app, view| {
                    let summary = {
                        let automation = app.automation.borrow();
                        automation
                            .session()
                            .get(view.id)
                            .map(|doc| {
                                let total = doc.signatures.len();
                                let valid = doc.signatures.iter().filter(|s| matches!(s.status, SignatureStatus::Valid)).count();
                                (total, valid)
                            })
                            .unwrap_or((0, 0))
                    };
                    app.toast.add_toast(adw::Toast::new(&format!("Firmas válidas: {} de {}", summary.1, summary.0)));
                });
            }
            "sign.panel" => self.panels.select_section("Firmas"),
            _ => {}
        }
    }
}

pub(crate) struct SignRequest {
    pub page: usize,
    pub rect: Option<[f64; 4]>,
    pub certify: Option<u8>,
    pub id_path: String,
    pub password: String,
    pub reason: String,
    pub location: String,
}

pub(crate) fn request_sign_output(view: &Rc<DocumentView>, request: SignRequest) {
    let base = view.name().trim_end_matches(".pdf").to_string();
    let dialog = gtk::FileDialog::new();
    dialog.set_title("Guardar documento firmado");
    dialog.set_initial_name(Some(&format!("{base}-firmado.pdf")));
    let window = view.root.upcast_ref::<gtk::Widget>().root().and_downcast::<gtk::Window>();
    let view_cb = view.clone();
    dialog.save(window.as_ref(), adw::gio::Cancellable::NONE, move |result| {
        if let Ok(file) = result
            && let Some(path) = file.path()
            && let Err(error) = perform_sign(&view_cb.automation, &view_cb, &request, &path)
        {
            show_error(&view_cb, &error);
        }
    });
}

pub(super) fn perform_sign(automation: &Shared, view: &Rc<DocumentView>, request: &SignRequest, out: &Path) -> Result<(), String> {
    let SignRequest { page, rect, certify, id_path, password, reason, location } = request;
    let bytes = std::fs::read(id_path).map_err(|error| format!("No se pudo leer el certificado: {error}"))?;
    let digital_id = pdfcraft_engine::sign::pkcs12::open(&bytes, password).map_err(|error| format!("Certificado: {error}"))?;
    let opts = SignOptions {
        page: *page,
        rect: *rect,
        certify: *certify,
        reason: (!reason.is_empty()).then(|| reason.to_string()),
        location: (!location.is_empty()).then(|| location.to_string()),
        ..SignOptions::default()
    };
    let signed = {
        let automation = automation.borrow();
        automation.session().sign(view.id, &digital_id, opts).map_err(|error| format!("Firma: {error}"))?
    };
    let tmp = out.with_extension("part");
    std::fs::write(&tmp, &*signed).map_err(|error| format!("Escritura: {error}"))?;
    std::fs::rename(&tmp, out).map_err(|error| format!("Escritura: {error}"))?;
    {
        let mut automation = automation.borrow_mut();
        automation.session_mut().set_trusted_certificates(vec![digital_id.certificate.clone()]);
        automation
            .session_mut()
            .mark_signed(view.id, signed, Some(out.to_string_lossy().into_owned()))
            .map_err(|error| format!("Registro de firma: {error}"))?;
    }
    view.refresh(automation);
    super::native::save_sign_id(id_path);
    Ok(())
}

fn show_error(view: &Rc<DocumentView>, message: &str) {
    let Some(window) = view.root.upcast_ref::<gtk::Widget>().root().and_downcast::<gtk::Window>() else {
        return;
    };
    let alert = adw::AlertDialog::new(Some("No se pudo firmar"), Some(message));
    alert.add_response("ok", "Aceptar");
    alert.present(Some(&window));
}
