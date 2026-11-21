//! Files written from a document's attachments: the files of a portfolio (ISO 32000-2
//! §7.11.6) or any document's attachment list, saved into a folder.

use std::io::Write;
use std::path::{Path, PathBuf};

use pdfcraft_render::AttachmentSource;

use crate::Document;

/// At most this many numbered names (`name (2).pdf` … ) are tried for one file.
const MAX_NUMBERED: u32 = 9999;

/// The longest file name the common file systems take, in bytes of UTF-8 (ext4, APFS); NTFS
/// counts UTF-16 units, never more than the UTF-8 bytes.
const MAX_NAME_BYTES: usize = 255;

/// What became of one attachment saved by [`save_attachments`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedAttachment {
    /// Its key in the document's `/EmbeddedFiles` name tree, as `doc_info` lists it.
    pub key: String,
    /// The name the Attachments panel shows.
    pub name: String,
    /// The file it was written to, or why it wasn't written.
    pub outcome: Result<PathBuf, String>,
}

/// Write `doc`'s document-level attachments into `folder`: those whose keys are in `keys`, or
/// all of them. Each file is named after its attachment ([`attachment_file_name`]); a name
/// already taken in the folder gets ` (2)`, ` (3)` … before its extension, so no file in the
/// folder, nor one written earlier in the same run, is ever replaced. An attachment that can't be
/// read (still encrypted, not embedded, not decodable) or written is reported with its reason and
/// the others are still written. Paper-clip comments on pages are not attachments here. A key
/// that names no document-level attachment refuses the whole call before anything is written.
pub fn save_attachments(doc: &Document, keys: Option<&[String]>, folder: &Path) -> Result<Vec<SavedAttachment>, String> {
    let listed: Vec<_> = doc.info.attachments.iter().filter(|a| matches!(a.source, AttachmentSource::Document { .. })).collect();
    let key_of = |a: &pdfcraft_render::Attachment| match &a.source {
        AttachmentSource::Document { key } => key.clone(),
        AttachmentSource::Annotation { .. } => String::new(),
    };
    let chosen: Vec<pdfcraft_render::Attachment> = match keys {
        None => listed.into_iter().cloned().collect(),
        Some(keys) => {
            if let Some(missing) = keys.iter().find(|k| !listed.iter().any(|a| key_of(a) == **k)) {
                return Err(format!("no attachment has the key {missing:?}"));
            }
            listed.into_iter().filter(|a| keys.contains(&key_of(a))).cloned().collect()
        }
    };
    let mut saved = Vec::with_capacity(chosen.len());
    pdfcraft_render::each_attachment_data(&doc.bytes, doc.password.as_deref(), &chosen, |i, data| {
        let Some(att) = chosen.get(i) else { return };
        let outcome = data.and_then(|bytes| write_new(folder, &attachment_file_name(&att.name), &bytes));
        saved.push(SavedAttachment { key: key_of(att), name: att.name.clone(), outcome });
    });
    Ok(saved)
}

/// Write `bytes` to a new file named `name` in `folder`, or the first of `name (2)`, `name (3)` …
/// not taken: the file is created only if nothing has that name (no file, folder or link is
/// followed or replaced). A file left half written is removed.
fn write_new(folder: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let (stem, ext) = split_extension(name);
    for n in 1..=MAX_NUMBERED {
        let suffix = if n == 1 { String::new() } else { format!(" ({n})") };
        let path = folder.join(numbered(stem, &suffix, ext));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                return match file.write_all(bytes).and_then(|()| file.sync_all()) {
                    Ok(()) => Ok(path),
                    Err(e) => {
                        drop(file);
                        let _ = std::fs::remove_file(&path);
                        Err(format!("{}: {e}", path.display()))
                    }
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Err(format!("{}: every numbered name up to ({MAX_NUMBERED}) is taken", folder.join(name).display()))
}

/// `name` split before its last extension (`archive.tar.gz` → `archive.tar`, `gz`). A name with
/// no stem before its dot (`.profile`), or an extension too long to be one, has none.
fn split_extension(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() && ext.len() <= 32 => (stem, Some(ext)),
        _ => (name, None),
    }
}

/// `stem` + `suffix` + `.ext`, with `stem` cut (between characters) so the name fits in
/// [`MAX_NAME_BYTES`], and no dot or space left where the cut ends it (Windows drops them).
fn numbered(stem: &str, suffix: &str, ext: Option<&str>) -> String {
    let tail = suffix.len() + ext.map_or(0, |e| e.len() + 1);
    let room = MAX_NAME_BYTES.saturating_sub(tail);
    let mut cut = stem;
    if cut.len() > room {
        let end = cut.char_indices().map(|(at, c)| at + c.len_utf8()).take_while(|end| *end <= room).last().unwrap_or(0);
        cut = cut.get(..end).unwrap_or_default().trim_end_matches(|c: char| c == '.' || c.is_whitespace());
        if cut.is_empty() {
            cut = "attachment";
        }
    }
    match ext {
        Some(ext) => format!("{cut}{suffix}.{ext}"),
        None => format!("{cut}{suffix}"),
    }
}

/// A plain file name for a PDF-supplied attachment name: the Save suggestion, and the tab name
/// (which later names files written from that tab). It never changes the stored name or a
/// destination the user selected.
pub fn attachment_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'))
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c })
        .collect();
    let name = cleaned.trim_start().trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    if name.is_empty() {
        return "attachment".into();
    }
    let device = name.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    let reserved = matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || device
            .strip_prefix("COM")
            .or_else(|| device.strip_prefix("LPT"))
            .is_some_and(|number| matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"));
    if reserved { format!("_{name}") } else { name.to_string() }
}

#[cfg(test)]
mod tests {
    use super::{MAX_NAME_BYTES, attachment_file_name, numbered, split_extension};

    /// A name too long for the file system is cut between characters before its number and
    /// extension, which stay, and the cut leaves no dot or space at the end of the stem.
    #[test]
    fn a_long_attachment_name_fits_a_file_name_and_keeps_its_number_and_extension() {
        for stem in ["文".repeat(120), format!("{}. .", "a".repeat(260))] {
            let name = format!("{stem}.pdf");
            let (s, ext) = split_extension(&name);
            for suffix in ["", " (2)", " (9999)"] {
                let out = numbered(s, suffix, ext);
                assert!(out.len() <= MAX_NAME_BYTES, "{} bytes", out.len());
                assert!(out.ends_with(&format!("{suffix}.pdf")), "{out}");
                let kept = out.trim_end_matches(&format!("{suffix}.pdf"));
                assert!(!kept.is_empty() && !kept.ends_with(['.', ' ']) && stem.starts_with(kept), "{kept}");
            }
        }
    }

    #[test]
    fn attachment_save_name_removes_bidi_without_hiding_the_extension() {
        assert_eq!(attachment_file_name("invoice\u{202e}fdp.exe"), "invoicefdp.exe");
        for c in [
            '\u{061c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}',
            '\u{2069}',
        ] {
            assert_eq!(attachment_file_name(&format!("a{c}b.pdf")), "ab.pdf");
        }
    }
    #[test]
    fn attachment_save_name_preserves_ordinary_unicode_and_extensions() {
        for name in ["report.pdf", "archive.tar.gz", "مرحبا.pdf", "שלום.txt", "資料.pdf", "می\u{200c}روم.txt", "👩\u{200d}💻.png", "COM10.txt"]
        {
            assert_eq!(attachment_file_name(name), name);
        }
    }
    #[test]
    fn attachment_save_name_handles_controls_paths_and_device_names() {
        assert_eq!(attachment_file_name("a\u{0}\n\u{85}b.pdf"), "ab.pdf");
        assert_eq!(attachment_file_name("../a\\b:c*?\"<>|.pdf"), ".._a_b_c______.pdf");
        for name in ["NUL.tar.gz", "con.txt", "COM1.exe", "LPT9.txt", "COM¹.txt", "LPT³.pdf"] {
            assert_eq!(attachment_file_name(name), format!("_{name}"));
        }
    }
    #[test]
    fn attachment_save_name_has_a_stable_nonempty_fallback() {
        for name in ["", " ", ".", "..", " . . ", "\u{202e}", "\u{0}\n"] {
            assert_eq!(attachment_file_name(name), "attachment");
        }
        assert_eq!(attachment_file_name(" report.pdf . \u{2003}."), "report.pdf");
        for name in ["invoice\u{202e}fdp.exe", "../a.pdf", "NUL.txt", " . . ", "report.pdf . ", "資料.pdf"] {
            let once = attachment_file_name(name);
            assert_eq!(attachment_file_name(&once), once);
        }
    }
}
