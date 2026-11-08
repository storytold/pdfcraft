//! Prepare a local, unsent MIME draft. No recipient, account access or sending API is involved.
//! Conversion uses the same engine as Open/Create; every attachment must succeed before a draft
//! is returned. Copies of the PDFs accompany the draft for webmail clients without .eml import.

use std::sync::Arc;

use base64::{Engine as _, engine::general_purpose::STANDARD};

pub const MAX_FILES: usize = 100;
pub const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_PDF_BYTES: usize = 128 * 1024 * 1024;
const BOUNDARY: &str = "=_PdfCraft_unsent_PDF_attachments";

pub struct Draft {
    message: Vec<u8>,
    attachments: Vec<(String, Arc<Vec<u8>>)>,
}

/// Convert selected files into independent PDFs and a MIME draft. A failed source never silently
/// disappears from the mail. Attachment names are local basenames, not paths or MIME headers.
pub fn prepare(session: &crate::Session, sources: impl IntoIterator<Item = Result<(String, Vec<u8>), String>>) -> Result<Draft, String> {
    prepare_cancelable(session, sources, &|| false)
}

/// Cancellation is cooperative between source conversions and during MIME construction.
pub fn prepare_cancelable(
    session: &crate::Session,
    sources: impl IntoIterator<Item = Result<(String, Vec<u8>), String>>,
    should_stop: &dyn Fn() -> bool,
) -> Result<Draft, String> {
    let mut attachments = Vec::new();
    let mut total = 0usize;
    for source in sources {
        cancelled(should_stop)?;
        if attachments.len() >= MAX_FILES {
            return Err(format!("email drafts are limited to {MAX_FILES} files"));
        }
        let (name, bytes) = source?;
        if bytes.len() > MAX_SOURCE_BYTES {
            return Err(format!("{name}: a source may not exceed 64 MiB"));
        }
        let (_, pdf) = session.convert_to_pdf(&name, &Arc::new(bytes)).map_err(|e| e.to_string())?;
        cancelled(should_stop)?;
        total = total.checked_add(pdf.len()).filter(|n| *n <= MAX_PDF_BYTES).ok_or("PDF attachments exceed 128 MiB")?;
        let stem = name.rsplit(['/', '\\']).next().unwrap_or("document");
        let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
        let mut length = 0usize;
        let stem: String = stem
            .chars()
            .filter(|c| c.is_alphanumeric() || " -_.,()".contains(*c))
            .take(120)
            .take_while(|c| {
                length += c.len_utf8();
                length <= 180
            })
            .collect();
        let stem = if stem.trim().is_empty() { "document" } else { stem.trim() };
        // The numeric prefix prevents both collisions and Windows device-name interpretation.
        attachments.push((format!("{:03}-{stem}.pdf", attachments.len() + 1), pdf));
    }
    if attachments.is_empty() {
        return Err("choose at least one source for the email draft".into());
    }
    let mut message = format!(
        "X-Unsent: 1\r\nMIME-Version: 1.0\r\nSubject: PDF\r\nContent-Type: multipart/mixed;\r\n boundary=\"{BOUNDARY}\"\r\n\r\n--{BOUNDARY}\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: 7bit\r\n\r\n\r\n"
    ).into_bytes();
    for (index, (name, pdf)) in attachments.iter().enumerate() {
        // RFC 2231 continuation parameters keep every header line short and preserve Unicode.
        message.extend_from_slice(format!("--{BOUNDARY}\r\nContent-Type: application/pdf; name=\"attachment-{}.pdf\"\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment", index + 1).as_bytes());
        let encoded: Vec<String> = name.as_bytes().chunks(16).map(|part| part.iter().map(|b| format!("%{b:02X}")).collect()).collect();
        for (i, part) in encoded.iter().enumerate() {
            let charset = if i == 0 { "UTF-8''" } else { "" };
            message.extend_from_slice(format!(";\r\n filename*{i}*={charset}{part}").as_bytes());
        }
        message.extend_from_slice(b"\r\n\r\n");
        // 57 source bytes become 76 base64 characters, as required by MIME. The fixed boundary
        // contains characters absent from base64, so it cannot collide with attachment content.
        for part in pdf.chunks(57) {
            cancelled(should_stop)?;
            message.extend_from_slice(STANDARD.encode(part).as_bytes());
            message.extend_from_slice(b"\r\n");
        }
    }
    message.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    Ok(Draft { message, attachments })
}

fn cancelled(should_stop: &dyn Fn() -> bool) -> Result<(), String> {
    if should_stop() { Err("cancelled".into()) } else { Ok(()) }
}

/// Read a bounded regular source. The limit applies to the bytes actually read, including a file
/// that grows after metadata was checked. No symlink target is executed: it is only read as data.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_source(path: &std::path::Path) -> Result<(String, Vec<u8>), String> {
    use std::io::Read;
    // Reject obvious devices/FIFOs before opening them, then recheck the opened handle below.
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_SOURCE_BYTES as u64 {
        return Err(format!("{}: choose a regular file no larger than 64 MiB", path.display()));
    }
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_SOURCE_BYTES as u64 {
        return Err(format!("{}: choose a regular file no larger than 64 MiB", path.display()));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(format!("{}: source grew beyond 64 MiB", path.display()));
    }
    let name = path.file_name().ok_or("the source has no filename")?.to_string_lossy().into_owned();
    Ok((name, bytes))
}

impl Draft {
    /// Complete MIME bytes for a host-provided export; contains no recipient or sending action.
    pub fn as_bytes(&self) -> &[u8] {
        &self.message
    }
    pub fn attachment_names(&self) -> impl Iterator<Item = &str> {
        self.attachments.iter().map(|(name, _)| name.as_str())
    }
    /// Write a new, unique bundle below the user-selected directory. Never overwrite an existing
    /// file. Keep the directory only when all files are complete and synchronized; a failed write
    /// cleans up only this newly owned staging directory. The returned draft and PDFs stay on disk
    /// after the mail app closes (or refuses to open), so webmail can attach the PDFs manually.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save_in(&self, parent: &std::path::Path) -> Result<std::path::PathBuf, String> {
        self.save_in_cancelable(parent, &|| false)
    }

    /// A cancellation or write failure removes only this operation's unpublished bundle.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save_in_cancelable(&self, parent: &std::path::Path, should_stop: &dyn Fn() -> bool) -> Result<std::path::PathBuf, String> {
        use std::io::Write;
        cancelled(should_stop)?;
        let dir = tempfile::Builder::new().prefix("PdfCraft-email-").tempdir_in(parent).map_err(|e| e.to_string())?;
        for (name, bytes) in self
            .attachments
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .chain(std::iter::once(("draft.eml", self.message.as_slice())))
        {
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(dir.path().join(name)).map_err(|e| e.to_string())?;
            for chunk in bytes.chunks(64 * 1024) {
                cancelled(should_stop)?;
                file.write_all(chunk).map_err(|e| e.to_string())?;
            }
            file.sync_all().map_err(|e| e.to_string())?;
        }
        #[cfg(unix)]
        {
            std::fs::File::open(dir.path()).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
            std::fs::File::open(parent).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
        }
        cancelled(should_stop)?;
        Ok(dir.keep().join("draft.eml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draft_retains_exact_pdf_bytes_and_unicode_names_without_header_injection() {
        let session = crate::Session::default();
        let draft = prepare(
            &session,
            [Ok(("../../Müller\r\nBcc: stolen@example.org.txt".into(), b"private notes".to_vec())), Ok(("Müller.txt".into(), b"second".to_vec()))],
        )
        .unwrap();
        let text = String::from_utf8(draft.message.clone()).unwrap();
        assert!(text.starts_with("X-Unsent: 1\r\n"));
        assert!(!text.contains("\r\nBcc:"));
        assert!(!text.contains("\r\nTo:") && !text.contains("\r\nFrom:"));
        assert!(draft.attachments.iter().all(|(name, _)| !name.contains(['/', '\\', '\r', '\n'])));
        assert!(text.contains("%C3%BC"));
        for (part, (_, pdf)) in text.split(&format!("--{BOUNDARY}\r\n")).skip(2).zip(&draft.attachments) {
            let body = part.split_once("\r\n\r\n").unwrap().1.split("--").next().unwrap().replace("\r\n", "");
            assert_eq!(STANDARD.decode(body).unwrap(), **pdf);
        }
    }

    #[test]
    fn one_bad_attachment_refuses_the_entire_draft() {
        let session = crate::Session::default();
        assert!(prepare(&session, [Ok(("valid.txt".into(), b"hi".to_vec())), Ok(("unsupported.docx".into(), b"PK".to_vec()))]).is_err());
        assert!(prepare(&session, []).is_err());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn cancelled_writes_leave_no_partial_bundle_and_long_names_fit_filesystems() {
        let parent = tempfile::tempdir().unwrap();
        let name = format!("{}.txt", "語".repeat(120));
        let draft = prepare(&crate::Session::default(), [Ok((name, b"one".to_vec()))]).unwrap();
        assert!(draft.attachment_names().all(|name| name.len() < 200));
        let checks = std::cell::Cell::new(0);
        let result = draft.save_in_cancelable(parent.path(), &|| {
            checks.set(checks.get() + 1);
            checks.get() >= 3
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
        assert!(draft.save_in(parent.path()).unwrap().is_file());
        assert!(prepare_cancelable(&crate::Session::default(), [Ok(("one.txt".into(), b"one".to_vec()))], &|| true).is_err());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn saving_creates_independent_durable_bundles_without_overwriting() {
        let parent = tempfile::tempdir().unwrap();
        let draft = prepare(&crate::Session::default(), [Ok(("CON.txt".into(), b"one".to_vec()))]).unwrap();
        let first = draft.save_in(parent.path()).unwrap();
        let second = draft.save_in(parent.path()).unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(first).unwrap(), draft.message);
        let (name, pdf) = &draft.attachments[0];
        assert_eq!(std::fs::read(second.parent().unwrap().join(name)).unwrap(), **pdf);
    }
}
