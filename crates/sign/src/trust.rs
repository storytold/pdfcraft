//! Trust lists: sets of CA certificates the user loads from a file, kept apart from the
//! certificates they trust themselves.
//!
//! Nothing here is trusted unless the user loads it (`TrustStore::lists`): which authorities a PDF
//! reader vouches for is the user's decision, not ours, so no list is embedded in PdfCraft. The
//! first intended source is the EU Trusted Lists' qualified CA services. Chain building
//! ([`crate::x509::build_chain`]) still applies to a list: a certificate in it only vouches for what
//! a chain of CA certificates under it leads to.
//!
//! A list vouches for signers. It never vouches for a time-stamp authority: a CA that is trusted
//! to issue signing certificates is not thereby a source of trusted time, so trusted time still
//! comes only from the user's own certificates.
//!
//! Adobe's AATL is not used in any form: Adobe-authored data outside the closed list in AGENTS.md §1.1
//! needs the owner's explicit approval, and the EU Trusted Lists cover the same ground.

use std::sync::Arc;

use crate::SignError;
use crate::der::Tlv;
use crate::x509::Certificate;

/// Certificates as DER one after the other. A certificate that doesn't parse (a key type PdfCraft
/// doesn't read, say) is skipped. Bytes that aren't a DER element at all, as in a file cut short,
/// are an error, so a damaged list doesn't load as a smaller one without a word.
fn parse_concatenated(mut rest: &[u8]) -> Result<Vec<Certificate>, SignError> {
    let mut out = Vec::new();
    while !rest.iter().all(u8::is_ascii_whitespace) {
        let Ok((t, r)) = Tlv::parse(rest) else {
            return Err(SignError::Malformed(format!("the trust list is damaged after {} certificates ({} bytes left)", out.len(), rest.len())));
        };
        if let Ok(c) = Certificate::parse(t.raw) {
            out.push(c);
        }
        rest = r;
    }
    Ok(out)
}

/// The most a list file may hold: far above the roughly 1.5 MB of the EU qualified CAs.
pub const MAX_LIST_BYTES: usize = 32 << 20;

/// A named set of trusted CA certificates the user loaded from a file, e.g. "EU Trusted List".
#[derive(Clone, Debug)]
pub struct TrustList {
    pub name: String,
    pub certs: Arc<Vec<Certificate>>,
}

impl TrustList {
    /// A list from a file's bytes: certificates as DER one after the other, or a PEM bundle.
    /// Certificates that don't parse are skipped; a file with none, a damaged one, or one over
    /// [`MAX_LIST_BYTES`], is an error.
    pub fn from_bytes(name: &str, bytes: &[u8]) -> Result<TrustList, SignError> {
        if bytes.len() > MAX_LIST_BYTES {
            return Err(SignError::Malformed(format!("trust list over {} MiB", MAX_LIST_BYTES >> 20)));
        }
        let mut certs = if bytes.first() == Some(&0x30) && !crate::x509::is_pem(bytes) {
            parse_concatenated(bytes)?
        } else {
            crate::x509::load_certificates(bytes)?
        };
        let mut seen = std::collections::HashSet::new();
        certs.retain(|c| seen.insert(c.raw.clone()));
        if certs.is_empty() {
            return Err(SignError::Malformed("no certificates in the trust list".into()));
        }
        Ok(TrustList { name: name.to_string(), certs: Arc::new(certs) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A few self-made certificates to load: the lists are tested without any real CA's data.
    fn certificates(n: usize) -> Vec<Vec<u8>> {
        (0..n)
            .map(|i| {
                let key = crate::PrivateKey::generate_p256().unwrap();
                let name = crate::x509::Name::build(&format!("List Test {i}"), "", "Trust Tests", "", "US");
                let alg = key.preferred_digest();
                let sig_alg = key.signature_algorithm(alg);
                let tbs = crate::der::seq(&[
                    &crate::der::uint(&[1 + i as u8]),
                    &sig_alg,
                    &name.raw,
                    &crate::der::seq(&[
                        &crate::der::Time { year: 2020, month: 1, day: 1, hour: 0, minute: 0, second: 0 }.encode(),
                        &crate::der::Time { year: 2040, month: 1, day: 1, hour: 0, minute: 0, second: 0 }.encode(),
                    ]),
                    &name.raw,
                    &key.public_key().spki(),
                ]);
                let signature = key.sign(alg, &tbs).unwrap();
                crate::der::seq(&[&tbs, &sig_alg, &crate::der::bit_string(&signature)])
            })
            .collect()
    }

    #[test]
    fn a_list_reads_concatenated_der_and_rejects_garbage() {
        let certs = certificates(3);
        let file = certs.concat();
        let list = TrustList::from_bytes("Test list", &file).unwrap();
        assert_eq!((list.name.as_str(), list.certs.len()), ("Test list", 3));
        // The same as a PEM bundle.
        let pem: String = list.certs.iter().map(crate::x509::to_pem).collect();
        assert_eq!(TrustList::from_bytes("pem", pem.as_bytes()).unwrap().certs.len(), 3);
        assert!(TrustList::from_bytes("none", b"not certificates").is_err());
        assert!(TrustList::from_bytes("none", &[]).is_err());
        assert!(TrustList::from_bytes("big", &vec![0x30; MAX_LIST_BYTES + 1]).is_err());
        // A file cut short is refused, not loaded as the certificates before the cut.
        assert!(TrustList::from_bytes("cut", &file[..file.len() - 5]).is_err());
        // Whitespace after the last certificate is not damage.
        let spaced = [file.as_slice(), b"\n"].concat();
        assert_eq!(TrustList::from_bytes("spaced", &spaced).unwrap().certs.len(), 3);
        // PEM that follows a note starting with "0" (the first byte DER starts with) is still PEM.
        let noted = format!("0 certificates have been revoked in this bundle\n{pem}");
        assert_eq!(TrustList::from_bytes("noted", noted.as_bytes()).unwrap().certs.len(), 3);
        // A certificate listed twice, even apart, is one entry.
        let repeated = [certs[0].clone(), certs[1].clone(), certs[0].clone()].concat();
        assert_eq!(TrustList::from_bytes("dups", &repeated).unwrap().certs.len(), 2);
    }
}
