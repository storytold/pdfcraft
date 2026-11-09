//! glTF 2.0 import (Khronos glTF 2.0 specification, also ISO/IEC 12113):
//! a `.gltf` JSON text or a `.glb` binary as a [`MeshScene`] for
//! [`crate::write_prc`].
//!
//! Triangles, normals, the node tree with its names and placements, each
//! material's base colour, emissive colour and opacity, and its PNG or JPEG base-colour
//! texture over its coordinates (`KHR_texture_transform` applied) carry
//! over. Other texture kinds, lines, points, skins and morph targets do
//! not; each loss is a note, and each guess (an alpha mask drawn as a
//! whole) an inference. Colour and up-axis handling are settings because
//! files differ.

mod accessor;
mod build;
mod container;
mod json;
mod textures;

pub use accessor::MAX_GLTF_VALUES;
pub use json::MAX_JSON_VALUES;
pub use textures::MAX_IMAGE_BYTES;

use json::Json;

use crate::MeshScene;

/// How material base colours are read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GltfColours {
    /// As glTF defines them: linear, encoded to sRGB for display
    /// (IEC 61966-2-1 transfer function only, no profile conversion).
    #[default]
    Linear,
    /// Used unchanged, for files that store sRGB values against the spec.
    AsStored,
}

/// Which way is up in the written scene.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum GltfUpAxis {
    /// glTF's +Y up turned to +Z up, the convention of CAD and 3D PDF
    /// viewers.
    #[default]
    ZUp,
    /// The file's coordinates unchanged.
    AsStored,
}

/// Settings for [`read_gltf`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct GltfOptions {
    /// Base-colour handling.
    pub colours: GltfColours,
    /// Up-axis handling.
    pub up_axis: GltfUpAxis,
}

/// A read glTF scene and what was lost or guessed reading it.
#[derive(Debug, Clone, PartialEq)]
pub struct GltfImport {
    /// The scene, in metres (`units_in_mm` 1000).
    pub scene: MeshScene,
    /// What was dropped or skipped, one sentence each.
    pub notes: Vec<String>,
    /// What was inferred rather than read, one sentence each.
    pub inferred: Vec<String>,
}

/// Why a glTF file could not be read at all. Problems confined to one
/// primitive are notes, not errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GltfError {
    /// Not a glTF 2.0 file, and why.
    #[error("not a glTF 2.0 file: {0}")]
    NotGltf(&'static str),
    /// The JSON is malformed, and where.
    #[error("malformed JSON: {0}")]
    Json(String),
    /// The file requires an extension this reader does not implement.
    #[error("the file requires the unsupported extension {0}")]
    RequiredExtension(String),
    /// Buffer `index` could not be loaded.
    #[error("buffer {index}: {why}")]
    Buffer {
        /// The buffer's index.
        index: usize,
        /// What was wrong.
        why: &'static str,
    },
}

/// Required extensions this reader honours.
const SUPPORTED_EXTENSIONS: &[&str] = &["KHR_mesh_quantization", "KHR_texture_transform"];

/// Read a glTF 2.0 file.
///
/// `resolve` is given each external buffer's or image's decoded relative
/// URI and returns its bytes, or `None` when it cannot; the engine itself
/// opens no files. Embedded (`data:`) and GLB buffers and images need no
/// resolver. An image that cannot be read is a note, not an error.
///
/// # Errors
///
/// [`GltfError`] when the container, JSON, version or a buffer is bad, or
/// a required extension is unsupported.
///
/// # Example
///
/// ```
/// use pdfcraft_prc::{GltfOptions, read_gltf, write_prc};
///
/// // One triangle: three float positions in an embedded buffer.
/// let floats: Vec<u8> = [0f32, 0., 0., 1., 0., 0., 0., 1., 0.]
///     .iter().flat_map(|f| f.to_le_bytes()).collect();
/// let b64 = "AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAA";
/// assert_eq!(floats.len(), 36);
/// let gltf = format!(r#"{{"asset":{{"version":"2.0"}},
///   "buffers":[{{"byteLength":36,"uri":"data:application/octet-stream;base64,{b64}"}}],
///   "bufferViews":[{{"buffer":0,"byteLength":36}}],
///   "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}}],
///   "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}}}}]}}],
///   "nodes":[{{"name":"tri","mesh":0}}], "scenes":[{{"nodes":[0]}}]}}"#);
/// let import = read_gltf(gltf.as_bytes(), &mut |_| None, GltfOptions::default())?;
/// assert_eq!(write_prc(&import.scene).unwrap().triangles, 1);
/// # Ok::<(), pdfcraft_prc::GltfError>(())
/// ```
pub fn read_gltf<R>(data: &[u8], resolve: &mut R, options: GltfOptions) -> Result<GltfImport, GltfError>
where
    R: FnMut(&str) -> Option<Vec<u8>>,
{
    let (doc, bin) = container::split(data)?;
    let version = doc.get("asset").and_then(|a| a.get("version")).and_then(Json::as_str).ok_or(GltfError::NotGltf("no asset.version"))?;
    if version.split('.').next() != Some("2") {
        return Err(GltfError::NotGltf("a version other than 2"));
    }
    // A file that needs a later 2.x reader names it [§3.2]; this one reads 2.0.
    if let Some(min) = doc.get("asset").and_then(|a| a.get("minVersion")).and_then(Json::as_str)
        && min != "2.0"
    {
        return Err(GltfError::NotGltf("a minVersion other than 2.0"));
    }
    if let Some(ext) = member(&doc, "extensionsRequired").iter().filter_map(Json::as_str).find(|e| !SUPPORTED_EXTENSIONS.contains(e)) {
        return Err(GltfError::RequiredExtension(ext.to_owned()));
    }
    let buffers = accessor::Buffers::new(container::buffers(&doc, bin, resolve)?);
    let images = textures::images(&doc, buffers.data(), resolve);
    Ok(build::scene(&doc, &buffers, &images, options))
}

/// `v[key]` as an array; empty when missing or not an array.
fn member<'a>(v: &'a Json, key: &str) -> &'a [Json] {
    v.get(key).and_then(Json::as_array).map_or(&[], Vec::as_slice)
}

#[cfg(test)]
mod tests;
