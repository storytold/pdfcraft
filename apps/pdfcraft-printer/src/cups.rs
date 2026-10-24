//! CUPS 2.x PDF destination. Preserve application-supplied PDF bytes without rendering.
#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_JOB: u64 = 256 * 1024 * 1024;

fn read_job(input: impl Read, canceled: &AtomicBool) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    let mut input = input.take(MAX_JOB + 1);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        check_cancel(canceled)?;
        let count = input.read(&mut chunk).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        if bytes.len().saturating_add(count) as u64 > MAX_JOB {
            return Err("PDF print job exceeds 256 MiB".into());
        }
        bytes.try_reserve(count).map_err(|_| "Not enough memory for the PDF print job")?;
        bytes.extend_from_slice(chunk.get(..count).ok_or("Invalid PDF read size")?);
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err("PdfCraft Printer accepts PDF print jobs; no raster or PostScript fallback was used".into());
    }
    Ok(bytes)
}

fn account(record: &str, requested: &str) -> Result<(u32, u32), String> {
    let fields: Vec<_> = record.trim_end().split(':').collect();
    if fields.len() != 7 || fields.first().copied() != Some(requested) {
        return Err("The print owner is not a local account".into());
    }
    let uid: u32 = fields.get(2).ok_or("Missing user id")?.parse().map_err(|_| "Invalid user id")?;
    let gid: u32 = fields.get(3).ok_or("Missing group id")?.parse().map_err(|_| "Invalid group id")?;
    if uid == 0 {
        return Err("Printing as root is not supported".into());
    }
    Ok((uid, gid))
}

fn save(directory: &Path, job: &str, bytes: &[u8], canceled: &AtomicBool) -> Result<PathBuf, String> {
    if job.is_empty() || job.len() > 20 || !job.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Invalid CUPS job id".into());
    }
    let mut temporary =
        tempfile::Builder::new().prefix(&format!("job-{job}-")).suffix(".partial").tempfile_in(directory).map_err(|e| e.to_string())?;
    for chunk in bytes.chunks(64 * 1024) {
        check_cancel(canceled)?;
        temporary.write_all(chunk).map_err(|e| e.to_string())?;
    }
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    check_cancel(canceled)?;
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt as _;
        let owner = directory.metadata().map_err(|e| e.to_string())?;
        rustix::fs::fchown(temporary.as_file(), Some(rustix::process::Uid::from_raw(owner.uid())), Some(rustix::process::Gid::from_raw(owner.gid())))
            .map_err(|e| e.to_string())?;
    }
    check_cancel(canceled)?;
    let path = temporary.path().with_extension("pdf");
    temporary.persist_noclobber(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

fn check_cancel(canceled: &AtomicBool) -> Result<(), String> {
    if canceled.load(Ordering::Relaxed) { Err("PDF print job canceled".into()) } else { Ok(()) }
}

#[cfg(target_os = "linux")]
pub fn run() -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _};
    let canceled = std::sync::Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, canceled.clone()).map_err(|e| e.to_string())?;
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() == 1 {
        println!("direct pdfcraft:/ \"PdfCraft Printer\" \"PdfCraft Printer\"");
        return Ok(());
    }
    if !matches!(args.len(), 6 | 7) || std::env::var("DEVICE_URI").as_deref() != Ok("pdfcraft:/") {
        return Err("Use the installed PdfCraft Printer CUPS queue".into());
    }
    if !rustix::process::getuid().is_root() {
        return Err("CUPS must run this backend with its installed root-only permissions".into());
    }
    let user = args.get(2).and_then(|s| s.to_str()).ok_or("Missing print owner")?;
    if user.is_empty() || user.len() > 256 || user.starts_with('-') || !user.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)) {
        return Err("Invalid local print owner".into());
    }
    let lookup = std::process::Command::new("/usr/bin/getent").args(["passwd", user]).output().map_err(|e| e.to_string())?;
    if !lookup.status.success() {
        return Err("The print owner has no local account".into());
    }
    let (uid, gid) = account(std::str::from_utf8(&lookup.stdout).map_err(|_| "Invalid local account record")?, user)?;
    let base = Path::new("/var/spool/pdfcraft-printer");
    let metadata = base.symlink_metadata().map_err(|e| format!("The private printer output directory is unavailable: {e}"))?;
    if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err("The printer output directory must be owned by root and not writable by other accounts".into());
    }
    let directory = base.join(uid.to_string());
    match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {
            let file = std::fs::File::open(&directory).map_err(|e| e.to_string())?;
            rustix::fs::fchown(&file, Some(rustix::process::Uid::from_raw(uid)), Some(rustix::process::Gid::from_raw(gid)))
                .map_err(|e| e.to_string())?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.to_string()),
    }
    let metadata = directory.symlink_metadata().map_err(|e| e.to_string())?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err("The user's printer output directory is not private".into());
    }
    let bytes = if let Some(path) = args.get(6) {
        read_job(std::fs::File::open(path).map_err(|e| e.to_string())?, &canceled)?
    } else {
        read_job(std::io::stdin().lock(), &canceled)?
    };
    let job = args.get(1).and_then(|s| s.to_str()).ok_or("Invalid CUPS job id")?;
    let path = save(&directory, job, &bytes, &canceled)?;
    eprintln!("INFO: PdfCraft Printer saved {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pdf_bytes_survive_and_repeated_jobs_never_overwrite() {
        let canceled = AtomicBool::new(false);
        let input = b"%PDF-1.4\nselectable text, vectors and original objects\n%%EOF";
        let directory = tempfile::tempdir().unwrap();
        let bytes = read_job(input.as_slice(), &canceled).unwrap();
        let a = save(directory.path(), "123", &bytes, &canceled).unwrap();
        let b = save(directory.path(), "123", &bytes, &canceled).unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read(a).unwrap(), input);
        assert_eq!(std::fs::read(b).unwrap(), input);
        canceled.store(true, Ordering::Relaxed);
        assert!(save(directory.path(), "124", input, &canceled).is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
        canceled.store(false, Ordering::Relaxed);
        assert!(read_job(b"%!PS".as_slice(), &canceled).is_err());
        assert!(save(directory.path(), "../../bad", input, &canceled).is_err());
    }
    #[test]
    fn account_lookup_requires_an_exact_non_root_owner() {
        assert_eq!(account("alice:x:1000:1000::/home/alice:/bin/sh\n", "alice").unwrap(), (1000, 1000));
        assert!(account("root:x:0:0::/root:/bin/sh", "root").is_err());
        assert!(account("bob:x:1000:1000::/home/bob:/bin/sh", "alice").is_err());
        assert!(account("alice:x:1000:1000", "alice").is_err());
    }
}
