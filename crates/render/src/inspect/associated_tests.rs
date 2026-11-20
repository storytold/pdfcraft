//! PDF 2.0 associated files (`/AF`, ISO 32000-2 §14.13): listed from the catalog, pages and
//! annotations, deduplicated by object, malformed entries skipped with a note, and extracted.

use super::associated::{MAX_ASSOCIATED_FILE_NOTES, MAX_ASSOCIATED_FILES};
use super::{Arc, AssociatedFileOwner as Owner, AssociatedFileRelationship as Rel, associated_file_data, inspect};

/// A document whose catalog, page and comment each list associated files: one shared by the
/// catalog and the page (by object), one written directly in the comment's `/AF`, one with a
/// relationship the standard doesn't define and one with none at all.
const ASSOCIATED: &[u8] = b"%PDF-2.0
1 0 obj << /Type /Catalog /Pages 2 0 R /AF [10 0 R] >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /AF [11 0 R 10 0 R] /Annots [4 0 R] >> endobj
4 0 obj << /Type /Annot /Subtype /Square /Rect [10 10 50 50] /AF [<< /Type /Filespec /F (note.txt) /AFRelationship /Supplement /EF << /F 22 0 R >> >> 12 0 R] >> endobj
10 0 obj << /Type /Filespec /F (invoice-old.xml) /UF <FEFF0069006E0076006F006900630065002E0078006D006C> /Desc (Invoice data) /AFRelationship /Data /EF << /F 20 0 R /UF 20 0 R >> >> endobj
11 0 obj << /Type /Filespec /F (page.svg) /AFRelationship /Source /EF << /F 21 0 R >> >> endobj
12 0 obj << /Type /Filespec /F (manifest.c2pa) /AFRelationship /C2PA_Manifest /EF << /F 23 0 R >> >> endobj
20 0 obj << /Type /EmbeddedFile /Subtype /text#2Fxml /Params << /Size 9 >> /Length 9 >> stream
<invoice>
endstream endobj
21 0 obj << /Type /EmbeddedFile /Subtype /image#2Fsvg+xml /Filter /ASCIIHexDecode /Length 11 >> stream
3C7376672F3E>
endstream endobj
22 0 obj << /Type /EmbeddedFile /Params << /Size -4 >> /Length 4 >> stream
note
endstream endobj
23 0 obj << /Type /EmbeddedFile /Length 4 >> stream
c2pa
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";

#[test]
fn associated_files_of_the_document_pages_and_annotations_are_listed_and_extracted() {
    let bytes = Arc::new(ASSOCIATED.to_vec());
    let info = inspect(bytes.clone(), None).expect("opens");
    assert!(info.associated_file_notes.is_empty(), "{:?}", info.associated_file_notes);
    let listed: Vec<_> = info
        .associated_files
        .iter()
        .map(|f| (f.name.as_deref(), f.relationship.name(), f.owners.clone(), f.index, f.mime.as_deref(), f.size))
        .collect();
    assert_eq!(
        listed,
        [
            // /UF before /F; the page lists the catalog's file too, by the same object.
            (Some("invoice.xml"), "Data", vec![Owner::Document, Owner::Page { page: 0 }], 0, Some("text/xml"), Some(9)),
            (Some("page.svg"), "Source", vec![Owner::Page { page: 0 }], 0, Some("image/svg+xml"), None),
            // A negative /Size gives no size.
            (Some("note.txt"), "Supplement", vec![Owner::Annotation { page: 0, index: 0 }], 0, None, None),
            (Some("manifest.c2pa"), "C2PA_Manifest", vec![Owner::Annotation { page: 0, index: 0 }], 1, None, None),
        ]
    );
    let first = &info.associated_files[0];
    assert_eq!(first.description.as_deref(), Some("Invoice data"));
    assert_eq!(first.relationship, Rel::Data);
    assert!(first.relationship.is_standard());
    assert_eq!(info.associated_files[3].relationship, Rel::Other("C2PA_Manifest".into()));
    assert!(!info.associated_files[3].relationship.is_standard());

    let data: Vec<Vec<u8>> = info.associated_files.iter().map(|f| associated_file_data(&bytes, None, f).expect("extracts")).collect();
    assert_eq!(data, [b"<invoice>".to_vec(), b"<svg/>".to_vec(), b"note".to_vec(), b"c2pa".to_vec()]);
}

/// Every kind of malformed entry is skipped with a note saying why, and reading never loops: an
/// object that is a reference to itself, an `/AF` array that lists itself, and a reference to an
/// object that doesn't exist.
#[test]
fn malformed_and_cyclic_associated_files_are_skipped_with_a_reason() {
    let pdf = b"%PDF-2.0
1 0 obj << /Type /Catalog /Pages 2 0 R /AF 9 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /AF (not an array) >> endobj
4 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /AF 9 0 R /Annots [5 0 R] >> endobj
5 0 obj << /Type /Annot /Subtype /Text /Rect [0 0 10 10] /AF [13 0 R 14 0 R] >> endobj
9 0 obj [42 9 0 R 10 0 R 11 0 R 12 0 R 13 0 R 14 0 R 99 0 R 15 0 R 16 0 R] endobj
10 0 obj << /Type /Filespec /F (external.txt) >> endobj
11 0 obj << /Type /Filespec /F (bad-ef.txt) /EF (nope) >> endobj
12 0 obj << /Type /Filespec /F (no-stream.txt) /EF << /F 20 0 R >> >> endobj
13 0 obj 13 0 R endobj
14 0 obj << /Type /Filespec /F (string.txt) /AFRelationship (Data) /EF << /F 21 0 R >> >> endobj
15 0 obj << /Type /Filespec /F (undecodable.bin) /EF << /F 22 0 R >> >> endobj
16 0 obj null endobj
20 0 obj << /Type /EmbeddedFile >> endobj
21 0 obj << /Type /EmbeddedFile /Length 2 >> stream
ok
endstream endobj
22 0 obj << /Type /EmbeddedFile /Filter /NoSuchDecode /Length 4 >> stream
data
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";
    let bytes = Arc::new(pdf.to_vec());
    let info = inspect(bytes.clone(), None).expect("opens");
    let names: Vec<_> = info.associated_files.iter().map(|f| (f.name.as_deref(), f.relationship.name(), f.owners.clone())).collect();
    assert_eq!(
        names,
        [
            // The catalog's array, shared by page 2, lists the two that are files.
            (Some("string.txt"), "Unspecified", vec![Owner::Document, Owner::Page { page: 1 }, Owner::Annotation { page: 1, index: 0 }]),
            (Some("undecodable.bin"), "Unspecified", vec![Owner::Document, Owner::Page { page: 1 }]),
        ]
    );
    let notes = &info.associated_file_notes;
    for expected in [
        "The document, /AF entry 1: it is not a file specification dictionary, so it was skipped.",
        "The document, /AF entry 2: it is not a file specification dictionary, so it was skipped.",
        "The document, /AF entry 3: it has no embedded file (/EF), so it was skipped.",
        "The document, /AF entry 4: its /EF is not a dictionary, so it was skipped.",
        "The document, /AF entry 5: its /EF holds no embedded file stream, so it was skipped.",
        "The document, /AF entry 6: its reference leads to no object, so it was skipped.",
        "The document, /AF entry 7: /AFRelationship is not a name, so it was read as Unspecified.",
        "The document, /AF entry 8: its reference leads to no object, so it was skipped.",
        "The document, /AF entry 10: it is not a file specification dictionary, so it was skipped.",
        "Page 1: /AF is not an array, so it was skipped.",
    ] {
        assert!(notes.iter().any(|n| n == expected), "{expected:?} not in {notes:#?}");
    }
    // A skipped entry the page and the comment share is noted once, where it was first read.
    assert_eq!(notes.len(), 10, "{notes:#?}");

    assert_eq!(associated_file_data(&bytes, None, &info.associated_files[0]).as_deref(), Ok(&b"ok"[..]));
    // A stream that can't be decoded is listed; reading it says why it can't be.
    let undecodable = associated_file_data(&bytes, None, &info.associated_files[1]).unwrap_err();
    assert!(undecodable.starts_with("the associated file could not be decoded"), "{undecodable}");
    // A file the document doesn't list where it says is an error, not a crash.
    let mut moved = info.associated_files[0].clone();
    moved.owners = vec![Owner::Annotation { page: 7, index: 3 }];
    assert!(associated_file_data(&bytes, None, &moved).is_err());
    moved.owners.clear();
    assert!(associated_file_data(&bytes, None, &moved).is_err());
}

/// The list and its notes are capped: a hostile `/AF` lists at most `MAX_ASSOCIATED_FILES`, and
/// says so.
#[test]
fn the_associated_file_list_and_its_notes_are_capped() {
    let entries: String = (0..MAX_ASSOCIATED_FILES + 10).map(|i| format!("<< /F (f{i}) /EF << /F 4 0 R >> >> ")).collect();
    let skipped: String = (0..MAX_ASSOCIATED_FILE_NOTES + 5).map(|_| "1 ").collect();
    let pdf = format!(
        "%PDF-2.0
1 0 obj << /Type /Catalog /Pages 2 0 R /AF [{skipped}{entries}] >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >> endobj
4 0 obj << /Type /EmbeddedFile /Length 1 >> stream
x
endstream endobj
trailer << /Root 1 0 R >>
%%EOF"
    );
    let info = inspect(Arc::new(pdf.into_bytes()), None).expect("opens");
    assert_eq!(info.associated_files.len(), MAX_ASSOCIATED_FILES);
    let notes = &info.associated_file_notes;
    assert_eq!(notes.len(), MAX_ASSOCIATED_FILE_NOTES + 2, "{notes:#?}");
    assert_eq!(notes[MAX_ASSOCIATED_FILE_NOTES], "5 more notes about associated files are not shown.");
    // The cap's own note is kept even when the others are full.
    assert_eq!(notes.last().map(String::as_str), Some(format!("Only the first {MAX_ASSOCIATED_FILES} associated files are listed.").as_str()));
}
