//! The single document-filter profile of the PDF public-key handler (§7.6.5, s5).
//! CMS and private-key operations belong to the caller. This standalone module derives
//! object-encryption keys from the recovered seed and the exact, ordered recipient envelopes.

use crate::{Algorithm, Auth, CryptError, EncryptDict, Method, SecurityHandler};
use sha1::Sha1;
use sha2::{Digest, Sha256};

pub const FILTER_NAME: &[u8] = b"DefaultCryptFilter";
const MAX_RECIPIENTS: usize = 64;
const MAX_ENVELOPE: usize = 1024 * 1024;
const MAX_RECIPIENT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKeyParameters {
    pub sub_filter: Vec<u8>,
    /// Exact CMS bytes, in PDF array order. Re-encoding or sorting changes the file key.
    pub recipients: Vec<Vec<u8>>,
}

fn malformed(message: &str) -> CryptError {
    CryptError::Malformed(message.into())
}

fn validate(dict: &EncryptDict) -> Result<&PublicKeyParameters, CryptError> {
    let params = dict.public_key.as_ref().ok_or_else(|| malformed("missing recipient parameters"))?;
    if dict.filter != b"Adobe.PubSec" || params.sub_filter != b"adbe.pkcs7.s5" {
        return Err(CryptError::Unsupported("expected Adobe.PubSec / adbe.pkcs7.s5".into()));
    }
    let method = match (dict.v, dict.length_bits) {
        (4, 128) => Method::Aes128,
        (5, 256) => Method::Aes256,
        _ => return Err(CryptError::Unsupported("public-key security requires AES-128 V4 or AES-256 V5".into())),
    };
    if dict.crypt_filters.as_slice() != [(FILTER_NAME.to_vec(), method)]
        || dict.stm_f != FILTER_NAME
        || dict.str_f != FILTER_NAME
        || !(dict.ef_f.is_empty() || dict.ef_f == FILTER_NAME || dict.ef_f == b"Identity")
    {
        return Err(CryptError::Unsupported("public-key security currently supports one DefaultCryptFilter for document strings and streams".into()));
    }
    if params.recipients.is_empty() || params.recipients.len() > MAX_RECIPIENTS {
        return Err(malformed("expected 1 to 64 recipient envelopes"));
    }
    let mut total = 0usize;
    for recipient in &params.recipients {
        total = total.saturating_add(recipient.len());
        if recipient.is_empty() || recipient.len() > MAX_ENVELOPE || total > MAX_RECIPIENT_BYTES {
            return Err(malformed("recipient envelope size limit exceeded"));
        }
    }
    Ok(params)
}

/// Build the supported s5 encryption descriptor. Creating new documents uses AES-256;
/// AES-128 is also available for compatibility fixtures and reading existing documents.
pub fn dictionary(algorithm: Algorithm, recipients: Vec<Vec<u8>>, encrypt_metadata: bool) -> Result<EncryptDict, CryptError> {
    let (v, length_bits, method) = match algorithm {
        Algorithm::Aes128 => (4, 128, Method::Aes128),
        Algorithm::Aes256 => (5, 256, Method::Aes256),
        _ => return Err(CryptError::Unsupported("RC4 certificate security is not supported".into())),
    };
    let dict = EncryptDict {
        filter: b"Adobe.PubSec".to_vec(),
        public_key: Some(PublicKeyParameters { sub_filter: b"adbe.pkcs7.s5".to_vec(), recipients }),
        v,
        length_bits,
        encrypt_metadata,
        crypt_filters: vec![(FILTER_NAME.to_vec(), method)],
        stm_f: FILTER_NAME.to_vec(),
        str_f: FILTER_NAME.to_vec(),
        ..Default::default()
    };
    validate(&dict)?;
    Ok(dict)
}

impl SecurityHandler {
    /// Validate the supported public-key descriptor and its size limits before invoking a
    /// caller-owned private key or prompting for a PIN.
    pub fn validate_public_key(dict: &EncryptDict) -> Result<(), CryptError> {
        validate(dict).map(|_| ())
    }

    /// Authenticate a supported public-key descriptor using a seed and permission word
    /// recovered from one of its CMS recipient envelopes. No password or device is retained.
    pub fn open_public_key(dict: EncryptDict, seed: &[u8; 20], permissions: i32) -> Result<Self, CryptError> {
        let params = validate(&dict)?;
        fn derive<D: Digest>(seed: &[u8; 20], params: &PublicKeyParameters, metadata: bool) -> Vec<u8> {
            let mut hash = D::new();
            hash.update(seed);
            for recipient in &params.recipients {
                hash.update(recipient);
            }
            if !metadata {
                hash.update([0xff; 4]);
            }
            hash.finalize().to_vec()
        }
        let key = if dict.v == 5 {
            derive::<Sha256>(seed, params, dict.encrypt_metadata)
        } else {
            let mut bytes = derive::<Sha1>(seed, params, dict.encrypt_metadata);
            bytes.truncate(16);
            bytes
        };
        Ok(Self::new(dict, key, Auth::Recipient { permissions }, None))
    }

    /// Create a writer context from caller-built recipient envelopes and their shared seed.
    /// The creator knows the file key and retains full access in this session. Reopening with
    /// a recipient's key applies only that recipient's encrypted permission word.
    pub fn create_public_key(algorithm: Algorithm, recipients: Vec<Vec<u8>>, seed: &[u8; 20], encrypt_metadata: bool) -> Result<Self, CryptError> {
        let mut handler = Self::open_public_key(dictionary(algorithm, recipients, encrypt_metadata)?, seed, -1)?;
        handler.auth = Auth::Owner;
        Ok(handler)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn keys_match_sha_vectors_with_ordered_envelopes_and_metadata_suffix() {
        for (algorithm, metadata, expected) in [
            (Algorithm::Aes256, true, "90ef97f20a75383743b5f7f72183d30fb5f601559674dd657ddccf2a3ab3ac02"),
            (Algorithm::Aes256, false, "287bb9c0077c33577353c94ce179743e62fd7a60877be1dfd443081a6dc478b1"),
            (Algorithm::Aes128, true, "077914da2f6fed0c52878d6fd57673f5"),
            (Algorithm::Aes128, false, "4c490267628bf7910209605aa1b11353"),
        ] {
            let dict = dictionary(algorithm, vec![b"recipient-a".to_vec(), b"recipient-b".to_vec()], metadata).unwrap();
            let h = SecurityHandler::open_public_key(dict.clone(), &[7; 20], 1 << 5).unwrap();
            assert_eq!(hex(h.file_key()), expected);
            assert!(h.permissions().annotate());
            assert!(!h.permissions().copy());
            assert!(!h.permissions().owner);
            let mut reordered = dict;
            reordered.public_key.as_mut().unwrap().recipients.reverse();
            assert_ne!(h.file_key(), SecurityHandler::open_public_key(reordered, &[7; 20], 0).unwrap().file_key());
            assert!(!format!("{h:?}").contains(&format!("{:?}", h.file_key())));
        }
    }

    #[test]
    fn public_permissions_do_not_trust_plaintext_p_or_password_ownership() {
        let mut dict = dictionary(Algorithm::Aes256, vec![vec![1]], true).unwrap();
        dict.p = -1;
        let recipient = SecurityHandler::open_public_key(dict.clone(), &[7; 20], 0).unwrap();
        assert_eq!(recipient.auth(), Auth::Recipient { permissions: 0 });
        assert!(!recipient.permissions().copy());
        assert!(recipient.authenticate_owner(b"id", "password").is_err());
        let control = SecurityHandler::open_public_key(dict, &[7; 20], 2).unwrap();
        assert!(control.permissions().owner);
        assert!(control.permissions().copy());
        assert!(control.permissions().modify());
    }

    #[test]
    fn invalid_profiles_and_recipient_sizes_are_rejected() {
        for recipients in [vec![], vec![vec![]], vec![vec![1]; 65], vec![vec![1; 1024 * 1024 + 1]], vec![vec![1; 1024 * 1024]; 5]] {
            assert!(dictionary(Algorithm::Aes256, recipients, true).is_err());
        }
        assert!(dictionary(Algorithm::Rc4_128, vec![vec![1]], true).is_err());
        let mut dict = dictionary(Algorithm::Aes256, vec![vec![1]], true).unwrap();
        dict.str_f = b"missing".to_vec();
        assert!(SecurityHandler::open_public_key(dict, &[7; 20], -1).is_err());
    }
}
