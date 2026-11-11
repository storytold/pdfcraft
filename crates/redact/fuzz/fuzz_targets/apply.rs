//! Feeds arbitrary bytes, read as a PDF, to the redaction marks and the apply entry.
//!
//! Results are discarded: the target checks that no input panics.
#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use pdfcraft_cos::Document;
use pdfcraft_redact::{apply, marks};

/// The largest input the target opens (16 MiB); larger inputs are skipped.
const MAX_INPUT: usize = 16 << 20;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT {
        return;
    }
    let Ok(mut doc) = Document::open(Arc::new(data.to_vec())) else {
        return;
    };
    let _count = marks(&doc).len();
    let _result = apply(&mut doc, None);
});
