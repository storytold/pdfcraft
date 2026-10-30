//! Read and write the supported document-wide public-key crypt filter.
use crate::{Algorithm, CosError, Dict, Document, EncryptDict, Object, PdfString, SecurityHandler};
use pdfcraft_crypt::public_key::{self, FILTER_NAME};

fn malformed(message: &str) -> CosError {
    CosError::Security(format!("public-key encryption: {message}"))
}

pub(super) fn read(doc: &Document, dict: &Dict) -> Result<EncryptDict, CosError> {
    if dict.name(b"SubFilter") != Some(b"adbe.pkcs7.s5") {
        return Err(malformed("only the adbe.pkcs7.s5 profile is supported"));
    }
    let filters = dict.get(b"CF").map(|o| doc.resolve(o)).ok_or_else(|| malformed("missing crypt filters"))?;
    let filters = filters.as_dict().ok_or_else(|| malformed("CF must be a dictionary"))?;
    if filters.len() != 1 {
        return Err(malformed("only one DefaultCryptFilter is supported"));
    }
    let filter = filters.get(FILTER_NAME).map(|o| doc.resolve(o)).ok_or_else(|| malformed("missing DefaultCryptFilter"))?;
    let filter = filter.as_dict().ok_or_else(|| malformed("DefaultCryptFilter must be a dictionary"))?;
    // A filter used by StmF or StrF authenticates at document open regardless of
    // AuthEvent (ISO 32000-1 Table 25); the entry is optional and defaults to DocOpen.
    let (algorithm, bits) = match (dict.int(b"V"), filter.name(b"CFM")) {
        (Some(4), Some(b"AESV2")) => (Algorithm::Aes128, 128),
        (Some(5), Some(b"AESV3")) => (Algorithm::Aes256, 256),
        _ => return Err(malformed("only AES-128 V4 or AES-256 V5 is supported")),
    };
    // Length is optional for AES, whose method fixes the key size. Present values may
    // be indirect, but must resolve to an integer in bits matching that method.
    for length in [filter.get(b"Length"), dict.get(b"Length")].into_iter().flatten() {
        if !matches!(&*doc.resolve(length), Object::Int(length) if *length == bits) {
            return Err(malformed("public-key Length must be an integer in bits and match its AES method"));
        }
    }
    if dict.name(b"StmF") != Some(FILTER_NAME) || dict.name(b"StrF") != Some(FILTER_NAME) {
        return Err(malformed("strings and streams must use DefaultCryptFilter"));
    }
    let recipients = filter.get(b"Recipients").map(|o| doc.resolve(o)).ok_or_else(|| malformed("missing Recipients"))?;
    let recipients = recipients.as_array().ok_or_else(|| malformed("Recipients must be an array"))?;
    if recipients.is_empty() || recipients.len() > 64 {
        return Err(malformed("expected 1 to 64 recipient envelopes"));
    }
    let mut bytes = Vec::with_capacity(recipients.len());
    let mut total = 0usize;
    for recipient in recipients {
        let object = doc.resolve(recipient);
        let value = object.as_string().ok_or_else(|| malformed("each recipient envelope must be a byte string"))?;
        total = total.saturating_add(value.bytes.len());
        if value.bytes.is_empty() || value.bytes.len() > 1024 * 1024 || total > 4 * 1024 * 1024 {
            return Err(malformed("recipient envelope size limit exceeded"));
        }
        bytes.push(value.bytes.clone());
    }
    let metadata = match filter.get(b"EncryptMetadata") {
        None | Some(Object::Bool(true)) => true,
        Some(Object::Bool(false)) => false,
        _ => return Err(malformed("EncryptMetadata must be a boolean")),
    };
    let mut descriptor = public_key::dictionary(algorithm, bytes, metadata).map_err(|e| malformed(&e.to_string()))?;
    if let Some(value) = dict.get(b"EFF") {
        let name = value.as_name().ok_or_else(|| malformed("EFF must name a crypt filter"))?;
        if name != FILTER_NAME && name != b"Identity" {
            return Err(malformed("unsupported EFF crypt filter"));
        }
        descriptor.ef_f = name.to_vec();
    }
    Ok(descriptor)
}

impl Document {
    /// Apply an authenticated public-key writer context on the next full save. The caller
    /// supplies a fresh, random file identifier for a document without one. Existing IDs stay.
    /// Higher layers must check permission to change security before calling this method.
    pub fn set_public_key_encryption(&mut self, handler: SecurityHandler, file_id: [u8; 16]) -> Result<(), CosError> {
        let dict = handler.dict();
        let params = dict.public_key.as_ref().ok_or_else(|| malformed("expected a public-key writer context"))?;
        let string = |bytes: &[u8]| Object::String(PdfString { bytes: bytes.to_vec(), hex: true });
        let mut filter = Dict::new();
        filter.set(b"Type".to_vec(), Object::name("CryptFilter"));
        filter.set(b"CFM".to_vec(), Object::name(if dict.v == 5 { "AESV3" } else { "AESV2" }));
        filter.set(b"Length".to_vec(), Object::Int(dict.length_bits));
        filter.set(b"AuthEvent".to_vec(), Object::name("DocOpen"));
        filter.set(b"EncryptMetadata".to_vec(), Object::Bool(dict.encrypt_metadata));
        filter.set(b"Recipients".to_vec(), Object::Array(params.recipients.iter().map(|r| string(r)).collect()));
        let mut filters = Dict::new();
        filters.set(FILTER_NAME.to_vec(), Object::Dict(filter));
        let mut encrypt = Dict::new();
        encrypt.set(b"Filter".to_vec(), Object::name("Adobe.PubSec"));
        encrypt.set(b"SubFilter".to_vec(), Object::name("adbe.pkcs7.s5"));
        encrypt.set(b"V".to_vec(), Object::Int(dict.v));
        encrypt.set(b"Length".to_vec(), Object::Int(dict.length_bits));
        encrypt.set(b"CF".to_vec(), Object::Dict(filters));
        encrypt.set(b"StmF".to_vec(), Object::Name(dict.stm_f.clone()));
        encrypt.set(b"StrF".to_vec(), Object::Name(dict.str_f.clone()));
        if !dict.ef_f.is_empty() {
            encrypt.set(b"EFF".to_vec(), Object::Name(dict.ef_f.clone()));
        }
        // Reject a context whose descriptor would change during serialization. Validation
        // happens before touching the trailer or freeing any existing encryption object.
        if read(self, &encrypt)? != *handler.dict() {
            return Err(malformed("the writer context is not a canonical public-key descriptor"));
        }
        let has_id =
            self.trailer().get(b"ID").and_then(Object::as_array).is_some_and(|ids| ids.len() == 2 && ids.iter().all(|v| v.as_string().is_some()));
        if !has_id {
            self.trailer_mut().set(b"ID".to_vec(), Object::Array(vec![string(&file_id), string(&file_id)]));
        }
        if let Some(Object::Ref(old)) = self.trailer().get(b"Encrypt").cloned() {
            self.free(old);
        }
        let reference = self.add(encrypt);
        self.trailer_mut().set(b"Encrypt".to_vec(), Object::Ref(reference));
        self.set_public_output(handler, reference.num);
        Ok(())
    }
}
