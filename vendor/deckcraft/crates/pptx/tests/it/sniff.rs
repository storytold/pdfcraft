//! `sniff` must recognise a package wherever the writer put `[Content_Types].xml`: recent
//! PowerPoint builds store it as the last zip entry, well past the first few kilobytes.

use std::io::{Cursor, Read, Write};

use deckcraft_engine::Session;
use zip::write::SimpleFileOptions;

fn sample_pptx() -> Vec<u8> {
    let mut s = Session::new();
    deckcraft_engine::sample::open_sample(&mut s).expect("sample");
    let p = s.doc().expect("doc").doc.as_ref().clone();
    deckcraft_pptx::export(&p).expect("export")
}

/// Re-pack `bytes` with `[Content_Types].xml` moved to the end and a padding part in front, so
/// the content-types name first appears far beyond any fixed-size prefix.
fn content_types_last(bytes: &[u8]) -> Vec<u8> {
    let mut src = zip::ZipArchive::new(Cursor::new(bytes)).expect("zip");
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    out.start_file("ppt/media/padding.bin", stored).expect("start");
    out.write_all(&vec![0u8; 64 * 1024]).expect("padding");
    let names: Vec<String> = (0..src.len()).map(|i| src.by_index(i).expect("entry").name().to_string()).collect();
    for name in names.iter().filter(|n| *n != "[Content_Types].xml").chain(std::iter::once(&"[Content_Types].xml".to_string())) {
        let mut data = Vec::new();
        src.by_name(name).expect("by_name").read_to_end(&mut data).expect("read");
        out.start_file(name, SimpleFileOptions::default()).expect("start");
        out.write_all(&data).expect("write");
    }
    out.finish().expect("finish").into_inner()
}

#[test]
fn sniff_accepts_content_types_anywhere_in_the_archive() {
    let bytes = sample_pptx();
    assert!(deckcraft_pptx::sniff(&bytes));

    let moved = content_types_last(&bytes);
    let first_4k = &moved[..4096.min(moved.len())];
    assert!(!first_4k.windows(19).any(|w| w == b"[Content_Types].xml"), "test setup: name must not be in the prefix");
    assert!(deckcraft_pptx::sniff(&moved));
    let p = deckcraft_pptx::import(&moved).expect("import");
    assert!(!p.slides.is_empty());
}

#[test]
fn sniff_rejects_non_packages() {
    assert!(!deckcraft_pptx::sniff(b""));
    assert!(!deckcraft_pptx::sniff(b"PK"));
    assert!(!deckcraft_pptx::sniff(b"%PDF-1.7 [Content_Types].xml"));

    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    out.start_file("hello.txt", SimpleFileOptions::default()).expect("start");
    out.write_all(b"not an office package").expect("write");
    let plain_zip = out.finish().expect("finish").into_inner();
    assert!(!deckcraft_pptx::sniff(&plain_zip));
}
