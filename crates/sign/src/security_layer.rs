//! Austrian Security Layer 1.2 CMS messages, independent of browser or network code.
//!
//! Sources: Core §2.1 and Core-1.2.xsd at
//! https://sl.eid.egiz.gv.at/konzept/securitylayer/spezifikation/20140114-en/ .
//! A-Trust requires its authentication pages to be displayed in a real browser:
//! https://docs.a-trust.at/apis/SecurityLayer/ . This module never reads login pages.

use base64::{Engine as _, prelude::BASE64_STANDARD};

use crate::{PreparedSignature, SignError};

/// Namespace shared by the original and provider-specific Security Layer interfaces.
pub const NAMESPACE: &str = "http://www.buergerkarte.at/namespaces/securitylayer/1.2#";
/// Maximum prepared PDF transferred by this integration.
pub const MAX_DOCUMENT_BYTES: usize = 100 * 1024 * 1024;
/// Maximum XML response, before decoding its CMS payload.
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

/// A detached CMS request containing the exact prepared PDF. The exclusion's end in
/// Security Layer is inclusive and zero-based; a Rust range's end is exclusive.
pub fn cms_request(prepared: &PreparedSignature) -> Result<Vec<u8>, SignError> {
    if prepared.bytes().len() > MAX_DOCUMENT_BYTES {
        return Err(SignError::Unsupported("ID Austria documents are limited to 100 MiB".into()));
    }
    let range = prepared.excluded_range();
    let to = range.end.checked_sub(1).filter(|end| *end >= range.start).ok_or_else(|| SignError::Malformed("empty signature exclusion".into()))?;
    let content = BASE64_STANDARD.encode(prepared.bytes());
    Ok(format!(
        "<sl:CreateCMSSignatureRequest xmlns:sl=\"{NAMESPACE}\" Structure=\"detached\">\
         <sl:KeyboxIdentifier>SecureSignatureKeypair</sl:KeyboxIdentifier>\
         <sl:DataObject><sl:MetaInfo><sl:MimeType>application/pdf</sl:MimeType></sl:MetaInfo>\
         <sl:Content><sl:Base64Content>{content}</sl:Base64Content></sl:Content>\
         <sl:ExcludedByteRange><sl:From>{}</sl:From><sl:To>{to}</sl:To></sl:ExcludedByteRange>\
         </sl:DataObject></sl:CreateCMSSignatureRequest>",
        range.start
    )
    .into_bytes())
}

/// Extract exactly one CMS payload or a numeric, non-sensitive provider error. No DTD,
/// external entity, stylesheet, reference resolution or raw provider error text is accepted.
pub fn cms_response(xml: &[u8]) -> Result<Vec<u8>, SignError> {
    if xml.len() > MAX_RESPONSE_BYTES {
        return Err(SignError::Malformed("Security Layer response exceeds 2 MiB".into()));
    }
    let xml = std::str::from_utf8(xml).map_err(|_| SignError::Malformed("Security Layer response must be UTF-8 XML".into()))?;
    let options = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: 8192, ..Default::default() };
    let doc =
        roxmltree::Document::parse_with_options(xml, options).map_err(|_| SignError::Malformed("invalid Security Layer XML response".into()))?;
    let root = doc.root_element();
    if root.tag_name().namespace() != Some(NAMESPACE) {
        return Err(SignError::Malformed("unexpected Security Layer response namespace".into()));
    }
    if root.tag_name().name() == "ErrorResponse" {
        let code = root.children().find(|n| n.has_tag_name((NAMESPACE, "ErrorCode"))).and_then(|n| n.text()).and_then(|s| s.parse::<u32>().ok());
        return Err(SignError::Crypto(match code {
            Some(code) => format!("the provider returned Security Layer error {code}; no signed file was saved"),
            None => "the provider returned a Security Layer error; no signed file was saved".into(),
        }));
    }
    let fields: Vec<_> = root.children().filter(roxmltree::Node::is_element).collect();
    let [cms] = fields.as_slice() else { return Err(SignError::Malformed("expected exactly one Security Layer CMS signature".into())) };
    if root.tag_name().name() != "CreateCMSSignatureResponse"
        || !cms.has_tag_name((NAMESPACE, "CMSSignature"))
        || cms.children().any(|n| n.is_element())
    {
        return Err(SignError::Malformed("unexpected Security Layer signature response".into()));
    }
    let encoded: String = cms.children().filter_map(|n| n.text()).flat_map(str::chars).filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = BASE64_STANDARD.decode(encoded).map_err(|_| SignError::Malformed("invalid Security Layer CMS encoding".into()))?;
    if bytes.is_empty() || bytes.len() > 1024 * 1024 {
        return Err(SignError::Malformed("the returned CMS must contain between 1 byte and 1 MiB".into()));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_namespaced_response_with_wrapped_base64() {
        let xml = format!("<CreateCMSSignatureResponse xmlns=\"{NAMESPACE}\"><CMSSignature>\n AQID \n</CMSSignature></CreateCMSSignatureResponse>");
        assert_eq!(cms_response(xml.as_bytes()).unwrap(), [1, 2, 3]);
    }

    #[test]
    fn provider_errors_never_echo_their_info_field() {
        let xml = format!("<ErrorResponse xmlns=\"{NAMESPACE}\"><ErrorCode>4501</ErrorCode><Info>secret-cookie-password</Info></ErrorResponse>");
        let error = cms_response(xml.as_bytes()).unwrap_err().to_string();
        assert!(error.contains("4501"));
        assert!(!error.contains("secret"));
    }

    #[test]
    fn rejects_entities_wrong_namespaces_duplicates_and_nested_payloads() {
        for xml in [
            format!("<!DOCTYPE x [<!ENTITY leak SYSTEM 'file:///private'>]><ErrorResponse xmlns=\"{NAMESPACE}\">&leak;</ErrorResponse>"),
            "<CreateCMSSignatureResponse><CMSSignature>AQID</CMSSignature></CreateCMSSignatureResponse>".into(),
            format!(
                "<CreateCMSSignatureResponse xmlns=\"{NAMESPACE}\"><CMSSignature>AQID</CMSSignature><CMSSignature>AQID</CMSSignature></CreateCMSSignatureResponse>"
            ),
            format!(
                "<CreateCMSSignatureResponse xmlns=\"{NAMESPACE}\"><CMSSignature><wrapped>AQID</wrapped></CMSSignature></CreateCMSSignatureResponse>"
            ),
            format!("<CreateCMSSignatureResponse xmlns=\"{NAMESPACE}\"><CMSSignature>!</CMSSignature></CreateCMSSignatureResponse>"),
        ] {
            assert!(cms_response(xml.as_bytes()).is_err());
        }
    }
}
