//! Page transitions and display times (ISO 32000-2 §12.4.4): the `/Trans` dictionary and `/Dur`
//! entry a full-screen presentation plays, as Acrobat's Organize ▸ Page Transitions sets them.
//! Reading them back is the renderer's `DocInfo::transitions`.

use pdfcraft_cos::{Dict, Document, Object};

use crate::{OrganizeError, check, walk};

/// The transition styles of ISO 32000-2 table 164, as written in `/S` (`R` is "replace": no
/// effect).
pub const TRANSITION_STYLES: [&str; 12] = ["Split", "Blinds", "Box", "Wipe", "Dissolve", "Glitter", "R", "Fly", "Push", "Cover", "Uncover", "Fade"];

/// The styles PDF 1.5 added (ISO 32000-1 table 162). A page given one raises a file that declares
/// an earlier version to 1.5.
const PDF_1_5_STYLES: [&str; 5] = ["Fly", "Push", "Cover", "Uncover", "Fade"];

/// The directions `/Di` allows, in degrees.
pub const TRANSITION_DIRECTIONS: [u16; 5] = [0, 90, 180, 270, 315];

/// A page transition to write: the `/Trans` dictionary's entries, with the spec's defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    /// One of [`TRANSITION_STYLES`].
    pub style: String,
    /// `/D`: seconds the transition takes (default 1).
    pub duration: f32,
    /// `/Dm`: horizontal (`/H`, the default) or vertical (`/V`); Split and Blinds.
    pub horizontal: bool,
    /// `/M`: inward (`/I`, the default) or outward (`/O`); Split, Box and Fly.
    pub inward: bool,
    /// `/Di`: one of [`TRANSITION_DIRECTIONS`] (default 0), or `None` for `/None` (Fly only);
    /// Wipe, Glitter, Fly, Cover, Uncover and Push.
    pub direction: Option<u16>,
    /// `/SS`: the Fly scale (default 1).
    pub scale: f32,
    /// `/B`: Fly with an opaque backdrop.
    pub opaque: bool,
}

impl Transition {
    /// A transition of `style` with every other entry at its default.
    pub fn new(style: &str) -> Self {
        Self { style: style.to_string(), duration: 1.0, horizontal: true, inward: true, direction: Some(0), scale: 1.0, opaque: false }
    }
}

/// What a page gets: its transition (`None` removes `/Trans`) and the seconds it stays before the
/// presentation advances (`None` removes `/Dur`: the page waits for the presenter).
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionSpec {
    pub transition: Option<Transition>,
    pub display: Option<f32>,
}

/// Set (or remove) the transition and display time of `pages` (0-based). Every value is checked
/// before any page changes: an unknown style, a direction that is not one of the five, a negative
/// or non-finite duration, scale or display time are refused.
pub fn set_transition(doc: &mut Document, pages: &[usize], spec: &TransitionSpec) -> Result<(), OrganizeError> {
    let bad = |m: String| OrganizeError::Invalid(m);
    if let Some(t) = &spec.transition {
        if !TRANSITION_STYLES.contains(&t.style.as_str()) {
            return Err(bad(format!("unknown transition style {:?}; one of {}", t.style, TRANSITION_STYLES.join(", "))));
        }
        if !(t.duration.is_finite() && t.duration >= 0.0) {
            return Err(bad("the transition duration must be zero or more seconds".into()));
        }
        if let Some(d) = t.direction
            && !TRANSITION_DIRECTIONS.contains(&d)
        {
            return Err(bad(format!("the direction must be one of {:?} degrees", TRANSITION_DIRECTIONS)));
        }
        if !(t.scale.is_finite() && t.scale > 0.0) {
            return Err(bad("the Fly scale must be more than zero".into()));
        }
    }
    if let Some(s) = spec.display
        && !(s.is_finite() && s >= 0.0)
    {
        return Err(bad("the display time must be zero or more seconds".into()));
    }
    let all = walk(doc)?;
    check(pages, all.len())?;
    let trans = spec.transition.as_ref().map(trans_dict);
    for &i in pages {
        let Some((page, _)) = all.get(i) else { continue };
        let trans = trans.clone();
        doc.update_dict(*page, |d| {
            match trans {
                Some(t) => d.set(b"Trans".to_vec(), Object::Dict(t)),
                None => {
                    d.remove(b"Trans");
                }
            }
            match spec.display {
                Some(s) => d.set(b"Dur".to_vec(), Object::Real(f64::from(s))),
                None => {
                    d.remove(b"Dur");
                }
            }
        })?;
    }
    if !pages.is_empty() && spec.transition.as_ref().is_some_and(|t| PDF_1_5_STYLES.contains(&t.style.as_str())) {
        at_least_version(doc, "1.5")?;
    }
    Ok(())
}

/// Whether the file declares `min` or later (§7.7.2): its catalog `/Version`, or its header. The
/// header must reach it both as the file has it, which an incremental save keeps, and as
/// [`Document::version`] says, which a full save writes. A version that isn't `major.minor` in
/// digits (`/Latest`, a damaged header) declares nothing.
fn at_least(doc: &Document, min: &str) -> bool {
    let min = parse_version(min.as_bytes());
    let reached = |v: &[u8]| parse_version(v).is_some_and(|v| Some(v) >= min);
    let catalog = doc.root().and_then(|root| doc.get(root).as_dict().and_then(|c| c.get(b"Version").cloned()));
    let catalog = catalog.and_then(|v| doc.resolve(&v).as_name().map(<[u8]>::to_vec));
    let bytes = doc.bytes();
    let kept = bytes.get(doc.header_offset()..).filter(|b| b.starts_with(b"%PDF-")).map(|b| {
        let v = &b[5..];
        &v[..v.iter().position(|c| !(c.is_ascii_digit() || *c == b'.')).unwrap_or(v.len())]
    });
    let header = reached(doc.version().as_bytes()) && (bytes.is_empty() || kept.is_some_and(reached));
    header || catalog.as_deref().is_some_and(reached)
}

/// `major.minor` of a PDF version, both in digits.
fn parse_version(v: &[u8]) -> Option<(u32, u32)> {
    let s = std::str::from_utf8(v).ok()?;
    let (major, minor) = s.split_once('.')?;
    let number = |p: &str| if !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) { p.parse().ok() } else { None };
    Some((number(major)?, number(minor)?))
}

/// A document whose header and catalog `/Version` both say less than `min` gets `/Version min` in
/// its catalog. The later of the two applies (§7.7.2), so an in-place save that keeps the header
/// still declares the version its content needs.
fn at_least_version(doc: &mut Document, min: &str) -> Result<(), OrganizeError> {
    let Some(root) = doc.root() else { return Ok(()) };
    if !at_least(doc, min) {
        doc.update_dict(root, |c| c.set(b"Version".to_vec(), Object::name(min)))?;
    }
    Ok(())
}

/// The `/Trans` dictionary for `t`: only the entries that differ from the defaults, and only the
/// ones the style reads (table 164), so the file says no more than it means.
fn trans_dict(t: &Transition) -> Dict {
    let style = t.style.as_str();
    let mut d = Dict::new();
    d.set(b"Type".to_vec(), Object::name("Trans"));
    d.set(b"S".to_vec(), Object::name(style));
    if t.duration != 1.0 {
        d.set(b"D".to_vec(), Object::Real(f64::from(t.duration)));
    }
    if matches!(style, "Split" | "Blinds") && !t.horizontal {
        d.set(b"Dm".to_vec(), Object::name("V"));
    }
    if matches!(style, "Split" | "Box" | "Fly") && !t.inward {
        d.set(b"M".to_vec(), Object::name("O"));
    }
    if matches!(style, "Wipe" | "Glitter" | "Fly" | "Cover" | "Uncover" | "Push") {
        match t.direction {
            None if style == "Fly" => d.set(b"Di".to_vec(), Object::name("None")),
            Some(deg) if deg != 0 => d.set(b"Di".to_vec(), Object::Int(i64::from(deg))),
            _ => {}
        }
    }
    if style == "Fly" {
        if t.scale != 1.0 {
            d.set(b"SS".to_vec(), Object::Real(f64::from(t.scale)));
        }
        if t.opaque {
            d.set(b"B".to_vec(), Object::Bool(true));
        }
    }
    d
}
