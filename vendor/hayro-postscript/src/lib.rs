/*!
A lightweight PostScript scanner.

This crate provides a scanner for tokenizing PostScript programs into typed objects.
It currently only implements a very small subset of the PostScript language,
with the main goal of being enough to parse CMAP files, but the scope _might_
be expanded upon in the future.

The supported types include integers and real numbers, name objects, strings and arrays.
Unsupported is anything else, including dictionaries, procedures, etc. An error
will be returned in case any of these is encountered.

## Safety
This crate forbids unsafe code via a crate-level attribute.
*/

#![no_std]
#![forbid(unsafe_code)]
#![allow(missing_docs)]

extern crate alloc;

mod array;
mod error;
mod name;
mod number;
mod object;
mod reader;
mod string;

pub use array::Array;
pub use error::{Error, Result};
pub use name::Name;
pub use number::Number;
pub use object::Object;
pub use string::String;

use reader::Reader;

/// A PostScript scanner that parses [`Object`]s from a byte stream.
pub struct Scanner<'a> {
    reader: Reader<'a>,
}

impl<'a> Scanner<'a> {
    /// Create a new scanner over the given bytes of a PostScript program.
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            reader: Reader::new(data),
        }
    }

    /// Returns `true` if there are no more objects to parse.
    pub fn at_end(&mut self) -> bool {
        object::at_end(&mut self.reader)
    }

    /// Parse the next object.
    pub fn parse_object(&mut self) -> Result<Object<'a>> {
        object::read(&mut self.reader)
    }

    /// PdfCraft patch (2): parse the next object; when it can't be read, read on from where
    /// the failed read stopped, or, when it read nothing, from one byte past where its token
    /// starts, so a caller can read on past a token it can't parse (a stray `>` in a name, a
    /// CMap name with spaces in it) instead of stopping there. Every failure moves forward, one
    /// failure per unreadable token, and what a failed read went through (an unclosed string,
    /// to the end of the data) is not read again, so a loop over this ends and reads the data
    /// about once.
    pub fn parse_object_or_skip(&mut self) -> Result<Object<'a>> {
        // The whitespace and comments before the token are not the token: skipped first, they
        // don't count as progress, so a token after a space fails once, not twice.
        object::skip_whitespace_and_comments(&mut self.reader);
        let start = self.reader.offset();
        let read = object::read(&mut self.reader);
        if read.is_err() && self.reader.offset() <= start {
            self.reader
                .jump(start.saturating_add(1).min(self.reader.data.len()));
        }
        read
    }

    /// Parse the next object as a [`Number`].
    pub fn parse_number(&mut self) -> Result<Number> {
        match self.parse_object()? {
            Object::Number(n) => Ok(n),
            _ => Err(Error::SyntaxError),
        }
    }

    /// Parse the next object as a [`Name`].
    pub fn parse_name(&mut self) -> Result<Name<'a>> {
        match self.parse_object()? {
            Object::Name(n) => Ok(n),
            _ => Err(Error::SyntaxError),
        }
    }

    /// Parse the next object as a [`String`].
    pub fn parse_string(&mut self) -> Result<String<'a>> {
        match self.parse_object()? {
            Object::String(s) => Ok(s),
            _ => Err(Error::SyntaxError),
        }
    }

    /// Parse the next object as an [`Array`].
    pub fn parse_array(&mut self) -> Result<Array<'a>> {
        match self.parse_object()? {
            Object::Array(a) => Ok(a),
            _ => Err(Error::SyntaxError),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmap_snippet() {
        let input = br#"/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
/CMapName /Test-H def
1 begincodespacerange
<00> <FF>
endcodespacerange
2 beginbfchar
<03> <0041>
<04> <0042>
endbfchar
endcmap"#;

        let mut s = Scanner::new(input);

        assert_eq!(s.parse_name().unwrap(), Name::new(b"CIDInit", true));
        assert_eq!(s.parse_name().unwrap(), Name::new(b"ProcSet", true));
        assert_eq!(s.parse_name().unwrap(), Name::new(b"findresource", false));
        assert_eq!(s.parse_name().unwrap(), Name::new(b"begin", false));
        assert_eq!(s.parse_number().unwrap(), Number::Integer(12));
        assert_eq!(s.parse_name().unwrap(), Name::new(b"dict", false));
        assert_eq!(s.parse_name().unwrap(), Name::new(b"begin", false));
        assert_eq!(s.parse_name().unwrap(), Name::new(b"begincmap", false));
        assert_eq!(s.parse_name().unwrap(), Name::new(b"CMapName", true));
        assert_eq!(s.parse_name().unwrap(), Name::new(b"Test-H", true));
        assert_eq!(s.parse_name().unwrap(), Name::new(b"def", false));
        assert_eq!(s.parse_number().unwrap(), Number::Integer(1));
        assert_eq!(
            s.parse_name().unwrap(),
            Name::new(b"begincodespacerange", false)
        );
        assert_eq!(s.parse_string().unwrap(), String::from_hex(b"00"));
        assert_eq!(s.parse_string().unwrap(), String::from_hex(b"FF"));
        assert_eq!(
            s.parse_name().unwrap(),
            Name::new(b"endcodespacerange", false)
        );
        assert_eq!(s.parse_number().unwrap(), Number::Integer(2));
        assert_eq!(s.parse_name().unwrap(), Name::new(b"beginbfchar", false));
        assert_eq!(s.parse_string().unwrap(), String::from_hex(b"03"));
        assert_eq!(s.parse_string().unwrap(), String::from_hex(b"0041"));
        assert_eq!(s.parse_string().unwrap(), String::from_hex(b"04"));
        assert_eq!(s.parse_string().unwrap(), String::from_hex(b"0042"));
        assert_eq!(s.parse_name().unwrap(), Name::new(b"endbfchar", false));
        assert_eq!(s.parse_name().unwrap(), Name::new(b"endcmap", false));
        assert!(s.at_end());
    }

    #[test]
    fn array_round_trip() {
        let input = b"[123 /abc (xyz)]";
        let mut scanner = Scanner::new(input);
        let arr = scanner.parse_array().unwrap();
        assert!(scanner.at_end());

        let mut inner = arr.objects();
        assert_eq!(inner.parse_number().unwrap(), Number::Integer(123));
        assert_eq!(inner.parse_name().unwrap(), Name::new(b"abc", true));
        assert_eq!(inner.parse_string().unwrap(), String::from_literal(b"xyz"));
        assert!(inner.at_end());
    }

    #[test]
    fn comments_skipped() {
        let input = b"% comment\n42 % another\n/Name";
        let mut scanner = Scanner::new(input);

        assert_eq!(scanner.parse_number().unwrap(), Number::Integer(42));
        assert_eq!(scanner.parse_name().unwrap(), Name::new(b"Name", true));
        assert!(scanner.at_end());
    }

    #[test]
    fn wrong_type_is_error() {
        let mut scanner = Scanner::new(b"42 ");
        assert!(scanner.parse_name().is_err());
    }

    /// PdfCraft patch (2) test: a token that can't be read fails once and is stepped over,
    /// whether or not whitespace or a comment comes before it, and the objects after it read.
    #[test]
    fn parse_object_or_skip_steps_over_an_unreadable_token() {
        let mut s = Scanner::new(b"/A> /B % note\n > /C");
        assert_eq!(
            s.parse_object_or_skip(),
            Ok(Object::Name(Name::new(b"A", true)))
        );
        assert!(s.parse_object_or_skip().is_err());
        assert_eq!(
            s.parse_object_or_skip(),
            Ok(Object::Name(Name::new(b"B", true)))
        );
        assert!(s.parse_object_or_skip().is_err());
        assert_eq!(
            s.parse_object_or_skip(),
            Ok(Object::Name(Name::new(b"C", true)))
        );
        assert!(s.at_end());
    }

    /// PdfCraft patch (2) test: what a failed read went through is not read again. An unclosed
    /// string reads to the end of the data, so one failure leaves nothing to read; the nested
    /// `(` inside it are not each read to the end once more.
    #[test]
    fn parse_object_or_skip_does_not_read_a_failed_read_again() {
        let mut s = Scanner::new(b"/A (((( x");
        assert_eq!(
            s.parse_object_or_skip(),
            Ok(Object::Name(Name::new(b"A", true)))
        );
        assert!(s.parse_object_or_skip().is_err());
        assert!(s.at_end());
    }

    /// PdfCraft patch (2) test: at the end of the data a read fails and the scanner stays at
    /// the end; it never steps past the data.
    #[test]
    fn parse_object_or_skip_stays_at_the_end() {
        for input in [&b""[..], b"  ", b">"] {
            let mut s = Scanner::new(input);
            for _ in 0..3 {
                assert!(s.parse_object_or_skip().is_err());
                assert!(s.reader.offset() <= input.len());
            }
            assert!(s.at_end());
        }
    }
}
