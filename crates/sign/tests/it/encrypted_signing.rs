//! The byte ranges of a signature on an encrypted document.

use std::sync::Arc;

use pdfcraft_cos::{Document, SaveOptions};
use pdfcraft_sign::{SignOptions, Status, TrustStore, pkcs12, signatures};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// One page with text and an unsigned signature field "Approval".
fn fixture() -> Vec<u8> {
    fixture_with(b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [6 0 R] >> >>", &[])
}

/// [`fixture`] with its catalog replaced by `catalog` and `more` objects from number 7 on.
fn fixture_with(catalog: &[u8], more: &[&[u8]]) -> Vec<u8> {
    let mut objs: Vec<&[u8]> = vec![
        catalog,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] >>",
        b"<< /Length 44 >>\nstream\nBT /F1 14 Tf 20 250 Td (Contract text) Tj ET\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        b"<< /Type /Annot /Subtype /Widget /FT /Sig /T (Approval) /Rect [150 20 280 70] /P 3 0 R /F 4 >>",
    ];
    objs.extend_from_slice(more);
    write_pdf(&objs)
}

/// A classic-xref PDF of `objs`, numbered from 1, with object 1 as the catalog.
fn write_pdf(objs: &[&[u8]]) -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

fn open(b: &[u8]) -> Document {
    Document::open(Arc::new(b.to_vec())).unwrap()
}

fn opts() -> SignOptions {
    SignOptions {
        page: 0,
        rect: Some([20.0, 100.0, 220.0, 150.0]),
        reason: Some("I approve this document".into()),
        location: Some("London".into()),
        date: "D:20261002120000+01'00'".into(),
        ..SignOptions::default()
    }
}

/// `fixture()` protected with an empty user password (opens without asking, like most
/// "secured" forms) and an owner password, then saved.
fn protected(alg: pdfcraft_cos::Algorithm, permissions: i32) -> Vec<u8> {
    protected_with(alg, "", permissions)
}

/// `fixture()` protected with the user password `user` and an owner password, then saved.
fn protected_with(alg: pdfcraft_cos::Algorithm, user: &str, permissions: i32) -> Vec<u8> {
    let mut doc = open(&fixture());
    doc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: alg,
        user_password: user,
        owner_password: "owner",
        permissions,
        encrypt_metadata: true,
        seed: [17; 32],
    })
    .unwrap();
    pdfcraft_cos::write_full(&doc, &SaveOptions::default()).unwrap()
}

fn open_pw(b: &[u8], pw: Option<&str>) -> Document {
    Document::open_with_password(Arc::new(b.to_vec()), pw).unwrap()
}

const ALL_ALGORITHMS: [pdfcraft_cos::Algorithm; 4] =
    [pdfcraft_cos::Algorithm::Rc4_40, pdfcraft_cos::Algorithm::Rc4_128, pdfcraft_cos::Algorithm::Aes128, pdfcraft_cos::Algorithm::Aes256];

/// Where `needle` first appears in `haystack`.
fn find(haystack: &[u8], needle: &[u8], alg: pdfcraft_cos::Algorithm) -> usize {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
        .unwrap_or_else(|| panic!("{alg:?}: {:?} is not in the added bytes", String::from_utf8_lossy(needle)))
}

/// The four numbers of the signature dictionary's `/ByteRange` in the added revision.
fn byte_range(added: &[u8], alg: pdfcraft_cos::Algorithm) -> [usize; 4] {
    let prefix = b"/ByteRange [";
    let at = find(added, prefix, alg) + prefix.len();
    let end = at + added[at..].iter().position(|b| *b == b']').unwrap_or_else(|| panic!("{alg:?}: the /ByteRange array is not closed"));
    let text = std::str::from_utf8(&added[at..end]).unwrap_or_else(|_| panic!("{alg:?}: the /ByteRange numbers are not ASCII"));
    let nums: Vec<usize> =
        text.split_ascii_whitespace().map(|n| n.parse().unwrap_or_else(|_| panic!("{alg:?}: {n:?} is not a byte offset"))).collect();
    assert_eq!(nums.len(), 4, "{alg:?}: /ByteRange has {} numbers, not 4", nums.len());
    [nums[0], nums[1], nums[2], nums[3]]
}

/// The signature dictionary's `/ByteRange` gives the offsets and lengths of the parts of the
/// file its digest covers. For a signature on an encrypted document that is the whole earlier
/// revision, its encrypted objects included, leaving out only the hex text of `/Contents`.
#[test]
fn signed_encrypted_documents_cover_their_earlier_revision() {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let trust = TrustStore { certs: vec![id.certificate.clone()] };
    for alg in ALL_ALGORITHMS {
        let base = protected(alg, -1);
        let doc = open_pw(&base, None);
        let signed = pdfcraft_sign::sign(&doc, &id, &opts()).unwrap();
        assert!(signed.starts_with(&base), "{alg:?}: an incremental update");
        let added = &signed[base.len()..];
        // The hole: the `<...>` hex string of `/Contents`.
        let at = base.len() + find(added, b"/Contents <", alg);
        let hole_start = at + b"/Contents ".len();
        let hole_end = hole_start + signed[hole_start..].iter().position(|b| *b == b'>').expect("a closed /Contents string") + 1;
        let [a, b, c, d] = byte_range(added, alg);
        assert_eq!(a, 0, "{alg:?}: the covered range starts at the start of the file");
        assert_eq!(b, hole_start, "{alg:?}: the covered range ends at {hole_start}, where /Contents begins");
        assert_eq!(c, hole_end, "{alg:?}: the second covered range starts at {hole_end}, where /Contents ends");
        assert_eq!(c + d, signed.len(), "{alg:?}: the covered range ends at the file's end ({c} + {d} of {})", signed.len());
        assert!(hole_start >= base.len(), "{alg:?}: the hole at {hole_start} comes after the whole earlier revision of {} bytes", base.len());
        assert_eq!(signed[hole_start], b'<', "{alg:?}: the hole opens with '<' at {hole_start}");
        assert_eq!(signed[hole_end - 1], b'>', "{alg:?}: the hole closes with '>' at {}", hole_end - 1);
        assert!(
            signed[hole_start + 1..hole_end - 1].iter().all(u8::is_ascii_hexdigit),
            "{alg:?}: /Contents between {hole_start} and {hole_end} is hexadecimal"
        );
        let sigs = signatures(&open_pw(&signed, None), &signed, &trust);
        let s = sigs.iter().find(|s| s.signed).expect("a signed field");
        assert_eq!(s.status, Status::Valid, "{alg:?}: {:?}", s.details);
        // One changed byte inside the covered range, in the earlier revision.
        let mut tampered = signed.clone();
        let flip = base.len() - 1;
        tampered[flip] ^= 1;
        let sigs = signatures(&open_pw(&tampered, None), &tampered, &trust);
        let signed_sigs: Vec<_> = sigs.iter().filter(|s| s.signed).collect();
        assert!(!signed_sigs.is_empty(), "{alg:?}: the tampered file still lists its signature");
        assert!(
            signed_sigs.iter().all(|s| s.status == Status::Invalid),
            "{alg:?}: the byte changed at {flip}, inside the covered range, makes every signature invalid"
        );
    }
}
