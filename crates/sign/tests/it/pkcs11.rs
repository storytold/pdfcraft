//! Signing with keys on a PKCS #11 token, against SoftHSM.
//!
//! SoftHSM finds its tokens through `SOFTHSM2_CONF`, which must be set before the module loads,
//! and a test can't set the environment of its own process (`set_var` is unsafe). So
//! `signing_on_a_softhsm_token` builds a token in a temporary directory with SoftHSM's and
//! OpenSC's tools, then runs `on_the_token` in a child copy of this test binary with the
//! variable set. Without SoftHSM, `pkcs11-tool` or `openssl` the test is skipped, unless
//! `PDFCRAFT_REQUIRE_SOFTHSM=1` (CI on Linux) makes that a failure.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use pdfcraft_cos::Document;
use pdfcraft_sign::pkcs11::{self, Pin};
use pdfcraft_sign::{SignOptions, Status, TrustStore, pkcs12, signatures};

const CHILD: &str = "PDFCRAFT_PKCS11_TEST_MODULE";
const PIN: &str = "123456";

fn data(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name)
}

fn softhsm_module() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("PDFCRAFT_SOFTHSM_MODULE") {
        return Some(PathBuf::from(p));
    }
    [
        "/usr/lib/softhsm/libsofthsm2.so",
        "/usr/lib/x86_64-linux-gnu/softhsm/libsofthsm2.so",
        "/usr/local/lib/softhsm/libsofthsm2.so",
        "/opt/homebrew/lib/softhsm/libsofthsm2.so",
    ]
    .iter()
    .map(PathBuf::from)
    .find(|p| p.is_file())
}

fn run(cmd: &mut Command) -> Vec<u8> {
    let out = cmd.output().unwrap_or_else(|e| panic!("{cmd:?}: {e}"));
    assert!(out.status.success(), "{cmd:?}: {}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}

/// The key, the signer's certificate and the CA certificates of a test `.p12`, as DER.
fn split_p12(p12: &Path, dir: &Path, stem: &str) -> (PathBuf, PathBuf, Option<PathBuf>) {
    let pem = |what: &[&str]| run(Command::new("openssl").arg("pkcs12").arg("-in").arg(p12).args(["-passin", "pass:test", "-nodes"]).args(what));
    let der = |kind: &str, pem: Vec<u8>, path: &Path| {
        let src = dir.join(format!("{stem}.{kind}.pem"));
        std::fs::write(&src, pem).unwrap();
        let tool = if kind == "key" { "pkey" } else { "x509" };
        run(Command::new("openssl").arg(tool).arg("-in").arg(&src).args(["-outform", "DER", "-out"]).arg(path));
    };
    let (key, cert, ca) = (dir.join(format!("{stem}.key.der")), dir.join(format!("{stem}.crt.der")), dir.join(format!("{stem}.ca.der")));
    der("key", pem(&["-nocerts"]), &key);
    der("crt", pem(&["-nokeys", "-clcerts"]), &cert);
    let cas = pem(&["-nokeys", "-cacerts"]);
    let ca = (!cas.is_empty()).then(|| {
        der("ca", cas, &ca);
        ca
    });
    (key, cert, ca)
}

#[test]
fn signing_on_a_softhsm_token() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    let required = std::env::var_os("PDFCRAFT_REQUIRE_SOFTHSM").is_some_and(|v| v == "1");
    let tools = ["softhsm2-util", "pkcs11-tool", "openssl"].iter().all(|t| Command::new(t).arg("--help").output().is_ok());
    let Some(module) = softhsm_module().filter(|_| tools) else {
        assert!(!required, "PDFCRAFT_REQUIRE_SOFTHSM=1 but SoftHSM, pkcs11-tool or openssl is missing");
        eprintln!("skipped: SoftHSM, pkcs11-tool or openssl is not installed");
        return;
    };
    let dir = std::env::temp_dir().join(format!("pdfcraft-softhsm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("tokens")).unwrap();
    let conf = dir.join("softhsm2.conf");
    std::fs::write(&conf, format!("directories.tokendir = {}\nobjectstore.backend = file\nlog.level = ERROR\n", dir.join("tokens").display()))
        .unwrap();
    let env = |c: &mut Command| -> Vec<u8> { run(c.env("SOFTHSM2_CONF", &conf)) };
    env(Command::new("softhsm2-util").args(["--init-token", "--free", "--label", "PdfCraft Test Card", "--so-pin", "87654321", "--pin", PIN]));
    // The RSA signer with its root CA (as a card carries the national hierarchy), and a P-256 signer.
    for (file, id, ca_id) in [("chain.p12", "01", "10"), ("ec-p256.p12", "02", "")] {
        let (key, cert, ca) = split_p12(&data(file), &dir, id);
        let write = |path: &Path, kind: &str, id: &str, label: &str| {
            env(Command::new("pkcs11-tool")
                .arg("--module")
                .arg(&module)
                .args(["--token-label", "PdfCraft Test Card", "--login", "--pin", PIN, "--write-object"])
                .arg(path)
                .args(["--type", kind, "--id", id, "--label", label]));
        };
        write(&key, "privkey", id, &format!("key {id}"));
        write(&cert, "cert", id, &format!("cert {id}"));
        if let Some(ca) = ca {
            write(&ca, "cert", ca_id, "root");
        }
    }
    let exe = std::env::current_exe().unwrap();
    let out = Command::new(exe)
        .args(["pkcs11::on_the_token", "--exact", "--ignored", "--nocapture"])
        .env("SOFTHSM2_CONF", &conf)
        .env(CHILD, &module)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && text.contains("1 passed"), "{text}");
}

fn fixture() -> Vec<u8> {
    let objs: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        b"<< /Length 44 >>\nstream\nBT /F1 14 Tf 20 250 Td (Contract text) Tj ET\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
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

#[test]
#[ignore = "run by signing_on_a_softhsm_token, with SOFTHSM2_CONF pointing at its token"]
fn on_the_token() {
    let Some(module) = std::env::var_os(CHILD).map(PathBuf::from) else { return };
    let rsa = pkcs12::open(&std::fs::read(data("chain.p12")).unwrap(), "test").unwrap();
    let ec = pkcs12::open(&std::fs::read(data("ec-p256.p12")).unwrap(), "test").unwrap();

    // Listing reads the certificates without logging in.
    let listed = pkcs11::identities(&module).unwrap();
    assert_eq!(listed.len(), 3, "{listed:?}");
    assert!(listed.iter().all(|c| c.token.label == "PdfCraft Test Card" && !c.token.pin_locked));
    let signers: Vec<_> = listed.iter().filter(|c| c.for_signing).map(|c| &c.certificate).collect();
    assert_eq!(signers.len(), 2, "the root CA is not a signing identity");
    assert!(signers.contains(&&rsa.certificate) && signers.contains(&&ec.certificate));

    // A wrong PIN fails without echoing it; no PIN fails on a reader without a PIN pad.
    let wrong = pkcs11::open(&module, &pkcs11::reference(&rsa.certificate), Some(Pin::from("999999"))).unwrap_err().to_string();
    assert!(wrong.contains("PIN is incorrect") && !wrong.contains("999999"), "{wrong}");
    let none = pkcs11::open(&module, &pkcs11::reference(&rsa.certificate), None).unwrap_err().to_string();
    assert!(none.contains("a PIN is needed"), "{none}");
    let missing = pkcs11::open(&module, "pkcs11:00", Some(Pin::from(PIN))).unwrap_err().to_string();
    assert!(missing.contains("no token holds"), "{missing}");

    // While a key is logged in, opening again still checks the PIN.
    let held = pkcs11::open(&module, &pkcs11::reference(&ec.certificate), Some(Pin::from(PIN))).unwrap();
    let again = pkcs11::open(&module, &pkcs11::reference(&rsa.certificate), Some(Pin::from("999999"))).unwrap_err().to_string();
    assert!(again.contains("PIN is incorrect"), "{again}");
    drop(held);

    // Signing on the token: by reference (RSA, with the card's CA) and by common name (ECDSA).
    for (by, expected, anchor) in
        [(pkcs11::reference(&rsa.certificate), &rsa, rsa.chain[0].clone()), ("Test Signer EC".to_string(), &ec, ec.certificate.clone())]
    {
        let id = pkcs11::open(&module, &by, Some(Pin::from(PIN))).unwrap();
        assert!(id.key.is_external(), "the key stays on the token");
        assert_eq!(id.certificate, expected.certificate);
        assert_eq!(id.chain, expected.chain, "the card's CA certificates, not its other end-entity certificates");
        let opts = SignOptions { page: 0, date: "D:20261010120000-06'00'".into(), ..SignOptions::default() };
        let signed = pdfcraft_sign::sign(&open(&fixture()), &id, &opts).unwrap();
        let sigs = signatures(&open(&signed), &signed, &TrustStore { certs: vec![anchor] });
        let s = sigs.iter().find(|s| s.signed).unwrap();
        assert_eq!(s.status, Status::Valid, "{by}: {:?}", s.details);
        assert_eq!(s.signer.as_deref(), expected.certificate.subject.common_name());

        // poppler agrees that the signature itself is valid (it doesn't trust the test root).
        let probe = Command::new("pdfsig").arg("-v").output();
        if probe.is_ok() {
            let file = std::env::temp_dir().join(format!("pdfcraft-pkcs11-{}-{}.pdf", std::process::id(), by.len()));
            std::fs::write(&file, &signed).unwrap();
            let out = Command::new("pdfsig").arg(&file).output().unwrap();
            let _ = std::fs::remove_file(&file);
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(text.contains("Signature Validation: Signature is Valid."), "{text}");
        }
    }
}
