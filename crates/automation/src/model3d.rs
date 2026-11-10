//! 3D model tools (Acrobat: Rich Media ▸ Add 3D, AGENTS.md §3).

use std::path::Path;

use pdfcraft_engine::{Activation3d, Edit, MAX_3D_MODEL_BYTES, MarkFile, Model3dFormat};
use serde_json::Value;

use crate::comments::rect_to_user;
use crate::{Args, Automation, Result, ToolError, failed};

/// The largest side of a 3D model's box, in points: the largest page PDF allows (ISO 32000-2 Annex C).
const MAX_MODEL_SIDE: f64 = 14_400.0;

impl Automation {
    /// A U3D or PRC model in `rect` on `page`, its poster the picture at `poster` (a PDF page or an
    /// image) fitted inside the box, or a labelled frame.
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
        let model = read_model(&path, MAX_3D_MODEL_BYTES)?;
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
        let edit = Edit::AddModel3d { page, rect: rect_to_user(&info, r), model: std::sync::Arc::new(model), poster, activation, name };
        self.apply(a, edit)
    }
}

/// Read a resolved model file of at most `max` bytes. Its size (from the file system) and its first
/// bytes are checked before the rest is read, so a large file that is not a model is never loaded.
fn read_model(path: &Path, max: usize) -> Result<Vec<u8>> {
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
    Model3dFormat::detect(&bytes).map_err(failed)?;
    let rest = cap.saturating_add(1).saturating_sub(bytes.len() as u64);
    file.take(rest).read_to_end(&mut bytes).map_err(io)?;
    if bytes.len() > max {
        return Err(failed(format!("the 3D model is more than {max} bytes, the largest accepted")));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(read_model(&file(dir.path(), "m.prc", &prc), 64).unwrap(), prc);
        assert_eq!(read_model(&file(dir.path(), "m.u3d", b"U3D\0x"), 6).unwrap(), b"U3D\0x");
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
}
