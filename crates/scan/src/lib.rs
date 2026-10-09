//! pdfcraft-scan — find scanners and scan pages, for Create ▸ PDF from Scanner (issue #312).
//! Layer L4: no UI, no PDF; the engine turns the scanned images into pages.
//!
//! Two backends, neither needing `unsafe` or C bindings:
//! - **eSCL** ([`escl`]): the HTTP + XML protocol of network scanners (AirScan, Mopria), on
//!   every platform. Scanners are found with mDNS (`_uscan._tcp`), or by their address.
//! - **SANE** ([`sane`]): the `scanimage` program, on Linux, FreeBSD and other Unix systems.
//!
//! Windows currently supports eSCL only; WIA is deferred until a suitable Rust binding exists.
//!
//! A scanner's id names the backend that drives it: `escl:<base URL>`, `sane:<device>` or
//! `wia:<device id>`. [`scanners`] lists what is reachable; [`scan`] scans with a
//! [`ScanSettings`] and returns the page images (PNG, JPEG, TIFF or BMP, as the device sends
//! them) with their resolution.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub mod escl;
#[cfg(feature = "fake-escl")]
pub mod fake;
pub mod sane;
#[cfg(test)]
mod tests;

/// Never more pages in one scan (a jammed feeder that keeps reporting paper stops here).
pub const MAX_PAGES: usize = 500;
/// The largest page image accepted from a scanner.
pub const MAX_PAGE_BYTES: u64 = 512 << 20;
/// The resolutions a scan may ask for.
pub const DPI_RANGE: std::ops::RangeInclusive<u32> = 50..=1200;

/// Which program or protocol drives a scanner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Backend {
    Escl,
    Sane,
    Wia,
}

impl Backend {
    /// The id prefix (`escl:`, `sane:`, `wia:`).
    pub fn prefix(self) -> &'static str {
        match self {
            Backend::Escl => "escl:",
            Backend::Sane => "sane:",
            Backend::Wia => "wia:",
        }
    }

    /// How the interface names the connection.
    pub fn label(self) -> &'static str {
        match self {
            Backend::Escl => "Network",
            Backend::Sane => "SANE",
            Backend::Wia => "WIA",
        }
    }
}

/// A scanner that can be scanned from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scanner {
    /// `escl:<base URL>`, `sane:<device>` or `wia:<device id>`.
    pub id: String,
    /// The make and model, as the device reports it.
    pub name: String,
    pub backend: Backend,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorMode {
    /// 1 bit per pixel (or the scanner's closest).
    BlackWhite,
    Gray,
    #[default]
    Color,
}

impl ColorMode {
    pub fn label(self) -> &'static str {
        match self {
            ColorMode::BlackWhite => "Black & White",
            ColorMode::Gray => "Grayscale",
            ColorMode::Color => "Color",
        }
    }

    pub fn from_name(s: &str) -> Option<ColorMode> {
        match s.to_ascii_lowercase().as_str() {
            "bw" | "black_white" | "blackwhite" | "lineart" | "mono" => Some(ColorMode::BlackWhite),
            "gray" | "grey" | "grayscale" | "greyscale" => Some(ColorMode::Gray),
            "color" | "colour" => Some(ColorMode::Color),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Source {
    #[default]
    Flatbed,
    /// The automatic document feeder, front sides only.
    Feeder,
    /// The automatic document feeder, both sides.
    FeederDuplex,
}

impl Source {
    pub fn is_feeder(self) -> bool {
        self != Source::Flatbed
    }

    pub fn from_name(s: &str) -> Option<Source> {
        match s.to_ascii_lowercase().as_str() {
            "flatbed" | "platen" => Some(Source::Flatbed),
            "feeder" | "adf" => Some(Source::Feeder),
            "duplex" | "feeder_duplex" | "adf_duplex" => Some(Source::FeederDuplex),
            _ => None,
        }
    }
}

/// The area to scan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Paper {
    #[default]
    Letter,
    Legal,
    A4,
    A5,
    /// Everything the scanner can reach.
    Full,
}

impl Paper {
    pub const ALL: [Paper; 5] = [Paper::Letter, Paper::Legal, Paper::A4, Paper::A5, Paper::Full];

    /// Width and height in millimetres (`None` for [`Paper::Full`]).
    pub fn size_mm(self) -> Option<(f64, f64)> {
        match self {
            Paper::Letter => Some((215.9, 279.4)),
            Paper::Legal => Some((215.9, 355.6)),
            Paper::A4 => Some((210.0, 297.0)),
            Paper::A5 => Some((148.0, 210.0)),
            Paper::Full => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Paper::Letter => "Letter",
            Paper::Legal => "Legal",
            Paper::A4 => "A4",
            Paper::A5 => "A5",
            Paper::Full => "Scanner's full area",
        }
    }

    pub fn from_name(s: &str) -> Option<Paper> {
        match s.to_ascii_lowercase().as_str() {
            "letter" => Some(Paper::Letter),
            "legal" => Some(Paper::Legal),
            "a4" => Some(Paper::A4),
            "a5" => Some(Paper::A5),
            "full" | "max" => Some(Paper::Full),
            _ => None,
        }
    }
}

/// What to scan and how.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScanSettings {
    pub color: ColorMode,
    /// Dots per inch; the scanner's closest supported resolution is used.
    pub dpi: u32,
    pub source: Source,
    pub paper: Paper,
}

impl Default for ScanSettings {
    fn default() -> Self {
        Preset::ColorDocument.settings()
    }
}

/// The ready-made scan settings, as Acrobat offers them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    BlackWhiteDocument,
    GrayscaleDocument,
    ColorDocument,
    ColorPhoto,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::BlackWhiteDocument, Preset::GrayscaleDocument, Preset::ColorDocument, Preset::ColorPhoto];

    pub fn label(self) -> &'static str {
        match self {
            Preset::BlackWhiteDocument => "Black & White Document",
            Preset::GrayscaleDocument => "Grayscale Document",
            Preset::ColorDocument => "Color Document",
            Preset::ColorPhoto => "Color Photo",
        }
    }

    /// The name agents pass (`bw_document`, `gray_document`, `color_document`, `color_photo`).
    pub fn name(self) -> &'static str {
        match self {
            Preset::BlackWhiteDocument => "bw_document",
            Preset::GrayscaleDocument => "gray_document",
            Preset::ColorDocument => "color_document",
            Preset::ColorPhoto => "color_photo",
        }
    }

    pub fn from_name(s: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.name() == s)
    }

    /// Colour mode and resolution; the source and paper are the default (flatbed, Letter).
    pub fn settings(self) -> ScanSettings {
        let (color, dpi) = match self {
            Preset::BlackWhiteDocument => (ColorMode::BlackWhite, 300),
            Preset::GrayscaleDocument => (ColorMode::Gray, 300),
            Preset::ColorDocument => (ColorMode::Color, 200),
            Preset::ColorPhoto => (ColorMode::Color, 300),
        };
        ScanSettings { color, dpi, source: Source::Flatbed, paper: Paper::Letter }
    }

    /// The preset whose colour mode and resolution `s` has, if any.
    pub fn matching(s: &ScanSettings) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| {
            let ps = p.settings();
            ps.color == s.color && ps.dpi == s.dpi
        })
    }
}

/// One scanned page: the image file as the scanner sent it, and its resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct ScannedPage {
    pub bytes: Vec<u8>,
    pub dpi: f64,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ScanError {
    #[error("{0:?} is not a scanner id (ids start with escl:, sane: or wia:; the scanners tool lists them)")]
    UnknownId(String),
    #[error("the scanner isn't connected or can't be reached: {0}")]
    NotFound(String),
    #[error("the document feeder is empty: load the pages and scan again")]
    FeederEmpty,
    #[error("the document feeder is jammed: clear it and scan again")]
    Jammed,
    #[error("the scanner is busy: try again when it is done")]
    Busy,
    #[error("the scanner's cover is open")]
    CoverOpen,
    #[error("this scanner has no {0}")]
    NoSource(&'static str),
    #[error("scanning was cancelled")]
    Cancelled,
    #[error("{0} isn't available on this computer")]
    BackendMissing(&'static str),
    #[error("the scanner sent no pages")]
    NoPages,
    #[error("scanning failed: {0}")]
    Failed(String),
}

/// Every scanner the backends of this platform can reach: eSCL scanners announced on the
/// local network within `wait`, plus SANE (Unix) devices. Never fails: a
/// backend that is missing or errors contributes nothing.
pub fn scanners(wait: Duration) -> Vec<Scanner> {
    let escl = std::thread::Builder::new().name("pdfcraft-scan-mdns".into()).spawn(move || escl::discover(wait)).ok();
    let mut found = Vec::new();
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    found.extend(sane::scanners());

    if let Some(h) = escl {
        found.extend(h.join().unwrap_or_default());
    }
    let mut seen = std::collections::HashSet::new();
    found.retain(|s| seen.insert(s.id.clone()));
    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
    found
}

/// Scan with the scanner `id` (see [`Scanner::id`]). An `escl:` id may also be a bare host or
/// address (`escl:192.168.1.20`). Setting `cancel` stops the scan at the next page boundary.
pub fn scan(id: &str, settings: &ScanSettings, cancel: &AtomicBool) -> Result<Vec<ScannedPage>, ScanError> {
    let settings = ScanSettings { dpi: settings.dpi.clamp(*DPI_RANGE.start(), *DPI_RANGE.end()), ..*settings };
    if let Some(rest) = id.strip_prefix(Backend::Escl.prefix()) {
        let base = escl::normalize_base(rest).ok_or_else(|| ScanError::UnknownId(id.to_string()))?;
        return escl::scan(&base, &settings, cancel);
    }
    if let Some(dev) = id.strip_prefix(Backend::Sane.prefix()) {
        return sane::scan(dev, &settings, cancel);
    }
    if id.starts_with(Backend::Wia.prefix()) {
        return Err(ScanError::BackendMissing("WIA (use an eSCL network scanner on Windows)"));
    }
    Err(ScanError::UnknownId(id.to_string()))
}

/// The scan area in millimetres for `paper`, within the device's maximum (`max_mm`, when it is
/// known). `None` means "the whole area" and no maximum is known.
pub fn area_mm(paper: Paper, max_mm: Option<(f64, f64)>) -> Option<(f64, f64)> {
    match (paper.size_mm(), max_mm) {
        (Some((w, h)), Some((mw, mh))) => Some((w.min(mw), h.min(mh))),
        (Some(size), None) => Some(size),
        (None, max) => max,
    }
}

/// The supported resolution closest to `dpi` (ties go to the higher one).
pub fn closest_dpi(supported: &[u32], dpi: u32) -> Option<u32> {
    supported.iter().copied().filter(|r| *r > 0).min_by_key(|r| (r.abs_diff(dpi), std::cmp::Reverse(*r)))
}

/// Human-readable text from untrusted program output: trimmed, one line, at most 300 chars.
pub(crate) fn tidy(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    one.chars().take(300).collect()
}
