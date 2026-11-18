//! Per-character desktop fallback, after the existing embedded and script-specific faces.
//!
//! The shared provider asks the OS for a font, without PdfCraft's filename allowlist. If that
//! lookup fails, try locally installed GNU Unifont. Neither provider downloads or embeds fonts;
//! both are disabled for published screenshots by `PDFCRAFT_SYSTEM_FONTS=0`.

use std::collections::HashSet;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use egui::{FallbackRequest, FontData, FontInsert, FontPriority, FontProvider, InsertFontFamily};

pub(crate) fn install(ctx: &egui::Context, locale: &str) {
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        craft_ui::fonts::install_font_providers(ctx, None, Vec::new());
    } else {
        craft_ui::fonts::install_font_providers(ctx, Some(locale), vec![Arc::new(Unifont { load: installed_unifont, faces: OnceLock::new() })]);
    }
}

struct Unifont {
    load: fn() -> Vec<FontData>,
    faces: OnceLock<Vec<FontData>>,
}

impl FontProvider for Unifont {
    fn is_last_resort(&self) -> bool {
        true
    }

    fn font_for(&self, request: &FallbackRequest<'_>) -> Option<FontInsert> {
        let c = request.cluster.chars().next()?;
        self.faces.get_or_init(self.load).iter().enumerate().find_map(|(slot, data)| {
            let index = super::face_with(data.bytes(), c)?;
            let mut data = data.clone();
            data.index = index;
            Some(FontInsert::new(
                &format!("system-unifont:{slot}:{index}"),
                crate::theme::on_interface_baseline(data),
                vec![InsertFontFamily { family: request.family.clone(), priority: FontPriority::Lowest }],
            ))
        })
    }
}

fn installed_unifont() -> Vec<FontData> {
    static FACES: OnceLock<Vec<FontData>> = OnceLock::new();
    FACES.get_or_init(|| load_files(unifont_paths())).clone()
}

fn unifont_paths() -> Vec<PathBuf> {
    #[cfg(all(unix, not(target_os = "macos")))]
    let paths = super::fontconfig::font_files(&super::fontconfig::config_path());
    #[cfg(any(windows, target_os = "macos"))]
    let paths = {
        let mut dirs = Vec::new();
        #[cfg(windows)]
        {
            dirs.push(super::windows_fonts());
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                dirs.push(PathBuf::from(local).join("Microsoft/Windows/Fonts"));
            }
        }
        #[cfg(target_os = "macos")]
        {
            dirs.extend([PathBuf::from("/Library/Fonts"), PathBuf::from("/System/Library/Fonts")]);
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(home).join("Library/Fonts"));
            }
        }
        dirs.into_iter()
            .filter_map(|dir| std::fs::read_dir(dir).ok())
            .flatten()
            .take(32_768)
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>()
    };
    #[cfg(not(any(unix, windows)))]
    let paths = Vec::new();
    let mut paths: Vec<_> = paths.into_iter().filter(|path| is_unifont(path)).collect();
    paths.sort();
    paths
}

fn is_unifont(path: &Path) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { return false };
    let stem = stem.to_ascii_lowercase();
    let name = stem == "unifont" || stem.starts_with("unifont-") || stem.starts_with("unifont_");
    name && path.extension().and_then(|s| s.to_str()).is_some_and(|ext| ["ttf", "otf", "ttc", "otc"].iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

fn load_files(paths: Vec<PathBuf>) -> Vec<FontData> {
    // Installed files can be malformed or huge. Bound retained bytes as well as each read,
    // including a file that grows after metadata(), and don't retain symlink aliases twice.
    const MAX_TOTAL: usize = 64 << 20;
    let mut remaining = MAX_TOTAL;
    let mut seen = HashSet::new();
    let mut faces = Vec::new();
    for path in paths.into_iter().take(32) {
        let Ok(path) = path.canonicalize() else { continue };
        if !seen.insert(path.clone()) {
            continue;
        }
        let Ok(meta) = path.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let Ok(file) = open_font(&path) else { continue };
        let limit = (super::MAX_BYTES as usize).min(remaining);
        let Ok(meta) = file.metadata() else { continue };
        if !meta.is_file() || meta.len() > limit as u64 {
            continue;
        }
        let mut bytes = Vec::new();
        if file.take(limit as u64 + 1).read_to_end(&mut bytes).is_err() || bytes.len() > limit {
            continue;
        }
        if skrifa::FontRef::from_index(&bytes, 0).is_err() {
            continue;
        }
        remaining -= bytes.len();
        faces.push(FontData::from_owned(bytes));
    }
    faces
}

fn open_font(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        // The path can become a FIFO after metadata(). Nonblocking open prevents waiting
        // for its writer; load_files still verifies the opened handle is a regular file.
        use rustix::fs::{Mode, OFlags};
        Ok(rustix::fs::open(path, OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty())?.into())
    }
    #[cfg(not(unix))]
    std::fs::File::open(path)
}

#[cfg(test)]
mod tests;
