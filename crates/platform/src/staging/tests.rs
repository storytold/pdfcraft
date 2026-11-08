use super::*;

/// A fresh, empty folder for one test.
fn test_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-platform-staging-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

/// The callers' tests plant files at these exact names; if a name changed, those tests would
/// plant where nothing writes and pass without testing anything.
#[test]
fn each_form_keeps_its_exact_name() {
    let dir = test_dir("forms");
    let (tools, _) = create_staging(&dir, "out.pdf", StagingName::SuffixThenTag, [0xab]).unwrap();
    assert_eq!(file_name(&tools), ".out.pdf.00000000000000ab.pdfcraft-tmp");
    let (desktop, _) = create_staging(&dir, "out.pdf", StagingName::TagThenSuffix, [0xab]).unwrap();
    assert_eq!(file_name(&desktop), ".out.pdf.pdfcraft-00000000000000ab.tmp");
    assert_eq!(std::fs::read(&tools).unwrap(), b"", "a new, empty file");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_long_name_is_cut_to_128_bytes_between_characters() {
    let dir = test_dir("long");
    // One byte, then four-byte characters: the 128-byte limit falls inside the 32nd one.
    let name = format!("a{}.pdf", "\u{1F600}".repeat(40));
    let (tmp, _) = create_staging(&dir, &name, StagingName::SuffixThenTag, [1]).unwrap();
    assert_eq!(file_name(&tmp), format!(".a{}.0000000000000001.pdfcraft-tmp", "\u{1F600}".repeat(31)));
    // One-byte characters: exactly 128 of them are kept.
    let (tmp, _) = create_staging(&dir, &"x".repeat(200), StagingName::SuffixThenTag, [2]).unwrap();
    assert_eq!(file_name(&tmp), format!(".{}.0000000000000002.pdfcraft-tmp", "x".repeat(128)));
    let _ = std::fs::remove_dir_all(&dir);
}

/// #2658: a read-only file is refused before anything is staged, on Unix (where a rename would
/// replace it) as on Windows (where the rename fails).
#[test]
fn a_read_only_target_is_refused_and_kept() {
    let dir = test_dir("readonly");
    let target = dir.join("out.pdf");
    std::fs::write(&target, "OLD").unwrap();
    let mut perms = std::fs::metadata(&target).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&target, perms.clone()).unwrap();
    let staged = std::cell::Cell::new(false);
    let e = write_atomically_with(&target, b"NEW", StagingName::TagThenSuffix, [7], |_: &std::fs::File| {
        staged.set(true);
        Ok(())
    })
    .unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
    assert!(!staged.get(), "nothing was staged");
    assert_eq!(std::fs::read(&target).unwrap(), b"OLD");
    assert!(std::fs::metadata(&target).unwrap().permissions().readonly(), "still read-only");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no staging file is left");
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&target, perms).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn running_out_of_suffixes_refuses_and_creates_nothing() {
    let dir = test_dir("empty");
    let e = create_staging(&dir, "out.pdf", StagingName::TagThenSuffix, []).unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_the_first_sixteen_names_are_tried() {
    let dir = test_dir("cap");
    let name = |s: u64| dir.join(format!(".out.pdf.{s:016x}.pdfcraft-tmp"));
    for s in 1..=STAGING_ATTEMPTS as u64 {
        std::fs::write(name(s), "PLANTED").unwrap();
    }
    assert!(create_staging(&dir, "out.pdf", StagingName::SuffixThenTag, 1..).is_err());
    assert!(!name(STAGING_ATTEMPTS as u64 + 1).exists(), "a 17th name was tried");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `write_atomically`: the target holds the new bytes only once they are complete; a refused
/// flush leaves the old file and removes the staging file; a missing folder writes nothing.
#[test]
fn an_atomic_write_replaces_the_target_only_when_complete() {
    let dir = test_dir("write");
    let target = dir.join("out.pdf");
    std::fs::write(&target, b"OLD").unwrap();
    let names = |dir: &Path| {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| file_name(&e.unwrap().path())).collect();
        v.sort();
        v
    };
    // The staging file is written and flushed before the target changes.
    let mut seen_old = false;
    write_atomically_with(&target, b"NEW", StagingName::SuffixThenTag, [7], |f| {
        seen_old = std::fs::read(&target).unwrap() == b"OLD" && f.metadata().unwrap().len() == 3;
        Ok(())
    })
    .unwrap();
    assert!(seen_old, "flushed before publication, with the target still the old file");
    assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
    assert_eq!(names(&dir), ["out.pdf"], "no staging file is left");
    // A refused flush keeps the target and cleans up.
    let e = write_atomically_with(&target, b"NEWER", StagingName::TagThenSuffix, [8], |_| Err(std::io::Error::other("refused"))).unwrap_err();
    assert!(e.to_string().contains("refused"));
    assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
    assert_eq!(names(&dir), ["out.pdf"]);
    // A folder that does not exist: nothing is written anywhere.
    assert!(write_atomically(&dir.join("missing").join("out.pdf"), b"X", StagingName::SuffixThenTag).is_err());
    assert_eq!(names(&dir), ["out.pdf"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(target_os = "macos")]
#[test]
fn atomic_saves_keep_finder_tags_and_binary_extended_attributes() {
    use rustix::fs::{XattrFlags, fgetxattr, setxattr};
    use std::os::macos::fs::FileTimesExt as _;
    let dir = test_dir("xattrs");
    let target = dir.join("out.pdf");
    let attributes: &[(&str, &[u8])] = &[
        ("com.apple.metadata:_kMDItemUserTags", b"<plist version=\"1.0\"><array><string>PdfCraft\n6</string></array></plist>"),
        ("com.pdfcraft.test.binary", b"\0\xff\x80original\0"),
        ("com.pdfcraft.test.empty", b""),
    ];
    std::fs::write(&target, b"OLD").unwrap();
    for (name, value) in attributes {
        setxattr(&target, *name, value, XattrFlags::empty()).unwrap();
    }
    let created = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_577_934_245);
    let original = std::fs::OpenOptions::new().write(true).open(&target).unwrap();
    original.set_times(std::fs::FileTimes::new().set_created(created)).unwrap();
    assert_eq!(original.metadata().unwrap().created().unwrap(), created);
    drop(original);
    // Both desktop and automation name forms, including a second save of the replacement.
    for form in [StagingName::TagThenSuffix, StagingName::SuffixThenTag] {
        write_atomically_with(&target, b"NEW", form, [7], |staged| {
            assert_eq!(staged.metadata().unwrap().created().unwrap(), created, "creation date must survive attribute copying before flush");
            for (name, value) in attributes {
                let mut bytes = [0u8; 256];
                let len = fgetxattr(staged, *name, &mut bytes[..]).unwrap();
                assert_eq!(&bytes[..len], *value, "metadata must be present before flush: {name}");
            }
            Ok(())
        })
        .unwrap();
        let saved = std::fs::File::open(&target).unwrap();
        assert_eq!(saved.metadata().unwrap().created().unwrap(), created);
        for (name, value) in attributes {
            let mut bytes = [0u8; 256];
            let len = fgetxattr(&saved, *name, &mut bytes[..]).unwrap();
            assert_eq!(&bytes[..len], *value);
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn a_failed_flush_keeps_the_original_extended_attributes() {
    use rustix::fs::{XattrFlags, getxattr, setxattr};
    let dir = test_dir("xattrs-flush");
    let target = dir.join("out.pdf");
    std::fs::write(&target, b"OLD").unwrap();
    setxattr(&target, "com.pdfcraft.test", b"original", XattrFlags::empty()).unwrap();
    let e = write_atomically_with(&target, b"NEW", StagingName::TagThenSuffix, [9], |_| Err(std::io::Error::other("flush refused"))).unwrap_err();
    assert!(e.to_string().contains("flush refused"));
    assert_eq!(std::fs::read(&target).unwrap(), b"OLD");
    let mut value = [0u8; 64];
    let len = getxattr(&target, "com.pdfcraft.test", &mut value[..]).unwrap();
    assert_eq!(&value[..len], b"original");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Model an HFS-style partial read while using real macOS metadata and the atomic writer.
/// The shared read validator must reject growth before the staging file can be published.
#[cfg(target_os = "macos")]
#[test]
fn a_growing_resource_fork_refuses_publication_and_keeps_original() {
    use rustix::fs::{XattrFlags, fgetxattr, fsetxattr, getxattr, setxattr};
    let dir = test_dir("xattrs-growing-fork");
    let target = dir.join("out.pdf");
    let name = "com.apple.ResourceFork";
    for form in [StagingName::TagThenSuffix, StagingName::SuffixThenTag] {
        std::fs::write(&target, b"ORIGINAL PDF").unwrap();
        setxattr(&target, name, b"ABC", XattrFlags::empty()).unwrap();
        let source = std::fs::File::open(&target).unwrap();
        let result = write_atomically_with(&target, b"REPLACEMENT PDF", form, [23], |staged| {
            let size = fgetxattr(&source, name, &mut [] as &mut [u8])?;
            let value = read_xattr_value(size, |buffer| {
                if buffer.is_empty() {
                    return Ok(fgetxattr(&source, name, buffer)?);
                }
                // HFS can return the prefix that fits instead of ERANGE. APFS need not
                // have that behaviour: model the kernel response, not a timing race.
                setxattr(&target, name, b"ABCD", XattrFlags::empty())?;
                buffer.get_mut(..3).unwrap().copy_from_slice(b"ABC");
                Ok(3)
            })?;
            fsetxattr(staged, name, &value, XattrFlags::empty())?;
            Ok(())
        });
        let error = result.expect_err("a short prefix of the grown fork must not be published");
        assert!(error.to_string().contains("changed size"), "{error}");
        assert_eq!(std::fs::read(&target).unwrap(), b"ORIGINAL PDF");
        let mut bytes = [0u8; 8];
        let len = getxattr(&target, name, &mut bytes[..]).unwrap();
        assert_eq!(&bytes[..len], b"ABCD", "the original's grown fork must remain intact");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "staging files are removed on refusal");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// Stable empty/binary values are accepted; short reads, growth and failed rechecks are refused.
#[cfg(target_os = "macos")]
#[test]
fn attribute_reads_require_a_complete_value_and_stable_size() {
    for bytes in [&b""[..], &b"\0\xffB"[..]] {
        let value = read_xattr_value(bytes.len(), |buffer| {
            if !buffer.is_empty() {
                buffer.get_mut(..bytes.len()).unwrap().copy_from_slice(bytes);
            }
            Ok(bytes.len())
        })
        .unwrap();
        assert_eq!(value, bytes);
    }
    for (before, read_len, after) in [(3, 2, 3), (3, 3, 4), (0, 1, 1), (3, 2, 2)] {
        let result = read_xattr_value(before, |buffer| Ok(if buffer.is_empty() { after } else { read_len }));
        assert!(result.unwrap_err().to_string().contains("changed size"));
    }
    let result = read_xattr_value(3, |buffer| {
        if buffer.is_empty() { Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "size recheck refused")) } else { Ok(3) }
    });
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::PermissionDenied);
}

#[cfg(target_os = "macos")]
#[test]
fn oversized_extended_attributes_refuse_the_save_without_replacing_the_target() {
    use rustix::fs::{XattrFlags, getxattr, setxattr};
    let dir = test_dir("xattrs-limit");
    let target = dir.join("out.pdf");
    std::fs::write(&target, b"OLD").unwrap();
    // A resource fork can exceed the ordinary attribute limit on macOS. No fixture file
    // is committed; only the OS metadata of this temporary synthetic file holds the bytes.
    let fork = vec![42u8; 16 * 1024 * 1024 + 1];
    setxattr(&target, "com.apple.ResourceFork", &fork, XattrFlags::empty()).unwrap();
    let mut flushed = false;
    let e = write_atomically_with(&target, b"NEW", StagingName::TagThenSuffix, [10], |_| {
        flushed = true;
        Ok(())
    })
    .unwrap_err();
    assert!(e.to_string().contains("extended attributes"), "{e}");
    assert!(e.to_string().contains("Save As"), "{e}");
    assert!(!flushed, "metadata failure must stop before flush and rename");
    assert_eq!(std::fs::read(&target).unwrap(), b"OLD");
    let size = getxattr(&target, "com.apple.ResourceFork", &mut [] as &mut [u8]).unwrap();
    assert_eq!(size, fork.len());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn replacing_a_symlink_does_not_copy_attributes_from_its_referent() {
    use rustix::fs::{XattrFlags, getxattr, setxattr};
    let dir = test_dir("xattrs-symlink");
    let outside = test_dir("xattrs-outside").join("private.pdf");
    std::fs::write(&outside, b"PRIVATE").unwrap();
    setxattr(&outside, "com.pdfcraft.test", b"private metadata", XattrFlags::empty()).unwrap();
    let target = dir.join("out.pdf");
    std::os::unix::fs::symlink(&outside, &target).unwrap();
    write_atomically(&target, b"NEW", StagingName::TagThenSuffix).unwrap();
    assert_eq!(std::fs::read(&outside).unwrap(), b"PRIVATE");
    assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
    assert!(!target.symlink_metadata().unwrap().file_type().is_symlink());
    let mut bytes = [0u8; 64];
    assert_eq!(getxattr(&target, "com.pdfcraft.test", &mut bytes[..]).unwrap_err(), rustix::io::Errno::NOATTR);
    let len = getxattr(&outside, "com.pdfcraft.test", &mut bytes[..]).unwrap();
    assert_eq!(&bytes[..len], b"private metadata");
    std::fs::remove_dir_all(dir).unwrap();
    std::fs::remove_dir_all(outside.parent().unwrap()).unwrap();
}

/// #1790: a replaced file keeps its permissions, and its staging file already has them while
/// it is written; a new file gets the same mode as any other new file.
#[cfg(unix)]
#[test]
fn a_replaced_file_keeps_its_permissions() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = test_dir("modes");
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let target = dir.join("out.pdf");
    for keep in [0o600, 0o640, 0o664] {
        std::fs::write(&target, b"OLD").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(keep)).unwrap();
        let mut staged = 0;
        write_atomically_with(&target, b"NEW", StagingName::SuffixThenTag, [keep as u64], |f| {
            staged = f.metadata()?.permissions().mode() & 0o777;
            Ok(())
        })
        .unwrap();
        assert_eq!(staged, keep, "the staging file has the target's mode before it is flushed");
        assert_eq!(mode(&target), keep, "the saved file keeps mode {keep:o}");
        assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
    }
    let fresh = dir.join("new.pdf");
    write_atomically(&fresh, b"NEW", StagingName::TagThenSuffix).unwrap();
    let plain = dir.join("plain");
    std::fs::write(&plain, b"").unwrap();
    assert_eq!(mode(&fresh), mode(&plain), "a new file is not made private");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A replaced file keeps its group as well as its mode, so group permissions never reach a
/// group that couldn't read the file before (the saver's own group, by default).
#[cfg(target_os = "linux")]
#[test]
fn a_replaced_file_keeps_its_group() {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let dir = test_dir("group");
    let target = dir.join("out.pdf");
    std::fs::write(&target, b"OLD").unwrap();
    let created = std::fs::metadata(&target).unwrap().gid();
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let groups = status.lines().find_map(|l| l.strip_prefix("Groups:")).unwrap_or_default();
    let Some(other) = groups.split_whitespace().filter_map(|g| g.parse::<u32>().ok()).find(|&g| g != created) else {
        eprintln!("skipped: the test needs a second group");
        return;
    };
    std::os::unix::fs::chown(&target, None, Some(other)).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
    let mut staged = (0, 0);
    write_atomically_with(&target, b"NEW", StagingName::SuffixThenTag, [7], |f| {
        let m = f.metadata()?;
        staged = (m.gid(), m.permissions().mode() & 0o777);
        Ok(())
    })
    .unwrap();
    assert_eq!(staged, (other, 0o640), "the staging file has the target's group before it is flushed");
    let m = std::fs::metadata(&target).unwrap();
    assert_eq!((m.gid(), m.permissions().mode() & 0o777), (other, 0o640));
    let _ = std::fs::remove_dir_all(&dir);
}

/// #2658: a save replaces the file, but the document is the same one: it keeps its creation
/// date where the system lets a program set it.
#[cfg(any(windows, target_os = "macos"))]
#[test]
fn a_replaced_file_keeps_its_creation_date() {
    #[cfg(target_os = "macos")]
    use std::os::macos::fs::FileTimesExt as _;
    #[cfg(windows)]
    use std::os::windows::fs::FileTimesExt as _;
    let dir = test_dir("created");
    let target = dir.join("doc.pdf");
    std::fs::write(&target, b"OLD").unwrap();
    // 2020-01-02 03:04:05 UTC.
    let then = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_577_934_245);
    let file = std::fs::OpenOptions::new().write(true).open(&target).unwrap();
    file.set_times(std::fs::FileTimes::new().set_created(then)).unwrap();
    drop(file);
    assert_eq!(std::fs::metadata(&target).unwrap().created().unwrap(), then, "the fixture's date is set");
    write_atomically(&target, b"NEW", StagingName::SuffixThenTag).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
    assert_eq!(std::fs::metadata(&target).unwrap().created().unwrap(), then, "the saved file keeps its creation date");
    // A new file gets a date of its own.
    let fresh = dir.join("new.pdf");
    write_atomically(&fresh, b"NEW", StagingName::SuffixThenTag).unwrap();
    assert!(std::fs::metadata(&fresh).unwrap().created().unwrap() > then);
    let _ = std::fs::remove_dir_all(&dir);
}

/// #4593: a save replaces the file, so a hidden or system file stays hidden or system, and
/// its staging file already is while it is written; a plain file stays plain, though the
/// staging name starts with a dot; a new file gets no attribute from anywhere.
#[cfg(windows)]
#[test]
fn a_replaced_file_keeps_its_hidden_and_system_attributes() {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    let dir = test_dir("attributes");
    let kept = |p: &Path| std::fs::metadata(p).unwrap().file_attributes() & (HIDDEN | SYSTEM);
    for (i, keep) in [0, HIDDEN, SYSTEM, HIDDEN | SYSTEM].into_iter().enumerate() {
        let target = dir.join(format!("doc{i}.pdf"));
        let mut fixture = std::fs::OpenOptions::new().write(true).create_new(true).attributes(keep).open(&target).unwrap();
        std::io::Write::write_all(&mut fixture, b"OLD").unwrap();
        drop(fixture);
        assert_eq!(kept(&target), keep, "the fixture has attributes {keep:#x}");
        for form in [StagingName::TagThenSuffix, StagingName::SuffixThenTag] {
            let mut staged = u32::MAX;
            write_atomically_with(&target, b"NEW", form, [i as u64], |f| {
                staged = f.metadata()?.file_attributes() & (HIDDEN | SYSTEM);
                Ok(())
            })
            .unwrap();
            assert_eq!(staged, keep, "the staging file has the target's attributes before it is flushed");
            assert_eq!(kept(&target), keep, "the saved file keeps attributes {keep:#x}");
            assert_eq!(std::fs::read(&target).unwrap(), b"NEW");
        }
    }
    let fresh = dir.join("new.pdf");
    write_atomically(&fresh, b"NEW", StagingName::TagThenSuffix).unwrap();
    assert_eq!(kept(&fresh), 0, "a new file is neither hidden nor system");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A save replaces a symbolic link, not the file it points to, so the saved file takes the
/// link's own attributes: a plain link to a hidden file saves as a plain file, and the file
/// it pointed to is left as it was.
#[cfg(windows)]
#[test]
fn a_replaced_link_keeps_its_own_attributes_not_its_targets() {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
    const HIDDEN: u32 = 0x2;
    let dir = test_dir("link-attributes");
    let referent = dir.join("referent.pdf");
    let mut fixture = std::fs::OpenOptions::new().write(true).create_new(true).attributes(HIDDEN).open(&referent).unwrap();
    std::io::Write::write_all(&mut fixture, b"OLD").unwrap();
    drop(fixture);
    let link = dir.join("link.pdf");
    if let Err(e) = std::os::windows::fs::symlink_file(&referent, &link) {
        eprintln!("skipped: creating a symbolic link needs Developer Mode or administrator rights ({e})");
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    assert_eq!(std::fs::symlink_metadata(&link).unwrap().file_attributes() & HIDDEN, 0, "the link itself is visible");
    write_atomically(&link, b"NEW", StagingName::TagThenSuffix).unwrap();
    let saved = std::fs::symlink_metadata(&link).unwrap();
    assert!(!saved.file_type().is_symlink(), "the save replaced the link");
    assert_eq!(saved.file_attributes() & HIDDEN, 0, "the link was visible, so the saved file is");
    assert_eq!(std::fs::read(&link).unwrap(), b"NEW");
    assert_eq!(std::fs::read(&referent).unwrap(), b"OLD", "the file the link pointed to is untouched");
    assert_ne!(std::fs::metadata(&referent).unwrap().file_attributes() & HIDDEN, 0, "and still hidden");
    let _ = std::fs::remove_dir_all(&dir);
}
