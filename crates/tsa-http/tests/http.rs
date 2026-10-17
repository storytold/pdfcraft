//! The time server transport against a loopback server: a signature gets the server's token, and a
//! server that can't be reached or answers wrongly costs only the token, never the signature.

use std::sync::Arc;

use pdfcraft_cos::Document;
use pdfcraft_sign::{SignOptions, Time, TrustStore, pkcs12, signatures};
use pdfcraft_tsa_http::{HttpAuthority, sign_with_fallback, test_server};

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../sign/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

const TSA_TIME: Time = Time { year: 2026, month: 10, day: 6, hour: 12, minute: 0, second: 0 };

/// One page with a line of text.
fn fixture() -> Vec<u8> {
    let objs: [&[u8]; 4] = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R >>",
        b"<< /Length 0 >>\nstream\n\nendstream",
    ];
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
    SignOptions { page: 0, rect: None, date: "D:20261006120000Z".into(), ..SignOptions::default() }
}

/// A time server that signs a proper RFC 3161 answer with a test digital ID.
fn tsa_answer(request: &[u8]) -> (u16, Vec<u8>) {
    let tsa = pkcs12::open(&data("rsa-aes.p12"), "test").unwrap();
    let Ok(q) = pdfcraft_sign::timestamp::parse_request(request) else { return (400, Vec::new()) };
    let answer =
        pdfcraft_sign::timestamp::respond(&tsa.key, &tsa.certificate, &tsa.chain, pdfcraft_sign::DigestAlg::Sha256, &q, "1.2.3.4", TSA_TIME, 7);
    (200, answer.unwrap())
}

/// Sign `fixture()` through `authority`, falling back as the app and the tools do.
fn sign(authority: &HttpAuthority) -> (Vec<u8>, Option<String>) {
    let id = pkcs12::open(&data("ec-p256.p12"), "test").unwrap();
    let doc = open(&fixture());
    sign_with_fallback(authority, |tsa| pdfcraft_sign::sign_with_timestamp(&doc, &id, &opts(), tsa), || pdfcraft_sign::sign(&doc, &id, &opts()))
        .unwrap()
}

fn signature(bytes: &[u8]) -> pdfcraft_sign::SignatureInfo {
    let trust = TrustStore { certs: vec![pkcs12::open(&data("rsa-aes.p12"), "test").unwrap().certificate] };
    signatures(&open(bytes), bytes, &trust).into_iter().find(|s| s.signed).expect("a signature")
}

#[test]
fn signing_with_a_time_server_embeds_its_timestamp() {
    let server = test_server::serve(tsa_answer).unwrap();
    let authority = HttpAuthority::new(&server.url).unwrap();
    assert_eq!(server.requests(), 0, "naming a server sends nothing");
    let (signed, unstamped) = sign(&authority);
    assert_eq!(unstamped, None);
    assert_eq!(server.requests(), 1);
    let s = signature(&signed);
    assert!(s.timestamp, "{:?}", s.details);
    assert_eq!(s.timestamp_time, Some(TSA_TIME), "{:?}", s.details);
}

#[test]
fn an_unreachable_server_signs_without_a_timestamp_and_says_why() {
    let url = test_server::unreachable_url().unwrap();
    let authority = HttpAuthority::new(&url).unwrap().with_timeout(std::time::Duration::from_secs(5));
    let (signed, unstamped) = sign(&authority);
    let reason = unstamped.expect("says why there is no timestamp");
    assert!(reason.contains(&url) && reason.contains("could not be reached"), "{reason}");
    let s = signature(&signed);
    assert!(!s.timestamp && s.timestamp_time.is_none(), "signed with this computer's clock: {:?}", s.details);
}

/// What a test time server answers a request with: an HTTP status and a body.
type Answer = Box<dyn Fn(&[u8]) -> (u16, Vec<u8>) + Send>;

#[test]
fn a_wrong_answer_signs_without_a_timestamp_and_says_why() {
    let other_imprint = |request: &[u8]| {
        let mut q = pdfcraft_sign::timestamp::parse_request(request).unwrap();
        q.imprint = vec![0; 32];
        tsa_answer(&q.encode().unwrap())
    };
    let answers: [(Answer, &str); 5] = [
        (Box::new(|_: &[u8]| (200, b"not DER".to_vec())), "refused"),
        (Box::new(|_: &[u8]| (200, Vec::new())), "refused"),
        (Box::new(|_: &[u8]| (500, Vec::new())), "HTTP 500"),
        // A redirect is not followed: the request goes to the chosen server only.
        (Box::new(|_: &[u8]| (302, Vec::new())), "HTTP 302"),
        (Box::new(other_imprint), "imprint"),
    ];
    for (answer, why) in answers {
        let server = test_server::serve(answer).unwrap();
        let (signed, unstamped) = sign(&HttpAuthority::new(&server.url).unwrap());
        let reason = unstamped.expect("says why there is no timestamp");
        assert!(reason.contains(why), "{why}: {reason}");
        assert!(!signature(&signed).timestamp, "{why}");
    }
}

#[test]
fn a_signing_error_before_the_server_is_asked_stays_an_error() {
    let server = test_server::serve(tsa_answer).unwrap();
    let authority = HttpAuthority::new(&server.url).unwrap();
    let mut plain_tried = false;
    let r: Result<((), Option<String>), String> = sign_with_fallback(
        &authority,
        |_| Err("the password is incorrect".to_string()),
        || {
            plain_tried = true;
            Ok(())
        },
    );
    assert_eq!(r, Err("the password is incorrect".to_string()));
    assert!(!plain_tried && server.requests() == 0);
}

#[test]
fn a_document_timestamp_comes_from_the_server() {
    let server = test_server::serve(tsa_answer).unwrap();
    let authority = HttpAuthority::new(&server.url).unwrap();
    let stamped = pdfcraft_sign::timestamp_document(&open(&fixture()), &authority, "D:20261006120000Z").unwrap();
    let trust = TrustStore { certs: vec![pkcs12::open(&data("rsa-aes.p12"), "test").unwrap().certificate] };
    let s = signatures(&open(&stamped), &stamped, &trust).into_iter().find(|s| s.doc_timestamp).expect("a document timestamp");
    assert_eq!(s.timestamp_time, Some(TSA_TIME), "{:?}", s.details);
}
