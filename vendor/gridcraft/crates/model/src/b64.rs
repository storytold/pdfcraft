//! Minimal base64 for serde (image bytes in JSON).

use serde::{Deserialize, Deserializer, Serializer};

const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = A.iter().position(|&x| x == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&encode(v))
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let s = String::deserialize(d)?;
    decode(&s).ok_or_else(|| serde::de::Error::custom("invalid base64"))
}

/// Base64 for byte blobs shared through `Arc` (floating pictures), so copies of a workbook, such as
/// its undo steps, share one payload.
pub mod shared {
    use std::sync::Arc;

    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Arc<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        super::serialize(v, s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<Vec<u8>>, D::Error> {
        super::deserialize(d).map(Arc::new)
    }
}

/// Base64 for optional shared byte blobs (VBA projects): `null`/missing stays `None`.
pub mod opt {
    use std::sync::Arc;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &Option<Arc<Vec<u8>>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(data) => s.serialize_str(&super::encode(data)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Arc<Vec<u8>>>, D::Error> {
        let s = Option::<String>::deserialize(d)?;
        match s {
            None => Ok(None),
            Some(s) => super::decode(&s).map(|v| Some(Arc::new(v))).ok_or_else(|| serde::de::Error::custom("invalid base64")),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        for data in [&b""[..], b"a", b"ab", b"abc", b"hello world", &[0, 255, 128, 7]] {
            assert_eq!(super::decode(&super::encode(data)).ok_or("none")?, data);
        }
        assert_eq!(super::encode(b"Man"), "TWFu");
        Ok(())
    }
}
