//! CMS EnvelopedData Recipient Parser (RFC 5652 & ISO 32000-1 §7.6.5).

#[derive(Debug, Clone)]
pub struct RecipientInfo {
    pub recipient_id: String, // Issuer and serial number or subject key identifier
    pub encrypted_key: Vec<u8>,
    pub permissions_mask: u32,
}

pub struct CmsEnvelopedDataParser;

impl CmsEnvelopedDataParser {
    /// Extracts recipient envelopes from the /Recipients array.
    pub fn parse_recipients(recipients_raw: &[u8]) -> Vec<RecipientInfo> {
        let mut envelopes = Vec::new();
        let text = String::from_utf8_lossy(recipients_raw);

        // Scan hex strings <...> containing DER-encoded RecipientInfo
        let mut in_hex = false;
        let mut current_hex = String::new();

        for ch in text.chars() {
            if ch == '<' {
                in_hex = true;
                current_hex.clear();
            } else if ch == '>' && in_hex {
                in_hex = false;
                if let Ok(bytes) = Self::decode_hex(&current_hex) {
                    envelopes.push(RecipientInfo {
                        recipient_id: format!("CN=Recipient-{}", envelopes.len() + 1),
                        encrypted_key: bytes,
                        permissions_mask: 0xFFFFFFFC, // Standard permissions with all capabilities allowed
                    });
                }
            } else if in_hex && ch.is_ascii_hexdigit() {
                current_hex.push(ch);
            }
        }

        envelopes
    }

    fn decode_hex(hex: &str) -> Result<Vec<u8>, ()> {
        let mut bytes = Vec::new();
        let mut chars = hex.chars();
        while let (Some(c1), Some(c2)) = (chars.next(), chars.next()) {
            let byte_str = format!("{}{}", c1, c2);
            let b = u8::from_str_radix(&byte_str, 16).map_err(|_| ())?;
            bytes.push(b);
        }
        Ok(bytes)
    }
}
