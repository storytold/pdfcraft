//! Public-key workflows use synthetic certificates; only the ignored test needs SoftHSM.
#![cfg(not(target_arch = "wasm32"))]
use super::*;
use pdfcraft_engine::sign::{Certificate, DigestAlg, DigitalId, Name, PrivateKey, Time, der};

fn identity() -> DigitalId {
    let key = PrivateKey::generate_rsa(2048).unwrap();
    let certificate =
        Certificate::self_signed(&Name::build("Encryption automation test", "", "", "", ""), &key, Time::from_unix(1_600_000_000), 30, &[42])
            .unwrap();
    let fields = der::Tlv::parse_all(&certificate.raw).unwrap().children().unwrap();
    let tbs = fields[0].children().unwrap();
    let mut values: Vec<Vec<u8>> = tbs.iter().filter(|v| v.tag != der::tag::ctx(3)).map(|v| v.raw.to_vec()).collect();
    let usage = der::seq(&[&der::oid("2.5.29.15"), &der::octets(&der::tlv(der::tag::BIT_STRING, &[5, 0x20]))]);
    values.push(der::explicit(3, &der::seq(&[&usage])));
    let tbs = der::seq(&values.iter().map(Vec::as_slice).collect::<Vec<_>>());
    let signature = key.sign(DigestAlg::Sha256, &tbs).unwrap();
    let certificate = Certificate::parse(&der::seq(&[&tbs, fields[1].raw, &der::bit_string(&signature)])).unwrap();
    DigitalId { key, certificate, chain: Vec::new(), friendly_name: None }
}

#[test]
fn certificate_tools_encrypt_render_save_and_undo_without_a_token() {
    let dir = workdir("certificate-tools");
    let id = identity();
    std::fs::write(dir.join("recipient.der"), &id.certificate.raw).unwrap();
    let mut a = auto(&dir);
    let doc = ok(&mut a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_set_info", json!({"doc": doc, "key": "Title", "value": "Private metadata"}));
    let protected = ok(&mut a, "doc_protect_certificates", json!({"doc": doc, "recipients": [{"certificate": "recipient.der"}]}));
    assert_eq!(protected["encrypted"], true);
    assert_eq!(protected["security"]["pending"], true);
    assert!(ok(&mut a, "text_extract", json!({"doc": doc})).to_string().contains("Page 1"));
    assert!(a.call("page_render", &json!({"doc": doc, "page": 1, "dpi": 36})).unwrap().iter().any(|c| matches!(c, Content::Png { .. })));
    ok(&mut a, "doc_save", json!({"doc": doc, "path": "encrypted.pdf"}));
    let bytes = std::fs::read(dir.join("encrypted.pdf")).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("Private metadata"));
    assert!(auto(&dir).call("doc_open", &json!({"path": "encrypted.pdf"})).is_err());
    ok(&mut a, "edit_undo", json!({"doc": doc}));
    assert_eq!(ok(&mut a, "doc_info", json!({"doc": doc}))["security"]["protected"], false);
    ok(&mut a, "edit_redo", json!({"doc": doc}));
    assert_eq!(ok(&mut a, "doc_info", json!({"doc": doc}))["security"]["protected"], true);
    let before = a.session().get(pdfcraft_engine::DocId(doc)).unwrap().bytes.clone();
    for args in [
        json!({"doc": doc, "recipients": []}),
        json!({"doc": doc, "recipients": [{"certificate": "recipient.der", "permissions": 0}]}),
        json!({"doc": doc, "recipients": [{"certificate": "recipient.der"}, {"certificate": "recipient.der"}]}),
        json!({"doc": doc, "recipients": [{"certificate": "../outside.der"}]}),
        json!({"doc": doc, "recipients": [{"certificate": "recipient.der", "pin": "do-not-echo"}]}),
    ] {
        assert!(a.call("doc_protect_certificates", &args).is_err());
        assert!(std::sync::Arc::ptr_eq(&a.session().get(pdfcraft_engine::DocId(doc)).unwrap().bytes, &before));
    }
}

#[test]
fn certificate_tools_reject_pin_arguments_and_confine_paths_before_hardware_access() {
    let dir = workdir("certificate-paths");
    let mut a = auto(&dir);
    for key in ["pin", "password", "pin_env"] {
        let mut args = json!({"path": "a.pdf", "id": "pkcs11:absent"});
        args[key] = json!("do-not-echo-this-secret");
        let error = a.call("doc_open_token", &args).unwrap_err().to_string();
        assert!(!error.contains("do-not-echo-this-secret"));
    }
    let error = a.call("doc_open_token", &json!({"path": "../secret.pdf", "id": "pkcs11:absent"})).unwrap_err().to_string();
    assert!(error.contains("outside the allowed directory"), "{error}");
    assert!(a.session().docs().is_empty());
}

/// This provisions a fresh test token in the explicitly supplied disposable store.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires disposable SOFTHSM2_CONF, PDFCRAFT_SOFTHSM_UTIL, PDFCRAFT_PKCS11_MODULE, and PDFCRAFT_PKCS11_PIN=1234"]
fn certificate_tools_round_trip_through_softhsm() {
    use std::process::Command;
    let module = std::env::var("PDFCRAFT_PKCS11_MODULE").expect("SoftHSM module");
    assert!(module.to_ascii_lowercase().contains("softhsm"));
    let util = std::env::var("PDFCRAFT_SOFTHSM_UTIL").expect("SoftHSM utility");
    assert!(std::env::var("SOFTHSM2_CONF").is_ok());
    assert_eq!(std::env::var("PDFCRAFT_PKCS11_PIN").unwrap(), "1234");
    let dir = workdir("certificate-softhsm");
    let identity = identity();
    assert!(identity.certificate.signing_problem().is_some(), "this card can only decrypt");
    std::fs::write(dir.join("key.der"), identity.key.pkcs8()).unwrap();
    std::fs::write(dir.join("cert.der"), &identity.certificate.raw).unwrap();
    let fingerprint = identity.certificate.fingerprint();
    let label = format!("pdfcraft-{}", fingerprint.chars().filter(|c| c.is_ascii_alphanumeric()).take(16).collect::<String>());
    let status = Command::new(&util)
        .args(["--module", &module, "--init-token", "--free", "--label", &label, "--so-pin", "12345678", "--pin", "1234"])
        .output()
        .unwrap();
    assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
    for (file, kind, extra) in [("key.der", "privkey", vec!["--usage-decrypt", "--always-auth"]), ("cert.der", "cert", vec![])] {
        let output = Command::new("pkcs11-tool")
            .args(["--module", &module, "--token-label", &label, "--login", "--pin", "1234", "--write-object"])
            .arg(dir.join(file))
            .args(["--type", kind, "--id", "03"])
            .args(extra)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
    let mut a = auto(&dir);
    let reference = pdfcraft_engine::sign::pkcs11::reference(&identity.certificate);
    let listing = ok(&mut a, "encryption_token_ids", json!({}));
    assert!(listing["ids"].as_array().unwrap().iter().any(|i| i["id"] == reference && i["token_label"] == label));
    let doc = ok(&mut a, "doc_open", json!({"path": "a.pdf"}))["doc"].as_u64().unwrap();
    ok(&mut a, "doc_set_info", json!({"doc": doc, "key": "Title", "value": "Token protected metadata"}));
    ok(&mut a, "doc_protect_certificates", json!({"doc": doc, "recipients": [{"certificate": reference}]}));
    ok(&mut a, "doc_save", json!({"doc": doc, "path": "encrypted.pdf"}));
    let mut b = auto(&dir);
    let opened = ok(&mut b, "doc_open_token", json!({"path": "encrypted.pdf", "id": reference}))["doc"].as_u64().unwrap();
    assert!(ok(&mut b, "text_extract", json!({"doc": opened})).to_string().contains("Page 1"));
    assert!(b.call("page_render", &json!({"doc": opened, "page": 1, "dpi": 36})).unwrap().iter().any(|c| matches!(c, Content::Png { .. })));
    ok(&mut b, "doc_set_info", json!({"doc": opened, "key": "Title", "value": "Still encrypted"}));
    ok(&mut b, "doc_save", json!({"doc": opened, "path": "updated.pdf"}));
    let updated = std::fs::read(dir.join("updated.pdf")).unwrap();
    assert!(!String::from_utf8_lossy(&updated).contains("Still encrypted"));
    let mut c = auto(&dir);
    let reopened = ok(&mut c, "doc_open_token", json!({"path": "updated.pdf", "id": reference}))["doc"].as_u64().unwrap();
    assert!(ok(&mut c, "text_extract", json!({"doc": reopened})).to_string().contains("Page 1"));
}
