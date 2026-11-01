//! The bold Arabic instance may be the first Arabic text a process shapes (a bold heading
//! edited before anything else). Its own test binary, so no other test has set up the face.

use std::sync::mpsc;
use std::time::Duration;

#[test]
fn bold_arabic_can_be_the_first_arabic_a_process_shapes() {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        // Nothing else in this process has touched the Arabic face yet.
        let shaped = pdfcraft_fonts::shape_arabic_bold("سلام", true).map(|clusters| clusters.len());
        let _ = done.send(shaped);
    });
    let shaped = finished.recv_timeout(Duration::from_secs(30)).expect("shaping bold Arabic first must not hang");
    match pdfcraft_fonts::document_arabic_font() {
        Some(_) if pdfcraft_fonts::arabic_bold_available() => assert_eq!(shaped, Ok(4)),
        _ => assert!(shaped.is_err()),
    }
}
