//! A lone `+` or `++` is accepted by `hayro-syntax`'s first pass over a dictionary or array,
//! which skips it as a number, but no number is read from it. Such an entry is unreadable, like
//! any other malformed value: a dictionary leaves it out and keeps its other entries, and an
//! array reads it as null in its slot (vendor patch (46)). Before, `Dict::entries` and
//! `Array::raw_iter` (and `Dict`'s `Debug`) unwrapped the failed read and panicked, and `Array::iter` ended at the
//! token, dropping the items after it. Sign runs a number is read from (`-` reads as 0) keep
//! reading as before.

use hayro_syntax::object::{Array, Dict, FromBytes, Number, Object};

/// Sign-only tokens, and two of them in a row.
const SIGN_TOKENS: [&str; 8] = ["+", "++", "-", "--", "+-", "-+", "+ -", "+ +"];

/// Whether no number is read from this sign run, so it is the unreadable kind.
fn unreadable(sign: &str) -> bool {
    Number::from_bytes(sign.as_bytes()).is_none()
}

#[test]
fn a_dictionary_leaves_out_an_entry_made_only_of_signs() {
    for tok in SIGN_TOKENS {
        let src = format!("<< /A {tok} /B 7 >>");
        let Some(dict) = Dict::from_bytes(src.as_bytes()) else {
            // Refused as a whole, as other unreadable dictionaries are: nothing to iterate.
            continue;
        };
        let entries: Vec<_> = dict.entries().map(|(k, _)| k.as_str().to_owned()).collect();
        let a = dict.get::<Object<'_>>("A");
        if unreadable(tok) {
            assert_eq!(entries, ["B"], "{src}");
            assert!(a.is_none(), "{src}");
        } else {
            assert_eq!(entries, ["A", "B"], "{src}");
            assert!(matches!(a, Some(Object::Number(_))), "{src}: {a:?}");
        }
        assert_eq!(dict.get::<i32>("B"), Some(7), "{src}");
    }
}

#[test]
fn an_array_reads_a_token_made_only_of_signs_as_null_in_its_slot() {
    for tok in SIGN_TOKENS {
        let src = format!("[0 {tok} 7]");
        let Some(array) = Array::from_bytes(src.as_bytes()) else {
            continue;
        };
        let signs = tok.split_whitespace().count();
        let raw: Vec<_> = array.raw_iter().collect();
        let items: Vec<_> = array.iter::<Object<'_>>().collect();
        assert_eq!(raw.len(), signs + 2, "{src}");
        assert_eq!(items.len(), signs + 2, "{src}");
        assert_eq!(items.first().and_then(|o| o.clone().into_i32()), Some(0), "{src}");
        assert_eq!(items.last().and_then(|o| o.clone().into_i32()), Some(7), "{src}");
        for (item, sign) in items.iter().skip(1).zip(tok.split_whitespace()) {
            if unreadable(sign) {
                assert!(matches!(item, Object::Null(_)), "{src}: {item:?}");
            } else {
                assert!(matches!(item, Object::Number(_)), "{src}: {item:?}");
            }
        }
        // A typed read still stops at a slot that is not a number, as at any other value.
        let numbers_before = 1 + tok.split_whitespace().take_while(|sign| !unreadable(sign)).count();
        let expected = if numbers_before == signs + 1 { signs + 2 } else { numbers_before };
        assert_eq!(array.iter::<f32>().count(), expected, "{src}");
    }
}

/// Every token of up to five characters over signs, digits, `.` and `e` that starts with a
/// sign, in each position a value can take, iterates without panicking, and an array never
/// loses the item after it.
#[test]
fn short_sign_led_tokens_never_panic_in_a_dictionary_or_array() {
    const ALPHABET: &[u8] = b"+-1.e ";
    let mut checked = 0usize;
    let mut tok = Vec::new();
    for len in 1..=5u32 {
        let total = ALPHABET.len().pow(len);
        for code in 0..total {
            tok.clear();
            let mut k = code;
            for _ in 0..len {
                tok.push(ALPHABET.get(k % ALPHABET.len()).copied().unwrap_or(b' '));
                k /= ALPHABET.len();
            }
            if !matches!(tok.first(), Some(b'+' | b'-')) || tok.last() == Some(&b' ') {
                continue;
            }
            let tok = std::str::from_utf8(&tok).unwrap_or_default().to_owned();
            checked += 1;
            for src in [format!("[{tok}]"), format!("[0 {tok} 7]"), format!("<< /A {tok} /B 7 >>")] {
                let outcome = std::panic::catch_unwind(|| {
                    if src.starts_with('[') {
                        Array::from_bytes(src.as_bytes()).map(|a| {
                            let _ = format!("{a:?}");
                            let raw = a.raw_iter().count();
                            let items: Vec<_> = a.iter::<Object<'_>>().collect();
                            (raw, items.len(), items.last().and_then(|o| o.clone().into_i32()))
                        })
                    } else {
                        Dict::from_bytes(src.as_bytes()).map(|d| {
                            let _ = format!("{d:?}");
                            let entries = d.entries().count();
                            let _ = d.get::<Object<'_>>("A");
                            (entries, entries, d.get::<i32>("B"))
                        })
                    }
                });
                let Ok(result) = outcome else {
                    panic!("{src:?} panicked");
                };
                let Some((raw, items, last)) = result else {
                    continue;
                };
                if src.starts_with('[') {
                    assert_eq!(raw, items, "{src:?}: raw_iter and iter disagree");
                    if src.starts_with("[0 ") {
                        assert_eq!(last, Some(7), "{src:?}: the item after the token was lost");
                    }
                } else {
                    assert_eq!(last, Some(7), "{src:?}: /B was lost");
                }
            }
        }
    }
    // Two leading signs, then 6^(len-2) middles and 5 last characters (no trailing space).
    assert_eq!(checked, 2 + 2 * 5 + 2 * 6 * 5 + 2 * 36 * 5 + 2 * 216 * 5);
    // A sign-led number still reads as one.
    assert_eq!(Number::from_bytes(b"+1").map(|n| n.as_f64()), Some(1.0));
}
