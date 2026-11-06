//! Headless browser signing. All paths pass through the same confinement as local signing.

use std::time::{Duration, Instant};

use pdfcraft_engine::{CompletedSignature, DocId, PendingSignature, sign};
use pdfcraft_platform::signing_browser::{SigningExchange, SigningTransport};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed};

pub(crate) struct RemoteJob {
    doc: DocId,
    pending: Option<PendingSignature>,
    completed: Option<CompletedSignature>,
    exchange: Option<Box<dyn SigningExchange>>,
    out: String,
    started: Instant,
    error: Option<String>,
}

impl Drop for RemoteJob {
    fn drop(&mut self) {
        if let Some(exchange) = &self.exchange {
            exchange.cancel();
        }
    }
}

fn invalid(message: &str) -> ToolError {
    ToolError::InvalidArgs(message.into())
}

fn job_id() -> Result<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| failed("could not create a signing operation identifier"))?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }
    #[cfg(target_arch = "wasm32")]
    {
        Err(failed("browser signing requires the desktop application"))
    }
}

impl Automation {
    /// Opt in to a specific transport. CLI/MCP hosts decide whether remote signing is allowed;
    /// tests inject synthetic exchanges. Merely constructing Automation opens no connection.
    pub fn with_signing_transport(mut self, transport: SigningTransport) -> Self {
        self.signing_transport = Some(transport);
        self
    }

    pub(crate) fn sign_providers(&self) -> Value {
        json!({"providers":[{"id":"id-austria","name":"ID Austria / A-Trust","enabled":cfg!(not(target_arch = "wasm32")) && self.signing_transport.is_some(),"document_upload":true,"requires":"Activated personal ID Austria and an installed Chromium, Chrome or Edge browser. No developer account is configured."}],"web_signing":false})
    }

    pub(crate) fn sign_begin(&mut self, a: &Args) -> Result<Value> {
        if cfg!(target_arch = "wasm32") {
            return Err(failed("ID Austria remote signing requires a desktop application"));
        }
        let wait = a.opt_bool("wait")?.unwrap_or(false);
        if a.str("provider")? != "id-austria" {
            return Err(invalid("provider must be id-austria"));
        }
        if a.opt_bool("allow_document_upload")? != Some(true) {
            return Err(invalid(
                "ID Austria sends the prepared PDF to A-Trust; set allow_document_upload: true after the user accepts this transfer",
            ));
        }
        let transport = self
            .signing_transport
            .clone()
            .ok_or_else(|| failed("remote signing is disabled by this host; the CLI/MCP host must opt in with --remote-signing"))?;
        if self.signing_jobs.len() >= 4 {
            return Err(failed("at most four signing operations may be retained; finish or cancel one first"));
        }
        let (doc, opts) = self.signing_options(a)?;
        if self.signing_jobs.values().any(|job| job.doc == doc) {
            return Err(failed("this document already has a pending signing operation; finish or cancel it first"));
        }
        let out = a.str("out")?.to_string();
        let path = self.resolve(&out, true)?;
        if std::fs::symlink_metadata(&path).is_ok() {
            return Err(failed("signing needs a new output file; the destination already exists"));
        }
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
        drop(tempfile::NamedTempFile::new_in(parent).map_err(failed)?);
        let pending = self.session.prepare_external_signature(doc, None, opts, sign::SignatureProfile::CmsDetached).map_err(failed)?;
        let xml = sign::security_layer::cms_request(pending.prepared()).map_err(failed)?;
        let id = job_id()?;
        let exchange = transport(xml).map_err(failed)?;
        self.signing_jobs.insert(
            id.clone(),
            RemoteJob { doc, pending: Some(pending), completed: None, exchange: Some(exchange), out, started: Instant::now(), error: None },
        );
        if wait {
            let args = json!({"job":id});
            loop {
                let status = self.sign_status(&Args(&args))?;
                match status.get("state").and_then(Value::as_str) {
                    Some("ready_to_save") => return self.sign_finish(&Args(&args)),
                    Some("failed") => {
                        return Err(failed(format!(
                            "signing operation {id}: {}",
                            status.get("error").and_then(Value::as_str).unwrap_or("authorization failed")
                        )));
                    }
                    _ => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        }
        Ok(
            json!({"job":id,"doc":doc.0,"state":"awaiting_authorization","document_upload":true,"next":"Approve in the provider browser, then call sign_status and sign_finish. sign_cancel keeps the source document."}),
        )
    }

    pub(crate) fn sign_status(&mut self, a: &Args) -> Result<Value> {
        let id = a.str("job")?;
        let job = self.signing_jobs.get_mut(id).ok_or_else(|| invalid("unknown or consumed signing operation"))?;
        if job.started.elapsed() > Duration::from_secs(600) && job.completed.is_none() && job.error.is_none() {
            if let Some(exchange) = job.exchange.take() {
                exchange.cancel();
            }
            job.pending = None;
            job.error = Some("the signing operation expired; cancel it and start again".into());
        }
        if let Some(result) = job.exchange.as_ref().and_then(|exchange| exchange.poll()) {
            job.exchange = None;
            let pending = job.pending.take().ok_or_else(|| failed("the signing operation has no prepared document"))?;
            let verified = result
                .and_then(|xml| sign::security_layer::cms_response(&xml).map_err(|e| e.to_string()))
                .and_then(|cms| self.session.complete_external_signature(pending, &cms).map_err(|e| e.to_string()));
            match verified {
                Ok(completed) => job.completed = Some(completed),
                Err(error) => job.error = Some(error),
            }
        }
        let state = if job.completed.is_some() {
            "ready_to_save"
        } else if job.error.is_some() {
            "failed"
        } else {
            "awaiting_authorization"
        };
        Ok(json!({"job":id,"doc":job.doc.0,"state":state,"error":job.error}))
    }

    pub(crate) fn sign_finish(&mut self, a: &Args) -> Result<Value> {
        let id = a.str("job")?;
        let _ = self.sign_status(a)?;
        let job = self.signing_jobs.get(id).ok_or_else(|| invalid("unknown or consumed signing operation"))?;
        let completed = job
            .completed
            .as_ref()
            .ok_or_else(|| failed(job.error.as_deref().unwrap_or("authorization is not complete; call sign_status or sign_cancel")))?;
        let out = a.opt_str("out")?.unwrap_or(&job.out);
        // Resolve again after the potentially long authorization, including changed symlinks.
        let path = self.resolve(out, true)?;
        self.session.check_completed_signature(completed).map_err(failed)?;
        pdfcraft_platform::staging::write_new_atomically(&path, completed.bytes()).map_err(failed)?;
        let doc = completed.doc();
        let bytes = completed.bytes().clone();
        self.session.mark_signed(doc, bytes.clone(), Some(path.to_string_lossy().into_owned())).map_err(failed)?;
        self.signing_jobs.remove(id);
        Ok(json!({"job":id,"doc":doc.0,"state":"saved","path":path,"bytes":bytes.len(),"qualified_status":"not_evaluated"}))
    }

    pub(crate) fn sign_cancel(&mut self, a: &Args) -> Result<Value> {
        let id = a.str("job")?;
        self.signing_jobs.remove(id);
        Ok(json!({"job":id,"state":"cancelled"}))
    }
}
