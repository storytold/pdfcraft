//! Signing with a certificate whose private key lives in a legacy CryptoAPI provider (a CSP),
//! such as the Certisign desktopID provider, which registers no CNG key storage provider.
//!
//! The workspace forbids `unsafe`; this crate is its one exception (see Cargo.toml). The FFI stays
//! behind two safe functions that take the certificate context rustls-cng already holds.

#![cfg(windows)]

use std::ptr;

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Security::Cryptography::{
    CALG_SHA_256, CALG_SHA_384, CALG_SHA_512, CERT_CONTEXT, CERT_KEY_PROV_INFO_PROP_ID, CERT_NCRYPT_KEY_SPEC, CRYPT_ACQUIRE_ALLOW_NCRYPT_KEY_FLAG,
    CRYPT_KEY_PROV_INFO, CertGetCertificateContextProperty, CryptAcquireCertificatePrivateKey, CryptCreateHash, CryptDestroyHash,
    CryptReleaseContext, CryptSetHashParam, CryptSignHashW, HP_HASHVAL, NCryptFreeObject,
};
use windows_sys::core::BOOL;

/// The digest a signature is made over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Digest {
    Sha256,
    Sha384,
    Sha512,
}

/// Whether the certificate's private key is in a legacy CryptoAPI provider (its key provider
/// info names a provider type other than 0, which CNG key storage providers use). Read from the
/// certificate's properties, so it never opens the key or shows a prompt. `None` when the
/// certificate has no key provider info.
pub fn is_legacy_key(cert: &CERT_CONTEXT) -> Option<bool> {
    let mut size = 0u32;
    // SAFETY: a valid certificate context; a null buffer asks for the size.
    let ok = unsafe { CertGetCertificateContextProperty(cert, CERT_KEY_PROV_INFO_PROP_ID, ptr::null_mut(), &mut size) };
    if ok == 0 || (size as usize) < std::mem::size_of::<CRYPT_KEY_PROV_INFO>() {
        return None;
    }
    // u64 words keep the buffer aligned for CRYPT_KEY_PROV_INFO.
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    // SAFETY: the buffer holds `size` bytes, as the first call asked for.
    let ok = unsafe { CertGetCertificateContextProperty(cert, CERT_KEY_PROV_INFO_PROP_ID, buf.as_mut_ptr().cast(), &mut size) };
    if ok == 0 {
        return None;
    }
    // SAFETY: Windows filled the buffer with a CRYPT_KEY_PROV_INFO (plus the strings it points to).
    let info = unsafe { &*buf.as_ptr().cast::<CRYPT_KEY_PROV_INFO>() };
    Some(info.dwProvType != 0)
}

/// What [`sign`] did.
pub enum Signed {
    /// The PKCS #1 v1.5 signature over the digest, big-endian as CMS wants it.
    Signature(Vec<u8>),
    /// The key turned out to be a CNG key: sign it through CNG instead.
    Cng,
}

/// Sign `digest` (already computed with `alg`) with the certificate's private key in a legacy
/// CryptoAPI provider, PKCS #1 v1.5. The key is acquired without the silent flag, so the provider
/// can show its own authorization (desktopID asks for approval in the remoteID app), and released
/// after each signature, so the next one asks again. Errors carry the Windows error code.
pub fn sign(cert: &CERT_CONTEXT, alg: Digest, digest: &[u8]) -> Result<Signed, u32> {
    let mut handle: usize = 0;
    let mut key_spec: u32 = 0;
    let mut caller_free: BOOL = 0;
    // SAFETY: valid context and out pointers; CNG keys are allowed so a CNG certificate is reported, not refused.
    let ok = unsafe {
        CryptAcquireCertificatePrivateKey(cert, CRYPT_ACQUIRE_ALLOW_NCRYPT_KEY_FLAG, ptr::null(), &mut handle, &mut key_spec, &mut caller_free)
    };
    if ok == 0 {
        return Err(last_error());
    }
    if key_spec == CERT_NCRYPT_KEY_SPEC {
        if caller_free != 0 {
            // SAFETY: an NCRYPT_KEY_HANDLE the call says the caller frees.
            unsafe { NCryptFreeObject(handle) };
        }
        return Ok(Signed::Cng);
    }
    let result = sign_with_provider(handle, key_spec, alg, digest);
    if caller_free != 0 {
        // SAFETY: an HCRYPTPROV the call says the caller releases.
        unsafe { CryptReleaseContext(handle, 0) };
    }
    result.map(Signed::Signature)
}

fn sign_with_provider(prov: usize, key_spec: u32, alg: Digest, digest: &[u8]) -> Result<Vec<u8>, u32> {
    let calg = match alg {
        Digest::Sha256 => CALG_SHA_256,
        Digest::Sha384 => CALG_SHA_384,
        Digest::Sha512 => CALG_SHA_512,
    };
    let mut hash: usize = 0;
    // SAFETY: a provider handle acquired above.
    if unsafe { CryptCreateHash(prov, calg, 0, 0, &mut hash) } == 0 {
        return Err(last_error());
    }
    let result = (|| {
        // SAFETY: HP_HASHVAL takes the digest bytes; the hash object is the right size for `alg`.
        if unsafe { CryptSetHashParam(hash, HP_HASHVAL, digest.as_ptr(), 0) } == 0 {
            return Err(last_error());
        }
        let mut len = 0u32;
        // SAFETY: a null buffer asks for the signature size.
        if unsafe { CryptSignHashW(hash, key_spec, ptr::null(), 0, ptr::null_mut(), &mut len) } == 0 {
            return Err(last_error());
        }
        let mut sig = vec![0u8; len as usize];
        // SAFETY: the buffer holds `len` bytes.
        if unsafe { CryptSignHashW(hash, key_spec, ptr::null(), 0, sig.as_mut_ptr(), &mut len) } == 0 {
            return Err(last_error());
        }
        sig.truncate(len as usize);
        // CryptoAPI returns the signature little-endian; CMS wants it big-endian.
        sig.reverse();
        Ok(sig)
    })();
    // SAFETY: the hash object created above.
    unsafe { CryptDestroyHash(hash) };
    result
}

fn last_error() -> u32 {
    // SAFETY: no preconditions.
    unsafe { GetLastError() }
}
