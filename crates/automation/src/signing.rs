//! Signature tools: list and validate signatures, create a self-signed digital ID, sign a
//! document, and manage trusted certificates. Rectangles are points from the top-left of the
//! displayed page.

use std::path::Path;
use std::sync::Arc;

use pdfcraft_engine::sign::{self, Certificate, DigitalId, Modification, Name, PrivateKey};
use pdfcraft_engine::{DocId, EditError, SignOptions, SignatureInfo, SignatureStatus};
use serde_json::{Value, json};

use crate::{Args, Automation, Result, ToolError, failed, write_atomic};

fn bad(m: impl Into<String>) -> ToolError {
    ToolError::InvalidArgs(m.into())
}

/// A signing error, as the caller's to fix when it is about what they passed: a graphic the signer
/// refuses, a key PdfCraft can't sign with, a signature that doesn't verify.
fn sign_error(e: EditError) -> ToolError {
    const CALLERS: [&str; 3] = ["the signature graphic", "PdfCraft can't sign with", "the signature doesn't match"];
    match e {
        EditError::Sign(m) if CALLERS.iter().any(|c| m.starts_with(c)) => bad(m),
        e => failed(e),
    }
}

/// The longest signature value sign_finish takes, in hex digits (a 16384-bit RSA value is 2048 bytes).
const MAX_SIGNATURE_HEX: usize = 4096;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `signature`: hex digits, two per byte, in either case.
fn unhex(text: &str) -> Result<Vec<u8>> {
    let too_long = || bad(format!("signature is longer than {MAX_SIGNATURE_HEX} hex digits"));
    // Room for a separator after each pair, checked before anything is copied.
    if text.len() > MAX_SIGNATURE_HEX * 2 {
        return Err(too_long());
    }
    // Whitespace and colons between the pairs are ignored.
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace() && *b != b':').collect();
    if digits.len() > MAX_SIGNATURE_HEX {
        return Err(too_long());
    }
    let not_hex = || bad("signature must be hex digits, two per byte");
    if digits.is_empty() || !digits.len().is_multiple_of(2) || !digits.iter().all(u8::is_ascii_hexdigit) {
        return Err(not_hex());
    }
    digits.chunks(2).map(|pair| std::str::from_utf8(pair).ok().and_then(|p| u8::from_str_radix(p, 16).ok()).ok_or_else(not_hex)).collect()
}

/// Seconds since 1970 → a DER time.
fn time(secs: i64) -> sign::Time {
    sign::Time::from_unix(secs)
}

/// The longest label `sign_document` takes (characters): a label is a few words.
const MAX_LABEL_CHARS: usize = 200;

/// `appearance`: which items the visible signature shows; those not given keep the default
/// (name, date and labels on; reason, location and distinguished name off).
fn appearance_arg(v: &Value) -> Result<sign::Appearance> {
    const KEYS: &str = "name, date, reason, location, distinguished_name, labels";
    let o = v.as_object().ok_or_else(|| bad(format!("appearance must be an object of true/false items ({KEYS})")))?;
    let mut shown = sign::Appearance::default();
    for (key, value) in o {
        let on = value.as_bool().ok_or_else(|| bad(format!("appearance.{key} must be true or false")))?;
        let item = match key.as_str() {
            "name" => &mut shown.name,
            "date" => &mut shown.date,
            "reason" => &mut shown.reason,
            "location" => &mut shown.location,
            "distinguished_name" => &mut shown.distinguished_name,
            "labels" => &mut shown.labels,
            other => return Err(bad(format!("unknown appearance item {other:?} ({KEYS})"))),
        };
        *item = on;
    }
    Ok(shown)
}

/// `labels`: the words of the visible signature's labels; those not given keep the English. Each
/// must be drawable in the signature's font. Nobody sees a preview here, so a label it can't draw
/// is refused even when this signature doesn't show it.
fn labels_arg(v: &Value) -> Result<sign::AppearanceLabels> {
    const KEYS: &str = "signed_by, distinguished_name, reason, location, date";
    let o = v.as_object().ok_or_else(|| bad(format!("labels must be an object of strings ({KEYS})")))?;
    let mut labels = sign::AppearanceLabels::default();
    for (key, value) in o {
        let text = value.as_str().ok_or_else(|| bad(format!("labels.{key} must be a string")))?;
        if text.chars().count() > MAX_LABEL_CHARS {
            return Err(bad(format!("labels.{key} is longer than {MAX_LABEL_CHARS} characters")));
        }
        let label = match key.as_str() {
            "signed_by" => &mut labels.signed_by,
            "distinguished_name" => &mut labels.distinguished_name,
            "reason" => &mut labels.reason,
            "location" => &mut labels.location,
            "date" => &mut labels.date,
            other => return Err(bad(format!("unknown label {other:?} ({KEYS})"))),
        };
        *label = text.to_string();
    }
    if let Some((field, c)) = labels.first_undrawable() {
        return Err(bad(format!(
            "labels.{field}: {}",
            pdfcraft_engine::undrawable_message(&format!("the {field} label of the visible signature"), c)
        )));
    }
    Ok(labels)
}

pub(crate) fn cert_json(c: &Certificate) -> Value {
    json!({
        "name": c.display_name(),
        "subject": c.subject.display(),
        "issuer": c.issuer.display(),
        "email": c.subject.email(),
        "serial": c.serial_hex(),
        "valid_from": c.not_before.to_string(),
        "valid_to": c.not_after.to_string(),
        "key": c.public_key.describe(),
        "self_signed": c.is_self_signed(),
        "sha256": c.fingerprint(),
    })
}

impl Automation {
    fn sig_json(&self, a: &Args, s: &SignatureInfo) -> Result<Value> {
        let doc = self.doc(a)?;
        let rect = match (s.page, s.rect) {
            (Some(p), Some(r)) if s.visible => doc.info.pages.get(p).map(|pi| {
                let (a0, a1) = (pi.user_to_view(r[0] as f32, r[3] as f32), pi.user_to_view(r[2] as f32, r[1] as f32));
                json!([a0[0].min(a1[0]), a0[1].min(a1[1]), a0[0].max(a1[0]), a0[1].max(a1[1])])
            }),
            _ => None,
        };
        let (modification, changes) = match &s.modification {
            Modification::None => ("none", Vec::new()),
            Modification::Allowed(c) => ("allowed", c.clone()),
            Modification::Disallowed(c) => ("disallowed", c.clone()),
        };
        Ok(json!({
            "field": s.field,
            "signed": s.signed,
            "status": if !s.signed { "unsigned" } else { match s.status { SignatureStatus::Valid => "valid", SignatureStatus::Unknown => "unknown", SignatureStatus::Invalid => "invalid" } },
            "summary": s.summary(),
            "signer": s.signer,
            "certificate": s.certificate.as_ref().map(cert_json),
            "chain": s.chain.iter().skip(1).map(cert_json).collect::<Vec<_>>(),
            "date": s.date,
            "reason": s.reason,
            "location": s.location,
            "contact": s.contact,
            "certify": s.certify,
            "page": s.page.map(|p| p + 1),
            "rect": rect,
            "visible": s.visible,
            "revision": s.revision,
            "sub_filter": s.sub_filter,
            "algorithm": s.algorithm,
            "timestamp": s.timestamp,
            "timestamp_time": s.timestamp_time.map(|t| t.to_string()),
            "modification": modification,
            "changes": changes,
            // Scripts read this, so the sentences stay in English, whatever the interface reads in.
            "details": s.details.iter().map(|d| d.message.clone()).collect::<Vec<_>>(),
        }))
    }

    pub(crate) fn sign_list(&self, a: &Args) -> Result<Value> {
        let doc = self.doc(a)?;
        let list: Vec<Value> = doc.signatures.iter().map(|s| self.sig_json(a, s)).collect::<Result<_>>()?;
        let signed = doc.signatures.iter().filter(|s| s.signed).count();
        let all_valid = signed > 0 && doc.signatures.iter().filter(|s| s.signed).all(|s| s.status == SignatureStatus::Valid);
        Ok(json!({ "count": list.len(), "signed": signed, "all_valid": all_valid, "signatures": list }))
    }

    pub(crate) fn sign_id_create(&mut self, a: &Args) -> Result<Value> {
        let name = a.str("name")?.trim().to_string();
        if name.is_empty() {
            return Err(bad("name must not be empty"));
        }
        let password = a.str("password")?;
        if password.chars().count() < sign::pkcs12::MIN_PASSWORD_CHARS {
            return Err(bad(format!("the password must have at least {} characters", sign::pkcs12::MIN_PASSWORD_CHARS)));
        }
        let path = self.resolve(a.str("path")?, true)?;
        let key = match a.opt_str("key")?.unwrap_or("rsa2048") {
            "rsa2048" => PrivateKey::generate_rsa(2048),
            "rsa3072" => PrivateKey::generate_rsa(3072),
            "rsa4096" => PrivateKey::generate_rsa(4096),
            "p256" => PrivateKey::generate_p256(),
            other => return Err(bad(format!("unknown key {other:?} (rsa2048, rsa3072, rsa4096, p256)"))),
        }
        .map_err(failed)?;
        let dn = Name::build(
            &name,
            a.opt_str("unit")?.unwrap_or(""),
            a.opt_str("organization")?.unwrap_or(""),
            a.opt_str("email")?.unwrap_or(""),
            a.opt_str("country")?.unwrap_or(""),
        );
        let years = a.opt_int("years")?.unwrap_or(5).clamp(1, 50) as u32;
        let now = self.session.now_secs();
        let serial = sign::keys::DigestAlg::Sha256.digest(&[name.as_bytes(), &now.to_be_bytes()])[..8].to_vec();
        let cert = Certificate::self_signed(&dn, &key, time(now), years, &serial).map_err(failed)?;
        let id = DigitalId { key, certificate: cert, chain: Vec::new(), friendly_name: Some(name) };
        let p12 = sign::pkcs12::write(&id, password).map_err(failed)?;
        write_atomic(&path, &p12)?;
        Ok(json!({ "path": path.to_string_lossy(), "certificate": cert_json(&id.certificate) }))
    }

    fn open_id(&self, a: &Args) -> Result<DigitalId> {
        // A macOS Keychain identity: "keychain:<fingerprint>" or "keychain:<common name>".
        if let Some(r) = a.str("id")?.strip_prefix("keychain:") {
            #[cfg(target_os = "macos")]
            return sign::keychain::find(&format!("keychain:{r}")).map_err(failed);
            #[cfg(not(target_os = "macos"))]
            return Err(failed(format!("keychain:{r}: Keychain identities are only available on macOS")));
        }
        if let Some(r) = a.str("id")?.strip_prefix("windows:") {
            #[cfg(target_os = "windows")]
            return sign::windows::find(&format!("windows:{r}")).map_err(failed);
            #[cfg(not(target_os = "windows"))]
            return Err(failed(format!("windows:{r}: Windows certificate store identities are only available on Windows")));
        }
        let path = self.resolve(a.str("id")?, false)?;
        let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        sign::pkcs12::open(&bytes, a.opt_str("password")?.unwrap_or("")).map_err(|e| match e {
            sign::SignError::WrongPassword => bad("the digital ID password is incorrect"),
            e => failed(e),
        })
    }

    /// The signature `a` asks for (sign_document, sign_prepare): an existing empty field, or a new
    /// one on `page` at `rect`; its reason, location, certification and appearance.
    fn sign_options(&self, a: &Args) -> Result<SignOptions> {
        let doc = self.doc(a)?;
        let field = a.opt_str("field")?.map(str::to_string);
        let page = match a.opt_int("page")? {
            Some(p) if p >= 1 && (p as usize) <= doc.info.pages.len() => p as usize - 1,
            Some(p) => return Err(bad(format!("page {p} does not exist"))),
            None => 0,
        };
        let rect = match a.nums::<4>("rect")? {
            Some(r) => {
                let p = doc.info.pages.get(page).ok_or_else(|| bad("the document has no page to put the signature on"))?;
                let (u0, u1) = (p.view_to_user(r[0] as f32, r[1] as f32), p.view_to_user(r[2] as f32, r[3] as f32));
                let u = [u0[0].min(u1[0]) as f64, u0[1].min(u1[1]) as f64, u0[0].max(u1[0]) as f64, u0[1].max(u1[1]) as f64];
                if u[2] - u[0] < 4.0 || u[3] - u[1] < 4.0 {
                    return Err(bad("the signature rectangle is too small"));
                }
                Some(u)
            }
            None => None,
        };
        let certify = match a.get("certify") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(match s.as_str() {
                "no_changes" => 1,
                "form_fill" => 2,
                "comments" => 3,
                other => return Err(bad(format!("unknown certify level {other:?} (no_changes, form_fill, comments)"))),
            }),
            Some(v) => Some(v.as_u64().filter(|n| (1..=3).contains(n)).ok_or_else(|| bad("certify must be 1, 2 or 3"))? as u8),
        };
        let graphic = match a.get("graphic") {
            None | Some(Value::Null) => None,
            Some(g) if rect.is_none() && field.is_none() => return Err(bad(format!("graphic {g} needs a visible signature: pass rect or field"))),
            Some(g) => Some(self.signature_graphic(g)?),
        };
        let visible = rect.is_some() || field.is_some();
        let appearance = match a.get("appearance") {
            None | Some(Value::Null) => sign::Appearance::default(),
            Some(_) if !visible => return Err(bad("appearance needs a visible signature: pass rect or field")),
            Some(v) => appearance_arg(v)?,
        };
        let appearance_labels = match a.get("labels") {
            None | Some(Value::Null) => sign::AppearanceLabels::default(),
            Some(_) if !visible => return Err(bad("labels needs a visible signature: pass rect or field")),
            Some(v) => labels_arg(v)?,
        };
        Ok(SignOptions {
            field,
            page,
            rect,
            reason: a.opt_str("reason")?.map(str::to_string),
            location: a.opt_str("location")?.map(str::to_string),
            contact: a.opt_str("contact")?.map(str::to_string),
            certify,
            appearance,
            appearance_labels,
            graphic,
            ..SignOptions::default()
        })
    }

    pub(crate) fn sign_document(&mut self, a: &Args) -> Result<Value> {
        let id = self.open_id(a)?;
        let doc_id = self.doc(a)?.id;
        let opts = self.sign_options(a)?;
        let out = self.resolve(a.str("out")?, true)?;
        let signed = self.session.sign(doc_id, &id, opts).map_err(sign_error)?;
        self.save_signed(a, doc_id, &out, signed)
    }

    /// sign_prepare: the first of two signing steps, for a key PdfCraft can't use itself (a
    /// hardware module, a signing service, the browser's Web Crypto). The request waits for
    /// sign_finish; preparing the document again replaces it.
    pub(crate) fn sign_prepare(&mut self, a: &Args) -> Result<Value> {
        let doc_id = self.doc(a)?.id;
        let path = self.resolve(a.str("certificate")?, false)?;
        let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
        let mut certs = sign::x509::load_certificates(&bytes).map_err(|e| bad(format!("certificate: {e}")))?.into_iter();
        let certificate = certs.next().ok_or_else(|| bad("certificate: the file holds no certificate"))?;
        let opts = self.sign_options(a)?;
        let request = self.session.signing_request(doc_id, certificate, certs.collect(), opts).map_err(sign_error)?;
        let scheme = if matches!(request.certificate().public_key, sign::PublicKey::Rsa { .. }) { "RSA PKCS #1 v1.5" } else { "ECDSA" };
        let prepared = json!({
            "to_sign": hex(request.message()),
            "digest": request.digest().name(),
            "scheme": scheme,
            "date": request.date(),
            "certificate": cert_json(request.certificate()),
        });
        // One request per open document: those of closed documents go.
        self.signing.retain(|doc, _| self.session.get(*doc).is_some());
        self.signing.insert(doc_id, request);
        Ok(prepared)
    }

    /// sign_finish: sign with the value the key made over sign_prepare's bytes, and save.
    pub(crate) fn sign_finish(&mut self, a: &Args) -> Result<Value> {
        let doc_id = self.doc(a)?.id;
        let signature = unhex(a.str("signature")?)?;
        let out = self.resolve(a.str("out")?, true)?;
        let request = self.signing.get(&doc_id).ok_or_else(|| bad("this document has no prepared signature: call sign_prepare first"))?;
        let signed = self.session.complete_signing(request, &signature).map_err(sign_error)?;
        // The request goes once the file is written: if `out` can't be written, the same
        // signature can be sent again with another `out`.
        let saved = self.save_signed(a, doc_id, &out, signed)?;
        self.signing.remove(&doc_id);
        Ok(saved)
    }

    /// Write the signed file to `out` and show it as the document's saved state (signing always
    /// saves, as in Acrobat); returns the file and its newest signature.
    fn save_signed(&mut self, a: &Args, doc_id: DocId, out: &Path, signed: Arc<Vec<u8>>) -> Result<Value> {
        write_atomic(out, &signed)?;
        let path = out.to_string_lossy().into_owned();
        self.session.mark_signed(doc_id, signed.clone(), Some(path.clone())).map_err(failed)?;
        let doc = self.doc(a)?;
        let newest = doc.signatures.iter().filter(|s| s.signed).max_by_key(|s| s.revision).cloned();
        Ok(json!({ "path": path, "bytes": signed.len(), "signature": newest.map(|s| self.sig_json(a, &s)).transpose()? }))
    }

    /// `{"image": path}` (PNG or JPEG), `{"text": name}` (typed in the script font) or
    /// `{"strokes": [[[x, y], …], …]}` (y up; the drawing pad is 1 wide).
    fn signature_graphic(&self, g: &Value) -> Result<sign::Graphic> {
        let kinds = "graphic must be an object with exactly one of image (a PNG or JPEG path), text, or strokes";
        let Some(o) = g.as_object().filter(|o| o.len() == 1) else { return Err(bad(kinds)) };
        if let Some(path) = o.get("image") {
            let path = self.resolve(path.as_str().ok_or_else(|| bad("graphic.image must be a path"))?, false)?;
            let file = std::fs::File::open(&path).map_err(|e| bad(format!("graphic.image {}: {e}", path.display())))?;
            return Ok(pdfcraft_engine::SignatureImage::read(file).map_err(|e| bad(e.to_string()))?.graphic());
        }
        if let Some(text) = o.get("text") {
            let text = text.as_str().ok_or_else(|| bad("graphic.text must be a string"))?;
            // Nobody sees a preview here: refuse what the script font and its fallback would leave out.
            if let Some(c) = pdfcraft_engine::script_missing(text) {
                return Err(bad(format!(
                    "the signature script font can't draw \"{}\" (U+{:04X}) in graphic.text; type it in Latin, Cyrillic or Greek letters, or pass graphic.image instead",
                    c.escape_debug(),
                    u32::from(c)
                )));
            }
            return pdfcraft_engine::typed_signature_graphic(text).ok_or_else(|| {
                bad(format!(
                    "graphic.text {text:?} has nothing the script font can draw (at most {} characters)",
                    pdfcraft_engine::MAX_SIGNATURE_CHARS
                ))
            });
        }
        if let Some(strokes) = o.get("strokes") {
            let strokes: Vec<Vec<[f64; 2]>> =
                serde_json::from_value(strokes.clone()).map_err(|e| bad(format!("graphic.strokes must be strokes of [x, y] points: {e}")))?;
            return Ok(sign::Graphic::Strokes(strokes));
        }
        Err(bad(kinds))
    }

    pub(crate) fn sign_trust(&mut self, a: &Args) -> Result<Value> {
        let mut certs: Vec<Certificate> =
            if a.opt_bool("clear")?.unwrap_or(false) { Vec::new() } else { self.session.trusted_certificates().to_vec() };
        let paths: Vec<String> = match a.get("paths") {
            None => Vec::new(),
            Some(v) => v
                .as_array()
                .ok_or_else(|| bad("paths must be an array"))?
                .iter()
                .map(|p| p.as_str().map(str::to_string).ok_or_else(|| bad("paths must be strings")))
                .collect::<Result<_>>()?,
        };
        for p in paths {
            let path = self.resolve(&p, false)?;
            let bytes = std::fs::read(&path).map_err(|e| failed(format!("{}: {e}", path.display())))?;
            let lower = p.to_ascii_lowercase();
            let found = if lower.ends_with(".p12") || lower.ends_with(".pfx") {
                let id = sign::pkcs12::open(&bytes, a.opt_str("password")?.unwrap_or("")).map_err(failed)?;
                std::iter::once(id.certificate).chain(id.chain).collect()
            } else {
                sign::x509::load_certificates(&bytes).map_err(failed)?
            };
            for c in found {
                if !certs.iter().any(|t| t.raw == c.raw) {
                    certs.push(c);
                }
            }
        }
        self.session.set_trusted_certificates(certs);
        Ok(json!({ "trusted": self.session.trusted_certificates().iter().map(cert_json).collect::<Vec<_>>() }))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pdfcraft_engine::sign::DigestAlg;

    use super::*;
    use crate::Content;

    const PDF: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >> endobj
trailer << /Root 1 0 R >>
%%EOF";

    fn id(file: &str) -> DigitalId {
        let path = format!("{}/../sign/tests/data/{file}", env!("CARGO_MANIFEST_DIR"));
        sign::pkcs12::open(&std::fs::read(path).unwrap(), "test").unwrap()
    }

    /// A folder with a.pdf, the RSA test ID and its certificate, and the P-256 test certificate.
    fn workdir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfcraft-sign-steps-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.pdf"), PDF).unwrap();
        let data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../sign/tests/data");
        std::fs::copy(data.join("rsa-aes.p12"), dir.join("rsa.p12")).unwrap();
        std::fs::copy(data.join("rsa.crt.pem"), dir.join("rsa.crt.pem")).unwrap();
        std::fs::write(dir.join("ec.cer"), &id("ec-p256.p12").certificate.raw).unwrap();
        dir
    }

    /// After the test certificates' start date (2026-10-02).
    fn automation(dir: &Path) -> Automation {
        Automation::new().with_root(dir).unwrap().with_clock(|| 1_800_000_000)
    }

    fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
        match a.call(tool, &args) {
            Ok(content) => match content.into_iter().next() {
                Some(Content::Json(v)) => v,
                other => panic!("{tool}: {other:?}"),
            },
            Err(e) => panic!("{tool}: {e}"),
        }
    }

    fn open(a: &mut Automation) -> u64 {
        ok(a, "doc_open", json!({ "path": "a.pdf" }))["doc"].as_u64().unwrap()
    }

    /// What the outside key does: sign `to_sign` with the digest it names.
    fn outside_sign(key: &PrivateKey, prepared: &Value) -> Vec<u8> {
        let digest = match prepared["digest"].as_str() {
            Some("SHA-384") => DigestAlg::Sha384,
            _ => DigestAlg::Sha256,
        };
        key.sign(digest, &unhex(prepared["to_sign"].as_str().unwrap()).unwrap()).unwrap()
    }

    #[test]
    fn an_outside_rsa_key_signs_in_two_steps_the_file_sign_document_writes() {
        let dir = workdir("rsa");
        let mut a = automation(&dir);
        let doc = open(&mut a);
        let options = json!({ "doc": doc, "page": 1, "rect": [20, 20, 180, 60], "reason": "Approved" });
        let mut prepare = options.clone();
        prepare["certificate"] = json!("rsa.crt.pem");
        let prepared = ok(&mut a, "sign_prepare", prepare);
        assert_eq!((prepared["digest"].as_str(), prepared["scheme"].as_str()), (Some("SHA-256"), Some("RSA PKCS #1 v1.5")));
        assert_eq!(prepared["certificate"]["name"], "Test Signer RSA");
        assert!(!dir.join("signed.pdf").exists(), "preparing writes nothing");
        let signature = hex(&outside_sign(&id("rsa-aes.p12").key, &prepared));
        let r = ok(&mut a, "sign_finish", json!({ "doc": doc, "signature": signature, "out": "signed.pdf" }));
        assert_eq!((r["signature"]["signer"].as_str(), r["signature"]["reason"].as_str()), (Some("Test Signer RSA"), Some("Approved")));
        // The same options and clock through sign_document, with the key in a .p12: the same bytes.
        let mut b = automation(&dir);
        let other = open(&mut b);
        let mut document = options;
        (document["doc"], document["id"], document["password"], document["out"]) =
            (json!(other), json!("rsa.p12"), json!("test"), json!("one-step.pdf"));
        ok(&mut b, "sign_document", document);
        assert_eq!(std::fs::read(dir.join("signed.pdf")).unwrap(), std::fs::read(dir.join("one-step.pdf")).unwrap());
        ok(&mut a, "sign_trust", json!({ "paths": ["rsa.crt.pem"] }));
        let list = ok(&mut a, "sign_list", json!({ "doc": doc }));
        assert_eq!((list["all_valid"].as_bool(), list["signatures"][0]["status"].as_str()), (Some(true), Some("valid")), "{list}");
    }

    #[test]
    fn an_outside_ecdsa_key_may_answer_with_raw_r_and_s() {
        let dir = workdir("ecdsa");
        let mut a = automation(&dir);
        let doc = open(&mut a);
        let prepared = ok(&mut a, "sign_prepare", json!({ "doc": doc, "certificate": "ec.cer" }));
        assert_eq!((prepared["digest"].as_str(), prepared["scheme"].as_str()), (Some("SHA-256"), Some("ECDSA")));
        let der = outside_sign(&id("ec-p256.p12").key, &prepared);
        let parts = sign::der::Tlv::parse_all(&der).unwrap().children().unwrap();
        let raw: Vec<u8> =
            parts.iter().flat_map(|i| std::iter::repeat_n(0u8, 32 - i.uint_bytes().len()).chain(i.uint_bytes().iter().copied())).collect();
        // Spaced and in upper case, as tools print it.
        let spaced: Vec<String> = raw.iter().map(|b| format!("{b:02X}")).collect();
        ok(&mut a, "sign_finish", json!({ "doc": doc, "signature": spaced.join(" "), "out": "signed.pdf" }));
        ok(&mut a, "sign_trust", json!({ "paths": ["ec.cer"] }));
        assert_eq!(ok(&mut a, "sign_list", json!({ "doc": doc }))["all_valid"], true);
    }

    #[test]
    fn a_wrong_signature_or_a_changed_document_is_refused_and_nothing_is_written() {
        let dir = workdir("refused");
        let mut a = automation(&dir);
        let doc = open(&mut a);
        let finish = |a: &mut Automation, signature: &str| a.call("sign_finish", &json!({ "doc": doc, "signature": signature, "out": "signed.pdf" }));
        let refused = |r: Result<Vec<Content>>| match r {
            Err(ToolError::InvalidArgs(m) | ToolError::Failed(m)) => m,
            other => panic!("expected a refusal: {other:?}"),
        };
        assert!(refused(finish(&mut a, "00")).contains("call sign_prepare first"));
        let prepared = ok(&mut a, "sign_prepare", json!({ "doc": doc, "certificate": "rsa.crt.pem" }));
        let by_another_key = hex(&outside_sign(&id("chain.p12").key, &prepared));
        for (signature, why) in [(by_another_key.as_str(), "doesn't match"), ("abc", "two per byte"), ("zz", "two per byte"), ("", "two per byte")] {
            let m = refused(finish(&mut a, signature));
            assert!(m.contains(why), "{signature:?}: {m}");
        }
        assert!(refused(finish(&mut a, &"00".repeat(MAX_SIGNATURE_HEX))).contains("longer than"));
        assert!(!dir.join("signed.pdf").exists());
        // The request survives a wrong answer, until the document changes.
        let right = hex(&outside_sign(&id("rsa-aes.p12").key, &prepared));
        ok(&mut a, "page_rotate", json!({ "doc": doc, "degrees": 90 }));
        assert!(refused(finish(&mut a, &right)).contains("changed while it was being signed"));
        assert!(!dir.join("signed.pdf").exists());
        // Prepared again, it signs. An `out` that can't be written (a folder) leaves the request,
        // so the same signature signs to another `out`.
        let prepared = ok(&mut a, "sign_prepare", json!({ "doc": doc, "certificate": "rsa.crt.pem" }));
        let signature = hex(&outside_sign(&id("rsa-aes.p12").key, &prepared));
        std::fs::create_dir_all(dir.join("a-folder")).unwrap();
        assert!(a.call("sign_finish", &json!({ "doc": doc, "signature": signature, "out": "a-folder" })).is_err());
        ok(&mut a, "sign_finish", json!({ "doc": doc, "signature": signature, "out": "signed.pdf" }));
        assert!(dir.join("signed.pdf").exists());
        // A file with no certificate in it.
        std::fs::write(dir.join("empty.pem"), b"no certificate here").unwrap();
        let m = refused(a.call("sign_prepare", &json!({ "doc": doc, "certificate": "empty.pem" })));
        assert!(m.contains("certificate"), "{m}");
    }
}
