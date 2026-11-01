//! Signatures in a PDF (ISO 32000-2 §12.8): listing and validating signature fields, and
//! signing with an incremental save.
//!
//! **Validation** reads `/ByteRange` and the CMS from the file's own bytes (not from the parsed
//! string, which a security handler would have transformed), recomputes the digest, checks the
//! signature and the signer's chain against the trust store, and diffs later revisions against
//! the signed one to classify changes made after signing (form fill, comments, further
//! signatures, or anything else) under the DocMDP permissions.
//!
//! **Signing** adds or fills a signature field, writes the document incrementally with a
//! zero-filled `/Contents` and a fixed-width `/ByteRange`, then patches both in place: the
//! PAdES B-B recipe (`ETSI.CAdES.detached`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, SaveOptions, Stream};

use crate::SignError;
use crate::cms::SignedData;
use crate::der::Time;
use crate::keys::DigestAlg;
use crate::pkcs12::DigitalId;
use crate::x509::{Certificate, build_chain, build_chain_noted};

mod changes;
mod listing;
mod signing;
mod types;
mod validate;

#[cfg(test)]
use changes::only_grew;
#[cfg(test)]
use listing::{DISCOVERY_CACHE_LIMIT, is_document_timestamp};
pub use listing::{DigestCache, list, list_cached};
pub(crate) use signing::fmt;
pub use signing::{Appearance, AppearanceLabels, SignOptions, display_date, sign, sign_with_timestamp, timestamp_document};
pub use types::{Detail, Fill, Modification, Status, TrustStore, WORDINGS, Words, in_english, kind};
pub(crate) use validate::signature_contents;
pub use validate::validate;
#[cfg(test)]
use validate::{DssEvidence, MAX_DSS_BYTES, MAX_DSS_ENTRIES};

#[cfg(test)]
#[path = "pdf_discovery_tests.rs"]
mod discovery_tests;

/// One signature field.
#[derive(Clone, Debug)]
pub struct SignatureInfo {
    /// Fully qualified field name.
    pub field: String,
    pub signed: bool,
    /// 0-based page of its widget, and the widget rectangle (user space).
    pub page: Option<usize>,
    pub rect: Option<[f64; 4]>,
    pub visible: bool,
    /// The signer: the certificate's common name, else `/Name`.
    pub signer: Option<String>,
    pub certificate: Option<Certificate>,
    /// The chain as built (signer first).
    pub chain: Vec<Certificate>,
    /// `/M` as written, and the signing time used for checks.
    pub date: Option<String>,
    pub signing_time: Option<Time>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact: Option<String>,
    pub sub_filter: Option<String>,
    /// True for a standalone document timestamp (`/ETSI.RFC3161`), not a field signature.
    pub doc_timestamp: bool,
    /// DocMDP permissions when this is a certification signature (1–3).
    pub certify: Option<u8>,
    /// 1-based revision the signature covers.
    pub revision: usize,
    /// Length of the signed revision (bytes) — "View signed version".
    pub signed_len: usize,
    /// Digest algorithm used for the document's signed bytes.
    pub digest: Option<DigestAlg>,
    pub algorithm: Option<String>,
    pub timestamp: bool,
    /// RFC 3161 generation time, when the token was validated and (for a signature's embedded
    /// token) its authority chains to the trust store. An untrusted token's time is never
    /// used as the validation time.
    pub timestamp_time: Option<Time>,
    pub status: Status,
    pub modification: Modification,
    /// Acrobat-style sentences explaining the verdict, each one ready to read in English and ready to
    /// be said in another language (see [`Detail`]).
    pub details: Vec<Detail>,
}

impl SignatureInfo {
    /// The one-line summary under the signature in the Signatures panel, in English and listed in
    /// [`WORDINGS`] for an interface to say in its own language.
    pub fn summary(&self) -> &'static str {
        if !self.signed {
            return "Unsigned signature field";
        }
        match self.status {
            Status::Valid => "Signature is valid",
            Status::Unknown => "Signature validity is unknown",
            Status::Invalid => "Signature is invalid",
        }
    }
}
