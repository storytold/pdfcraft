//! Fixups: small, safe changes that address what a check found. None of them changes how a page
//! looks; colour conversion and output intents are not fixups here.
//!
//! - [`Fixup::SetTrimBox`]: pages with neither a TrimBox nor an ArtBox get a TrimBox equal to the
//!   visible page (the CropBox within the MediaBox), kept inside the BleedBox.
//! - [`Fixup::SetTrapped`]: the document information's `Trapped` is set to `/True` or `/False`,
//!   and the metadata's `pdf:Trapped`, where it has one, to the same.
//! - [`Fixup::RemoveScripts`]: document JavaScript, additional actions, and action chains that run
//!   JavaScript or Launch a program are removed, and the actions of their own that run them are
//!   emptied. Links that go to a page or a web address stay.

use pdfcraft_cos::{CosError, Dict, Document, Object, Stream};
use serde::{Deserialize, Serialize};

/// A fixup and its parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "fixup", rename_all = "kebab-case")]
pub enum Fixup {
    SetTrimBox,
    SetTrapped { trapped: bool },
    RemoveScripts,
}

/// Every fixup, as a picker lists them.
pub const FIXUPS: [Fixup; 4] = [Fixup::SetTrimBox, Fixup::SetTrapped { trapped: false }, Fixup::SetTrapped { trapped: true }, Fixup::RemoveScripts];

impl Fixup {
    /// A stable id (the JSON `fixup` tag).
    pub fn id(self) -> &'static str {
        match self {
            Fixup::SetTrimBox => "set-trim-box",
            Fixup::SetTrapped { .. } => "set-trapped",
            Fixup::RemoveScripts => "remove-scripts",
        }
    }

    /// A fixup by id; `set-trapped` takes `trapped` (default `false`, not trapped).
    pub fn from_id(id: &str, trapped: Option<bool>) -> Option<Fixup> {
        match id {
            "set-trim-box" => Some(Fixup::SetTrimBox),
            "set-trapped" => Some(Fixup::SetTrapped { trapped: trapped.unwrap_or(false) }),
            "remove-scripts" => Some(Fixup::RemoveScripts),
            _ => None,
        }
    }

    /// What it does, as a wording (also its name in the edit history).
    pub fn label(self) -> &'static str {
        match self {
            Fixup::SetTrimBox => "Set missing TrimBoxes from the CropBox",
            Fixup::SetTrapped { trapped: false } => "Set Trapped to False",
            Fixup::SetTrapped { trapped: true } => "Set Trapped to True",
            Fixup::RemoveScripts => "Remove JavaScript and actions",
        }
    }
}

/// Apply `fixup` to `doc`; how many objects it changed (0: there was nothing to fix).
pub fn fix(doc: &mut Document, fixup: Fixup) -> Result<usize, CosError> {
    match fixup {
        Fixup::SetTrimBox => set_trim_box(doc),
        Fixup::SetTrapped { trapped } => set_trapped(doc, trapped),
        Fixup::RemoveScripts => remove_scripts(doc),
    }
}

fn set_trim_box(doc: &mut Document) -> Result<usize, CosError> {
    let mut n = 0;
    for p in pdfcraft_model::pages(doc) {
        if p.dict.contains(b"TrimBox") || p.dict.contains(b"ArtBox") {
            continue;
        }
        let mut trim = p.crop(doc);
        // The trim lies inside the bleed (ISO 32000-2 14.11.2), so a BleedBox narrower than the
        // visible page narrows it.
        if let Some(b) = super::checks::rect(doc, p.dict.get(b"BleedBox")) {
            let within = [trim[0].max(b[0]), trim[1].max(b[1]), trim[2].min(b[2]), trim[3].min(b[3])];
            if within[2] > within[0] && within[3] > within[1] {
                trim = within;
            }
        }
        if !trim.iter().all(|v| v.is_finite()) || trim[2] <= trim[0] || trim[3] <= trim[1] {
            continue;
        }
        let rect = Object::Array(trim.iter().map(|v| pdfcraft_content::num(*v)).collect());
        doc.update_dict(p.obj, |d| d.set(b"TrimBox".to_vec(), rect))?;
        n += 1;
    }
    Ok(n)
}

fn set_trapped(doc: &mut Document, trapped: bool) -> Result<usize, CosError> {
    let word = if trapped { "True" } else { "False" };
    let value = Object::name(word);
    let mut n = 0;
    match doc.trailer().get(b"Info").cloned() {
        Some(Object::Ref(r)) if doc.get(r).as_dict().is_some() => {
            if doc.get(r).as_dict().and_then(|d| d.get(b"Trapped")) != Some(&value) {
                doc.update_dict(r, |d| d.set(b"Trapped".to_vec(), value))?;
                n += 1;
            }
        }
        info => {
            // An information dictionary written into the trailer becomes an object of its own, as
            // Document Properties writes it.
            let mut d = match info {
                Some(Object::Dict(d)) => d,
                _ => Dict::new(),
            };
            if d.get(b"Trapped") != Some(&value) {
                d.set(b"Trapped".to_vec(), value);
                let r = doc.add(Object::Dict(d));
                doc.trailer_mut().set(b"Info".to_vec(), Object::Ref(r));
                n += 1;
            }
        }
    }
    // The metadata says it too, where it says it at all.
    if let Some(Object::Ref(m)) = crate::catalog(doc).get(b"Metadata").cloned()
        && let Object::Stream(s) = &*doc.get(m)
        && let Ok(bytes) = s.decoded()
        && let Ok(text) = String::from_utf8(bytes)
        && let Some(updated) = set_xmp_trapped(&text, word)
    {
        let mut dict = s.dict.clone();
        // Written uncompressed, as Save as PDF/A writes metadata, so other tools can find it.
        dict.remove(b"Filter");
        dict.remove(b"DecodeParms");
        doc.set(m, Object::Stream(Stream::from_raw(dict, updated.into_bytes())));
        n += 1;
    }
    Ok(n)
}

/// The packet with its `pdf:Trapped` value replaced by `word`, written as an attribute or an
/// element; `None` when it has none or already says `word`.
fn set_xmp_trapped(xmp: &str, word: &str) -> Option<String> {
    for (open, close) in [("pdf:Trapped=\"", "\""), ("pdf:Trapped='", "'"), ("<pdf:Trapped>", "</pdf:Trapped>")] {
        let Some(at) = xmp.find(open) else { continue };
        // Every pattern here is ASCII, so these are char boundaries.
        let start = at + open.len();
        let end = start + xmp.get(start..)?.find(close)?;
        if xmp.get(start..end)?.trim() == word {
            return None;
        }
        return Some(format!("{}{word}{}", xmp.get(..start)?, xmp.get(end..)?));
    }
    None
}

/// How far an action's `/Next` chain is followed.
const MAX_CHAIN: u8 = 8;

/// What an action runs that preflight doesn't allow: JavaScript, a program (Launch), or a
/// rendition with a script.
pub(super) fn kind(d: &Dict) -> Option<&'static str> {
    match d.name(b"S")? {
        b"JavaScript" => Some("JavaScript"),
        b"Launch" => Some("Launch"),
        b"Rendition" if d.get(b"JS").is_some() => Some("Rendition"),
        _ => None,
    }
}

/// The first action in the chain `o` starts that runs what preflight doesn't allow.
pub(super) fn runs(doc: &Document, o: &Object) -> Option<&'static str> {
    runs_within(doc, o, 0)
}

fn runs_within(doc: &Document, o: &Object, depth: u8) -> Option<&'static str> {
    if depth > MAX_CHAIN {
        return None;
    }
    let a = doc.resolve(o);
    let d = a.as_dict()?;
    if let Some(k) = kind(d) {
        return Some(k);
    }
    let next = doc.resolve(d.get(b"Next")?);
    match &*next {
        Object::Array(list) => list.iter().find_map(|x| runs_within(doc, x, depth.saturating_add(1))),
        other => runs_within(doc, other, depth.saturating_add(1)),
    }
}

/// Take a holder's additional actions, and its action when the chain runs a script or a program.
fn strip(doc: &Document, d: &mut Dict) -> bool {
    let mut changed = d.remove(b"AA").is_some();
    if d.get(b"A").is_some_and(|a| runs(doc, a).is_some()) {
        d.remove(b"A");
        changed = true;
    }
    changed
}

fn remove_scripts(doc: &mut Document) -> Result<usize, CosError> {
    let mut n = 0;
    let root = doc.root();
    if let Some(root) = root {
        let mut cat = crate::catalog(doc);
        let mut changed = cat.remove(b"AA").is_some();
        if cat.get(b"OpenAction").is_some_and(|a| runs(doc, a).is_some()) {
            cat.remove(b"OpenAction");
            changed = true;
        }
        match cat.get(b"Names").cloned() {
            Some(Object::Ref(r)) if doc.get(r).as_dict().is_some_and(|d| d.contains(b"JavaScript")) => {
                doc.update_dict(r, |names| {
                    names.remove(b"JavaScript");
                })?;
                n += 1;
            }
            Some(Object::Dict(mut names)) if names.contains(b"JavaScript") => {
                names.remove(b"JavaScript");
                cat.set(b"Names".to_vec(), Object::Dict(names));
                changed = true;
            }
            _ => {}
        }
        if changed {
            doc.update_dict(root, |c| *c = cat)?;
            n += 1;
        }
    }
    // Pages, annotations, fields and outline items: what they run.
    for (r, o) in crate::objects(doc) {
        if Some(r) == root {
            continue;
        }
        let Object::Dict(d) = &*o else { continue };
        let mut nd = d.clone();
        let mut changed = strip(doc, &mut nd);
        // Annotations written into a page rather than as objects of their own.
        if let Some(Object::Array(annots)) = nd.get(b"Annots").cloned() {
            let mut annots = annots;
            let mut any = false;
            for a in annots.iter_mut() {
                if let Object::Dict(ad) = a {
                    any |= strip(doc, ad);
                }
            }
            if any {
                nd.set(b"Annots".to_vec(), Object::Array(annots));
                changed = true;
            }
        }
        if changed {
            doc.set(r, Object::Dict(nd));
            n += 1;
        }
    }
    // The actions themselves: nothing refers to them any more, and emptied they can't run.
    for (r, o) in crate::objects(doc) {
        if let Object::Dict(d) = &*o
            && crate::is_action(d)
            && kind(d).is_some()
        {
            doc.set(r, Object::Null);
            n += 1;
        }
    }
    Ok(n)
}
