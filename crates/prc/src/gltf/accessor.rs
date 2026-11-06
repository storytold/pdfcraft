//! Reading accessors: typed views into buffer bytes [glTF 2.0 §3.6.2,
//! §5.1].

use super::json::{Json, JsonExt};

use std::cell::Cell;

use super::member;

/// The most accessor components one import reads, all accessors together:
/// several primitives naming one large accessor each pay for it.
pub const MAX_GLTF_VALUES: usize = 1 << 24;

/// The file's buffers and what is left of the import's value budget.
pub(super) struct Buffers {
    data: Vec<Vec<u8>>,
    left: Cell<usize>,
}

impl Buffers {
    pub(super) fn new(data: Vec<Vec<u8>>) -> Self {
        Self { data, left: Cell::new(MAX_GLTF_VALUES) }
    }

    pub(super) fn data(&self) -> &[Vec<u8>] {
        &self.data
    }

    /// Take `n` values from the budget; `false`, taking none, when fewer remain.
    fn take(&self, n: usize) -> bool {
        match self.left.get().checked_sub(n) {
            Some(rest) => {
                self.left.set(rest);
                true
            }
            None => false,
        }
    }
}

/// Why an accessor could not be read; the primitive using it is skipped
/// with this reason in a note.
pub(super) type Refusal = String;

fn uint(v: &Json, key: &str) -> Option<usize> {
    v.get(key).and_then(Json::as_u32).map(|n| n as usize)
}

/// Bytes per component of a `componentType` [§3.6.2.2].
fn component_size(kind: u32) -> Option<usize> {
    match kind {
        5120 | 5121 => Some(1),
        5122 | 5123 => Some(2),
        5125 | 5126 => Some(4),
        _ => None,
    }
}

/// Components per element of an accessor `type`.
fn components(kind: &str) -> Option<usize> {
    Some(match kind {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" | "MAT2" => 4,
        "MAT3" => 9,
        "MAT4" => 16,
        _ => return None,
    })
}

/// One component as a number; `normalized` integers map to 0..1 (or
/// -1..1) [§3.11, "Normalized integers"].
fn component(b: &[u8], kind: u32, normalized: bool) -> f64 {
    let at = |n: usize| b.get(..n).unwrap_or(&[]);
    match (kind, normalized) {
        (5126, _) => at(4).try_into().map_or(0.0, |a| f64::from(f32::from_le_bytes(a))),
        (5125, _) => at(4).try_into().map_or(0.0, |a| f64::from(u32::from_le_bytes(a))),
        (5123, n) => {
            let v = at(2).try_into().map_or(0, u16::from_le_bytes);
            if n { f64::from(v) / 65535.0 } else { f64::from(v) }
        }
        (5122, n) => {
            let v = at(2).try_into().map_or(0, i16::from_le_bytes);
            if n { (f64::from(v) / 32767.0).max(-1.0) } else { f64::from(v) }
        }
        (5121, n) => {
            let v = b.first().copied().unwrap_or(0);
            if n { f64::from(v) / 255.0 } else { f64::from(v) }
        }
        (_, n) => {
            let v = b.first().map_or(0, |&v| v as i8);
            if n { (f64::from(v) / 127.0).max(-1.0) } else { f64::from(v) }
        }
    }
}

/// Accessor `index`'s elements, flattened, after checking it has `want`
/// components per element.
pub(super) fn read(doc: &Json, buffers: &Buffers, index: usize, want: usize) -> Result<Vec<f64>, Refusal> {
    let acc = member(doc, "accessors").get(index).ok_or_else(|| format!("accessor {index} does not exist"))?;
    let kind = acc.get("componentType").and_then(Json::as_u32).unwrap_or(0);
    let size = component_size(kind).ok_or_else(|| format!("accessor {index} has an unknown componentType"))?;
    let n = acc.get("type").and_then(Json::as_str).and_then(components);
    if n != Some(want) {
        return Err(format!("accessor {index} is not {want}-component"));
    }
    if acc.get("sparse").is_some() {
        return Err(format!("accessor {index} is sparse, which is not read"));
    }
    let count = uint(acc, "count").ok_or_else(|| format!("accessor {index} has no count"))?;
    let view =
        uint(acc, "bufferView").and_then(|v| member(doc, "bufferViews").get(v)).ok_or_else(|| format!("accessor {index} has no buffer view"))?;
    let bytes = view_bytes(view, buffers.data()).ok_or_else(|| format!("accessor {index}'s buffer view is out of range"))?;
    let elem = size * want;
    let stride = uint(view, "byteStride").filter(|&s| s > 0).unwrap_or(elem);
    let start = uint(acc, "byteOffset").unwrap_or(0);
    let end = count.checked_sub(1).map_or(Some(start), |last| last.checked_mul(stride)?.checked_add(start)?.checked_add(elem));
    if stride < elem || end.is_none_or(|e| e > bytes.len()) {
        return Err(format!("accessor {index} runs past its buffer view"));
    }
    let normalized = acc.get("normalized").and_then(Json::as_bool).unwrap_or(false);
    // The bounds check above makes `count * want` at most the view's length.
    if !buffers.take(count * want) {
        return Err(format!("accessor {index} is past the import's {MAX_GLTF_VALUES}-value budget"));
    }
    let mut out = Vec::with_capacity(count * want);
    for i in 0..count {
        let at = start + i * stride;
        for c in 0..want {
            let b = bytes.get(at + c * size..).unwrap_or(&[]);
            out.push(component(b, kind, normalized));
        }
    }
    // Refused here, so only the primitive that reads it is lost.
    if out.iter().any(|v| !v.is_finite()) {
        return Err(format!("accessor {index} holds a number that is not finite"));
    }
    Ok(out)
}

/// A buffer view's bytes; `None` when it runs past its buffer.
pub(super) fn view_bytes<'a>(view: &Json, buffers: &'a [Vec<u8>]) -> Option<&'a [u8]> {
    let buffer = buffers.get(uint(view, "buffer")?)?;
    let start = uint(view, "byteOffset").unwrap_or(0);
    buffer.get(start..start.checked_add(uint(view, "byteLength")?)?)
}

/// Accessor `index` as triangle-corner indices: integral and non-negative.
pub(super) fn indices(doc: &Json, buffers: &Buffers, index: usize) -> Result<Vec<u32>, Refusal> {
    read(doc, buffers, index, 1)?
        .into_iter()
        .map(|v| {
            (v.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&v))
                .then_some(v as u32)
                .ok_or_else(|| format!("accessor {index} holds a non-integer index"))
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn doc(acc: &str, view: &str) -> Json {
        let text = format!(r#"{{"accessors":[{acc}],"bufferViews":[{view}]}}"#);
        crate::gltf::json::parse(text.as_bytes()).unwrap()
    }

    #[test]
    fn strided_floats_and_normalized_bytes_read() {
        let mut buf = Vec::new();
        for v in [1.0f32, 2.0, 3.0] {
            buf.extend(v.to_le_bytes());
        }
        buf.extend([0u8; 4]);
        for v in [4.0f32, 5.0, 6.0] {
            buf.extend(v.to_le_bytes());
        }
        let d = doc(r#"{"bufferView":0,"componentType":5126,"count":2,"type":"VEC3"}"#, r#"{"buffer":0,"byteLength":28,"byteStride":16}"#);
        assert_eq!(read(&d, &Buffers::new(vec![buf]), 0, 3).unwrap(), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let d = doc(r#"{"bufferView":0,"componentType":5121,"normalized":true,"count":2,"type":"SCALAR"}"#, r#"{"buffer":0,"byteLength":2}"#);
        assert_eq!(read(&d, &Buffers::new(vec![vec![255, 0]]), 0, 1).unwrap(), [1.0, 0.0]);
    }

    #[test]
    fn bad_accessors_are_refused_not_read() {
        let view = r#"{"buffer":0,"byteLength":4}"#;
        let short = doc(r#"{"bufferView":0,"componentType":5126,"count":2,"type":"SCALAR"}"#, view);
        assert!(read(&short, &Buffers::new(vec![vec![0; 4]]), 0, 1).is_err());
        let wrong = doc(r#"{"bufferView":0,"componentType":5126,"count":1,"type":"VEC2"}"#, view);
        assert!(read(&wrong, &Buffers::new(vec![vec![0; 4]]), 0, 1).is_err());
        let huge = doc(r#"{"bufferView":0,"componentType":5126,"count":4294967295,"type":"MAT4"}"#, view);
        assert!(read(&huge, &Buffers::new(vec![vec![0; 4]]), 0, 16).is_err());
        let sparse = doc(r#"{"bufferView":0,"componentType":5126,"count":1,"type":"SCALAR","sparse":{}}"#, view);
        assert!(read(&sparse, &Buffers::new(vec![vec![0; 4]]), 0, 1).is_err());
        let ints = doc(r#"{"bufferView":0,"componentType":5126,"count":1,"type":"SCALAR"}"#, view);
        assert!(indices(&ints, &Buffers::new(vec![0.5f32.to_le_bytes().to_vec()]), 0).is_err());
    }

    #[test]
    fn rereading_one_accessor_spends_the_shared_budget() {
        let d = doc(r#"{"bufferView":0,"componentType":5121,"count":4,"type":"SCALAR"}"#, r#"{"buffer":0,"byteLength":4}"#);
        let buffers = Buffers::new(vec![vec![0; 4]]);
        buffers.left.set(10);
        assert!(read(&d, &buffers, 0, 1).is_ok());
        assert!(read(&d, &buffers, 0, 1).is_ok());
        let third = read(&d, &buffers, 0, 1).unwrap_err();
        assert!(third.contains("budget"), "{third}");
    }
}
