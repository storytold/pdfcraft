//! Check a smart card through its PKCS #11 module with PdfCraft's own card code, without the app.
//!
//! ```text
//! pkcs11_probe [--module PATH] [--sign] [--tsa URL] [--show-names]
//! ```
//!
//! Without `--sign` it never asks for a PIN: it loads the module, lists the cards, their
//! certificates (which one PdfCraft would offer for signing) and the signing mechanisms the card
//! supports. With `--sign` it asks for the PIN once (not echoed), signs a one-page test PDF on
//! the card, checks the signature and writes `pkcs11-probe-signed.pdf` next to it. The signature
//! is dated in the computer's timezone and, with `--tsa URL`, carries an RFC 3161 timestamp from
//! that authority.
//!
//! Names and ID numbers are hidden unless `--show-names` is given, so the output can be shared.

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    if let Err(e) = run() {
        eprintln!("FAILED: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn run() -> Result<(), String> {
    use std::path::PathBuf;

    use pdfcraft_sign::pkcs11;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let sign = args.iter().any(|a| a == "--sign");
    let show = args.iter().any(|a| a == "--show-names");
    let tsa = args.iter().position(|a| a == "--tsa").and_then(|i| args.get(i + 1)).cloned();
    let module = match args.iter().position(|a| a == "--module").and_then(|i| args.get(i + 1)) {
        Some(m) => PathBuf::from(m),
        None => pkcs11::candidate_modules().into_iter().next().ok_or("no known card module found; pass --module PATH")?,
    };
    println!("module: {}", module.display());

    let certs = pkcs11::identities(&module).map_err(|e| format!("listing the cards: {e}"))?;
    println!("certificates found: {}", certs.len());
    if certs.is_empty() {
        return Err("no certificate on any card; is the card in the reader?".into());
    }
    // A name like "JANE DOE (SIGNING)" keeps only its role in parentheses unless --show-names.
    let shown = |name: String| -> String {
        if show {
            return name;
        }
        match (name.rfind('('), name.ends_with(')')) {
            (Some(i), true) => format!("[name hidden] {}", &name[i..]),
            _ => "[name hidden]".into(),
        }
    };
    let mut seen_tokens = Vec::new();
    for (i, c) in certs.iter().enumerate() {
        let t = &c.token;
        if !seen_tokens.contains(&(&t.label, &t.model)) {
            seen_tokens.push((&t.label, &t.model));
            println!(
                "card: label={:?} manufacturer={:?} model={:?} pin_pad={} pin_locked={} pin_final_try={}",
                if show { t.label.clone() } else { "[hidden]".into() },
                t.manufacturer,
                t.model,
                t.protected_authentication_path,
                t.pin_locked,
                t.pin_final_try
            );
        }
        let cert = &c.certificate;
        println!(
            "  [{i}] {} | issuer: {} | key: {} | keyUsage: {} | is_ca: {} | expires: {:04}-{:02}-{:02} | offered for signing: {}",
            shown(cert.display_name()),
            cert.issuer.common_name().unwrap_or("?"),
            cert.public_key.describe(),
            cert.key_usage.map_or("none".to_string(), |u| format!("{u:#06b}")),
            cert.is_ca,
            cert.not_after.year,
            cert.not_after.month,
            cert.not_after.day,
            c.for_signing,
        );
    }
    match pkcs11::mechanism_names(&module) {
        Ok(m) => println!("signing mechanisms: {}", m.join(", ")),
        Err(e) => println!("signing mechanisms: couldn't be read ({e})"),
    }

    let signers: Vec<_> = certs.iter().filter(|c| c.for_signing).collect();
    let Some(signer) = signers.first() else { return Err("no certificate would be offered for signing".into()) };
    if signers.len() > 1 {
        println!("note: {} certificates would be offered; --sign uses the first", signers.len());
    }
    if !sign {
        println!("OK: the card is readable. Run again with --sign to sign a test PDF (asks for the PIN once).");
        return Ok(());
    }

    if signer.token.pin_final_try {
        return Err("the card says one more wrong PIN locks it; not trying".into());
    }
    let reference = pkcs11::reference(&signer.certificate);
    let pin = if signer.token.protected_authentication_path {
        println!("enter the PIN on the reader's keypad");
        None
    } else {
        let p = rpassword::prompt_password(format!("PIN for {} (not shown while typing): ", shown(signer.certificate.display_name())))
            .map_err(|e| format!("reading the PIN: {e}"))?;
        Some(pkcs11::Pin::from(p))
    };
    let id = pkcs11::open(&module, &reference, pin).map_err(|e| format!("logging in: {e}"))?;
    println!("logged in; chain certificates from the card: {}", id.chain.len());

    let doc = pdfcraft_cos::Document::open(std::sync::Arc::new(test_pdf())).map_err(|e| e.to_string())?;
    let opts = pdfcraft_sign::SignOptions {
        page: 0,
        rect: Some([40.0, 40.0, 260.0, 100.0]),
        reason: Some("PdfCraft smart card test".into()),
        date: local_pdf_date(),
        ..Default::default()
    };
    println!("signing time: {}", opts.date);
    let signed = match &tsa {
        Some(url) => {
            println!("timestamp authority: {url}");
            pdfcraft_sign::sign_with_timestamp(&doc, &id, &opts, &HttpTsa { url: url.clone() })
                .map_err(|e| format!("signing with a timestamp: {e} (leave out --tsa to sign without one)"))?
        }
        None => pdfcraft_sign::sign(&doc, &id, &opts).map_err(|e| format!("signing: {e}"))?,
    };
    let out = PathBuf::from("pkcs11-probe-signed.pdf");
    std::fs::write(&out, &signed).map_err(|e| format!("{}: {e}", out.display()))?;
    let signed_doc = pdfcraft_cos::Document::open(std::sync::Arc::new(signed.clone())).map_err(|e| e.to_string())?;
    let trust = pdfcraft_sign::TrustStore { certs: id.chain.clone() };
    for s in pdfcraft_sign::signatures(&signed_doc, &signed, &trust).iter().filter(|s| s.signed) {
        println!("signature: {:?}", s.status);
        if let Some(t) = &s.timestamp_time {
            println!("  trusted timestamp: {t:?}");
        }
        for d in &s.details {
            println!("  {}", if show { d.message.clone() } else { d.message.replace(&id.certificate.display_name(), "[name hidden]") });
        }
    }
    println!("OK: signed on the card; wrote {}. Open it in Acrobat to compare.", out.display());
    Ok(())
}

/// RFC 3161 over HTTP (RFC 3161 §3.4): POST the request, read the reply (capped at 1 MiB).
#[cfg(not(target_arch = "wasm32"))]
struct HttpTsa {
    url: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl pdfcraft_sign::TimestampAuthority for HttpTsa {
    fn timestamp(&self, request: &[u8]) -> Result<Vec<u8>, pdfcraft_sign::SignError> {
        let failed = |e: String| pdfcraft_sign::SignError::Crypto(format!("{}: {e}", self.url));
        let agent = ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(20))).build().new_agent();
        let mut response = agent
            .post(&self.url)
            .header("Content-Type", "application/timestamp-query")
            .send(request)
            .map_err(|e| failed(format!("no answer ({e})")))?;
        response.body_mut().with_config().limit(1 << 20).read_to_vec().map_err(|e| failed(format!("unreadable answer ({e})")))
    }
}

/// Now as a PDF date in this computer's timezone, e.g. `D:20261012093000-06'00'`.
#[cfg(not(target_arch = "wasm32"))]
fn local_pdf_date() -> String {
    let now = chrono::Local::now();
    let offset = now.offset().local_minus_utc() / 60;
    let (sign, m) = if offset < 0 { ('-', -offset) } else { ('+', offset) };
    format!("{}{sign}{:02}'{:02}'", now.format("D:%Y%m%d%H%M%S"), m / 60, m % 60)
}

/// One page saying what it is.
#[cfg(not(target_arch = "wasm32"))]
fn test_pdf() -> Vec<u8> {
    let objs: [&[u8]; 5] = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        b"<< /Length 49 >>\nstream\nBT /F1 14 Tf 20 150 Td (PdfCraft card test) Tj ET\nendstream",
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
