//! A file name is limited in bytes (UTF-8 on ext4 and APFS) and UTF-16 units (NTFS), not in
//! characters, so a long name of wide characters has to be refused before the rename fails.

use std::path::Path;

use pdfcraft_engine::{RenameError, new_file_name};

fn name(typed: &str) -> Result<String, RenameError> {
    new_file_name(Path::new("/docs/report.pdf"), typed)
}

#[test]
fn a_name_over_255_bytes_is_refused() {
    // 94 characters of 3 bytes each: 282 bytes, then ".pdf" makes 286. Under 255 characters.
    let cjk = "年".repeat(94);
    assert!(cjk.chars().count() < 255);
    assert!(matches!(name(&cjk), Err(RenameError::Invalid(_))), "282 bytes of CJK must be refused");

    // 251 ASCII characters + ".pdf" is exactly 255 bytes: accepted.
    let exact = "x".repeat(251);
    assert_eq!(name(&exact), Ok(format!("{exact}.pdf")));

    // One more byte is refused.
    assert!(matches!(name(&"x".repeat(252)), Err(RenameError::Invalid(_))));

    // 80 CJK characters + ".pdf" is 244 bytes: accepted.
    let short = "年".repeat(80);
    assert_eq!(name(&short), Ok(format!("{short}.pdf")));

    // Characters outside the BMP take 2 UTF-16 units and 4 bytes: 63 + ".pdf" is 256 bytes.
    assert!(matches!(name(&"😀".repeat(63)), Err(RenameError::Invalid(_))));
}
