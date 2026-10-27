use super::{SaveTarget, is_mail_temp_copy, save_dialog_folder, write_atomically, write_atomically_syncing, write_atomically_with};

#[test]
fn save_dialogs_open_in_the_documents_folder() {
    use std::path::PathBuf;
    assert_eq!(save_dialog_folder(Some("/home/a/plans/f.pdf")), Some(PathBuf::from("/home/a/plans")));
    // A new document has no folder yet; a bare name has an empty parent; the root has none.
    assert_eq!(save_dialog_folder(None), None);
    assert_eq!(save_dialog_folder(Some("f.pdf")), None);
    assert_eq!(save_dialog_folder(Some("/")), None);
    // An email attachment's temporary copy is not offered.
    assert_eq!(save_dialog_folder(Some("C:\\Users\\a\\AppData\\Local\\Microsoft\\Windows\\INetCache\\Content.Outlook\\AB12\\f.pdf")), None);
    assert_eq!(save_dialog_folder(Some("/Users/a/Library/Containers/com.apple.mail/Data/Library/Mail Downloads/UUID/f.pdf")), None);
}

/// The Windows rules for `Path::parent` (UNC share roots included) only apply on Windows.
#[cfg(windows)]
#[test]
fn save_dialog_folder_handles_unc_paths() {
    use std::path::PathBuf;
    assert_eq!(save_dialog_folder(Some(r"\\nas\projets\2026\f.pdf")), Some(PathBuf::from(r"\\nas\projets\2026")));
    assert_eq!(save_dialog_folder(Some(r"\\nas\projets\f.pdf")), Some(PathBuf::from(r"\\nas\projets\")));
    assert_eq!(save_dialog_folder(Some(r"\\nas\projets")), None);
    assert_eq!(save_dialog_folder(Some(r"C:\Users\a\f.pdf")), Some(PathBuf::from(r"C:\Users\a")));
}
use pdfcraft_platform::staging::{STAGING_ATTEMPTS, staging_suffixes};
use std::path::{Path, PathBuf};

/// A fresh, empty folder for one staging test.
fn staging_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-ui-staging-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn staged(dir: &Path, suffix: u64) -> PathBuf {
    dir.join(format!(".out.pdf.pdfcraft-{suffix:016x}.tmp"))
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

/// A symbolic link to a file, where the system allows one (Windows needs Developer Mode or an
/// administrator for it).
fn file_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, link)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(std::io::Error::other(format!("no symbolic links here: {} {}", target.display(), link.display())))
    }
}

#[test]
fn saving_never_writes_through_a_file_planted_at_the_staging_name() {
    let dir = staging_dir("planted");
    let outside = dir.join("outside.txt");
    std::fs::write(&outside, "PRECIOUS").unwrap();
    let target = dir.join("out.pdf");
    let path = target.to_string_lossy().into_owned();
    // A hard link needs no privileges on any system, and writing to it writes to `outside`.
    std::fs::hard_link(&outside, staged(&dir, 1)).unwrap();
    std::fs::write(staged(&dir, 2), "PLANTED").unwrap();
    write_atomically_with(&path, b"NEW", [1, 2, 3]).unwrap();
    assert_eq!(read(&target), "NEW");
    assert_eq!(read(&outside), "PRECIOUS");
    assert_eq!(read(&staged(&dir, 1)), "PRECIOUS");
    assert_eq!(read(&staged(&dir, 2)), "PLANTED");
    assert!(!staged(&dir, 3).exists(), "the staging file was renamed into place");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saving_never_writes_through_a_symbolic_link_at_the_staging_name() {
    let dir = staging_dir("symlink");
    let outside = dir.join("outside.txt");
    std::fs::write(&outside, "PRECIOUS").unwrap();
    let target = dir.join("out.pdf");
    if let Err(e) = file_symlink(&outside, &staged(&dir, 1)) {
        eprintln!("symbolic links not checked: {e}");
        return;
    }
    write_atomically_with(&target.to_string_lossy(), b"NEW", [1, 2]).unwrap();
    assert_eq!(read(&target), "NEW");
    assert_eq!(read(&outside), "PRECIOUS");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saving_never_creates_a_file_through_a_dangling_link_at_the_staging_name() {
    let dir = staging_dir("dangling");
    let unborn = dir.join("created-through-a-link.txt");
    let target = dir.join("out.pdf");
    if let Err(e) = file_symlink(&unborn, &staged(&dir, 1)) {
        eprintln!("symbolic links not checked: {e}");
        return;
    }
    write_atomically_with(&target.to_string_lossy(), b"NEW", [1, 2]).unwrap();
    assert_eq!(read(&target), "NEW");
    assert!(!unborn.exists(), "nothing was created through the dangling link");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_at_the_staging_name_is_left_alone() {
    let dir = staging_dir("folder");
    std::fs::create_dir(staged(&dir, 1)).unwrap();
    std::fs::write(staged(&dir, 1).join("inside.txt"), "PLANTED").unwrap();
    let target = dir.join("out.pdf");
    write_atomically_with(&target.to_string_lossy(), b"NEW", [1, 2]).unwrap();
    assert_eq!(read(&target), "NEW");
    assert_eq!(read(&staged(&dir, 1).join("inside.txt")), "PLANTED");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn saving_gives_up_rather_than_reuse_a_taken_name() {
    let dir = staging_dir("taken");
    let target = dir.join("out.pdf");
    std::fs::write(&target, "OLD").unwrap();
    for s in 1..=STAGING_ATTEMPTS as u64 {
        std::fs::write(staged(&dir, s), "PLANTED").unwrap();
    }
    assert!(write_atomically_with(&target.to_string_lossy(), b"NEW", 1..).is_err());
    assert_eq!(read(&target), "OLD");
    for s in 1..=STAGING_ATTEMPTS as u64 {
        assert_eq!(read(&staged(&dir, s)), "PLANTED");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The names in a folder: what a test can see was left behind.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

#[test]
fn saving_a_long_name_leaves_only_the_saved_file() {
    assert_ne!(staging_suffixes().next(), staging_suffixes().next(), "each save draws new names");
    // 60 four-byte characters: a 244-byte name, within every system's limit. Its staging name
    // must be too (on Linux the whole name in it would be 275 bytes).
    let dir = staging_dir("clean");
    let name = format!("{}.pdf", "\u{1F600}".repeat(60));
    let path = dir.join(&name).to_string_lossy().into_owned();
    write_atomically(&path, b"NEW").unwrap();
    write_atomically(&path, b"NEWER").unwrap();
    assert_eq!(read(&dir.join(&name)), "NEWER");
    assert_eq!(listing(&dir), [name], "no staging file is left behind");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A read-only target is refused before anything is staged, so no staging file is left
/// behind.
#[cfg(windows)]
#[test]
fn a_failed_save_removes_the_staging_file() {
    let dir = staging_dir("readonly");
    let target = dir.join("out.pdf");
    std::fs::write(&target, "OLD").unwrap();
    let mut perms = std::fs::metadata(&target).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&target, perms.clone()).unwrap();
    let e = write_atomically_with(&target.to_string_lossy(), b"NEW", [7]).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "a read-only target is refused: {e}");
    assert_eq!(read(&target), "OLD");
    assert_eq!(listing(&dir), ["out.pdf"], "the staging file was removed");
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&target, perms).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// On macOS `sync_all` asks for a full flush, which SMB and FUSE volumes (a Cryptomator vault,
/// #981) refuse as unsupported: ENOTSUP (45) or ENOTTY (25). The save still goes through.
#[cfg(target_os = "macos")]
#[test]
fn a_volume_without_full_flush_still_saves() {
    for errno in [45, 25] {
        let dir = staging_dir(&format!("nofullsync-{errno}"));
        let target = dir.join("out.pdf");
        std::fs::write(&target, "OLD").unwrap();
        let asked = std::cell::Cell::new(false);
        let refuse = |_: &std::fs::File| {
            asked.set(true);
            Err(std::io::Error::from_raw_os_error(errno))
        };
        write_atomically_syncing(&target.to_string_lossy(), b"NEW", [7], refuse).unwrap();
        assert!(asked.get(), "the full flush was tried");
        assert_eq!(read(&target), "NEW");
        assert_eq!(listing(&dir), ["out.pdf"], "no staging file is left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Only "not supported" is let through: any other flush failure fails the save and keeps the
/// original.
#[test]
fn a_failed_flush_keeps_the_original() {
    let dir = staging_dir("syncfails");
    let target = dir.join("out.pdf");
    std::fs::write(&target, "OLD").unwrap();
    let e = write_atomically_syncing(&target.to_string_lossy(), b"NEW", [7], |_| Err(std::io::Error::other("disk gone"))).unwrap_err();
    assert_eq!(e.to_string(), "disk gone");
    assert_eq!(read(&target), "OLD");
    assert_eq!(listing(&dir), ["out.pdf"], "the staging file was removed");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(target_os = "macos")]
#[test]
fn a_save_preserves_opening_settings_changed_while_staging() {
    let dir = staging_dir("opening-metadata-changed");
    let target = dir.join("out.pdf");
    std::fs::write(&target, b"OLD").unwrap();
    xattr::set(&target, "com.apple.LaunchServices.OpenWith", b"old handler").unwrap();
    xattr::set(&target, "com.apple.quarantine", b"0082;1234;Preview;").unwrap();
    let suffixes = std::iter::once_with(|| {
        xattr::set(&target, "com.apple.LaunchServices.OpenWith", b"new handler").unwrap();
        xattr::set(&target, "com.apple.quarantine", b"0082;5678;another app;").unwrap();
        7
    });
    write_atomically_with(&target.to_string_lossy(), b"NEW", suffixes).unwrap();
    assert_eq!(read(&target), "NEW");
    assert_eq!(xattr::get(&target, "com.apple.LaunchServices.OpenWith").unwrap(), Some(b"new handler".to_vec()));
    assert_eq!(xattr::get(&target, "com.apple.quarantine").unwrap(), Some(b"0082;5678;another app;".to_vec()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn outlook_attachment_copies_are_recognised_on_either_separator_and_in_any_case() {
    for p in [
        r"C:\Users\me\AppData\Local\Microsoft\Windows\INetCache\Content.Outlook\AB12CD34\Quote.pdf",
        r"c:\users\me\appdata\local\microsoft\windows\inetcache\content.outlook\ab12cd34\quote (2).pdf",
        r"C:\Users\me\AppData\Local\Microsoft\Windows\Temporary Internet Files\Content.Outlook\AB12CD34\Quote.pdf",
        r"C:\Users\me\AppData\Local\Packages\Microsoft.OutlookForWindows_8wekyb3d8bbwe\LocalCache\Quote.pdf",
        "C:/Users/me/AppData/Local/Microsoft/Windows/INetCache/Content.Outlook/AB12CD34/Quote.pdf",
    ] {
        assert!(is_mail_temp_copy(p), "{p}");
    }
}

#[test]
fn apple_mail_attachment_copies_are_recognised() {
    for p in [
        "/Users/me/Library/Containers/com.apple.mail/Data/Library/Mail Downloads/6F1C2A9E-3B4D-4E5F-8A7B-9C0D1E2F3A4B/booking.pdf",
        "/Users/me/library/containers/COM.APPLE.MAIL/data/library/mail downloads/6F1C2A9E/Booking (2).pdf",
    ] {
        assert!(is_mail_temp_copy(p), "{p}");
    }
}

#[test]
fn ordinary_paths_are_not_mail_copies() {
    for p in [
        r"C:\Users\me\Documents\Quote.pdf",
        r"C:\Users\me\OneDrive - Contoso\Content.Outlook notes\Quote.pdf",
        r"C:\Users\me\AppData\Local\Microsoft\Windows\INetCache\IE\Quote.pdf",
        "/home/microsoft.outlookforwindows/report.pdf",
        "/x/inetcache/content.outlook/y.pdf",
        "/x/temporary internet files/content.outlook/y.pdf",
        "/home/me/Documents/Quote.pdf",
        "/Users/me/Downloads/Mail Downloads/booking.pdf",
        "/Users/me/Library/Containers/com.apple.Notes/Data/Library/Mail Downloads/booking.pdf",
        "Quote.pdf",
    ] {
        assert!(!is_mail_temp_copy(p), "{p}");
    }
}

/// A one-page document with a proper xref table.
fn one_page_pdf() -> Vec<u8> {
    let objs =
        ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>", "<< /Type /Page /Parent 2 0 R >>"];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

#[test]
fn save_commits_the_open_text_box() {
    let dir = staging_dir("open-text-box");
    let dest = dir.join("out.pdf");
    let mut app = crate::PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("a.pdf", None, one_page_pdf()).unwrap();
    app.save_override = Some(dest.to_string_lossy().into_owned());
    let style = crate::content_ui::default_style();
    app.views[0].content.draft =
        Some(crate::content_ui::TextDraft { page: 0, index: None, rect: [20.0, 20.0, 120.0, 40.0], text: "Typed text".into(), style, focus: true });

    assert!(app.save_active(SaveTarget::InPlace));
    let id = app.views[0].id;
    let added = app.session.get(id).unwrap().added();
    assert_eq!(added.len(), 1, "the open text box was committed before the save");
    assert!(app.views[0].content.draft.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_asks_where_to_keep_an_email_attachment_and_leaves_the_temporary_copy_alone() {
    let dir = staging_dir("mail-temp");
    let cache = dir.join("INetCache").join("Content.Outlook").join("AB12CD34");
    std::fs::create_dir_all(&cache).unwrap();
    let attachment = cache.join("Quote.pdf");
    let original = one_page_pdf();
    std::fs::write(&attachment, &original).unwrap();
    let kept = dir.join("Quote.pdf");

    let mut app = crate::PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.open_bytes("Quote.pdf", Some(attachment.to_string_lossy().into_owned()), original.clone()).unwrap();
    assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
    app.pick_override = Some(vec![kept.to_string_lossy().into_owned()]);

    assert!(!app.save_active(SaveTarget::InPlace), "Save asks instead of writing the temporary copy");
    app.process_picked();
    assert_eq!(std::fs::read(&attachment).unwrap(), original, "the attachment's copy is untouched");
    assert!(std::fs::read(&kept).unwrap().starts_with(b"%PDF"), "the document went where the user chose");

    // The chosen file is the document's file now: the next Save writes it without asking.
    app.pick_override = None;
    assert!(app.apply_edit(pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
    assert!(app.save_active(SaveTarget::InPlace));
    let _ = std::fs::remove_dir_all(&dir);
}
