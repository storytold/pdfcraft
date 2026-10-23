//! Signing in the browser (`sign.web-signing`). A web page can't open a file by its path, so the
//! digital ID file the browser's picker gives is held in memory for the session ([`IdFile`]),
//! listed as `browser:<file name>` and never written to the settings; its password is asked at
//! each signing, as for a file on disk. The signed file goes where the web build's saves go: to
//! the embedding page (`?host=parent`) or a download.
//!
//! ECDSA keys sign in Rust, as on the desktop (RustCrypto, deterministic nonces). RSA keys sign
//! with the browser's Web Crypto (`RSASSA-PKCS1-v1_5`), because the `rsa` crate is only trusted to
//! verify (ADR-0009). Web Crypto answers asynchronously, so the document is signed in two steps
//! ([`pdfcraft_engine::SigningRequest`]): the key is imported as not extractable, and the copies of
//! it PdfCraft held are wiped once it has been handed over. Nothing leaves the browser.

use std::sync::Arc;

use pdfcraft_engine::sign::Certificate;

use crate::PdfCraftApp;

/// How the list names a digital ID file held in memory: `browser:<file name>`.
const PREFIX: &str = "browser:";

/// A digital ID file held in memory instead of by path (what the browser's file picker read).
#[derive(Clone, PartialEq)]
pub struct IdFile {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
}

impl std::fmt::Debug for IdFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IdFile({:?}, {} bytes)", self.name, self.bytes.len())
    }
}

impl IdFile {
    /// Its list path (`DigitalIdEntry::path`).
    pub fn path(&self) -> String {
        format!("{PREFIX}{}", self.name)
    }
}

/// Whether a list path names a digital ID file held in memory.
pub(crate) fn in_memory(path: &str) -> bool {
    path.starts_with(PREFIX)
}

impl PdfCraftApp {
    /// The bytes of the digital ID file listed at `path`: held in memory, else read from disk.
    pub(crate) fn id_file_bytes(&self, path: &str) -> Result<Arc<Vec<u8>>, String> {
        if in_memory(path) {
            return self
                .id_files
                .iter()
                .find(|f| f.path() == path)
                .map(|f| f.bytes.clone())
                .ok_or_else(|| "This digital ID file is no longer open. Add it again with Configure New Digital ID.".to_string());
        }
        std::fs::read(path).map(Arc::new).map_err(|e| format!("{path}: {e}"))
    }

    /// Hold `file` in memory and list it, replacing a file of the same name. Returns its index in
    /// the list.
    pub(crate) fn add_id_file(&mut self, file: IdFile, cert: &Certificate) -> usize {
        let path = file.path();
        self.id_files.retain(|f| f.path() != path);
        self.id_files.push(file);
        self.add_digital_id(&path, cert)
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) use browser::Inbox;

#[cfg(target_arch = "wasm32")]
mod browser {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use pdfcraft_engine::SignatureImage;
    use pdfcraft_engine::sign::{DigestAlg, DigitalId, PublicKey};
    use pdfcraft_engine::signature_image::{MAX_SIGNATURE_IMAGE_BYTES, SignatureImageError};
    use pdfcraft_engine::{DocId, SignOptions, SigningRequest};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::IdFile;
    use crate::PdfCraftApp;
    use crate::sign_ui::SignGraphic;

    /// Web Crypto's name for RSA PKCS #1 v1.5 signatures.
    const RSA: &str = "RSASSA-PKCS1-v1_5";

    /// The largest file taken as a digital ID: a .p12 with a long chain is tens of KB.
    const MAX_ID_FILE_BYTES: usize = 1 << 20;

    /// What the browser's file picker read for the Sign dialogs.
    enum Picked {
        /// Configure ▸ Browse…: a digital ID file.
        IdFile(Result<IdFile, String>),
        /// Sign as ▸ Choose image…: the signature's graphic.
        Image(Result<Vec<u8>, String>),
    }

    /// A signature Web Crypto made (or couldn't), waiting for the next frame.
    struct Answer {
        request: SigningRequest,
        name: String,
        choice: SignGraphic,
        signature: Result<Vec<u8>, String>,
    }

    /// What arrives asynchronously: a picked file (with the dialog epoch it was asked in) and the
    /// signatures Web Crypto made.
    #[derive(Clone, Default)]
    pub(crate) struct Inbox {
        /// A picker is open (one at a time).
        busy: Arc<AtomicBool>,
        picked: Arc<Mutex<Option<(u64, Picked)>>>,
        answers: Arc<Mutex<Vec<Answer>>>,
    }

    /// A JavaScript error's message (a `DOMException` says what failed).
    fn message(e: &JsValue) -> String {
        js_sys::Reflect::get(e, &JsValue::from_str("message"))
            .ok()
            .and_then(|m| m.as_string())
            .or_else(|| e.as_string())
            .unwrap_or_else(|| "unknown error".into())
    }

    /// A picked file's bytes, refused when larger than `limit` (before reading and after). Read
    /// from its Blob, so a read the browser refuses is an error (rfd's read helper unwraps it).
    async fn read(file: &rfd::FileHandle, limit: usize, too_large: &str) -> Result<Vec<u8>, String> {
        if file.inner().size() > limit as f64 {
            return Err(too_large.into());
        }
        let buffer = JsFuture::from(file.inner().array_buffer()).await.map_err(|e| message(&e))?;
        let bytes = js_sys::Uint8Array::new(&buffer);
        if bytes.length() as usize > limit {
            return Err(too_large.into());
        }
        Ok(bytes.to_vec())
    }

    /// Sign `msg` with the RSA key `pkcs8` through Web Crypto (it hashes with `digest`). The key is
    /// imported as not extractable; both copies of it, Rust's and the one handed to JavaScript, are
    /// wiped once the browser has taken it.
    async fn rsa_sign(pkcs8: zeroize::Zeroizing<Vec<u8>>, digest: DigestAlg, msg: &[u8]) -> Result<Vec<u8>, String> {
        let failed = |e: JsValue| format!("the browser couldn't sign: {}", message(&e));
        let subtle = web_sys::window().ok_or("no browser window")?.crypto().map_err(failed)?.subtle();
        let hash = match digest {
            DigestAlg::Sha384 => "SHA-384",
            DigestAlg::Sha512 => "SHA-512",
            _ => "SHA-256",
        };
        let algorithm = js_sys::Object::new();
        js_sys::Reflect::set(&algorithm, &JsValue::from_str("name"), &JsValue::from_str(RSA)).map_err(failed)?;
        js_sys::Reflect::set(&algorithm, &JsValue::from_str("hash"), &JsValue::from_str(hash)).map_err(failed)?;
        let key_data = js_sys::Uint8Array::from(pkcs8.as_slice());
        drop(pkcs8);
        let usages = js_sys::Array::of1(&JsValue::from_str("sign"));
        let import = subtle.import_key_with_object("pkcs8", &key_data, &algorithm, false, &usages);
        // importKey copies the bytes before it returns (Web Crypto, importKey step 2).
        key_data.fill(0, 0, key_data.length());
        let key = JsFuture::from(import.map_err(failed)?).await.map_err(failed)?;
        let key: web_sys::CryptoKey = key.dyn_into().map_err(|_| "the browser returned no key".to_string())?;
        let signature = JsFuture::from(subtle.sign_with_str_and_u8_array(RSA, &key, msg).map_err(failed)?).await.map_err(failed)?;
        Ok(js_sys::Uint8Array::new(&signature).to_vec())
    }

    impl PdfCraftApp {
        /// Configure ▸ Browse…: a digital ID file from the browser's picker.
        pub(crate) fn pick_id_file(&mut self) {
            self.pick_for_signing(false);
        }

        /// Sign as ▸ Choose image…: the signature's graphic from the browser's picker.
        pub(crate) fn pick_sign_graphic_image(&mut self) {
            self.pick_for_signing(true);
        }

        /// The browser's file picker for the Sign dialogs. The file arrives on a later frame, into
        /// the dialog that asked for it if it is still open ([`Self::take_signing_pick`]).
        fn pick_for_signing(&mut self, image: bool) {
            if self.web_signing.busy.swap(true, Ordering::SeqCst) {
                self.notify_tr("Another file dialog is still open. Finish with it first.");
                return;
            }
            let epoch = self.dialog_epoch();
            let inbox = self.web_signing.clone();
            let ctx = self.ctx.clone();
            let dialog = if image {
                rfd::AsyncFileDialog::new().add_filter(tl!("Image"), &["png", "jpg", "jpeg"])
            } else {
                rfd::AsyncFileDialog::new().add_filter(tl!("Digital ID"), &["p12", "pfx"])
            };
            wasm_bindgen_futures::spawn_local(async move {
                let picked = match dialog.pick_file().await {
                    Some(file) if image => Some(Picked::Image(read(&file, MAX_SIGNATURE_IMAGE_BYTES, &SignatureImageError::Size.to_string()).await)),
                    Some(file) => {
                        let name = file.file_name();
                        let too_large = format!("{name} is too large to be a digital ID file.");
                        let bytes = read(&file, MAX_ID_FILE_BYTES, &too_large).await;
                        Some(Picked::IdFile(bytes.map(|bytes| IdFile { name, bytes: Arc::new(bytes) })))
                    }
                    None => None,
                };
                inbox.busy.store(false, Ordering::SeqCst);
                if let Some(picked) = picked
                    && let Ok(mut slot) = inbox.picked.lock()
                {
                    *slot = Some((epoch, picked));
                }
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
        }

        /// A file the picker read, into the Sign dialog that asked for it (dropped if that dialog
        /// closed meanwhile).
        pub(crate) fn take_signing_pick(&mut self) {
            let Some((epoch, picked)) = self.web_signing.picked.lock().ok().and_then(|mut p| p.take()) else { return };
            if self.dialog_epoch() != epoch || self.dialog != Some(crate::Dialog::Sign) {
                return;
            }
            match picked {
                Picked::IdFile(file) => {
                    let Some(d) = self.sign_draft.as_mut() else { return };
                    match file {
                        Ok(file) => {
                            d.new_id.file = file.name.clone();
                            d.new_id.picked = Some(file);
                            d.error = None;
                        }
                        Err(e) => d.error = Some(e),
                    }
                }
                Picked::Image(bytes) => {
                    // Scanned paper becomes transparent, as for Fill & Sign image signatures.
                    let image = bytes.and_then(|bytes| {
                        pdfcraft_engine::guard(|| SignatureImage::from_bytes(&bytes).and_then(|image| image.remove_white_background(245, 35)))
                            .map_err(|e| e.to_string())?
                            .map_err(|e| e.to_string())
                    });
                    match (image, self.sign_draft.as_mut()) {
                        (Ok(image), Some(d)) => {
                            d.graphic = SignGraphic::Image(image.clone());
                            d.image = Some(image);
                            d.error = None;
                        }
                        (Ok(_), None) => {}
                        (Err(e), _) => self.notify_fmt("Couldn't import the signature image: {e}", &[("e", &e)]),
                    }
                }
            }
        }

        /// Sign `doc` with `id` and hand the signed file, `name`, to the browser: now for an ECDSA
        /// key, once Web Crypto has answered for an RSA one (the dialog closes meanwhile; a failure
        /// from then on arrives as a notice).
        pub(crate) fn sign_in_browser(
            &mut self,
            doc: DocId,
            id: DigitalId,
            opts: SignOptions,
            name: String,
            choice: SignGraphic,
        ) -> Result<(), String> {
            // What was typed in a form field is signed with the document (#166).
            if !self.commit_typing_in(doc) {
                return Err(String::new());
            }
            if !matches!(id.certificate.public_key, PublicKey::Rsa { .. }) {
                let signed = self.session.sign(doc, &id, opts).map_err(|e| e.to_string())?;
                return self.deliver_signed(doc, signed, &name, choice);
            }
            if !web_sys::window().is_some_and(|w| w.is_secure_context()) {
                return Err(
                    "Signing with an RSA key needs PdfCraft served over HTTPS (or from this computer): browsers offer their cryptography only there."
                        .into(),
                );
            }
            let request = self.session.signing_request(doc, id.certificate.clone(), id.chain.clone(), opts).map_err(|e| e.to_string())?;
            let pkcs8 = zeroize::Zeroizing::new(id.key.pkcs8().to_vec());
            drop(id);
            let answers = self.web_signing.answers.clone();
            let ctx = self.ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let signature = rsa_sign(pkcs8, request.digest(), request.message()).await;
                if let Ok(mut a) = answers.lock() {
                    a.push(Answer { request, name, choice, signature });
                }
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
            Ok(())
        }

        /// Finish the signatures Web Crypto has answered (every frame).
        pub(crate) fn finish_browser_signatures(&mut self) {
            let answers: Vec<Answer> = self.web_signing.answers.lock().map(|mut a| std::mem::take(&mut *a)).unwrap_or_default();
            for Answer { request, name, choice, signature } in answers {
                let signed = signature.and_then(|s| self.session.complete_signing(&request, &s).map_err(|e| e.to_string()));
                let done = signed.and_then(|signed| self.deliver_signed(request.doc(), signed, &name, choice));
                if let Err(e) = done
                    && !e.is_empty()
                {
                    self.notify_fmt("Couldn't sign: {e}", &[("e", &e)]);
                }
            }
        }

        /// Hand the signed file to the embedding page or download it, then show it as the
        /// document's saved state, as signing on the desktop does.
        fn deliver_signed(&mut self, doc: DocId, signed: Arc<Vec<u8>>, name: &str, choice: SignGraphic) -> Result<(), String> {
            let to_host = self.host_save.is_some();
            let delivered = match &self.host_save {
                Some(save) => save(name, &signed, true),
                None => crate::editing::download(name, &signed),
            };
            if let Err(e) = delivered {
                let template = if to_host { "Couldn't save {name}: {e}" } else { "Couldn't download {name}: {e}" };
                self.notify_fmt(template, &[("name", name), ("e", &e)]);
                return Err(String::new());
            }
            self.session.mark_signed(doc, signed, None).map_err(|e| e.to_string())?;
            self.sign_graphic = choice;
            if let Some(view) = self.views.iter_mut().find(|v| v.id == doc) {
                view.invalidate_content();
            }
            self.right = Some(crate::RightPanel::Signatures);
            // The embedding page reports where the file went itself.
            if !to_host {
                self.notify_fmt("Downloaded {name}", &[("name", name)]);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use pdfcraft_engine::SignatureStatus;

    use super::*;
    use crate::{Dialog, SignStep};

    const FIXTURE: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    fn id_file(name: &str) -> IdFile {
        let bytes = std::fs::read(format!("{}/../sign/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
        IdFile { name: name.into(), bytes: Arc::new(bytes) }
    }

    /// The Sign dialog for a visible signature on page 1, at Configure (no IDs yet), with the
    /// signed file saved to `out`.
    fn harness(out: &std::path::Path) -> Harness<'static, PdfCraftApp> {
        let out = out.to_string_lossy().into_owned();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            // After the test certificates' start date (2026-10-02).
            app.session = std::mem::take(&mut app.session).with_clock(|| 1_800_000_000);
            app.open_bytes("contract.pdf", None, FIXTURE.to_vec()).unwrap();
            app.save_override = Some(out);
            app.start_signing(0, Some([40.0, 40.0, 220.0, 90.0]), None, None);
            app
        });
        h.run_steps(3);
        h
    }

    /// What the browser's picker does: the file arrives in the open Configure dialog.
    fn pick(h: &mut Harness<'static, PdfCraftApp>, file: IdFile, password: &str) {
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        d.new_id.create = false;
        d.new_id.file = file.name.clone();
        d.new_id.picked = Some(file);
        d.new_id.file_password = password.into();
        // The dialog changes size to the file form over a few frames before its button can be hit.
        h.run_steps(3);
        h.get_by_label("Continue").click();
        h.run_steps(3);
    }

    fn scratch(test: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-web-sign-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("contract_signed.pdf")
    }

    #[test]
    fn a_digital_id_file_held_in_memory_signs_and_never_reaches_the_settings() {
        let out = scratch("signs");
        let mut h = harness(&out);
        assert_eq!(h.state().sign_draft.as_ref().map(|d| d.step), Some(SignStep::Configure));
        pick(&mut h, id_file("rsa-aes.p12"), "test");
        let s = h.state();
        assert_eq!(s.sign_draft.as_ref().map(|d| d.step), Some(SignStep::Choose));
        assert_eq!(
            s.digital_ids.iter().map(|e| (e.path.as_str(), e.name.as_str())).collect::<Vec<_>>(),
            [("browser:rsa-aes.p12", "Test Signer RSA")]
        );
        assert_eq!(s.id_files.len(), 1);
        h.get_by_label("Continue").click();
        h.run_steps(3);
        h.state_mut().sign_draft.as_mut().unwrap().password = "test".into();
        h.get_by_label("Sign").click();
        h.run_steps(4);

        let s = h.state();
        assert_eq!(s.dialog, None, "{:?}", s.sign_draft.as_ref().and_then(|d| d.error.clone()));
        let signed = std::fs::read(&out).expect("the signed file was saved");
        let doc = s.session.get(s.views[0].id).unwrap();
        let sig = doc.signatures.iter().find(|x| x.signed).expect("signed");
        assert_eq!((sig.status, sig.page, sig.signer.as_deref()), (SignatureStatus::Unknown, Some(0), Some("Test Signer RSA")));
        assert_eq!(doc.bytes.as_slice(), signed.as_slice());
        // The file stays in memory: the settings never hold it, and a fresh start doesn't list it.
        let saved = s.persist();
        assert!(!saved.contains("browser:"), "{saved}");
        let mut fresh = PdfCraftApp::new();
        fresh.restore(&saved);
        assert!(fresh.digital_ids.is_empty() && fresh.id_files.is_empty());
        // Removing it from the list lets go of the bytes.
        assert!(h.state_mut().remove_digital_id(0));
        assert!(h.state().id_files.is_empty());
    }

    #[test]
    fn a_picked_file_is_checked_before_it_is_listed_and_the_same_name_replaces_it() {
        let out = scratch("checked");
        let mut h = harness(&out);
        pick(&mut h, id_file("ec-p256.p12"), "wrong");
        let s = h.state();
        assert_eq!(s.sign_draft.as_ref().and_then(|d| d.error.as_deref()), Some("The password is incorrect."));
        assert!(s.digital_ids.is_empty() && s.id_files.is_empty());
        let not_an_id = IdFile { name: "notes.p12".into(), bytes: Arc::new(b"not a PKCS #12 file".to_vec()) };
        pick(&mut h, not_an_id, "test");
        assert!(h.state().sign_draft.as_ref().and_then(|d| d.error.as_deref()).is_some_and(|e| e.contains("PKCS #12")));
        assert!(h.state().id_files.is_empty());
        // Nothing picked and no path: say so instead of reading "".
        let d = h.state_mut().sign_draft.as_mut().unwrap();
        (d.new_id.picked, d.new_id.file) = (None, String::new());
        h.get_by_label("Continue").click();
        h.run_steps(2);
        assert_eq!(h.state().sign_draft.as_ref().and_then(|d| d.error.as_deref()), Some("Choose a digital ID file."));
        // Picked twice under one name: one entry, the newer bytes.
        pick(&mut h, id_file("ec-p256.p12"), "test");
        h.get_by_label("Configure New Digital ID").click();
        h.run_steps(2);
        let newer = IdFile { name: "ec-p256.p12".into(), ..id_file("ec-legacy.p12") };
        pick(&mut h, newer.clone(), "test");
        let s = h.state();
        assert_eq!((s.digital_ids.len(), s.id_files.as_slice()), (1, std::slice::from_ref(&newer)));
        assert_eq!(s.dialog, Some(Dialog::Sign));
    }

    #[test]
    fn a_listed_file_no_longer_in_memory_says_so() {
        let app = PdfCraftApp::new();
        let err = app.id_file_bytes("browser:gone.p12").unwrap_err();
        assert!(err.contains("no longer open"), "{err}");
        assert!(in_memory("browser:gone.p12") && !in_memory("C:/IDs/me.p12") && !in_memory("keychain:me"));
    }
}
