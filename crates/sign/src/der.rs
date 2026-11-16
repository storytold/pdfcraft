//! BER/DER codecs delegated to the existing shared `craft-pkcs7` implementation.
//!
//! This facade preserves PdfCraft's error type and borrowed signed encodings. Certificate
//! trust, signing policy and document interpretation remain in their existing app modules.

use crate::SignError;
use craft_pkcs7::der as shared;
pub use shared::{Form, Time, algorithm, bit_string, boolean, explicit, ia5, int, null, octets, oid, printable, seq, set_of, tag, tlv, uint, utf8};
use std::borrow::Cow;

pub(crate) fn app_error(error: craft_pkcs7::Error) -> SignError {
    match error {
        craft_pkcs7::Error::Malformed(message) => SignError::Malformed(message),
        craft_pkcs7::Error::Unsupported(message) => SignError::Unsupported(message),
        craft_pkcs7::Error::WrongPassword => SignError::WrongPassword,
        craft_pkcs7::Error::Crypto(message) => SignError::Crypto(message),
    }
}

/// A borrowed shared element with the app's existing fallible method signatures.
/// Dereferencing exposes the same public tag, contents and exact original encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tlv<'a>(shared::Tlv<'a>);

impl<'a> std::ops::Deref for Tlv<'a> {
    type Target = shared::Tlv<'a>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Tlv<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<'a> Tlv<'a> {
    pub fn is_indefinite(&self) -> bool {
        self.0.is_indefinite()
    }
    pub fn uint_bytes(&self) -> &'a [u8] {
        self.0.uint_bytes()
    }
    pub fn text(&self) -> Option<String> {
        self.0.text()
    }

    pub fn parse(input: &'a [u8]) -> Result<(Self, &'a [u8]), SignError> {
        shared::Tlv::parse(input).map(|(element, rest)| (Self(element), rest)).map_err(app_error)
    }
    pub fn parse_ber(input: &'a [u8]) -> Result<(Self, &'a [u8]), SignError> {
        shared::Tlv::parse_ber(input).map(|(element, rest)| (Self(element), rest)).map_err(app_error)
    }
    pub fn parse_all(input: &'a [u8]) -> Result<Self, SignError> {
        shared::Tlv::parse_all(input).map(Self).map_err(app_error)
    }
    pub fn parse_all_ber(input: &'a [u8]) -> Result<Self, SignError> {
        shared::Tlv::parse_all_ber(input).map(Self).map_err(app_error)
    }
    pub fn children(&self) -> Result<Vec<Self>, SignError> {
        self.0.children().map(|children| children.into_iter().map(Self).collect()).map_err(app_error)
    }
    pub fn expect(self, tag: u8, what: &str) -> Result<Self, SignError> {
        self.0.expect(tag, what).map(Self).map_err(app_error)
    }
    pub fn inner(&self) -> Result<Self, SignError> {
        self.0.inner().map(Self).map_err(app_error)
    }
    pub fn octets(&self, what: &str) -> Result<Cow<'a, [u8]>, SignError> {
        self.0.octets(what).map_err(app_error)
    }
    pub fn oid(&self) -> Result<String, SignError> {
        self.0.oid().map_err(app_error)
    }
    pub fn u64(&self) -> Result<u64, SignError> {
        self.0.u64().map_err(app_error)
    }
    pub fn enumerated(&self) -> Result<u64, SignError> {
        self.0.enumerated().map_err(app_error)
    }
    pub fn bits(&self) -> Result<&'a [u8], SignError> {
        self.0.bits().map_err(app_error)
    }
    pub fn time(&self) -> Result<Time, SignError> {
        self.0.time().map_err(app_error)
    }
}

pub fn try_oid(dotted: &str) -> Result<Vec<u8>, SignError> {
    shared::try_oid(dotted).map_err(app_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_adapter_preserves_borrowing_and_error_variants() {
        let encoded = seq(&[&int(7), &octets(b"signed bytes")]);
        let element = Tlv::parse_all(&encoded).unwrap();
        assert_eq!(element.raw.as_ptr(), encoded.as_ptr());
        let children = element.children().unwrap();
        let bytes = children[1].octets("payload").unwrap();
        assert!(matches!(bytes, Cow::Borrowed(_)));
        assert_eq!(bytes.as_ptr(), children[1].value.as_ptr());
        assert!(matches!(Tlv::parse_all(&[0x30, 0x80, 0, 0]), Err(SignError::Malformed(_))));
        assert!(matches!(try_oid("1..2"), Err(SignError::Malformed(_))));
        for length in 0..encoded.len() {
            let local = Tlv::parse_all(&encoded[..length]).unwrap_err();
            let shared = shared::Tlv::parse_all(&encoded[..length]).unwrap_err();
            assert_eq!(local.to_string(), shared.to_string());
        }
    }

    #[test]
    fn a_nine_byte_integer_needs_a_zero_pad_to_fit_64_bits() {
        let tlv = |value: &[u8]| {
            let mut b = vec![tag::INTEGER, value.len() as u8];
            b.extend_from_slice(value);
            b
        };
        // 2^64 would fold to 0: refused, not read as 0
        assert!(Tlv::parse_all(&tlv(&[1, 0, 0, 0, 0, 0, 0, 0, 0])).unwrap().u64().is_err());
        // u64::MAX with its pad is fine
        let max = [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        assert_eq!(Tlv::parse_all(&tlv(&max)).unwrap().u64().unwrap(), u64::MAX);
    }

    #[test]
    fn try_oid_rejects_signs_and_empty_arcs() {
        for bad in ["+1.2.840", "1.+2", "1.2.-3", "1..2"] {
            assert!(try_oid(bad).is_err(), "accepted {bad}");
        }
        assert_eq!(try_oid("1.2.840.113549").unwrap(), oid("1.2.840.113549"));
    }

    #[test]
    fn round_trips() {
        let o = oid("1.2.840.113549.1.7.2");
        let t = Tlv::parse_all(&o).unwrap();
        assert_eq!(t.oid().unwrap(), "1.2.840.113549.1.7.2");
        let big = vec![7u8; 300];
        let e = octets(&big);
        assert_eq!(&e[..4], &[0x04, 0x82, 0x01, 0x2C]);
        assert_eq!(Tlv::parse_all(&e).unwrap().value, &big[..]);
        assert_eq!(uint(&[0x80, 1]), vec![0x02, 3, 0, 0x80, 1]);
        assert_eq!(Tlv::parse_all(&int(65537)).unwrap().u64().unwrap(), 65537);
        let s = set_of(&[&int(2), &int(1)]);
        assert_eq!(s, vec![0x31, 6, 2, 1, 1, 2, 1, 2]);
        assert!(Tlv::parse(&[0x30, 5, 1]).is_err());
    }

    #[test]
    fn reads_ber_indefinite_lengths_and_segmented_strings() {
        // SEQUENCE { INTEGER 3, OCTET STRING { "hi", "!" } }, every wrapper in the indefinite form:
        // the shape Windows gives a .p12 file.
        let ber: &[u8] = &[0x30, 0x80, 0x02, 0x01, 0x03, 0x24, 0x80, 0x04, 0x02, b'h', b'i', 0x04, 0x01, b'!', 0, 0, 0, 0];
        let e = Tlv::parse_all(ber).unwrap_err().to_string();
        assert!(e.contains("indefinite lengths are not allowed in DER"), "{e}");

        let t = Tlv::parse_all_ber(ber).unwrap();
        assert_eq!(t.tag, tag::SEQUENCE);
        assert_eq!(t.raw, ber, "the end-of-contents octets belong to the element");
        assert_eq!(t.value, &ber[2..ber.len() - 2], "but not to its contents");
        let c = t.children().unwrap();
        assert_eq!(c[0].u64().unwrap(), 3);
        assert_eq!(c[1].octets("segments").unwrap().as_ref(), b"hi!", "the segments are joined");
        assert!(c[0].octets("not a string").is_err());

        // Definite lengths read the same either way, and a segmented string stays a DER error.
        assert_eq!(Tlv::parse_all_ber(&int(65537)).unwrap().u64().unwrap(), 65537);
        assert_eq!(Tlv::parse_all_ber(&octets(b"hi!")).unwrap().octets("plain").unwrap().as_ref(), b"hi!");
        assert!(Tlv::parse_all(&[0x24, 2, 0x04, 0]).unwrap().octets("segments").is_err());

        // Malformed: no end-of-contents, and an indefinite length on a primitive element.
        assert!(Tlv::parse_all_ber(&ber[..ber.len() - 2]).is_err());
        assert!(Tlv::parse_all_ber(&[0x04, 0x80, 0, 0]).is_err());
        // Nesting is bounded, so a file made of nothing but wrappers fails instead of crashing.
        let deep = [[0x30u8, 0x80].repeat(400), [0u8, 0].repeat(400)].concat();
        assert!(Tlv::parse_all_ber(&deep).is_err());
    }

    #[test]
    fn times() {
        let t = Time { year: 2026, month: 10, day: 2, hour: 9, minute: 30, second: 5 };
        assert_eq!(Time::from_unix(t.unix()), t);
        assert_eq!(Tlv::parse_all(&t.encode()).unwrap().time().unwrap(), t);
        let far = Time { year: 2051, ..t };
        assert_eq!(Tlv::parse_all(&far.encode()).unwrap().tag, tag::GENERALIZED_TIME);
        assert_eq!(Time::from_pdf("D:20261002113005+02'00'"), Some(Time { year: 2026, month: 10, day: 2, hour: 9, minute: 30, second: 5 }));
        assert_eq!(Time::from_unix(0), Time { year: 1970, month: 1, day: 1, hour: 0, minute: 0, second: 0 });
        // Past chrono's ±262143-year range `from_unix` keeps the proleptic Gregorian date the old
        // hand-rolled converter gave, not the epoch. No real signing time is near this; the `year`
        // field is a `u32`, so the year wraps exactly as the old `as u32` cast did (values pinned
        // from the old formula). This also covers both i64 extremes so neither div_euclid panics.
        assert_eq!(Time::from_unix(10_000_000_000_000), Time { year: 318857, month: 5, day: 20, hour: 17, minute: 46, second: 40 });
        assert_eq!(Time::from_unix(-10_000_000_000_000), Time { year: 4_294_652_378, month: 8, day: 13, hour: 6, minute: 13, second: 20 });
        assert_eq!(Time::from_unix(i64::MAX), Time { year: 219_250_468, month: 12, day: 4, hour: 15, minute: 30, second: 7 });
        assert_eq!(Time::from_unix(i64::MIN), Time { year: 4_075_720_767, month: 1, day: 27, hour: 8, minute: 29, second: 52 });
    }
}
