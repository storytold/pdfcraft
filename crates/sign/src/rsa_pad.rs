//! RSA signature checks with the padding done here: the raw public-key operation comes from the
//! `rsa` crate, and the EMSA-PKCS1-v1_5 / EMSA-PSS encodings (RFC 8017 §9) are checked against
//! any digest, any MGF1 hash and any salt length, which the `rsa` crate's typed API can't do.
//! Verification only; public data, so nothing here needs to be constant-time.

use rsa::{BoxedUint, PublicKey};

use crate::SignError;
use crate::der;
use crate::keys::{DigestAlg, PssParams};

/// The encoded message `sig^e mod n` as exactly `n.len()` bytes, or `None` if the signature is
/// out of range. `n` is the modulus without leading zeros. `notes` gets an entry when the
/// signature value was not written with exactly the modulus' length (RFC 8017 §8.2.2 wants
/// that); some signers drop or add leading zero bytes.
fn encoded_message(n: &[u8], e: &[u8], sig: &[u8], notes: &mut Vec<&'static str>) -> Result<Option<Vec<u8>>, SignError> {
    // Redundant leading zeros would size the big-integer arithmetic (and the backend's own size
    // limit) by the encoding rather than the number, so they are dropped before anything is built.
    let (n, e) = (trim_zeros(n), trim_zeros(e));
    let k = n.len();
    // `PublicKey::new` refuses a modulus under 1024 bits, which documents signed years ago still
    // carry. Those signatures are checked as before (up to the crate's 8192-bit cap); the
    // certificate's key reports itself weak below 2048 bits (`keys::PublicKey::weakness`).
    let key = PublicKey::new_with_size_limits(BoxedUint::from_be_slice_vartime(n), BoxedUint::from_be_slice_vartime(e), 1..=PublicKey::MAX_SIZE)
        .map_err(|e| SignError::Malformed(format!("RSA key: {e}")))?;
    if sig.len() != k {
        notes
            .push("The RSA signature value does not have the length of the key's modulus (leading zero bytes dropped or added); it was read anyway.");
    }
    let sig = match sig.iter().position(|b| *b != 0) {
        Some(i) => sig.get(i..).unwrap_or_default(),
        None => return Ok(None),
    };
    // RFC 8017 §5.2.2: the signature representative must be below the modulus. Otherwise s and
    // s + n are the same number modulo n and both would verify. Both are big-endian without
    // leading zeros here, so a longer one is larger and equal lengths compare byte by byte.
    let modulus = n.get(n.iter().position(|b| *b != 0).unwrap_or(n.len())..).unwrap_or_default();
    if sig.len() > modulus.len() || (sig.len() == modulus.len() && sig >= modulus) {
        return Ok(None);
    }
    let Ok(em) = rsa::hazmat::rsa_encrypt(&key, &BoxedUint::from_be_slice_vartime(sig)) else { return Ok(None) };
    let bytes = em.to_be_bytes();
    let mut out = vec![0u8; k.saturating_sub(bytes.len())];
    let skip = bytes.len().saturating_sub(k);
    out.extend_from_slice(bytes.get(skip..).unwrap_or_default());
    Ok(Some(out))
}

fn trim_zeros(b: &[u8]) -> &[u8] {
    b.get(b.iter().position(|x| *x != 0).unwrap_or(b.len())..).unwrap_or_default()
}

/// `DigestInfo ::= SEQUENCE { AlgorithmIdentifier, OCTET STRING }`.
fn digest_info(alg: DigestAlg, digest: &[u8], null_params: bool) -> Vec<u8> {
    let id = der::algorithm(alg.oid(), null_params.then(der::null).as_deref());
    der::seq(&[&id, &der::octets(digest)])
}

/// RSASSA-PKCS1-v1_5 (RFC 8017 §8.2.2): the signature is checked against the one encoding the
/// message digest should have, built here and compared byte for byte, so there is nothing for a
/// forger to put in trailing or padding bytes. Two `DigestInfo` encodings are accepted: with the
/// NULL parameter (the standard one) and without it, which older signers write (RFC 8017 App. B.1
/// has verifiers accept both); the second is noted in `notes`. A signature over the bare digest,
/// without any `DigestInfo`, is not accepted: it does not say which hash algorithm it uses.
pub fn pkcs1_verify(n: &[u8], e: &[u8], alg: DigestAlg, digest: &[u8], sig: &[u8], notes: &mut Vec<&'static str>) -> Result<bool, SignError> {
    let Some(em) = encoded_message(n, e, sig, notes)? else { return Ok(false) };
    let k = em.len();
    let forms = [
        (digest_info(alg, digest, true), None),
        (
            digest_info(alg, digest, false),
            Some("The RSA signature's DigestInfo has no NULL parameter (older signers write it so); it was read anyway."),
        ),
    ];
    for (t, note) in forms {
        let Some(ps) = k.checked_sub(t.len() + 3).filter(|ps| *ps >= 8) else { continue };
        let mut expected = vec![0x00, 0x01];
        expected.extend(std::iter::repeat_n(0xFF, ps));
        expected.push(0x00);
        expected.extend_from_slice(&t);
        if expected == em {
            notes.extend(note);
            return Ok(true);
        }
    }
    Ok(false)
}

/// MGF1 (RFC 8017 §B.2.1).
fn mgf1(hash: DigestAlg, seed: &[u8], len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + hash.output_len());
    let mut counter = 0u32;
    while out.len() < len {
        out.extend(hash.digest(&[seed, &counter.to_be_bytes()]));
        counter = counter.wrapping_add(1);
    }
    out.truncate(len);
    out
}

/// EMSA-PSS-VERIFY (RFC 8017 §9.1.2) with `digest` (of the message, with `alg`) as `mHash`.
pub fn pss_verify(
    n: &[u8],
    e: &[u8],
    alg: DigestAlg,
    params: PssParams,
    digest: &[u8],
    sig: &[u8],
    notes: &mut Vec<&'static str>,
) -> Result<bool, SignError> {
    let Some(raw) = encoded_message(n, e, sig, notes)? else { return Ok(false) };
    let n = trim_zeros(n);
    let mod_bits = n.first().map_or(0, |b| (n.len() * 8) as u32 - b.leading_zeros());
    let Some(em_bits) = mod_bits.checked_sub(1) else { return Ok(false) };
    let em_len = em_bits.div_ceil(8) as usize;
    // emLen is k, or k - 1 when the modulus is 8m+1 bits (then the top byte must be zero).
    let Some(lead) = raw.len().checked_sub(em_len) else { return Ok(false) };
    if raw.get(..lead).is_none_or(|l| l.iter().any(|b| *b != 0)) {
        return Ok(false);
    }
    let em = raw.get(lead..).unwrap_or_default();
    let h_len = alg.output_len();
    if digest.len() != h_len {
        return Ok(false);
    }
    let Some(db_len) = em_len.checked_sub(h_len + 1).filter(|l| *l >= 1) else { return Ok(false) };
    if em.last() != Some(&0xBC) {
        return Ok(false);
    }
    let Some((masked_db, rest)) = em.split_at_checked(db_len) else { return Ok(false) };
    let Some(h) = rest.get(..h_len) else { return Ok(false) };
    let spare = (8 * em_len as u32).saturating_sub(em_bits);
    let top_mask = 0xFFu8.checked_shr(spare).unwrap_or(0);
    if masked_db.first().is_none_or(|b| b & !top_mask != 0) {
        return Ok(false);
    }
    let mut db: Vec<u8> = masked_db.iter().zip(mgf1(params.mgf, h, db_len)).map(|(a, b)| a ^ b).collect();
    if let Some(b) = db.first_mut() {
        *b &= top_mask;
    }
    let Some(sep) = db.iter().position(|b| *b != 0) else { return Ok(false) };
    if db.get(sep) != Some(&1) {
        return Ok(false);
    }
    let salt = db.get(sep + 1..).unwrap_or_default();
    if params.salt_len.is_some_and(|l| l != salt.len()) {
        return Ok(false);
    }
    Ok(alg.digest(&[&[0u8; 8], digest, salt]) == h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).filter_map(|i| s.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect()
    }

    /// A 512-bit key, as old documents carry: `rsa` 0.10 refuses it in `PublicKey::new`, but its
    /// signatures still verify here (the key is reported weak elsewhere). Made with
    /// `openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:512` and
    /// `openssl dgst -sha256 -sign` over the bytes `legacy key`.
    #[test]
    fn a_key_under_1024_bits_still_verifies() {
        let n =
            hex("F8AD355D80DF13A88CA540E6DE3F67F253692A7D6BE4ED7965DB3E4D73F1A2CBC3C81E983E3913FADB4DB44A6DA8A43DDB296862CB3173979B5D0EC4479D0139");
        let sig =
            hex("8dc7452df5b6a294a2094e0eda60fef22dcbfa605d1390cb1241ab049260edaefc10c415a89a91c83984af90ee2df3723028ed3c221c41db8fb6cd7fde20bd45");
        let digest = hex("db8539851b985a88a95f53b3ecec0001ef24ab2ca1cb00387e7ac514a8624673");
        let e = [1, 0, 1];
        let mut notes = Vec::new();
        assert_eq!(pkcs1_verify(&n, &e, DigestAlg::Sha256, &digest, &sig, &mut notes).ok(), Some(true));
        assert!(notes.is_empty(), "{notes:?}");
        let mut other = digest.clone();
        if let Some(b) = other.first_mut() {
            *b ^= 1;
        }
        assert_eq!(pkcs1_verify(&n, &e, DigestAlg::Sha256, &other, &sig, &mut notes).ok(), Some(false));
    }
}
