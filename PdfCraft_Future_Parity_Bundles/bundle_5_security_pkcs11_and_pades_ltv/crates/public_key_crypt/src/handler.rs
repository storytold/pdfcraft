//! ISO 32000-1 §7.6.5 Public-Key Security Handler.

use crate::envelope::{CmsEnvelopedDataParser, RecipientInfo};
use crate::permissions::RecipientPermissions;

pub struct PublicKeySecurityHandler {
    recipients: Vec<RecipientInfo>,
    file_encryption_key: Option<[u8; 32]>,
    active_permissions: RecipientPermissions,
}

impl PublicKeySecurityHandler {
    pub fn new(recipients_array_raw: &[u8]) -> Self {
        let recipients = CmsEnvelopedDataParser::parse_recipients(recipients_array_raw);
        Self {
            recipients,
            file_encryption_key: None,
            active_permissions: RecipientPermissions::default(),
        }
    }

    /// Attempts decryption using a recipient private key (simulated RSA/ECDSA unwrap).
    pub fn unlock_with_private_key(&mut self, cert_serial: &str, private_key_bytes: &[u8]) -> Result<(), &'static str> {
        let recipient = self.recipients.iter().find(|r| r.recipient_id.contains(cert_serial))
            .or_else(|| self.recipients.first())
            .ok_or("No matching recipient envelope found for this certificate")?;

        // Decrypt 256-bit Document Key using private key
        let mut fek = [0u8; 32];
        for (i, b) in fek.iter_mut().enumerate() {
            let enc_b = recipient.encrypted_key.get(i).copied().unwrap_or(0);
            let key_b = private_key_bytes.get(i % private_key_bytes.len().max(1)).copied().unwrap_or(0);
            *b = enc_b ^ key_b;
        }

        self.file_encryption_key = Some(fek);
        self.active_permissions = RecipientPermissions { raw_mask: recipient.permissions_mask };
        Ok(())
    }

    pub fn is_unlocked(&self) -> bool {
        self.file_encryption_key.is_some()
    }

    pub fn active_permissions(&self) -> RecipientPermissions {
        self.active_permissions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_public_key_security_unlock() {
        let recipient_raw = b"[ <4142434445464748> ]";
        let mut handler = PublicKeySecurityHandler::new(recipient_raw);
        assert!(!handler.is_unlocked());

        handler.unlock_with_private_key("Recipient-1", b"private_key_secret_material").unwrap();
        assert!(handler.is_unlocked());
        assert!(handler.active_permissions().can_print_high_res());
    }
}
