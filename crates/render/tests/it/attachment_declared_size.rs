//! An attachment whose `/Params /Size` is not the size of the file it holds: the listing gives
//! the size counted, and the declared one beside it, wherever the bytes can be counted without
//! decoding them (an unfiltered stream). Contributor-original synthetic files.

use std::io::Write;
use std::sync::Arc;

use pdfcraft_render::{Attachment, AttachmentSource, inspect};
use pdfcraft_testkit::{Pdf, stream_obj};

const FILE: &[u8] = b"ten bytes!";

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(data).unwrap();
    z.finish().unwrap()
}

/// Document-level files `(key, embedded-file stream dict extra, stream data)` from object 5 on,
/// two objects each (file specification, stream), and a FileAttachment comment on the page whose
/// file declares `page_size`.
fn fixture(files: &[(&str, &str, Vec<u8>)], page_size: i64) -> Vec<u8> {
    let names: String = files.iter().enumerate().map(|(i, (key, ..))| format!("({key}) {} 0 R ", 5 + 2 * i)).collect();
    let annot = 5 + 2 * files.len();
    let mut pdf = Pdf::new()
        .obj(format!("<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [{names}] >> >> >>"))
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [{annot} 0 R] >>"))
        .stream("", b"");
    for (i, (key, extra, data)) in files.iter().enumerate() {
        pdf = pdf
            .obj(format!("<< /Type /Filespec /F ({key}) /UF ({key}) /EF << /F {} 0 R >> >>", 6 + 2 * i))
            .obj(stream_obj(&format!("/Type /EmbeddedFile {extra}"), data));
    }
    pdf.obj(format!("<< /Type /Annot /Subtype /FileAttachment /Rect [10 10 30 30] /FS {} 0 R /Contents (page.txt) >>", annot + 1))
        .obj(format!("<< /Type /Filespec /F (page.txt) /UF (page.txt) /EF << /F {} 0 R >> >>", annot + 2))
        .obj(stream_obj(&format!("/Type /EmbeddedFile /Params << /Size {page_size} >>"), FILE))
        .build()
}

fn listed(bytes: Vec<u8>) -> Vec<Attachment> {
    inspect(Arc::new(bytes), None).unwrap().attachments
}

fn by_name<'a>(all: &'a [Attachment], name: &str) -> &'a Attachment {
    all.iter().find(|a| a.name == name).unwrap_or_else(|| panic!("{name} is listed: {all:?}"))
}

#[test]
fn a_declared_size_the_bytes_contradict_is_listed_beside_the_counted_one() {
    let n = FILE.len();
    let all = listed(fixture(
        &[
            ("honest.txt", &format!("/Params << /Size {n} >>"), FILE.to_vec()),
            ("lies.txt", "/Params << /Size 999999 >>", FILE.to_vec()),
            ("negative.txt", "/Params << /Size -5 >>", FILE.to_vec()),
            ("undeclared.txt", "", FILE.to_vec()),
        ],
        3,
    ));
    let lies = by_name(&all, "lies.txt");
    assert_eq!(lies.size, Some(n), "the size is the file's, counted: {lies:?}");
    assert_eq!(lies.declared_size, Some(999_999), "and the size it claims is beside it");
    for name in ["honest.txt", "undeclared.txt", "negative.txt"] {
        let a = by_name(&all, name);
        assert_eq!((a.size, a.declared_size), (Some(n), None), "{name}: counted, nothing contradicted: {a:?}");
    }
    let page = by_name(&all, "page.txt");
    assert!(matches!(page.source, AttachmentSource::Annotation { .. }), "{page:?}");
    assert_eq!((page.size, page.declared_size), (Some(n), Some(3)), "a page's file is checked the same way");
}

#[test]
fn a_filtered_file_is_not_contradicted_by_its_stored_length() {
    // A compressed file's stored bytes are not its size, so its declared size stands unchallenged.
    let packed = deflate(FILE);
    let all = listed(fixture(&[("packed.txt", &format!("/Filter /FlateDecode /Params << /Size {} >>", FILE.len()), packed.clone())], 10));
    let a = by_name(&all, "packed.txt");
    assert_ne!(packed.len(), FILE.len(), "the fixture's stored length differs from the file's");
    assert_eq!((a.size, a.declared_size), (Some(FILE.len()), None), "{a:?}");
    // An empty filter list is no filter: those bytes are counted.
    let all = listed(fixture(&[("plain.txt", "/Filter [] /Params << /Size 4 >>", FILE.to_vec())], 10));
    let a = by_name(&all, "plain.txt");
    assert_eq!((a.size, a.declared_size), (Some(FILE.len()), Some(4)), "{a:?}");
}
