#[test]
fn remote_documents_must_be_regular_pdf_files() {
    use super::{RemoteDocumentPathError as E, read_remote_document as read};
    let dir = std::env::temp_dir().join(format!("pdfcraft-gotor-read-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("folder.pdf")).unwrap();
    std::fs::write(dir.join("doc.PDF"), b"%PDF-1.7\n").unwrap();
    std::fs::write(dir.join("notes.txt"), b"text").unwrap();
    assert_eq!(read(&dir.join("doc.PDF")).as_deref(), Ok(&b"%PDF-1.7\n"[..]));
    assert_eq!(read(&dir.join("folder.pdf")), Err(E::NotAFile), "a directory is refused");
    assert_eq!(read(&dir.join("notes.txt")), Err(E::NotPdf));
    assert_eq!(read(&dir), Err(E::NotPdf));
    #[cfg(unix)]
    {
        // Sparse: the size guard must reject this without allocating or reading a GiB.
        let large = dir.join("large.pdf");
        std::fs::File::create(&large).unwrap().set_len(super::MAX_REMOTE_DOCUMENT_BYTES + 1).unwrap();
        assert_eq!(read(&large), Err(E::TooLarge));
    }
    assert!(matches!(read(&dir.join("missing.pdf")), Err(E::Read(_))));
    #[cfg(unix)]
    {
        // A device (like /dev/zero, which never ends) and a FIFO (which blocks) are never opened.
        std::os::unix::fs::symlink("/dev/zero", dir.join("zero.pdf")).unwrap();
        assert_eq!(read(&dir.join("zero.pdf")), Err(E::NotAFile));
        let fifo = dir.join("pipe.pdf");
        if std::process::Command::new("mkfifo").arg(&fifo).status().is_ok_and(|s| s.success()) {
            assert_eq!(read(&fifo), Err(E::NotAFile));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remote_document_absolute_classification_uses_the_original_platform_path() {
    use super::remote_document_path_is_absolute as absolute;
    for relative in ["target.pdf", "../appendix/target.pdf", "folder/target.pdf"] {
        assert!(!absolute(relative), "{relative}");
    }
    #[cfg(unix)]
    {
        assert!(absolute("/manual/target.pdf"));
        assert!(!absolute(r"C:\manual\target.pdf"));
    }
    #[cfg(windows)]
    {
        assert!(absolute(r"C:\manual\target.pdf"));
        assert!(absolute("C:/manual/target.pdf"));
        assert!(!absolute(r"C:target.pdf"), "drive-relative is not absolute");
        assert!(!absolute(r"\manual\target.pdf"), "rooted without a drive is not absolute");
    }
}

#[test]
fn remote_document_paths_are_local_and_source_relative() {
    use super::{RemoteDocumentPathError as E, resolve_remote_document_path as resolve};
    use std::path::Path;

    let source = std::env::temp_dir().join("chapter").join("source.pdf");
    let source = source.as_path();
    let folder = source.parent().unwrap();
    assert_eq!(resolve(Some(source), "target.pdf"), Ok(folder.join("target.pdf")));
    assert_eq!(resolve(Some(source), "../target.pdf").as_deref(), Ok(folder.parent().unwrap().join("target.pdf").as_path()));
    #[cfg(unix)]
    assert_eq!(resolve(Some(source), "/manual/target.pdf").as_deref(), Ok(Path::new("/manual/target.pdf")));
    assert_eq!(resolve(None, "target.pdf"), Err(E::MissingSourcePath));
    assert_eq!(resolve(Some(Path::new("source.pdf")), "target.pdf"), Err(E::MissingSourcePath), "a relative source is not used");
    #[cfg(unix)]
    assert_eq!(resolve(Some(source), "C:\\x.pdf"), Err(E::NotLocal(craft_docpath::Refused::Invalid)), "a Windows path on Unix needs no source");
    assert_eq!(resolve(Some(source), ""), Err(E::Empty));
    assert_eq!(resolve(Some(source), "   "), Err(E::Empty));
    for url in
        ["file:///tmp/a.pdf", "file://h/s/a.pdf", "https://example.org/a.pdf", "smb://host/share/a.pdf", "ftp://host/a.pdf", "custom:target.pdf"]
    {
        assert!(matches!(resolve(Some(source), url), Err(E::NotLocal(_))), "{url}");
    }
    assert_eq!(resolve(Some(source), "folder/hidden\u{200B}.pdf"), Err(E::Hidden));
    #[cfg(windows)]
    assert_eq!(resolve(Some(source), "folder/a:b.pdf"), Err(E::NotLocal(craft_docpath::Refused::Invalid)));
    assert_eq!(resolve(Some(source), &"a".repeat(super::MAX_REMOTE_DOCUMENT_PATH_BYTES + 1)), Err(E::TooLong));
}

#[test]
fn remote_document_paths_to_other_computers_and_devices_are_refused() {
    use super::{RemoteDocumentPathError as E, resolve_remote_document_path as resolve};

    let source = std::env::temp_dir().join("chapter").join("source.pdf");
    let source = source.as_path();
    // Refused before any filesystem access: none of these exist, and a stat of a network path
    // would contact its host.
    for not_local in [
        "//server/share/a.pdf",
        "\\\\server\\share\\a.pdf",
        "\\\\h\\s\\a.pdf",
        "\\\\?\\UNC\\h\\s\\a.pdf",
        "\\??\\UNC\\h\\s\\a.pdf",
        "/??/x",
        "\\??\\C:\\a.pdf",
        "\\\\.\\device\\a.pdf",
        "/\\server/share/a.pdf",
        "NUL.pdf",
    ] {
        assert!(matches!(resolve(Some(source), not_local), Err(E::NotLocal(_))), "{not_local:?}");
    }
    // Linux reads `//tmp/x` as `/tmp/x`, but on Windows it names a share.
    #[cfg(unix)]
    {
        let doubled = format!("/{}", source.with_file_name("a.pdf").display());
        assert!(matches!(resolve(Some(source), &doubled), Err(E::NotLocal(_))), "{doubled}");
    }
}

#[test]
fn urls_follow_the_artcraft_scheme() {
    assert_eq!(super::APP_PAGE, format!("{}/apps/{}", super::WEBSITE, super::APP));
    assert_eq!(super::GITHUB, format!("https://github.com/storytold/{}", super::APP));
    for l in super::LINKS {
        assert!(l.url.starts_with("https://"), "{}", l.url);
        assert!(crate::commands::command(l.command).is_some(), "{} is a registered command", l.command);
    }
}

#[test]
fn documents_may_offer_web_and_email_addresses() {
    use super::document_url;
    for ok in [
        "https://example.org",
        "HTTPS://Example.org/a?b=c#d",
        "http://example.org:8080/x",
        "mailto:someone@example.org",
        "https://[::1]:8443/",
        // Plain spaces turn up in real documents' links and are visible in the dialog.
        "mailto:orders@example.org?subject=Order form",
        "https://example.org/My Report.pdf",
    ] {
        assert_eq!(document_url(ok).as_deref(), Ok(ok), "{ok}");
    }
    let multibyte = format!("https://example.org/{}", "é".repeat(super::MAX_DOCUMENT_URL - 20));
    assert!(document_url(&multibyte).is_ok(), "the limit counts characters, not bytes");
    assert_eq!(document_url("  https://example.org/ \n").as_deref(), Ok("https://example.org/"), "surrounding whitespace is trimmed");
}

#[test]
fn documents_may_not_offer_other_kinds_of_address() {
    use super::{BlockedLink as B, MAX_DOCUMENT_URL, document_url};
    let scheme = |s: &str| Err(B::Scheme(s.into()));
    assert_eq!(document_url("file:///C:/Windows/System32/calc.exe"), scheme("file"));
    assert_eq!(document_url("FILE://server/share/x.exe"), scheme("file"));
    assert_eq!(document_url("javascript:alert(1)"), scheme("javascript"));
    assert_eq!(document_url("data:text/html,<script>x</script>"), scheme("data"));
    assert_eq!(document_url("ms-settings:privacy"), scheme("ms-settings"));
    assert_eq!(document_url("smb://server/share"), scheme("smb"));
    assert_eq!(document_url(""), Err(B::Empty));
    assert_eq!(document_url("   "), Err(B::Empty));
    for bad in [
        "example.org",
        "//example.org",
        "1http://x",
        "ht tp://x",
        "https:",
        "https://",
        "https:///path",
        "https://user@/x",
        "mailto:",
        "https://trusted.example /x",
    ] {
        assert_eq!(document_url(bad), Err(B::Malformed), "{bad}");
    }
    for hidden in [
        "https://exa\u{202E}gro.elpmaxe",
        "https://example.org\u{200B}.evil",
        "https://a.org/\u{0007}",
        "https://a.org/x\ty",
        "https://a.org/x\ny",
        "https://example.org/a\u{00A0}b",
    ] {
        assert_eq!(document_url(hidden), Err(B::Hidden), "{hidden:?}");
    }
    let long = format!("https://example.org/?q={}", "a".repeat(MAX_DOCUMENT_URL));
    assert!(matches!(document_url(&long), Err(B::TooLong(_))));
}

#[test]
fn the_host_shown_is_the_one_the_address_goes_to() {
    use super::host;
    assert_eq!(host("https://example.org/path"), Some("example.org"));
    assert_eq!(host("https://trusted.example@other.example/"), Some("other.example"));
    assert_eq!(host("http://user:pw@other.example:8080?x"), Some("other.example"));
    assert_eq!(host("https://[::1]:8443/"), Some("[::1]"));
    // Browsers read `\` as `/`, so this goes to other.example, not trusted.example.
    assert_eq!(host("https://other.example\\@trusted.example/"), Some("other.example"));
    assert_eq!(host("https://other.example\\x@trusted.example/"), Some("other.example"));
    assert_eq!(host("mailto:a@example.org"), None);
    assert_eq!(host("https:///nohost"), None);
}

#[test]
fn addresses_the_url_parser_rejects_are_refused() {
    use super::{BlockedLink as B, document_url};
    // The webbrowser crate would open each of these as a local file path.
    for bad in ["mailto://a^b/x", "mailto://a:99999/", "mailto://[x]/", "mailto://a\\b/", "mailto://a b/", "https://a^b/", "http://a<b/"] {
        assert_eq!(document_url(bad), Err(B::Malformed), "{bad}");
    }
    for ok in ["mailto://example.org/x", "mailto:a@example.org?subject=a b"] {
        assert_eq!(document_url(ok).as_deref(), Ok(ok), "{ok}");
    }
}

#[test]
fn email_links_may_not_ask_for_a_local_file() {
    use super::{BlockedLink as B, document_url};
    for bad in [
        // Evolution, KMail and Claws Mail attach the named file.
        "mailto:a@example.org?attach=/home/me/.ssh/id_ed25519",
        "mailto:a@example.org?subject=Hi&attachment=file:///etc/passwd",
        // Claws Mail reads an `insert` file into the message body.
        "mailto:a@example.org?insert=/home/me/notes.txt",
        // Names are matched case-insensitively (Evolution, KMail) and after percent-decoding (KMail).
        "mailto:a@example.org?ATTACH=x",
        "mailto:a@example.org?Attachment=x",
        "mailto:a@example.org?%61ttach=x",
        "mailto:a@example.org?%2561ttach=x",
        "mailto:a@example.org?%49NSERT=x",
        "mailto:a@example.org?attach%3D/etc/passwd",
        // An empty value, a repeated name, spaces or quotes before `=`.
        "mailto:a@example.org?attach=",
        "mailto:a@example.org?subject=a&attach=x&attach=y",
        "mailto:a@example.org?subject=a& attach =x",
        "mailto:a@example.org?subject=a&'attachment'='x'",
        // No recipient, no `?` at all (xdg-email), a second `?`, and a `?` that only exists once decoded.
        "mailto:?attach=x",
        "mailto:a@example.org&attach=/etc/passwd",
        "mailto:a@example.org?subject=a?attach=x",
        "mailto:a@example.org%3Fattach=x",
        // KMail reads the query from the first `?` even inside a fragment, and reads `&#38;` as `&`.
        "mailto:a@example.org#?attach=x",
        "mailto:a@example.org?subject=a#&attach=x",
        "mailto:a@example.org?subject=a&#38;attach=x",
        "mailto:a@example.org?subject=a;attach=x",
        // An encoded `&`, and a comma (xdg-email copies the subject into a comma-separated list).
        "mailto:a@example.org?subject=a%26attach=x",
        "mailto:a@example.org?subject=a,attachment='/etc/passwd'",
        // A `+`, an encoded tab or an encoded space between the name and `=`.
        "mailto:a@example.org?attach+=x",
        "mailto:a@example.org?attach%09=x",
        "mailto:a@example.org?attach%20=x",
    ] {
        assert_eq!(document_url(bad), Err(B::MailFile), "{bad}");
    }
    // KMail decodes RFC 2047 encoded words across the whole address before reading it, so
    // `=?utf-8?q?attach?=` could become `attach`. Such words never belong in a link.
    for bad in [
        "mailto:a@example.org?=?utf-8?q?attach?==/etc/passwd",
        "mailto:a@example.org?subject==?UTF-8?B?YXR0YWNo?=",
        "mailto:a@example.org?subject=%3D%3Futf-8%3Fq%3Fattach%3F%3D",
        "mailto:a@example.org?subject==??q?attach?=",
        "mailto:a@example.org?subject==?utf-8?base64?YXR0YWNo?=",
        // Backslash escapes that `echo` in xdg-email decodes, raw and percent-encoded.
        "mailto:a@example.org?\\0141ttach=x",
        "mailto:x\\x61ttachment=y",
        "mailto:a@example.org?%5C0141ttach=x",
    ] {
        assert_eq!(document_url(bad), Err(B::MailEncodedWord), "{bad}");
    }
    for ok in [
        "mailto:orders@example.org?subject=Order form&body=Please attach the invoice",
        "mailto:a@example.org?subject=Attachments%20to%20follow",
        "mailto:a@example.org?body=Questions%3F%20Attach%20your%20CV",
        "mailto:a@example.org?body=Hi%3B%20insert%20your%20name",
        "mailto:a@example.org?cc=b@example.org&bcc=c@example.org&in-reply-to=%3Cid@example.org%3E",
        "mailto:attach@example.org",
        "mailto:a@example.org?attach",
        "mailto:a@example.org?x-attach=1&reattach=2",
        "mailto:a@example.org?body=Saved in C:\\Users\\me\\Documents",
    ] {
        assert_eq!(document_url(ok).as_deref(), Ok(ok), "{ok}");
    }
}

#[test]
fn hosts_are_shown_as_the_browser_receives_them() {
    use super::{BlockedLink as B, HostDisplay, display_host, document_url};
    let plain = |a: &str| Some(HostDisplay { ascii: a.into(), international: false, mixed_scripts: false });
    assert_eq!(display_host("https://example.org/path"), plain("example.org"));
    assert_eq!(display_host("HTTPS://EXAMPLE.ORG/"), plain("example.org"), "lowercased, as the browser does");
    assert_eq!(display_host("https://example.org./"), plain("example.org."));
    assert_eq!(display_host("https://ex%61mple.org/"), plain("example.org"), "percent-decoded, as the browser does");
    assert_eq!(display_host("https://ｅｘａｍｐｌｅ。org/"), plain("example.org"), "fullwidth letters and `。` are mapped");
    assert_eq!(display_host("https://trusted.example@other.example/"), plain("other.example"));
    assert_eq!(display_host("https://other.example\\@trusted.example/"), plain("other.example"));
    assert_eq!(display_host("mailto:a@example.org"), None);
    // Numeric hosts in the form the browser connects to, so a local address can't hide.
    assert_eq!(display_host("https://[::1]:8443/"), plain("[::1]"));
    assert_eq!(display_host("https://[0:0:0:0:0:0:0:1]/"), plain("[::1]"));
    assert_eq!(display_host("http://3232235777/"), plain("192.168.1.1"));
    assert_eq!(display_host("http://0x7f.1/"), plain("127.0.0.1"));
    assert_eq!(display_host("http://0300.0250.01.01/"), plain("192.168.1.1"));

    // International hosts are shown in punycode and marked.
    let bucher = display_host("https://bücher.example/").unwrap();
    assert_eq!(bucher, HostDisplay { ascii: "xn--bcher-kva.example".into(), international: true, mixed_scripts: false });
    assert!(!display_host("https://ソニーstore.example/").unwrap().mixed_scripts, "Latin with Japanese is normal (UTS #39)");
    assert!(!display_host("https://example.ελ/").unwrap().mixed_scripts, "scripts are compared within a label, not across labels");
    // Cyrillic `а` (U+0430) in a Latin name: punycode, and flagged as mixed.
    let lookalike = display_host("https://p\u{0430}ypal.com/").unwrap();
    assert_eq!(lookalike, HostDisplay { ascii: "xn--pypal-4ve.com".into(), international: true, mixed_scripts: true });
    // The same lookalike written in punycode in the document, and upper-case.
    assert_eq!(display_host("https://XN--PYPAL-4VE.com/"), Some(lookalike));
    // A whole-script lookalike (all Cyrillic) isn't mixed, but is still shown in punycode, marked.
    let cyrillic = display_host("https://\u{0430}\u{0440}\u{0440}\u{04CF}\u{0435}.com/").unwrap();
    assert!(cyrillic.ascii.starts_with("xn--") && cyrillic.international && !cyrillic.mixed_scripts, "{cyrillic:?}");

    // A host the browser can't turn into an address is refused rather than shown.
    for bad in [
        "https://xn--a.example/",
        "https://exa%FFmple.org/",
        "https://\u{0300}a.example/",
        "https://%E2%80%AEexample.org/",
        "https://trusted.example%40other.example/",
        "https://[evil.example]/",
        "https://example.org:port/",
        "https://example.org:99999/",
    ] {
        assert_eq!(display_host(bad), None, "{bad}");
        assert_eq!(document_url(bad), Err(B::Malformed), "{bad}");
    }
}
