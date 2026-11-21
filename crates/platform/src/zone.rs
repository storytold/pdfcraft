//! Mark-of-the-Web (#3990): the `Zone.Identifier` stream Windows keeps beside a file that came
//! from the internet. SmartScreen, Office's Protected View and archive tools read it to decide how
//! far to trust the file. A file saved out of a document (an attachment) gets the zone of the
//! document it came from, so a PDF from an email doesn't turn the files inside it into trusted
//! local ones.

use std::path::Path;

/// What Windows writes for a file from the internet (`URLZONE_INTERNET`). A file saved out of a
/// document that carries no zone gets this one: where the document came from is unknown.
pub const INTERNET_ZONE: &str = "[ZoneTransfer]\r\nZoneId=3\r\n";

/// A zone stream is a few short lines; a larger one isn't copied.
#[cfg(windows)]
const MAX_ZONE_STREAM: u64 = 64 * 1024;

/// Give `target` the zone of `source`: its `Zone.Identifier` stream, copied as it is, or the
/// Internet zone when `source` is `None` or has no zone. Windows only; elsewhere nothing is
/// written. Fails where the volume keeps no alternate data streams (FAT32, exFAT, some network
/// shares), so callers treat the mark as best effort.
pub fn mark_from(target: &Path, source: Option<&Path>) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let zone = source.and_then(read_zone).unwrap_or_else(|| INTERNET_ZONE.as_bytes().to_vec());
        write_zone(target, &zone)
    }
    #[cfg(not(windows))]
    {
        let _ = (target, source);
        Ok(())
    }
}

/// Give `target` the zone of `source` when `source` has one, and leave `target` as it is when
/// not: a Save As copy of a document from the internet is that same document (#7887), while a
/// copy of a local one stays local. Windows only; best effort, as [`mark_from`].
pub fn copy_zone(target: &Path, source: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        match read_zone(source) {
            Some(zone) => write_zone(target, &zone),
            None => Ok(()),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (target, source);
        Ok(())
    }
}

/// Write `zone` as `path`'s `Zone.Identifier` stream.
#[cfg(windows)]
pub(crate) fn write_zone(path: &Path, zone: &[u8]) -> std::io::Result<()> {
    std::fs::write(zone_stream(path), zone)
}

/// `path`'s `Zone.Identifier` stream, when it has one that reads as a zone.
#[cfg(windows)]
pub fn read_zone(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(zone_stream(path)).ok()?;
    let mut zone = Vec::new();
    file.take(MAX_ZONE_STREAM.saturating_add(1)).read_to_end(&mut zone).ok()?;
    let fits = u64::try_from(zone.len()).is_ok_and(|n| n <= MAX_ZONE_STREAM);
    (fits && has_zone_id(&zone)).then_some(zone)
}

/// Whether `stream` has a `ZoneId=<number>` entry inside its `[ZoneTransfer]` section. Text that
/// only mentions those words elsewhere (say a `HostUrl` ending in `?ZoneId=3`) is not a zone.
#[cfg(windows)]
fn has_zone_id(stream: &[u8]) -> bool {
    let mut in_section = false;
    for line in stream.split(|&b| b == b'\n').map(<[u8]>::trim_ascii) {
        if line.starts_with(b"[") {
            in_section = line.eq_ignore_ascii_case(b"[ZoneTransfer]");
            continue;
        }
        let Some(eq) = line.iter().position(|&b| b == b'=') else { continue };
        let (key, value) = line.split_at(eq);
        let value = value.get(1..).unwrap_or_default().trim_ascii();
        if in_section && key.trim_ascii().eq_ignore_ascii_case(b"ZoneId") && (1..=10).contains(&value.len()) && value.iter().all(u8::is_ascii_digit) {
            return true;
        }
    }
    false
}

/// The path of `path`'s `Zone.Identifier` alternate data stream.
#[cfg(windows)]
fn zone_stream(path: &Path) -> std::path::PathBuf {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    stream.into()
}

#[cfg(test)]
#[cfg(windows)]
mod tests {
    use super::*;

    /// A fresh folder in the temp directory, removed on drop.
    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!("pdfcraft-zone-{name}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_file_from_a_document_without_a_zone_gets_the_internet_zone() {
        let dir = Dir::new("none");
        let target = dir.0.join("invoice.exe");
        std::fs::write(&target, b"MZ").unwrap();
        mark_from(&target, None).unwrap();
        assert_eq!(read_zone(&target).unwrap(), INTERNET_ZONE.as_bytes());
        let source = dir.0.join("local.pdf");
        std::fs::write(&source, b"%PDF").unwrap();
        mark_from(&target, Some(&source)).unwrap();
        assert_eq!(read_zone(&target).unwrap(), INTERNET_ZONE.as_bytes());
        assert_eq!(std::fs::read(&target).unwrap(), b"MZ", "the file's own bytes are untouched");
    }

    #[test]
    fn a_file_from_a_document_gets_that_documents_zone() {
        let dir = Dir::new("copy");
        let source = dir.0.join("from mail.pdf");
        std::fs::write(&source, b"%PDF").unwrap();
        let zone = "[ZoneTransfer]\r\nZoneId=3\r\nReferrerUrl=https://mail.example/\r\nHostUrl=https://mail.example/a.pdf\r\n";
        std::fs::write(zone_stream(&source), zone).unwrap();
        let target = dir.0.join("macro.docm");
        std::fs::write(&target, b"PK").unwrap();
        mark_from(&target, Some(&source)).unwrap();
        assert_eq!(read_zone(&target).unwrap(), zone.as_bytes());
    }

    /// A Save As copy takes the zone of a document from the internet (#7887), and a copy of a
    /// local document stays as it was: unlike [`mark_from`], no Internet zone is made up.
    #[test]
    fn a_copy_takes_only_a_zone_the_source_has() {
        let dir = Dir::new("save-as");
        let source = dir.0.join("from mail.pdf");
        std::fs::write(&source, b"%PDF").unwrap();
        let zone = "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://mail.example/a.pdf\r\n";
        std::fs::write(zone_stream(&source), zone).unwrap();
        let copy = dir.0.join("copy.pdf");
        std::fs::write(&copy, b"%PDF").unwrap();
        copy_zone(&copy, &source).unwrap();
        assert_eq!(read_zone(&copy).unwrap(), zone.as_bytes());
        let local = dir.0.join("local.pdf");
        std::fs::write(&local, b"%PDF").unwrap();
        let local_copy = dir.0.join("local copy.pdf");
        std::fs::write(&local_copy, b"%PDF").unwrap();
        copy_zone(&local_copy, &local).unwrap();
        assert_eq!(read_zone(&local_copy), None);
    }

    #[test]
    fn a_stream_that_isnt_a_zone_is_not_copied() {
        let dir = Dir::new("junk");
        let source = dir.0.join("odd.pdf");
        std::fs::write(&source, b"%PDF").unwrap();
        std::fs::write(zone_stream(&source), b"not a zone").unwrap();
        let target = dir.0.join("notes.txt");
        std::fs::write(&target, b"hello").unwrap();
        mark_from(&target, Some(&source)).unwrap();
        assert_eq!(read_zone(&target).unwrap(), INTERNET_ZONE.as_bytes());
        for odd in [
            "[ZoneTransfer]\r\nHostUrl=https://example.test/?ZoneId=3\r\n",
            "[Other]\r\nZoneId=3\r\n[ZoneTransfer]\r\nReferrerUrl=x\r\n",
            "[ZoneTransfer]\r\nZoneId=\r\n",
            "[ZoneTransfer]\r\nZoneId=three\r\n",
        ] {
            std::fs::write(zone_stream(&source), odd).unwrap();
            mark_from(&target, Some(&source)).unwrap();
            assert_eq!(read_zone(&target).unwrap(), INTERNET_ZONE.as_bytes(), "{odd:?}");
        }
        let huge = [INTERNET_ZONE.as_bytes(), &vec![b'x'; 70 * 1024]].concat();
        std::fs::write(zone_stream(&source), huge).unwrap();
        mark_from(&target, Some(&source)).unwrap();
        assert_eq!(read_zone(&target).unwrap(), INTERNET_ZONE.as_bytes());
    }
}
