//! Feeds arbitrary bytes to every decoder, with parameters taken from the first bytes.
#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfcraft_filters::{Filter, Params, decode, decode_tolerant};

const FILTERS: [Filter; 5] = [Filter::Flate, Filter::Lzw, Filter::AsciiHex, Filter::Ascii85, Filter::RunLength];
const PREDICTORS: [i64; 8] = [1, 2, 10, 11, 12, 13, 14, 15];
const BPC: [i64; 5] = [1, 2, 4, 8, 16];

fuzz_target!(|data: &[u8]| {
    let Some((head, body)) = data.split_first_chunk::<4>() else { return };
    // Up to two chained filters; the second only when head[0] says so.
    let params = Params {
        predictor: PREDICTORS[usize::from(head[1] & 7)],
        colors: i64::from(head[1] >> 3 & 3) + 1,
        bits_per_component: BPC[usize::from(head[2]) % BPC.len()],
        columns: i64::from(head[3]) + 1,
        early_change: i64::from(head[2] >> 7),
    };
    let mut chain = vec![(FILTERS[usize::from(head[0] & 7) % FILTERS.len()].clone(), params.clone())];
    if head[0] & 0x80 != 0 {
        chain.insert(0, (FILTERS[usize::from(head[0] >> 3 & 7) % FILTERS.len()].clone(), Params::default()));
    }
    let max = 1 << 22;
    if let Ok(out) = decode(&chain, body, max) {
        assert!(out.len() <= max);
    }
    if let Ok((out, _)) = decode_tolerant(&chain, body, max) {
        assert!(out.len() <= max);
    }
});
