//! Page transitions and display times (ISO 32000-2 §12.4.4): the `/Trans` dictionary and `/Dur`
//! entry a full-screen presentation plays, as Acrobat's Organize ▸ Page Transitions sets them.
//! Reading them back is the renderer's `DocInfo::transitions`.

use pdfcraft_cos::{Dict, Document, Object};

use crate::{OrganizeError, check, walk};

/// The transition styles of ISO 32000-2 table 164, as written in `/S` (`R` is "replace": no
/// effect).
pub const TRANSITION_STYLES: [&str; 12] = ["Split", "Blinds", "Box", "Wipe", "Dissolve", "Glitter", "R", "Fly", "Push", "Cover", "Uncover", "Fade"];

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

/// Check that `t`'s style, duration, direction, and scale satisfy ISO 32000-2 table 164.
fn validate_transition(t: &Transition) -> Result<(), OrganizeError> {
    let bad = OrganizeError::Invalid;
    let style = t.style.as_str();
    if !TRANSITION_STYLES.contains(&style) {
        return Err(bad(format!("unknown transition style {:?}; one of {}", t.style, TRANSITION_STYLES.join(", "))));
    }
    if !(t.duration.is_finite() && t.duration >= 0.0) {
        return Err(bad("the transition duration must be zero or more seconds".into()));
    }
    match t.direction {
        None if style != "Fly" => {
            return Err(bad("direction None (/Di /None) is only valid for Fly transitions".into()));
        }
        Some(d) if !TRANSITION_DIRECTIONS.contains(&d) => {
            return Err(bad(format!("the direction must be one of {:?} degrees", TRANSITION_DIRECTIONS)));
        }
        Some(315) if matches!(style, "Fly" | "Push" | "Cover" | "Uncover" | "Wipe") => {
            return Err(bad(format!("direction 315 degrees is not valid for {style} transitions")));
        }
        _ => {}
    }
    if !(t.scale.is_finite() && t.scale > 0.0) {
        return Err(bad("the Fly scale must be greater than zero".into()));
    }
    Ok(())
}

/// Set (or remove) the transition and display time of `pages` (0-based). Every value is checked
/// before any page changes: an unknown style, a direction that is not one of the five, `None` on a
/// non-`Fly` style, `315` degrees on `Fly`, `Push`, `Cover`, `Uncover`, or `Wipe`, or a negative
/// or non-finite duration, scale or display time are refused.
pub fn set_transition(doc: &mut Document, pages: &[usize], spec: &TransitionSpec) -> Result<(), OrganizeError> {
    if let Some(t) = &spec.transition {
        validate_transition(t)?;
    }
    if let Some(s) = spec.display
        && !(s.is_finite() && s >= 0.0)
    {
        return Err(OrganizeError::Invalid("the display time must be zero or more seconds".into()));
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn one_page_doc() -> Document {
        let objs = [
            &b"<< /Type /Catalog /Pages 2 0 R >>"[..],
            &b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>"[..],
            &b"<< /Type /Page /Parent 2 0 R >>"[..],
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        Document::open(Arc::new(out)).expect("fixture")
    }

    #[test]
    fn rejects_none_direction_for_non_fly_and_diagonal_for_cardinal_styles() {
        let mut doc = one_page_doc();
        for style in ["Wipe", "Glitter", "Push", "Cover", "Uncover", "Split"] {
            let spec = TransitionSpec { transition: Some(Transition { direction: None, ..Transition::new(style) }), display: None };
            assert!(
                matches!(set_transition(&mut doc, &[0], &spec), Err(OrganizeError::Invalid(msg)) if msg.contains("direction")),
                "{style} must reject direction: None"
            );
        }
        let fly_none = TransitionSpec { transition: Some(Transition { direction: None, ..Transition::new("Fly") }), display: None };
        assert!(set_transition(&mut doc, &[0], &fly_none).is_ok(), "Fly allows direction: None");

        for style in ["Fly", "Push", "Cover", "Uncover", "Wipe"] {
            let spec = TransitionSpec { transition: Some(Transition { direction: Some(315), ..Transition::new(style) }), display: None };
            assert!(
                matches!(set_transition(&mut doc, &[0], &spec), Err(OrganizeError::Invalid(msg)) if msg.contains("direction")),
                "{style} must reject 315 degrees"
            );
            for deg in [0, 90, 180, 270] {
                let spec = TransitionSpec { transition: Some(Transition { direction: Some(deg), ..Transition::new(style) }), display: None };
                assert!(set_transition(&mut doc, &[0], &spec).is_ok(), "{style} allows {deg} degrees");
            }
        }
        for style in ["Glitter", "Split"] {
            let spec = TransitionSpec { transition: Some(Transition { direction: Some(315), ..Transition::new(style) }), display: None };
            assert!(set_transition(&mut doc, &[0], &spec).is_ok(), "{style} allows 315 degrees");
        }
    }
}
