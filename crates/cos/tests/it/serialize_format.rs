//! `serialize` writes integers, references, hexadecimal strings, octal escapes and `#xx` name
//! escapes with its own digit loops. They must give the bytes the standard formatter gives.

use pdfcraft_cos::{ObjRef, Object, PdfString, serialize};
use proptest::prelude::*;

fn bytes_of(o: &Object) -> Vec<u8> {
    let mut v = Vec::new();
    serialize(o, &mut v);
    v
}

#[test]
fn integer_extremes_and_digit_boundaries() {
    let mut cases = vec![0i64, 1, -1, 9, 10, 99, 100, i64::MAX, i64::MIN, i64::MIN + 1, i64::from(i32::MAX), i64::from(i32::MIN)];
    let mut p = 1i64;
    while let Some(next) = p.checked_mul(10) {
        cases.extend([next - 1, next, next + 1, -next]);
        p = next;
    }
    for i in cases {
        assert_eq!(bytes_of(&Object::Int(i)), i.to_string().into_bytes(), "{i}");
    }
}

#[test]
fn reference_extremes() {
    for (n, g) in [(0, 0), (1, 0), (u32::MAX, u16::MAX), (10, 65535), (4_000_000_000, 7)] {
        assert_eq!(bytes_of(&Object::Ref(ObjRef::new(n, g))), format!("{n} {g} R").into_bytes());
    }
}

#[test]
fn every_byte_value_in_hex_strings_octal_escapes_and_names() {
    let all: Vec<u8> = (0..=255).collect();
    let hex: String = all.iter().map(|b| format!("{b:02X}")).collect();
    assert_eq!(bytes_of(&Object::String(PdfString { bytes: all.clone(), hex: true })), format!("<{hex}>").into_bytes());
    // A literal string with a few control bytes escapes them in octal.
    let lit = bytes_of(&Object::String(PdfString { bytes: b"abcdefghijkl\x01\x7f\xffz".to_vec(), hex: false }));
    assert_eq!(lit, b"(abcdefghijkl\\001\\177\\377z)");
    let name = bytes_of(&Object::Name(all.clone()));
    let mut want = b"/".to_vec();
    for &b in &all {
        let regular = (b'!'..=b'~').contains(&b) && !matches!(b, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%' | b'#');
        if regular {
            want.push(b);
        } else {
            want.extend_from_slice(format!("#{b:02X}").as_bytes());
        }
    }
    assert_eq!(name, want);
}

proptest! {
    #[test]
    fn integers_match_the_formatter(i in any::<i64>()) {
        prop_assert_eq!(bytes_of(&Object::Int(i)), format!("{i}").into_bytes());
    }

    #[test]
    fn whole_reals_match_the_formatter(i in any::<i32>()) {
        prop_assert_eq!(bytes_of(&Object::Real(f64::from(i))), format!("{i}").into_bytes());
    }

    #[test]
    fn references_match_the_formatter(n in any::<u32>(), g in any::<u16>()) {
        prop_assert_eq!(bytes_of(&Object::Ref(ObjRef::new(n, g))), format!("{n} {g} R").into_bytes());
    }

    #[test]
    fn hex_strings_match_the_formatter(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
        let mut want = String::from("<");
        for b in &bytes {
            want.push_str(&format!("{b:02X}"));
        }
        want.push('>');
        prop_assert_eq!(bytes_of(&Object::String(PdfString { bytes, hex: true })), want.into_bytes());
    }

    #[test]
    fn binary_literal_strings_match_the_formatter(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
        // Mostly-binary input is written as hex; mostly-text input with escapes is a literal.
        let got = bytes_of(&Object::String(PdfString { bytes: bytes.clone(), hex: false }));
        let binary = bytes.iter().filter(|b| !(b' '..=b'~').contains(*b) && !matches!(b, b'\n' | b'\r' | b'\t')).count();
        let mut want = Vec::new();
        if binary * 4 > bytes.len().max(1) {
            want.push(b'<');
            for b in &bytes {
                want.extend_from_slice(format!("{b:02X}").as_bytes());
            }
            want.push(b'>');
        } else {
            want.push(b'(');
            for &b in &bytes {
                match b {
                    b'(' | b')' | b'\\' => want.extend_from_slice(&[b'\\', b]),
                    b'\n' => want.extend_from_slice(b"\\n"),
                    b'\r' => want.extend_from_slice(b"\\r"),
                    b'\t' => want.extend_from_slice(b"\\t"),
                    0x08 => want.extend_from_slice(b"\\b"),
                    0x0C => want.extend_from_slice(b"\\f"),
                    b' '..=b'~' => want.push(b),
                    _ => want.extend_from_slice(format!("\\{b:03o}").as_bytes()),
                }
            }
            want.push(b')');
        }
        prop_assert_eq!(got, want);
    }
}
