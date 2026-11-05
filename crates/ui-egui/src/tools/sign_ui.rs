//! Use a certificate (execution plan M9.6): Digitally sign and Certify (drag a rectangle, or
//! click an empty signature field), the Sign with a Digital ID / Configure New Digital ID /
//! Sign as dialogs, the Signatures panel, and the signature message bar.

#[cfg(not(target_arch = "wasm32"))]
use crate::file_filter::AddFilterAnyCase as _;
use std::path::PathBuf;

use egui::{Align, Color32, CornerRadius, Layout, Pos2, Rect, Stroke, vec2};
use pdfcraft_engine::sign::{self, Appearance, AppearanceLabels, Certificate, DigitalId, Name, PrivateKey};
use pdfcraft_engine::{SignOptions, SignatureImage, SignatureInfo, SignatureStatus};

use crate::canvas::{DocView, PageToolContext, Snapshot, SnapshotKind};
use crate::fill_sign::SavedSig;
use crate::page_cursor::Layer;
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, icons, widgets};

mod dialog;
mod panel;

#[cfg(test)]
use dialog::detail_lines;
use dialog::graphic_of;
pub(crate) use dialog::{appearance_labels, dialog};
pub(crate) use panel::{CertAction, banner, cert_viewer, panel, signature_status_style};
pub use panel::{CertTab, CertViewer, PanelAction, certificate_pem};

/// A digital ID the app knows about (Acrobat: Digital ID files). The file stays where it is;
/// its password is asked for at each signing.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DigitalIdEntry {
    pub path: String,
    pub name: String,
    pub issuer: String,
    pub email: String,
    pub expires: String,
    /// Why the ID can't sign (a Windows store certificate whose key PdfCraft can't use): shown
    /// greyed with the reason, and never selectable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unusable: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignStep {
    /// Sign with a Digital ID: pick one.
    Choose,
    /// Configure a Digital ID for signing: from a file or a new self-signed one.
    Configure,
    /// Sign as "Name": appearance, password, Sign.
    SignAs,
}

/// The key algorithms of a new digital ID (Acrobat offers RSA; P-256 also works in browsers).
pub const KEY_ALGORITHMS: [(&str, &str); 4] =
    [("2048-bit RSA", "rsa2048"), ("3072-bit RSA", "rsa3072"), ("4096-bit RSA", "rsa4096"), ("256-bit ECDSA (P-256)", "p256")];

#[derive(Clone, Debug, PartialEq)]
pub struct NewIdDraft {
    /// `true`: create a self-signed ID; `false`: use an ID from a file.
    pub create: bool,
    pub name: String,
    pub unit: String,
    pub organization: String,
    pub email: String,
    pub country: String,
    pub key: usize,
    pub password: String,
    pub confirm: String,
    /// Use a Digital ID from a file: its path and password.
    pub file: String,
    pub file_password: String,
}

impl Default for NewIdDraft {
    fn default() -> Self {
        Self {
            create: true,
            name: String::new(),
            unit: String::new(),
            organization: String::new(),
            email: String::new(),
            country: String::new(),
            key: 0,
            password: String::new(),
            confirm: String::new(),
            file: String::new(),
            file_password: String::new(),
        }
    }
}

/// What a visible signature shows on its left (Acrobat: Configure Signature Appearance ▸ Text,
/// Draw or Image). The one last signed with is remembered.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SignGraphic {
    /// The signer's name in large type (when Name is on).
    #[default]
    Name,
    /// The saved Fill & Sign signature, as it is when signing.
    FillSign,
    /// An imported PNG or JPEG.
    Image(#[serde(with = "crate::fill_sign::image_data")] SignatureImage),
}

/// The signing dialogs' state.
#[derive(Clone, Debug, PartialEq)]
pub struct SignDraft {
    pub page: usize,
    /// The signature rectangle (user space); `None`: invisible.
    pub rect: Option<[f64; 4]>,
    /// Sign this empty signature field instead.
    pub field: Option<String>,
    /// Certify (DocMDP) with these permissions: 1 none, 2 form fill and signing, 3 also comments.
    pub certify: Option<u8>,
    pub step: SignStep,
    pub selected: Option<usize>,
    pub password: String,
    pub reason: String,
    pub location: String,
    pub appearance: Appearance,
    pub graphic: SignGraphic,
    /// The image the dialog holds (remembered or chosen), so Image can be chosen again.
    pub image: Option<SignatureImage>,
    pub new_id: NewIdDraft,
    pub error: Option<String>,
    /// Sign with a Digital ID ▸ Remove was clicked for this list entry: asking to confirm.
    pub confirm_remove: Option<usize>,
}

impl SignDraft {
    /// Starts at Choose when there are IDs to show (even only unusable ones, so their reasons
    /// are seen), with the first usable one selected.
    fn new(page: usize, rect: Option<[f64; 4]>, field: Option<String>, certify: Option<u8>, ids: &[DigitalIdEntry]) -> Self {
        Self {
            page,
            rect,
            field,
            certify,
            step: if ids.is_empty() { SignStep::Configure } else { SignStep::Choose },
            selected: first_usable(ids),
            password: String::new(),
            reason: String::new(),
            location: String::new(),
            appearance: Appearance::default(),
            graphic: SignGraphic::Name,
            image: None,
            new_id: NewIdDraft::default(),
            error: None,
            confirm_remove: None,
        }
    }
}

/// What a page asks of the signing flow.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignView {
    /// A signature rectangle being dragged: (page, start on screen).
    pub drag: Option<(usize, Pos2)>,
    /// A rectangle was drawn: (page, user-space rect).
    pub drawn: Option<(usize, [f64; 4])>,
    /// An empty signature field was clicked (its name).
    pub field: Option<String>,
}

/// Drag a signature rectangle on one page (Digitally sign, Certify (visible signature)), from a
/// press nobody holds (`free`).
pub(crate) fn page_input(cx: PageToolContext<'_>, free: bool, view: &mut DocView) {
    let PageToolContext { ui, resp, xf, ptr, page, info, .. } = cx;
    let (pointer, origin) = (ptr.pos, ptr.origin);
    if pointer.is_some_and(|p| xf.rect.contains(p)) {
        view.cursor.claim(Layer::ToolArea, egui::CursorIcon::Crosshair);
    }
    // The tool's own press, of the primary button: one nobody holds.
    if resp.drag_started_by(egui::PointerButton::Primary)
        && free
        && let Some(o) = origin.filter(|o| xf.rect.contains(*o))
    {
        view.sign.drag = Some((page, o));
    }
    if let Some((p, start)) = view.sign.drag
        && p == page
    {
        // Where the pointer is, or where it went down when it is off the window.
        let end = pointer.unwrap_or(start);
        let end = Pos2::new(end.x.clamp(xf.rect.left(), xf.rect.right()), end.y.clamp(xf.rect.top(), xf.rect.bottom()));
        let r = Rect::from_two_pos(start, end);
        ui.painter().rect_filled(r, CornerRadius::ZERO, Color32::from_rgba_unmultiplied(0x14, 0x73, 0xE6, 28));
        ui.painter().rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_rgb(0x14, 0x73, 0xE6)), egui::StrokeKind::Middle);
        if resp.drag_stopped() || !ptr.primary_down {
            view.sign.drag = None;
            if r.width() >= 8.0 && r.height() >= 8.0 {
                let user = |q: Pos2| {
                    let (vx, vy) = xf.screen_to_view(q);
                    info.pages[page].view_to_user(vx, vy)
                };
                let (a, b) = (user(r.left_top()), user(r.right_bottom()));
                let rect = [a[0].min(b[0]) as f64, a[1].min(b[1]) as f64, a[0].max(b[0]) as f64, a[1].max(b[1]) as f64];
                view.sign.drawn = Some((page, rect));
            }
        }
    }
}

/// Where new digital IDs are saved: next to the recovery folder (`…/PdfCraft/Digital IDs`, or
/// `PdfCraftData/Digital IDs` in portable mode).
fn id_dir() -> Option<PathBuf> {
    crate::recovery::RecoveryStore::default_dir().and_then(|d| d.parent().map(|p| p.join("Digital IDs")))
}

/// Save a new digital ID as `<stem>.p12`, or `<stem> 2.p12`, … when the name is taken. The file
/// is always created fresh, never opened through a file or link already at the name (checking
/// first and then writing would let one be planted in between), and on Unix only its owner can
/// read it: it holds the private key.
fn save_new_id_file(dir: &std::path::Path, stem: &str, p12: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::{ErrorKind, Write};
    for i in 1..=10_000u32 {
        let path = dir.join(if i == 1 { format!("{stem}.p12") } else { format!("{stem} {i}.p12") });
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let file = match opts.open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            // Windows refuses `create_new` on a folder with "access denied"; the name is taken all
            // the same. A folder we can't write to, with nothing at the name, still fails here.
            Err(e) if e.kind() == ErrorKind::PermissionDenied && path.symlink_metadata().is_ok() => continue,
            Err(e) => return Err(e),
        };
        // The block closes the file before a failed one is removed (Windows can't remove an open file).
        let written = {
            let mut file = file;
            file.write_all(p12).and_then(|()| file.sync_all())
        };
        if let Err(e) = written {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        return Ok(path);
    }
    Err(std::io::Error::new(ErrorKind::AlreadyExists, "no free file name for the digital ID"))
}

/// A Keychain identity by its `keychain:` reference.
fn keychain_id(reference: &str) -> Result<DigitalId, String> {
    #[cfg(target_os = "macos")]
    return sign::keychain::find(reference).map_err(|e| e.to_string());
    #[cfg(not(target_os = "macos"))]
    Err(format!("{reference}: Keychain identities are only available on macOS"))
}

/// A Windows store identity by its `windows:` reference.
fn windows_id(reference: &str) -> Result<DigitalId, String> {
    #[cfg(target_os = "windows")]
    return sign::windows::find(reference).map_err(|e| e.to_string());
    #[cfg(not(target_os = "windows"))]
    Err(format!("{reference}: Windows certificate store identities are only available on Windows"))
}

pub fn entry_for(path: &str, c: &Certificate) -> DigitalIdEntry {
    DigitalIdEntry {
        path: path.to_string(),
        name: c.display_name(),
        issuer: c.issuer.common_name().map(str::to_string).unwrap_or_else(|| c.issuer.display()),
        email: c.subject.email().unwrap_or("").to_string(),
        expires: expiry_stamp(c.not_after),
        unusable: None,
    }
}

/// The current time, in the form certificates state theirs.
fn time_now() -> sign::Time {
    let elapsed = web_time::SystemTime::now().duration_since(web_time::SystemTime::UNIX_EPOCH);
    sign::Time::from_unix(elapsed.ok().and_then(|d| i64::try_from(d.as_secs()).ok()).unwrap_or(0))
}

/// A date as `DigitalIdEntry::expires` holds it. These compare in date order.
fn expiry_stamp(t: sign::Time) -> String {
    format!("{:04}.{:02}.{:02}", t.year, t.month, t.day)
}

/// Whether `entry` expired before the day `today` (an `expiry_stamp`). An entry with no date,
/// such as a greyed Windows certificate, isn't known to have expired.
fn has_expired(entry: &DigitalIdEntry, today: &str) -> bool {
    !entry.expires.is_empty() && entry.expires.as_str() < today
}

/// Remove the expired IDs of an OS key store listing and say how many went.
fn drop_expired(listed: &mut Vec<DigitalIdEntry>, today: &str) -> usize {
    let before = listed.len();
    listed.retain(|e| !has_expired(e, today));
    before - listed.len()
}

/// Refuse a certificate past its end. The picker works by the day and from what it saved, so
/// this is what stops an ID that expired while the dialog was open.
fn refuse_expired(c: &Certificate, now: sign::Time) -> Result<(), String> {
    if c.not_after < now { Err(tl!("This digital ID has expired.").to_string()) } else { Ok(()) }
}

/// The list positions of `ids` in the picker's alphabetical order. Only the picker sorts: the
/// list itself keeps its order, so `SignDraft::selected` and the saved settings don't move.
fn picker_order(ids: &[DigitalIdEntry]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..ids.len()).collect();
    order.sort_by_cached_key(|&i| crate::i18n::name_sort_key(&ids[i].name));
    order
}

/// The first ID the picker can select, top down.
fn first_usable(ids: &[DigitalIdEntry]) -> Option<usize> {
    picker_order(ids).into_iter().find(|&i| ids[i].unusable.is_none())
}

/// The usable ID after `selected` in the picker whose name starts with `letter`, wrapping, so
/// pressing a letter again walks through the IDs that start with it.
fn next_with_letter(ids: &[DigitalIdEntry], order: &[usize], selected: Option<usize>, letter: char) -> Option<usize> {
    let letter = crate::i18n::name_sort_key(&letter.to_string());
    let start = selected.and_then(|s| order.iter().position(|&i| i == s)).map_or(0, |at| at + 1);
    order
        .iter()
        .cycle()
        .skip(start)
        .take(order.len())
        .copied()
        .find(|&i| ids[i].unusable.is_none() && crate::i18n::name_sort_key(&ids[i].name).starts_with(&letter))
}

/// A greyed entry for a Windows store certificate that can't sign, with the reason.
#[cfg(target_os = "windows")]
fn entry_unusable(u: &sign::windows::Unusable) -> DigitalIdEntry {
    DigitalIdEntry {
        path: format!("windows:{}", u.fingerprint.split(' ').collect::<String>().to_ascii_lowercase()),
        name: u.subject.clone(),
        issuer: String::new(),
        email: String::new(),
        expires: String::new(),
        unusable: Some(u.reason.clone()),
    }
}

impl PdfCraftApp {
    /// Export certificate: write `c` as PEM into a chosen folder, named after its subject.
    pub(crate) fn export_certificate(&mut self, c: &Certificate) {
        let name = format!("{}.cer", crate::files::attachment_file_name(&c.display_name()));
        self.write_files(&[(name, std::sync::Arc::new(certificate_pem(c).into_bytes()))], "Export certificate");
    }

    /// Start signing: the rectangle (or field) is known; show Sign with a Digital ID.
    pub fn start_signing(&mut self, page: usize, rect: Option<[f64; 4]>, field: Option<String>, certify: Option<u8>) {
        self.refresh_os_key_store_ids();
        let mut draft = SignDraft::new(page, rect, field, certify, &self.digital_ids);
        draft.graphic = match &self.sign_graphic {
            SignGraphic::FillSign if self.signature.is_none() => SignGraphic::Name,
            g => g.clone(),
        };
        if let SignGraphic::Image(image) = &self.sign_graphic {
            draft.image = Some(image.clone());
        }
        self.sign_draft = Some(draft);
        self.dialog = Some(crate::Dialog::Sign);
    }

    /// List OS key store signing identities (after the file-based IDs). A failed listing keeps
    /// the entries already listed. Expired OS IDs aren't listed; an expired ID file stays,
    /// greyed, because the picker is where it can be removed.
    fn refresh_os_key_store_ids(&mut self) {
        let today = expiry_stamp(time_now());
        for e in self.digital_ids.iter_mut().filter(|e| e.unusable.is_none() && has_expired(e, &today)) {
            e.unusable = Some(tl!("This digital ID has expired.").to_string());
        }
        let Some(mut listed) = self.list_os_key_store_ids() else { return };
        let dropped = drop_expired(&mut listed, &today);
        self.digital_ids.retain(|e| !e.path.starts_with("keychain:") && !e.path.starts_with("windows:"));
        self.digital_ids.extend(listed);
        if dropped > 0 && first_usable(&self.digital_ids).is_none() {
            self.notify(tl!("The digital IDs on this computer have expired."));
        }
    }

    /// The OS key store's signing identities, or `None` after telling the user it couldn't be read.
    fn list_os_key_store_ids(&mut self) -> Option<Vec<DigitalIdEntry>> {
        #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(unused_mut))]
        let mut listed = Vec::new();
        #[cfg(target_os = "macos")]
        if self.os_key_store_ids {
            match sign::keychain::identities(None) {
                Ok(ids) => {
                    for id in ids {
                        let mut entry = entry_for(&sign::keychain::reference(&id.certificate), &id.certificate);
                        // Smart card identities say so ("… (smart card)").
                        if let Some(name) = id.friendly_name {
                            entry.name = name;
                        }
                        listed.push(entry);
                    }
                }
                Err(e) => {
                    self.notify_fmt("The Keychain's digital IDs couldn't be listed: {e}", &[("e", &e.to_string())]);
                    return None;
                }
            }
        }
        #[cfg(target_os = "windows")]
        if self.os_key_store_ids {
            match sign::windows::list() {
                Ok(listing) => {
                    for id in &listing.ids {
                        listed.push(entry_for(&sign::windows::reference(&id.certificate), &id.certificate));
                    }
                    // Certificates without a private key aren't identities; the rest are shown
                    // greyed with why they can't sign (issue #179).
                    for u in listing.unusable.iter().filter(|u| !u.no_private_key) {
                        listed.push(entry_unusable(u));
                    }
                }
                Err(e) => {
                    self.notify_fmt("The Windows store's digital IDs couldn't be listed: {e}", &[("e", &e.to_string())]);
                    return None;
                }
            }
        }
        Some(listed)
    }

    /// Refresh: list the OS key store again, keeping the selection on its ID. Refresh on the
    /// empty Configure step goes to the picker once there is something to pick.
    fn refresh_digital_ids(&mut self) {
        let selected = self.sign_draft.as_ref().and_then(|d| d.selected).and_then(|i| self.digital_ids.get(i)).map(|e| e.path.clone());
        self.refresh_os_key_store_ids();
        if let Some(d) = self.sign_draft.as_mut() {
            d.confirm_remove = None;
            d.selected = selected
                .and_then(|path| self.digital_ids.iter().position(|e| e.path == path && e.unusable.is_none()))
                .or_else(|| first_usable(&self.digital_ids));
            if !self.digital_ids.is_empty() {
                d.step = SignStep::Choose;
            }
        }
    }

    /// Add a digital ID file to the list (or select it if it's there).
    pub fn add_digital_id(&mut self, path: &str, cert: &Certificate) -> usize {
        if let Some(i) = self.digital_ids.iter().position(|e| e.path == path) {
            self.digital_ids[i] = entry_for(path, cert);
            return i;
        }
        self.digital_ids.push(entry_for(path, cert));
        self.digital_ids.len() - 1
    }

    /// Take the digital ID file at `index` off the list (issue #1396). The file itself stays on
    /// disk. Keychain and Windows store IDs belong to the OS and are listed again on every
    /// signing, so they can't be removed here. The open dialog's selection follows the list.
    pub fn remove_digital_id(&mut self, index: usize) -> bool {
        if self.digital_ids.get(index).is_none_or(|e| e.path.starts_with("keychain:") || e.path.starts_with("windows:")) {
            return false;
        }
        self.digital_ids.remove(index);
        if let Some(d) = self.sign_draft.as_mut() {
            d.confirm_remove = None;
            d.selected = match d.selected {
                Some(s) if s > index => Some(s - 1),
                Some(s) if s == index => first_usable(&self.digital_ids),
                s => s,
            };
            if self.digital_ids.is_empty() {
                d.step = SignStep::Configure;
            }
        }
        true
    }

    /// Configure New Digital ID ▸ Create: a self-signed ID saved as a .p12 next to the app data
    /// (or in `export_dir_override`). Returns its list index.
    fn create_digital_id(&mut self) -> Result<usize, String> {
        let Some(d) = self.sign_draft.as_ref().map(|d| d.new_id.clone()) else { return Err("nothing to create".into()) };
        if d.name.trim().is_empty() {
            return Err("Enter a name.".into());
        }
        // The message is a catalogue key, so it spells the number out; keep the two in step.
        const _: () = assert!(sign::pkcs12::MIN_PASSWORD_CHARS == 8);
        if d.password.chars().count() < sign::pkcs12::MIN_PASSWORD_CHARS {
            return Err("The password must have at least 8 characters.".into());
        }
        if d.password != d.confirm {
            return Err("The passwords do not match.".into());
        }
        let key = match KEY_ALGORITHMS[d.key.min(KEY_ALGORITHMS.len() - 1)].1 {
            "rsa3072" => PrivateKey::generate_rsa(3072),
            "rsa4096" => PrivateKey::generate_rsa(4096),
            "p256" => PrivateKey::generate_p256(),
            _ => PrivateKey::generate_rsa(2048),
        }
        .map_err(|e| e.to_string())?;
        let now = self.session.now_secs();
        let dn = Name::build(&d.name, &d.unit, &d.organization, &d.email, &d.country);
        let serial = sign::keys::DigestAlg::Sha256.digest(&[d.name.as_bytes(), &now.to_be_bytes()])[..8].to_vec();
        let cert = Certificate::self_signed(&dn, &key, sign::Time::from_unix(now), 5, &serial).map_err(|e| e.to_string())?;
        let id = DigitalId { key, certificate: cert.clone(), chain: Vec::new(), friendly_name: Some(d.name.trim().to_string()) };
        let p12 = sign::pkcs12::write(&id, &d.password).map_err(|e| e.to_string())?;
        let dir = match &self.export_dir_override {
            Some(p) => Some(PathBuf::from(p)),
            None => id_dir(),
        }
        .ok_or("There is no folder to save the digital ID in.")?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let stem: String = d.name.trim().chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
        let path = save_new_id_file(&dir, &stem, &p12).map_err(|e| e.to_string())?;
        Ok(self.add_digital_id(&path.to_string_lossy(), &cert))
    }

    /// Configure New Digital ID ▸ Use a Digital ID from a file.
    fn import_digital_id(&mut self) -> Result<usize, String> {
        let Some(d) = self.sign_draft.as_ref().map(|d| d.new_id.clone()) else { return Err("nothing to import".into()) };
        let bytes = std::fs::read(&d.file).map_err(|e| format!("{}: {e}", d.file))?;
        let id = sign::pkcs12::open(&bytes, &d.file_password).map_err(|e| match e {
            sign::SignError::WrongPassword => "The password is incorrect.".to_string(),
            e => e.to_string(),
        })?;
        Ok(self.add_digital_id(&d.file, &id.certificate))
    }

    /// Sign as …: open the ID, sign, save the signed file (Save As), and show it.
    fn finish_signing(&mut self) -> Result<(), String> {
        // What's typed in a form field is signed with the document (#166). A rejected value keeps
        // the dialog open; the notice says why.
        if !self.commit_form_typing() {
            return Err(String::new());
        }
        let Some((_, doc_id)) = self.active_ids() else { return Err("no document".into()) };
        let d = self.sign_draft.clone().ok_or("nothing to sign")?;
        let entry = d.selected.and_then(|i| self.digital_ids.get(i)).filter(|e| e.unusable.is_none()).cloned().ok_or("Choose a digital ID.")?;
        let id = if entry.path.starts_with("keychain:") {
            keychain_id(&entry.path)?
        } else if entry.path.starts_with("windows:") {
            windows_id(&entry.path)?
        } else {
            let bytes = std::fs::read(&entry.path).map_err(|e| format!("{}: {e}", entry.path))?;
            sign::pkcs12::open(&bytes, &d.password).map_err(|e| match e {
                sign::SignError::WrongPassword => "The password is incorrect.".to_string(),
                e => e.to_string(),
            })?
        };
        refuse_expired(&id.certificate, time_now())?;
        let some = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
        // Only a visible signature shows its graphic.
        let graphic = match self.graphic_sig(&d.graphic) {
            Some(saved) if d.rect.is_some() || d.field.is_some() => Some(saved.graphic().ok_or("The saved signature has nothing to draw.")?),
            _ => None,
        };
        let choice = d.graphic.clone();
        let opts = SignOptions {
            field: d.field.clone(),
            page: d.page,
            rect: d.rect,
            reason: some(&d.reason),
            location: some(&d.location),
            certify: d.certify,
            appearance: d.appearance.clone(),
            // The labels the preview shows: in the interface language, or all in English.
            appearance_labels: appearance_labels(crate::i18n::current()).0,
            graphic,
            ..SignOptions::default()
        };
        // Signing saves, as in Acrobat: choose where (a cancelled save cancels signing). The
        // document is signed once the user has chosen.
        let name = self.session.get(doc_id).map(|d| d.name.clone()).unwrap_or_default();
        #[cfg(not(target_arch = "wasm32"))]
        let folder = self.session.get(doc_id).and_then(|d| d.path.clone());
        let stem = name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string();
        let sign_and_save = move |app: &mut Self, path: PathBuf| -> Result<(), String> {
            // Sign what was typed while the save panel was open, too (#166); a refused value
            // has said why already.
            if !app.commit_typing_in(doc_id) {
                return Err(String::new());
            }
            let signed = app.session.sign(doc_id, &id, opts).map_err(|e| e.to_string())?;
            crate::editing::write_atomically(&path.to_string_lossy(), signed.as_slice()).map_err(|e| format!("Could not save: {e}"))?;
            // Our own write is not a change on disk to reload (#431).
            #[cfg(not(target_arch = "wasm32"))]
            app.watch.known_now(doc_id, &path.to_string_lossy());
            app.session.mark_signed(doc_id, signed, Some(path.to_string_lossy().into_owned())).map_err(|e| e.to_string())?;
            #[cfg(target_os = "macos")]
            crate::macos_association::repair(&path);
            app.sign_graphic = choice;
            if let Some(view) = app.views.iter_mut().find(|v| v.id == doc_id) {
                view.invalidate_content();
            }
            app.right = Some(crate::RightPanel::Signatures);
            app.notify_fmt("Signed and saved to {path}", &[("path", &path.display().to_string())]);
            Ok(())
        };
        match self.save_override.clone() {
            Some(p) => sign_and_save(self, PathBuf::from(p)),
            #[cfg(not(target_arch = "wasm32"))]
            None => {
                let dialog = rfd::AsyncFileDialog::new().add_filter_any_case("PDF", &["pdf"]).set_file_name(format!("{stem}_signed.pdf"));
                let dialog = crate::editing::in_document_folder(dialog, folder.as_deref());
                // The signature's page, rectangle and field refer to the document as it is now:
                // the pick is dropped (with a notice) if it is edited or switched away from
                // meanwhile. Once the picker shows, the dialog closes; an error from here on
                // arrives as a notice. If no picker could show, the dialog stays open.
                let asked = self.ask_one(crate::pickers::Ask::Save(dialog), Some(doc_id), move |app, path| {
                    if let Err(e) = sign_and_save(app, path)
                        && !e.is_empty()
                    {
                        app.notify(e);
                    }
                });
                if asked { Ok(()) } else { Err(String::new()) }
            }
            #[cfg(target_arch = "wasm32")]
            None => {
                let _ = (stem, sign_and_save);
                Err(String::new())
            }
        }
    }

    /// The saved signature or image `g` stands for (`None`: the name).
    fn graphic_sig(&self, g: &SignGraphic) -> Option<SavedSig> {
        graphic_of(self.signature.as_ref(), g)
    }

    /// Sign as ▸ Choose image…: answered only into the signing dialog that asked.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn pick_sign_graphic_image(&mut self) {
        let epoch = self.dialog_epoch();
        let dialog = rfd::AsyncFileDialog::new().add_filter_any_case(tl!("Image"), &["png", "jpg", "jpeg"]);
        self.ask_one(crate::pickers::Ask::File(dialog), None, move |app, path| {
            if app.dialog_epoch() != epoch || app.dialog != Some(crate::Dialog::Sign) {
                return;
            }
            // Scanned paper becomes transparent, as for Fill & Sign image signatures.
            let image = std::fs::File::open(path)
                .map_err(pdfcraft_engine::signature_image::SignatureImageError::from)
                .and_then(SignatureImage::read)
                .and_then(|image| image.remove_white_background(245, 35));
            match (image, app.sign_draft.as_mut()) {
                (Ok(image), Some(d)) => {
                    d.graphic = SignGraphic::Image(image.clone());
                    d.image = Some(image);
                    d.error = None;
                }
                (Ok(_), None) => {}
                (Err(e), _) => app.notify_fmt("Couldn't import the signature image: {e}", &[("e", &e.to_string())]),
            }
        });
    }

    /// Trust a certificate (Signatures panel ▸ Add to trusted certificates), revalidating.
    pub fn trust_certificate(&mut self, cert: Certificate) {
        let mut certs = self.session.trusted_certificates().to_vec();
        if !certs.iter().any(|c| c.raw == cert.raw) {
            certs.push(cert);
        }
        self.apply_trusted_certificates(certs);
    }

    /// Replace the trust store and redraw from the rebuilt display copy. One helper
    /// so no caller can revalidate without invalidating: trust changes verdicts,
    /// verdicts choose what the display copy shows, and the old rasters would
    /// otherwise linger while the badge already follows the new verdict.
    /// Trust alone is not an edit: nothing becomes dirty.
    pub fn apply_trusted_certificates(&mut self, certs: Vec<Certificate>) {
        self.session.set_trusted_certificates(certs);
        for view in &mut self.views {
            view.invalidate_content();
        }
    }

    /// A file name without its PDF extension, for snapshot tab names.
    fn file_stem(name: &str) -> String {
        name.trim_end_matches(".pdf").trim_end_matches(".PDF").to_string()
    }

    /// Open saved revision `n` (1 = the oldest) of the active document as a new document.
    /// A repeat reuses the tab: every click used to open another identical one.
    pub fn open_revision(&mut self, n: usize) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        // The revision's byte range, as `Session::open_revision` reads it. An unknown `n`
        // skips the lookup: the whole file could otherwise match the latest revision's tab
        // instead of erroring.
        let ends = doc.revision_ends();
        if let Some(&end) = n.checked_sub(1).and_then(|k| ends.get(k)) {
            let end = end.min(doc.bytes.len());
            if let Some(index) = self.find_snapshot(SnapshotKind::Revision, &doc.name, &doc.bytes[..end]) {
                self.active = Some(index);
                return;
            }
        }
        let from = doc.name.clone();
        match self.session.open_revision(id, n) {
            Ok(new) => {
                // The reading-colours preference is app-wide: the newly opened revision starts with it.
                self.session.set_reading_colors(new, self.reading_colors);
                let Some(doc) = self.session.get(new) else { return };
                let mut view = DocView::new(new, &doc.info, self.view_defaults);
                if self.full_screen {
                    view.enter_presentation();
                }
                view.snapshot = Some(Snapshot { kind: SnapshotKind::Revision, revision: n, from });
                self.views.push(view);
                self.active = Some(self.views.len() - 1);
            }
            Err(e) => self.notify_fmt("Couldn't open revision {n}: {e}", &[("n", &n.to_string()), ("e", &e.to_string())]),
        }
    }

    /// A tab already showing these exact snapshot bytes: its index, so a repeat
    /// switches to it instead of opening another identical tab. An exact match on
    /// the source document wins, so the banner names the file opened from; a
    /// bytes-only match still reuses the tab (incremental saves append, so one
    /// file's revision can be byte-identical to another tab's, and a snapshot's
    /// own repeats name the snapshot itself).
    fn find_snapshot(&self, kind: SnapshotKind, from: &str, bytes: &[u8]) -> Option<usize> {
        let mut fallback = None;
        for (i, v) in self.views.iter().enumerate() {
            let same_kind = v.snapshot.as_ref().is_some_and(|s| s.kind == kind);
            let same_bytes = same_kind && self.session.get(v.id).is_some_and(|d| d.bytes.as_slice() == bytes);
            if !same_bytes {
                continue;
            }
            if v.snapshot.as_ref().is_some_and(|s| s.from == from) {
                return Some(i);
            }
            fallback.get_or_insert(i);
        }
        fallback
    }

    /// Open the signed version of a signature as a new document (View signed version).
    /// A repeat reuses the tab: every click used to open another identical one.
    pub fn view_signed_version(&mut self, len: usize, revision: usize) {
        let Some((_, id)) = self.active_ids() else { return };
        let Some(doc) = self.session.get(id) else { return };
        let end = len.min(doc.bytes.len());
        if let Some(index) = self.find_snapshot(SnapshotKind::SignedVersion, &doc.name, &doc.bytes[..end]) {
            self.active = Some(index);
            return;
        }
        let from = doc.name.clone();
        let bytes = doc.bytes[..end].to_vec();
        let name = format!("{} (signed version)", Self::file_stem(&from));
        match self.session.open(format!("{name}.pdf"), None, std::sync::Arc::new(bytes), doc.password.clone().as_deref()) {
            Ok(new) => {
                // The reading-colours preference is app-wide: the newly opened copy starts with it.
                self.session.set_reading_colors(new, self.reading_colors);
                let Some(doc) = self.session.get(new) else { return };
                let mut view = DocView::new(new, &doc.info, self.view_defaults);
                if self.full_screen {
                    view.enter_presentation();
                }
                view.snapshot = Some(Snapshot { kind: SnapshotKind::SignedVersion, revision, from });
                self.views.push(view);
                self.active = Some(self.views.len() - 1);
            }
            Err(e) => self.notify_fmt("Couldn't open the signed version: {e}", &[("e", &e.to_string())]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder per call: tests run in parallel.
    fn scratch() -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("pdfcraft-sign-ui-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Create a digital ID named "Grace Hopper" in `dir`, as Configure New Digital ID ▸ Create
    /// does. Returns where it was saved.
    fn create_in(dir: &std::path::Path) -> Result<PathBuf, String> {
        let mut app = PdfCraftApp::new();
        app.export_dir_override = Some(dir.to_string_lossy().into_owned());
        let mut draft = SignDraft::new(0, None, None, None, &[]);
        // P-256 keeps the test fast.
        let key = KEY_ALGORITHMS.iter().position(|k| k.1 == "p256").unwrap();
        draft.new_id =
            NewIdDraft { name: "Grace Hopper".into(), key, password: "secret12".into(), confirm: "secret12".into(), ..NewIdDraft::default() };
        app.sign_draft = Some(draft);
        let i = app.create_digital_id()?;
        Ok(PathBuf::from(&app.digital_ids[i].path))
    }

    fn unusable_id(path: &str, name: &str) -> DigitalIdEntry {
        DigitalIdEntry { unusable: Some("no key".to_string()), ..listed(path, name) }
    }

    #[test]
    fn the_picker_lists_ids_alphabetically_and_selects_the_first_usable_one() {
        let ids = [listed("/z", "Zeca"), unusable_id("/a", "água Viva"), listed("/b", "beto"), listed("/v", "Ávila")];
        let order = picker_order(&ids);
        assert_eq!(order.iter().map(|&i| ids[i].name.as_str()).collect::<Vec<_>>(), ["água Viva", "Ávila", "beto", "Zeca"]);
        assert_eq!(first_usable(&ids), Some(3), "the greyed ID on top can't be selected");
    }

    #[test]
    fn a_letter_walks_through_the_ids_that_start_with_it() {
        let ids = [listed("/o", "Nora"), listed("/a", "Ana"), listed("/d", "Nádia"), unusable_id("/i", "Nilo")];
        let order = picker_order(&ids);
        assert_eq!(next_with_letter(&ids, &order, Some(1), 'n'), Some(2), "accents and case don't matter");
        assert_eq!(next_with_letter(&ids, &order, Some(2), 'N'), Some(0), "the greyed Nilo is skipped");
        assert_eq!(next_with_letter(&ids, &order, Some(0), 'n'), Some(2), "wraps to the first");
        assert_eq!(next_with_letter(&ids, &order, None, 'a'), Some(1));
        assert_eq!(next_with_letter(&ids, &order, Some(1), 'x'), None);
        assert_eq!(next_with_letter(&[], &[], None, 'x'), None);
    }

    #[test]
    fn refreshing_the_picker_keeps_the_selection_on_its_id() {
        let mut app = PdfCraftApp::new();
        app.digital_ids = vec![listed("windows:gone", "Gone"), listed("/ids/a.p12", "Ana"), listed("/ids/b.p12", "Beto")];
        let mut draft = SignDraft::new(0, None, None, None, &app.digital_ids);
        draft.selected = Some(2);
        draft.confirm_remove = Some(1);
        app.sign_draft = Some(draft);
        app.refresh_digital_ids();
        let d = app.sign_draft.as_ref().unwrap();
        assert_eq!(d.selected.map(|i| app.digital_ids[i].name.as_str()), Some("Beto"), "the store entry above it is gone");
        assert_eq!(d.confirm_remove, None);
    }

    #[test]
    fn an_expired_os_id_is_not_listed() {
        let dated = |path, expires: &str| DigitalIdEntry { expires: expires.into(), ..listed(path, path) };
        let mut ids = vec![dated("windows:old", "2026.11.04"), dated("windows:today", "2026.11.05"), unusable_id("windows:nokey", "No key")];
        assert_eq!(drop_expired(&mut ids, "2026.11.05"), 1);
        let left: Vec<_> = ids.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(left, ["windows:today", "windows:nokey"], "it lasts through its last day, and no date isn't expired");
    }

    #[test]
    fn an_expired_id_file_stays_greyed_and_is_not_selected() {
        let mut app = PdfCraftApp::new();
        app.digital_ids = vec![DigitalIdEntry { expires: "2000.01.01".into(), ..listed("/ids/a.p12", "Ana") }, listed("/ids/b.p12", "Bia")];
        app.sign_draft = Some(SignDraft::new(0, None, None, None, &app.digital_ids));
        app.sign_draft.as_mut().unwrap().selected = Some(0);
        app.refresh_digital_ids();
        assert_eq!(app.digital_ids.len(), 2, "still there to be removed");
        assert!(app.digital_ids[0].unusable.is_some());
        assert_eq!(app.sign_draft.as_ref().unwrap().selected, Some(1));
    }

    #[test]
    fn signing_refuses_a_certificate_past_its_end() {
        let key = PrivateKey::generate_p256().unwrap();
        let cert = Certificate::self_signed(&Name::build("Old", "", "", "", ""), &key, sign::Time::from_unix(0), 1, &[1]).unwrap();
        assert!(refuse_expired(&cert, cert.not_after).is_ok(), "valid up to its last second");
        assert!(refuse_expired(&cert, time_now()).is_err());
    }

    #[test]
    fn refreshing_the_empty_configure_step_opens_the_picker() {
        let mut app = PdfCraftApp::new();
        app.sign_draft = Some(SignDraft::new(0, None, None, None, &app.digital_ids));
        app.refresh_digital_ids();
        assert_eq!(app.sign_draft.as_ref().unwrap().step, SignStep::Configure, "still nothing to pick");
        app.digital_ids = vec![listed("/ids/a.p12", "Ana")];
        app.refresh_digital_ids();
        let d = app.sign_draft.as_ref().unwrap();
        assert_eq!((d.step, d.selected), (SignStep::Choose, Some(0)));
    }

    #[test]
    fn an_exported_certificate_is_written_inside_the_chosen_folder() {
        let dir = scratch();
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let key = PrivateKey::generate_p256().unwrap();
        let cert = Certificate::self_signed(&Name::build("../x", "", "", "", ""), &key, sign::Time::from_unix(0), 1, &[1]).unwrap();
        let mut app = PdfCraftApp::new();
        app.export_dir_override = Some(out.to_string_lossy().into_owned());
        app.export_certificate(&cert);
        let written: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(written, vec![std::ffi::OsString::from("out")]);
        let pem = std::fs::read_to_string(out.join(".._x.cer")).unwrap();
        assert!(pem.starts_with("-----BEGIN CERTIFICATE-----"), "{pem}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn signature_status_style_gives_each_verdict_its_panel_icon() {
        assert_eq!(signature_status_style(SignatureStatus::Valid).0, "circle-check");
        assert_eq!(signature_status_style(SignatureStatus::Unknown).0, "triangle-alert");
        assert_eq!(signature_status_style(SignatureStatus::Invalid).0, "circle-x");
    }

    #[test]
    fn a_new_digital_id_skips_names_already_taken_without_writing_through_them() {
        let dir = scratch();
        // Another file's hard link at the first name, and a folder at the second (Windows refuses
        // `create_new` on a folder with "access denied" rather than "already exists").
        std::fs::write(dir.join("victim.txt"), "keep me").unwrap();
        std::fs::hard_link(dir.join("victim.txt"), dir.join("Grace Hopper.p12")).unwrap();
        std::fs::create_dir(dir.join("Grace Hopper 2.p12")).unwrap();
        let saved = create_in(&dir).unwrap();
        assert_eq!(saved, dir.join("Grace Hopper 3.p12"));
        assert_eq!(std::fs::read_to_string(dir.join("victim.txt")).unwrap(), "keep me");
        assert!(sign::pkcs12::open(&std::fs::read(&saved).unwrap(), "secret12").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_digital_id_is_not_written_through_a_dangling_link() {
        // `exists()` follows links, so a link to a file that doesn't exist yet looked free, and
        // `fs::write` then created the private key wherever the link pointed.
        let dir = scratch();
        let (link, target) = (dir.join("Grace Hopper.p12"), dir.join("elsewhere.p12"));
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&target, &link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&target, &link);
        #[cfg(not(any(unix, windows)))]
        let made: std::io::Result<()> = {
            let _ = (&target, &link);
            Err(std::io::ErrorKind::Unsupported.into())
        };
        if let Err(e) = made {
            // Windows needs Developer Mode (or admin) for symlinks.
            eprintln!("skipped: can't create a symlink here: {e}");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let saved = create_in(&dir).unwrap();
        assert!(!target.exists(), "nothing written through the link");
        assert_eq!(saved, dir.join("Grace Hopper 2.p12"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_digital_id_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch();
        let saved = create_in(&dir).unwrap();
        let mode = std::fs::metadata(&saved).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "the private key is not readable by others: {mode:o}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn listed(path: &str, name: &str) -> DigitalIdEntry {
        DigitalIdEntry { path: path.into(), name: name.into(), issuer: String::new(), email: String::new(), expires: String::new(), unusable: None }
    }

    /// Issue #1396: an imported ID can be taken off the list (its file stays), the dialog's
    /// selection follows, an OS key store ID or a stale index is refused, and the removal is saved.
    #[test]
    fn a_digital_id_file_is_removed_from_the_list_and_stays_removed() {
        let dir = scratch();
        let file = dir.join("old.p12");
        std::fs::write(&file, b"not opened").unwrap();
        let mut app = PdfCraftApp::new();
        app.digital_ids = vec![listed("windows:abc", "Store"), listed(&file.to_string_lossy(), "Old"), listed("/ids/new.p12", "New")];
        let mut draft = SignDraft::new(0, None, None, None, &app.digital_ids);
        draft.selected = Some(2);
        draft.confirm_remove = Some(1);
        app.sign_draft = Some(draft);

        assert!(!app.remove_digital_id(0), "OS key store IDs stay");
        assert!(!app.remove_digital_id(9), "an index past the list is refused");
        assert!(app.remove_digital_id(1));
        assert_eq!(app.digital_ids.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Store", "New"]);
        assert!(file.exists(), "the file itself is not deleted");
        let d = app.sign_draft.as_ref().unwrap();
        assert_eq!((d.selected, d.confirm_remove, d.step), (Some(1), None, SignStep::Choose), "still New, no pending question");

        // Removing the selected ID selects the first usable one; removing the last file ID
        // leaves only the store ID, which isn't saved.
        assert!(app.remove_digital_id(1));
        assert_eq!(app.sign_draft.as_ref().unwrap().selected, Some(0));
        let mut fresh = PdfCraftApp::new();
        fresh.restore(&app.persist());
        assert!(fresh.digital_ids.is_empty(), "the removal survives a restart: {:?}", fresh.digital_ids);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The labels a signature is stamped with are all in the interface language, or all in
    /// English when the signature's font can't draw them there (or the language is right to
    /// left): never a mix, and never a label signing would refuse.
    #[test]
    fn appearance_labels_are_translated_or_all_english() {
        if !crate::i18n::needs_catalogs() {
            return;
        }
        use crate::i18n::{Lang, has, tr};
        let english = AppearanceLabels::default();
        assert_eq!(appearance_labels(Lang::EN), (english.clone(), false));
        // Latin-script catalogs the signature's font can draw: stamped in the language once every
        // label is translated there.
        for code in ["de", "es", "fr", "it", "pt-br", "pl", "tr", "sl", "hu"] {
            let lang = Lang::from_code(code).unwrap();
            if !english.fields().into_iter().all(|(_, source)| has(lang, source)) {
                continue;
            }
            let (labels, in_english) = appearance_labels(lang);
            assert!(!in_english, "{code}: {labels:?}");
            for ((field, ours), (_, source)) in labels.fields().into_iter().zip(english.fields()) {
                assert_eq!(ours, tr(lang, source), "{code}: {field}");
            }
        }
        for code in ["zh-hans", "zh-hant", "ja", "ru", "uk", "bg", "kk", "el", "te", "cs", "lt", "ar"] {
            assert_eq!(appearance_labels(Lang::from_code(code).unwrap()), (english.clone(), true), "{code}");
        }
        for lang in Lang::all() {
            let (labels, in_english) = appearance_labels(lang);
            assert_eq!(labels.first_undrawable(), None, "{}: signing would refuse {labels:?}", lang.code());
            assert_eq!(in_english, lang != Lang::EN && labels == english, "{}", lang.code());
        }
    }

    /// The preview's details are the stamp's: the label above the name, then each detail after
    /// its label, in the words given, and only the values when the labels are off.
    #[test]
    fn preview_lines_use_the_labels_signing_stamps() {
        if !crate::i18n::needs_catalogs() {
            return;
        }
        let (de, _) = appearance_labels(crate::i18n::Lang::from_code("de").unwrap());
        let all = Appearance { reason: true, location: true, ..Appearance::default() };
        let lines = detail_lines(&all, &de, "Ada", " Genehmigt ", "Berlin", "2026.10.14 09:30:00 Z");
        assert_eq!(lines, ["Digital signiert von", "Ada", "Grund: Genehmigt", "Ort: Berlin", "Datum: 2026.10.14 09:30:00 Z"]);
        let bare = Appearance { labels: false, ..all.clone() };
        assert_eq!(detail_lines(&bare, &de, "Ada", "Genehmigt", "", "D"), ["Ada", "Genehmigt", "D"]);
        let nothing = Appearance { name: false, date: false, ..all };
        assert!(detail_lines(&nothing, &de, "Ada", "  ", "", "D").is_empty());
    }

    fn signature(certify: Option<u8>, status: SignatureStatus) -> SignatureInfo {
        SignatureInfo {
            field: "Signature".into(),
            signed: true,
            page: None,
            rect: None,
            visible: false,
            signer: None,
            certificate: None,
            chain: Vec::new(),
            date: None,
            signing_time: None,
            reason: None,
            location: None,
            contact: None,
            sub_filter: None,
            doc_timestamp: false,
            certify,
            revision: 1,
            signed_len: 0,
            digest: None,
            algorithm: None,
            timestamp: false,
            timestamp_time: None,
            status,
            modification: sign::Modification::None,
            details: Vec::new(),
        }
    }

    /// A valid certification doesn't hide a later signature that is invalid or can't be checked:
    /// the message bar names the problem, and says "Certified by" only when all are valid.
    #[test]
    fn the_message_bar_puts_a_signature_problem_before_the_certification() {
        use SignatureStatus::{Invalid, Unknown, Valid};
        let bar = |sigs: &[SignatureInfo]| banner(sigs).map(|b| (b.0, b.2));
        let certified = signature(Some(2), Valid);
        assert_eq!(bar(std::slice::from_ref(&certified)), Some(("badge-check", "Certified by {by}")));
        assert_eq!(bar(&[certified.clone(), signature(None, Valid)]), Some(("badge-check", "Certified by {by}")));
        assert_eq!(bar(&[certified.clone(), signature(None, Invalid)]), Some(("circle-x", "At least one signature is invalid.")));
        assert_eq!(bar(&[certified, signature(None, Unknown)]), Some(("triangle-alert", "At least one signature has problems.")));
        assert_eq!(bar(&[signature(Some(2), Unknown), signature(None, Valid)]), Some(("triangle-alert", "At least one signature has problems.")));
        assert_eq!(bar(&[signature(None, Valid)]), Some(("circle-check", "Signed and all signatures are valid.")));
        assert_eq!(bar(&[SignatureInfo { signed: false, ..signature(None, Invalid) }]), None);
    }
}
