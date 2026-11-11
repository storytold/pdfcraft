use super::{
    sweep::{add_needle, bytes_counts, bytes_hits},
    util::Ctx,
};
use std::collections::HashSet;

#[test]
fn a_thousand_needles_share_one_multimegabyte_scan() {
    let mut needles = Vec::new();
    for i in 0..1000 {
        add_needle(&mut needles, false, format!("SECRET_{i:04}"), None);
    }
    let hay = vec![b'z'; 8 << 20];
    assert_eq!(bytes_hits(&needles, &hay, 6, &Ctx::new(HashSet::new())), Some(Vec::new()));
    assert_eq!(bytes_counts(&needles, &hay, 6, &Ctx::new(HashSet::new())), Some(Vec::new()));
}

#[test]
fn overlapping_encodings_count_one_occurrence() {
    let mut needles = Vec::new();
    add_needle(&mut needles, false, "SECRET42".into(), None);
    let text: Vec<u8> = "SECRET42".encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut hay = vec![0xfe, 0xff];
    hay.extend(text);
    assert_eq!(bytes_counts(&needles, &hay, 6, &Ctx::new(HashSet::new())), Some(vec![(0, 1)]));
}

#[test]
fn match_count_exhaustion_is_incomplete_not_a_truncated_pass() {
    let mut needles = Vec::new();
    add_needle(&mut needles, true, "SECRET42".into(), None);
    let hay = b"SECRET42 ".repeat((1 << 16) + 1);
    assert!(bytes_counts(&needles, &hay, 6, &Ctx::new(HashSet::new())).is_none());
}
