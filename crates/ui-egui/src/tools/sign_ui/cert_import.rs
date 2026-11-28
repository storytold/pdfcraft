//! Signatures panel ▸ Import certificates…: read certificate files on a worker, then let the user confirm which of
//! their certificates to trust. Nothing is trusted until then, since a trusted root vouches for every signature
//! under it.

use craft::ui::choice::Checkbox;
use craft::ui::contract::{Bind, Props as _, Scroll, Show as _, ShowContainer as _};
use craft::ui::dialog::Dialog;
use craft::ui::display::{Shade, Text};
use craft::ui::layout_components::Stack;
use craft::ui::sections::{DialogRole, FooterButton};

use super::panel::hex;
use super::{Certificate, PdfCraftApp, sign};

/// The files Import certificates… offers: single certificates (DER or PEM) and PKCS #7 bundles.
#[cfg(not(target_arch = "wasm32"))]
pub const CERTIFICATE_EXTENSIONS: &[&str] = &["cer", "crt", "pem", "der", "p7b", "p7c"];

/// The largest file Import certificates… reads. A national PKI's whole bundle is a few hundred
/// kilobytes; anything far larger is not a certificate file.
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_CERTIFICATE_FILE_BYTES: u64 = 8 << 20;

/// The most files one Import certificates… reads: ICP-Brasil's whole set is a few hundred.
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_CERTIFICATE_FILES: usize = 2000;

/// The most bytes one Import certificates… reads in all, whatever the number of files.
#[cfg(not(target_arch = "wasm32"))]
pub const MAX_CERTIFICATE_IMPORT_BYTES: u64 = 32 << 20;

/// The footer index of Trust selected; Cancel comes after it.
const TRUST: usize = 0;

/// Import certificates…: the certificates read from the chosen files and which of them to trust.
#[derive(Clone, Debug)]
pub struct CertImport {
    /// The file they came from, as shown to the user.
    pub file: String,
    /// Each certificate once, in file order.
    pub certs: Vec<Certificate>,
    /// Whether each of `certs` is ticked to be trusted.
    pub chosen: Vec<bool>,
    /// Each of `certs`' SHA-256 fingerprint as shown, computed once rather than every frame.
    pub fingerprints: Vec<String>,
}

impl CertImport {
    /// An empty list read from `file`.
    pub fn new(file: &str) -> CertImport {
        CertImport { file: file.to_string(), certs: Vec::new(), chosen: Vec::new(), fingerprints: Vec::new() }
    }

    /// Add `c` unless it is listed already. Authorities start ticked, end-entity certificates
    /// don't: trusting a person's certificate is the rarer, deliberate choice.
    pub fn push(&mut self, c: Certificate) {
        if self.certs.iter().any(|x| x.raw == c.raw) {
            return;
        }
        self.chosen.push(c.is_ca);
        self.fingerprints.push(hex(&sign::keys::DigestAlg::Sha256.digest(&[&c.raw])));
        self.certs.push(c);
    }

    /// The certificates in `bytes` (DER, PEM or a `.p7b`/`.p7c` bundle) from `file`.
    pub fn read(file: &str, bytes: &[u8]) -> Result<CertImport, String> {
        let mut v = CertImport::new(file);
        for c in sign::x509::load_certificates(bytes).map_err(|e| e.to_string())? {
            v.push(c);
        }
        Ok(v)
    }

    /// Read `path`, refusing a file too large to be a certificate file before reading it.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file(path: &std::path::Path) -> Result<CertImport, String> {
        Self::from_files(std::slice::from_ref(&path.to_path_buf()))
    }

    /// Read every file in `paths` into one list, each certificate once. Each must be a regular
    /// file, and sizes are checked for every file and for all of them together, before any is
    /// read; a file that isn't, is too large or holds no readable certificate refuses the whole
    /// import, naming it.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_files(paths: &[std::path::PathBuf]) -> Result<CertImport, String> {
        if paths.len() > MAX_CERTIFICATE_FILES {
            return Err(format!("{} files: choose at most {MAX_CERTIFICATE_FILES}", paths.len()));
        }
        let mut total: u64 = 0;
        for path in paths {
            let file = path.display().to_string();
            let meta = std::fs::metadata(path).map_err(|e| format!("{file}: {e}"))?;
            // A folder, pipe or device is no certificate file, and opening a pipe could block.
            if !meta.is_file() {
                return Err(format!("{file}: not a file"));
            }
            if meta.len() > MAX_CERTIFICATE_FILE_BYTES {
                return Err(format!("{file}: too large for a certificate file"));
            }
            total = total.saturating_add(meta.len());
        }
        if total > MAX_CERTIFICATE_IMPORT_BYTES {
            return Err(format!("{} files: together too large to be certificate files", paths.len()));
        }
        let label = match paths {
            [one] => one.display().to_string(),
            _ => format!("{} files", paths.len()),
        };
        let mut all = CertImport::new(&label);
        for path in paths {
            let file = path.display().to_string();
            // Read no more than the size checked above, should the file have grown since.
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(
                &mut std::io::Read::take(std::fs::File::open(path).map_err(|e| format!("{file}: {e}"))?, MAX_CERTIFICATE_FILE_BYTES + 1),
                &mut bytes,
            )
            .map_err(|e| format!("{file}: {e}"))?;
            if bytes.len() as u64 > MAX_CERTIFICATE_FILE_BYTES {
                return Err(format!("{file}: too large for a certificate file"));
            }
            for c in Self::read(&file, &bytes).map_err(|e| format!("{file}: {e}"))?.certs {
                all.push(c);
            }
        }
        Ok(all)
    }

    /// The ticked certificates that `trusted` doesn't already hold.
    pub fn picked(&self, trusted: &[Certificate]) -> Vec<Certificate> {
        self.picked_iter(trusted).cloned().collect()
    }

    /// How many [`Self::picked`] would return, without cloning them (the dialog asks every frame).
    pub fn picked_count(&self, trusted: &[Certificate]) -> usize {
        self.picked_iter(trusted).count()
    }

    fn picked_iter<'a>(&'a self, trusted: &'a [Certificate]) -> impl Iterator<Item = &'a Certificate> + 'a {
        self.certs.iter().zip(&self.chosen).filter(|(c, chosen)| **chosen && !trusted.iter().any(|t| t.raw == c.raw)).map(|(c, _)| c)
    }
}

/// The dialog's content: each certificate with who it names, who issued it, its validity and SHA-256 fingerprint
/// (to compare with the one the authority publishes), and a tick.
fn body(ui: &mut egui::Ui, v: &mut CertImport, trusted: &[Certificate]) {
    Text::new(crate::i18n::fmt(tl!("Certificates in {file}"), &[("file", &v.file)])).shade(Shade::Muted).show(ui);
    Text::new(tl!(
        "Signatures made under a trusted certificate are shown as valid. Trust only certificates from a source you trust, and compare each fingerprint with the one its authority publishes."
    ))
    .show(ui);
    let muted = |s: String| Text::new(s).shade(Shade::Muted);
    Stack::new().scroll(Scroll::vertical()).show(ui, |ui, _| {
        for (i, c) in v.certs.iter().enumerate() {
            let already = trusted.iter().any(|x| x.raw == c.raw);
            if already {
                Checkbox::new(Bind::bound(&mut true)).label(c.display_name()).enabled(false).show(ui);
                muted(tl!("Already trusted").to_string()).show(ui);
            } else if let Some(chosen) = v.chosen.get_mut(i) {
                Checkbox::new(Bind::bound(chosen)).label(c.display_name()).show(ui);
            }
            muted(tl!(if c.is_ca { "Certificate authority" } else { "End entity" }).to_string()).show(ui);
            muted(crate::i18n::fmt(tl!("Issued by {issuer}"), &[("issuer", &c.issuer.display())])).show(ui);
            muted(crate::i18n::fmt(tl!("Valid from {from} to {to}"), &[("from", &c.not_before.to_string()), ("to", &c.not_after.to_string())]))
                .show(ui);
            muted(format!("SHA-256 {}", v.fingerprints.get(i).map(String::as_str).unwrap_or_default())).show(ui);
        }
    });
}

/// Shows Import Certificates while the app holds what a read found. Trust selected trusts the ticked certificates
/// not trusted yet; it, Cancel, Escape and the close action all drop what was read.
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let trusted = app.session.trusted_certificates().to_vec();
    let Some(v) = app.cert_import.as_mut() else { return };
    let picked = v.picked_count(&trusted);
    let label = crate::i18n::fmt(tl!("Trust selected ({n})"), &[("n", &picked.to_string())]);
    let shown = Dialog::new(tl!("Import Certificates"))
        .buttons([FooterButton::new(label, DialogRole::Accept).enabled(picked > 0), FooterButton::new(tl!("Cancel"), DialogRole::Cancel)])
        .show(ui, |ui, _| body(ui, v, &trusted));
    let trust = if shown.output.picked == Some(TRUST) { v.picked(&trusted) } else { Vec::new() };
    if shown.output.closed.is_some() || shown.output.picked.is_some() {
        app.cert_import = None;
    }
    if !trust.is_empty() {
        let added = app.trust_certificates(trust);
        app.notify_fmt("Added {n} to your trusted certificates.", &[("n", &added.to_string())]);
    }
}
