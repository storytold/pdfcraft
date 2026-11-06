//! Provider-free end-to-end checks through the public automation table.
use super::*;
use base64::{Engine as _, prelude::BASE64_STANDARD};
use pdfcraft_engine::sign::{self, Certificate, DigestAlg, Name, PrivateKey, Time};
use pdfcraft_platform::signing_browser::{SigningExchange, SigningTransport};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Response(std::sync::Mutex<Option<Result<Vec<u8>, String>>>);
impl SigningExchange for Response {
    fn poll(&self) -> Option<Result<Vec<u8>, String>> {
        self.0.lock().unwrap().take()
    }
    fn cancel(&self) {}
}

fn transport(calls: Arc<AtomicUsize>) -> SigningTransport {
    let key = PrivateKey::generate_p256().unwrap();
    let certificate =
        Certificate::self_signed(&Name::build("Synthetic Signer", "", "", "", "AT"), &key, Time::from_unix(1_700_000_000), 10, &[1]).unwrap();
    Arc::new(move |request| {
        calls.fetch_add(1, Ordering::SeqCst);
        let request = std::str::from_utf8(&request).unwrap();
        let field = |tag: &str| request.split_once(&format!("<sl:{tag}>")).unwrap().1.split_once(&format!("</sl:{tag}>")).unwrap().0;
        let bytes = BASE64_STANDARD.decode(field("Base64Content")).unwrap();
        let start: usize = field("From").parse().unwrap();
        let end = field("To").parse::<usize>().unwrap() + 1;
        let digest = DigestAlg::Sha256.digest(&[&bytes[..start], &bytes[end..]]);
        let cms = sign::cms::sign_detached(&key, &certificate, &[], DigestAlg::Sha256, &digest).unwrap();
        let xml = format!(
            "<sl:CreateCMSSignatureResponse xmlns:sl=\"{}\"><sl:CMSSignature>{}</sl:CMSSignature></sl:CreateCMSSignatureResponse>",
            sign::security_layer::NAMESPACE,
            BASE64_STANDARD.encode(cms)
        );
        Ok(Box::new(Response(std::sync::Mutex::new(Some(Ok(xml.into_bytes()))))))
    })
}

fn begin(a: &mut Automation, doc: u64, out: &str) -> Value {
    ok(
        a,
        "sign_begin",
        json!({"doc":doc,"provider":"id-austria","allow_document_upload":true,"out":out,"page":2,"rect":[20,20,180,80],"graphic":{"text":"Approved"}}),
    )
}

#[test]
fn remote_signing_requires_opt_in_consent_and_confined_new_destination_before_transport() {
    let dir = workdir("remote-preflight");
    let mut a = auto(&dir);
    assert_eq!(ok(&mut a, "sign_providers", json!({}))["providers"][0]["enabled"], false);
    let doc = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    assert!(a.call("sign_begin", &json!({"doc":doc,"provider":"id-austria","allow_document_upload":true,"out":"signed.pdf"})).is_err());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut a = a.with_signing_transport(transport(calls.clone()));
    for args in [
        json!({"out":"signed.pdf"}),
        json!({"allow_document_upload":true,"out":"../escape.pdf"}),
        json!({"allow_document_upload":true,"out":"a.pdf"}),
        json!({"allow_document_upload":true,"out":"missing/s.pdf"}),
    ] {
        let mut full = json!({"doc":doc,"provider":"id-austria"});
        full.as_object_mut().unwrap().extend(args.as_object().unwrap().clone());
        assert!(a.call("sign_begin", &full).is_err());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn remote_signing_saves_verified_copy_and_retries_without_another_authorization() {
    let dir = workdir("remote-finish");
    let original = std::fs::read(dir.join("a.pdf")).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut a = auto(&dir).with_signing_transport(transport(calls.clone()));
    let doc = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    let job = begin(&mut a, doc, "signed.pdf")["job"].clone();
    assert_eq!(ok(&mut a, "sign_status", json!({"job":job}))["state"], "ready_to_save");
    std::fs::write(dir.join("signed.pdf"), b"destination planted while authorizing").unwrap();
    assert!(a.call("sign_finish", &json!({"job":job})).is_err());
    assert!(a.call("sign_finish", &json!({"job":job,"out":"../escape.pdf"})).is_err());
    assert_eq!(std::fs::read(dir.join("signed.pdf")).unwrap(), b"destination planted while authorizing");
    assert_eq!(ok(&mut a, "sign_finish", json!({"job":job,"out":"retry.pdf"}))["state"], "saved");
    let list = ok(&mut a, "sign_list", json!({"doc":doc}));
    assert_eq!(list["signatures"][0]["signer"], "Synthetic Signer");
    assert_eq!(list["signatures"][0]["page"], 2);
    assert_ne!(list["signatures"][0]["status"], "invalid");
    assert_eq!(std::fs::read(dir.join("a.pdf")).unwrap(), original);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(a.call("sign_finish", &json!({"job":job})).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn remote_signing_rejects_edits_during_authorization_and_cancel_never_saves() {
    let dir = workdir("remote-stale");
    let calls = Arc::new(AtomicUsize::new(0));
    let mut a = auto(&dir).with_signing_transport(transport(calls));
    let doc = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    let job = begin(&mut a, doc, "signed.pdf")["job"].clone();
    ok(&mut a, "page_rotate", json!({"doc":doc,"pages":[1],"degrees":90}));
    assert_eq!(ok(&mut a, "sign_status", json!({"job":job}))["state"], "failed");
    assert!(a.call("sign_finish", &json!({"job":job})).is_err());
    ok(&mut a, "sign_cancel", json!({"job":job}));
    let job = begin(&mut a, doc, "cancelled.pdf")["job"].clone();
    ok(&mut a, "sign_cancel", json!({"job":job}));
    assert!(!dir.join("signed.pdf").exists());
    assert!(!dir.join("cancelled.pdf").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn remote_signing_wait_mode_finishes_in_one_call_for_cli_scripts() {
    let dir = workdir("remote-wait");
    let calls = Arc::new(AtomicUsize::new(0));
    let mut a = auto(&dir).with_signing_transport(transport(calls.clone()));
    let doc = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    let result = ok(&mut a, "sign_begin", json!({"doc":doc,"provider":"id-austria","allow_document_upload":true,"out":"signed.pdf","wait":true}));
    assert_eq!(result["state"], "saved");
    assert!(dir.join("signed.pdf").exists());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn remote_signing_rechecks_the_document_after_a_verified_result_was_retained() {
    let dir = workdir("remote-ready-stale");
    let mut a = auto(&dir).with_signing_transport(transport(Arc::new(AtomicUsize::new(0))));
    let doc = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    let job = begin(&mut a, doc, "signed.pdf")["job"].clone();
    assert_eq!(ok(&mut a, "sign_status", json!({"job":job}))["state"], "ready_to_save");
    ok(&mut a, "page_rotate", json!({"doc":doc,"pages":[1],"degrees":90}));
    assert!(a.call("sign_finish", &json!({"job":job})).unwrap_err().to_string().contains("changed"));
    assert!(!dir.join("signed.pdf").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn remote_signing_rechecks_output_symlinks_after_authorization() {
    let dir = workdir("remote-symlink");
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.join("output")).unwrap();
    let mut a = auto(&dir).with_signing_transport(transport(Arc::new(AtomicUsize::new(0))));
    let doc = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
    let job = begin(&mut a, doc, "output/signed.pdf")["job"].clone();
    std::fs::rename(dir.join("output"), dir.join("old-output")).unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.join("output")).unwrap();
    assert!(a.call("sign_finish", &json!({"job":job})).is_err());
    assert!(!outside.path().join("signed.pdf").exists());
    assert_eq!(ok(&mut a, "sign_finish", json!({"job":job,"out":"safe.pdf"}))["state"], "saved");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn remote_signing_refuses_certification_conflicts_before_upload() {
    let dir = workdir("remote-certification");
    let calls = Arc::new(AtomicUsize::new(0));
    let mut a = auto(&dir).with_signing_transport(transport(calls.clone()));
    ok(&mut a, "sign_id_create", json!({"name":"Synthetic", "key":"p256", "password":"synthetic-test-only", "path":"id.p12"}));
    for (certify, output) in [(Some("no_changes"), "certified.pdf"), (None, "approved.pdf")] {
        let doc = ok(&mut a, "doc_open", json!({"path":"a.pdf"}))["doc"].as_u64().unwrap();
        let mut args = json!({"doc":doc,"id":"id.p12","password":"synthetic-test-only","out":output});
        if let Some(certify) = certify {
            args["certify"] = json!(certify);
        }
        ok(&mut a, "sign_document", args);
        let mut begin = json!({"doc":doc,"provider":"id-austria","allow_document_upload":true,"out":"remote.pdf"});
        if certify.is_none() {
            begin["certify"] = json!("form_fill");
        }
        let error = a.call("sign_begin", &begin).unwrap_err().to_string();
        assert!(error.contains(if certify.is_some() { "no changes" } else { "first signature" }), "{error}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(!dir.join("remote.pdf").exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn remote_signing_refuses_a_read_only_signature_field_before_upload() {
    use pdfcraft_cos::{Dict, Document, Object, PdfString, SaveOptions};
    let dir = workdir("remote-readonly-field");
    let mut pdf = Document::open(Arc::new(fixture(1))).unwrap();
    let mut field = Dict::new();
    field.set(b"FT".to_vec(), Object::name("Sig"));
    field.set(b"T".to_vec(), Object::String(PdfString::literal("Approval")));
    field.set(b"Ff".to_vec(), Object::Int(1));
    let field = pdf.add(Object::Dict(field));
    let mut form = Dict::new();
    form.set(b"Fields".to_vec(), Object::Array(vec![Object::Ref(field)]));
    pdf.update_dict(pdf.root().unwrap(), |catalog| {
        catalog.set(b"AcroForm".to_vec(), Object::Dict(form));
    })
    .unwrap();
    std::fs::write(dir.join("readonly.pdf"), pdfcraft_cos::write_full(&pdf, &SaveOptions::default()).unwrap()).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let mut a = auto(&dir).with_signing_transport(transport(calls.clone()));
    let doc = ok(&mut a, "doc_open", json!({"path":"readonly.pdf"}))["doc"].as_u64().unwrap();
    let error = a
        .call("sign_begin", &json!({"doc":doc,"provider":"id-austria","allow_document_upload":true,"out":"remote.pdf","field":"Approval"}))
        .unwrap_err()
        .to_string();
    assert!(error.contains("read-only"), "{error}");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(dir).unwrap();
}
