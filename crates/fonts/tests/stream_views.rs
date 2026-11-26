//! Memory regression for the font reads charged to an allowance: a stream whose `/DecodeParms`
//! is given by reference is read through a view that shares its data and copies no more of its
//! dictionary than its filters and parameters, not through a copy of the whole stream. A test
//! binary of its own, so that the one test is all the process runs while its peak is measured.

use pdfcraft_cos::{DecodeBudget, Dict, Document, ObjRef, Object, Stream};
use pdfcraft_fonts::pdf::{Metrics, embedded_sfnt_within};

/// How much the process's peak resident memory grows, in KiB, while `f` runs. `None` where the
/// peak can't be reset (`/proc/self/clear_refs` is Linux's).
fn peak_growth<T>(f: impl FnOnce() -> T) -> Option<(T, u64)> {
    fn peak_kib() -> Option<u64> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        status.lines().find_map(|l| l.strip_prefix("VmHWM:"))?.split_whitespace().next()?.parse().ok()
    }
    std::fs::write("/proc/self/clear_refs", "5").ok()?;
    let before = peak_kib()?;
    let out = f();
    Some((out, peak_kib()?.saturating_sub(before)))
}

/// A stream of `data`, its `/DecodeParms` the reference `parms`, its dictionary holding `junk`.
fn stream(doc: &mut Document, data: &[u8], parms: ObjRef, junk: &Object) -> Object {
    let mut s = Stream::flate(Dict::new(), data);
    s.dict.set(b"DecodeParms".to_vec(), Object::Ref(parms));
    s.dict.set(b"Junk".to_vec(), junk.clone());
    Object::Ref(doc.add(Object::Stream(s)))
}

/// A font whose program and ToUnicode each hold an unrelated array of a million numbers in their
/// dictionaries, with `/DecodeParms` given by reference, read eight times within an allowance by
/// the editor's program read and by the metrics read: neither copies an array. A read through a
/// copy of the stream would add about as much as one array holds.
#[test]
fn reading_a_fonts_streams_copies_no_dictionary() {
    let junk = Object::Array(vec![Object::Int(0); 1 << 20]);
    let array_kib = (std::mem::size_of::<Object>() << 20) as u64 >> 10;
    let mut doc = Document::new_empty();
    let parms = doc.add(Object::Dict(Dict::from_iter([(b"Predictor".to_vec(), Object::Int(1))])));
    let program = stream(&mut doc, include_bytes!("../../../assets/fonts/Inter-Regular.ttf"), parms, &junk);
    let unicode = stream(&mut doc, b"1 beginbfchar <41> <0058> endbfchar", parms, &junk);
    drop(junk);
    let mut descriptor = Dict::new();
    descriptor.set(b"FontFile2".to_vec(), program);
    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("TrueType"));
    font.set(b"FontDescriptor".to_vec(), Object::Dict(descriptor));
    font.set(b"ToUnicode".to_vec(), unicode);
    let Some(((), growth)) = peak_growth(|| {
        for _ in 0..8 {
            let mut budget = DecodeBudget::new(64 << 20, 0);
            assert!(embedded_sfnt_within(&doc, &font, &mut budget).is_some());
            let m = Metrics::from_dict_within(&doc, &font, &mut budget);
            assert_eq!(m.text_of(0x41).as_deref(), Some("X"));
        }
    }) else {
        eprintln!("the peak can't be measured here");
        return;
    };
    assert!(growth < array_kib / 2, "the peak grew {growth} KiB, an array is {array_kib} KiB");
}
