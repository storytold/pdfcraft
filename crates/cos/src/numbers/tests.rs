use std::cell::Cell;

use proptest::prelude::*;

use super::*;
use crate::{Lexer, ObjRef};

thread_local! {
    static RESOLUTIONS: Cell<usize> = const { Cell::new(0) };
    static FAIL_RESERVE: Cell<bool> = const { Cell::new(false) };
}

/// The reader followed one reference (the counting seam: no wall-clock thresholds).
pub(super) fn count_resolution() {
    RESOLUTIONS.with(|c| c.set(c.get().saturating_add(1)));
}

pub(super) fn reservation_fails() -> bool {
    FAIL_RESERVE.with(Cell::get)
}

/// References `f` followed.
fn resolutions<T>(f: impl FnOnce() -> T) -> (T, usize) {
    RESOLUTIONS.with(|c| c.set(0));
    let out = f();
    (out, RESOLUTIONS.with(Cell::get))
}

fn parse(text: &[u8]) -> Object {
    Lexer::new(text, 0).object().unwrap()
}

/// `value` behind a chain of `hops` references.
fn chain(doc: &mut Document, value: Object, hops: usize) -> Object {
    (0..hops).fold(value, |v, _| Object::Ref(doc.add(v)))
}

#[test]
fn scalars_are_finite_integers_and_reals() {
    let doc = Document::new_empty();
    let r = NumberReader::new(&doc);
    assert_eq!(r.finite(&Object::Int(-3)), Ok(-3.0));
    assert_eq!(r.finite(&Object::Real(0.0)), Ok(0.0));
    assert_eq!(r.finite(&Object::Real(-0.5)), Ok(-0.5));
    assert_eq!(r.finite(&Object::Null), Err(NumberError::NotNumber));
    assert_eq!(r.finite(&Object::Bool(true)), Err(NumberError::NotNumber));
    assert_eq!(r.finite(&parse(b"(12)")), Err(NumberError::NotNumber), "a string is never parsed");
    assert_eq!(r.finite(&parse(b"[1]")), Err(NumberError::NotNumber));
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(r.finite(&Object::Real(bad)), Err(NumberError::NonFinite));
    }
    // The parser reads `-nan` as a real NaN.
    assert_eq!(r.finite(&parse(b"-nan")), Err(NumberError::NonFinite));
}

#[test]
fn references_are_followed_to_the_resolve_bound() {
    let mut doc = Document::new_empty();
    // An initial reference plus up to 32 more: 33 lookups reach the value, 34 do not.
    for hops in [1, 2, 32, 33] {
        let v = chain(&mut doc, Object::Int(20), hops);
        assert_eq!(NumberReader::new(&doc).finite(&v), Ok(20.0), "hops={hops}");
    }
    let v = chain(&mut doc, Object::Int(20), 34);
    assert_eq!(NumberReader::new(&doc).finite(&v), Err(NumberError::UnresolvedReference));
    // A self-cycle and a two-object cycle end, unresolved.
    let me = doc.add(Object::Null);
    doc.set(me, Object::Ref(me));
    assert_eq!(NumberReader::new(&doc).finite(&Object::Ref(me)), Err(NumberError::UnresolvedReference));
    let (a, b) = (doc.add(Object::Null), doc.add(Object::Null));
    doc.set(a, Object::Ref(b));
    doc.set(b, Object::Ref(a));
    assert_eq!(NumberReader::new(&doc).finite(&Object::Ref(a)), Err(NumberError::UnresolvedReference));
    // A missing target reads as null.
    let missing = Object::Ref(ObjRef { num: 9_999, generation: 0 });
    assert_eq!(NumberReader::new(&doc).finite(&missing), Err(NumberError::NotNumber));
    // One bounded resolve per value, not one per hop.
    let v = chain(&mut doc, Object::Int(1), 30);
    assert_eq!(resolutions(|| NumberReader::new(&doc).finite(&v)), (Ok(1.0), 1));
}

#[test]
fn the_direct_reader_follows_no_reference() {
    let mut doc = Document::new_empty();
    let r = doc.add(Object::Int(5));
    assert_eq!(NumberReader::direct().finite(&Object::Ref(r)), Err(NumberError::UnresolvedReference));
    assert_eq!(NumberReader::direct().tuple::<1>(&Object::Ref(r)), Err(ArrayError::UnresolvedReference));
    let entries = Object::Array(vec![Object::Int(1), Object::Ref(r)]);
    assert_eq!(NumberReader::direct().tuple::<2>(&entries), Err(ArrayError::InvalidEntry { index: 1, error: NumberError::UnresolvedReference }));
    assert_eq!(NumberReader::direct().tuple::<1>(&parse(b"[2.5]")), Ok([2.5]));
}

#[test]
fn a_tuple_has_exactly_its_length_of_valid_entries() {
    let doc = Document::new_empty();
    let r = NumberReader::new(&doc);
    assert_eq!(r.tuple::<4>(&parse(b"[1 2 3 4]")), Ok([1.0, 2.0, 3.0, 4.0]));
    assert_eq!(r.tuple::<4>(&parse(b"[1 2 3]")), Err(ArrayError::WrongLength { expected: 4, actual: 3 }));
    assert_eq!(r.tuple::<4>(&parse(b"[1 2 3 4 5]")), Err(ArrayError::WrongLength { expected: 4, actual: 5 }), "no prefix");
    assert_eq!(r.tuple::<4>(&parse(b"[]")), Err(ArrayError::WrongLength { expected: 4, actual: 0 }));
    assert_eq!(r.tuple::<0>(&parse(b"[]")), Ok([]));
    assert_eq!(r.tuple::<0>(&parse(b"[1]")), Err(ArrayError::WrongLength { expected: 0, actual: 1 }));
    // A bad entry is never skipped to make up the length.
    assert_eq!(r.tuple::<4>(&parse(b"[null 1 2 3 4]")), Err(ArrayError::WrongLength { expected: 4, actual: 5 }));
    assert_eq!(r.tuple::<4>(&parse(b"[1 null 3 4]")), Err(ArrayError::InvalidEntry { index: 1, error: NumberError::NotNumber }));
    assert_eq!(r.tuple::<2>(&parse(b"[1 -nan]")), Err(ArrayError::InvalidEntry { index: 1, error: NumberError::NonFinite }));
    assert_eq!(r.tuple::<4>(&Object::Int(4)), Err(ArrayError::NotArray));
    // The longest tuple allowed reads; a longer one does not compile (see `MAX_TUPLE_ENTRIES`).
    let sixteen = Object::Array((0..16).map(Object::Int).collect());
    assert_eq!(r.tuple::<MAX_TUPLE_ENTRIES>(&sixteen).map(|t| t[15]), Ok(15.0));
}

#[test]
fn a_tuple_of_the_wrong_length_resolves_no_entry() {
    let mut doc = Document::new_empty();
    let entries: Vec<Object> = (0..5).map(|i| Object::Ref(doc.add(Object::Int(i)))).collect();
    let five = Object::Array(entries.clone());
    assert_eq!(resolutions(|| NumberReader::new(&doc).tuple::<4>(&five)).1, 0);
    // An indirect array costs the one resolution of the array itself.
    let indirect = Object::Ref(doc.add(Object::Array(entries.clone())));
    assert_eq!(resolutions(|| NumberReader::new(&doc).tuple::<4>(&indirect)).1, 1);
    // The right length resolves each entry once.
    let four = Object::Array(entries.into_iter().take(4).collect());
    assert_eq!(resolutions(|| NumberReader::new(&doc).tuple::<4>(&four)), (Ok([0.0, 1.0, 2.0, 3.0]), 4));
}

#[test]
fn a_huge_array_is_rejected_before_its_entries_are_read() {
    let mut doc = Document::new_empty();
    // cap + 1 references: a walk of the entries would count them.
    let target = doc.add(Object::Int(1));
    let huge = Object::Array(vec![Object::Ref(target); MAX_NUMERIC_ENTRIES + 1]);
    let (out, n) = resolutions(|| NumberReader::new(&doc).sequence(&huge, usize::MAX));
    assert_eq!((out, n), (Err(ArrayError::TooLong { len: MAX_NUMERIC_ENTRIES + 1, cap: MAX_NUMERIC_ENTRIES }), 0));
    assert_eq!(resolutions(|| NumberReader::new(&doc).tuple::<4>(&huge)).1, 0);
    let indirect = Object::Ref(doc.add(huge));
    let (out, n) = resolutions(|| NumberReader::new(&doc).sequence(&indirect, usize::MAX));
    assert_eq!((out, n), (Err(ArrayError::TooLong { len: MAX_NUMERIC_ENTRIES + 1, cap: MAX_NUMERIC_ENTRIES }), 1));
}

#[test]
fn a_sequence_keeps_every_slot_in_place() {
    let mut doc = Document::new_empty();
    let five = doc.add(Object::Int(5));
    let seven = chain(&mut doc, Object::Real(7.5), 3);
    let a = Object::Array(vec![Object::Null, Object::Ref(five), Object::Int(1), Object::Real(f64::NAN), seven, Object::Int(2)]);
    let s = NumberReader::new(&doc).sequence(&a, 64).unwrap();
    assert_eq!(
        s.entries,
        vec![Err(NumberError::NotNumber), Ok(5.0), Ok(1.0), Err(NumberError::NonFinite), Ok(7.5), Ok(2.0)],
        "a null at 0 stays at 0 and no later slot moves"
    );
    assert_eq!(NumberReader::new(&doc).sequence(&parse(b"[]"), 64), Ok(NumberSequence { entries: Vec::new() }));
    assert_eq!(NumberReader::new(&doc).sequence(&Object::Null, 64), Err(ArrayError::NotArray));
}

#[test]
fn a_sequence_is_capped_without_a_prefix() {
    let doc = Document::new_empty();
    let r = NumberReader::new(&doc);
    let eight = parse(b"[1 2 3 4 5 6 7 8]");
    assert_eq!(r.sequence(&eight, 8).map(|s| s.entries.len()), Ok(8));
    assert_eq!(r.sequence(&eight, 7), Err(ArrayError::TooLong { len: 8, cap: 7 }));
    // A cap above the global one is the global one.
    let at = Object::Array(vec![Object::Int(0); MAX_NUMERIC_ENTRIES]);
    assert_eq!(r.sequence(&at, usize::MAX).map(|s| s.entries.len()), Ok(MAX_NUMERIC_ENTRIES));
    let over = Object::Array(vec![Object::Int(0); MAX_NUMERIC_ENTRIES + 1]);
    assert_eq!(r.sequence(&over, usize::MAX), Err(ArrayError::TooLong { len: MAX_NUMERIC_ENTRIES + 1, cap: MAX_NUMERIC_ENTRIES }));
}

#[test]
fn a_failed_reservation_is_an_error() {
    let doc = Document::new_empty();
    FAIL_RESERVE.with(|f| f.set(true));
    let out = NumberReader::new(&doc).sequence(&parse(b"[1 2]"), 8);
    FAIL_RESERVE.with(|f| f.set(false));
    assert_eq!(out, Err(ArrayError::Allocation));
}

/// A slot: a finite number, or something that is not one.
fn slot() -> impl Strategy<Value = Object> {
    prop_oneof![
        (-1e6f64..1e6).prop_map(Object::Real),
        any::<i32>().prop_map(|i| Object::Int(i.into())),
        Just(Object::Null),
        Just(Object::Real(f64::NAN)),
        Just(Object::Real(f64::INFINITY)),
        Just(Object::Bool(false)),
    ]
}

proptest! {
    #[test]
    fn a_value_behind_a_chain_reads_as_itself(v in slot(), hops in 0usize..=33) {
        let mut doc = Document::new_empty();
        let behind = chain(&mut doc, v.clone(), hops);
        prop_assert_eq!(NumberReader::new(&doc).finite(&behind), NumberReader::new(&doc).finite(&v));
    }

    #[test]
    fn an_invalid_slot_never_moves_another(entries in proptest::collection::vec(slot(), 0..24), at in 0usize..24) {
        let doc = Document::new_empty();
        let at = at.min(entries.len());
        let mut with = entries.clone();
        with.insert(at, Object::Null);
        let before = NumberReader::new(&doc).sequence(&Object::Array(entries), 64).unwrap().entries;
        let after = NumberReader::new(&doc).sequence(&Object::Array(with), 64).unwrap().entries;
        prop_assert_eq!(after.len(), before.len() + 1);
        prop_assert_eq!(after.get(at), Some(&Err(NumberError::NotNumber)));
        prop_assert_eq!(&after[..at], &before[..at]);
        prop_assert_eq!(&after[at + 1..], &before[at..]);
    }

    #[test]
    fn a_tuple_succeeds_iff_its_length_and_entries_are_valid(entries in proptest::collection::vec(slot(), 0..7)) {
        let doc = Document::new_empty();
        let r = NumberReader::new(&doc);
        let all_valid = entries.iter().all(|e| r.finite(e).is_ok());
        let ok = r.tuple::<4>(&Object::Array(entries.clone())).is_ok();
        prop_assert_eq!(ok, entries.len() == 4 && all_valid);
    }
}
