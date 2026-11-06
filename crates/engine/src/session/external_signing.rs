//! A remote authorization is bound to one immutable document generation, never the active tab.

use std::sync::Arc;

use pdfcraft_sign::{Certificate, PreparedSignature, SignOptions, SignatureProfile};

use crate::{DocId, EditError, Session, guard, writable};

struct SignatureGuard {
    doc: DocId,
    generation: u64,
    source: Arc<Vec<u8>>,
}

impl std::fmt::Debug for SignatureGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignatureGuard").field("doc", &self.doc).field("generation", &self.generation).finish_non_exhaustive()
    }
}

/// Prepared PDF and its original session generation. No network or authorization occurs here.
#[derive(Debug)]
pub struct PendingSignature {
    guard: SignatureGuard,
    prepared: PreparedSignature,
}

impl PendingSignature {
    /// Immutable signing input, for a protocol adapter that never changes the signed bytes.
    pub fn prepared(&self) -> &PreparedSignature {
        &self.prepared
    }
    /// The document this operation belongs to, regardless of the selected tab.
    pub fn doc(&self) -> DocId {
        self.guard.doc
    }
}

/// A checked result retained for saving again when a disk write fails. Reusing it never
/// contacts the provider or spends another authorization. The session is not yet marked saved.
pub struct CompletedSignature {
    guard: SignatureGuard,
    bytes: Arc<Vec<u8>>,
}

impl std::fmt::Debug for CompletedSignature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompletedSignature").field("doc", &self.guard.doc).field("bytes", &self.bytes.len()).finish_non_exhaustive()
    }
}

impl CompletedSignature {
    /// The document whose state was signed.
    pub fn doc(&self) -> DocId {
        self.guard.doc
    }
    /// Checked PDF output; callers still check the session generation immediately before writing.
    pub fn bytes(&self) -> &Arc<Vec<u8>> {
        &self.bytes
    }
}

impl Session {
    /// Prepare signing without holding a session lock during the user's authorization.
    pub fn prepare_external_signature(
        &self,
        doc: DocId,
        cert: Option<&Certificate>,
        mut opts: SignOptions,
        profile: SignatureProfile,
    ) -> Result<PendingSignature, EditError> {
        let d = self.get(doc).ok_or(EditError::NoDocument)?;
        // Refuse before handing any bytes to a remote authorization transport. A P=1
        // certification permits only invisible document timestamps, never approval signing.
        if d.signatures.iter().any(|signature| signature.signed && signature.certify == Some(1)) {
            return Err(EditError::Sign("this document's certification allows no changes; adding an approval signature is not permitted".into()));
        }
        if opts.certify.is_some() && d.signatures.iter().any(|signature| signature.signed) {
            return Err(EditError::Sign("certification must be the document's first signature; choose an approval signature instead".into()));
        }
        if let Some(name) = opts.field.as_deref()
            && d.form.iter().any(|field| field.name == name && field.read_only())
        {
            return Err(EditError::Sign("the selected signature field is read-only; choose an editable unsigned field".into()));
        }
        if d.bytes.len() > pdfcraft_sign::security_layer::MAX_DOCUMENT_BYTES {
            return Err(EditError::Sign("remote signing documents are limited to 100 MiB".into()));
        }
        let editor = d.editor.as_ref().ok_or_else(|| EditError::ReadOnly(d.read_only_reason.clone().unwrap_or_default()))?;
        if opts.date.is_empty() {
            opts.date = self.signing_date();
        }
        let prepared = guard(|| -> Result<_, EditError> {
            let cos = writable(&editor.cos)?;
            Ok(pdfcraft_sign::prepare_signature(&cos, cert, &opts, profile)?)
        })
        .map_err(EditError::Sign)??;
        Ok(PendingSignature { guard: SignatureGuard { doc, generation: d.edit_generation(), source: d.bytes.clone() }, prepared })
    }

    fn check_signature_guard(&self, expected: &SignatureGuard) -> Result<(), EditError> {
        let doc = self.get(expected.doc).ok_or(EditError::NoDocument)?;
        if doc.edit_generation() != expected.generation || !Arc::ptr_eq(&doc.bytes, &expected.source) {
            return Err(EditError::Sign("the document changed during authorization; check the current version and start signing again".into()));
        }
        Ok(())
    }

    /// Verify a provider CMS against its exact prepared document, without changing the session.
    pub fn complete_external_signature(&self, pending: PendingSignature, cms: &[u8]) -> Result<CompletedSignature, EditError> {
        self.check_signature_guard(&pending.guard)?;
        let bytes = Arc::new(guard(|| pending.prepared.complete(cms)).map_err(EditError::Sign)??);
        guard(|| self.check_save(pending.guard.doc, &bytes)).map_err(EditError::Sign)??;
        Ok(CompletedSignature { guard: pending.guard, bytes })
    }

    /// Check again immediately before writing a retained result (including retries after a
    /// write error). A later edit or reload never silently becomes a signed document.
    pub fn check_completed_signature(&self, completed: &CompletedSignature) -> Result<(), EditError> {
        self.check_signature_guard(&completed.guard)?;
        guard(|| self.check_save(completed.guard.doc, &completed.bytes)).map_err(EditError::Sign)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_external_signature_is_bound_to_its_document_even_after_a_same_bytes_reload() {
        let mut session = Session::new();
        let bytes = session.create_from_text("synthetic", "Approve this synthetic document").unwrap();
        let doc = session.open("synthetic.pdf", None, bytes.clone(), None).unwrap();
        let pending = session.prepare_external_signature(doc, None, SignOptions::default(), SignatureProfile::CmsDetached).unwrap();
        // Reload an equal document through a separate allocation, not the old captured bytes.
        session.reload(doc, Arc::new(bytes.as_ref().clone())).unwrap();
        let error = session.complete_external_signature(pending, b"not a CMS").unwrap_err().to_string();
        assert!(error.contains("changed"), "{error}");
    }
}
