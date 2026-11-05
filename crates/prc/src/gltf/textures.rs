//! glTF base-colour textures as [`SceneTexture`]s, and the coordinates
//! that draw them [glTF 2.0 §3.8, §3.9.2; `KHR_texture_transform`].

use super::json::{Json, JsonExt};

use super::accessor::{self, Refusal};
use super::container::{data_uri, percent_decode};
use super::{GltfImport, member};
use crate::{SceneTexture, TextureWrap};

/// The most image bytes one import loads, all images together, and
/// separately the most it copies into the scene's textures.
pub const MAX_IMAGE_BYTES: usize = 1 << 28;

/// Each image's file bytes, or why it could not be loaded.
pub(super) type Images = Vec<Result<Vec<u8>, &'static str>>;

/// Every image, from a `data:` URI, the resolver or a buffer view
/// [§3.8.2]. A failure is held for the materials that use the image.
pub(super) fn images<R>(doc: &Json, buffers: &[Vec<u8>], resolve: &mut R) -> Images
where
    R: FnMut(&str) -> Option<Vec<u8>>,
{
    let views = member(doc, "bufferViews");
    let image = |im: &Json, resolve: &mut R| match im.get("uri").and_then(Json::as_str) {
        Some(uri) if uri.starts_with("data:") => data_uri(uri).ok_or("a bad data URI"),
        Some(uri) => resolve(&percent_decode(uri)).ok_or("its file could not be read"),
        None => im
            .get("bufferView")
            .and_then(Json::as_u32)
            .and_then(|v| views.get(v as usize))
            .and_then(|v| accessor::view_bytes(v, buffers))
            .map(<[u8]>::to_vec)
            .ok_or("its buffer view is out of range"),
    };
    let mut left = MAX_IMAGE_BYTES;
    member(doc, "images").iter().map(|im| image(im, resolve).and_then(|b| checked(b, &mut left))).collect()
}

fn checked(b: Vec<u8>, left: &mut usize) -> Result<Vec<u8>, &'static str> {
    *left = left.checked_sub(b.len()).ok_or("it is past the import's image budget")?;
    match crate::write::picture_info(&b) {
        Some(_) => Ok(b),
        None => Err("it is not a PNG or JPEG file"),
    }
}

/// How one material draws its base-colour texture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TextureUse {
    /// The `TEXCOORD_n` set read.
    pub(super) set: usize,
    /// glTF (u, v) to picture (u, v): `u' = m[0]·(u, v, 1)`, `v'` from
    /// `m[1]`.
    pub(super) matrix: [[f64; 3]; 2],
}

/// The glTF texture-to-scene map: one [`SceneTexture`] per (texture,
/// alpha) pair a material uses.
#[derive(Debug, Default)]
pub(super) struct Textures {
    seen: Vec<((usize, bool), usize)>,
    /// Image bytes copied into the scene so far.
    copied: usize,
}

impl Textures {
    /// Material `label`'s base-colour texture `info`, added to the scene;
    /// `alpha` when the material blends. `None`, with a note, when it
    /// cannot be drawn.
    pub(super) fn base_colour(
        &mut self,
        doc: &Json,
        images: &Images,
        info: &Json,
        (label, alpha): (&str, bool),
        out: &mut GltfImport,
    ) -> Option<(usize, TextureUse)> {
        let drawn = |why: &str| format!("{label}: base colour texture {why}; drawn in its base colour");
        let Some(t) = info.get("index").and_then(Json::as_u32).map(|t| t as usize) else {
            out.notes.push(drawn("names no texture"));
            return None;
        };
        let Some(tex) = member(doc, "textures").get(t) else {
            out.notes.push(drawn("does not exist"));
            return None;
        };
        let Some(source) = tex.get("source").and_then(Json::as_u32) else {
            out.notes.push(drawn("uses an image format this reader does not read (WebP, KTX2)"));
            return None;
        };
        let image = match images.get(source as usize) {
            Some(Ok(b)) => b,
            Some(Err(why)) => {
                out.notes.push(drawn(&format!("dropped: {why}")));
                return None;
            }
            None => {
                out.notes.push(drawn("names no image"));
                return None;
            }
        };
        let k = match self.seen.iter().find(|(key, _)| *key == (t, alpha)) {
            Some(&(_, k)) => k,
            None => {
                match self.copied.checked_add(image.len()) {
                    Some(n) if n <= MAX_IMAGE_BYTES => self.copied = n,
                    _ => {
                        out.notes.push(drawn("dropped: it is past the import's image budget"));
                        return None;
                    }
                }
                let k = out.scene.textures.len();
                out.scene.textures.push(SceneTexture { image: image.clone(), wrap: wraps(doc, tex), alpha });
                self.seen.push(((t, alpha), k));
                k
            }
        };
        Some((k, uses(info)))
    }
}

/// A sampler's wrap modes along u and v; repeat when unset [§3.8.4].
fn wraps(doc: &Json, tex: &Json) -> [TextureWrap; 2] {
    let sampler = tex.get("sampler").and_then(Json::as_u32).and_then(|s| member(doc, "samplers").get(s as usize));
    let wrap = |key| match sampler.and_then(|s| s.get(key)).and_then(Json::as_u32) {
        Some(33071) => TextureWrap::Clamp,
        Some(33648) => TextureWrap::MirroredRepeat,
        _ => TextureWrap::Repeat,
    };
    [wrap("wrapS"), wrap("wrapT")]
}

/// The coordinate set and `KHR_texture_transform` of a texture info: the
/// matrix `T · R · S`, rotation counter-clockwise in glTF's top-left
/// (u, v) space.
fn uses(info: &Json) -> TextureUse {
    let set = |v: &Json| v.get("texCoord").and_then(Json::as_u32).map(|s| s as usize);
    let tt = info.get("extensions").and_then(|e| e.get("KHR_texture_transform"));
    let pair = |key, default: [f64; 2]| {
        let a = tt.and_then(|t| t.get(key)).and_then(Json::as_array);
        match a.map(|a| a.iter().map(Json::as_f64).collect::<Option<Vec<_>>>()) {
            Some(Some(v)) => match v[..] {
                [a, b] => [a, b],
                _ => default,
            },
            _ => default,
        }
    };
    let [ox, oy] = pair("offset", [0.0, 0.0]);
    let [sx, sy] = pair("scale", [1.0, 1.0]);
    let r = tt.and_then(|t| t.get("rotation")).and_then(Json::as_f64).unwrap_or(0.0);
    let (sin, cos) = r.sin_cos();
    TextureUse { set: tt.and_then(set).or_else(|| set(info)).unwrap_or(0), matrix: [[cos * sx, sin * sy, ox], [-sin * sx, cos * sy, oy]] }
}

/// A primitive's `TEXCOORD_n` through `using`, flipped from glTF's
/// top-left origin to the scene's bottom-left; `count` is its vertex
/// count.
pub(super) fn coordinates(doc: &Json, buffers: &accessor::Buffers, attrs: &Json, using: TextureUse, count: usize) -> Result<Vec<[f64; 2]>, Refusal> {
    let key = format!("TEXCOORD_{}", using.set);
    let Some(a) = attrs.get(&key).and_then(Json::as_u32) else {
        return Err(format!("it has no {key}"));
    };
    let flat = accessor::read(doc, buffers, a as usize, 2)?;
    if flat.len() != 2 * count {
        return Err(format!("its {key} and POSITION counts differ"));
    }
    let [m0, m1] = using.matrix;
    Ok(flat
        .chunks_exact(2)
        .filter_map(|c| match *c {
            [u, v] => Some((u, v)),
            _ => None,
        })
        .map(|(u, v)| {
            let tu = m0[0] * u + m0[1] * v + m0[2];
            let tv = m1[0] * u + m1[1] * v + m1[2];
            [tu, 1.0 - tv]
        })
        .collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_texture_transform_is_offset_rotation_scale() {
        let info = crate::gltf::json::parse(
            br#"{"index":0,"texCoord":0,"extensions":{"KHR_texture_transform":
                {"offset":[0.5,0.25],"rotation":1.5707963267948966,"scale":[2,3],"texCoord":1}}}"#,
        )
        .unwrap();
        let u = uses(&info);
        assert_eq!(u.set, 1, "the extension's texCoord wins");
        // (1, 0): scaled to (2, 0), turned to (0, -2), offset to (0.5, -1.75).
        let [m0, m1] = u.matrix;
        assert!((m0[0] + m0[2] - 0.5).abs() < 1e-12);
        assert!((m1[0] + m1[2] + 1.75).abs() < 1e-12);
        let plain = uses(&crate::gltf::json::parse(br#"{"index":0}"#).unwrap());
        assert_eq!(plain.set, 0);
        assert_eq!(plain.matrix, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    }

    #[test]
    fn images_share_one_byte_budget() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x01\0\0\0\x02".to_vec();
        let mut left = 2 * png.len() + 3;
        assert!(checked(png.clone(), &mut left).is_ok());
        assert!(checked(png.clone(), &mut left).is_ok());
        assert_eq!(checked(png.clone(), &mut left), Err("it is past the import's image budget"));
        assert_eq!(left, 3, "a refused image spends nothing");
        assert_eq!(checked(b"GI".to_vec(), &mut left), Err("it is not a PNG or JPEG file"));
    }
}
