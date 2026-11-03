//! What the Windows certificate store (`src/windows.rs`, built only on Windows) may sign with,
//! checked on every platform: which certificate keys it lists, and how a CNG signature becomes
//! the one a CMS signature carries.

use p256::ecdsa::signature::hazmat::PrehashSigner;
use pdfcraft_sign::keys::{DigestAlg, Scheme, StoreKey};
use pdfcraft_sign::{PublicKey, SignError};

/// One key of every kind, with what the store must hold to sign for it.
fn every_kind() -> Vec<(PublicKey, Option<StoreKey>)> {
    let point = vec![4u8; 65];
    let keys = vec![
        (PublicKey::Rsa { n: vec![0xc5; 256], e: vec![1, 0, 1] }, Some(StoreKey::Rsa)),
        (PublicKey::P256(point.clone()), Some(StoreKey::Ecdsa(256))),
        (PublicKey::P384(point.clone()), Some(StoreKey::Ecdsa(384))),
        (PublicKey::P521(point.clone()), None),
        (PublicKey::BrainpoolP256(point.clone()), None),
        (PublicKey::BrainpoolP384(point.clone()), None),
        (PublicKey::BrainpoolP512(point.clone()), None),
        (PublicKey::Ed25519(vec![7; 32]), None),
        (PublicKey::Unsupported { what: "elliptic curve 1.3.132.0.10".into(), spki: Vec::new() }, None),
    ];
    // Every variant is named, with no `_` arm: a new one stops this test compiling until it is
    // listed above with the answer the store should give.
    for (key, _) in &keys {
        match key {
            PublicKey::Rsa { .. }
            | PublicKey::P256(_)
            | PublicKey::P384(_)
            | PublicKey::P521(_)
            | PublicKey::BrainpoolP256(_)
            | PublicKey::BrainpoolP384(_)
            | PublicKey::BrainpoolP512(_)
            | PublicKey::Ed25519(_)
            | PublicKey::Unsupported { .. } => {}
        }
    }
    keys
}

#[test]
fn the_store_signs_only_with_rsa_p256_and_p384() {
    for (key, want) in every_kind() {
        assert_eq!(key.store_signing_key(), want, "{}", key.describe());
        if want.is_none() {
            // Skipped when listing, and refused (not a panic) if one ever reached signing.
            let refused = key.store_signature_der(&[0; 64]);
            assert!(matches!(refused, Err(SignError::Unsupported(_))), "{}: {refused:?}", key.describe());
        }
    }
}

#[test]
fn cng_signatures_become_cms_signatures_that_verify() {
    let msg = b"hello";

    let k = p256::ecdsa::SigningKey::from_slice(&[0x11; 32]).unwrap();
    let public = PublicKey::P256(k.verifying_key().to_sec1_point(false).as_bytes().to_vec());
    let digest = DigestAlg::Sha256.digest(&[msg]);
    let s: p256::ecdsa::Signature = k.sign_prehash(&digest).unwrap();
    // CNG returns r ‖ s, 32 bytes each for P-256.
    let der = public.store_signature_der(&s.to_bytes()).unwrap();
    assert_eq!(der.first(), Some(&0x30), "a DER SEQUENCE");
    assert!(public.verify(Scheme::Ecdsa, DigestAlg::Sha256, &digest, &der).unwrap());

    let k = p384::ecdsa::SigningKey::from_slice(&[0x22; 48]).unwrap();
    let public = PublicKey::P384(k.verifying_key().to_sec1_point(false).as_bytes().to_vec());
    let digest = DigestAlg::Sha384.digest(&[msg]);
    let s: p384::ecdsa::Signature = k.sign_prehash(&digest).unwrap();
    let der = public.store_signature_der(&s.to_bytes()).unwrap();
    assert!(public.verify(Scheme::Ecdsa, DigestAlg::Sha384, &digest, &der).unwrap());

    // RSA PKCS #1 v1.5 signatures are already what CMS carries.
    let rsa = PublicKey::Rsa { n: vec![0xc5; 256], e: vec![1, 0, 1] };
    assert_eq!(rsa.store_signature_der(&[9; 256]).unwrap(), vec![9; 256]);
}

#[test]
fn a_malformed_cng_signature_is_an_error() {
    // Wrong lengths, and right lengths whose r and s are zero or not below the group order.
    let p256: [&[u8]; 5] = [&[], &[1; 63], &[1; 65], &[0; 64], &[0xff; 64]];
    let p384: [&[u8]; 5] = [&[], &[1; 95], &[1; 97], &[0; 96], &[0xff; 96]];
    for (key, bad) in [(PublicKey::P256(Vec::new()), p256), (PublicKey::P384(Vec::new()), p384)] {
        for raw in bad {
            let got = key.store_signature_der(raw);
            assert!(matches!(got, Err(SignError::Crypto(_))), "{}, {} bytes: {got:?}", key.describe(), raw.len());
        }
    }
}
