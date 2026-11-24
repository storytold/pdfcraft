//! Inverse lookup: a point on a page to the source line that produced it.

use std::cmp::Reverse;
use std::path::{Component, Path, PathBuf, Prefix};

use crate::parse::{BoxKind, BoxRec, Point};
use crate::{PdfPos, Rect, SourcePos, SyncTex, UnknownSource};

/// How far, in points, from a box a point may be and still answer.
pub const NEAR: f64 = 18.0;

fn contains(r: &Rect, x: f64, y: f64) -> bool {
    x >= r[0] && x <= r[2] && y >= r[1] && y <= r[3]
}

fn area(r: &Rect) -> f64 {
    (r[2] - r[0]) * (r[3] - r[1])
}

/// Euclidean distance from a point to a rectangle; 0 inside.
fn distance(r: &Rect, x: f64, y: f64) -> f64 {
    let dx = (r[0] - x).max(x - r[2]).max(0.0);
    let dy = (r[1] - y).max(y - r[3]).max(0.0);
    dx.hypot(dy)
}

/// Made absolute (against the current directory) with `.` dropped and `..` resolved, without
/// touching the file system.
fn normalise(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(p) };
    let mut parts: Vec<Component> = Vec::new();
    for c in abs.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(parts.last(), Some(Component::Normal(_))) {
                    parts.pop();
                }
            }
            other => parts.push(other),
        }
    }
    parts.into_iter().collect()
}

/// Whether `p` may be looked up on disk to compare it. On Windows a network share or a device
/// path is not: resolving one contacts the host or opens the device. Only a drive, plain or in
/// the verbatim form, is. Input names come from the SyncTeX file, so they are not trusted to be
/// local. On other systems every path may be looked up.
fn on_a_local_disk(p: &Path) -> bool {
    match p.components().next() {
        Some(Component::Prefix(prefix)) => matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)),
        _ => true,
    }
}

/// `p` with its links followed, when it is on a local disk and exists.
fn real_path(p: &Path) -> Option<PathBuf> {
    if !on_a_local_disk(p) {
        return None;
    }
    std::fs::canonicalize(p).ok()
}

impl SyncTex {
    /// The tags of the inputs that are the path `want`: by lexical path first, then by
    /// `canonicalize`, which is not tried on a network or device path.
    fn tags_at(&self, want: &Path) -> Vec<u32> {
        let lexical: Vec<u32> = self.inputs.iter().filter(|(_, p)| normalise(p) == want).map(|(t, _)| *t).collect();
        if !lexical.is_empty() {
            return lexical;
        }
        let Some(real) = real_path(want) else { return Vec::new() };
        self.inputs.iter().filter(|(_, p)| real_path(&normalise(p)).is_some_and(|c| c == real)).map(|(t, _)| *t).collect()
    }

    /// The tags of the inputs that are `file`. A relative `file` is read against the folder the
    /// SyncTeX file's own relative inputs are, so a name written as the file writes it matches from
    /// any working directory; only when nothing there matches is it read against the current
    /// directory, so the two readings never answer together.
    fn tags_of(&self, file: &Path) -> Vec<u32> {
        if !file.is_absolute() {
            let beside = self.tags_at(&normalise(&self.base_dir.join(file)));
            if !beside.is_empty() {
                return beside;
            }
        }
        self.tags_at(&normalise(file))
    }

    /// Where `line` of `file` landed in the output: the lowest sheet with a match and the
    /// matching boxes there. With no output for that line, the nearest line that has some is
    /// used (the later one on a tie). `Err` when `file` is not one of the inputs; `Ok(None)`
    /// when it is but has no output.
    pub fn forward(&self, file: &Path, line: u32) -> Result<Option<PdfPos>, UnknownSource> {
        let tags = self.tags_of(file);
        if tags.is_empty() {
            return Err(UnknownSource);
        }
        // Glue and kerns (`g`, `k`) are not output. A box's own line counts only when it has no
        // point records at all, as in `inverse`; void `h`/`v` boxes are such boxes.
        let box_lines = |b: &BoxRec| -> Vec<u32> {
            if b.points.is_empty() {
                return if tags.contains(&b.tag) { vec![b.line] } else { Vec::new() };
            }
            b.points.iter().filter(|p| matches!(p.kind, 'x' | '$' | 'r') && tags.contains(&p.tag)).map(|p| p.line).collect()
        };
        let horizontal = || self.sheets.iter().flat_map(|s| s.boxes.iter()).filter(|b| b.kind == BoxKind::Horizontal);
        let Some(target) = horizontal().flat_map(&box_lines).min_by_key(|l| (l.abs_diff(line), Reverse(*l))) else {
            return Ok(None);
        };
        let hit = |b: &BoxRec| box_lines(b).contains(&target);
        let Some(sheet) = self.sheets.iter().filter(|s| s.boxes.iter().any(|b| b.kind == BoxKind::Horizontal && hit(b))).min_by_key(|s| s.number)
        else {
            return Ok(None);
        };
        let mut rects: Vec<Rect> = Vec::new();
        for b in sheet.boxes.iter().filter(|b| b.kind == BoxKind::Horizontal && hit(b)) {
            if !rects.contains(&b.rect) {
                rects.push(b.rect);
            }
        }
        // Zero-area rects (void boxes) are noise when a real one matched.
        if rects.iter().any(|r| area(r) > 0.0) {
            rects.retain(|r| area(r) > 0.0);
        }
        Ok(Some(PdfPos { sheet: sheet.number, rects }))
    }

    /// The source position for the point `(x, y)` (points from the top-left) on `sheet` (1-based).
    pub fn inverse(&self, sheet: u32, x: f64, y: f64) -> Option<SourcePos> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let sheet = self.sheets.iter().find(|s| s.number == sheet)?;
        let horizontal = || sheet.boxes.iter().filter(|b| b.kind == BoxKind::Horizontal);

        let inside = horizontal().filter(|b| contains(&b.rect, x, y)).min_by(|a, b| area(&a.rect).total_cmp(&area(&b.rect)));
        let chosen: &BoxRec = match inside {
            Some(b) => b,
            None => {
                let (b, d) = horizontal().map(|b| (b, distance(&b.rect, x, y))).min_by(|a, b| a.1.total_cmp(&b.1))?;
                if d > NEAR {
                    return None;
                }
                b
            }
        };

        let nearest: Option<&Point> = chosen.points.iter().min_by(|a, b| (a.x - x).abs().total_cmp(&(b.x - x).abs()));
        let (tag, line, column) = match nearest {
            Some(p) => (p.tag, p.line, p.column),
            None => (chosen.tag, chosen.line, 0),
        };
        let file = self.inputs.iter().find(|(t, _)| *t == tag).map(|(_, p)| p.clone())?;
        Some(SourcePos { file, line, column })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const FIXTURE: &str = "SyncTeX Version:1
Input:1:/work/main.tex
Input:2:./chap/one.tex
Output:pdf
Magnification:1000
Unit:1
X Offset:0
Y Offset:0
Content:
!0
{1
[1,1:0,0:0,0,0
(1,12:6578176,13156352:19734528,657818,0
x1,12:6578176,13156352
x1,13:16445440,13156352
)
(2,5:6578176,19734528:19734528,657818,0
x2,5:6578176,19734528
)
]
}1
{2
[1,1:0,0:0,0,0
(1,40:6578176,13156352:19734528,657818,0
x1,40:6578176,13156352
)
]
}2
Postamble:
Count:9
";

    fn pos(sheet: u32, x: f64, y: f64) -> Option<(String, u32, u32)> {
        let st = SyncTex::parse(FIXTURE.as_bytes(), Path::new("/work")).unwrap();
        let p = st.inverse(sheet, x, y)?;
        Some((p.file.to_str()?.to_string(), p.line, p.column))
    }

    fn check(got: Option<(String, u32, u32)>, want: Option<(&str, u32, u32)>) {
        assert_eq!(got.as_ref().map(|(f, l, c)| (f.as_str(), *l, *c)), want);
    }

    #[test]
    fn inverse_lookup() {
        check(pos(1, 120.0, 195.0), Some(("/work/main.tex", 12, 0)));
        check(pos(1, 300.0, 195.0), Some(("/work/main.tex", 13, 0)));
        check(pos(1, 120.0, 295.0), Some(("/work/chap/one.tex", 5, 0)));
        check(pos(1, 120.0, 210.0), Some(("/work/main.tex", 12, 0)));
        check(pos(1, 120.0, 245.0), None);
        check(pos(3, 120.0, 195.0), None);
        check(pos(0, 120.0, 195.0), None);
        check(pos(1, f64::NAN, 195.0), None);
        check(pos(1, 120.0, f64::INFINITY), None);
    }

    fn fwd(file: &str, line: u32) -> Result<Option<(u32, Vec<Rect>)>, UnknownSource> {
        let st = SyncTex::parse(FIXTURE.as_bytes(), Path::new("/work")).unwrap();
        Ok(st.forward(Path::new(file), line)?.map(|p| (p.sheet, p.rects)))
    }

    #[test]
    fn forward_lookup() {
        let (sheet, rects) = fwd("/work/main.tex", 40).unwrap().unwrap();
        assert_eq!((sheet, rects.len()), (2, 1));
        for (g, w) in rects[0].iter().zip([100.0, 190.0, 400.0, 200.0]) {
            assert!((g - w).abs() < 0.01, "{rects:?}");
        }
        assert_eq!(fwd("/work/main.tex", 12).unwrap().unwrap().0, 1);
        assert_eq!(fwd("/work/main.tex", 20).unwrap().unwrap().0, 1);
        assert_eq!(fwd("/work/./chap/../chap/one.tex", 5).unwrap().unwrap().0, 1);
        assert_eq!(fwd("/elsewhere/main.tex", 12), Err(UnknownSource));
    }

    #[test]
    fn ties_go_to_the_later_line() {
        let text = "SyncTeX Version:1\nInput:1:/work/a.tex\nContent:\n{1\n(1,10:6578176,13156352:19734528,657818,0\n)\n(1,20:6578176,19734528:19734528,657818,0\n)\n}1\n";
        let st = SyncTex::parse(text.as_bytes(), Path::new("/work")).unwrap();
        let p = st.forward(Path::new("/work/a.tex"), 15).unwrap().unwrap();
        assert_eq!(p.rects.len(), 1);
        assert!((p.rects[0][1] - 290.0).abs() < 0.01, "{:?}", p.rects);
    }

    #[test]
    fn a_known_input_with_no_output_is_none() {
        let text = "SyncTeX Version:1\nInput:1:/work/a.tex\nContent:\n{1\n}1\n";
        let st = SyncTex::parse(text.as_bytes(), Path::new("/work")).unwrap();
        assert_eq!(st.forward(Path::new("/work/a.tex"), 3), Ok(None));
    }

    /// A share or device named as an input, or asked for, is compared by name only.
    #[cfg(windows)]
    #[test]
    fn network_and_device_paths_are_not_looked_up() {
        for p in [r"\\host\share\main.tex", r"\\?\UNC\host\share\main.tex", r"\\.\pipe\main.tex"] {
            assert!(!on_a_local_disk(Path::new(p)), "{p}");
            assert_eq!(real_path(Path::new(p)), None, "{p}");
        }
        for p in [r"C:\work\main.tex", r"\\?\C:\work\main.tex"] {
            assert!(on_a_local_disk(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn a_plain_path_may_be_looked_up() {
        assert!(on_a_local_disk(Path::new("/work/main.tex")));
        assert!(on_a_local_disk(Path::new("main.tex")));
    }

    /// A relative name is the one the SyncTeX file wrote, wherever the process runs.
    #[test]
    fn a_relative_source_is_read_against_the_synctex_folder() {
        let base = std::env::temp_dir().join("pdfcraft-synctex-no-such-folder");
        let text = "SyncTeX Version:1\nInput:1:./chap/one.tex\nContent:\n{1\n(1,7:6578176,13156352:19734528,657818,0\n)\n}1\n";
        let st = SyncTex::parse(text.as_bytes(), &base).unwrap();
        assert_eq!(st.forward(Path::new("./chap/one.tex"), 7).unwrap().map(|p| p.sheet), Some(1));
        assert_eq!(st.forward(Path::new("chap/one.tex"), 7).unwrap().map(|p| p.sheet), Some(1));
        assert_eq!(st.forward(&base.join("chap/one.tex"), 7).unwrap().map(|p| p.sheet), Some(1));
        assert_eq!(st.forward(Path::new("chap/two.tex"), 7), Err(UnknownSource));
    }

    /// With the same relative name in the SyncTeX folder and in the current directory, both
    /// inputs, the SyncTeX folder's is the one meant.
    #[test]
    fn the_synctex_folder_wins_over_the_current_directory() {
        let base = std::env::temp_dir().join("pdfcraft-synctex-no-such-folder");
        let here = std::env::current_dir().unwrap().join("main.tex");
        // The current directory's file is on sheet 1 and the SyncTeX folder's on sheet 2, so an
        // answer from both would name sheet 1.
        let text = format!(
            "SyncTeX Version:1\nInput:1:{}\nInput:2:./main.tex\nContent:\n{{1\n(1,7:6578176,13156352:19734528,657818,0\n)\n}}1\n{{2\n(2,7:6578176,13156352:19734528,657818,0\n)\n}}2\n",
            here.display()
        );
        let st = SyncTex::parse(text.as_bytes(), &base).unwrap();
        assert_eq!(st.forward(Path::new("main.tex"), 7).unwrap().map(|p| p.sheet), Some(2));
        assert_eq!(st.forward(&here, 7).unwrap().map(|p| p.sheet), Some(1));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_spelling_matches() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-synctex-fwd-{}", std::process::id()));
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("main tex é.tex"), "x").unwrap();
        std::os::unix::fs::symlink(&real, dir.join("link")).unwrap();
        let text = format!(
            "SyncTeX Version:1\nInput:1:{}\nContent:\n{{1\n(1,7:6578176,13156352:19734528,657818,0\n)\n}}1\n",
            real.join("main tex é.tex").display()
        );
        let st = SyncTex::parse(text.as_bytes(), Path::new("/work")).unwrap();
        let got = st.forward(&dir.join("link").join("main tex é.tex"), 7);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(got.unwrap().map(|p| p.sheet), Some(1));
    }

    #[test]
    fn box_without_points_answers_with_its_own_line() {
        let text = "SyncTeX Version:1\nInput:1:/work/a.tex\nContent:\n{1\n(1,7:6578176,13156352:19734528,657818,0\n)\n}1\n";
        let st = SyncTex::parse(text.as_bytes(), Path::new("/work")).unwrap();
        let p = st.inverse(1, 120.0, 195.0).unwrap();
        assert_eq!((p.file.as_path(), p.line, p.column), (Path::new("/work/a.tex"), 7, 0));
    }

    #[test]
    fn glue_and_kerns_are_not_output() {
        let text = "SyncTeX Version:1\nInput:1:/work/main.tex\nContent:\n{1\n\
            (1,6:6578176,13156352:19734528,657818,0\nk1,6:6578176,13156352:100\ng1,6:6578176,13156352\n)\n\
            (1,8:6578176,19734528:19734528,657818,0\nx1,7:6578176,19734528\n)\n}1\n";
        let st = SyncTex::parse(text.as_bytes(), Path::new("/work")).unwrap();
        for line in [5, 6] {
            let p = st.forward(Path::new("/work/main.tex"), line).unwrap().unwrap();
            assert_eq!(p.rects.len(), 1, "line {line}");
            assert!((p.rects[0][1] - 290.0).abs() < 0.01, "line {line}: {:?}", p.rects);
        }
    }

    #[test]
    fn zero_area_rects_are_dropped_when_real_ones_match() {
        let text = "SyncTeX Version:1\nInput:1:/work/main.tex\nContent:\n{1\n\
            h1,7:6578176,13156352:19734528,0,0\n\
            (1,9:6578176,19734528:19734528,657818,0\nx1,7:6578176,19734528\n)\n}1\n";
        let st = SyncTex::parse(text.as_bytes(), Path::new("/work")).unwrap();
        let p = st.forward(Path::new("/work/main.tex"), 7).unwrap().unwrap();
        assert_eq!(p.rects.len(), 1);
        assert!((p.rects[0][1] - 290.0).abs() < 0.01, "{:?}", p.rects);
        // Alone, a zero-area rect is kept.
        let only = "SyncTeX Version:1\nInput:1:/work/main.tex\nContent:\n{1\nh1,7:6578176,13156352:19734528,0,0\n}1\n";
        let st = SyncTex::parse(only.as_bytes(), Path::new("/work")).unwrap();
        assert_eq!(st.forward(Path::new("/work/main.tex"), 7).unwrap().unwrap().rects.len(), 1);
    }
}
