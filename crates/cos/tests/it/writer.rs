//! Full saves with object streams and cross-reference streams (M1.9), read back by our parser
//! and by hayro, and incremental saves stacked on top of them.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, SaveOptions, Stream, write_full, write_incremental};
use pdfcraft_testkit::Pdf;

/// Four pages, a shared font, document info with a non-ASCII title.
fn fixture() -> Vec<u8> {
    let kids: Vec<String> = (0..4).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    let mut pdf = Pdf::new()
        .version("1.4")
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj(format!("<< /Type /Pages /Kids [{}] /Count 4 /MediaBox [0 0 200 300] >>", kids.join(" ")))
        .obj("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    for i in 0..4 {
        pdf = pdf
            .obj(format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 5 + 2 * i))
            .stream("", format!("BT /F1 24 Tf 20 150 Td (Page {}) Tj ET", i + 1));
    }
    let info = pdf.next_obj();
    pdf.obj("<< /Title (Caf\\351 \\(draft\\)) >>").trailer_entry(&format!("/Info {info} 0 R")).build()
}

fn title(doc: &Document) -> Vec<u8> {
    let info = doc.resolve(doc.trailer().get(b"Info").unwrap());
    info.as_dict().unwrap().get(b"Title").unwrap().as_string().unwrap().bytes.clone()
}

fn hayro_pages(bytes: &[u8]) -> usize {
    hayro_syntax::Pdf::new(Arc::new(bytes.to_vec())).expect("hayro opens").pages().len()
}

#[test]
fn full_save_packs_objects_into_object_streams() {
    let doc = Document::open(Arc::new(fixture())).unwrap();
    let packed = write_full(&doc, &SaveOptions::default()).unwrap();
    let text = String::from_utf8_lossy(&packed);
    assert!(packed.starts_with(b"%PDF-1.5"), "object streams need PDF 1.5");
    assert!(text.contains("/ObjStm") && text.contains("/XRef"));
    assert!(!text.contains("\nxref\n"), "no classic table");
    assert!(!text.contains("/Type /Catalog") && !text.contains("/Type/Catalog"), "the catalog is compressed");

    let back = Document::open(Arc::new(packed.clone())).unwrap();
    assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
    assert_eq!(title(&back), b"Caf\xe9 (draft)");
    assert_eq!(hayro_pages(&packed), 4);

    let classic = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    assert!(classic.starts_with(b"%PDF-1.4"));
    assert!(String::from_utf8_lossy(&classic).contains("\nxref\n"));
    assert_eq!(hayro_pages(&classic), 4);
}

#[test]
fn incremental_saves_stack_on_an_xref_stream_file() {
    let doc = Document::open(Arc::new(fixture())).unwrap();
    let packed = Arc::new(write_full(&doc, &SaveOptions::default()).unwrap());
    let mut doc = Document::open(packed.clone()).unwrap();
    // Edit an object that lives in an object stream.
    let info = doc.trailer().reference(b"Info").unwrap();
    doc.update_dict(info, |d: &mut Dict| d.set(b"Title".to_vec(), Object::String(pdfcraft_cos::PdfString { bytes: b"Second".to_vec(), hex: false })))
        .unwrap();
    let added = doc.add(Object::Int(42));
    let updated = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert!(updated.starts_with(&packed), "an incremental save only appends");
    let tail = String::from_utf8_lossy(&updated[packed.len()..]);
    assert!(tail.contains("/XRef") && tail.contains("/Prev"), "the update uses an xref stream too");

    let back = Document::open(Arc::new(updated.clone())).unwrap();
    assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
    assert_eq!(title(&back), b"Second");
    assert_eq!(*back.get(ObjRef::new(added.num, 0)), Object::Int(42));
    assert_eq!(back.revisions().len(), 2);
    assert_eq!(hayro_pages(&updated), 4);
}

/// The revision is joined to the original once, at its exact size. A revision of more than a few
/// kilobytes used to outgrow the buffer: the whole original was copied a second time, and the
/// result kept up to as much again unused for as long as it was the document's working bytes.
#[test]
fn incremental_saves_are_allocated_at_their_exact_size() {
    let table = Arc::new(fixture());
    let stream = Arc::new(write_full(&Document::open(table.clone()).unwrap(), &SaveOptions::default()).unwrap());
    let no_newline = Arc::new(fixture().trim_ascii_end().to_vec());
    for original in [table, stream, no_newline] {
        let mut doc = Document::open(original.clone()).unwrap();
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let added = doc.add(Object::Stream(Stream { dict: Dict::new(), raw: data.clone().into() }));
        let updated = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert_eq!(updated.capacity(), updated.len());
        assert!(updated.starts_with(&original), "an incremental save only appends");
        let back = Document::open(Arc::new(updated.clone())).unwrap();
        assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
        match &*back.get(added) {
            Object::Stream(s) => assert_eq!(*s.raw, data),
            other => panic!("{other:?}"),
        }
        assert_eq!(hayro_pages(&updated), 4);
    }
}

/// A full save grows its buffer as it writes, so it used to end with up to as much again unused,
/// kept alive with the document's working bytes. It now ends at its exact size.
#[test]
fn full_saves_are_allocated_at_their_exact_size() {
    let mut doc = Document::open(Arc::new(fixture())).unwrap();
    let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let added = doc.add(Object::Stream(Stream { dict: Dict::new(), raw: Arc::new(data.clone()).into() }));
    let root = doc.root().unwrap();
    doc.update_dict(root, |d| d.set(b"Extra".to_vec(), Object::Ref(added))).unwrap();
    for object_streams in [true, false] {
        let full = write_full(&doc, &SaveOptions { object_streams, ..SaveOptions::default() }).unwrap();
        assert_eq!(full.capacity(), full.len(), "object streams: {object_streams}");
        let back = Document::open(Arc::new(full.clone())).unwrap();
        assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
        let extra = back.dict(&Object::Ref(back.root().unwrap())).and_then(|d| d.get(b"Extra").cloned()).unwrap();
        match &*back.resolve(&extra) {
            Object::Stream(s) => assert_eq!(*s.raw, data),
            other => panic!("{other:?}"),
        }
        assert_eq!(hayro_pages(&full), 4);
    }
}

/// A one-page classic-table file with no `/ID`, the case that used to append `xref` and then
/// `trailer` with no subsection (#1339).
fn classic_table() -> Vec<u8> {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << >> /Contents 4 0 R >>",
        "<< /Length 0 >>\nstream\n\nendstream",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
    bytes
}

fn qpdf_check(qpdf: &pdfcraft_oracle::Oracle, bytes: &[u8]) -> std::process::Output {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("pdfcraft-xref-{}-{n}.pdf", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let check = qpdf.command().arg("--check").arg(&path).output().unwrap();
    let _ = std::fs::remove_file(&path);
    check
}

fn page(doc: &Document) -> Dict {
    let root = doc.resolve(doc.trailer().get(b"Root").unwrap());
    let pages = doc.resolve(root.as_dict().unwrap().get(b"Pages").unwrap());
    let kids = pages.as_dict().unwrap().get(b"Kids").unwrap().as_array().unwrap();
    doc.resolve(kids.first().unwrap()).as_dict().cloned().unwrap()
}

fn assert_qpdf_clean(bytes: &[u8]) {
    let qpdf = pdfcraft_oracle::or_skip!(pdfcraft_oracle::qpdf());
    let check = qpdf_check(&qpdf, bytes);
    let report = format!("{}{}", String::from_utf8_lossy(&check.stdout), String::from_utf8_lossy(&check.stderr));
    assert!(check.status.success() && !report.contains("WARNING"), "{report}");
}

#[test]
fn unchanged_incremental_save_of_a_classic_table_is_a_valid_xref() {
    let input = classic_table();
    let doc = Document::open(Arc::new(input.clone())).unwrap();
    assert!(!doc.is_modified());
    assert!(doc.repair_log().is_empty());
    assert_eq!(doc.revisions().len(), 1);
    assert!(!doc.revisions()[0].is_stream);

    let unchanged = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert!(unchanged.starts_with(&input));
    let appended = &unchanged[input.len()..];
    assert!(appended.windows(8).any(|w| w == b"xref\n0 0"), "a section with no entries still needs a subsection");
    assert!(!appended.windows(13).any(|w| w == b"xref\ntrailer"));

    let back = Document::open(Arc::new(unchanged.clone())).unwrap();
    assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
    assert_eq!(back.revisions().len(), 2);
    assert_eq!(page(&back).name(b"Type"), Some(b"Page".as_slice()));
    assert!(back.trailer().get(b"ID").is_some(), "a file without an identifier still receives one");
    assert_eq!(hayro_pages(&unchanged), 1);
    assert_qpdf_clean(&unchanged);

    let mut edited = doc.clone();
    edited.update_dict(ObjRef::new(3, 0), |d| d.set(b"Rotate".to_vec(), Object::Int(90))).unwrap();
    let edited = write_incremental(&edited, &SaveOptions::default()).unwrap();
    let edited_back = Document::open(Arc::new(edited.clone())).unwrap();
    assert!(edited_back.repair_log().is_empty(), "{:?}", edited_back.repair_log());
    assert_eq!(edited_back.get(ObjRef::new(3, 0)).as_dict().unwrap().int(b"Rotate"), Some(90));
    assert_qpdf_clean(&edited);

    let stamped = write_incremental(&doc, &SaveOptions { mod_date: Some("D:20260101000000Z".into()), ..SaveOptions::default() }).unwrap();
    let stamped_back = Document::open(Arc::new(stamped.clone())).unwrap();
    assert!(stamped_back.repair_log().is_empty(), "{:?}", stamped_back.repair_log());
    let info = stamped_back.resolve(stamped_back.trailer().get(b"Info").unwrap());
    assert_eq!(
        info.as_dict().unwrap().get(b"ModDate").and_then(|d| d.as_string()).map(|s| s.bytes.as_slice()),
        Some(b"D:20260101000000Z".as_slice())
    );
    assert_qpdf_clean(&stamped);
}

#[test]
fn incremental_save_keeps_a_trailer_only_edit() {
    let input = classic_table();
    let mut doc = Document::open(Arc::new(input)).unwrap();
    assert!(!doc.is_modified());
    doc.trailer_mut().set(b"Custom".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"keep me".to_vec())));
    let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
    let back = Document::open(Arc::new(saved.clone())).unwrap();
    assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
    assert_eq!(back.trailer().get(b"Custom").and_then(|o| o.as_string()).map(|s| s.bytes.as_slice()), Some(b"keep me".as_slice()));
    assert_eq!(hayro_pages(&saved), 1);
    assert_qpdf_clean(&saved);
}

/// What one edit added and a later one unlinked (a deleted comment, a cleared redaction mark) is
/// not written: the file never held it, and writing it would keep what was removed. Additions the
/// update still refers to, from the trailer or a changed object and through other additions, are.
#[test]
fn incremental_save_leaves_out_additions_nothing_refers_to() {
    let mut doc = Document::open(Arc::new(classic_table())).unwrap();
    let text = |s: &str| Object::String(pdfcraft_cos::PdfString::literal(s.as_bytes().to_vec()));
    let dict = |entries: &[(&str, Object)]| {
        let mut d = Dict::new();
        entries.iter().for_each(|(k, v)| d.set(k.as_bytes().to_vec(), v.clone()));
        Object::Dict(d)
    };
    let page_obj = ObjRef::new(3, 0);
    let leaf = doc.add(text("kept leaf"));
    let kept = doc.add(dict(&[("Leaf", Object::Ref(leaf))]));
    doc.update_dict(page_obj, |d| d.set(b"PieceInfo".to_vec(), Object::Ref(kept))).unwrap();
    let by_trailer = doc.add(text("kept by the trailer"));
    doc.trailer_mut().set(b"Custom".to_vec(), Object::Ref(by_trailer));
    // Linked from the page and unlinked again, as clearing a mark does.
    let dropped_leaf = doc.add(text("dropped leaf"));
    let dropped = doc.add(dict(&[("Leaf", Object::Ref(dropped_leaf))]));
    doc.update_dict(page_obj, |d| d.set(b"Annots".to_vec(), Object::Array(vec![Object::Ref(dropped)]))).unwrap();
    doc.update_dict(page_obj, |d| {
        d.remove(b"Annots");
    })
    .unwrap();
    // Unlinked additions that refer to each other.
    let first = doc.add(Object::Null);
    let second = doc.add(dict(&[("Next", Object::Ref(first)), ("Note", text("dropped cycle"))]));
    doc.set(first, dict(&[("Next", Object::Ref(second))]));

    // Off by default: a writer may add what only it finds again, by scanning.
    let kept_all = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert!(kept_all.windows(12).any(|w| w == b"dropped leaf"));
    let saved = write_incremental(&doc, &SaveOptions { drop_unlinked: true, ..SaveOptions::default() }).unwrap();
    let holds = |s: &str| saved.windows(s.len()).any(|w| w == s.as_bytes());
    assert!(holds("kept leaf") && holds("kept by the trailer"));
    assert!(!holds("dropped leaf") && !holds("dropped cycle"), "unlinked additions are written");
    let back = Document::open(Arc::new(saved.clone())).unwrap();
    assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
    assert_eq!(back.get(leaf).as_string().map(|s| s.bytes.as_slice()), Some(b"kept leaf".as_slice()));
    for gone in [dropped, dropped_leaf, first, second] {
        assert_eq!(*back.get(gone), Object::Null);
    }
    assert!(!page(&back).contains(b"Annots"));
    assert_qpdf_clean(&saved);

    // Freed beforehand, they are written the same way without the option, and what is added
    // after that, nothing referring to it either (a document timestamp), is kept.
    let mut freed = doc.clone();
    freed.free_unlinked_additions();
    assert_eq!(write_incremental(&freed, &SaveOptions::default()).unwrap(), saved, "the same objects, freed the same way");
    freed.add(text("added after"));
    let stamped = write_incremental(&freed, &SaveOptions::default()).unwrap();
    assert!(stamped.windows(11).any(|w| w == b"added after") && !stamped.windows(12).any(|w| w == b"dropped leaf"));
}

#[test]
fn qpdf_accepts_object_stream_output() {
    let qpdf = pdfcraft_oracle::or_skip!(pdfcraft_oracle::qpdf());
    let doc = Document::open(Arc::new(fixture())).unwrap();
    let packed = write_full(&doc, &SaveOptions::default()).unwrap();
    let path = std::env::temp_dir().join(format!("pdfcraft-objstm-{}.pdf", std::process::id()));
    std::fs::write(&path, &packed).unwrap();
    let check = qpdf.command().arg("--check").arg(&path).output().unwrap();
    let _ = std::fs::remove_file(&path);
    let report = String::from_utf8_lossy(&check.stdout);
    assert!(check.status.success() && report.contains("No syntax or stream encoding errors"), "{report}");
}

#[test]
fn the_catalog_of_an_encrypted_document_stays_outside_object_streams() {
    // Acrobat reports an encrypted file as damaged when its catalog is in an object stream,
    // before asking for the password (#774). Unencrypted output still packs the catalog.
    fn root_is_packed(bytes: &[u8], password: Option<&str>) -> bool {
        let doc = Document::open_with_password(Arc::new(bytes.to_vec()), password).unwrap();
        let root = doc.trailer().reference(b"Root").unwrap();
        let direct = format!("\n{} 0 obj", root.num);
        !bytes.windows(direct.len()).any(|w| w == direct.as_bytes())
    }
    let plain = Document::open(Arc::new(fixture())).unwrap();
    assert!(root_is_packed(&write_full(&plain, &SaveOptions::default()).unwrap(), None), "unencrypted: catalog in an object stream");
    let mut enc = plain.clone();
    enc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "user",
        owner_password: "owner",
        permissions: -4,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    let packed = write_full(&enc, &SaveOptions::default()).unwrap();
    assert!(!root_is_packed(&packed, Some("user")), "encrypted: catalog must be a standalone object");
    let back = Document::open_with_password(Arc::new(packed.clone()), Some("user")).expect("reopens");
    assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
    assert_eq!(title(&back), b"Caf\xe9 (draft)");
}

#[test]
fn objects_referenced_by_the_encrypt_dictionary_stay_outside_object_streams() {
    // Encrypt a document, then move its crypt filters into indirect objects, as some producers
    // do (pdf.js issue7665). A full save must keep them readable before decryption.
    let doc = Document::open(Arc::new(fixture())).unwrap();
    let mut enc = doc.clone();
    enc.set_encryption(&pdfcraft_cos::NewEncryption {
        algorithm: pdfcraft_cos::Algorithm::Aes256,
        user_password: "",
        owner_password: "owner",
        permissions: -4,
        encrypt_metadata: true,
        seed: [7; 32],
    })
    .unwrap();
    let bytes = Arc::new(write_full(&enc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap());
    let mut doc = Document::open(bytes).unwrap();
    let enc_ref = doc.trailer().reference(b"Encrypt").expect("indirect /Encrypt");
    let cf = doc.get(enc_ref).as_dict().unwrap().get(b"CF").cloned().unwrap();
    let cf_ref = doc.add(cf);
    doc.update_dict(enc_ref, |d| d.set(b"CF".to_vec(), Object::Ref(cf_ref))).unwrap();
    let packed = write_full(&doc, &SaveOptions::default()).unwrap();
    let back = Document::open(Arc::new(packed.clone())).expect("reopens");
    assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
    assert_eq!(title(&back), b"Caf\xe9 (draft)");
    assert_eq!(hayro_pages(&packed), 4);
}

#[test]
fn page_tree_loops_in_object_streams_do_not_crash_readers() {
    // pdf.js Pages-tree-refs.pdf: /Kids 3 → 4 → 5 → 3. Written faithfully into object streams,
    // it used to overflow hayro-syntax's stack (vendored patch: cycle guard in `resolve_pages`).
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [6 0 R 3 0 R] /Count 2 /MediaBox [0 0 595 842] >>",
        "<< /Type /Pages /Kids [4 0 R] /Count 1 >>",
        "<< /Type /Pages /Kids [5 0 R] /Count 1 >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R >>",
    ];
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
    let doc = Document::open(Arc::new(out)).unwrap();
    let packed = write_full(&doc, &SaveOptions::default()).unwrap();
    assert!(String::from_utf8_lossy(&packed).contains("/ObjStm"));
    assert_eq!(hayro_pages(&packed), 1);
}

/// Catalog, empty page tree, and trailer entries a reader must not invent: a direct name, a
/// direct string, and a string object reachable only from the trailer.
fn trailer_fixture() -> Vec<u8> {
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [] /Count 0 >>", "(only from the trailer)"].iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size 4 /Root 1 0 R /Version /1.7 /Custom (keep me) /Note 3 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    out
}

fn trailer_custom(doc: &Document) -> Option<Vec<u8>> {
    doc.trailer().get(b"Custom").and_then(Object::as_string).map(|s| s.bytes.clone())
}

fn trailer_note(doc: &Document) -> Option<Vec<u8>> {
    doc.trailer().get(b"Note").map(|o| doc.resolve(o)).and_then(|o| o.as_string().map(|s| s.bytes.clone()))
}

#[test]
fn full_save_preserves_unknown_trailer_data() {
    let input = trailer_fixture();
    let doc = Document::open(Arc::new(input)).unwrap();
    assert_eq!(trailer_custom(&doc), Some(b"keep me".to_vec()));
    assert_eq!(trailer_note(&doc).as_deref(), Some(b"only from the trailer".as_slice()));
    assert_eq!(doc.trailer().get(b"Version").and_then(Object::as_name), Some(&b"1.7"[..]));
    assert!(doc.repair_log().is_empty());
    assert!(!doc.full_save_required());

    let inc = Document::open(Arc::new(write_incremental(&doc, &SaveOptions::default()).unwrap())).unwrap();
    assert_eq!(trailer_custom(&inc), trailer_custom(&doc));
    assert_eq!(trailer_note(&inc), trailer_note(&doc));
    assert_eq!(inc.trailer().get(b"Version"), doc.trailer().get(b"Version"));
    assert!(inc.repair_log().is_empty());

    for object_streams in [true, false] {
        let opts = SaveOptions { object_streams, ..SaveOptions::default() };
        let bytes = write_full(&doc, &opts).unwrap();
        let back = Document::open(Arc::new(bytes)).unwrap();
        assert!(back.root().is_some());
        assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
        assert_eq!(trailer_custom(&back), trailer_custom(&doc));
        assert_eq!(trailer_note(&back), trailer_note(&doc));
        assert_eq!(back.trailer().get(b"Version"), doc.trailer().get(b"Version"));
    }

    let mut required = doc.clone();
    required.require_full_save();
    let back = Document::open(Arc::new(write_incremental(&required, &SaveOptions::default()).unwrap())).unwrap();
    assert!(back.root().is_some());
    assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
    assert_eq!(trailer_custom(&back), trailer_custom(&doc));
    assert_eq!(trailer_note(&back), trailer_note(&doc));
    assert_eq!(back.trailer().get(b"Version"), doc.trailer().get(b"Version"));
}

/// The `startxref` value at the end of `bytes`.
fn last_startxref(bytes: &[u8]) -> usize {
    let at = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
    String::from_utf8_lossy(&bytes[at + 9..]).split_whitespace().next().unwrap().parse().unwrap()
}

#[test]
fn incremental_saves_of_a_file_with_bytes_before_its_header_count_from_the_header() {
    // Offsets written before the prefix was added count from `%PDF-`, as in the KillerPDF file
    // of #2364; the appended section must count the same way as the `/Prev` it points back to.
    let prefix = vec![b'#'; 128];
    let as_stream = write_full(&Document::open(Arc::new(fixture())).unwrap(), &SaveOptions::default()).unwrap();
    for (name, body, marker) in [("table", classic_table(), &b"xref"[..]), ("stream", as_stream, &b" 0 obj"[..])] {
        let input = [prefix.as_slice(), &body].concat();
        let mut doc = Document::open(Arc::new(input.clone())).unwrap();
        assert!(doc.revisions().last().unwrap().header_relative, "{name}");
        doc.update_dict(ObjRef::new(1, 0), |d| d.set(b"Lang".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"en".to_vec())))).unwrap();
        let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
        assert!(saved.starts_with(&input), "{name}");

        let xref = last_startxref(&saved) + prefix.len();
        let section = saved.get(xref..).unwrap_or_default();
        assert!(section.windows(marker.len()).position(|w| w == marker).is_some_and(|p| p < 16), "{name}: startxref misses its section");
        let back = Document::open(Arc::new(saved.clone())).unwrap();
        assert_eq!(back.revisions().len(), doc.revisions().len() + 1, "{name}");
        assert!(back.revisions().iter().all(|r| r.header_relative), "{name}: one convention across the chain");
        let edited = back.get(ObjRef::new(1, 0));
        assert_eq!(edited.as_dict().unwrap().get(b"Lang").and_then(|o| o.as_string()).map(|s| s.bytes.as_slice()), Some(b"en".as_slice()));
        assert!(back.xref_entry_is_exact(1), "{name}");
        assert_qpdf_clean(&saved);
    }
}

#[test]
fn incremental_saves_of_a_file_whose_offsets_count_from_its_first_byte_keep_counting_so() {
    // The other convention: the same file, but its offsets include the bytes before the header.
    let prefix = vec![b'#'; 128];
    let mut input = [prefix.as_slice(), &classic_table()].concat();
    let xref = input.windows(6).rposition(|w| w == b"\nxref\n").unwrap() + 1;
    let table: String = String::from_utf8_lossy(&input[xref..])
        .lines()
        .map(|line| match line.strip_suffix(" 00000 n ").and_then(|o| o.parse::<usize>().ok()) {
            Some(o) => format!("{:010} 00000 n \n", o + prefix.len()),
            None if line.parse::<usize>().is_ok() => format!("{xref}\n"),
            None => format!("{line}\n"),
        })
        .collect();
    input.truncate(xref);
    input.extend_from_slice(table.as_bytes());

    let mut doc = Document::open(Arc::new(input.clone())).unwrap();
    assert!(doc.repair_log().is_empty(), "{:?}", doc.repair_log());
    assert!(!doc.revisions().last().unwrap().header_relative);
    doc.update_dict(ObjRef::new(1, 0), |d| d.set(b"Lang".to_vec(), Object::String(pdfcraft_cos::PdfString::literal(b"en".to_vec())))).unwrap();
    let saved = write_incremental(&doc, &SaveOptions::default()).unwrap();
    assert!(saved.get(last_startxref(&saved)..).unwrap_or_default().starts_with(b"xref"));
    let back = Document::open(Arc::new(saved.clone())).unwrap();
    assert!(back.repair_log().is_empty(), "{:?}", back.repair_log());
    assert_eq!(back.revisions().len(), 2);
    assert!(back.xref_entry_is_exact(1));
    // No qpdf check here: qpdf counts every offset from the header in a file with bytes before
    // it, so it already reconstructs this input before anything is appended.
}
