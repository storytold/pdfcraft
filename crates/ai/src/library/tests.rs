use super::*;

fn doc(name: &str, pages: &[&str]) -> NewDocument {
    NewDocument {
        doc: DocRef::of(name.as_bytes()),
        path: format!("/papers/{name}.pdf"),
        title: name.into(),
        pages: u32::try_from(pages.len()).unwrap_or(0),
        added_at: 1_700_000_000,
        content: Content::Pages(pages.iter().enumerate().map(|(i, t)| PageText { page: u32::try_from(i).unwrap_or(0), text: (*t).into() }).collect()),
    }
}

fn corpus() -> Library {
    let mut lib = Library::new();
    lib.add(doc(
        "bm25",
        &["Okapi BM25 ranks documents by term frequency and inverse document frequency.", "The length normalisation parameter b is usually 0.75."],
    ))
    .unwrap();
    lib.add(doc("pdf", &["PDF 2.0 is specified in ISO 32000-2.", "Incremental updates append a new cross-reference section to the file.", ""]))
        .unwrap();
    lib.add(doc("jp", &["東京都は日本の首都です。"])).unwrap();
    lib
}

#[test]
fn ranks_the_matching_passage_first() {
    let lib = corpus();
    let hits = lib.search("What is the length normalisation parameter?", 3, &[]);
    assert_eq!(hits.first().map(|h| (h.doc, h.page)), Some((Sha256::of(b"bm25"), 1)), "{hits:?}");
    let hits = lib.search("cross-reference INCREMENTAL", 3, &[]);
    assert_eq!(hits.first().map(|h| (h.doc, h.page)), Some((Sha256::of(b"pdf"), 1)));
    let hits = lib.search("首都", 3, &[]);
    assert_eq!(hits.first().map(|h| h.doc), Some(Sha256::of(b"jp")));
    assert!(lib.search("zebra", 3, &[]).is_empty());
    assert!(lib.search("", 3, &[]).is_empty());
    assert!(lib.search("pdf", 0, &[]).is_empty());
    // The document being read is left out: its own text goes in the request anyway.
    assert!(lib.search("ISO 32000-2", 3, &[Sha256::of(b"pdf")]).iter().all(|h| h.doc != Sha256::of(b"pdf")));
}

#[test]
fn entries_record_status_and_pages_without_text() {
    let lib = corpus();
    let pdf = lib.entry(&Sha256::of(b"pdf")).unwrap();
    assert_eq!((pdf.status, pdf.pages, pdf.pages_without_text, pdf.passages), (Status::Indexed, 3, 1, 2));
    let mut lib = Library::new();
    let scan = NewDocument { content: Content::Pages(vec![PageText { page: 0, text: " ".into() }]), ..doc("scan", &[" "]) };
    assert_eq!(lib.add(scan), Ok(Added::New { status: Status::NoText, passages: 0 }));
    let locked = NewDocument { content: Content::NeedsPassword, ..doc("locked", &[]) };
    assert_eq!(lib.add(locked), Ok(Added::New { status: Status::NeedsPassword, passages: 0 }));
    let forbidden = NewDocument { content: Content::NotPermitted, ..doc("forbidden", &[]) };
    assert_eq!(lib.add(forbidden), Ok(Added::New { status: Status::NotPermitted, passages: 0 }));
    assert_eq!(lib.passage_count(), 0);
}

#[test]
fn the_same_bytes_are_one_document_wherever_they_are() {
    let mut lib = corpus();
    let moved = NewDocument { path: "/elsewhere/renamed.pdf".into(), ..doc("pdf", &["PDF 2.0 is specified in ISO 32000-2."]) };
    assert_eq!(lib.add(moved), Ok(Added::Known { status: Status::Indexed, passages: 2 }));
    assert_eq!(lib.entries().len(), 3);
    assert_eq!(
        lib.entry(&Sha256::of(b"pdf")).map(|e| e.paths.clone()),
        Some(vec!["/elsewhere/renamed.pdf".to_owned(), "/papers/pdf.pdf".to_owned()])
    );

    // Locked when first added, indexed once its text can be read.
    let mut lib = Library::new();
    lib.add(NewDocument { content: Content::NeedsPassword, ..doc("secret", &[]) }).unwrap();
    assert_eq!(lib.add(doc("secret", &["Now readable."])), Ok(Added::Known { status: Status::Indexed, passages: 1 }));
    assert_eq!(lib.search("readable", 1, &[]).len(), 1);
}

#[test]
fn removing_a_document_removes_its_passages() {
    let mut lib = corpus();
    assert!(lib.remove(&Sha256::of(b"bm25")));
    assert!(!lib.remove(&Sha256::of(b"bm25")));
    assert!(lib.search("BM25", 5, &[]).is_empty());
    assert_eq!(lib.search("ISO", 5, &[]).first().map(|h| h.doc), Some(Sha256::of(b"pdf")));
}

#[test]
fn results_spread_across_pages_and_documents() {
    let mut lib = Library::new();
    let pages: Vec<String> = (0..10).map(|i| format!("Kestrel kestrel kestrel number {i}.")).collect();
    let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
    lib.add(doc("birds", &refs)).unwrap();
    lib.add(doc("other", &["A kestrel is a falcon."])).unwrap();
    let hits = lib.search("kestrel", 4, &[]);
    assert_eq!(hits.len(), 4);
    assert!(hits.iter().any(|h| h.doc == Sha256::of(b"other")), "{hits:?}");
    let pages: HashSet<(Sha256, u32)> = hits.iter().map(|h| (h.doc, h.page)).collect();
    assert_eq!(pages.len(), 4);
    // With one document matching, the cap per document doesn't apply.
    assert_eq!(lib.search("number", 6, &[]).len(), 6);
}

#[test]
fn saved_form_round_trips() {
    let lib = corpus();
    let bytes = lib.to_json().unwrap();
    let (back, warnings) = Library::from_json(&bytes).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(back.entries(), lib.entries());
    assert_eq!(back.search("BM25", 3, &[]), lib.search("BM25", 3, &[]));
}

#[test]
fn damaged_files_are_errors_or_warnings_never_panics() {
    assert!(matches!(Library::from_json(b""), Err(LibraryError::Unreadable(_))));
    assert!(matches!(Library::from_json(b"{\"format\":\"pdfcraft-ai-library\""), Err(LibraryError::Unreadable(_))));
    assert!(matches!(Library::from_json(br#"{"format":"something-else","version":1,"entries":[],"passages":[]}"#), Err(LibraryError::Unreadable(_))));
    assert_eq!(
        Library::from_json(br#"{"format":"pdfcraft-ai-library","version":2,"entries":[],"passages":[]}"#).err(),
        Some(LibraryError::WrongVersion(2))
    );
    assert_eq!(Library::from_json_within(&[b' '; 65], 64, MAX_TEXT_BYTES).err(), Some(LibraryError::FileTooLarge));

    let lib = corpus();
    let mut v: serde_json::Value = serde_json::from_slice(&lib.to_json().unwrap()).unwrap();
    let first = v["entries"][0].clone();
    v["entries"].as_array_mut().unwrap().push(first);
    v["passages"].as_array_mut().unwrap().push(serde_json::json!({"doc": Sha256::of(b"nobody").to_hex(), "page": 0, "text": "orphan"}));
    let (back, warnings) = Library::from_json(v.to_string().as_bytes()).unwrap();
    assert_eq!(back.entries().len(), 3);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(back.search("orphan", 3, &[]).is_empty());
}

#[test]
fn a_full_library_refuses_without_changing() {
    let mut lib = Library::new();
    lib.text_bytes = MAX_TEXT_BYTES;
    assert_eq!(lib.add(doc("one", &["Some text."])), Err(LibraryError::Full("text")));
    assert!(lib.entries().is_empty());
    assert_eq!(lib.passage_count(), 0);
}

proptest::proptest! {
    #[test]
    fn search_is_total(pages in proptest::collection::vec("\\PC{0,400}", 0..6), query in "\\PC{0,80}", k in 0usize..80) {
        let mut lib = Library::new();
        let refs: Vec<&str> = pages.iter().map(String::as_str).collect();
        lib.add(doc("d", &refs)).unwrap();
        let hits = lib.search(&query, k, &[]);
        proptest::prop_assert!(hits.len() <= k.min(MAX_RESULTS));
        proptest::prop_assert!(hits.iter().all(|h| h.score.is_finite() && h.score > 0.0));
        let (back, _) = Library::from_json(&lib.to_json().unwrap()).unwrap();
        proptest::prop_assert_eq!(back.search(&query, k, &[]), hits);
    }

    #[test]
    fn arbitrary_files_never_panic(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..512)) {
        let _ = Library::from_json(&bytes);
    }
}

#[test]
fn a_later_read_that_reaches_further_updates_the_entry() {
    let mut lib = Library::new();
    let locked = || NewDocument { content: Content::NeedsPassword, ..doc("x", &[]) };
    let forbidden = || NewDocument { content: Content::NotPermitted, ..doc("x", &[]) };
    lib.add(locked()).unwrap();
    // Opened with the user password: copying is forbidden. That is more than "needs a password".
    assert_eq!(lib.add(forbidden()), Ok(Added::Known { status: Status::NotPermitted, passages: 0 }));
    // Adding the file again without a password doesn't take it back.
    assert_eq!(lib.add(locked()), Ok(Added::Known { status: Status::NotPermitted, passages: 0 }));
    // Opened with the owner password: the text can be read.
    assert_eq!(lib.add(doc("x", &["Readable at last."])), Ok(Added::Known { status: Status::Indexed, passages: 1 }));
    assert_eq!(lib.add(forbidden()), Ok(Added::Known { status: Status::Indexed, passages: 1 }));
    assert_eq!(lib.entries().len(), 1);
    assert_eq!(lib.passage_count(), 1);
}

#[test]
fn one_damaged_entry_is_dropped_alone() {
    let lib = corpus();
    let mut v: serde_json::Value = serde_json::from_slice(&lib.to_json().unwrap()).unwrap();
    v["entries"][0]["doc"] = serde_json::json!("not a document reference");
    v["passages"][0]["page"] = serde_json::json!("not a page");
    let (back, warnings) = Library::from_json(v.to_string().as_bytes()).unwrap();
    assert_eq!(back.entries().len(), 2, "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("couldn't be read")), "{warnings:?}");
    assert!(!back.search("ISO", 3, &[]).is_empty());
}

#[test]
fn an_entry_that_lost_passages_is_indexed_again_when_added_again() {
    let lib = corpus();
    let mut v: serde_json::Value = serde_json::from_slice(&lib.to_json().unwrap()).unwrap();
    let bm25 = Sha256::of(b"bm25").to_hex();
    v["passages"].as_array_mut().unwrap().retain(|p| p["doc"] != serde_json::json!(bm25));
    let (mut back, warnings) = Library::from_json(v.to_string().as_bytes()).unwrap();
    assert!(warnings.iter().any(|w| w.contains("lost passages")), "{warnings:?}");
    assert!(back.search("BM25", 3, &[]).is_empty());
    let again = back.add(doc(
        "bm25",
        &["Okapi BM25 ranks documents by term frequency and inverse document frequency.", "The length normalisation parameter b is usually 0.75."],
    ));
    assert_eq!(again, Ok(Added::Known { status: Status::Indexed, passages: 2 }));
    assert_eq!(back.search("BM25", 3, &[]).first().map(|h| h.doc), Some(Sha256::of(b"bm25")));
    // Already complete: adding it again changes nothing.
    assert_eq!(back.add(doc("bm25", &["Okapi BM25 ranks documents."])), Ok(Added::Known { status: Status::Indexed, passages: 2 }));
    assert_eq!(back.passage_count(), lib.passage_count());
}

#[test]
fn a_loaded_library_is_never_over_its_text_cap() {
    let mut lib = Library::new();
    lib.add(doc("two", &["Ten bytes.", "Ten again."])).unwrap();
    let (back, warnings) = Library::from_json_within(&lib.to_json().unwrap(), MAX_FILE_BYTES, 15).unwrap();
    assert_eq!(back.passage_count(), 1, "{warnings:?}");
    assert!(back.text_bytes <= 15);
}

#[test]
fn one_cjk_character_finds_the_words_it_is_in() {
    let mut lib = Library::new();
    lib.add(doc("tokyo", &["東京都の地図"])).unwrap();
    assert_eq!(lib.search("京", 3, &[]).len(), 1);
    assert_eq!(lib.search("東京", 3, &[]).len(), 1);
}
