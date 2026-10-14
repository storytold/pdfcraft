//! CAdES envelope files (`.pdf.p7m`, `.p7s`; issue #1449): a CMS SignedData (RFC 5652) that
//! either wraps the whole document (attached, what a `.pdf.p7m` is) or sits beside it (detached,
//! a `.p7s`), signed per CAdES (ETSI EN 319 122). Opening one means classifying the file by its
//! first bytes ([`is_cms_envelope`]) and reading what it carries ([`read`]): the document, who
//! signed it, and whether the signature still matches the content.

use crate::SignError;
use crate::cms::SignedData;
use crate::der::{Time, Tlv, tag};

/// The content octets of the SignedData OID (1.2.840.113549.1.7.2), for the sniff.
const SIGNED_DATA_OID: [u8; 9] = [0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x02];

/// What [`read`] reports when the envelope does not carry the signer's certificate.
const NO_CERTIFICATE: &str = "the signer's certificate was not carried";

/// An envelope read: the document it wraps, who signed it, and whether the signature still
/// covers the content as it is now.
pub struct Envelope {
    /// The encapsulated content; `None` is a detached signature, whose document sits outside
    /// the envelope.
    pub content: Option<Vec<u8>>,
    /// The signer's name as their certificate shows it, or a word about its absence.
    pub signer_name: String,
    /// When the signer says they signed (the signing-time attribute), if they said.
    pub signing_time: Option<Time>,
    /// Whether the signature value still matches the content read here (`false` for a detached
    /// signature, whose content is not part of the envelope).
    pub digest_ok: bool,
    /// How many signers the envelope carries (the children of the signerInfos SET, RFC 5652 §5.1).
    pub signers: usize,
    /// Encoding irregularities that were tolerated on the way, for the details.
    pub quirks: Vec<&'static str>,
}

/// Whether `bytes` begin like a CMS SignedData object: a SEQUENCE, whatever length header the
/// encoder chose (short form, long form of up to four octets, or BER's indefinite form), then
/// the SignedData OID. A look at the first bytes only — no parsing, no allocation — so any
/// surprise is "no".
pub fn is_cms_envelope(bytes: &[u8]) -> bool {
    let Some((&tag::SEQUENCE, rest)) = bytes.split_first() else { return false };
    let Some((&len, rest)) = rest.split_first() else { return false };
    // How many length octets follow: none in the short form, 1–4 in the long form, and BER's
    // indefinite form is the 0x80 marker itself (its end-of-contents octets come later, and
    // the content — which is what we look at next — starts right here).
    let skip = match len {
        0x80 => 0,
        n @ 0x81..=0x84 => (n & 0x7F) as usize,
        0x00..=0x7F => 0,
        _ => return false,
    };
    let Some(rest) = rest.get(skip..) else { return false };
    let Some((&tag::OID, rest)) = rest.split_first() else { return false };
    let Some((&9, rest)) = rest.split_first() else { return false };
    rest.starts_with(&SIGNED_DATA_OID)
}

/// Read a `.p7m` or `.p7s` envelope: its content, its signer, and whether the signature still
/// checks against the content it carries. Bytes after the object (a PDF placeholder's zero
/// padding) are ignored, as [`SignedData::parse`] does.
pub fn read(bytes: &[u8]) -> Result<Envelope, SignError> {
    let sd = SignedData::parse(bytes)?;
    let cert = sd.signer_certificate();
    let signer_name = cert.map_or_else(|| NO_CERTIFICATE.to_string(), |c| c.display_name());
    let digest_ok = match cert.zip(sd.content.as_deref()) {
        Some((cert, content)) => {
            // The signature commits to a digest of the content. With signed attributes —
            // CAdES always — what is signed is the message-digest attribute rather than the
            // content, so the attribute is what ties the content to the signature: it is
            // compared first (as the PDF validator does), and the signature over it is
            // checked as well. An algorithm that cannot be checked here is "not verified".
            let digest = sd.signer.digest.digest(&[content]);
            sd.signer.message_digest.as_ref().is_none_or(|md| md == &digest) && matches!(sd.verify_signature(cert, &digest), Ok(true))
        }
        // Detached (nothing carried to check against), or no certificate to check with.
        None => false,
    };
    Ok(Envelope {
        content: sd.content,
        signer_name,
        signing_time: sd.signer.signing_time,
        digest_ok,
        signers: signerinfos_count(bytes)?,
        quirks: sd.quirks,
    })
}

/// How many SignerInfos the envelope carries: the children of the signerInfos SET, the last
/// field of SignedData (RFC 5652 §5.1), found the way [`SignedData::parse`] walks the object —
/// from the end, so the SET OF digest algorithms before it is skipped.
fn signerinfos_count(bytes: &[u8]) -> Result<usize, SignError> {
    let (ci, _) = Tlv::parse_ber(bytes)?;
    let ci = ci.expect(tag::SEQUENCE, "ContentInfo")?.children()?;
    let [_, content] = ci.as_slice() else { return Err(bad("ContentInfo")) };
    let signed_data = content.expect(tag::ctx(0), "content")?.inner()?.children()?;
    let set = signed_data.iter().rev().find(|t| t.tag == tag::SET).ok_or_else(|| bad("no signerInfos"))?;
    Ok(set.children()?.len())
}

fn bad(what: &str) -> SignError {
    SignError::Malformed(format!("p7m: {what}"))
}
