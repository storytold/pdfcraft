//! A CJK face already installed on this machine, for documents whose composite (CID) font the
//! PDF does not embed.
//!
//! [`crate::fallback`] answers "the document does not carry this program; what stands in for it".
//! Its fifth step has no Simplified Chinese face to offer — PdfCraft ships only what it is licensed
//! to ship, craft-fonts carries no `Hans` face, and AGENTS.md §1.1 bars the obvious candidates
//! (Noto CJK is Source Han under another name) — so a PDF that names `STSong-Light` with the
//! `Adobe-GB1` collection used to draw nothing at all for its Chinese text.
//!
//! Windows installs faces that do cover those characters, so on Windows we read one at runtime and
//! hand it to the renderer as the substitute. Only the copy already on this machine is read:
//! nothing is bundled, embedded or redistributed, and nothing is written back.
//!
//! Windows only. Every other target answers [`installed_cjk_face`] with `None`, which leaves the
//! renderer exactly as it was.

use std::sync::{Arc, OnceLock};

/// 欢 (U+6B22): a Simplified Chinese character the Japanese craft-fonts faces do not have, so a
/// face that maps it is a real Hans face rather than a CJK face that happens to be installed.
const HANS_PROBE: char = '\u{6B22}';
/// Faces read out of one `.ttc` before the file is given up on.
const MAX_FACES: u32 = 16;
/// Larger files are not read: a file in the fonts directory is still untrusted input.
const MAX_BYTES: u64 = 96 << 20;

/// Candidate faces under `%SystemRoot%\Fonts`, most specific first: the Song/Ming faces a document
/// naming `STSong-Light`, `SimSun` or `MSung` meant, then Kai and FangSong, then the sans faces.
/// `(file, serif)`.
const CANDIDATES: &[(&str, bool)] = &[
    ("simsun.ttc", true),
    ("nsimsun.ttc", true),
    ("simkai.ttf", true),
    ("simfang.ttf", true),
    ("simhei.ttf", false),
    ("msyh.ttc", false),
    ("msyhl.ttc", false),
];

/// One face read off disk: the whole file (a `.ttc` holds several) and which face in it to use.
struct Face {
    bytes: Arc<Vec<u8>>,
    index: u32,
}

/// The two faces kept, read at most once per process.
struct Installed {
    serif: Option<Face>,
    sans: Option<Face>,
}

/// Faces as they were found on this machine, cached. `None` means "looked, found none", so a
/// machine without them is not re-read on every page.
static FACES: OnceLock<Option<Installed>> = OnceLock::new();

/// One installed face that covers Simplified Chinese: `(bytes, index of the face in them)`.
///
/// `serif` picks the Song/Ming face when there is one, which is what a document naming
/// `STSong-Light` asked for; either kind stands in for the other when only one is installed.
/// The document's own widths still decide each advance, so a substitute changes the shapes, not
/// the layout — and a substitute is always reported as one (see [`crate::fallback`]).
///
/// `None` off Windows, or when Windows has no face covering Simplified Chinese.
pub fn installed_cjk_face(serif: bool) -> Option<(Arc<Vec<u8>>, u32)> {
    let found = FACES.get_or_init(read_installed).as_ref()?;
    let face = if serif { found.serif.as_ref().or(found.sans.as_ref()) } else { found.sans.as_ref().or(found.serif.as_ref()) };
    face.map(|f| (f.bytes.clone(), f.index))
}

/// Read the first serif and the first sans candidate that cover Simplified Chinese.
#[cfg(target_os = "windows")]
fn read_installed() -> Option<Installed> {
    let dir = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    let mut serif = None;
    let mut sans = None;
    for (file, is_serif) in CANDIDATES {
        let slot = if *is_serif { &mut serif } else { &mut sans };
        if slot.is_some() {
            continue;
        }
        if let Some(face) = read_face(&dir.join(file)) {
            *slot = Some(face);
        }
    }
    (serif.is_some() || sans.is_some()).then_some(Installed { serif, sans })
}

/// Off Windows: nothing is read, and nothing stands in.
#[cfg(not(target_os = "windows"))]
fn read_installed() -> Option<Installed> {
    None
}

/// Read `path` and pick the face in it that covers Simplified Chinese.
#[cfg(target_os = "windows")]
fn read_face(path: &std::path::Path) -> Option<Face> {
    use skrifa::{FontRef, MetadataProvider as _};
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    for index in 0..MAX_FACES {
        let covers = match FontRef::from_index(&bytes, index) {
            Ok(face) => face.charmap().map(HANS_PROBE).is_some(),
            Err(_) => break,
        };
        if covers {
            return Some(Face { bytes: Arc::new(bytes), index });
        }
    }
    None
}
