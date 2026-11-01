use super::*;
use pdfcraft_platform::staging::STAGING_ATTEMPTS;

/// `n` pages, page i showing "Page i".
fn numbered_pages(n: usize) -> Vec<u8> {
    let mut objs: Vec<String> = vec!["<< /Type /Catalog /Pages 2 0 R >>".into()];
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objs.push(format!("<< /Type /Pages /Kids [{}] /Count {n} /MediaBox [0 0 200 300] >>", kids.join(" ")));
    objs.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
    for i in 0..n {
        objs.push(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i));
        let body = format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1);
        objs.push(format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()));
    }
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

fn json(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args).map(|mut c| c.remove(0)) {
        Ok(Content::Json(v)) => v,
        other => panic!("{tool} {args}: {other:?}"),
    }
}

fn search_document(a: &mut Automation, pages: usize) -> DocId {
    a.session.open("synthetic.pdf", None, Arc::new(numbered_pages(pages)), None).unwrap()
}

#[test]
fn search_cache_stays_bounded_reextracts_and_follows_edits_and_close() {
    let mut a = Automation::new();
    a.texts = TextCache::new(16 * 1024);
    let first = search_document(&mut a, 200);
    let second = search_document(&mut a, 200);
    for id in [first, second] {
        let result = json(&mut a, "text_find", json!({ "doc": id.0, "query": "absent" }));
        assert_eq!(result["complete"], true);
        assert!(a.texts.bytes() <= 16 * 1024);
        assert!(a.texts.len() < 200, "an exhaustive search must not retain every page");
        let found = json(&mut a, "text_find", json!({ "doc": id.0, "query": "Page", "limit": 1000 }));
        assert_eq!(found["count"], 200, "eviction inside a batch must not discard its matches");
        eprintln!("200 pages searched: {} resident pages, {} estimated bytes (budget 16384)", a.texts.len(), a.texts.bytes());
    }
    assert!(a.texts.get(first, 0).is_none());
    let text = a.page_texts(first, &[0]).unwrap().remove(0);
    assert_eq!(text.plain_text(), "Page 1");
    let again = a.page_texts(first, &[0]).unwrap().remove(0);
    assert!(Arc::ptr_eq(&text, &again), "cache hits reuse glyph storage");
    assert!(a.texts.bytes() <= 16 * 1024);
    json(&mut a, "page_delete", json!({ "doc": first.0, "pages": [1] }));
    let edited = a.page_texts(first, &[0]).unwrap().remove(0);
    assert_eq!(edited.plain_text(), "Page 2");
    assert!(!Arc::ptr_eq(&text, &edited));
    json(&mut a, "doc_close", json!({ "doc": first.0, "discard_changes": true }));
    json(&mut a, "doc_close", json!({ "doc": second.0 }));
    assert_eq!((a.texts.bytes(), a.texts.len()), (0, 0));
}

#[test]
fn strict_text_collection_refuses_partial_results_but_keeps_successful_pages() {
    let mut a = Automation::new();
    let id = search_document(&mut a, 3);
    let error = a.page_texts(id, &[0, 3, 1]).unwrap_err();
    assert!(error.to_string().contains("page 4: page is out of range"));
    assert!(a.texts.get(id, 0).is_some());
    assert!(a.texts.get(id, 1).is_some(), "successful extraction after the failed entry is retained");
    let results = a.page_text_results(id, &[2], &mut |_, _, _| vec![]).unwrap();
    assert!(results[0].1.is_err(), "a missing worker result must not become an empty page");
}

#[test]
fn search_keeps_matches_around_failed_pages_and_continuations() {
    let mut a = Automation::new();
    let id = search_document(&mut a, 16);
    let mut extract = |bytes: &Arc<Vec<u8>>, password, pages: &[usize]| {
        extract_parallel(bytes, password, pages)
            .into_iter()
            .map(|(p, result)| (p, if [2, 6].contains(&p) { Err(format!("synthetic page {} failure", p + 1)) } else { result }))
            .collect()
    };
    let all = a.text_find_with(&Args(&json!({ "doc": id.0, "query": "Page" })), &mut extract).unwrap();
    assert_eq!(all["count"], 14);
    assert_eq!(all["complete"], false);
    assert_eq!(all["warning_count"], 2);
    assert_eq!(all["warnings"][0]["page"], 3);
    assert_eq!(all["warnings"][1]["page"], 7);
    assert!(all.get("next").is_none());
    assert_eq!(all["matches"][13]["page"], 16);
    assert!(a.texts.get(id, 7).is_some(), "success after a failure in the same batch is cached");
    let mut args = json!({ "doc": id.0, "query": "Page", "limit": 4 });
    let mut matches = Vec::new();
    let mut warned = Vec::new();
    loop {
        let part = a.text_find_with(&Args(&args), &mut extract).unwrap();
        matches.extend(part["matches"].as_array().unwrap().iter().cloned());
        warned.extend(part["warnings"].as_array().unwrap().iter().map(|w| w["page"].clone()));
        if !part["warnings"].as_array().unwrap().is_empty() {
            assert_eq!(part["complete"], false);
        }
        let Some(next) = part.get("next") else { break };
        args["start_page"] = next["start_page"].clone();
        args["skip"] = next["skip"].clone();
    }
    assert_eq!(matches, *all["matches"].as_array().unwrap(), "no readable match is lost or repeated");
    assert_eq!(warned, vec![json!(3), json!(7)]);
}

#[test]
fn search_bounds_warning_volume_and_does_not_reach_distant_failures() {
    let mut a = Automation::new();
    let id = search_document(&mut a, 200);
    let first = a
        .text_find_with(&Args(&json!({ "doc": id.0, "query": "Page", "limit": 1 })), &mut |bytes, password, pages| {
            assert!(pages.iter().all(|p| *p < FIND_FIRST_BATCH), "a bounded search must not reach the bad page at the end");
            extract_parallel(bytes, password, pages)
        })
        .unwrap();
    assert_eq!(first["count"], 1);
    assert_eq!(first["warning_count"], 0);
    assert_eq!(first["next"], json!({ "start_page": 2, "skip": 0 }));
    a.texts.remove(&id);
    let failures = a
        .text_find_with(&Args(&json!({ "doc": id.0, "query": "Page" })), &mut |_, _, pages| {
            pages.iter().map(|p| (*p, Err("界".repeat(2000)))).collect()
        })
        .unwrap();
    assert_eq!(failures["count"], 0);
    assert_eq!(failures["complete"], false);
    assert!(failures.get("next").is_none());
    assert_eq!(failures["warning_count"], 200);
    assert_eq!(failures["warnings_truncated"], true);
    let warnings = failures["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), FIND_MAX_WARNINGS);
    assert!(warnings.iter().all(|w| w["error"].as_str().unwrap().len() <= FIND_MAX_WARNING_BYTES && w["error_truncated"] == true));
}

#[test]
fn a_bounded_text_find_reads_only_the_pages_it_needs_and_says_it_stopped() {
    let dir = std::env::temp_dir().join(format!("pdfcraft-automation-find-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("long.pdf"), numbered_pages(200)).unwrap();
    let mut a = Automation::new().with_root(&dir).unwrap();
    let doc = json(&mut a, "doc_open", json!({ "path": "long.pdf" }))["doc"].clone();
    let extracted = |a: &Automation| a.texts.len();

    // #971: the first match is on page 1 and the next on page 10, so the rest of the 200 pages
    // aren't extracted or cached.
    let first = json(&mut a, "text_find", json!({ "doc": doc, "query": "Page 1", "limit": 1 }));
    assert_eq!(first["count"], 1);
    assert_eq!(first["matches"][0]["page"], 1);
    assert_eq!(first["complete"], false, "{first}");
    assert_eq!(first["next"], json!({ "start_page": 10, "skip": 0 }));
    assert!(extracted(&a) <= 3 * FIND_FIRST_BATCH, "{} pages extracted for one match", extracted(&a));

    // Following `next` returns every match exactly once, in order, and then says it's complete.
    // "1" appears twice on pages like 11 and 101, so continuing has to skip within a page.
    let all = json(&mut a, "text_find", json!({ "doc": doc, "query": "1", "limit": 10_000 }));
    assert_eq!(all["complete"], true);
    assert!(all.get("next").is_none());
    let mut pieced = Vec::new();
    let mut args = json!({ "doc": doc, "query": "1", "limit": 3 });
    loop {
        let found = json(&mut a, "text_find", args.clone());
        pieced.extend(found["matches"].as_array().unwrap().iter().map(|m| (m["page"].clone(), m["rects"].clone())));
        if found["complete"] == true {
            break;
        }
        args["start_page"] = found["next"]["start_page"].clone();
        args["skip"] = found["next"]["skip"].clone();
    }
    let expected: Vec<_> = all["matches"].as_array().unwrap().iter().map(|m| (m["page"].clone(), m["rects"].clone())).collect();
    assert_eq!(all["count"], expected.len());
    assert_eq!(pieced, expected);

    // A search that runs to the end below its limit is complete; a start page past the end is refused.
    let few = json(&mut a, "text_find", json!({ "doc": doc, "query": "Page 200", "start_page": 150 }));
    assert_eq!((few["count"].clone(), few["complete"].clone()), (json!(1), json!(true)));
    assert!(a.call("text_find", &json!({ "doc": doc, "query": "x", "start_page": 201 })).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dot_dot_resolves_by_name_and_never_climbs_above_the_start() {
    let base = std::env::temp_dir();
    let l = |rel: &str| lexical(&base.join(rel));
    assert_eq!(l("a/./b/../c"), Some(base.join("a").join("c")));
    assert_eq!(l("missing/../../x"), lexical(&base.join("..").join("x")));
    assert_eq!(l("a/.."), Some(lexical(&base).unwrap()));
    let deep = "../".repeat(base.components().count() + 1);
    assert_eq!(lexical(&base.join(&deep)), None);
    assert_eq!(lexical(Path::new("..")), None);
}

#[test]
fn names_from_documents_become_one_plain_file_name() {
    let dir = std::env::temp_dir().join("out");
    for name in ["../../x.png", "/abs/x.png", r"..\..\x.png", "C:x.png", r"\\host\share\x.png", "a:stream", "..", ".", "", "tab\there", "ok.png"] {
        let p = child(&dir, name);
        assert_eq!(p.parent(), Some(dir.as_path()), "{name:?} -> {}", p.display());
        assert_eq!(p.components().count(), dir.components().count() + 1, "{name:?} -> {}", p.display());
    }
    assert_eq!(child(&dir, "ok.png"), dir.join("ok.png"));
    assert_eq!(child(&dir, "../x.png"), dir.join(".._x.png"));
}

#[cfg(windows)]
#[test]
fn other_shares_and_device_paths_are_refused_by_name() {
    let local = Path::new(r"\\?\C:\work\root");
    for p in [
        r"\\host\share\x.pdf",
        "//host/share/x.pdf",
        r"\/host/share/x.pdf",
        r"\\?\UNC\host\share\x.pdf",
        r"\\.\pipe\x",
        r"\\.\C:\work\root\x.pdf",
        r"\\?\GLOBALROOT\Device\x",
    ] {
        assert!(foreign_share(Path::new(p), local), "{p}");
    }
    for p in [r"C:\work\root\x.pdf", r"c:\elsewhere\x.pdf", r"\\?\C:\work\root\x.pdf", r"D:\x.pdf", r"C:x.pdf"] {
        assert!(!foreign_share(Path::new(p), local), "{p}");
    }
    // A root on a share accepts that share, however it is spelled, and nothing else.
    let shared = Path::new(r"\\?\UNC\Server\Docs\root");
    assert!(!foreign_share(Path::new(r"\\server\docs\root\x.pdf"), shared));
    assert!(!foreign_share(Path::new(r"\\?\UNC\SERVER\DOCS\x.pdf"), shared));
    assert!(foreign_share(Path::new(r"\\server\other\x.pdf"), shared));
    assert!(foreign_share(Path::new(r"\\attacker\docs\x.pdf"), shared));
}

/// A fresh, empty folder for one staging test.
fn staging_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pdfcraft-staging-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn staged(dir: &Path, suffix: u64) -> PathBuf {
    dir.join(format!(".out.pdf.{suffix:016x}.pdfcraft-tmp"))
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
fn staging_never_writes_through_a_file_planted_at_its_name() {
    let dir = staging_dir("planted");
    // A file elsewhere that a planted link points at.
    let outside = dir.join("outside.txt");
    std::fs::write(&outside, "PRECIOUS").unwrap();
    let target = dir.join("out.pdf");
    // A hard link needs no privileges on any system, and writing to it writes to `outside`.
    std::fs::hard_link(&outside, staged(&dir, 1)).unwrap();
    std::fs::write(staged(&dir, 2), "PLANTED").unwrap();
    write_atomic_with(&target, b"NEW", [1, 2, 3]).unwrap();
    assert_eq!(read(&target), "NEW");
    assert_eq!(read(&outside), "PRECIOUS");
    assert_eq!(read(&staged(&dir, 1)), "PRECIOUS");
    assert_eq!(read(&staged(&dir, 2)), "PLANTED");
    assert!(!staged(&dir, 3).exists(), "the staging file was renamed into place");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn staging_never_writes_through_a_symbolic_link_planted_at_its_name() {
    let dir = staging_dir("symlink");
    let outside = dir.join("outside.txt");
    std::fs::write(&outside, "PRECIOUS").unwrap();
    let target = dir.join("out.pdf");
    if let Err(e) = file_symlink(&outside, &staged(&dir, 1)) {
        eprintln!("symbolic links not checked: {e}");
        return;
    }
    write_atomic_with(&target, b"NEW", [1, 2]).unwrap();
    assert_eq!(read(&target), "NEW");
    assert_eq!(read(&outside), "PRECIOUS");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn staging_never_creates_a_file_through_a_dangling_link_at_its_name() {
    let dir = staging_dir("dangling");
    let unborn = dir.join("created-through-a-link.txt");
    let target = dir.join("out.pdf");
    if let Err(e) = file_symlink(&unborn, &staged(&dir, 1)) {
        eprintln!("symbolic links not checked: {e}");
        return;
    }
    write_atomic_with(&target, b"NEW", [1, 2]).unwrap();
    assert_eq!(read(&target), "NEW");
    assert!(!unborn.exists(), "nothing was created through the dangling link");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn staging_gives_up_rather_than_reuse_a_taken_name() {
    let dir = staging_dir("taken");
    let target = dir.join("out.pdf");
    std::fs::write(&target, "OLD").unwrap();
    for s in 1..=STAGING_ATTEMPTS as u64 {
        std::fs::write(staged(&dir, s), "PLANTED").unwrap();
    }
    assert!(write_atomic_with(&target, b"NEW", 1..).is_err());
    assert_eq!(read(&target), "OLD");
    for s in 1..=STAGING_ATTEMPTS as u64 {
        assert_eq!(read(&staged(&dir, s)), "PLANTED");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_staging_sync_keeps_the_destination_and_cleans_up() {
    for exists in [false, true] {
        let dir = staging_dir(if exists { "sync-existing" } else { "sync-new" });
        let target = dir.join("out.pdf");
        if exists {
            std::fs::write(&target, "OLD").unwrap();
        }
        let result = write_atomic_synced(&target, b"NEW", [1], |_| {
            assert_eq!(std::fs::read(staged(&dir, 1)).unwrap(), b"NEW", "sync follows writing");
            if exists {
                assert_eq!(read(&target), "OLD", "sync precedes publication");
            }
            Err(std::io::Error::other("injected storage sync failure"))
        });
        assert!(result.is_err(), "sync failure must refuse publication");
        assert!(result.unwrap_err().to_string().contains("injected storage sync failure"));
        if exists {
            assert_eq!(read(&target), "OLD");
            assert_eq!(listing(&dir), ["out.pdf"]);
        } else {
            assert!(!target.exists());
            assert!(listing(&dir).is_empty());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn invalid_save_bytes_never_reach_publication_or_mark_saved() {
    let dir = staging_dir("invalid-save-bytes");
    let target = dir.join("out.pdf");
    let mut a = Automation::new().with_root(&dir).unwrap();
    let id = search_document(&mut a, 2);
    json(&mut a, "doc_save", json!({"doc": id.0, "path": "out.pdf"}));
    let old = std::fs::read(&target).unwrap();
    json(&mut a, "page_rotate", json!({"doc": id.0, "pages": [1], "degrees": 90}));
    let before = summary(a.session.get(id).unwrap());
    // Exercise both parse failure and a valid PDF that loses a page. This does not
    // depend on a malformed input remaining unrepaired by future reader improvements.
    for bytes in [b"not a PDF".to_vec(), numbered_pages(1)] {
        let mut published = false;
        let result = a.publish_save(id, &target, Arc::new(bytes), false, |_, _| {
            published = true;
            Ok(())
        });
        assert!(result.is_err(), "invalid output must be refused");
        assert!(result.unwrap_err().to_string().contains("could not be reopened"));
        assert!(!published, "validation must precede the writer");
        assert_eq!(summary(a.session.get(id).unwrap()), before, "failed validation must preserve document state");
        assert_eq!(std::fs::read(&target).unwrap(), old);
        assert_eq!(listing(&dir), ["out.pdf"]);
    }
    json(&mut a, "doc_save", json!({"doc": id.0}));
    assert!(!a.session.get(id).unwrap().dirty);
    assert_ne!(std::fs::read(&target).unwrap(), old);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_failed_save_sync_keeps_the_document_dirty_and_can_be_retried() {
    let dir = staging_dir("sync-dirty");
    let target = dir.join("out.pdf");
    let mut a = Automation::new().with_root(&dir).unwrap();
    let id = search_document(&mut a, 2);
    json(&mut a, "doc_save", json!({"doc": id.0, "path": "out.pdf"}));
    let old = std::fs::read(&target).unwrap();
    json(&mut a, "page_rotate", json!({"doc": id.0, "pages": [1], "degrees": 90}));
    let args = json!({"doc": id.0});
    let before = summary(a.doc(&Args(&args)).unwrap());
    let result = a.doc_save_with(&Args(&args), |path, bytes| {
        write_atomic_synced(path, bytes, [1], |_| Err(std::io::Error::other("injected storage sync failure")))
    });
    assert!(result.is_err());
    let doc = a.doc(&Args(&args)).unwrap();
    assert!(doc.dirty);
    assert_eq!(summary(doc), before, "failed save must not reset history or saved state");
    assert_eq!(std::fs::read(&target).unwrap(), old);
    assert_eq!(listing(&dir), ["out.pdf"]);
    json(&mut a, "doc_save", args.clone());
    assert!(!a.doc(&Args(&args)).unwrap().dirty);
    assert_ne!(std::fs::read(&target).unwrap(), old);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn successful_staging_sync_precedes_publication() {
    let dir = staging_dir("sync-success");
    let target = dir.join("out.pdf");
    std::fs::write(&target, "OLD").unwrap();
    let mut called = false;
    write_atomic_synced(&target, b"NEW", [1], |file| {
        called = true;
        assert_eq!(read(&target), "OLD");
        assert_eq!(std::fs::read(staged(&dir, 1)).unwrap(), b"NEW");
        file.sync_all()
    })
    .unwrap();
    assert!(called);
    assert_eq!(read(&target), "NEW");
    assert_eq!(listing(&dir), ["out.pdf"]);
    std::fs::remove_dir_all(dir).unwrap();
}

/// The names in a folder: what a test can see was left behind.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

#[test]
fn staging_names_differ_between_saves_and_fit_long_names() {
    let suffixes: std::collections::HashSet<u64> = staging_suffixes().take(64).collect();
    assert_eq!(suffixes.len(), 64);
    assert_ne!(staging_suffixes().next(), staging_suffixes().next(), "each save draws new names");
    // 60 four-byte characters: a 244-byte name, within every system's limit. Its staging name
    // must be too (on Linux the whole name in it would be 275 bytes).
    let dir = staging_dir("long");
    let name = format!("{}.pdf", "\u{1F600}".repeat(60));
    write_atomic(&dir.join(&name), b"NEW").unwrap();
    assert_eq!(read(&dir.join(&name)), "NEW");
    assert_eq!(listing(&dir), [name], "no staging file is left behind");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_folder_at_the_staging_name_is_left_alone() {
    let dir = staging_dir("folder");
    std::fs::create_dir(staged(&dir, 1)).unwrap();
    std::fs::write(staged(&dir, 1).join("inside.txt"), "PLANTED").unwrap();
    let target = dir.join("out.pdf");
    write_atomic_with(&target, b"NEW", [1, 2]).unwrap();
    assert_eq!(read(&target), "NEW");
    assert_eq!(read(&staged(&dir, 1).join("inside.txt")), "PLANTED");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Windows refuses to replace a read-only file, so the rename fails: the staging file must not
/// be left behind.
#[cfg(windows)]
#[test]
fn a_failed_rename_removes_the_staging_file() {
    let dir = staging_dir("readonly");
    let target = dir.join("out.pdf");
    std::fs::write(&target, "OLD").unwrap();
    let mut perms = std::fs::metadata(&target).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&target, perms.clone()).unwrap();
    let e = write_atomic_with(&target, b"NEW", [7]).unwrap_err();
    assert!(e.to_string().contains(&target.display().to_string()), "the rename failed, not the staging: {e}");
    assert_eq!(read(&target), "OLD");
    assert_eq!(listing(&dir), ["out.pdf"], "the staging file was removed");
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&target, perms).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A recovered document, or a new one, rotated and then undone still has never been saved:
/// `doc_close` refuses it unless told to discard it (#2346 review).
#[test]
fn closing_a_recovered_or_new_document_after_undo_is_refused() {
    let mut a = Automation::new();
    let recovered = a.session.open("Report.pdf", None, Arc::new(numbered_pages(2)), None).unwrap();
    a.session.mark_recovered(recovered, Some("Report.pdf".into()));
    let new = a.session.open_new("Combined.pdf", Arc::new(numbered_pages(2))).unwrap();
    for id in [recovered, new] {
        json(&mut a, "page_rotate", json!({"doc": id.0, "pages": [1], "degrees": 90}));
        json(&mut a, "edit_undo", json!({"doc": id.0}));
        let refused = a.call("doc_close", &json!({"doc": id.0}));
        assert!(matches!(&refused, Err(e) if e.to_string().contains("unsaved changes")), "{id:?}: {refused:?}");
        assert!(a.session.get(id).is_some(), "still open");
        json(&mut a, "doc_close", json!({"doc": id.0, "discard_changes": true}));
        assert!(a.session.get(id).is_none());
    }
}

#[test]
fn tool_file_reads_refuse_inputs_exceeding_512_mib() {
    let dir = staging_dir("capped-read");
    let huge = dir.join("huge.bin");
    let file = std::fs::File::create(&huge).unwrap();
    file.set_len(MAX_INPUT + 1).unwrap();
    drop(file);
    let huge_str = huge.to_string_lossy().into_owned();

    let mut a = Automation::new();
    let id = a.session.open("a.pdf", None, Arc::new(numbered_pages(2)), None).unwrap().0;
    for (tool, args) in [
        ("doc_open", json!({ "path": huge_str })),
        ("page_insert_file", json!({ "doc": id, "at": 1, "path": huge_str })),
        ("page_replace", json!({ "doc": id, "pages": [1], "path": huge_str })),
        ("page_add_image", json!({ "doc": id, "page": 1, "path": huge_str })),
        ("doc_import_data", json!({ "doc": id, "path": huge_str })),
        ("doc_create", json!({ "from": "text", "path": huge_str })),
        ("doc_create", json!({ "from": "images", "paths": [huge_str] })),
        ("doc_combine", json!({ "paths": [huge_str, huge_str] })),
        ("form_merge_data", json!({ "paths": [huge_str], "path": dir.join("out.csv").to_string_lossy() })),
        ("comment_add", json!({ "doc": id, "page": 1, "type": "attachment", "at": [72.0, 72.0], "path": huge_str })),
        ("stamp_custom", json!({ "doc": id, "page": 1, "at": [72.0, 72.0], "path": huge_str })),
        ("sign_trust", json!({ "paths": [huge_str] })),
        ("attachment_add", json!({ "doc": id, "path": huge_str })),
    ] {
        let err = a.call(tool, &args).unwrap_err().to_string();
        assert!(err.contains("input exceeds 512 MiB"), "{tool}: {err}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
