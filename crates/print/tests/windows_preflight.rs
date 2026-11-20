//! Windows submission preflight regressions. These tests never start a print job.
#![cfg(windows)]

use pdfcraft_print::{
    PrintError, in_english,
    spool::{self, Job, Scaling},
};

// Text-only, original synthetic objects: no fonts, images, or stored PDF fixtures.
fn pdf(page_sizes: &[(u32, u32)]) -> Vec<u8> {
    let mut objects = vec!["<< /Type /Catalog /Pages 2 0 R >>".to_string()];
    let kids: Vec<_> = (0..page_sizes.len()).map(|i| format!("{} 0 R", i + 3)).collect();
    objects.push(format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), page_sizes.len()));
    for (w, h) in page_sizes {
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] >>"));
    }
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).as_bytes());
    bytes
}

fn refused_before_device_lookup(bytes: &[u8], expected: &str) {
    let original = bytes.to_vec();
    // Win32 printer names cannot contain an embedded NUL. Even if a future
    // preflight regresses, select_device's exact-name match cannot submit a job.
    let job = Job { printer: Some("p1-synthetic\0no-device-lookup".into()), scaling: Scaling::Exact, ..Job::default() };
    let mut progress = Vec::new();
    // Pinned craft-print windows.rs reads and validates these failing inputs before
    // PrinterDevice::all, driver_ticket, temporary files, or DXGI context creation.
    let error = spool::submit_with_progress(bytes, &job, &mut |sent, total| progress.push((sent, total))).unwrap_err();
    assert!(progress.is_empty());
    assert!(matches!(&error, PrintError::Spool(message) if message.contains(expected)), "{error}");
    assert_eq!(error.said(&in_english), error.to_string());
    assert_eq!(spool::submit(bytes, &job).unwrap_err(), error);
    assert_eq!(bytes, original);
    // The facade preserves the shared library's error text and classification.
    assert_eq!(error.to_string(), craft_print::submit(bytes, &job).unwrap_err().to_string());
}

#[test]
fn malformed_bytes_fail_without_any_sheet_progress() {
    for bytes in [b"".as_slice(), b"%PDF-1.7\n%%EOF", b"not a PDF"] {
        refused_before_device_lookup(bytes, "no cross-reference table");
    }
}

#[test]
fn empty_page_tree_fails_before_printer_discovery() {
    refused_before_device_lookup(&pdf(&[]), "has no pages");
}

#[test]
fn mixed_exact_sheet_sizes_fail_before_printer_discovery() {
    refused_before_device_lookup(&pdf(&[(612, 792), (420, 595)]), "Windows prints one paper size per job");
}
