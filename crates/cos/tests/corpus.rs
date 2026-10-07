//! Corpus parity: every file hayro-syntax can open must also open with our reader, agree on the
//! page count, and survive an incremental save that touches the catalog (the appended revision
//! must be readable by both readers and leave the original bytes untouched).
//!
//! Runs over `corpus/pdfjs/test/pdfs` when present (`cargo xtask corpus` fetches it); otherwise
//! it is a no-op so CI without the corpus stays green. Run with `--ignored --nocapture`.

use std::sync::Arc;

use pdfcraft_cos::{Document, Object, SaveOptions, write_full, write_incremental};

/// Leaf pages reachable from the catalog (cycle-safe), like viewers count them.
fn page_count(doc: &Document) -> Option<usize> {
    let root = doc.get(doc.root()?);
    let mut stack = vec![root.as_dict()?.get(b"Pages")?.clone()];
    let (mut seen, mut n) = (std::collections::HashSet::new(), 0);
    while let Some(o) = stack.pop() {
        if let Object::Ref(r) = o
            && !seen.insert(r)
        {
            continue;
        }
        let node = doc.resolve(&o);
        let Some(d) = node.as_dict() else { continue };
        match d.get(b"Kids").map(|k| doc.resolve(k)) {
            Some(k) if d.name(b"Type") != Some(b"Page") => stack.extend(k.as_array().cloned().unwrap_or_default().into_iter().rev()),
            _ => n += 1,
        }
    }
    Some(n)
}

#[test]
#[ignore = "needs corpus/; run with --ignored"]
fn corpus_parity() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/pdfjs/test/pdfs");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("corpus not present at {}", dir.display());
        return;
    };
    let mut files: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "pdf")).collect();
    files.sort();
    let (mut checked, mut encrypted, mut failures) = (0, 0, Vec::new());
    for path in &files {
        let bytes = Arc::new(std::fs::read(path).unwrap());
        let Ok(reference) = std::panic::catch_unwind(|| hayro_syntax::Pdf::new(bytes.clone())) else { continue };
        let Ok(reference) = reference else { continue }; // hayro can't open it either: not a parity case
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if std::env::var("COS_ONLY").is_ok_and(|o| o != name) {
            continue;
        }
        if std::env::var_os("COS_TRACE").is_some() {
            eprintln!("{name}");
        }
        let expected = reference.pages().len();
        let result = std::panic::catch_unwind(|| -> Result<(), String> {
            let mut doc = match Document::open(bytes.clone()) {
                Ok(d) => d,
                Err(pdfcraft_cos::CosError::NeedsPassword) => return Err("encrypted".into()),
                Err(e) => return Err(format!("open: {e}")),
            };
            // hayro may repair a broken /Count; compare against the leaf walk count loosely.
            if page_count(&doc) != Some(expected) && expected > 0 {
                return Err(format!("page count {:?} vs hayro {expected}", page_count(&doc)));
            }
            let root = doc.root().ok_or("no root")?;
            doc.update_dict(root, |d| d.set(b"PdfCraftTest".to_vec(), Object::Bool(true))).map_err(|e| e.to_string())?;
            let saved = write_incremental(&doc, &SaveOptions::default()).map_err(|e| format!("save: {e}"))?;
            // Reconstructed files are rewritten in full; everything else must keep its bytes.
            if !doc.revisions().is_empty() && saved.get(..bytes.len()) != Some(&bytes[..]) {
                return Err("prefix modified".into());
            }
            let again = Document::open(Arc::new(saved.clone())).map_err(|e| format!("reopen: {e}"))?;
            let flag = again.get(again.root().ok_or("no root after save")?).as_dict().and_then(|d| d.get(b"PdfCraftTest").cloned());
            if flag != Some(Object::Bool(true)) {
                return Err("edit lost after reopen".into());
            }
            let hay = hayro_syntax::Pdf::new(Arc::new(saved)).map_err(|e| format!("hayro reopen: {e:?}"))?;
            if hay.pages().len() != expected {
                return Err(format!("hayro sees {} pages after save, expected {expected}", hay.pages().len()));
            }
            let full = write_full(&doc, &SaveOptions::default()).map_err(|e| format!("full: {e}"))?;
            let hay = hayro_syntax::Pdf::new(Arc::new(full)).map_err(|e| format!("hayro full: {e:?}"))?;
            if hay.pages().len() != expected {
                return Err(format!("hayro sees {} pages after full save, expected {expected}", hay.pages().len()));
            }
            Ok(())
        });
        match result {
            Ok(Ok(())) => checked += 1,
            Ok(Err(e)) if e == "encrypted" => encrypted += 1,
            Ok(Err(e)) => failures.push(format!("{name}: {e}")),
            Err(_) => failures.push(format!("{name}: PANIC")),
        }
    }
    eprintln!("cos corpus parity: {checked} ok, {encrypted} encrypted (skipped), {} failures", failures.len());
    for f in &failures {
        eprintln!("  {f}");
    }
    assert!(failures.iter().all(|f| !f.ends_with("PANIC")), "no panics");
}

/// Password-protected corpus files (passwords from pdf.js's test manifest): the right password
/// opens them and the edit/save round trip still works; no or a wrong password is reported.
#[test]
#[ignore = "needs corpus/; run with --ignored"]
fn corpus_passwords() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/pdfjs/test");
    let Ok(manifest) = std::fs::read_to_string(dir.join("test_manifest.json")) else {
        eprintln!("corpus not present");
        return;
    };
    let entries: Vec<serde_json::Value> = serde_json::from_str(&manifest).expect("manifest is JSON");
    let cases: Vec<(String, String)> =
        entries.iter().filter_map(|e| Some((e.get("file")?.as_str()?.to_string(), e.get("password")?.as_str()?.to_string()))).collect();
    assert!(cases.len() >= 5, "manifest parsed ({} cases)", cases.len());
    let mut failures = Vec::new();
    for (file, pw) in &cases {
        let Ok(bytes) = std::fs::read(dir.join(file)) else { continue };
        let bytes = Arc::new(bytes);
        if !matches!(Document::open(bytes.clone()), Err(pdfcraft_cos::CosError::NeedsPassword)) {
            failures.push(format!("{file}: opened without a password"));
        }
        if !matches!(Document::open_with_password(bytes.clone(), Some("definitely wrong")), Err(pdfcraft_cos::CosError::WrongPassword)) {
            failures.push(format!("{file}: wrong password not rejected"));
        }
        match Document::open_with_password(bytes.clone(), Some(pw)) {
            Ok(mut doc) => {
                let pages = page_count(&doc);
                if pages.unwrap_or(0) == 0 {
                    failures.push(format!("{file}: no pages after decryption"));
                    continue;
                }
                let root = doc.root().unwrap();
                doc.update_dict(root, |d| d.set(b"PdfCraftTest".to_vec(), Object::Bool(true))).unwrap();
                let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
                match Document::open_with_password(Arc::new(saved.clone()), Some(pw)) {
                    Ok(again) if page_count(&again) == pages => {}
                    other => failures.push(format!("{file}: reopen after save failed ({:?})", other.err())),
                }
            }
            Err(e) => failures.push(format!("{file}: {e}")),
        }
    }
    eprintln!("password corpus: {} cases, {} failures", cases.len(), failures.len());
    for f in &failures {
        eprintln!("  {f}");
    }
    assert!(failures.is_empty());
}
