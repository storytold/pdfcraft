//! 3D model tools (Acrobat: Rich Media ▸ Add 3D, AGENTS.md §3).

use std::path::{Component, Path};

use pdfcraft_engine::{Activation3d, Edit, MAX_3D_MODEL_BYTES, MarkFile, Model3dFormat, View3d};
use pdfcraft_prc::{GltfOptions, read_gltf, write_prc};
use serde_json::{Value, json};

use crate::comments::rect_to_user;
use crate::{Args, Automation, Result, ToolError, failed};

/// The largest side of a 3D model's box, in points: the largest page PDF allows (ISO 32000-2 Annex C).
const MAX_MODEL_SIDE: f64 = 14_400.0;

impl Automation {
    /// A U3D or PRC model, or a glTF scene converted to PRC, in `rect` on `page`, its poster the
    /// picture at `poster` (a PDF page or an image) fitted inside the box, or a labelled frame. A
    /// converted scene gets a lit default view fitted to it, since it carries no lights or views
    /// of its own; a U3D or PRC file keeps its own.
    pub(crate) fn model_add(&mut self, a: &Args) -> Result<Value> {
        let page = self.page(a)?;
        let info = self.doc(a)?.info.pages.get(page).cloned().ok_or_else(|| ToolError::InvalidArgs(format!("page {} does not exist", page + 1)))?;
        let r = a.nums::<4>("rect")?.ok_or_else(|| ToolError::InvalidArgs("missing argument rect".into()))?;
        let (w, h) = ((r[2] - r[0]).abs(), (r[3] - r[1]).abs());
        if !((1.0..=MAX_MODEL_SIDE).contains(&w) && (1.0..=MAX_MODEL_SIDE).contains(&h)) {
            return Err(ToolError::InvalidArgs(format!(
                "rect must be [x0, y0, x1, y1] with each side at least 1 pt and at most {MAX_MODEL_SIDE} pt"
            )));
        }
        let activation = match a.opt_str("activation")? {
            None => Activation3d::default(),
            Some(word) => Activation3d::ALL
                .iter()
                .find(|(_, w)| *w == word)
                .map(|(act, _)| *act)
                .ok_or_else(|| ToolError::InvalidArgs(format!("activation must be one of: {}", Activation3d::ALL.map(|(_, w)| w).join(", "))))?,
        };
        let path = self.resolve(a.str("path")?, false)?;
        let (model, converted, view) = match read_model(&path, MAX_3D_MODEL_BYTES)? {
            ModelFile::Native(bytes) => (bytes, None, None),
            ModelFile::Gltf(bytes) => {
                let (prc, report, bounds) = self.gltf_to_prc(&path, &bytes)?;
                (prc, Some(report), bounds.and_then(|b| View3d::fitted(b, w / h)))
            }
        };
        let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let name = a.opt_str("name")?.map(str::to_owned).unwrap_or_else(|| file.rsplit_once('.').map_or(file.clone(), |(s, _)| s.to_owned()));
        let poster = match a.opt_str("poster")? {
            None => None,
            Some(p) => {
                let path = self.resolve(p, false)?;
                let bytes = crate::read_input(&path)?;
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let page = usize::try_from(a.opt_int("poster_page")?.unwrap_or(1).max(1)).unwrap_or(1).saturating_sub(1);
                Some(MarkFile { name, bytes: std::sync::Arc::new(bytes), page })
            }
        };
        let edit = Edit::AddModel3d { page, rect: rect_to_user(&info, r), model: std::sync::Arc::new(model), poster, activation, name, view };
        let mut out = self.apply(a, edit)?;
        if let Some(report) = converted {
            out["converted"] = report;
        }
        Ok(out)
    }

    /// The glTF file at `path` (its bytes `data`) written as PRC, what the conversion reports, and
    /// the box the scene fills.
    /// External buffers and images are read only from relative paths below the file's folder,
    /// through the automation roots, at most [`MAX_3D_MODEL_BYTES`] in all.
    fn gltf_to_prc(&self, path: &Path, data: &[u8]) -> Result<(Vec<u8>, Value, Option<Bounds>)> {
        let folder = path.parent().unwrap_or(Path::new(""));
        let mut left = MAX_3D_MODEL_BYTES;
        let mut resolve = |uri: &str| -> Option<Vec<u8>> {
            let mut joined = folder.to_path_buf();
            for part in Path::new(uri).components() {
                match part {
                    Component::Normal(name) if !uri.contains(':') => joined.push(name),
                    Component::CurDir => {}
                    _ => return None,
                }
            }
            let target = self.resolve(joined.to_str()?, false).ok()?;
            let bytes = read_at_most(&target, left)?;
            left = left.saturating_sub(bytes.len());
            Some(bytes)
        };
        let shown = path.display();
        let import = read_gltf(data, &mut resolve, GltfOptions::default()).map_err(|e| failed(format!("{shown}: {e}")))?;
        let report = write_prc(&import.scene).map_err(|e| failed(format!("{shown}: cannot convert to PRC: {e}")))?;
        if report.prc.len() > MAX_3D_MODEL_BYTES {
            return Err(failed(format!("{shown}: the converted model is more than {MAX_3D_MODEL_BYTES} bytes, the largest accepted")));
        }
        let summary = json!({
            "from": "glTF",
            "triangles": report.triangles,
            "textured_primitives": report.textured,
            "notes": import.notes,
            "inferred": import.inferred,
        });
        Ok((report.prc, summary, import.scene.bounds()))
    }
}

/// An axis-aligned box `[min, max]` in model space.
type Bounds = [[f64; 3]; 2];

/// A model file as read: embedded as it is (U3D, PRC), or a glTF scene to convert.
#[derive(Debug)]
enum ModelFile {
    Native(Vec<u8>),
    Gltf(Vec<u8>),
}

/// Whether a model file whose first bytes are `head` is glTF: the GLB magic, or a JSON text (a `{`
/// after spaces, or a byte-order mark).
fn is_gltf(head: &[u8]) -> bool {
    head.starts_with(b"glTF") || head.starts_with(b"\xEF\xBB\xBF") || head.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{')
}

/// The file at `path` when it holds at most `max` bytes.
fn read_at_most(path: &Path, max: usize) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).ok()?;
    let cap = u64::try_from(max).unwrap_or(u64::MAX);
    if file.metadata().is_ok_and(|m| m.is_file() && m.len() > cap) {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(cap.saturating_add(1)).read_to_end(&mut bytes).ok()?;
    (bytes.len() <= max).then_some(bytes)
}

/// Read a resolved model file of at most `max` bytes. Its size (from the file system) and its first
/// bytes are checked before the rest is read, so a large file that is not a model is never loaded.
fn read_model(path: &Path, max: usize) -> Result<ModelFile> {
    use std::io::Read as _;
    let io = |e: std::io::Error| failed(format!("{}: {e}", path.display()));
    let too_large = |size: u64| failed(format!("the 3D model is {size} bytes; the largest accepted is {max}"));
    let cap = u64::try_from(max).unwrap_or(u64::MAX);
    let mut file = std::fs::File::open(path).map_err(io)?;
    if let Ok(meta) = file.metadata()
        && meta.is_file()
        && meta.len() > cap
    {
        return Err(too_large(meta.len()));
    }
    let mut bytes = Vec::new();
    (&mut file).take(4).read_to_end(&mut bytes).map_err(io)?;
    let gltf = is_gltf(&bytes);
    if !gltf {
        Model3dFormat::detect(&bytes).map_err(failed)?;
    }
    let rest = cap.saturating_add(1).saturating_sub(bytes.len() as u64);
    file.take(rest).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() > max {
        return Err(failed(format!("the 3D model is more than {max} bytes, the largest accepted")));
    }
    Ok(if gltf { ModelFile::Gltf(bytes) } else { ModelFile::Native(bytes) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn native(m: ModelFile) -> Vec<u8> {
        match m {
            ModelFile::Native(b) => b,
            ModelFile::Gltf(_) => panic!("read as glTF"),
        }
    }

    fn file(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_model_is_read_whole_when_it_fits() {
        let dir = tempfile::tempdir().unwrap();
        let mut prc = b"PRC".to_vec();
        prc.extend_from_slice(&[7; 40]);
        assert_eq!(native(read_model(&file(dir.path(), "m.prc", &prc), 64).unwrap()), prc);
        assert_eq!(native(read_model(&file(dir.path(), "m.u3d", b"U3D\0x"), 6).unwrap()), b"U3D\0x");
    }

    #[test]
    fn a_model_past_the_cap_is_refused_by_its_size_before_it_is_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "big.prc", &[b'P', b'R', b'C', 0, 0, 0, 0, 0, 0, 0]);
        let err = read_model(&path, 4).unwrap_err().to_string();
        assert!(err.contains("is 10 bytes") && err.contains("largest accepted is 4"), "the file's own size, from the file system: {err}");
    }

    #[test]
    fn a_file_that_is_not_a_model_is_refused_by_its_header() {
        let dir = tempfile::tempdir().unwrap();
        for junk in [&b"%PDF-1.7 a page"[..], b"PR", b"", b"U3DX and more"] {
            let err = read_model(&file(dir.path(), "junk.bin", junk), 64).unwrap_err().to_string();
            assert!(err.contains("not a 3D model"), "{junk:?}: {err}");
        }
    }

    #[test]
    fn gltf_is_known_by_its_magic_or_its_brace() {
        assert!(is_gltf(b"glTF"));
        assert!(is_gltf(b" \n{\""));
        assert!(is_gltf(b"\xEF\xBB\xBF{"));
        assert!(!is_gltf(b"PRC\0") && !is_gltf(b"U3D\0") && !is_gltf(b"   ") && !is_gltf(b""));
    }
}
