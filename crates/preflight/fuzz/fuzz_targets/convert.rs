//! Feeds arbitrary bytes to `convert` at level A2b; the report it returns is discarded.
#![no_main]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use pdfcraft_cos::Document;
use pdfcraft_preflight::{Level, convert};

/// Largest input the target parses; a longer input is dropped before `Document::open` copies it.
const MAX_INPUT: usize = 16 << 20;

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT {
        return;
    }
    let Ok(mut doc) = Document::open(Arc::new(data.to_vec())) else {
        return;
    };
    let _ = convert(&mut doc, Level::A2b);
});
