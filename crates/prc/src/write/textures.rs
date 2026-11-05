//! The globals' texture entities: `Picture` (703), `TextureDefinition` (712)
//! and the materials array of `Material` (702) / `TextureApplication` (711)
//! pairs [WD 7.5.4-7.5.7].

use super::TextureWrap;
use super::sections::{base, ref_base};
use super::{BitWriter, MeshScene};

const MATERIAL: u32 = 702;
const PICTURE: u32 = 703;
const TEXTURE_APPLICATION: u32 = 711;
const TEXTURE_DEFINITION: u32 = 712;

/// `EPRCPictureDataFormat`, counted from 0.
pub(super) const FORMAT_PNG: u32 = 0;
pub(super) const FORMAT_JPEG: u32 = 1;
/// Mapping type "stored": the coordinates come from the tessellation.
const MAPPING_STORED: i32 = 1;
/// `PRC_TEXTURE_MAPPING_DIFFUSE`. The specification does not give its
/// value; the first bit is an assumption no viewer has confirmed here.
const MAPPING_DIFFUSE: u32 = 0x0001;
/// Components: red, green, blue, and alpha (WD 7.5.7 Table 100).
const COMPONENTS_RGB: u8 = 0x07;
const COMPONENTS_RGBA: u8 = 0x0F;
/// Texture function "modulate", 0-based: glTF multiplies the base colour
/// factor by the texel.
const FUNCTION_MODULATE: i32 = 1;
/// Applying mode: lighting.
const APPLY_LIGHTING: u8 = 0x01;

/// A picture's format, width and height, read from its file header; `None`
/// when it is neither a PNG nor a JPEG with a frame header.
pub(crate) fn picture_info(image: &[u8]) -> Option<(u32, u32, u32)> {
    if image.starts_with(b"\x89PNG\r\n\x1a\n") && image.get(12..16) == Some(b"IHDR") {
        let be = |at: usize| Some(u32::from_be_bytes(image.get(at..at + 4)?.try_into().ok()?));
        return Some((FORMAT_PNG, be(16)?, be(20)?));
    }
    jpeg_size(image).map(|(w, h)| (FORMAT_JPEG, w, h))
}

/// The width and height in a JPEG's first frame header (SOFn, ITU-T T.81
/// B.2.2).
fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
    if !b.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut at = 2;
    loop {
        // Fill bytes before a marker.
        while b.get(at) == Some(&0xFF) && b.get(at + 1) == Some(&0xFF) {
            at += 1;
        }
        if *b.get(at)? != 0xFF {
            return None;
        }
        let marker = *b.get(at + 1)?;
        if matches!(marker, 0x01 | 0xD0..=0xD7) {
            at += 2;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([*b.get(at + 2)?, *b.get(at + 3)?]));
        let sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if sof {
            let h = u16::from_be_bytes([*b.get(at + 5)?, *b.get(at + 6)?]);
            let w = u16::from_be_bytes([*b.get(at + 7)?, *b.get(at + 8)?]);
            return Some((u32::from(w), u32::from(h)));
        }
        if marker == 0xDA || len < 2 {
            return None;
        }
        at += 2 + len;
    }
}

/// The stored wrapping mode, 0-based as a reader defaults to
/// (`WrapBase::ZeroBased`): 1 repeat, 4 clamp to edge, 5 mirrored repeat.
fn wrap_code(w: TextureWrap) -> i32 {
    match w {
        TextureWrap::Repeat => 1,
        TextureWrap::Clamp => 4,
        TextureWrap::MirroredRepeat => 5,
    }
}

/// Pictures, one per texture, file `k` of the structure header for
/// texture `k`; `info` is each one's [`picture_info`].
pub(super) fn pictures(w: &mut BitWriter, info: &[(u32, u32, u32)]) {
    w.uint(info.len() as u32);
    for (k, &(format, width, height)) in info.iter().enumerate() {
        w.uint(PICTURE);
        base(w);
        w.uint(format).uint(k as u32 + 1).uint(width).uint(height);
    }
}

/// Texture definitions, one per texture over picture `k`, with unique ids
/// from `first_id`.
pub(super) fn definitions(w: &mut BitWriter, scene: &MeshScene, first_id: u32) {
    w.uint(scene.textures.len() as u32);
    for (k, t) in scene.textures.iter().enumerate() {
        w.uint(TEXTURE_DEFINITION);
        ref_base(w, None, first_id + k as u32);
        // Picture (biased), dimension 2, stored mapping, diffuse.
        w.uint(k as u32 + 1).put(2, 8).int(MAPPING_STORED).uint(MAPPING_DIFFUSE);
        // No intensities (1.0); RGB(A) components; modulate; no blend
        // sources; lighting; the S and T wraps; no transformation.
        let components = if t.alpha { COMPONENTS_RGBA } else { COMPONENTS_RGB };
        w.uint(0).uint(1).put(u64::from(components), 8);
        w.int(FUNCTION_MODULATE).int(0).int(0);
        w.put(u64::from(APPLY_LIGHTING), 8);
        w.int(wrap_code(t.wrap[0])).int(wrap_code(t.wrap[1]));
        w.bit(false);
    }
}

/// The materials array: per textured material, in `textured` order, its
/// base `Material` then the `TextureApplication` naming it, with unique ids
/// from `first_id`. `textured` lists the textured materials' indices.
pub(super) fn materials(w: &mut BitWriter, scene: &MeshScene, textured: &[usize], first_id: u32) {
    w.uint(2 * textured.len() as u32);
    let mut id = first_id;
    for (j, &m) in textured.iter().enumerate() {
        let Some(mat) = scene.materials.get(m) else {
            continue;
        };
        let alpha = mat.rgba[3].clamp(0.0, 1.0);
        // Double-scaled colour index of material m's colour (WD 7.5.3).
        let colour = 3 * m as u32 + 1;
        w.uint(MATERIAL);
        ref_base(w, None, id);
        // Ambient and diffuse the base colour; no emissive or specular.
        w.uint(colour).uint(colour).uint(0).uint(0);
        w.double(0.0).doubles([alpha; 4]);
        w.uint(TEXTURE_APPLICATION);
        ref_base(w, None, id + 1);
        let texture = mat.texture.map_or(0, |t| t as u32 + 1);
        // Base material (biased entry), definition, no next level, UV set 1.
        w.uint(2 * j as u32 + 1).uint(texture).uint(0).uint(1);
        id += 2;
    }
}

#[cfg(test)]
// Test code: a panic is the failure report.
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn png_and_jpeg_sizes_are_read_from_their_headers() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend([0, 0, 1, 2, 0, 0, 0, 3]);
        assert_eq!(picture_info(&png), Some((FORMAT_PNG, 258, 3)));
        // SOI, an APP0 of 4 bytes, fill, SOF2 (progressive) 8-bit 5 x 7.
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xFF, 0xC2, 0, 11, 8, 0, 5, 0, 7];
        assert_eq!(picture_info(&jpeg), Some((FORMAT_JPEG, 7, 5)));
        // A DHT (C4) is not a frame header; scan data before a frame is not
        // read through.
        let no_frame = [0xFF, 0xD8, 0xFF, 0xC4, 0, 2, 0xFF, 0xDA, 0, 2];
        assert_eq!(picture_info(&no_frame), None);
        assert_eq!(picture_info(b"RIFF....WEBP"), None);
        assert_eq!(picture_info(&png[..20]), None, "a cut IHDR");
    }
}
