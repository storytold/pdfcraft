//! Headless certificate security. PINs come from the host, never tool arguments or scripts.
use crate::{Args, Automation, Result, ToolError, failed};
use serde_json::{Value, json};

impl Automation {
    pub(crate) fn encryption_token_ids(&self) -> Result<Value> {
        #[cfg(target_os = "linux")]
        {
            let identities = pdfcraft_engine::sign::pkcs11::decryption_identities().map_err(failed)?;
            let ids: Vec<_> = identities
                .iter()
                .map(|identity| {
                    json!({
                        "id": pdfcraft_engine::sign::pkcs11::reference(identity.certificate()),
                        "token_label": identity.token_label(),
                        "certificate": crate::signing::cert_json(identity.certificate()),
                    })
                })
                .collect();
            Ok(json!({ "count": ids.len(), "ids": ids }))
        }
        #[cfg(not(target_os = "linux"))]
        Err(failed("PKCS #11 encryption identities require Linux"))
    }

    pub(crate) fn doc_open_token(&mut self, a: &Args) -> Result<Value> {
        // Resolve and read the PDF before any discovery or authentication.
        let path = self.resolve(a.str("path")?, false)?;
        let bytes = std::sync::Arc::new(crate::read_input(&path)?);
        let reference = a.str("id")?;
        #[cfg(target_os = "linux")]
        {
            let identity = pdfcraft_engine::sign::pkcs11::find_decryption_identity(reference).map_err(failed)?;
            let pin = std::env::var("PDFCRAFT_PKCS11_PIN").ok().map(zeroize::Zeroizing::new);
            let resolver = pdfcraft_engine::certificate_security::TokenResolver { identity: &identity, pin: pin.as_ref().map(|p| p.as_str()) };
            let name = path.file_name().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let id = self.session.open_with_security_resolver(name, Some(path.to_string_lossy().into_owned()), bytes, &resolver).map_err(failed)?;
            Ok(crate::json::summary(self.session.get(id).ok_or_else(|| failed("the document vanished"))?))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (bytes, reference);
            Err(failed("opening with a PKCS #11 token requires Linux"))
        }
    }

    pub(crate) fn doc_protect_certificates(&mut self, a: &Args) -> Result<Value> {
        use pdfcraft_engine::{
            Algorithm, Edit,
            certificate_security::{CertificateProtection, CertificateRecipient},
        };
        self.doc(a)?;
        let recipients = a
            .get("recipients")
            .and_then(Value::as_array)
            .filter(|r| !r.is_empty() && r.len() <= 64)
            .ok_or_else(|| ToolError::InvalidArgs("recipients must contain 1 to 64 certificate/permissions objects".into()))?;
        let mut entries = Vec::with_capacity(recipients.len());
        for recipient in recipients {
            let object = recipient.as_object().ok_or_else(|| ToolError::InvalidArgs("each recipient must be an object".into()))?;
            if object.keys().any(|k| !matches!(k.as_str(), "certificate" | "permissions")) {
                return Err(ToolError::InvalidArgs("recipient fields are certificate and permissions; PINs are not accepted".into()));
            }
            let r = Args(recipient);
            let reference = r.str("certificate")?;
            let certificate = if reference.starts_with("pkcs11:") {
                #[cfg(target_os = "linux")]
                {
                    pdfcraft_engine::sign::pkcs11::find_decryption_identity(reference).map_err(failed)?.certificate().raw.clone()
                }
                #[cfg(not(target_os = "linux"))]
                return Err(failed("token certificate references require Linux"));
            } else {
                let path = self.resolve(reference, false)?;
                use std::io::Read;
                let mut bytes = Vec::new();
                std::fs::File::open(&path).map_err(failed)?.take(64 * 1024 + 1).read_to_end(&mut bytes).map_err(failed)?;
                if bytes.len() > 64 * 1024 {
                    return Err(ToolError::InvalidArgs("recipient DER certificates must not exceed 64 KiB".into()));
                }
                bytes
            };
            let permissions = i32::try_from(r.opt_int("permissions")?.unwrap_or(-1))
                .map_err(|_| ToolError::InvalidArgs("recipient permissions must be a signed 32-bit integer".into()))?;
            entries.push(CertificateRecipient { certificate, permissions });
        }
        let algorithm = match a.opt_str("compatibility")?.unwrap_or("aes-256") {
            "aes-128" => Algorithm::Aes128,
            "aes-256" => Algorithm::Aes256,
            _ => return Err(ToolError::InvalidArgs("certificate compatibility must be aes-128 or aes-256".into())),
        };
        let edit = Edit::ProtectCertificates(CertificateProtection {
            recipients: entries,
            algorithm,
            encrypt_metadata: a.opt_bool("encrypt_metadata")?.unwrap_or(true),
        });
        let mut out = self.apply(a, edit)?;
        out["security"] = crate::json::security(self.doc(a)?);
        Ok(out)
    }
}
