//! The copy of a document an email program is handed as an attachment, and the names made for it
//! (shared by File ▸ Attach to email… and the `doc_email` tool).

use std::path::{Path, PathBuf};

/// Writes a copy with unsaved edits for the email app: into a new folder of its own under the
/// system's temp folder ([`new_copy_dir`]), created fresh as well, so nothing already at the name
/// (a file or a link someone else put there) is opened or written through, and only its owner can
/// read it (mode 0600 on Unix; on Windows the folder's owner-only access list, set before the file
/// exists, which the file inherits). Each send gets its own folder, so a later send of the same document never
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
/// link, makes it try another), made for the current user alone before anything is put in it
/// ([`crate::mail::create_private_dir`]: mode 0700 on Unix, an owner-only access list on Windows,
/// which the copy inherits; the send fails where that can't be set up). The name is PdfCraft's, the
/// process's, the time's and a count's, so two sends never share one.
fn new_copy_dir(parent: &Path) -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut taken = std::io::Error::from(std::io::ErrorKind::AlreadyExists);
    for _ in 0..COPY_DIR_ATTEMPTS {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let count = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = parent.join(format!("pdfcraft-send-email-{}-{nanos:x}-{count}", std::process::id()));
        match crate::mail::create_private_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => taken = e,
            Err(e) => return Err(e),
        }
    }
    Err(taken)
}

/// Whether the file at `path` holds exactly `bytes` (the size first, then the contents in
/// pieces, so a file that differs is not read whole).
pub fn file_holds(path: &Path, bytes: &[u8]) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else { return false };
    if !file.metadata().is_ok_and(|m| m.is_file() && m.len() == bytes.len() as u64) {
        return false;
    }
    let mut buf = vec![0u8; 64 * 1024];
    for expected in bytes.chunks(buf.len()) {
        let Some(got) = buf.get_mut(..expected.len()) else { return false };
        if file.read_exact(got).is_err() || got != expected {
            return false;
        }
    }
    // Nothing more in it: a file that grew since the size was read is not the same one.
    file.read(&mut buf[..1]).is_ok_and(|n| n == 0)
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
    fn a_file_holds_the_open_version_only_when_every_byte_matches() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-file-holds-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.pdf");
        let long: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&file, &long).unwrap();
        assert!(file_holds(&file, &long));
        assert!(!file_holds(&file, b""));
        let mut changed_last = long.clone();
        *changed_last.last_mut().unwrap() ^= 1;
        assert!(!file_holds(&file, &changed_last), "a difference past the first piece");
        assert!(!file_holds(&file, &long[..long.len() - 1]) && !file_holds(&file, &[long.as_slice(), b"x"].concat()), "another size");
        assert!(!file_holds(&dir.join("missing.pdf"), &long) && !file_holds(&dir, &long));
        let _ = std::fs::remove_dir_all(&dir);
    }

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
