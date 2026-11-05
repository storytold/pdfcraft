//! Asynchronous file-dialog adapters for an embedding application such as the mobile host.
//! A save is acknowledged only after the host confirms the write; cancellation and
//! concurrent edits never clear the document's unsaved state or close its tab.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use pdfcraft_engine::DocId;

use crate::{PdfCraftApp, SaveTarget};

pub struct HostDocument {
    pub name: String,
    pub bytes: Vec<u8>,
    /// Opaque host identity; it is attached to a document, never inferred from a name.
    pub origin: Option<String>,
}
pub struct HostSave {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub save_as: bool,
    pub origin: Option<String>,
}
pub type OpenReply = Box<dyn FnOnce(Result<Option<HostDocument>, String>) + Send>;
pub type SaveReply = Box<dyn FnOnce(Result<bool, String>) + Send>;
type After = Box<dyn FnOnce(&mut PdfCraftApp) + Send>;
type OpenHandler = Box<dyn Fn(OpenReply) -> Result<(), String>>;
type SaveHandler = Box<dyn Fn(HostSave, SaveReply) -> Result<(), String>>;

/// File access belongs to the host; the UI owns documents and confirms completion.
pub struct HostFiles {
    pub open: OpenHandler,
    pub save: SaveHandler,
    pending: Arc<Mutex<Option<Completion>>>,
    active: bool,
    origins: HashMap<DocId, String>,
    /// OS-delivered imports retain the same host identity as picker imports.
    pub incoming: Arc<Mutex<Vec<HostDocument>>>,
}

enum Completion {
    Open(Result<Option<HostDocument>, String>),
    Save { id: DocId, generation: u64, name: String, bytes: Arc<Vec<u8>>, result: Result<bool, String>, after: After },
}

impl HostFiles {
    pub fn new(open: OpenHandler, save: SaveHandler) -> Self {
        Self { open, save, pending: Arc::new(Mutex::new(None)), active: false, origins: HashMap::new(), incoming: Arc::new(Mutex::new(Vec::new())) }
    }
}

impl PdfCraftApp {
    fn open_host_document(&mut self, file: HostDocument) {
        let before = self.views.len();
        if let Err(error) = self.open_bytes(&file.name, None, file.bytes) {
            self.notify_error(error);
            return;
        }
        if let Some(origin) = file.origin {
            if let Some(prompt) = &mut self.password_prompt {
                prompt.host_origin = Some(origin);
            } else if self.views.len() > before {
                self.bind_host_origin(origin);
            }
        }
    }

    pub(crate) fn bind_host_origin(&mut self, origin: String) {
        if let Some((_, id)) = self.active_ids()
            && let Some(host) = &mut self.host_files
        {
            host.origins.insert(id, origin);
        }
    }

    pub(crate) fn open_from_host(&mut self) {
        let Some(host) = &mut self.host_files else { return };
        if host.active {
            self.notify_tr("Another file dialog is still open. Finish with it first.");
            return;
        }
        host.active = true;
        let pending = host.pending.clone();
        let ctx = self.ctx.clone();
        let result = (host.open)(Box::new(move |result| {
            if let Ok(mut slot) = pending.lock() {
                *slot = Some(Completion::Open(result));
            }
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        }));
        if let Err(error) = result {
            host.active = false;
            self.notify_error(error);
        }
    }

    pub(crate) fn save_to_host(&mut self, index: usize, target: SaveTarget, after: impl FnOnce(&mut Self) + Send + 'static) -> bool {
        if self.host_files.as_ref().is_some_and(|host| host.active) {
            self.notify_tr("Another file dialog is still open. Finish with it first.");
            return false;
        }
        let Some(id) = self.views.get(index).map(|view| view.id) else { return false };
        if !self.flatten_fill_sign_for_save(id) {
            return false;
        }
        let Some(doc) = self.session.get(id) else { return false };
        let name = doc.name.clone();
        let generation = doc.edit_generation();
        let bytes = match self.session.save_bytes(id) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.notify_error(error);
                return false;
            }
        };
        let Some(host) = &mut self.host_files else { return false };
        host.active = true;
        let pending = host.pending.clone();
        let ctx = self.ctx.clone();
        let saved_bytes = bytes.clone();
        let saved_name = name.clone();
        let result = (host.save)(
            HostSave { name: name.clone(), bytes, save_as: matches!(target, SaveTarget::As), origin: host.origins.get(&id).cloned() },
            Box::new(move |result| {
                if let Ok(mut slot) = pending.lock() {
                    *slot = Some(Completion::Save { id, generation, name: saved_name, bytes: saved_bytes, result, after: Box::new(after) });
                }
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            }),
        );
        if let Err(error) = result {
            host.active = false;
            self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &error)]);
        }
        false // Completion arrives on a later frame, even if the adapter replied immediately.
    }

    pub(crate) fn poll_host_files(&mut self) {
        if self.password_prompt.is_none() {
            let incoming = self
                .host_files
                .as_ref()
                .and_then(|host| host.incoming.lock().ok().and_then(|mut queue| if queue.is_empty() { None } else { Some(queue.remove(0)) }));
            if let Some(file) = incoming {
                self.open_host_document(file);
            }
        }
        let Some(host) = &mut self.host_files else { return };
        host.origins.retain(|id, _| self.session.get(*id).is_some());
        let completion = host.pending.lock().ok().and_then(|mut slot| slot.take());
        let Some(completion) = completion else { return };
        host.active = false;
        match completion {
            Completion::Open(Ok(Some(file))) => self.open_host_document(file),
            Completion::Open(Err(error)) => self.notify_error(error),
            Completion::Open(Ok(None)) | Completion::Save { result: Ok(false), .. } => {}
            Completion::Save { name, result: Err(error), .. } => self.notify_fmt("Couldn't save {name}: {e}", &[("name", &name), ("e", &error)]),
            Completion::Save { id, generation, name, bytes, result: Ok(true), after } => {
                if let Some(host) = &mut self.host_files {
                    host.origins.remove(&id);
                }
                // An older snapshot was exported. Keep newer edits and the tab open.
                if self.session.get(id).is_none_or(|doc| doc.edit_generation() != generation) {
                    return;
                }
                if let Err(error) = self.session.mark_saved(id, bytes, None) {
                    self.notify_error(error);
                    return;
                }
                if let Some(index) = self.views.iter().position(|view| view.id == id) {
                    if let (Some(doc), Some(view)) = (self.session.get(id), self.views.get_mut(index)) {
                        view.document_changed(&doc.info);
                    }
                    self.notify_fmt("Saved {name}", &[("name", &name)]);
                    if !self.has_unsaved_work(index) {
                        after(self);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn app() -> (PdfCraftApp, Arc<Mutex<Option<SaveReply>>>) {
        let mut app = PdfCraftApp::new();
        let pdf = app.session.create_blank(100.0, 100.0, 1).unwrap();
        app.open_bytes("document.pdf", None, (*pdf).clone()).unwrap();
        assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        let pending = Arc::new(Mutex::new(None));
        let reply = pending.clone();
        app.host_files = Some(HostFiles::new(
            Box::new(|reply| {
                reply(Ok(None));
                Ok(())
            }),
            Box::new(move |_, response| {
                *reply.lock().unwrap() = Some(response);
                Ok(())
            }),
        ));
        (app, pending)
    }

    #[test]
    fn encrypted_recovery_identity_survives_wrong_password_and_queued_imports() {
        let (mut app, _) = app();
        let bytes = app.session.create_blank(100.0, 100.0, 1).unwrap();
        let mut doc = pdfcraft_cos::Document::open(bytes.clone()).unwrap();
        doc.set_encryption(&pdfcraft_cos::NewEncryption {
            algorithm: pdfcraft_cos::Algorithm::Aes256,
            user_password: "secret",
            owner_password: "owner",
            permissions: -1,
            encrypt_metadata: true,
            seed: [2; 32],
        })
        .unwrap();
        let encrypted = pdfcraft_cos::write_full(&doc, &Default::default()).unwrap();
        app.open_host_document(HostDocument { name: "same.pdf".into(), bytes: encrypted, origin: Some("recovered".into()) });
        // Main's deferred tool and the mobile recovery identity share the password prompt.
        app.password_prompt.as_mut().unwrap().then = Some("page.organize");
        app.host_files.as_ref().unwrap().incoming.lock().unwrap().push(HostDocument {
            name: "same.pdf".into(),
            bytes: (*bytes).clone(),
            origin: None,
        });
        app.poll_host_files();
        assert_eq!(app.password_prompt.as_ref().unwrap().host_origin.as_deref(), Some("recovered"));
        app.submit_password(Some("wrong".into()));
        assert_eq!(app.password_prompt.as_ref().unwrap().host_origin.as_deref(), Some("recovered"));
        assert_eq!(app.password_prompt.as_ref().unwrap().then, Some("page.organize"));
        app.submit_password(Some("secret".into()));
        let recovered = app.active_ids().unwrap().1;
        assert!(app.password_prompt.is_none());
        assert!(app.views[app.active.unwrap()].organize, "deferred tool runs after unlocking the recovered file");
        assert_eq!(app.host_files.as_ref().unwrap().origins.get(&recovered).map(String::as_str), Some("recovered"));
        app.poll_host_files();
        let other = app.active_ids().unwrap().1;
        assert_ne!(recovered, other);
        assert!(!app.host_files.as_ref().unwrap().origins.contains_key(&other));
    }

    #[test]
    fn same_named_import_cannot_inherit_another_documents_recovery_identity() {
        let (mut app, _) = app();
        let pdf = (*app.session.create_blank(100.0, 100.0, 1).unwrap()).clone();
        app.open_host_document(HostDocument { name: "same.pdf".into(), bytes: pdf.clone(), origin: Some("recovery-1".into()) });
        let recovered = app.active_ids().unwrap().1;
        app.open_host_document(HostDocument { name: "same.pdf".into(), bytes: pdf, origin: None });
        let regular = app.active_ids().unwrap().1;
        assert_ne!(recovered, regular);
        let host = app.host_files.as_ref().unwrap();
        assert_eq!(host.origins.get(&recovered).map(String::as_str), Some("recovery-1"));
        assert!(!host.origins.contains_key(&regular));
    }

    #[test]
    fn host_save_waits_for_confirmation_and_never_cleans_a_cancelled_or_failed_save() {
        for result in [Ok(false), Err("provider unavailable".into()), Ok(true)] {
            let success = matches!(result, Ok(true));
            let (mut app, reply) = app();
            let after = Arc::new(AtomicBool::new(false));
            let ran = after.clone();
            assert!(!app.save_then(0, SaveTarget::As, move |_| {
                ran.store(true, Ordering::SeqCst);
            }));
            assert!(app.has_unsaved_work(0));
            assert!(!after.load(Ordering::SeqCst));
            reply.lock().unwrap().take().unwrap()(result);
            assert!(app.has_unsaved_work(0));
            app.poll_host_files();
            assert_eq!(app.has_unsaved_work(0), !success);
            assert_eq!(after.load(Ordering::SeqCst), success);
        }
    }

    #[test]
    fn an_exported_older_snapshot_cannot_replace_newer_edits_or_close_the_tab() {
        let (mut app, reply) = app();
        let after = Arc::new(AtomicBool::new(false));
        let ran = after.clone();
        app.save_then(0, SaveTarget::As, move |_| {
            ran.store(true, Ordering::SeqCst);
        });
        assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        let id = app.views[0].id;
        let generation = app.session.get(id).unwrap().edit_generation();
        reply.lock().unwrap().take().unwrap()(Ok(true));
        app.poll_host_files();
        assert!(app.has_unsaved_work(0));
        assert_eq!(app.session.get(id).unwrap().edit_generation(), generation);
        assert!(!after.load(Ordering::SeqCst));
    }
}
