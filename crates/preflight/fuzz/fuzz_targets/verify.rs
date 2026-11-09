//! Feeds arbitrary bytes to `verify` at both levels; the issue list it returns is discarded.
#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use pdfcraft_cos::Document;
use pdfcraft_preflight::{Level, verify};

/// Largest input the target parses; a longer input is dropped before `Document::open` copies it.
const MAX_INPUT: usize = 16 << 20;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT {
        return;
    }
    let Ok(doc) = Document::open(Arc::new(data.to_vec())) else {
        return;
    };
    let _a2b = verify(&doc, Level::A2b);
    let _a3b = verify(&doc, Level::A3b);
});
