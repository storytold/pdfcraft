// Keep in sync with `hayro-syntax/src/object/string.rs` (`read_literal`).

use alloc::vec::Vec;

use crate::reader::Reader;

pub(crate) fn decode_into(data: &[u8], out: &mut Vec<u8>) -> Option<()> {
    let mut r = Reader::new(data);

    while let Some(byte) = r.read_byte() {
        match byte {
            b'\\' => {
                let next = r.read_byte()?;

                // PdfCraft patch (3): `\ddd` is one to three octal digits "with high-order
                // overflow ignored" (PostScript Language Reference, third edition,
                // §3.2.2), so `\737` is 0o737 & 0xFF, 0xDF, as in hayro-syntax.
                if let Some(first) = octal_digit(next) {
                    let mut value = first;
                    for _ in 0..2 {
                        match r.peek_byte().and_then(octal_digit) {
                            Some(digit) => {
                                r.read_byte();
                                // At most 0o777 after three digits: no overflow in u16.
                                value = value * 8 + digit;
                            }
                            None => break,
                        }
                    }

                    let [low, _] = value.to_le_bytes();
                    out.push(low);
                } else {
                    match next {
                        b'n' => out.push(0xA),
                        b'r' => out.push(0xD),
                        b't' => out.push(0x9),
                        b'b' => out.push(0x8),
                        b'f' => out.push(0xC),
                        b'(' => out.push(b'('),
                        b')' => out.push(b')'),
                        b'\\' => out.push(b'\\'),
                        b'\n' | b'\r' => {
                            // "If the \ is followed immediately by a newline
                            // (CR, LF, or CR-LF pair), the scanner ignores
                            // both the initial \ and the newline; this breaks
                            // a string into multiple lines without including
                            // the newline character as part of the string."
                            r.skip_eol();
                        }
                        _ => out.push(next),
                    }
                }
            }
            b'(' | b')' => out.push(byte),
            // "But if a newline appears without a preceding \, the result is
            // equivalent to \n."
            b'\n' | b'\r' => {
                out.push(b'\n');
                r.skip_eol();
            }
            other => out.push(other),
        }
    }

    Some(())
}

/// The value of an octal digit, `None` for any other byte.
fn octal_digit(byte: u8) -> Option<u16> {
    match byte {
        b'0'..=b'7' => Some(u16::from(byte - b'0')),
        _ => None,
    }
}
