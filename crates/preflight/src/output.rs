//! Output conditions: the printing condition an output intent names (ISO 32000-2 14.11.5).
//!
//! A condition is either a characterization in the ICC Characterization Data Registry, named by
//! its reference name without a profile (PDF/X-1a and PDF/X-3 allow that, CGATS PDF/X notes 3.3,
//! 5.1), or an ICC profile the user supplies, embedded as the intent's `DestOutputProfile`. No
//! printer profile ships with PdfCraft: the common ones are vendors' assets (AGENTS.md §1.1), so
//! the profile is always the user's own file.

use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object, PdfString, Stream};

/// The `RegistryName` of the ICC Characterization Data Registry.
pub const ICC_REGISTRY: &str = "http://www.color.org";

/// A characterization in the ICC registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Characterization {
    /// The registry's reference name: the output intent's `OutputConditionIdentifier`.
    pub id: &'static str,
    /// What it characterizes, for people (the intent's `OutputCondition`).
    pub condition: &'static str,
}

/// Commonly used CMYK characterizations in the ICC registry, by their reference names. The
/// first is the default for PDF/X-1a and PDF/X-3.
pub const CHARACTERIZATIONS: &[Characterization] = &[
    Characterization { id: "FOGRA39", condition: "Offset printing on coated paper (ISO 12647-2:2004/Amd 1, paper types 1 and 2)" },
    Characterization { id: "FOGRA51", condition: "Offset printing on premium coated paper (ISO 12647-2:2013, PSO Coated v3)" },
    Characterization { id: "FOGRA52", condition: "Offset printing on wood-free uncoated paper (ISO 12647-2:2013, PSO Uncoated v3)" },
    Characterization { id: "FOGRA47", condition: "Offset printing on uncoated paper (ISO 12647-2:2004/Amd 1, paper type 4)" },
    Characterization { id: "CGATS TR 006", condition: "Sheet-fed offset printing on coated paper (GRACoL 2006 Coated #1)" },
    Characterization { id: "CGATS TR 001", condition: "Web offset publication printing on coated paper (SWOP)" },
    Characterization { id: "JC200103", condition: "Offset printing on coated paper (Japan Color 2001 Coated)" },
    Characterization { id: "IFRA26", condition: "Coldset offset newspaper printing (ISO 12647-3:2004)" },
];

/// A characterization by its reference name, ignoring case and spaces (`fogra39`, `CGATS TR 006`).
pub fn characterization(id: &str) -> Option<Characterization> {
    let key = |s: &str| s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect::<String>();
    let id = key(id);
    CHARACTERIZATIONS.iter().copied().find(|c| key(c.id) == id)
}

/// The colour space an ICC profile describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Colorants {
    Gray,
    Rgb,
    Cmyk,
}

impl Colorants {
    /// The number of colour components (`/N` of an ICCBased space).
    pub fn components(self) -> i64 {
        match self {
            Colorants::Gray => 1,
            Colorants::Rgb => 3,
            Colorants::Cmyk => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Colorants::Gray => "gray",
            Colorants::Rgb => "RGB",
            Colorants::Cmyk => "CMYK",
        }
    }
}

/// Profiles larger than this are refused: printer profiles run to a few megabytes.
pub const MAX_PROFILE: usize = 64 * 1024 * 1024;

/// What an ICC profile's header and description say.
#[derive(Clone, PartialEq)]
pub struct IccProfile {
    pub bytes: Arc<Vec<u8>>,
    /// The device class (`prtr` for an output profile, `mntr` for a display).
    pub class: [u8; 4],
    /// The data colour space; `None` when it isn't gray, RGB or CMYK.
    pub colorants: Option<Colorants>,
    /// The header's colour space signature, as written (`CMYK`, `RGB `, `Lab `…).
    pub space: [u8; 4],
    /// The major version (2, 4…).
    pub version: u8,
    /// The `desc` tag.
    pub description: Option<String>,
}

/// The profile's size rather than its megabytes of tables: edits that carry a profile show up in
/// logs and test failures.
impl std::fmt::Debug for IccProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IccProfile")
            .field("bytes", &self.bytes.len())
            .field("class", &self.class_label())
            .field("space", &self.space_label())
            .field("version", &self.version)
            .field("description", &self.description)
            .finish()
    }
}

impl IccProfile {
    /// Read a profile's header (ICC.1: size at 0, version at 8, class at 12, colour space at 16,
    /// `acsp` at 36; tags after byte 128).
    pub fn read(bytes: Vec<u8>) -> Result<IccProfile, String> {
        if bytes.len() > MAX_PROFILE {
            return Err(format!("the ICC profile is larger than {} MB", MAX_PROFILE / (1024 * 1024)));
        }
        let word = |at: usize| -> Option<[u8; 4]> { bytes.get(at..at.checked_add(4)?)?.try_into().ok() };
        let size = word(0).map(u32::from_be_bytes).and_then(|s| usize::try_from(s).ok());
        let valid = size.is_some_and(|s| s >= 132 && s <= bytes.len()) && word(36) == Some(*b"acsp");
        let (Some(class), Some(space), Some(version), true) = (word(12), word(16), bytes.get(8).copied(), valid) else {
            return Err("the file isn't an ICC profile".into());
        };
        let colorants = match &space {
            b"GRAY" => Some(Colorants::Gray),
            b"RGB " => Some(Colorants::Rgb),
            b"CMYK" => Some(Colorants::Cmyk),
            _ => None,
        };
        let description = description(&bytes);
        Ok(IccProfile { bytes: Arc::new(bytes), class, colorants, space, version, description })
    }

    /// Whether it is an output (printer) profile.
    pub fn is_output(&self) -> bool {
        self.class == *b"prtr"
    }

    /// The header's class and colour space as text, for messages.
    pub fn class_label(&self) -> String {
        String::from_utf8_lossy(&self.class).trim().to_owned()
    }

    pub fn space_label(&self) -> String {
        String::from_utf8_lossy(&self.space).trim().to_owned()
    }

    /// The characterization the description names, if it names exactly one (`Coated FOGRA39
    /// (ISO 12647-2:2004)` names FOGRA39).
    pub fn named_characterization(&self) -> Option<Characterization> {
        let desc = self.description.as_deref()?.to_ascii_lowercase();
        let compact: String = desc.chars().filter(|c| !c.is_whitespace()).collect();
        let mut found = CHARACTERIZATIONS.iter().filter(|c| {
            let id: String = c.id.chars().filter(|ch| !ch.is_whitespace()).flat_map(char::to_lowercase).collect();
            // A whole name: FOGRA39 isn't FOGRA391.
            compact.match_indices(&id).any(|(at, _)| !compact.as_bytes().get(at + id.len()).is_some_and(u8::is_ascii_digit))
        });
        let first = found.next().copied();
        first.filter(|_| found.next().is_none())
    }
}

/// The profile's description: a version 2 `desc` (ASCII) or a version 4 `mluc` (UTF-16, the first
/// record).
fn description(p: &[u8]) -> Option<String> {
    let u32_at = |at: usize| -> Option<usize> { usize::try_from(u32::from_be_bytes(p.get(at..at.checked_add(4)?)?.try_into().ok()?)).ok() };
    let count = u32_at(128)?;
    // A count the profile can't hold is damage, not a long loop.
    if count > p.len().saturating_sub(132) / 12 {
        return None;
    }
    let (offset, len) = (0..count).find_map(|i| {
        let entry = 132usize.checked_add(i.checked_mul(12)?)?;
        (p.get(entry..entry.checked_add(4)?)? == b"desc").then(|| Some((u32_at(entry.checked_add(4)?)?, u32_at(entry.checked_add(8)?)?))).flatten()
    })?;
    let tag = p.get(offset..offset.checked_add(len)?)?;
    let text = match tag.get(..4)? {
        b"desc" => {
            let n = usize::try_from(u32::from_be_bytes(tag.get(8..12)?.try_into().ok()?)).ok()?;
            let ascii = tag.get(12..12usize.checked_add(n)?)?;
            String::from_utf8_lossy(ascii.split(|b| *b == 0).next().unwrap_or_default()).into_owned()
        }
        b"mluc" => {
            let records = usize::try_from(u32::from_be_bytes(tag.get(8..12)?.try_into().ok()?)).ok()?;
            if records == 0 {
                return None;
            }
            let len = usize::try_from(u32::from_be_bytes(tag.get(20..24)?.try_into().ok()?)).ok()?;
            let at = usize::try_from(u32::from_be_bytes(tag.get(24..28)?.try_into().ok()?)).ok()?;
            let (pairs, _) = tag.get(at..at.checked_add(len)?)?.as_chunks::<2>();
            let units: Vec<u16> = pairs.iter().map(|c| u16::from_be_bytes(*c)).collect();
            String::from_utf16_lossy(&units)
        }
        _ => return None,
    };
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// The printing condition an output intent names.
#[derive(Clone, Debug, PartialEq)]
pub enum OutputCondition {
    /// A characterization in the ICC registry, without a profile.
    Registered(Characterization),
    /// A profile the user chose, embedded. `identifier` is the `OutputConditionIdentifier`: the
    /// registry name the user gave, else the one the profile's description names, else its
    /// description.
    Profile { profile: IccProfile, identifier: Option<String> },
}

impl OutputCondition {
    /// A registered characterization by its reference name.
    pub fn registered(id: &str) -> Option<OutputCondition> {
        characterization(id).map(OutputCondition::Registered)
    }

    /// A profile, named by `identifier` when given (a registry name, or any condition name).
    pub fn profile(profile: IccProfile, identifier: Option<String>) -> OutputCondition {
        OutputCondition::Profile { profile, identifier: identifier.filter(|i| !i.trim().is_empty()) }
    }

    /// The embedded profile, if any.
    pub fn icc(&self) -> Option<&IccProfile> {
        match self {
            OutputCondition::Profile { profile, .. } => Some(profile),
            OutputCondition::Registered(_) => None,
        }
    }

    /// The colour space it prints in: registry characterizations here are all CMYK.
    pub fn colorants(&self) -> Option<Colorants> {
        match self {
            OutputCondition::Registered(_) => Some(Colorants::Cmyk),
            OutputCondition::Profile { profile, .. } => profile.colorants,
        }
    }

    /// The registry characterization it is, if it is one.
    pub fn characterization(&self) -> Option<Characterization> {
        match self {
            OutputCondition::Registered(c) => Some(*c),
            OutputCondition::Profile { profile, identifier } => match identifier {
                Some(id) => characterization(id),
                None => profile.named_characterization(),
            },
        }
    }

    /// The `OutputConditionIdentifier`.
    pub fn identifier(&self) -> String {
        match (self, self.characterization()) {
            (_, Some(c)) => c.id.to_owned(),
            (OutputCondition::Profile { identifier: Some(id), .. }, None) => id.trim().to_owned(),
            (OutputCondition::Profile { profile, .. }, None) => profile.description.clone().unwrap_or_else(|| "Custom".into()),
            // A registered condition always has its characterization.
            (OutputCondition::Registered(c), None) => c.id.to_owned(),
        }
    }

    /// How it is described in messages: the identifier, and the profile's description when that
    /// says more.
    pub fn label(&self) -> String {
        let id = self.identifier();
        match self.icc().and_then(|p| p.description.as_deref()) {
            Some(desc) if desc != id => format!("{id} ({desc})"),
            _ => id,
        }
    }
}

/// The output intent dictionary for `condition`, of type `subtype` (`GTS_PDFX`, `GTS_PDFA1`).
/// `profile` is the profile stream already written for this conversion, if any; the one written
/// here is returned so every intent shares it (ISO 19005-2 6.2.3: intents with a profile all
/// point at the same object).
pub(crate) fn intent(doc: &mut Document, subtype: &str, condition: &OutputCondition, profile: &mut Option<ObjRef>) -> Dict {
    let mut oi = Dict::new();
    oi.set(b"Type".to_vec(), Object::name("OutputIntent"));
    oi.set(b"S".to_vec(), Object::name(subtype));
    oi.set(b"OutputConditionIdentifier".to_vec(), PdfString::text(&condition.identifier()));
    if let Some(c) = condition.characterization() {
        oi.set(b"OutputCondition".to_vec(), PdfString::text(c.condition));
        oi.set(b"RegistryName".to_vec(), PdfString::text(ICC_REGISTRY));
    }
    if let Some(icc) = condition.icc() {
        if let Some(desc) = &icc.description {
            oi.set(b"Info".to_vec(), PdfString::text(desc));
        }
        let r = match *profile {
            Some(r) => r,
            None => {
                let mut d = Dict::new();
                if let Some(c) = icc.colorants {
                    d.set(b"N".to_vec(), Object::Int(c.components()));
                }
                let r = doc.add(Object::Stream(Stream::flate(d, &icc.bytes)));
                *profile = Some(r);
                r
            }
        };
        oi.set(b"DestOutputProfile".to_vec(), Object::Ref(r));
    } else if let Some(c) = condition.characterization() {
        oi.set(b"Info".to_vec(), PdfString::text(c.condition));
    }
    oi
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A profile header with a `desc` tag holding `text`.
    fn profile(class: &[u8; 4], space: &[u8; 4], text: &str) -> Vec<u8> {
        let mut tag = b"desc\0\0\0\0".to_vec();
        tag.extend(((text.len() + 1) as u32).to_be_bytes());
        tag.extend(text.as_bytes());
        tag.push(0);
        let offset = 128 + 4 + 12;
        let size = offset + tag.len();
        let mut p = vec![0u8; 128];
        p[0..4].copy_from_slice(&(size as u32).to_be_bytes());
        p[8] = 2;
        p[12..16].copy_from_slice(class);
        p[16..20].copy_from_slice(space);
        p[36..40].copy_from_slice(b"acsp");
        p.extend(1u32.to_be_bytes());
        p.extend(b"desc");
        p.extend((offset as u32).to_be_bytes());
        p.extend((tag.len() as u32).to_be_bytes());
        p.extend(tag);
        p
    }

    #[test]
    fn reads_a_printer_profile_and_the_characterization_it_names() {
        let p = IccProfile::read(profile(b"prtr", b"CMYK", "Coated FOGRA39 (ISO 12647-2:2004)")).unwrap();
        assert!(p.is_output());
        assert_eq!(p.colorants, Some(Colorants::Cmyk));
        assert_eq!(p.description.as_deref(), Some("Coated FOGRA39 (ISO 12647-2:2004)"));
        assert_eq!(p.named_characterization().map(|c| c.id), Some("FOGRA39"));
        let c = OutputCondition::profile(p.clone(), None);
        assert_eq!(c.identifier(), "FOGRA39");
        // A name the user gives wins; one that isn't registered is used as it is.
        assert_eq!(OutputCondition::profile(p.clone(), Some("My press".into())).identifier(), "My press");
        assert_eq!(OutputCondition::profile(p, Some("my press".into())).characterization(), None);
    }

    #[test]
    fn a_description_naming_no_characterization_or_two_names_none() {
        let p = IccProfile::read(profile(b"prtr", b"CMYK", "House press, FOGRA391 paper")).unwrap();
        assert_eq!(p.named_characterization(), None);
        let p = IccProfile::read(profile(b"prtr", b"CMYK", "FOGRA39 or FOGRA51")).unwrap();
        assert_eq!(p.named_characterization(), None);
        assert_eq!(OutputCondition::profile(p, None).identifier(), "FOGRA39 or FOGRA51");
    }

    #[test]
    fn what_isnt_a_profile_is_refused() {
        assert!(IccProfile::read(b"not a profile".to_vec()).is_err());
        let mut p = profile(b"prtr", b"CMYK", "x");
        p[36..40].copy_from_slice(b"nope");
        assert!(IccProfile::read(p).is_err());
        // A size larger than the file.
        let mut p = profile(b"prtr", b"CMYK", "x");
        p[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(IccProfile::read(p).is_err());
        // A tag count the file can't hold reads as no description, not a panic.
        let mut p = profile(b"prtr", b"CMYK", "x");
        p[128..132].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(IccProfile::read(p).unwrap().description, None);
    }

    #[test]
    fn registry_names_are_found_whatever_their_case_and_spacing() {
        assert_eq!(characterization("fogra39").map(|c| c.id), Some("FOGRA39"));
        assert_eq!(characterization("CGATSTR006").map(|c| c.id), Some("CGATS TR 006"));
        assert_eq!(characterization("FOGRA1"), None);
    }
}
