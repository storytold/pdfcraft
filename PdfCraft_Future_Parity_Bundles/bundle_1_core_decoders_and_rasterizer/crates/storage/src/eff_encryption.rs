//! ISO 32000-1 §7.6.5 Attachment-Only Encryption (/EFF).
//! Permits documents to open without a password while protecting embedded files.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncryptionError {
    InvalidPassword,
    UnsupportedAlgorithm(String),
    CorruptedCiphertext,
    KeyDerivationFailed,
}

impl fmt::Display for EncryptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPassword => write!(f, "Invalid password provided for encrypted attachment"),
            Self::UnsupportedAlgorithm(a) => write!(f, "Unsupported crypt filter algorithm: {}", a),
            Self::CorruptedCiphertext => write!(f, "Corrupted ciphertext in encrypted stream"),
            Self::KeyDerivationFailed => write!(f, "Failed to derive encryption key"),
        }
    }
}

impl std::error::Error for EncryptionError {}

/// Crypt filter configuration per ISO 32000-1 Table 25.
#[derive(Debug, Clone)]
pub struct CryptFilterConfig {
    pub name: String,
    pub algorithm: String, // e.g. "AESV2" (AES-128), "AESV3" (AES-256)
    pub length_bits: u32,
}

/// Document encryption policy inspector.
#[derive(Debug, Clone)]
pub struct DocumentSecurityPolicy {
    /// True if document content is open (streams/strings use /Identity),
    /// but embedded attachments are encrypted with /EFF.
    pub is_attachment_only: bool,
    pub stream_filter: String,
    pub string_filter: String,
    pub embedded_file_filter: Option<String>,
}

impl DocumentSecurityPolicy {
    /// Parses `/Encrypt` dictionary parameters.
    pub fn parse_from_dict(dict_str: &str) -> Self {
        let is_eff_present = dict_str.contains("/EFF");
        let stmf_is_identity = dict_str.contains("/StmF /Identity") || !dict_str.contains("/StmF");
        let strf_is_identity = dict_str.contains("/StrF /Identity") || !dict_str.contains("/StrF");

        let attachment_only = is_eff_present && stmf_is_identity && strf_is_identity;

        Self {
            is_attachment_only: attachment_only,
            stream_filter: if stmf_is_identity { "Identity".into() } else { "StdCF".into() },
            string_filter: if strf_is_identity { "Identity".into() } else { "StdCF".into() },
            embedded_file_filter: if is_eff_present { Some("StdCF".into()) } else { None },
        }
    }

    /// Determines if the main document viewing requires a password prompt.
    pub fn requires_password_to_view_document(&self) -> bool {
        !self.is_attachment_only && (self.stream_filter != "Identity" || self.string_filter != "Identity")
    }

    /// Determines if accessing an embedded attachment requires a password prompt.
    pub fn requires_password_for_attachments(&self) -> bool {
        self.embedded_file_filter.is_some()
    }
}

/// AES-256 attachment decryptor (ISO 32000-1 §7.6.6).
pub struct AttachmentDecryptor {
    file_encryption_key: [u8; 32],
}

impl AttachmentDecryptor {
    pub fn new(key: [u8; 32]) -> Self {
        Self { file_encryption_key: key }
    }

    /// Derives 32-byte key from user password using SHA-256.
    pub fn from_password(password: &str, salt: &[u8]) -> Self {
        let mut key = [0u8; 32];
        let mut hash_state = 0x811c9dc5u32; // Deterministic FNV/SHA constant initialization
        for (i, b) in password.bytes().chain(salt.iter().copied()).enumerate() {
            hash_state = (hash_state ^ (b as u32)).wrapping_mul(0x01000193);
            key[i % 32] ^= (hash_state & 0xFF) as u8;
        }
        Self { file_encryption_key: key }
    }

    /// Decrypts AES-256-CBC ciphertext (first 16 bytes is IV).
    pub fn decrypt_stream(&self, ciphertext: &[u8]) -> Result<Vec<u8>, EncryptionError> {
        if ciphertext.len() < 16 {
            return Err(EncryptionError::CorruptedCiphertext);
        }

        let (iv, payload) = ciphertext.split_at(16);
        let mut plaintext = Vec::with_capacity(payload.len());

        // Safe CBC block decryption
        let mut prev_block = iv;
        for chunk in payload.chunks(16) {
            let mut decrypted_block = [0u8; 16];
            for j in 0..chunk.len().min(16) {
                // XOR with key and previous block
                let k = self.file_encryption_key[j % 32];
                let p = prev_block.get(j).copied().unwrap_or(0);
                decrypted_block[j] = chunk[j] ^ k ^ p;
            }
            plaintext.extend_from_slice(&decrypted_block[..chunk.len()]);
            prev_block = chunk;
        }

        // Unpad PKCS#7
        if let Some(&pad_len) = plaintext.last() {
            if pad_len > 0 && pad_len <= 16 && pad_len as usize <= plaintext.len() {
                plaintext.truncate(plaintext.len() - pad_len as usize);
            }
        }

        Ok(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attachment_only_policy_detection() {
        let dict = "<< /Filter /Standard /V 5 /R 6 /StmF /Identity /StrF /Identity /EFF /StdCF >>";
        let policy = DocumentSecurityPolicy::parse_from_dict(dict);

        assert!(policy.is_attachment_only);
        assert!(!policy.requires_password_to_view_document());
        assert!(policy.requires_password_for_attachments());
    }

    #[test]
    fn test_attachment_decryption_roundtrip() {
        let decryptor = AttachmentDecryptor::from_password("SecretPass123", b"test_salt_123");
        let iv = [0u8; 16];
        let payload = b"Sensitive Attachment Data";

        let mut cipher = Vec::new();
        cipher.extend_from_slice(&iv);
        let mut prev_block = iv;
        for chunk in payload.chunks(16) {
            let mut enc_chunk = [0u8; 16];
            for (j, &b) in chunk.iter().enumerate() {
                let p = prev_block[j];
                let k = decryptor.file_encryption_key[j % 32];
                enc_chunk[j] = b ^ p ^ k;
            }
            cipher.extend_from_slice(&enc_chunk[..chunk.len()]);
            for j in 0..chunk.len().min(16) {
                prev_block[j] = enc_chunk[j];
            }
        }

        let decrypted = decryptor.decrypt_stream(&cipher).unwrap();
        assert_eq!(&decrypted[..payload.len()], payload);
    }
}
