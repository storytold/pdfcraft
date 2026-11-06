//! The glTF container: a JSON text or a GLB binary [glTF 2.0 §4.4], and
//! the buffers it names, embedded, data-URI or external [§3.6.1].

use super::json::Json;
#[cfg(test)]
use super::json::JsonExt;

use super::{GltfError, member};

const GLB_MAGIC: &[u8; 4] = b"glTF";
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;
/// The largest buffer read: glTF's own counts and offsets are 32-bit.
const MAX_BUFFER: u64 = u32::MAX as u64;

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at.checked_add(4)?)?;
    b.try_into().ok().map(u32::from_le_bytes)
}

/// The JSON document and the GLB binary chunk, when there is one.
pub(super) fn split(data: &[u8]) -> Result<(Json, Option<&[u8]>), GltfError> {
    if !data.starts_with(GLB_MAGIC) {
        let text = data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(data);
        return Ok((crate::gltf::json::parse(text)?, None));
    }
    let not = GltfError::NotGltf;
    if u32_at(data, 4) != Some(2) {
        return Err(not("a GLB container other than version 2"));
    }
    let total = u32_at(data, 8).ok_or(not("a truncated GLB header"))? as usize;
    let data = data.get(..total).ok_or(not("a GLB shorter than its header says"))?;
    let chunk = |at: usize| -> Option<(u32, &[u8], usize)> {
        let len = u32_at(data, at)? as usize;
        let kind = u32_at(data, at.checked_add(4)?)?;
        let start = at.checked_add(8)?;
        let end = start.checked_add(len)?;
        Some((kind, data.get(start..end)?, end))
    };
    let (kind, json, next) = chunk(12).ok_or(not("a truncated GLB chunk"))?;
    if kind != CHUNK_JSON {
        return Err(not("a GLB whose first chunk is not JSON"));
    }
    // Chunks are 4-byte aligned; the JSON chunk is space-padded.
    let bin = match next.checked_next_multiple_of(4).and_then(chunk) {
        Some((CHUNK_BIN, body, _)) => Some(body),
        _ => None,
    };
    Ok((crate::gltf::json::parse(json)?, bin))
}

/// Every buffer's bytes, cut to its `byteLength`.
pub(super) fn buffers<R>(doc: &Json, bin: Option<&[u8]>, resolve: &mut R) -> Result<Vec<Vec<u8>>, GltfError>
where
    R: FnMut(&str) -> Option<Vec<u8>>,
{
    let list = member(doc, "buffers");
    let mut out = Vec::with_capacity(list.len());
    for (i, b) in list.iter().enumerate() {
        let err = |why| GltfError::Buffer { index: i, why };
        let len = b
            .get("byteLength")
            .and_then(Json::as_f64)
            .filter(|n| n.fract() == 0.0 && (0.0..=MAX_BUFFER as f64).contains(n))
            .ok_or(err("no valid byteLength"))? as usize;
        let mut bytes = match b.get("uri").and_then(Json::as_str) {
            None if i == 0 => bin.ok_or(err("no GLB binary chunk"))?.to_vec(),
            None => return Err(err("no uri")),
            Some(uri) if uri.starts_with("data:") => data_uri(uri).ok_or(err("a bad data URI"))?,
            Some(uri) => resolve(&percent_decode(uri)).ok_or(err("its file could not be read"))?,
        };
        if bytes.len() < len {
            return Err(err("shorter than its byteLength"));
        }
        bytes.truncate(len);
        out.push(bytes);
    }
    Ok(out)
}

/// A `data:` URI's payload; only base64 payloads carry binary [§3.6.1.2]. Its `%XX` escapes are
/// resolved first (RFC 2397 §3).
pub(super) fn data_uri(uri: &str) -> Option<Vec<u8>> {
    let (head, body) = uri.split_once(',')?;
    head.ends_with(";base64").then(|| base64(&percent_decode(body))).flatten()
}

/// RFC 4648 §4 base64, padding optional; the URL-safe alphabet (§5) is
/// accepted too.
fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().take_while(|&c| c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// `%XX` escapes resolved; a URI is a relative path otherwise [§3.6.1.1].
pub(super) fn percent_decode(uri: &str) -> String {
    let b = uri.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let hex = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
        match (c, hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        for (enc, dec) in [("", ""), ("Zg==", "f"), ("Zm8=", "fo"), ("Zm9v", "foo"), ("Zm9vYmFy", "foobar")] {
            assert_eq!(base64(enc).unwrap(), dec.as_bytes());
        }
        assert_eq!(base64("Zm9vYg").unwrap(), b"foob");
        assert!(base64("Zm9v!").is_none());
    }

    #[test]
    fn a_data_uri_may_escape_its_base64() {
        assert_eq!(data_uri("data:application/octet-stream;base64,Pz8%2B").unwrap(), b"??>");
        assert_eq!(data_uri("data:application/octet-stream;base64,Zg%3D%3D").unwrap(), b"f");
    }

    #[test]
    fn percent_escapes_resolve() {
        assert_eq!(percent_decode("my%20mesh.bin"), "my mesh.bin");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn a_glb_splits_into_json_and_bin() {
        let json = br#"{"a":1}     "#; // space-padded to 4
        let mut glb = b"glTF".to_vec();
        glb.extend(2u32.to_le_bytes());
        glb.extend(((12 + 8 + json.len() + 8 + 4) as u32).to_le_bytes());
        glb.extend((json.len() as u32).to_le_bytes());
        glb.extend(CHUNK_JSON.to_le_bytes());
        glb.extend(json);
        glb.extend(4u32.to_le_bytes());
        glb.extend(CHUNK_BIN.to_le_bytes());
        glb.extend([1, 2, 3, 4]);
        let (doc, bin) = split(&glb).unwrap();
        assert_eq!(doc.get("a").and_then(Json::as_u32), Some(1));
        assert_eq!(bin, Some(&[1u8, 2, 3, 4][..]));
        glb[4] = 1;
        assert!(matches!(split(&glb), Err(GltfError::NotGltf(_))));
        assert!(matches!(split(&glb[..10]), Err(GltfError::NotGltf(_))));
    }
}
