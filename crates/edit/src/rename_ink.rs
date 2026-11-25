//! Rename a spot ink. Every `Separation` and `DeviceN` colour space that names it (ISO 32000-1
//! 8.6.6.4, 8.6.6.5) names the new ink instead, with the `DeviceN` attributes keyed by it
//! (`/Colorants`, and `/MixingHints` `/Solidities`, `/DotGain` and `/PrintingOrder`) and a page's
//! `/SeparationInfo /DeviceColorant` (14.11.4). Renaming to an ink the file already names makes
//! the two print on that one plate.
//!
//! Each space keeps its own alternate space and tint transform, so the page looks the same on
//! screen; [`InkRenamed::alternates_differ`] says when the two inks were defined with different
//! alternates. A `DeviceN` space naming both inks is left as it is and counted in
//! [`InkRenamed::refused`]: renaming would give it two components of one ink.
//!
//! Ink names are given as text: UTF-8, with `#` followed by two hex digits for any other byte
//! and for `#` itself, as in a PDF name (ISO 32000-1 7.3.5), so a name listed that way can be
//! renamed.
//!
//! Content streams aren't read or rewritten: they name colour spaces by resource name, not by
//! ink. An inline image's own colour space keeps the old name.
//!
//! Adapted from pdfcer's `crates/pdfcer-core/src/prepress/ink_alias.rs` and
//! `crates/pdfcer-core/src/edit/ink_manager.rs` at b76734ff (same author, MIT).

use std::collections::{BTreeMap, BTreeSet};

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::EditError;

/// Nesting followed inside one object. Past it, [`InkRenamed::complete`] is false.
const MAX_DEPTH: u8 = 64;
/// Values, direct or indirect, one rename looks at in all. Past it, [`InkRenamed::complete`] is
/// false.
const MAX_VISITS: usize = 1 << 22;
/// The longest name a PDF reader has to accept (ISO 32000-1 Annex C).
pub const MAX_INK_NAME: usize = 127;
/// The process colorants, which name the device's own plates.
const PROCESS: [&[u8]; 4] = [b"Cyan", b"Magenta", b"Yellow", b"Black"];

/// What [`rename_ink`] changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InkRenamed {
    /// Colour spaces, and pages' `/SeparationInfo` colorants, that named the ink and now name
    /// the new one.
    pub spaces: usize,
    /// Objects rewritten.
    pub objects: usize,
    /// `DeviceN` spaces naming both inks, left as they were.
    pub refused: usize,
    /// The new name was already an ink in the file, so two inks now print as one.
    pub merged: bool,
    /// The two inks were defined with different alternate space families.
    pub alternates_differ: bool,
    /// False when the walk stopped at a limit or an object couldn't be read, so a space may
    /// still name the old ink.
    pub complete: bool,
}

/// The bytes of ink name `name`, given as text with `#XX` for a byte (see the module doc), or
/// `None` when a `#` isn't followed by two hex digits.
#[must_use]
pub fn ink_name_bytes(name: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(name.len());
    let mut rest = name.as_bytes();
    while let Some((&b, tail)) = rest.split_first() {
        if b == b'#' {
            let hex = tail.get(..2).filter(|h| h.iter().all(u8::is_ascii_hexdigit)).and_then(|h| std::str::from_utf8(h).ok())?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            rest = tail.get(2..).unwrap_or_default();
        } else {
            out.push(b);
            rest = tail;
        }
    }
    Some(out)
}

/// Why `from` can't be renamed to `to`, or `None` when it can.
#[must_use]
pub fn ink_rename_refusal(from: &str, to: &str) -> Option<String> {
    match (ink_name_bytes(from), ink_name_bytes(to)) {
        (Some(from), Some(to)) => byte_refusal(&from, &to),
        _ => Some("a # in an ink name must be followed by two hex digits".into()),
    }
}

fn byte_refusal(from: &[u8], to: &[u8]) -> Option<String> {
    let reserved = |n: &[u8]| n == b"All" || n == b"None";
    if from.is_empty() || to.is_empty() {
        return Some("an ink name can't be empty".into());
    }
    if from.len() > MAX_INK_NAME || to.len() > MAX_INK_NAME {
        return Some(format!("an ink name can be at most {MAX_INK_NAME} bytes"));
    }
    if reserved(from) || reserved(to) {
        return Some("All and None aren't inks".into());
    }
    if PROCESS.contains(&from) {
        return Some(format!("{} is a process ink and can't be renamed", String::from_utf8_lossy(from)));
    }
    if from == to {
        return Some("the ink already has that name".into());
    }
    None
}

/// Rename spot ink `from` to `to` in every colour space that names it; see the module doc.
/// `to` may be a process ink (`Black`) or another spot. Nothing changes when no space names
/// `from`: the counts are zero.
///
/// # Errors
///
/// [`EditError::Invalid`] when [`ink_rename_refusal`] refuses the names.
pub fn rename_ink(doc: &mut Document, from: &str, to: &str) -> Result<InkRenamed, EditError> {
    if let Some(why) = ink_rename_refusal(from, to) {
        return Err(EditError::Invalid(why));
    }
    let (Some(from), Some(to)) = (ink_name_bytes(from), ink_name_bytes(to)) else {
        return Err(EditError::Invalid("a # in an ink name must be followed by two hex digits".into()));
    };
    let mut r = Renamer::new(doc, &from, &to);
    for (at, read) in doc.scan_objects_checked() {
        if read.is_err() {
            r.complete = false;
            continue;
        }
        let mut obj = r.current(at);
        if r.rename(&mut obj, 0) {
            r.work.insert(at, obj);
        }
    }
    if r.work.keys().any(|at| r.protected.contains(at)) {
        return Err(EditError::Invalid(
            "a DeviceN space naming both inks shares objects with a space being renamed, so renaming would change it; nothing was changed".into(),
        ));
    }
    let alternates_differ = !r.from_alternates.is_empty() && !r.to_alternates.is_empty() && r.from_alternates != r.to_alternates;
    let mut report = r.report;
    report.objects = r.work.len();
    report.alternates_differ = alternates_differ;
    report.complete = r.complete;
    for (at, obj) in r.work {
        doc.set(at, obj);
    }
    Ok(report)
}

struct Renamer<'d> {
    doc: &'d Document,
    from: &'d [u8],
    to: &'d [u8],
    /// Objects rewritten so far, by reference.
    work: BTreeMap<ObjRef, Object>,
    report: InkRenamed,
    from_alternates: BTreeSet<Vec<u8>>,
    to_alternates: BTreeSet<Vec<u8>>,
    /// Objects a refused `DeviceN` reaches through its names or attributes: none may change.
    protected: BTreeSet<ObjRef>,
    visits: usize,
    complete: bool,
}

impl<'d> Renamer<'d> {
    fn new(doc: &'d Document, from: &'d [u8], to: &'d [u8]) -> Self {
        Self {
            doc,
            from,
            to,
            work: BTreeMap::new(),
            report: InkRenamed::default(),
            from_alternates: BTreeSet::new(),
            to_alternates: BTreeSet::new(),
            protected: BTreeSet::new(),
            visits: MAX_VISITS,
            complete: true,
        }
    }

    /// An object with this rename's earlier rewrites applied.
    fn current(&self, at: ObjRef) -> Object {
        self.work.get(&at).cloned().unwrap_or_else(|| (*self.doc.get(at)).clone())
    }

    /// A value as the file has it, through one reference.
    fn original(&self, o: &Object) -> Object {
        match o {
            Object::Ref(r) => (*self.doc.get(*r)).clone(),
            other => other.clone(),
        }
    }

    /// Apply `f` to `obj`, or, when it is a reference, to the object it names (recorded as a
    /// rewrite). True when `obj` itself changed.
    fn through(&mut self, obj: &mut Object, f: fn(&mut Self, &mut Object) -> bool) -> bool {
        let Object::Ref(at) = obj else {
            return f(self, obj);
        };
        let at = *at;
        let mut target = self.current(at);
        if f(self, &mut target) {
            self.work.insert(at, target);
        }
        false
    }

    /// Spend one visit; false once the allowance is gone.
    fn spend(&mut self) -> bool {
        match self.visits.checked_sub(1) {
            Some(left) => {
                self.visits = left;
                true
            }
            None => {
                self.complete = false;
                false
            }
        }
    }

    /// Rename inside `obj`; true when it changed. References aren't followed: every indirect
    /// object is visited on its own.
    fn rename(&mut self, obj: &mut Object, depth: u8) -> bool {
        if depth > MAX_DEPTH {
            self.complete = false;
            return false;
        }
        if !self.spend() {
            return false;
        }
        let deeper = depth.saturating_add(1);
        match obj {
            Object::Array(items) => self.array(items, deeper),
            Object::Dict(d) => self.dict(d, deeper),
            Object::Stream(s) => self.dict(&mut s.dict, deeper),
            _ => false,
        }
    }

    fn dict(&mut self, d: &mut Dict, depth: u8) -> bool {
        let keys: Vec<Vec<u8>> = d.iter().map(|(k, _)| k.clone()).collect();
        let mut changed = false;
        for key in keys {
            let colorant = key == b"DeviceColorant";
            let Some(value) = d.get_mut(&key) else { continue };
            if colorant && self.is_from(value) {
                *value = Object::Name(self.to.to_vec());
                self.report.spaces = self.report.spaces.saturating_add(1);
                changed = true;
            } else {
                changed |= self.rename(value, depth);
            }
        }
        changed
    }

    /// The name or string `from`, directly or through a reference.
    fn is_from(&self, o: &Object) -> bool {
        match self.original(o) {
            Object::Name(n) => n == self.from,
            Object::String(s) => s.bytes == self.from,
            _ => false,
        }
    }

    /// The family name of an alternate space: its name, or its array's first entry.
    fn family(&self, o: Option<&Object>) -> Option<Vec<u8>> {
        match self.original(o?) {
            Object::Name(n) => Some(n),
            Object::Array(a) => a.first().map(|f| self.original(f)).and_then(|f| f.as_name().map(<[u8]>::to_vec)),
            _ => None,
        }
    }

    /// The inks a `Separation` or `DeviceN` array names, and whether it is a `Separation`;
    /// `None` for any other array.
    fn ink_space(&self, items: &[Object]) -> Option<(bool, Vec<Vec<u8>>)> {
        let family = self.original(items.first()?);
        let separation = match family.as_name()? {
            b"Separation" => true,
            b"DeviceN" => false,
            _ => return None,
        };
        let names = match self.original(items.get(1)?) {
            Object::Name(n) => vec![n],
            Object::Array(a) => a.iter().filter_map(|n| self.original(n).as_name().map(<[u8]>::to_vec)).collect(),
            _ => Vec::new(),
        };
        Some((separation, names))
    }

    fn array(&mut self, items: &mut [Object], depth: u8) -> bool {
        let Some((separation, names)) = self.ink_space(items) else {
            let mut changed = false;
            for item in items.iter_mut() {
                changed |= self.rename(item, depth);
            }
            return changed;
        };
        let names_from = names.iter().any(|n| n == self.from);
        let names_to = names.iter().any(|n| n == self.to);
        let alternate = self.family(items.get(2));
        if !names_from {
            if names_to {
                self.report.merged = true;
                self.to_alternates.extend(alternate);
            }
            return items.get_mut(4).is_some_and(|attrs| self.rename(attrs, depth));
        }
        if !separation && names_to {
            self.report.refused = self.report.refused.saturating_add(1);
            for slot in [items.get(1), items.get(4)].into_iter().flatten() {
                self.protect(slot, depth);
            }
            return false;
        }
        self.report.spaces = self.report.spaces.saturating_add(1);
        self.from_alternates.extend(alternate);
        let mut changed = match items.get_mut(1) {
            Some(name) if self.original(name).as_name().is_some() => self.entry(name),
            Some(names) => self.through(names, Self::entries),
            None => false,
        };
        if let Some(attrs) = items.get_mut(4) {
            changed |= self.through(attrs, Self::attributes);
            changed |= self.rename(attrs, depth);
        }
        changed
    }

    /// Each entry of a names array.
    fn entries(&mut self, obj: &mut Object) -> bool {
        match obj {
            Object::Array(entries) => entries.iter_mut().fold(false, |changed, e| self.entry(e) | changed),
            other => self.entry(other),
        }
    }

    /// One ink name, given directly or through a reference. A reference is replaced by the new
    /// name itself, so an indirect name object that other spaces may share is never rewritten.
    fn entry(&self, e: &mut Object) -> bool {
        if self.original(e).as_name() != Some(self.from) {
            return false;
        }
        *e = Object::Name(self.to.to_vec());
        true
    }

    /// Record every object `o` reaches, through references, as [`Renamer::protected`].
    fn protect(&mut self, o: &Object, depth: u8) {
        if depth > MAX_DEPTH {
            self.complete = false;
            return;
        }
        if !self.spend() {
            return;
        }
        let deeper = depth.saturating_add(1);
        match o {
            Object::Ref(at) => {
                if self.protected.insert(*at) {
                    let target = self.doc.get(*at);
                    self.protect(&target, deeper);
                }
            }
            Object::Array(items) => items.iter().for_each(|i| self.protect(i, deeper)),
            Object::Dict(d) => d.iter().for_each(|(_, v)| self.protect(v, deeper)),
            Object::Stream(s) => s.dict.iter().for_each(|(_, v)| self.protect(v, deeper)),
            _ => {}
        }
    }

    /// A `DeviceN` attributes dictionary: its `/Colorants` and `/MixingHints`.
    fn attributes(&mut self, obj: &mut Object) -> bool {
        let Some(attrs) = obj.as_dict_mut() else { return false };
        let mut changed = attrs.get_mut(b"Colorants").is_some_and(|c| self.through(c, Self::rekey));
        if let Some(hints) = attrs.get_mut(b"MixingHints") {
            changed |= self.through(hints, Self::mixing_hints);
        }
        changed
    }

    fn mixing_hints(&mut self, obj: &mut Object) -> bool {
        let Some(hints) = obj.as_dict_mut() else { return false };
        let mut changed = false;
        for key in [&b"Solidities"[..], b"DotGain"] {
            if let Some(v) = hints.get_mut(key) {
                changed |= self.through(v, Self::rekey);
            }
        }
        if let Some(order) = hints.get_mut(b"PrintingOrder") {
            changed |= self.through(order, Self::reorder);
        }
        changed
    }

    /// Rename the key `from`, or drop it when `to` is already a key, keeping `to`'s value.
    fn rekey(&mut self, obj: &mut Object) -> bool {
        let Some(d) = obj.as_dict_mut() else { return false };
        let Some(value) = d.remove(self.from) else { return false };
        if !d.contains(self.to) {
            d.set(self.to.to_vec(), value);
        }
        true
    }

    /// Rename `from` in a `/PrintingOrder` array, or drop it when `to` is already listed.
    fn reorder(&mut self, obj: &mut Object) -> bool {
        let Object::Array(order) = obj else { return false };
        let is = |o: &Object, n: &[u8]| o.as_name() == Some(n);
        if !order.iter().any(|o| is(o, self.from)) {
            return false;
        }
        if order.iter().any(|o| is(o, self.to)) {
            order.retain(|o| !is(o, self.from));
            true
        } else {
            self.entries(obj)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_cos::{Document, ObjRef, Object};

    use super::{MAX_DEPTH, MAX_INK_NAME, rename_ink};

    /// A document of the objects `objs`, numbered from 1, with object 1 its catalog.
    fn build(objs: &[String]) -> Document {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        Document::open(Arc::new(out)).unwrap()
    }

    fn doc(objs: &[&str]) -> Document {
        build(&objs.iter().map(|o| (*o).to_owned()).collect::<Vec<_>>())
    }

    /// How often `name` appears, as a name or a dictionary key, anywhere in the document.
    fn mentions(doc: &Document, name: &[u8]) -> usize {
        fn count(o: &Object, name: &[u8]) -> usize {
            match o {
                Object::Name(n) => usize::from(n == name),
                Object::Array(a) => a.iter().map(|v| count(v, name)).sum(),
                Object::Dict(d) => d.iter().map(|(k, v)| usize::from(k == name) + count(v, name)).sum(),
                Object::Stream(s) => s.dict.iter().map(|(k, v)| usize::from(k == name) + count(v, name)).sum(),
                _ => 0,
            }
        }
        doc.scan_objects().map(|(_, o)| count(&o, name)).sum()
    }

    fn get(doc: &Document, num: u32) -> Object {
        (*doc.get(ObjRef::new(num, 0))).clone()
    }

    const TINT: &str = "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0 1 0] /N 1 >>";

    #[test]
    fn renaming_an_ink_renames_every_space_and_attribute_that_names_it() {
        let mut d = doc(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            concat!(
                "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /G 4 0 R ",
                "/N [/DeviceN 5 0 R /DeviceCMYK 6 0 R << /Colorants << /Gold [/Separation /Gold /DeviceCMYK 6 0 R] >> ",
                "/MixingHints << /Solidities << /Gold 0.5 >> /DotGain << /Gold 6 0 R >> /PrintingOrder [/Gold /Cyan] >> >>] >> >> ",
                "/SeparationInfo << /Pages [3 0 R] /DeviceColorant /Gold >> >>"
            ),
            "[/Separation /Gold /DeviceCMYK 6 0 R]",
            // The DeviceN names array, given by reference.
            "[/Gold /Cyan]",
            TINT,
        ]);
        let done = rename_ink(&mut d, "Gold", "Silver").unwrap();
        assert_eq!(mentions(&d, b"Gold"), 0, "every mention of the ink is renamed: {done:?}");
        assert!(mentions(&d, b"Silver") >= 6, "{done:?}");
        assert!(done.spaces >= 3 && done.objects >= 2 && done.complete && !done.merged && done.refused == 0, "{done:?}");
        assert_eq!(get(&d, 6), get(&doc(&[TINT]), 1), "tint transforms are kept");
    }

    #[test]
    fn renaming_onto_a_named_ink_merges_them_and_leaves_a_devicen_naming_both() {
        let mut d = doc(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /A 4 0 R /B 5 0 R /C 6 0 R /D 7 0 R >> >> >>",
            "[/Separation /Gold /DeviceCMYK 8 0 R]",
            "[/Separation /Silver /DeviceRGB 8 0 R]",
            "[/DeviceN [/Gold /Silver] /DeviceCMYK 8 0 R]",
            concat!(
                "[/DeviceN [/Gold /Cyan] /DeviceCMYK 8 0 R << /Colorants << /Gold [/Separation /Gold /DeviceCMYK 8 0 R] ",
                "/Silver [/Separation /Silver /DeviceRGB 8 0 R] >> /MixingHints << /PrintingOrder [/Gold /Silver /Cyan] >> >>]"
            ),
            TINT,
        ]);
        let before = get(&d, 6);
        let done = rename_ink(&mut d, "Gold", "Silver").unwrap();
        assert!(done.merged && done.alternates_differ, "{done:?}");
        assert_eq!(done.refused, 1, "{done:?}");
        assert_eq!(get(&d, 6), before, "a DeviceN naming both inks is left as it was");
        let space = get(&d, 7);
        let attrs = space.as_array().and_then(|a| a.get(4)).and_then(Object::as_dict).cloned().unwrap_or_default();
        let colorants = attrs.get(b"Colorants").and_then(Object::as_dict).cloned().unwrap_or_default();
        assert!(colorants.get(b"Gold").is_none(), "{colorants:?}");
        let kept = colorants.get(b"Silver").and_then(Object::as_array).and_then(|a| a.get(2)).and_then(Object::as_name);
        assert_eq!(kept, Some(&b"DeviceRGB"[..]), "the ink already there keeps its own definition");
        let hints = attrs.get(b"MixingHints").and_then(Object::as_dict).cloned().unwrap_or_default();
        let order = hints.get(b"PrintingOrder").and_then(Object::as_array).cloned().unwrap_or_default();
        assert_eq!(order.iter().filter(|o| o.as_name() == Some(b"Silver")).count(), 1, "{order:?}");
    }

    #[test]
    fn an_indirect_name_is_renamed_where_it_is_used_and_never_in_the_object_shared() {
        let mut d = doc(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /A [/DeviceN [5 0 R /Cyan] /DeviceCMYK 4 0 R] /B [/Separation 5 0 R /DeviceCMYK 4 0 R] /C [/DeviceN [5 0 R /Silver] /DeviceCMYK 4 0 R] >> >> >>",
            TINT,
            "/Gold",
        ]);
        let done = rename_ink(&mut d, "Gold", "Silver").unwrap();
        assert_eq!((done.spaces, done.refused), (2, 1), "{done:?}");
        assert_eq!(get(&d, 5), Object::Name(b"Gold".to_vec()), "the shared name object is left alone");
        let spaces = get(&d, 3);
        let cs = |key: &[u8]| {
            let d = spaces
                .as_dict()
                .and_then(|p| p.get(b"Resources"))
                .and_then(Object::as_dict)
                .and_then(|r| r.get(b"ColorSpace"))
                .and_then(Object::as_dict);
            d.and_then(|d| d.get(key)).and_then(Object::as_array).cloned().unwrap_or_default()
        };
        assert_eq!(cs(b"A").get(1).and_then(Object::as_array).and_then(|n| n.first()), Some(&Object::Name(b"Silver".to_vec())));
        assert_eq!(cs(b"B").get(1), Some(&Object::Name(b"Silver".to_vec())));
        assert_eq!(
            cs(b"C").get(1).and_then(Object::as_array).and_then(|n| n.first()),
            Some(&Object::Ref(ObjRef::new(5, 0))),
            "the refused space still names Gold"
        );
    }

    #[test]
    fn a_refused_devicen_sharing_objects_with_a_renamed_space_refuses_the_rename() {
        let mut d = doc(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /A 5 0 R /B [/DeviceN [/Gold /Silver] /DeviceCMYK 4 0 R << /Colorants << /Gold 5 0 R >> >>] >> >> >>",
            TINT,
            "[/Separation /Gold /DeviceCMYK 4 0 R]",
        ]);
        assert!(rename_ink(&mut d, "Gold", "Silver").is_err());
        assert!(!d.is_modified(), "nothing was changed");
    }

    #[test]
    fn names_that_cant_be_renamed_are_refused_and_an_unnamed_ink_changes_nothing() {
        let mut d = doc(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /G [/Separation /Gold /DeviceCMYK 4 0 R] >> >> >>",
            TINT,
        ]);
        let long = "x".repeat(MAX_INK_NAME + 1);
        for (from, to) in [("Black", "Gold"), ("All", "Gold"), ("Gold", "None"), ("Gold", "Gold"), ("", "X"), ("Gold", long.as_str())] {
            assert!(rename_ink(&mut d, from, to).is_err(), "{from} -> {to}");
        }
        let done = rename_ink(&mut d, "Rust", "Silver").unwrap();
        assert_eq!((done.spaces, done.objects), (0, 0), "{done:?}");
        assert!(!d.is_modified());
    }

    #[test]
    fn a_name_given_with_hex_escapes_is_the_ink_with_those_bytes() {
        let mut d = doc(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /A [/Separation /Spot#80 /DeviceCMYK 4 0 R] /B [/Separation /No#232 /DeviceCMYK 4 0 R] >> >> /SeparationInfo << /Pages [3 0 R] /DeviceColorant /Plate#80 >> >>",
            TINT,
        ]);
        assert_eq!(rename_ink(&mut d, "Spot#80", "Gold#20Leaf").unwrap().spaces, 1);
        assert_eq!(rename_ink(&mut d, "No#232", "Silver").unwrap().spaces, 1, "#23 is a literal #");
        assert_eq!(rename_ink(&mut d, "Plate#80", "Copper").unwrap().spaces, 1, "a page's separation colorant alone counts");
        assert_eq!(mentions(&d, b"Gold Leaf"), 1);
        assert_eq!(mentions(&d, b"Silver") + mentions(&d, b"Copper"), 2);
        assert_eq!(mentions(&d, b"Spot\x80") + mentions(&d, b"No#2") + mentions(&d, b"Plate\x80"), 0);
        for bad in ["Gold#2", "Gold#+1", "Gold#zz"] {
            assert!(rename_ink(&mut d, bad, "X").is_err(), "{bad}: a # needs two hex digits");
        }
        let at_limit = "#41".repeat(MAX_INK_NAME);
        assert!(rename_ink(&mut d, "Silver", &at_limit).is_ok(), "the limit is on the decoded bytes");
        let past = "#41".repeat(MAX_INK_NAME + 1);
        assert!(rename_ink(&mut d, &at_limit, &past).is_err(), "one byte past the limit");
    }

    #[test]
    fn nesting_past_the_limit_leaves_the_rename_incomplete() {
        let depth = usize::from(MAX_DEPTH) + 4;
        let nested = format!("{}[/Separation /Gold /DeviceCMYK 4 0 R]{}", "[".repeat(depth), "]".repeat(depth));
        let page = format!(
            "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /G 5 0 R /H [/Separation /Gold /DeviceCMYK 4 0 R] >> >> /Junk {nested} >>"
        );
        let mut d = build(&[
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".to_owned(),
            page,
            TINT.to_owned(),
            "[/Separation /Gold /DeviceCMYK 4 0 R]".to_owned(),
        ]);
        let done = rename_ink(&mut d, "Gold", "Silver").unwrap();
        assert!(!done.complete, "{done:?}");
        assert!(done.spaces >= 2, "what is in reach is still renamed: {done:?}");
    }
}
