//! What decryption reads and leaves alone: only the document's own metadata stream stays in the
//! clear under `/EncryptMetadata false` (ISO 32000-2 Table 21), the strings of a cross-reference
//! stream's dictionary are never encrypted (§7.5.8.2), and an `/Encrypt` that is there but can't be
//! read is kept, not dropped as if the file had none.

use std::io::Write;
use std::sync::Arc;

use pdfcraft_cos::{Algorithm, Document, NewEncryption, Object, SaveOptions, write_full, write_incremental};
use pdfcraft_crypt::{SecurityHandler, StreamKind};

const DOC_XMP: &str = "<x:xmpmeta>document-level</x:xmpmeta>";
const PAGE_XMP: &str = "<x:xmpmeta>page-level</x:xmpmeta>";
const SECRET: &str = "BT /F1 18 Tf 20 100 Td (Secret page) Tj ET";
const ID: &str = "00112233445566778899aabbccddeeff";

/// A file's bytes from its objects (numbered from 1) and the rest of its trailer.
fn file(objs: &[String], trailer: &str) -> Vec<u8> {
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
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R{trailer} >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
    out
}

/// A stream object of ASCII `data` (encrypted data is put in by [`encrypted_by_hand`]).
fn stream(dict: &str, data: &[u8]) -> String {
    format!("<< {dict} /Length {} >>\nstream\n{}\nendstream", data.len(), String::from_utf8_lossy(data))
}

/// The objects of a one-page document with document- and page-level metadata: object 4 is
/// `content`, 5 the catalog's metadata `doc_meta` and 6 the page's, `page_meta`.
fn metadata_objects(content: &str, doc_meta: &str, page_meta: &str) -> Vec<String> {
    vec![
        "<< /Type /Catalog /Pages 2 0 R /Metadata 5 0 R >>".into(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R /Metadata 6 0 R /Resources << /Font << /F1 7 0 R >> >> >>".into(),
        content.into(),
        doc_meta.into(),
        page_meta.into(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ]
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// The `/Encrypt` dictionary of `h`, with `/EncryptMetadata false`.
fn encrypt_dict(h: &SecurityHandler) -> String {
    let d = h.dict();
    let mut e = format!("<< /Filter /Standard /V {} /R {} /Length {} /O <{}> /U <{}> /P {}", d.v, d.r, d.length_bits, hex(&d.o), hex(&d.u), d.p);
    if d.v >= 4 {
        let (cfm, len) = if d.v >= 5 { ("AESV3", 32) } else { ("AESV2", 16) };
        e.push_str(&format!(" /CF << /StdCF << /CFM /{cfm} /Length {len} /AuthEvent /DocOpen >> >> /StmF /StdCF /StrF /StdCF"));
    }
    if d.v >= 5 {
        e.push_str(&format!(" /OE <{}> /UE <{}> /Perms <{}>", hex(&d.oe), hex(&d.ue), hex(&d.perms)));
    }
    e.push_str(" /EncryptMetadata false >>");
    e
}

/// A file encrypted with `alg`, its `/Encrypt` saying `/EncryptMetadata false`, written here and
/// not by the writer under test: the content and the page's metadata stream encrypted, and the
/// catalog's in the clear, unless `/V` is below 4, where the entry means nothing and every stream
/// is encrypted.
fn encrypted_by_hand(alg: Algorithm) -> Vec<u8> {
    let params = NewEncryption { algorithm: alg, user_password: "", owner_password: "own", permissions: -4, encrypt_metadata: false, seed: [3; 32] };
    let h = pdfcraft_crypt::create(&params, &unhex(ID)).unwrap();
    let content = h.encrypt_stream(4, 0, SECRET.as_bytes(), StreamKind::Normal);
    let doc_meta = if h.dict().v >= 4 { DOC_XMP.as_bytes().to_vec() } else { h.encrypt_stream(5, 0, DOC_XMP.as_bytes(), StreamKind::Normal) };
    let page_meta = h.encrypt_stream(6, 0, PAGE_XMP.as_bytes(), StreamKind::Normal);
    let mut objs = metadata_objects("@4", "@5", "@6");
    objs.push(encrypt_dict(&h));
    let text = file(&objs, &format!(" /Encrypt 8 0 R /ID [<{ID}> <{ID}>]"));
    // Put the stream data in where the placeholders are, then index the objects again.
    let mut out = Vec::new();
    let mut rest = text.as_slice();
    for (mark, data) in [(&b"@4"[..], &content), (&b"@5"[..], &doc_meta), (&b"@6"[..], &page_meta)] {
        let at = rest.windows(2).position(|w| w == mark).unwrap();
        out.extend_from_slice(&rest[..at]);
        let dict = if mark == b"@4" { "" } else { "/Type /Metadata /Subtype /XML " };
        let body = [
            format!(
                "<< {dict}/Length {} >>
stream
",
                data.len()
            )
            .into_bytes(),
            data.clone(),
            b"
endstream"
                .to_vec(),
        ]
        .concat();
        out.extend_from_slice(&body);
        rest = &rest[at + 2..];
    }
    out.extend_from_slice(rest);
    renumber_xref(&out)
}

/// `bytes` with its cross-reference table rewritten from where its object headers are.
fn renumber_xref(bytes: &[u8]) -> Vec<u8> {
    // The table, not `startxref`.
    let xref = bytes.windows(6).rposition(|w| w == b"\nxref\n").unwrap() + 1;
    let mut out = bytes[..xref].to_vec();
    let mut offsets = Vec::new();
    for n in 1.. {
        let header = format!("\n{n} 0 obj\n");
        match out.windows(header.len()).position(|w| w == header.as_bytes()) {
            Some(at) => offsets.push(at + 1),
            None => break,
        }
    }
    let tail = &bytes[xref..];
    let trailer = tail.windows(7).position(|w| w == b"trailer").unwrap();
    let start = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for o in offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    let trailer_text = String::from_utf8_lossy(&tail[trailer..]);
    let dict_end = trailer_text.find("startxref").unwrap();
    out.extend_from_slice(trailer_text[..dict_end].as_bytes());
    out.extend_from_slice(format!("startxref\n{start}\n%%EOF\n").as_bytes());
    out
}

fn data_of(doc: &Document, num: u32) -> Vec<u8> {
    match doc.try_get(num).unwrap().as_ref() {
        Object::Stream(s) => s.decoded().unwrap(),
        other => panic!("object {num} is not a stream: {other:?}"),
    }
}

fn holds(bytes: &[u8], text: &str) -> bool {
    bytes.windows(text.len()).any(|w| w == text.as_bytes())
}

/// `/EncryptMetadata false` leaves the catalog's metadata stream in the clear, and only that one: a
/// page's metadata stream is encrypted as any stream is, and reads as what it encrypts.
#[test]
fn a_pages_metadata_stream_is_decrypted_when_metadata_is_left_in_the_clear() {
    for alg in [Algorithm::Aes128, Algorithm::Aes256] {
        let bytes = encrypted_by_hand(alg);
        assert!(!holds(&bytes, PAGE_XMP) && holds(&bytes, DOC_XMP), "{alg:?}: the fixture encrypts only the page's metadata");
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert!(doc.security().is_some_and(|h| !h.encrypts_metadata()), "{alg:?}");
        assert_eq!(data_of(&doc, 4), SECRET.as_bytes(), "{alg:?}: the content decrypts");
        assert_eq!(data_of(&doc, 5), DOC_XMP.as_bytes(), "{alg:?}: the catalog's metadata is read as stored");
        assert_eq!(data_of(&doc, 6), PAGE_XMP.as_bytes(), "{alg:?}: the page's metadata decrypts");
    }
}

/// `/EncryptMetadata` means something only with `/V` 4 or 5 (Table 21): an RC4 file whose
/// dictionary says false has its catalog's metadata encrypted as any stream, and it decrypts.
#[test]
fn encrypt_metadata_means_nothing_below_v4() {
    let bytes = encrypted_by_hand(Algorithm::Rc4_128);
    assert!(!holds(&bytes, DOC_XMP) && !holds(&bytes, PAGE_XMP), "the fixture encrypts both");
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert!(doc.security().is_some_and(|h| h.dict().v == 2));
    assert_eq!(data_of(&doc, 4), SECRET.as_bytes());
    assert_eq!(data_of(&doc, 5), DOC_XMP.as_bytes(), "the catalog's metadata was read as stored");
    assert_eq!(data_of(&doc, 6), PAGE_XMP.as_bytes());
}

/// Saving with `/EncryptMetadata false` encrypts every metadata stream but the catalog's, and
/// removing the security writes what each of them holds, in the clear.
#[test]
fn only_the_documents_metadata_is_written_in_the_clear() {
    let plain = file(
        &metadata_objects(
            &stream("", SECRET.as_bytes()),
            &stream("/Type /Metadata /Subtype /XML", DOC_XMP.as_bytes()),
            &stream("/Type /Metadata /Subtype /XML", PAGE_XMP.as_bytes()),
        ),
        &format!(" /ID [<{ID}> <{ID}>]"),
    );
    for alg in [Algorithm::Aes128, Algorithm::Aes256] {
        let mut doc = Document::open(Arc::new(plain.clone())).unwrap();
        let params =
            NewEncryption { algorithm: alg, user_password: "", owner_password: "own", permissions: -4, encrypt_metadata: false, seed: [4; 32] };
        doc.set_encryption(&params).unwrap();
        let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
        assert!(holds(&bytes, DOC_XMP), "{alg:?}: the document's metadata stays readable");
        assert!(!holds(&bytes, PAGE_XMP), "{alg:?}: the page's metadata was written in the clear");
        // The file written reads back, and with its security removed holds both in the clear.
        let mut doc = Document::open_with_password(Arc::new(bytes), Some("own")).unwrap();
        doc.remove_encryption();
        let bytes = write_full(&doc, &SaveOptions::default()).unwrap();
        let doc = Document::open(Arc::new(bytes)).unwrap();
        assert!(doc.security().is_none());
        let metadata = |page: bool| {
            let catalog = doc.get(doc.root().unwrap()).as_dict().cloned().unwrap();
            let owner = if page {
                let pages = doc.get(catalog.reference(b"Pages").unwrap()).as_dict().cloned().unwrap();
                doc.resolve(&pages.get(b"Kids").unwrap().as_array().unwrap()[0]).as_dict().cloned().unwrap()
            } else {
                catalog
            };
            match doc.resolve(owner.get(b"Metadata").unwrap()).as_ref() {
                Object::Stream(s) => s.decoded().unwrap(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(metadata(false), DOC_XMP.as_bytes(), "{alg:?}");
        assert_eq!(metadata(true), PAGE_XMP.as_bytes(), "{alg:?}");
    }
}

/// A file someone else encrypted with `/EncryptMetadata false` and a page-level metadata stream
/// (as qpdf's `--cleartext-metadata` writes one) loses its security with the page's metadata
/// decrypted, not with its ciphertext written as if it were the stream's data.
#[test]
fn removing_security_writes_a_pages_metadata_decrypted() {
    let mut doc = Document::open_with_password(Arc::new(encrypted_by_hand(Algorithm::Aes128)), Some("own")).unwrap();
    doc.remove_encryption();
    let bytes = write_full(&doc, &SaveOptions { object_streams: false, ..SaveOptions::default() }).unwrap();
    assert!(holds(&bytes, PAGE_XMP), "the page's metadata was written as its ciphertext");
    assert!(holds(&bytes, DOC_XMP));
}

/// The same with qpdf as the encrypting party, when it is installed.
#[test]
fn qpdf_cleartext_metadata_leaves_only_the_catalogs_in_the_clear() {
    let qpdf = pdfcraft_oracle::or_skip!(pdfcraft_oracle::qpdf());
    let plain = file(
        &metadata_objects(
            &stream("", SECRET.as_bytes()),
            &stream("/Type /Metadata /Subtype /XML", DOC_XMP.as_bytes()),
            &stream("/Type /Metadata /Subtype /XML", PAGE_XMP.as_bytes()),
        ),
        &format!(" /ID [<{ID}> <{ID}>]"),
    );
    let dir = crate::encryption::private_temp_dir("pdfcraft-cleartext-metadata");
    let (src, out) = (dir.join("plain.pdf"), dir.join("enc.pdf"));
    std::fs::File::create_new(&src).and_then(|mut f| f.write_all(&plain)).unwrap();
    for bits in ["128", "256"] {
        let status = qpdf
            .command()
            .args(["--encrypt", "--user-password=", "--owner-password=own", &format!("--bits={bits}")])
            .args(if bits == "128" { &["--use-aes=y"][..] } else { &[][..] })
            // Streams as they are: qpdf versions differ in what they compress by default.
            .args(["--cleartext-metadata", "--", "--object-streams=disable", "--stream-data=preserve"])
            .arg(&src)
            .arg(&out)
            .status()
            .unwrap();
        assert!(status.success(), "qpdf --encrypt {bits}");
        let bytes = std::fs::read(&out).unwrap();
        // What this test compares against: a qpdf that leaves the page's metadata in the clear
        // too has nothing to say about it (the spec has it encrypted, as the tests above check).
        if holds(&bytes, PAGE_XMP) {
            eprintln!("skipping --bits={bits}: this qpdf leaves a page's metadata stream in the clear");
            continue;
        }
        let doc = Document::open(Arc::new(bytes)).unwrap();
        let found: Vec<Vec<u8>> = doc
            .object_numbers()
            .into_iter()
            .filter_map(|n| match doc.try_get(n).ok()?.as_ref() {
                Object::Stream(s) if s.dict.name(b"Type") == Some(b"Metadata") => s.decoded().ok(),
                _ => None,
            })
            .collect();
        assert!(found.contains(&PAGE_XMP.as_bytes().to_vec()), "{bits}: the page's metadata decrypts");
        assert!(found.contains(&DOC_XMP.as_bytes().to_vec()), "{bits}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// The strings of a cross-reference stream's dictionary are not encrypted (§7.5.8.2): its `/ID`
/// reads as written, the same as the trailer's.
#[test]
fn a_cross_reference_streams_id_reads_as_written() {
    let plain = file(
        &metadata_objects(
            &stream("", SECRET.as_bytes()),
            &stream("/Type /Metadata /Subtype /XML", DOC_XMP.as_bytes()),
            &stream("/Type /Metadata /Subtype /XML", PAGE_XMP.as_bytes()),
        ),
        &format!(" /ID [<{ID}> <{ID}>]"),
    );
    for alg in [Algorithm::Rc4_128, Algorithm::Aes128, Algorithm::Aes256] {
        let mut doc = Document::open(Arc::new(plain.clone())).unwrap();
        let params =
            NewEncryption { algorithm: alg, user_password: "", owner_password: "own", permissions: -4, encrypt_metadata: true, seed: [6; 32] };
        doc.set_encryption(&params).unwrap();
        let bytes = write_full(&doc, &SaveOptions { object_streams: true, ..SaveOptions::default() }).unwrap();
        let doc = Document::open(Arc::new(bytes)).unwrap();
        let xref = doc
            .object_numbers()
            .into_iter()
            .find_map(|n| match doc.try_get(n).ok()?.as_ref() {
                Object::Stream(s) if s.dict.name(b"Type") == Some(b"XRef") => Some(s.dict.clone()),
                _ => None,
            })
            .expect("a cross-reference stream");
        let id = |d: &pdfcraft_cos::Dict| {
            d.get(b"ID").and_then(Object::as_array).map(|a| a.iter().filter_map(|s| s.as_string().map(|s| s.bytes.clone())).collect::<Vec<_>>())
        };
        assert_eq!(id(&xref).unwrap()[0], unhex(ID), "{alg:?}: the cross-reference stream's /ID was decrypted");
        assert_eq!(id(&xref), id(doc.trailer()), "{alg:?}");
    }
}

/// The objects of a file whose `/Encrypt` is object 5, given by `encrypt`.
fn with_encrypt(encrypt: &str) -> Vec<u8> {
    let objs = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 300 200] >>".into(),
        "<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".into(),
        stream("", b"0 0 m 10 10 l S"),
        encrypt.into(),
    ];
    file(&objs, &format!(" /Encrypt 5 0 R /ID [<{ID}> <{ID}>]"))
}

/// An `/Encrypt` object that is there but doesn't parse is not an absent one: the entry stays,
/// the repair log says so, and neither save writes the file without it (its encrypted data could
/// never be read again) or with new data in the clear under it.
#[test]
fn an_unreadable_encrypt_dictionary_is_kept_and_saving_is_refused() {
    // A damaged dictionary (`E<` for `<<`), as fuzzed files have; and a hex string broken inside.
    for damaged in [
        "E< /Filter /Standard /V 4 /R 4 /Length 128 /P -4 /O <00> /U <00> >>",
        "<< /Filter /Standard /V 5 /R 6 /P -4 /O <00> /U <00> /OE <f7[f3> >>",
        "42",
    ] {
        let doc = Document::open(Arc::new(with_encrypt(damaged))).unwrap();
        assert!(doc.encryption_unreadable(), "{damaged}");
        assert!(doc.trailer().get(b"Encrypt").is_some(), "{damaged}: the /Encrypt entry was dropped");
        assert!(doc.repair_log().iter().any(|l| l.contains("/Encrypt dictionary is damaged")), "{damaged}: {:?}", doc.repair_log());
        assert!(!doc.repair_log().iter().any(|l| l.contains("does not exist")), "{damaged}");
        let opts = SaveOptions::default();
        assert!(write_full(&doc, &opts).is_err(), "{damaged}: a full save dropped the encryption");
        let mut edited = doc.clone();
        edited.trailer_mut().set(b"Info".to_vec(), Object::Null);
        assert!(write_incremental(&edited, &opts).is_err(), "{damaged}: an incremental save went ahead");
        let mut removed = doc.clone();
        removed.remove_encryption();
        assert!(write_full(&removed, &opts).is_err(), "{damaged}: removing the security wrote the ciphertext as data");
    }
}

/// An `/Encrypt` that refers to nothing still reads as no encryption at all, and saves.
#[test]
fn a_dangling_encrypt_reference_reads_as_unencrypted() {
    let mut bytes = with_encrypt("<< /Filter /Standard >>");
    // Object 5 becomes free.
    let row = format!("{:010} 00000 n \n", bytes.windows(8).position(|w| w == b"\n5 0 obj").unwrap() + 1);
    let at = bytes.windows(row.len()).position(|w| w == row.as_bytes()).unwrap();
    bytes.splice(at..at + row.len(), b"0000000000 65535 f \n".iter().copied());
    let doc = Document::open(Arc::new(bytes)).unwrap();
    assert!(!doc.encryption_unreadable());
    assert!(doc.security().is_none() && doc.trailer().get(b"Encrypt").is_none());
    assert_eq!(doc.repair_log().iter().filter(|l| l.contains("does not exist")).count(), 1, "{:?}", doc.repair_log());
    assert!(write_full(&doc, &SaveOptions::default()).is_ok());
}
