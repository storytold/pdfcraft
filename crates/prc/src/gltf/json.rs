//! The JSON layer: `serde_json` values, parsed under a value cap.

pub(crate) use serde_json::Value as Json;

use super::GltfError;

/// The most JSON values (array elements, members, strings) a document may
/// hold; a larger one is refused before it is parsed.
pub const MAX_JSON_VALUES: usize = 1 << 21;

/// glTF's integer reads: a non-negative integral number below 2^32.
pub(crate) trait JsonExt {
    fn as_u32(&self) -> Option<u32>;
}

impl JsonExt for Json {
    fn as_u32(&self) -> Option<u32> {
        if let Some(n) = self.as_u64() {
            return u32::try_from(n).ok();
        }
        let f = self.as_f64()?;
        (f.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&f)).then_some(f as u32)
    }
}

/// `text` as JSON. Values are counted by their separators first, so a
/// document past [`MAX_JSON_VALUES`] costs no allocation; nesting is
/// bounded by `serde_json`'s own depth limit.
pub(crate) fn parse(text: &[u8]) -> Result<Json, GltfError> {
    let (mut in_string, mut escaped, mut values) = (false, false, 0usize);
    for &b in text {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b',' | b'[' | b'{' => values += 1,
            _ => continue,
        }
        if values > MAX_JSON_VALUES {
            return Err(GltfError::NotGltf("a JSON document with too many values"));
        }
    }
    serde_json::from_slice(text).map_err(|e| GltfError::Json(e.to_string()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn integers_read_as_u32() {
        let v = parse(br#"[1, 2.0, -1, 4294967296, 0.5, "1"]"#).unwrap();
        let got: Vec<_> = v.as_array().unwrap().iter().map(JsonExt::as_u32).collect();
        assert_eq!(got, [Some(1), Some(2), None, None, None, None]);
    }

    #[test]
    fn too_many_values_are_refused_before_parsing() {
        let mut text = b"[".to_vec();
        text.extend(std::iter::repeat_n(b"0,".as_slice(), MAX_JSON_VALUES).flatten());
        text.extend(b"0]");
        assert_eq!(parse(&text), Err(GltfError::NotGltf("a JSON document with too many values")));
        // Separators inside strings are not values.
        let quoted = format!(r#"["{}"]"#, ",".repeat(MAX_JSON_VALUES + 1));
        assert!(parse(quoted.as_bytes()).is_ok());
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(matches!(parse(b"{"), Err(GltfError::Json(_))));
    }
}
