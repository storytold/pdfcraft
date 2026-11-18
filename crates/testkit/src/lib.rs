//! Synthetic PDF fixtures: one way to write the bytes, so tests differ only where they mean to.
//!
//! 55 files across the workspace hand-rolled the same serialiser — number the objects, record each
//! one's offset, emit a classic cross-reference table and a trailer — 111 times over. The object
//! *bodies* are properly the business of the crate under test; the envelope around them is not,
//! and every copy of it is a place for a fixture to be subtly wrong in a way that makes a test
//! prove less than it looks like it proves.
//!
//! [`Pdf`] writes that envelope. It is deliberately dependency-free, so the crates at the bottom of
//! the layering (`cos`, `geom`, `filters`, `crypt`) can use it in their own tests without a cycle,
//! and it hands back bytes rather than a parsed document for the same reason.
//!
//! ```
//! use pdfcraft_testkit::Pdf;
//! let bytes = Pdf::new()
//!     .obj("<< /Type /Catalog /Pages 2 0 R >>")
//!     .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
//!     .obj("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] /Contents 4 0 R >>")
//!     .stream("", b"BT /F1 12 Tf (hi) Tj ET")
//!     .build();
//! assert!(bytes.starts_with(b"%PDF-1.7\n"));
//! assert!(bytes.ends_with(b"%%EOF\n"));
//! ```
//!
//! This crate is test scaffolding (`Class::Testkit` in `xtask/src/layers.rs`): a dev-dependency
//! only, never compiled into the product.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// A small PDF with a classic cross-reference table, built one object at a time.
///
/// Objects are numbered from 1 in the order they are added, all at generation 0, so the first
/// object added is `1 0 R`. The trailer names `1 0 R` as `/Root`, which is what every caller of
/// this builder wants; [`Pdf::trailer_entry`] adds the rest.
#[derive(Debug, Clone)]
pub struct Pdf {
    version: String,
    objects: Vec<Vec<u8>>,
    trailer: Vec<String>,
}

impl Default for Pdf {
    fn default() -> Self {
        Self::new()
    }
}

impl Pdf {
    /// An empty PDF 1.7 file. Add objects with [`Pdf::obj`].
    pub fn new() -> Self {
        Self { version: "1.7".to_owned(), objects: Vec::new(), trailer: Vec::new() }
    }

    /// Set the header version, as it appears after `%PDF-` (for example `"1.4"`).
    #[must_use]
    pub fn version(mut self, version: &str) -> Self {
        self.version = version.to_owned();
        self
    }

    /// Append one object body — everything between `N 0 obj` and `endobj`.
    #[must_use]
    pub fn obj(mut self, body: impl AsRef<[u8]>) -> Self {
        self.objects.push(body.as_ref().to_vec());
        self
    }

    /// Append several object bodies, in order.
    #[must_use]
    pub fn objs<I: IntoIterator<Item = T>, T: AsRef<[u8]>>(mut self, bodies: I) -> Self {
        self.objects.extend(bodies.into_iter().map(|b| b.as_ref().to_vec()));
        self
    }

    /// Append a stream object: `dict` holds any entries besides `/Length`, which is computed.
    #[must_use]
    pub fn stream(self, dict: &str, data: impl AsRef<[u8]>) -> Self {
        self.obj(stream_obj(dict, data))
    }

    /// Add an entry to the trailer dictionary, such as `"/Info 9 0 R"`. `/Size` and `/Root` are
    /// always written.
    #[must_use]
    pub fn trailer_entry(mut self, entry: &str) -> Self {
        self.trailer.push(entry.to_owned());
        self
    }

    /// The number the next object added will get, so a caller can refer forward to it.
    ///
    /// ```
    /// # use pdfcraft_testkit::Pdf;
    /// let mut p = Pdf::new().obj("<< /Type /Catalog /Pages 2 0 R >>");
    /// assert_eq!(p.next_obj(), 2);
    /// ```
    pub fn next_obj(&self) -> usize {
        self.objects.len().saturating_add(1)
    }

    /// Serialise the file.
    pub fn build(&self) -> Vec<u8> {
        let mut out = format!("%PDF-{}\n", self.version).into_bytes();
        let mut offsets = Vec::with_capacity(self.objects.len());
        for (i, body) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i.saturating_add(1)).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let startxref = out.len();
        let size = self.objects.len().saturating_add(1);
        out.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        let extra = self.trailer.iter().map(|e| format!(" {e}")).collect::<String>();
        out.extend_from_slice(format!("trailer\n<< /Size {size} /Root 1 0 R{extra} >>\nstartxref\n{startxref}\n%%EOF\n").as_bytes());
        out
    }
}

/// A stream object body with `/Length` filled in, for callers assembling object lists themselves.
///
/// `dict` holds any entries besides `/Length`; an empty one gives `<< /Length N >>` rather than
/// the doubled space the hand-rolled copies produced, which parsers accept but readers trip over.
pub fn stream_obj(dict: &str, data: impl AsRef<[u8]>) -> Vec<u8> {
    let data = data.as_ref();
    let dict = dict.trim();
    let sep = if dict.is_empty() { "" } else { " " };
    let mut out = format!("<<{sep}{dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    out
}

/// A three-page file (200, 300 and 400 points wide, in that order, the second with a link to
/// `https://example.com/`) whose page tree sits in an object stream of its first revision, plus an update that rewrites the catalog and the first
/// page in a cross-reference stream: with `/Prev` when `linked`, without it otherwise (the shape
/// of the signed PDFBOX-2720 and PDFBOX-2942 files). Without `/Prev` a reader that follows the
/// chain (ISO 32000-2 §7.5.6, §7.5.8.2) finds no page tree, and a scan for page dictionaries
/// outside object streams finds only the rewritten page.
pub fn update_without_prev(linked: bool) -> Vec<u8> {
    let (mut out, eight) = unlinked_original();
    // The update: the catalog and the first page again, and a section for them and itself.
    let (catalog, first) = unlinked_rewrites(&mut out);
    let nine = out.len();
    let mut rows = Vec::new();
    xref_row(&mut rows, 1, catalog, 0);
    xref_row(&mut rows, 1, first, 0);
    xref_row(&mut rows, 1, nine, 0);
    let prev = if linked { format!(" /Prev {eight}") } else { String::new() };
    out.extend_from_slice(
        format!("9 0 obj\n<< /Type /XRef /Size 11 /Root 1 0 R /Index [1 1 3 1 9 1] /W [1 4 2]{prev} /Length {} >>\nstream\n", rows.len()).as_bytes(),
    );
    out.extend_from_slice(&rows);
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{nine}\n%%EOF\n").as_bytes());
    out
}

/// [`update_without_prev`] with the update written as a hybrid-reference section (ISO 32000-2
/// §7.5.8.4): a classic table whose trailer names a cross-reference stream with `/XRefStm`. That
/// stream belongs to the same update; it is not an earlier section, so without `/Prev` the page
/// tree is cut off just the same.
pub fn hybrid_update_without_prev(linked: bool) -> Vec<u8> {
    let (mut out, eight) = unlinked_original();
    let (catalog, first) = unlinked_rewrites(&mut out);
    // The update's cross-reference stream, locating only itself.
    let nine = out.len();
    let mut rows = Vec::new();
    xref_row(&mut rows, 1, nine, 0);
    out.extend_from_slice(format!("9 0 obj\n<< /Type /XRef /Size 11 /Index [9 1] /W [1 4 2] /Length {} >>\nstream\n", rows.len()).as_bytes());
    out.extend_from_slice(&rows);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    let table = out.len();
    out.extend_from_slice(format!("xref\n1 1\n{catalog:010} 00000 n \n3 1\n{first:010} 00000 n \n").as_bytes());
    let prev = if linked { format!(" /Prev {eight}") } else { String::new() };
    out.extend_from_slice(format!("trailer\n<< /Size 11 /Root 1 0 R /XRefStm {nine}{prev} >>\nstartxref\n{table}\n%%EOF\n").as_bytes());
    out
}

/// One row of a cross-reference stream with `/W [1 4 2]`.
fn xref_row(out: &mut Vec<u8>, kind: u8, field: usize, index: u16) {
    out.push(kind);
    out.extend_from_slice(&u32::try_from(field).unwrap_or(u32::MAX).to_be_bytes());
    out.extend_from_slice(&index.to_be_bytes());
}

/// The update both fixtures append: the catalog and the first page again. Their offsets.
fn unlinked_rewrites(out: &mut Vec<u8>) -> (usize, usize) {
    let catalog = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Lang (en) >>\nendobj\n");
    let first = out.len();
    out.extend_from_slice(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Annots [] >>\nendobj\n");
    (catalog, first)
}

/// The first revision of [`update_without_prev`], and the offset of its cross-reference stream.
fn unlinked_original() -> (Vec<u8>, usize) {
    let row = xref_row;
    let page = |width: u32, extra: &str| format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} 200] /Contents 4 0 R{extra} >>");
    let mut out = b"%PDF-1.7\n".to_vec();
    let one = out.len();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let four = out.len();
    out.extend_from_slice(b"4 0 obj\n<< /Length 14 >>\nstream\n0 0 50 50 re f\nendstream\nendobj\n");
    // Objects 2 (the page tree), 3, 6 and 7 (the pages) and 10 (the link) in object stream 5.
    let members = [
        (2, "<< /Type /Pages /Kids [3 0 R 6 0 R 7 0 R] /Count 3 >>".to_string()),
        (3, page(200, "")),
        (6, page(300, " /Annots [10 0 R]")),
        (7, page(400, "")),
        (10, "<< /Type /Annot /Subtype /Link /Rect [10 10 60 60] /A << /S /URI /URI (https://example.com/) >> >>".to_string()),
    ];
    let (mut header, mut body) = (String::new(), String::new());
    for (num, member) in &members {
        header.push_str(&format!("{num} {} ", body.len()));
        body.push_str(member);
        body.push(' ');
    }
    let five = out.len();
    out.extend_from_slice(
        format!(
            "5 0 obj\n<< /Type /ObjStm /N 5 /First {} /Length {} >>\nstream\n{header}{body}\nendstream\nendobj\n",
            header.len(),
            header.len() + body.len()
        )
        .as_bytes(),
    );
    let eight = out.len();
    let mut rows = Vec::new();
    row(&mut rows, 0, 0, 65535);
    row(&mut rows, 1, one, 0);
    row(&mut rows, 2, 5, 0);
    row(&mut rows, 2, 5, 1);
    row(&mut rows, 1, four, 0);
    row(&mut rows, 1, five, 0);
    row(&mut rows, 2, 5, 2);
    row(&mut rows, 2, 5, 3);
    row(&mut rows, 1, eight, 0);
    row(&mut rows, 0, 0, 0);
    row(&mut rows, 2, 5, 4);
    out.extend_from_slice(format!("8 0 obj\n<< /Type /XRef /Size 11 /Root 1 0 R /W [1 4 2] /Length {} >>\nstream\n", rows.len()).as_bytes());
    out.extend_from_slice(&rows);
    out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{eight}\n%%EOF\n").as_bytes());
    (out, eight)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The serialiser the workspace had 111 copies of, kept here verbatim as the thing [`Pdf`] has
    /// to agree with. Migrating a crate to the builder must not change a single byte of its
    /// fixtures, or the test that fixture feeds silently starts testing something else.
    fn hand_rolled(version: &str, objs: &[Vec<u8>], trailer_extra: &str) -> Vec<u8> {
        let mut out = format!("%PDF-{version}\n").into_bytes();
        let mut offs = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offs.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let x = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offs {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R{trailer_extra} >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    fn sample_objects() -> Vec<Vec<u8>> {
        vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>".to_vec(),
            stream_obj("", b"BT /F1 24 Tf 20 150 Td (Page 1) Tj ET"),
        ]
    }

    #[test]
    fn the_builder_is_byte_identical_to_the_serialiser_it_replaces() {
        let objs = sample_objects();
        for version in ["1.4", "1.7", "2.0"] {
            let built = Pdf::new().version(version).objs(objs.clone()).build();
            assert_eq!(built, hand_rolled(version, &objs, ""), "version {version}");
        }
        // ...including the trailer entries the copies varied on.
        let built = Pdf::new().objs(objs.clone()).trailer_entry("/Info 5 0 R").build();
        assert_eq!(built, hand_rolled("1.7", &objs, " /Info 5 0 R"));
    }

    /// Offsets must point at the `N 0 obj` that starts each object, counted from the file's first
    /// byte. A fixture whose table is wrong tests the repair path, not the path it meant to.
    #[test]
    fn every_offset_in_the_table_points_at_its_object() {
        let bytes = Pdf::new().objs(sample_objects()).build();
        let text = String::from_utf8_lossy(&bytes);
        let table = text.split("xref\n0 ").nth(1).expect("an xref table");
        let rows: Vec<&str> = table.lines().skip(1).take_while(|l| l.ends_with(" n ") || l.ends_with(" f ")).collect();
        assert_eq!(rows.len(), 5, "free entry plus four objects: {rows:?}");
        for (i, row) in rows.iter().enumerate().skip(1) {
            let offset: usize = row.split_whitespace().next().and_then(|n| n.parse().ok()).expect("an offset");
            let expected = format!("{i} 0 obj\n");
            assert!(bytes.get(offset..).is_some_and(|rest| rest.starts_with(expected.as_bytes())), "object {i} at {offset}");
        }
    }

    /// `/Size` is one more than the object count (object 0 is the free head), and `startxref`
    /// points at the table.
    #[test]
    fn the_trailer_agrees_with_the_body() {
        let bytes = Pdf::new().objs(sample_objects()).build();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Size 5"), "{text}");
        let startxref: usize = text.rsplit("startxref\n").next().and_then(|t| t.lines().next()).and_then(|n| n.parse().ok()).expect("startxref");
        assert!(bytes.get(startxref..).is_some_and(|rest| rest.starts_with(b"xref\n")), "startxref {startxref}");
    }

    #[test]
    fn stream_length_counts_the_data_not_the_keywords() {
        let obj = stream_obj("/Filter /FlateDecode", b"abc");
        assert_eq!(obj, b"<< /Filter /FlateDecode /Length 3 >>\nstream\nabc\nendstream".to_vec());
        // Binary data, including NUL and bytes that are not UTF-8, survives untouched.
        let data = [0u8, 0xFF, b'\n', 0x80];
        let obj = stream_obj("", data);
        assert!(obj.ends_with(b"stream\n\0\xff\n\x80\nendstream"), "{obj:?}");
        // An empty dict gives one space, not two.
        assert!(obj.starts_with(b"<< /Length 4 >>"), "{obj:?}");
        assert_eq!(stream_obj("  /Type /XObject  ", b""), stream_obj("/Type /XObject", b""));
    }

    /// An empty file is still a parseable shape rather than something that panics or truncates.
    #[test]
    fn a_file_with_no_objects_is_still_well_formed() {
        let bytes = Pdf::new().build();
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF-1.7\n"), "{text}");
        assert!(text.contains("xref\n0 1\n0000000000 65535 f \n"), "{text}");
        assert!(text.contains("/Size 1 /Root 1 0 R"), "{text}");
        assert!(text.ends_with("%%EOF\n"), "{text}");
    }

    #[test]
    fn next_obj_tracks_what_has_been_added() {
        let p = Pdf::new();
        assert_eq!(p.next_obj(), 1);
        let p = p.obj("<< >>").obj("<< >>");
        assert_eq!(p.next_obj(), 3);
        // Forward references are the reason this exists: the page can name a stream not yet added.
        let n = p.next_obj();
        let bytes = p.obj(format!("<< /Contents {n} 0 R >>")).build();
        assert!(String::from_utf8_lossy(&bytes).contains("/Contents 3 0 R"));
    }

    /// Hostile and awkward object bodies must not panic or corrupt the offsets.
    #[test]
    fn odd_object_bodies_keep_the_table_honest() {
        let bodies: Vec<Vec<u8>> = vec![Vec::new(), b"\n".to_vec(), vec![0u8; 1024], "<< /T (\u{1F4A5}) >>".as_bytes().to_vec(), b"endobj".to_vec()];
        let bytes = Pdf::new().objs(bodies.clone()).build();
        assert_eq!(bytes, hand_rolled("1.7", &bodies, ""));
        // The offsets still land on their objects even when a body contains `endobj` itself.
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("/Size 6"), "{text}");
    }
}
