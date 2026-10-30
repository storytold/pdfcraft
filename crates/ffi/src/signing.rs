//! Cryptographic digital signatures (PAdES B-B, ISO 32000-2 §12.8, PKCS#12, X.509).

use crate::document::FfiRect;
use crate::edit::FfiPoint;

/// Details of an imported or generated digital identity certificate (X.509).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FfiDigitalIdInfo {
    pub friendly_name: Option<String>,
    pub common_name: String,
    pub organization: Option<String>,
    pub unit: Option<String>,
    pub email: Option<String>,
    pub country: Option<String>,
    pub serial_hex: String,
    pub sha256_fingerprint: String,
    pub not_before: String,
    pub not_after: String,
    pub key_algorithm: String,
}

/// A digital signature found in a document and its verification status (§12.8).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiDigitalSignature {
    pub field: String,
    pub signed: bool,
    pub status: String,
    pub summary: String,
    pub signer: Option<String>,
    pub date: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact: Option<String>,
    pub page: Option<u32>,
    pub rect: Option<FfiRect>,
    pub certify: Option<u8>,
    pub revision: u32,
    pub details: Vec<String>,
}

/// Options for digitally signing a PDF document.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiSignOptions {
    /// Field name if signing an existing signature field in the form.
    pub field_name: Option<String>,
    /// 0-based page index if creating a new signature field.
    pub page: Option<u32>,
    /// Bounding rectangle in view coordinates on `page` if creating a visible signature.
    pub rect: Option<FfiRect>,
    /// Reason for signing (e.g. "I approve this document").
    pub reason: Option<String>,
    /// Geographic location where signing occurred.
    pub location: Option<String>,
    /// Signer contact information.
    pub contact: Option<String>,
    /// DocMDP certification level: 1 = No changes, 2 = Form fill allowed, 3 = Comments allowed.
    pub certify_level: Option<u8>,
    /// Typed text name to render as a script calligraphy graphic.
    pub graphic_text: Option<String>,
    /// Freehand drawn ink strokes on the drawing pad to render as the signature graphic.
    pub graphic_strokes: Option<Vec<Vec<FfiPoint>>>,
    /// Target path to save the signed document to. If omitted, the session is updated in memory.
    pub out_path: Option<String>,
}

/// Result returned after digitally signing a document.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiSignResult {
    pub success: bool,
    pub bytes: Vec<u8>,
    pub saved_path: Option<String>,
    pub signer_name: String,
    pub field_name: String,
    pub revision: u32,
}

/// Helper to parse a `DigitalId` into `FfiDigitalIdInfo`.
#[allow(dead_code)]
pub(crate) fn parse_digital_id_info(id: &pdfcraft_engine::sign::DigitalId) -> FfiDigitalIdInfo {
    let cert = &id.certificate;
    FfiDigitalIdInfo {
        friendly_name: id.friendly_name.clone(),
        common_name: cert.display_name(),
        organization: cert.subject.organization().map(|s| s.to_string()),
        unit: cert.subject.unit().map(|s| s.to_string()),
        email: cert.subject.email().map(|s| s.to_string()),
        country: cert.subject.country().map(|s| s.to_string()),
        serial_hex: cert.serial_hex(),
        sha256_fingerprint: cert.fingerprint(),
        not_before: cert.not_before.to_string(),
        not_after: cert.not_after.to_string(),
        key_algorithm: cert.public_key.describe(),
    }
}

/// Helper to convert engine `SignatureInfo` to `FfiDigitalSignature`.
#[allow(dead_code)]
pub(crate) fn signature_info_to_ffi(s: &pdfcraft_engine::SignatureInfo, pages: &[pdfcraft_render::PageInfo]) -> FfiDigitalSignature {
    let status_str = if !s.signed {
        "unsigned"
    } else {
        match s.status {
            pdfcraft_engine::SignatureStatus::Valid => "valid",
            pdfcraft_engine::SignatureStatus::Unknown => "unknown",
            pdfcraft_engine::SignatureStatus::Invalid => "invalid",
        }
    };

    let page_u32 = s.page.map(|p| p as u32);
    let rect_ffi = s.rect.and_then(|r| {
        let page_idx = s.page?;
        let page = pages.get(page_idx)?;
        let [vx0, vy0] = page.user_to_view(r[0] as f32, r[1] as f32);
        let [vx1, vy1] = page.user_to_view(r[2] as f32, r[3] as f32);
        let min_x = vx0.min(vx1);
        let min_y = vy0.min(vy1);
        let width = (vx1 - vx0).abs();
        let height = (vy1 - vy0).abs();
        Some(FfiRect { x: min_x, y: min_y, width, height })
    });

    FfiDigitalSignature {
        field: s.field.clone(),
        signed: s.signed,
        status: status_str.to_string(),
        summary: s.summary().to_string(),
        signer: s.signer.clone(),
        date: s.date.clone(),
        reason: s.reason.clone(),
        location: s.location.clone(),
        contact: s.contact.clone(),
        page: page_u32,
        rect: rect_ffi,
        certify: s.certify,
        revision: s.revision as u32,
        details: s.details.iter().map(|d| d.message.clone()).collect(),
    }
}
