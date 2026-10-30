//! Explicit Crypt is consumed by object decryption, before compression decoding.
use hayro_syntax::{
    DecryptionTarget, ExternalDecryptor, Pdf,
    object::{ObjectIdentifier, Stream},
};
use std::io::Write;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Debug, Default)]
struct Decrypt(AtomicUsize);
impl ExternalDecryptor for Decrypt {
    fn decrypt(&self, _: ObjectIdentifier, data: &[u8], target: DecryptionTarget) -> Option<Vec<u8>> {
        self.0.fetch_add(1, Ordering::Relaxed);
        match target {
            DecryptionTarget::Stream { filter: Some(name), .. } if name != b"Identity" && name != b"DefaultCryptFilter" => None,
            _ => Some(data.to_vec()),
        }
    }
}
fn fixture(selector: &str, compressed: bool) -> Vec<u8> {
    let content = b"q 0 0 20 20 re f Q";
    let data = if compressed {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(content).unwrap();
        z.finish().unwrap()
    } else {
        content.to_vec()
    };
    let mut stream = format!("<< /Length {} {selector} >>\nstream\n", data.len()).into_bytes();
    stream.extend_from_slice(&data);
    stream.extend_from_slice(b"\nendstream");
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_vec(),
        stream,
        b"<< /Filter /Adobe.PubSec >>".to_vec(),
    ];
    let mut bytes = b"%PDF-2.0\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        bytes.extend_from_slice(object);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size 6 /Root 1 0 R /Encrypt 5 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    assert!(bytes.windows(6).any(|s| s == b"/Crypt"), "the fixture must retain its explicit selector");
    bytes
}
#[test]
fn authenticated_explicit_crypt_is_consumed_once_before_flate() {
    for selector in [
        "/Filter /Crypt /DecodeParms << /Name /DefaultCryptFilter >>",
        "/Filter /Crypt /DecodeParms << /Name /Identity >>",
        "/Filter /Crypt",
        "/Filter [/Crypt] /DecodeParms [<< /Name /Identity >>]",
        "/Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /DefaultCryptFilter >> null]",
        "/Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /Identity >> null]",
    ] {
        let decrypt = Arc::new(Decrypt::default());
        let pdf = Pdf::new_with_external_decryptor(fixture(selector, selector.contains("FlateDecode")), decrypt.clone()).unwrap();
        let stream = pdf.xref().get::<Stream<'_>>(ObjectIdentifier::new(4, 0)).unwrap();
        let before = decrypt.0.load(Ordering::Relaxed);
        assert_eq!(&*stream.decoded().unwrap(), b"q 0 0 20 20 re f Q", "{selector}");
        assert_eq!(decrypt.0.load(Ordering::Relaxed) - before, 1, "one object decrypt per decode");
        assert!(stream.decoded_within(8).unwrap().len() <= 8);
    }
}
#[test]
fn authenticated_explicit_crypt_rejects_unsupported_and_malformed_selectors() {
    for selector in [
        "/Filter /Crypt /DecodeParms << /Name /Unknown >>",
        "/Filter /Crypt /DecodeParms << /Name 42 >>",
        "/Filter [/Crypt /Crypt]",
        "/Filter [/FlateDecode /Crypt] /DecodeParms [null << /Name /Identity >>]",
    ] {
        let pdf = Pdf::new_with_external_decryptor(fixture(selector, false), Arc::new(Decrypt::default())).unwrap();
        let stream = pdf.xref().get::<Stream<'_>>(ObjectIdentifier::new(4, 0)).unwrap();
        assert!(stream.decoded().is_err(), "{selector}");
    }
}
