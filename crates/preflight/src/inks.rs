//! The inks a document names: every colorant of a `Separation` or `DeviceN` colour space
//! (ISO 32000-1 8.6.6.4 and 8.6.6.5), the alternate spaces it is defined with, and the pages
//! whose resources can draw with it.
//!
//! Read only. A page counts when its resources, the forms, patterns, shadings, images and Type 3
//! fonts they hold, or its annotations' appearances name the ink, whether or not its content
//! paints with it.
//!
//! Adapted from pdfcer's `crates/pdfcer-core/src/prepress/inks.rs` at b76734ff (same author, MIT).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use pdfcraft_cos::{Document, ObjRef, Object};

/// How deep a direct object is followed: as deep as the parser nests them.
const MAX_DEPTH: u8 = 128;
/// Objects, direct or indirect, one document's page walks look at in all. Past it, pages are not
/// followed further and [`InkList::complete`] is false.
const MAX_VISITS: usize = 1 << 22;
/// Distinct spot inks listed; the four process inks are always kept. Further names are counted in
/// [`InkList::dropped`].
pub const MAX_INKS: usize = 1024;
/// Distinct alternate spaces kept for one ink. Past it, [`InkList::complete`] is false.
pub const MAX_ALTERNATES: usize = 16;
/// Colorants read from one `DeviceN` space: the most a reader has to accept (ISO 32000-1
/// Annex C). Past it the rest aren't read and [`InkList::complete`] is false, so a names array
/// shared by many spaces costs each of them at most this much.
pub const MAX_COLORANTS: usize = 32;
/// Distinct dropped names remembered, to count them; past it each further sighting counts.
const MAX_DROPPED_NAMES: usize = 1 << 16;
/// The process colorants, in plate order.
const PROCESS: [&str; 4] = ["Cyan", "Magenta", "Yellow", "Black"];

/// One ink a document names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ink {
    /// The colorant name: its bytes as UTF-8, with `#` and any bytes that aren't UTF-8 written
    /// `#XX` as in a PDF name, so two different names never read the same.
    pub name: String,
    /// `Cyan`, `Magenta`, `Yellow` or `Black`, which print on the process plates.
    pub process: bool,
    /// The family of each alternate space it is defined with (`DeviceCMYK`, `ICCBased`, `Lab`,
    /// ...), sorted. More than one means the definitions disagree on how the ink looks without
    /// its own plate. An alternate whose family can't be read is left out.
    pub alternates: Vec<String>,
    /// 0-based pages whose resources name it, ascending. Empty when only objects no page draws
    /// from name it.
    pub pages: Vec<usize>,
}

/// Every ink a document names, from [`inks`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InkList {
    /// Process inks first, in plate order, then spot inks by name.
    pub inks: Vec<Ink>,
    /// Names past [`MAX_INKS`], not listed: distinct up to 65 536 of them, and past that each
    /// further sighting counts, so the count is approximate.
    pub dropped: usize,
    /// False when a walk stopped at a limit (its allowance, its depth, [`MAX_ALTERNATES`] for one
    /// ink, or [`MAX_COLORANTS`] for one space), so an ink, a page or an alternate may be missing. It covers the pages
    /// [`pdfcraft_model::pages`] yields; a reference to a missing object counts as null.
    pub complete: bool,
}

#[derive(Default)]
struct Found {
    alternates: BTreeSet<String>,
    pages: BTreeSet<usize>,
}

/// The inks named anywhere in `doc`, and the pages of `pages` (as [`pdfcraft_model::pages`] lists
/// them) whose resources name each. `All` and `None` are not inks.
pub fn inks(doc: &Document, pages: &[pdfcraft_model::Page]) -> InkList {
    let page_refs = pages.iter().map(|p| p.obj).collect();
    let mut scan = Scan { doc, found: BTreeMap::new(), dropped: BTreeSet::new(), dropped_more: 0, page: None, page_refs, capped: false };
    for n in doc.object_numbers() {
        let o = doc.get(ObjRef::new(n, doc.generation(n)));
        scan.walk(&o, 0);
    }
    let mut visits = MAX_VISITS;
    let mut complete = true;
    for (i, p) in pages.iter().enumerate() {
        scan.page = Some(i);
        let mut seen = HashSet::new();
        complete &= scan.page_walk(&p.dict, &mut seen, &mut visits);
    }
    let mut inks: Vec<Ink> = scan
        .found
        .into_iter()
        .map(|(name, f)| Ink {
            process: PROCESS.contains(&name.as_str()),
            name,
            alternates: f.alternates.into_iter().collect(),
            pages: f.pages.into_iter().collect(),
        })
        .collect();
    inks.sort_by_key(|i| (!i.process, PROCESS.iter().position(|p| *p == i.name), i.name.clone()));
    let dropped = scan.dropped.len().saturating_add(scan.dropped_more);
    InkList { inks, dropped, complete: complete && !scan.capped }
}

struct Scan<'a> {
    doc: &'a Document,
    found: BTreeMap<String, Found>,
    /// Names past [`MAX_INKS`], kept to count them once each; at most one per name met.
    dropped: BTreeSet<String>,
    /// Sightings of dropped names past [`MAX_DROPPED_NAMES`].
    dropped_more: usize,
    /// The page being walked, when the walk is a page's.
    page: Option<usize>,
    /// The page objects, so a walk stops at one even when it has no `/Type`.
    page_refs: HashSet<ObjRef>,
    /// An ink had more alternates than [`MAX_ALTERNATES`].
    capped: bool,
}

impl Scan<'_> {
    /// Look at every array inside `o`, references not followed.
    fn walk(&mut self, o: &Object, depth: u8) {
        if depth > MAX_DEPTH {
            return;
        }
        let deeper = depth.saturating_add(1);
        match o {
            Object::Array(a) => {
                self.space(a);
                a.iter().for_each(|v| self.walk(v, deeper));
            }
            Object::Dict(d) => d.iter().for_each(|(_, v)| self.walk(v, deeper)),
            Object::Stream(s) => s.dict.iter().for_each(|(_, v)| self.walk(v, deeper)),
            _ => {}
        }
    }

    /// A page's resources and its annotations' appearances, within `visits`. False when the
    /// walk stopped at a limit.
    fn page_walk(&mut self, page: &pdfcraft_cos::Dict, seen: &mut HashSet<ObjRef>, visits: &mut usize) -> bool {
        let mut complete = page.get(b"Resources").is_none_or(|r| self.follow(r, 0, seen, visits));
        let annots = page.get(b"Annots").map(|a| self.doc.resolve(a));
        for annot in annots.as_deref().and_then(Object::as_array).into_iter().flatten() {
            let Some(left) = visits.checked_sub(1) else { return false };
            *visits = left;
            let annot = self.doc.resolve(annot);
            if let Some(ap) = annot.as_dict().and_then(|d| d.get(b"AP")) {
                complete &= self.follow(ap, 0, seen, visits);
            }
        }
        complete
    }

    /// Look at `o` and everything it refers to, except page objects and page-tree nodes (a link
    /// or an annotation's `/P` leads to another page's resources), within `visits`. False when `visits` ran out or
    /// the nesting passed [`MAX_DEPTH`] somewhere; the rest is still looked at.
    fn follow(&mut self, o: &Object, depth: u8, seen: &mut HashSet<ObjRef>, visits: &mut usize) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        let Some(left) = visits.checked_sub(1) else { return false };
        *visits = left;
        let deeper = depth.saturating_add(1);
        match o {
            Object::Ref(r) => {
                if !seen.insert(*r) {
                    return true;
                }
                if self.page_refs.contains(r) {
                    return true;
                }
                let o = self.doc.get(*r);
                is_page(&o) || self.follow(&o, deeper, seen, visits)
            }
            Object::Array(a) => {
                self.space(a);
                a.iter().map(|v| self.follow(v, deeper, seen, visits)).fold(true, |ok, done| ok & done)
            }
            Object::Dict(d) => d.iter().map(|(_, v)| self.follow(v, deeper, seen, visits)).fold(true, |ok, done| ok & done),
            Object::Stream(s) => s.dict.iter().map(|(_, v)| self.follow(v, deeper, seen, visits)).fold(true, |ok, done| ok & done),
            _ => true,
        }
    }

    /// Record the inks of `a` when it is `[/Separation name alt tint]` or
    /// `[/DeviceN names alt tint attributes?]`.
    fn space(&mut self, a: &[Object]) {
        let head = a.first().map(|v| self.doc.resolve(v));
        let separation = match head.as_deref().and_then(Object::as_name) {
            Some(b"Separation") => true,
            Some(b"DeviceN") => false,
            _ => return,
        };
        let (Some(names), Some(alternate)) = (a.get(1).map(|n| self.doc.resolve(n)), a.get(2)) else { return };
        let alternate = self.family(alternate);
        let alternate = alternate.as_deref();
        let names: Vec<Vec<u8>> = if separation {
            names.as_name().map(<[u8]>::to_vec).into_iter().collect()
        } else {
            let names = names.as_array().map(Vec::as_slice).unwrap_or_default();
            self.capped |= names.len() > MAX_COLORANTS;
            names.iter().take(MAX_COLORANTS).filter_map(|n| self.doc.resolve(n).as_name().map(<[u8]>::to_vec)).collect()
        };
        for name in names.iter().filter(|n| n.as_slice() != b"All" && n.as_slice() != b"None") {
            self.record(name_text(name), alternate);
        }
    }

    fn record(&mut self, name: String, alternate: Option<&str>) {
        let process = PROCESS.contains(&name.as_str());
        let spots = self.found.len().saturating_sub(PROCESS.iter().filter(|p| self.found.contains_key(**p)).count());
        if !process && !self.found.contains_key(&name) && spots >= MAX_INKS {
            if self.dropped.len() < MAX_DROPPED_NAMES || self.dropped.contains(&name) {
                self.dropped.insert(name);
            } else {
                self.dropped_more = self.dropped_more.saturating_add(1);
            }
            return;
        }
        let page = self.page;
        let f = self.found.entry(name).or_default();
        if let Some(alternate) = alternate.filter(|a| !f.alternates.contains(*a)) {
            if f.alternates.len() < MAX_ALTERNATES {
                f.alternates.insert(alternate.to_owned());
            } else {
                self.capped = true;
            }
        }
        f.pages.extend(page);
    }

    /// The family name of a colour space: its name, or its array's first entry.
    fn family(&self, cs: &Object) -> Option<String> {
        let cs = self.doc.resolve(cs);
        let name = match &*cs {
            Object::Array(a) => a.first().and_then(|v| self.doc.resolve(v).as_name().map(<[u8]>::to_vec)),
            o => o.as_name().map(<[u8]>::to_vec),
        };
        name.map(|n| name_text(&n))
    }
}

/// `name` as text that keeps distinct names distinct: UTF-8 as is, `#` and bytes that aren't UTF-8
/// as `#XX`.
fn name_text(name: &[u8]) -> String {
    let mut out = String::with_capacity(name.len());
    for chunk in name.utf8_chunks() {
        for c in chunk.valid().chars() {
            if c == '#' {
                out.push_str("#23");
            } else {
                out.push(c);
            }
        }
        for b in chunk.invalid() {
            out.push_str(&format!("#{b:02X}"));
        }
    }
    out
}

/// A page or page-tree node.
fn is_page(o: &Object) -> bool {
    o.as_dict().and_then(|d| d.get(b"Type")).and_then(Object::as_name).is_some_and(|t| t == b"Page" || t == b"Pages")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use pdfcraft_cos::Document;

    use super::{InkList, MAX_ALTERNATES, MAX_COLORANTS, MAX_INKS, inks};

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

    /// An array of `DeviceN` spaces naming `/S0` to `/S{count - 1}`, at most [`MAX_COLORANTS`]
    /// to a space, each tinted by function `f`.
    fn floods(count: usize, f: usize) -> String {
        let names: Vec<String> = (0..count).map(|i| format!("/S{i}")).collect();
        let spaces: String = names.chunks(MAX_COLORANTS).map(|c| format!("[/DeviceN [{}] /DeviceCMYK {f} 0 R] ", c.join(" "))).collect();
        format!("[{spaces}]")
    }

    fn list(objs: &[&str]) -> InkList {
        let doc = build(&objs.iter().map(|o| (*o).to_owned()).collect::<Vec<_>>());
        inks(&doc, &pdfcraft_model::pages(&doc))
    }

    #[test]
    fn inks_are_listed_with_their_alternates_and_the_pages_that_draw_from_them() {
        let found = list(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 200] >>",
            // Page 1: a spot in its own resources, and a form whose resources name process and spot.
            "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /P [/Separation /Gold /DeviceCMYK 7 0 R] >> /XObject << /F 5 0 R >> >> >>",
            // Page 2: the same spot defined differently, in an annotation's appearance whose
            // form points back at page 1, and a link to page 1. Neither makes page 1's inks page
            // 2's.
            "<< /Type /Page /Parent 2 0 R /Annots [6 0 R 9 0 R] >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /ColorSpace << /N [/DeviceN [/Cyan /Silver /None] /DeviceRGB 7 0 R] >> >> /Length 0 >>\nstream\n\nendstream",
            "<< /Type /Annot /Subtype /Square /Rect [0 0 1 1] /P 4 0 R /AP << /N 8 0 R >> >>",
            "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0 1 0] /N 1 >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Page 3 0 R /Resources << /ColorSpace << /G [/Separation /Gold [/Lab << /WhitePoint [1 1 1] >>] 7 0 R] >> >> /Length 0 >>\nstream\n\nendstream",
            "<< /Type /Annot /Subtype /Link /Rect [0 0 1 1] /Dest [3 0 R /Fit] >>",
            // Named, but drawn from by no page.
            "[/Separation /Unused /DeviceGray 7 0 R]",
            "[/Separation /All /DeviceGray 7 0 R]",
        ]);
        let names: Vec<&str> = found.inks.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Cyan", "Gold", "Silver", "Unused"], "process first, then spots by name; All and None aren't inks");
        let ink = |n: &str| found.inks.iter().find(|i| i.name == n).unwrap();
        assert!(ink("Cyan").process && !ink("Gold").process);
        assert_eq!(ink("Gold").alternates, ["DeviceCMYK", "Lab"], "the two definitions disagree");
        assert_eq!(ink("Gold").pages, [0, 1]);
        assert_eq!(ink("Silver").pages, [0], "a form's resources count; a link to the page doesn't");
        assert!(ink("Unused").pages.is_empty());
        assert!(found.complete && found.dropped == 0);
    }

    #[test]
    fn a_resource_cycle_ends_and_a_flood_of_names_is_cut_off() {
        let devicen = floods(MAX_INKS + 10, 6);
        let found = list(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources 4 0 R >>",
            // The page's resources hold a form whose resources are the page's again.
            "<< /XObject << /F 5 0 R >> /ColorSpace << /N 7 0 R >> >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources 4 0 R /Length 0 >>\nstream\n\nendstream",
            "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0 1 0] /N 1 >>",
            &devicen,
        ]);
        assert!(found.inks.len() <= MAX_INKS && found.dropped > 0, "{} listed, {} dropped", found.inks.len(), found.dropped);
        assert!(found.inks.iter().all(|i| i.pages == [0]));
        assert!(found.complete, "a cycle is not a limit");
    }

    #[test]
    fn process_inks_survive_a_flood_and_too_many_alternates_leave_the_list_incomplete() {
        let flood = floods(MAX_INKS + 10, 4);
        let alternates: String = (0..=MAX_ALTERNATES).map(|i| format!("/A{i} [/Separation /Gold /Alt{i} 4 0 R] ")).collect();
        let found = list(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            &format!("<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << {alternates} /F 5 0 R /K 6 0 R >> >> >>"),
            "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0 1 0] /N 1 >>",
            &flood,
            // Met after the flood, in object order.
            "[/Separation /Black /DeviceCMYK 4 0 R]",
        ]);
        assert!(found.inks.iter().any(|i| i.name == "Black" && i.process), "a process ink met after the cap is kept");
        assert!(!found.complete, "an ink's alternates were cut off");
    }

    #[test]
    fn a_names_array_longer_than_a_space_can_be_is_read_only_to_the_limit() {
        let names: String = (0..=MAX_COLORANTS).map(|i| format!("/S{i} ")).collect();
        let found = list(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /A 6 0 R /B 7 0 R >> >> >>",
            "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0 1 0] /N 1 >>",
            &format!("[{names}]"),
            // Two spaces share the one names array.
            "[/DeviceN 5 0 R /DeviceCMYK 4 0 R]",
            "[/DeviceN 5 0 R /DeviceRGB 4 0 R]",
        ]);
        assert_eq!(found.inks.len(), MAX_COLORANTS, "the names past the limit aren't read");
        assert!(!found.complete, "a space was cut off");
    }

    #[test]
    fn a_page_with_no_type_is_still_a_page_the_walk_stops_at() {
        let found = list(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 200 200] >>",
            // Page 1, with no /Type, names Silver.
            "<< /Parent 2 0 R /Resources << /ColorSpace << /S [/Separation /Silver /DeviceGray 7 0 R] >> >> >>",
            // Page 2's annotation appearance points back at page 1.
            "<< /Type /Page /Parent 2 0 R /Annots [5 0 R] >>",
            "<< /Type /Annot /Subtype /Square /Rect [0 0 1 1] /AP << /N 6 0 R >> >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Page 3 0 R /Length 0 >>
stream

endstream",
            "<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>",
        ]);
        let silver = found.inks.iter().find(|i| i.name == "Silver").map(|i| i.pages.clone());
        assert_eq!(silver, Some(vec![0]), "page 1's ink isn't page 2's");
    }

    #[test]
    fn names_that_differ_only_in_bytes_that_arent_text_stay_two_inks() {
        let found = list(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>",
            "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /A [/Separation /Spot#80 /DeviceGray 4 0 R] /B [/Separation /Spot#81 /DeviceGray 4 0 R] /C [/Separation /Spot#2380 /DeviceGray 4 0 R] >> >> >>",
            "<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>",
        ]);
        let names: BTreeSet<&str> = found.inks.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names.len(), 3, "three different names: {names:?}");
    }

    #[test]
    fn resources_nested_past_the_depth_limit_leave_the_list_incomplete() {
        let mut objs = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /C 4 0 R /D [/Separation /Beside /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>] >> >> >>".to_owned(),
        ];
        // A chain of references deeper than the walk goes, a spot at its far end.
        let links = usize::from(super::MAX_DEPTH) + 10;
        objs.extend((0..links).map(|i| format!("[{} 0 R]", i + 5)));
        objs.push("[/Separation /Deep /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>]".to_owned());
        let doc = build(&objs);
        let found = inks(&doc, &pdfcraft_model::pages(&doc));
        assert!(!found.complete, "the page walk stopped before the end of the chain");
        let beside = found.inks.iter().find(|i| i.name == "Beside").map(|i| i.pages.clone());
        assert_eq!(beside, Some(vec![0]), "the rest of the page is still read past the limit");
    }
}
