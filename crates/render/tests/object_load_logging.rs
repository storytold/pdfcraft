//! Keep compatibility-reader diagnostics useful when one damaged file has many bad objects.
//! This integration binary owns its logger; the lock isolates captures between tests, while
//! lopdf's default Rayon workers still execute concurrently inside each read.

use std::sync::{Arc, Mutex, Once};

use log::{Level, LevelFilter, Log, Metadata, Record};
use lopdf::{Document, LoadOptions};

static TEST_LOCK: Mutex<()> = Mutex::new(());
static RECORDS: Mutex<Vec<(Level, String)>> = Mutex::new(Vec::new());
static INIT: Once = Once::new();
struct Capture;
impl Log for Capture {
    /// Capture only diagnostics emitted by the compatibility reader.
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target().starts_with("lopdf::")
    }
    /// Record matching messages and severity for the serialized test capture.
    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            RECORDS.lock().unwrap_or_else(|e| e.into_inner()).push((record.level(), record.args().to_string()));
        }
    }
    /// Keep captured records until the test explicitly clears them.
    fn flush(&self) {}
}

/// Install the shared logger once and clear records under the caller's test lock.
fn start_capture() {
    INIT.call_once(|| {
        log::set_logger(&Capture).unwrap();
        log::set_max_level(LevelFilter::Trace);
    });
    RECORDS.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Snapshot captured diagnostics, recovering the test mutex after poisoning.
fn records() -> Vec<(Level, String)> {
    RECORDS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Build a readable synthetic page with requested malformed objects and stream containers.
fn fixture(bad_objects: usize, bad_streams: usize) -> Vec<u8> {
    // The page is readable even though unrelated indirect objects are not.
    let names = if bad_objects > 0 { " /Names 6 0 R" } else { "" };
    let mut objects = vec![
        format!("<< /Type /Catalog /Pages 2 0 R{names} >>").into_bytes(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
        b"<< /Length 38 >>\nstream\nBT /F1 12 Tf 20 150 Td (Visible) Tj ET\nendstream".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    objects.extend((0..bad_objects).map(|_| b"not-a-pdf-object".to_vec()));
    // The member-header offset lies past the one-byte decoded content.
    objects.extend((0..bad_streams).map(|_| b"<< /Type /ObjStm /N 1 /First 99 /Length 1 >>\nstream\nx\nendstream".to_vec()));
    pdf_with_objects(objects)
}

/// Serialize synthetic indirect objects with a classic cross-reference table.
fn pdf_with_objects(objects: Vec<Vec<u8>>) -> Vec<u8> {
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    pdf
}

/// Misplace startxref to exercise compatibility inspection and its diagnostic budget.
fn compatibility_fixture() -> Vec<u8> {
    // A known producer mistake: startxref points just after "xref", so COS records a repair
    // and inspect uses the eager compatibility reader. The ordinary parser repairs it too.
    let pdf = String::from_utf8(fixture(1250, 0)).unwrap();
    let xref = pdf.find("xref\n").unwrap();
    let start = pdf.rfind("startxref\n").unwrap();
    format!("{}startxref\n{}\n%%EOF\n", &pdf[..start], xref + 4).into_bytes()
}

/// Require eight error details, one exact summary, and no unbudgeted messages.
fn assert_bounded(messages: &[(Level, String)], total: usize, streams: usize) {
    let details = messages.iter().filter(|(_, s)| s.starts_with("Object load error") || s.starts_with("Object stream load error")).count();
    assert_eq!(details, 8, "expected bounded details, captured {} records", messages.len());
    let summaries: Vec<_> = messages.iter().filter(|(_, s)| s.starts_with("Object loading:")).collect();
    assert_eq!(summaries.len(), 1);
    assert!(summaries[0].1.contains(&format!("{total} failed objects ({streams} object streams)")), "{summaries:?}");
    assert!(summaries[0].1.contains(&format!("{} additional messages suppressed", total - 8)), "{summaries:?}");
    assert_eq!(messages.len(), 9, "no extra per-object log sites should bypass the budget");
    assert!(messages.iter().all(|(level, _)| *level == Level::Error));
}

/// Keep a readable page while summarizing 1,250 malformed objects in nine records.
#[test]
fn malformed_objects_log_bounded_details_and_an_exact_summary() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();
    let doc = Document::load_mem(&fixture(1250, 0)).unwrap();
    assert_eq!(doc.get_pages().len(), 1);
    assert!(doc.get_object((5, 0)).is_ok(), "readable objects are retained");
    assert_bounded(&records(), 1250, 0);
}

/// Count ordinary and compressed-object failures in the same eight-detail allowance.
#[test]
fn ordinary_and_object_stream_failures_share_one_log_budget() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();
    let doc = Document::load_mem(&fixture(20, 30)).unwrap();
    assert_eq!(doc.get_pages().len(), 1);
    assert_bounded(&records(), 50, 30);
}

/// Give each compatibility inspection its own budget while retaining the xref repair warning.
#[test]
fn compatibility_inspection_gets_a_fresh_budget_on_every_open() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let pdf = Arc::new(compatibility_fixture());
    for _ in 0..2 {
        start_capture();
        let info = pdfcraft_render::inspect(pdf.clone(), None).unwrap();
        assert_eq!(info.pages.len(), 1);
        let messages = records();
        assert!(messages.iter().any(|(level, text)| *level == Level::Warn && text.starts_with("Cross-reference offset ")));
        let object_messages: Vec<_> = messages.into_iter().filter(|(_, text)| !text.starts_with("Cross-reference offset ")).collect();
        assert_bounded(&object_messages, 1250, 0);
    }
}

/// Keep healthy reads silent and preserve every diagnostic within the allowance.
#[test]
fn healthy_and_small_damaged_files_keep_their_diagnostics() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();
    Document::load_mem(&fixture(0, 0)).unwrap();
    assert!(records().is_empty());
    for count in [1, 3, 8] {
        start_capture();
        Document::load_mem(&fixture(count, 0)).unwrap();
        let messages = records();
        assert_eq!(messages.len(), count);
        assert!(messages.iter().all(|(level, s)| *level == Level::Error && s.starts_with("Object load error at offset ")));
    }
}

/// Preserve strict read errors while bounding diagnostics from concurrent workers.
#[test]
fn strict_loading_still_refuses_malformed_objects() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (objects, streams) in [(1250, 0), (0, 1250)] {
        start_capture();
        let result = Document::load_mem_with_options(&fixture(objects, streams), LoadOptions { strict: true, ..Default::default() });
        assert!(result.is_err());
        let messages = records();
        assert!(!messages.is_empty());
        assert!(messages.len() <= 9, "strict Rayon reads also share a bounded budget");
        assert!(messages.iter().all(|(level, _)| *level == Level::Error));
    }
}

/// Give concurrent documents separate detail allowances and summary totals.
#[test]
fn concurrent_document_reads_have_independent_budgets() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();
    let a = std::thread::spawn(|| Document::load_mem(&fixture(50, 0)).unwrap().get_pages().len());
    let b = std::thread::spawn(|| Document::load_mem(&fixture(50, 0)).unwrap().get_pages().len());
    assert_eq!(a.join().unwrap(), 1);
    assert_eq!(b.join().unwrap(), 1);
    let messages = records();
    assert_eq!(messages.len(), 18, "two reads each get eight details and one summary");
    assert_eq!(messages.iter().filter(|(_, s)| s.starts_with("Object loading:") && s.contains("42 additional messages suppressed")).count(), 2);
}

// Missing/broken indirect Length leaves a deferred stream; its later lookup also
// belongs to this read's budget, even though eager object collection has finished.
/// Charge missing and broken indirect lengths to the original read budget.
#[test]
fn deferred_stream_length_errors_share_the_load_budget() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for length in ["", " /Length 999999 0 R"] {
        start_capture();
        let objects = (0..1250).map(|_| format!("<<{length} >>\nstream\nx\nendstream").into_bytes()).collect();
        let doc = Document::load_mem(&pdf_with_objects(objects)).unwrap();
        assert_eq!(doc.objects.len(), 1250, "diagnostic suppression must retain the streams");
        let messages = records();
        assert_eq!(messages.len(), 9, "missing/broken Length logs bypass the budget");
        assert_eq!(messages.iter().filter(|(_, s)| s.contains("missing the Length entry")).count(), 8);
        let summary = &messages.last().unwrap().1;
        assert!(summary.contains("1250 stream length errors"), "{summary}");
        assert!(summary.contains("1242 additional messages suppressed"), "{summary}");
        assert!(messages.iter().all(|(level, _)| *level == Level::Error));
    }
}

/// Build synthetic object-stream members with bad, duplicate, or empty offsets.
fn member_stream(count: usize, kind: &str) -> Vec<u8> {
    let index: String = (0..count)
        .map(|i| {
            let offset = match kind {
                "outside" => 10000,
                "duplicate" => 0,
                "whitespace" => i,
                _ => unreachable!(),
            };
            format!("{} {offset} ", 10000 + i)
        })
        .collect();
    let body = if kind == "whitespace" { " ".repeat(count) } else { "42".to_owned() };
    let content = format!("{index}{body}");
    format!("<< /Type /ObjStm /N {count} /First {} /Length {} >>\nstream\n{content}\nendstream", index.len(), content.len()).into_bytes()
}

/// Share one warning allowance across every object-stream member in a document.
#[test]
fn object_stream_member_warnings_are_bounded_across_all_streams() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for kind in ["outside", "duplicate", "whitespace"] {
        start_capture();
        Document::load_mem(&pdf_with_objects(vec![member_stream(1000, kind), member_stream(1000, kind)])).unwrap();
        let messages = records();
        let total = if kind == "duplicate" { 1998 } else { 2000 };
        assert_eq!(messages.len(), 9, "{kind}: per-member logs bypass the budget");
        let summary = &messages.last().unwrap().1;
        assert!(summary.contains(&format!("{total} object stream warnings")), "{summary}");
        assert!(summary.contains(&format!("{} additional messages suppressed", total - 8)), "{summary}");
        assert!(messages.iter().all(|(level, _)| *level == Level::Warn));
    }
}

/// Combine diagnostic categories in one summary at the highest encountered severity.
#[test]
fn mixed_errors_and_warnings_share_one_budget_and_summary() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();
    let mut objects = vec![member_stream(1000, "outside"), b"not-an-object".to_vec()];
    objects.extend((0..20).map(|_| b"<< >>\nstream\nx\nendstream".to_vec()));
    Document::load_mem(&pdf_with_objects(objects)).unwrap();
    let messages = records();
    assert_eq!(messages.len(), 9);
    let (level, summary) = messages.last().unwrap();
    assert_eq!(*level, Level::Error, "the summary must retain the highest severity");
    for text in
        ["1 failed objects (0 object streams)", "1000 object stream warnings", "20 stream length errors", "1013 additional messages suppressed"]
    {
        assert!(summary.contains(text), "{summary}");
    }
}

/// Emit exact suppressed-message totals before returning a deterministic strict failure.
#[test]
fn strict_error_still_reports_the_suppressed_diagnostics() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // A single worker deterministically finishes the first stream's >8 warnings
    // before encountering the bad object. No reliance on Rayon racing errors.
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    start_capture();
    let pdf = pdf_with_objects(vec![member_stream(1000, "outside"), b"not-an-object".to_vec()]);
    let result = pool.install(|| Document::load_mem_with_options(&pdf, LoadOptions { strict: true, ..Default::default() }));
    assert!(result.is_err(), "strict refusal must survive log suppression");
    let messages = records();
    assert_eq!(messages.len(), 9, "strict failure must emit the summary before returning");
    let (level, summary) = messages.last().unwrap();
    assert_eq!(*level, Level::Error);
    assert!(summary.starts_with("Object loading:"), "{messages:?}");
    assert!(summary.contains("1 failed objects (0 object streams)"), "{summary}");
    assert!(summary.contains("1000 object stream warnings"), "{summary}");
    assert!(summary.contains("993 additional messages suppressed"), "{summary}");
}

/// Preserve small diagnostic batches and retain valid members despite duplicate offsets.
#[test]
fn small_stream_diagnostics_keep_their_text_level_and_objects() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for count in [1, 8] {
        start_capture();
        let doc = Document::load_mem(&pdf_with_objects(vec![member_stream(count, "outside")])).unwrap();
        assert_eq!(doc.objects.len(), 1, "the object-stream container is retained");
        let messages = records();
        assert_eq!(messages.len(), count);
        assert!(messages.iter().all(|(level, text)| *level == Level::Warn && text == "out-of-bounds offset in object stream"));
        start_capture();
        let objects = (0..count).map(|_| b"<< >>\nstream\nx\nendstream".to_vec()).collect();
        let doc = Document::load_mem(&pdf_with_objects(objects)).unwrap();
        assert_eq!(doc.objects.len(), count);
        let messages = records();
        assert_eq!(messages.len(), count);
        assert!(messages.iter().all(|(level, text)| *level == Level::Error && text.contains("missing the Length entry")));
    }
    start_capture();
    let doc = Document::load_mem(&pdf_with_objects(vec![member_stream(1000, "duplicate")])).unwrap();
    assert_eq!(doc.get_object((10000, 0)).unwrap().as_i64().unwrap(), 42, "the first valid member survives duplicate offsets");
}

/// Bound standalone object-stream parsing, including its dictionary and member warnings.
#[test]
fn direct_object_stream_parsing_also_has_a_bounded_budget() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();
    let index: String = (0..1000).map(|i| format!("{} 10000 ", i + 1)).collect();
    let mut dict = lopdf::Dictionary::new();
    dict.set("First", i64::try_from(index.len()).unwrap());
    dict.set("N", 999); // One dictionary warning and 1000 member warnings.
    let stream = lopdf::Stream::new(dict, format!("{index}42").into_bytes());
    assert!(lopdf::ObjectStream::new(&stream).unwrap().objects.is_empty());
    let messages = records();
    assert_eq!(messages.len(), 9);
    assert!(messages.last().unwrap().1.contains("1001 object stream warnings"));
    assert!(messages.last().unwrap().1.contains("993 additional messages suppressed"));
    assert!(messages.iter().all(|(level, _)| *level == Level::Warn));
}

/// A valid compressed Length/Info pair in a stream whose N header is deliberately wrong.
/// Every content-stream lookup reparses that same container before eager collection completes.
fn compressed_lengths_fixture(count: usize) -> Vec<u8> {
    let info = "<< /Title (Captured) >>";
    let index = format!("5 0 6 {} ", info.len() + 1);
    let members = format!("{index}{info} 3");
    let mut objects = vec![
        (1usize, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec()),
        (3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << >> /Contents 7 0 R >>".to_vec()),
        (4, format!("<< /Type /ObjStm /N 3 /First {} /Length {} >>\nstream\n{members}\nendstream", index.len(), members.len()).into_bytes()),
    ];
    objects.extend((0..count).map(|i| (7 + i, b"<< /Length 6 0 R >>\nstream\nq Q\nendstream".to_vec())));
    let xref_id = 7 + count;
    let mut offsets = vec![0u32; xref_id + 1];
    let mut pdf = b"%PDF-1.7\n".to_vec();
    for (id, object) in objects {
        offsets[id] = u32::try_from(pdf.len()).unwrap();
        pdf.extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        pdf.extend_from_slice(&object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    offsets[xref_id] = u32::try_from(xref).unwrap();
    let mut entries = Vec::new();
    for (id, offset) in offsets.into_iter().enumerate() {
        let (kind, first, second): (u8, u32, u16) = match id {
            0 => (0, 0, 65535),
            5 => (2, 4, 0),
            6 => (2, 4, 1),
            _ => (1, offset, 0),
        };
        entries.push(kind);
        entries.extend_from_slice(&first.to_be_bytes());
        entries.extend_from_slice(&second.to_be_bytes());
    }
    pdf.extend_from_slice(
        format!("{xref_id} 0 obj\n<< /Type /XRef /Size {} /Root 1 0 R /Info 5 0 R /W [1 4 2] /Length {} >>\nstream\n", xref_id + 1, entries.len())
            .as_bytes(),
    );
    pdf.extend_from_slice(&entries);
    pdf.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    pdf
}

/// Indirect compressed Length lookups share the eager budget, including strict reads.
#[test]
fn compressed_lengths_share_the_eager_load_budget() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for strict in [false, true] {
        for count in [1, 32] {
            start_capture();
            let doc = Document::load_mem_with_options(&compressed_lengths_fixture(count), LoadOptions { strict, ..Default::default() }).unwrap();
            assert_eq!(doc.get_pages().len(), 1);
            assert_eq!(doc.get_object((5, 0)).unwrap().as_dict().unwrap().get(b"Title").unwrap().as_str().unwrap(), b"Captured");
            for i in 0..count {
                assert_eq!(doc.get_object((u32::try_from(7 + i).unwrap(), 0)).unwrap().as_stream().unwrap().content, b"q Q");
            }
            let messages = records();
            let total = count + 1; // One header warning per lookup plus the eager container read.
            assert_eq!(messages.len(), if total <= 8 { total } else { 9 }, "strict={strict}, count={count}: {messages:?}");
            assert!(messages.iter().all(|(level, _)| *level == Level::Warn));
            if total > 8 {
                let summary = &messages.last().unwrap().1;
                assert!(summary.starts_with("Object loading:"), "{summary}");
                assert!(summary.contains(&format!("{total} object stream warnings")), "{summary}");
                assert!(summary.contains(&format!("{} additional messages suppressed", total - 8)), "{summary}");
            }
        }
    }
}

/// Lightweight metadata keeps compressed fields without creating standalone log batches.
#[test]
fn compressed_lengths_metadata_does_not_start_diagnostic_batches() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    start_capture();
    let metadata = Document::load_metadata_mem(&compressed_lengths_fixture(1)).unwrap();
    assert_eq!(metadata.title.as_deref(), Some("Captured"));
    assert_eq!(metadata.page_count, 1);
    assert!(records().is_empty(), "metadata lookups have no eager-load logging context: {:?}", records());
}

/// An encrypted fixture retains real compressed references in an incremental xref stream.
/// The writer omits existing containers, so retain them and add compressed references explicitly.
fn encrypted_container_fixture(count: usize) -> Vec<u8> {
    encrypted_stream_fixture(count, false)
}

/// Build encrypted compressed references, optionally replacing containers with malformed streams.
fn encrypted_stream_fixture(count: usize, broken: bool) -> Vec<u8> {
    let mut doc = Document::load_mem(&fixture(0, 0)).unwrap();
    doc.trailer.set("ID", lopdf::Object::Array(vec![lopdf::Object::string_literal("BBBBBBBBBBBBBBBB"); 2]));
    let mut containers = Vec::new();
    for i in 0..count {
        let member = 10000 + u32::try_from(i).unwrap();
        let index = format!("{member} 0 ");
        let mut dict = lopdf::Dictionary::new();
        // The writer omits existing ObjStm containers. Restore this same-length name after
        // serialization; names are not encrypted and object offsets remain unchanged.
        dict.set("Type", "ObjStx");
        dict.set("N", if broken { 1 } else { 2 }); // Otherwise one valid member with a wrong count.
        dict.set("First", if broken { 99 } else { i64::try_from(index.len()).unwrap() });
        let container = doc.add_object(lopdf::Stream::new(dict, format!("{index}42").into_bytes()));
        containers.push(container.0);
    }
    let version = lopdf::EncryptionVersion::V2 {
        document: &doc,
        owner_password: "owner",
        user_password: "user",
        key_length: 128,
        permissions: lopdf::Permissions::all(),
    };
    doc.encrypt(&lopdf::EncryptionState::try_from(version).unwrap()).unwrap();
    let encrypt = doc.trailer.get(b"Encrypt").unwrap().as_reference().unwrap();
    let mut pdf = Vec::new();
    doc.save_to(&mut pdf).unwrap();
    let mut restored = 0;
    for i in 0..=pdf.len() - 6 {
        if &pdf[i..i + 6] == b"ObjStx" {
            pdf[i + 5] = b'm';
            restored += 1;
        }
    }
    assert_eq!(restored, count);
    let marker = b"startxref\n";
    let start = pdf.windows(marker.len()).rposition(|w| w == marker).unwrap() + marker.len();
    let prev: usize = std::str::from_utf8(&pdf[start..]).unwrap().lines().next().unwrap().parse().unwrap();
    let xref = pdf.len();
    let xref_id = 10000 + count;
    let mut entries = Vec::new();
    for container in containers {
        entries.push(2);
        entries.extend_from_slice(&container.to_be_bytes());
        entries.extend_from_slice(&0u16.to_be_bytes());
    }
    entries.push(1);
    entries.extend_from_slice(&u32::try_from(xref).unwrap().to_be_bytes());
    entries.extend_from_slice(&0u16.to_be_bytes());
    let id = "42".repeat(16);
    pdf.extend_from_slice(format!("\n{xref_id} 0 obj\n<< /Type /XRef /Size {} /Root 1 0 R /Encrypt {} {} R /ID [<{id}> <{id}>] /Prev {prev} /Index [10000 {}] /W [1 4 2] /Length {} >>\nstream\n", xref_id + 1, encrypt.0, encrypt.1, count + 1, entries.len()).as_bytes());
    pdf.extend_from_slice(&entries);
    pdf.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    pdf
}

/// All encrypted containers share one budget, retaining their decrypted members and page.
#[test]
fn encrypted_object_streams_share_the_load_budget() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for strict in [false, true] {
        for count in [1, 32] {
            let pdf = encrypted_container_fixture(count);
            start_capture();
            let doc = Document::load_mem_with_options(&pdf, LoadOptions { strict, ..LoadOptions::with_password("user") }).unwrap();
            assert_eq!(doc.get_pages().len(), 1);
            for i in 0..count {
                let member = 10000 + u32::try_from(i).unwrap();
                let value = doc.get_object((member, 0));
                assert!(
                    value.is_ok(),
                    "missing member {member}: xref={:?}, container={:?}, logs={:?}",
                    doc.reference_table.get(member),
                    doc.get_object((6 + u32::try_from(i).unwrap(), 0)),
                    records()
                );
                assert_eq!(value.unwrap().as_i64().unwrap(), 42);
            }
            let messages = records();
            assert_eq!(messages.len(), if count <= 8 { count } else { 9 }, "strict={strict}, count={count}: {messages:?}");
            assert!(messages.iter().all(|(level, _)| *level == Level::Warn));
            if count > 8 {
                let summary = &messages.last().unwrap().1;
                assert!(summary.starts_with("Object loading:"), "{summary}");
                assert!(summary.contains(&format!("{count} object stream warnings")), "{summary}");
                assert!(summary.contains(&format!("{} additional messages suppressed", count - 8)), "{summary}");
            }
        }
    }
}

/// Count malformed encrypted containers without changing the diagnostic limit.
#[test]
fn encrypted_container_errors_are_counted_and_bounded() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for strict in [false, true] {
        for count in [1, 32] {
            let pdf = encrypted_stream_fixture(count, true);
            start_capture();
            let doc = Document::load_mem_with_options(&pdf, LoadOptions { strict, ..LoadOptions::with_password("user") }).unwrap();
            assert_eq!(doc.get_pages().len(), 1);
            assert!(doc.get_object((10000, 0)).is_err(), "the bad container was not parsed");
            let messages = records();
            if count > 8 {
                assert_bounded(&messages, count, count);
            } else {
                assert_eq!(messages.len(), count, "failed encrypted containers must be recorded: {messages:?}");
                assert!(messages.iter().all(|(level, text)| *level == Level::Error && text.starts_with("Object stream load error")));
            }
        }
    }
}

/// Build cyclic or excessive indirect-Length chains that emit lookup warnings.
fn lookup_warning_fixture(count: usize, cycle: bool) -> Vec<u8> {
    let mut objects = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_vec(),
        b"<< /Length 3 >>\nstream\nq Q\nendstream".to_vec(),
        b"42".to_vec(),
    ];
    let chain = if cycle { 1 } else { 18 };
    for i in 0..chain {
        let length = if cycle {
            "6 0 R".to_string()
        } else if i + 1 == chain {
            "1".to_string()
        } else {
            format!("{} 0 R", 7 + i)
        };
        objects.push(format!("<< /Length {length} >>\nstream\nx\nendstream").into_bytes());
    }
    objects.extend((0..count).map(|_| b"<< /Length 6 0 R >>\nstream\nx\nendstream".to_vec()));
    String::from_utf8(pdf_with_objects(objects)).unwrap().replace("/Root 1 0 R", "/Root 1 0 R /Info 6 0 R").into_bytes()
}

/// Charge cyclic and excessive-depth lookup warnings to the enclosing read budget.
#[test]
fn lookup_warnings_share_the_load_budget() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for cycle in [false, true] {
        for strict in [false, true] {
            for count in [1, 32] {
                let pdf = lookup_warning_fixture(count, cycle);
                start_capture();
                let doc = Document::load_mem_with_options(&pdf, LoadOptions { strict, ..Default::default() }).unwrap();
                assert_eq!(doc.get_pages().len(), 1);
                assert_eq!(doc.get_object((5, 0)).unwrap().as_i64().unwrap(), 42);
                let messages = records();
                let warnings = count + 1;
                let lengths = count + if cycle { 1 } else { 17 };
                let total = warnings + lengths;
                assert_eq!(messages.len(), if total <= 8 { total } else { 9 }, "cycle={cycle}, strict={strict}, count={count}: {messages:?}");
                if total > 8 {
                    let (level, summary) = messages.last().unwrap();
                    assert_eq!(*level, Level::Error);
                    assert!(summary.contains("0 failed objects (0 object streams)"), "{summary}");
                    assert!(summary.contains(&format!("{warnings} object stream warnings")), "{summary}");
                    assert!(summary.contains(&format!("{lengths} stream length errors")), "{summary}");
                    assert!(summary.contains(&format!("{} additional messages suppressed", total - 8)), "{summary}");
                }
            }
        }
    }
}

/// Read lightweight metadata without starting object-load diagnostic batches.
#[test]
fn metadata_lookup_warnings_stay_silent() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for cycle in [false, true] {
        let pdf = lookup_warning_fixture(0, cycle);
        start_capture();
        let info = Document::load_metadata_mem(&pdf).unwrap();
        assert_eq!(info.page_count, 1);
        assert!(records().is_empty(), "metadata lookup diagnostics escaped the silent context: {:?}", records());
    }
}
