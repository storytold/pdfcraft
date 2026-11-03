//! What an answer can cite, and how the passages sent to the provider are named.
//!
//! Each passage goes out as a `search_result` block whose `source` is a [`SourceId`] string, and
//! citations come back naming the block they quote. Pages are 0-based page indices throughout;
//! only text meant for people (titles, labels) says "page 4".

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::Digest;

/// Longest quote kept from a citation.
pub const MAX_QUOTE: usize = 2_000;
/// Longest title kept from a citation.
pub const MAX_TITLE: usize = 512;
/// Longest web address kept from a citation.
pub const MAX_URL: usize = 2_048;

/// The SHA-256 of a file's bytes: how the library knows a document wherever it is stored.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sha256(pub [u8; 32]);

impl Sha256 {
    pub fn of(bytes: &[u8]) -> Self {
        Self(sha2::Sha256::digest(bytes).into())
    }

    pub fn to_hex(&self) -> String {
        use fmt::Write;
        let mut s = String::with_capacity(64);
        for b in self.0 {
            // Writing to a String can't fail.
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    /// Parse 64 hex digits (either case).
    pub fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 64 || !s.is_ascii() {
            return None;
        }
        let mut out = [0u8; 32];
        let (pairs, _) = s.as_bytes().as_chunks::<2>();
        for (slot, pair) in out.iter_mut().zip(pairs) {
            let pair = std::str::from_utf8(pair).ok()?;
            *slot = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(Self(out))
    }
}

impl fmt::Debug for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha256({})", self.to_hex())
    }
}

impl fmt::Display for Sha256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for Sha256 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Sha256 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::from_hex(&s).ok_or_else(|| serde::de::Error::custom("expected 64 hex digits"))
    }
}

/// A document's identity: its hash, and its size as a cheap first check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DocRef {
    pub sha256: Sha256,
    pub size: u64,
}

impl DocRef {
    pub fn of(bytes: &[u8]) -> Self {
        Self { sha256: Sha256::of(bytes), size: u64::try_from(bytes.len()).unwrap_or(u64::MAX) }
    }
}

const DOC_PREFIX: &str = "pdfcraft-doc:i";
const LIB_PREFIX: &str = "pdfcraft-lib:";

/// The `source` of a passage sent to the provider: `pdfcraft-doc:i<page>` for the document being
/// read, `pdfcraft-lib:<sha256>:i<page>` for a library document (`i` for page index).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceId {
    ThisDocument { page: u32 },
    Library { sha256: Sha256, page: u32 },
}

impl SourceId {
    pub fn encode(&self) -> String {
        match self {
            Self::ThisDocument { page } => format!("{DOC_PREFIX}{page}"),
            Self::Library { sha256, page } => format!("{LIB_PREFIX}{sha256}:i{page}"),
        }
    }

    /// Parse a `source` string. Anything not in exactly the form [`Self::encode`] writes is `None`.
    pub fn parse(s: &str) -> Option<Self> {
        if let Some(page) = s.strip_prefix(DOC_PREFIX) {
            return Some(Self::ThisDocument { page: parse_page(page)? });
        }
        let rest = s.strip_prefix(LIB_PREFIX)?;
        let (hash, page) = rest.split_once(":i")?;
        // Lowercase only, as `encode` writes it, so one hash has one spelling.
        if !hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return None;
        }
        Some(Self::Library { sha256: Sha256::from_hex(hash)?, page: parse_page(page)? })
    }
}

/// Decimal digits only, no sign, no leading zeros: one spelling per page, so a string the model
/// rewrote can't alias another page.
fn parse_page(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0')) {
        return None;
    }
    s.parse().ok()
}

/// What an answer cites. Every field came from the provider: callers check `page` against the
/// document's page count before going there.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    ThisDocument { page: u32, quote: String },
    Library { sha256: Sha256, page: u32, quote: String },
    Web { url: String, title: String, quote: String },
}

impl Source {
    pub fn from_id(id: SourceId, quote: &str) -> Self {
        let quote = crate::text::capped(quote, MAX_QUOTE);
        match id {
            SourceId::ThisDocument { page } => Self::ThisDocument { page, quote },
            SourceId::Library { sha256, page } => Self::Library { sha256, page, quote },
        }
    }

    pub fn quote(&self) -> &str {
        match self {
            Self::ThisDocument { quote, .. } | Self::Library { quote, .. } | Self::Web { quote, .. } => quote,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes() {
        // FIPS 180-2 test vector for "abc".
        let h = Sha256::of(b"abc");
        assert_eq!(h.to_hex(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(Sha256::from_hex(&h.to_hex()), Some(h));
        assert_eq!(Sha256::from_hex(&h.to_hex().to_uppercase()), Some(h));
        assert_eq!(Sha256::from_hex("ab"), None);
        assert_eq!(Sha256::from_hex(&"g".repeat(64)), None);
        assert_eq!(Sha256::from_hex(&"é".repeat(32)), None);
        assert_eq!(serde_json::to_string(&h).ok(), Some(format!("\"{h}\"")));
        assert_eq!(DocRef::of(b"abc").size, 3);
    }

    #[test]
    fn source_ids_round_trip_and_reject_lookalikes() {
        let h = Sha256::of(b"x");
        for id in [SourceId::ThisDocument { page: 0 }, SourceId::ThisDocument { page: u32::MAX }, SourceId::Library { sha256: h, page: 41 }] {
            assert_eq!(SourceId::parse(&id.encode()), Some(id));
        }
        assert_eq!(SourceId::ThisDocument { page: 3 }.encode(), "pdfcraft-doc:i3");
        for bad in [
            "pdfcraft-doc:i",
            "pdfcraft-doc:i03",
            "pdfcraft-doc:i-1",
            "pdfcraft-doc:i+1",
            "pdfcraft-doc:i4294967296",
            "pdfcraft-doc:p3",
            "pdfcraft-lib:abc:i1",
            "https://example.com",
            "",
        ] {
            assert_eq!(SourceId::parse(bad), None, "{bad}");
        }
        assert_eq!(SourceId::parse(&format!("pdfcraft-lib:{h}:i1:i2")), None);
        assert_eq!(SourceId::parse(&format!("pdfcraft-lib:{}:i1", h.to_hex().to_uppercase())), None);
    }

    #[test]
    fn quotes_are_capped() {
        let s = Source::from_id(SourceId::ThisDocument { page: 1 }, &"é".repeat(MAX_QUOTE));
        assert!(s.quote().len() <= MAX_QUOTE);
    }

    proptest::proptest! {
        #[test]
        fn parse_is_total(s in ".*") {
            if let Some(id) = SourceId::parse(&s) {
                proptest::prop_assert_eq!(id.encode(), s);
            }
        }
    }
}
