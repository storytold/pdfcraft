//! The copy of a document an email program is handed as an attachment, and the names made for it
//! (shared by File ▸ Attach to email… and the `doc_email` tool).

use std::path::{Path, PathBuf};

/// Writes a copy with unsaved edits for the email app: into a new folder of its own under the
/// system's temp folder ([`new_copy_dir`]), created fresh as well, so nothing already at the name
/// (a file or a link someone else put there) is opened or written through, and on Unix only its
/// owner can read it. Each send gets its own folder, so a later send of the same document never
/// replaces a copy the email app may not have read yet. The folders are left for the system's
/// temp clean-up: the email app can read the file any time until the message is sent, and nothing
/// here can tell when it has.
pub fn write_private_copy(name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let dir = new_copy_dir(&std::env::temp_dir())?;
    let target = dir.join(attachment_name(name));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    let mut file = match opts.open(&target) {
        Ok(file) => file,
        Err(e) => {
            let _ = std::fs::remove_dir(&dir);
            return Err(e);
        }
    };
    if let Err(e) = file.write_all(bytes) {
        // Nothing half-written is left for the email app. The file is closed first (Windows can't
        // remove an open file), and only what was created here is removed.
        drop(file);
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_dir(&dir);
        return Err(e);
    }
    Ok(target)
}

/// Remove a copy made by [`write_private_copy`] that was never handed to an email app: the file,
/// then its folder (which only goes if empty). Anything else is left.
pub fn remove_copy(copy: Option<&Path>) {
    let Some(copy) = copy else { return };
    let _ = std::fs::remove_file(copy);
    if let Some(dir) = copy.parent() {
        let _ = std::fs::remove_dir(dir);
    }
}

/// How many names [`new_copy_dir`] tries before it gives up.
const COPY_DIR_ATTEMPTS: u32 = 16;

/// A folder made new under `parent` (never one already there: anything at the name, a folder or a
/// link, makes it try another), on Unix readable only by its owner. The name is PdfCraft's, the
/// process's, the time's and a count's, so two sends never share one.
fn new_copy_dir(parent: &Path) -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // Only Unix sets a mode on the builder.
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    let mut taken = std::io::Error::from(std::io::ErrorKind::AlreadyExists);
    for _ in 0..COPY_DIR_ATTEMPTS {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let count = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = parent.join(format!("pdfcraft-send-email-{}-{nanos:x}-{count}", std::process::id()));
        match builder.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => taken = e,
            Err(e) => return Err(e),
        }
    }
    Err(taken)
}

/// The attachment's file name: the document's, ending in .pdf, with nothing a path would read.
pub fn attachment_name(name: &str) -> String {
    let base: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '\0' | '?' | '*' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let base = base.trim().trim_start_matches('.');
    let base = if base.trim().is_empty() { "Document" } else { base };
    if base.to_ascii_lowercase().ends_with(".pdf") { base.to_string() } else { format!("{base}.pdf") }
}

/// The message's subject: the document's name without its extension.
pub fn subject_for(name: &str) -> String {
    let n = name.trim();
    let stem =
        n.len().checked_sub(4).filter(|&i| n.get(i..).is_some_and(|ext| ext.eq_ignore_ascii_case(".pdf"))).and_then(|i| n.get(..i)).unwrap_or(n);
    let stem = stem.trim();
    if stem.is_empty() { "Document".to_string() } else { stem.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_subjects() {
        assert_eq!(attachment_name("Invoice 26340.pdf"), "Invoice 26340.pdf");
        assert_eq!(attachment_name("Invoice 26340"), "Invoice 26340.pdf");
        assert_eq!(attachment_name("a/b:c.PDF"), "a_b_c.PDF");
        assert_eq!(attachment_name("Q3: what? <draft> | \"final\"*.pdf"), "Q3_ what_ _draft_ _ _final__.pdf", "Windows' reserved characters");
        assert_eq!(attachment_name("   "), "Document.pdf");
        assert_eq!(attachment_name("..pdf"), "pdf.pdf");
        assert_eq!(subject_for("Invoice 26340.pdf"), "Invoice 26340");
        assert_eq!(subject_for("letter.PDF"), "letter");
        assert_eq!(subject_for(".pdf"), "Document");
        assert_eq!(subject_for("Año"), "Año");
        assert_eq!(subject_for("ñ.pdf"), "ñ");
    }
}
