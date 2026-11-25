//! The inks a document names: every colorant of a `Separation` or `DeviceN` colour space
//! (ISO 32000-1 8.6.6.4 and 8.6.6.5), the alternate spaces it is defined with, and the pages
//! whose resources can draw with it.
//!
//! Read only. A page counts when its resources, the forms, patterns, shadings, images and Type 3
//! fonts they hold, or its annotations' appearances name the ink, whether or not its content
//! paints with it.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use pdfcraft_cos::{Document, ObjRef, Object};

/// How deep a direct object is followed: as deep as the parser nests them.
const MAX_DEPTH: u8 = 128;
/// Indirect objects one document's page walks visit in all. Past it, pages are not followed
/// further and [`InkList::complete`] is false.
const MAX_VISITS: usize = 1 << 20;
/// Distinct inks listed. Further names are counted in [`InkList::dropped`].
pub const MAX_INKS: usize = 1024;
/// Distinct alternate spaces kept for one ink.
const MAX_ALTERNATES: usize = 16;
/// The process colorants, in plate order.
const PROCESS: [&str; 4] = ["Cyan", "Magenta", "Yellow", "Black"];
/// Entries that lead away from what a page draws: up the page tree, or to the page an annotation
/// is on.
const NOT_FOLLOWED: [&[u8]; 2] = [b"Parent", b"P"];

/// One ink a document names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ink {
    /// The colorant name, as written (lossily decoded).
    pub name: String,
    /// `Cyan`, `Magenta`, `Yellow` or `Black`, which print on the process plates.
    pub process: bool,
    /// The family of each alternate space it is defined with (`DeviceCMYK`, `ICCBased`, `Lab`,
    /// ...), sorted. More than one means the definitions disagree on how the ink looks without
    /// its own plate.
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
    /// Distinct names past [`MAX_INKS`], not listed.
    pub dropped: usize,
    /// False when the page walks ran out of their allowance, so some pages may be missing from
    /// [`Ink::pages`].
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
    let mut scan = Scan { doc, found: BTreeMap::new(), dropped: BTreeSet::new(), page: None };
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
    InkList { inks, dropped: scan.dropped.len(), complete }
}

struct Scan<'a> {
    doc: &'a Document,
    found: BTreeMap<String, Found>,
    /// Names past [`MAX_INKS`], kept to count them once each; at most one per name met.
    dropped: BTreeSet<String>,
    /// The page being walked, when the walk is a page's.
    page: Option<usize>,
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
            let annot = self.doc.resolve(annot);
            if let Some(ap) = annot.as_dict().and_then(|d| d.get(b"AP")) {
                complete &= self.follow(ap, 0, seen, visits);
            }
        }
        complete
    }

    /// Look at `o` and everything it refers to, except up the page tree, within `visits`.
    /// False when `visits` ran out or the nesting passed [`MAX_DEPTH`].
    fn follow(&mut self, o: &Object, depth: u8, seen: &mut HashSet<ObjRef>, visits: &mut usize) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        let deeper = depth.saturating_add(1);
        match o {
            Object::Ref(r) => {
                if !seen.insert(*r) {
                    return true;
                }
                let Some(left) = visits.checked_sub(1) else { return false };
                *visits = left;
                let o = self.doc.get(*r);
                self.follow(&o, deeper, seen, visits)
            }
            Object::Array(a) => {
                self.space(a);
                a.iter().all(|v| self.follow(v, deeper, seen, visits))
            }
            Object::Dict(d) => d.iter().filter(|(k, _)| !NOT_FOLLOWED.contains(&k.as_slice())).all(|(_, v)| self.follow(v, deeper, seen, visits)),
            Object::Stream(s) => s.dict.iter().all(|(_, v)| self.follow(v, deeper, seen, visits)),
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
        let names: Vec<Vec<u8>> = if separation {
            names.as_name().map(<[u8]>::to_vec).into_iter().collect()
        } else {
            names.as_array().map(|n| n.iter().filter_map(|n| self.doc.resolve(n).as_name().map(<[u8]>::to_vec)).collect()).unwrap_or_default()
        };
        for name in names.iter().filter(|n| n.as_slice() != b"All" && n.as_slice() != b"None") {
            self.record(String::from_utf8_lossy(name).into_owned(), &alternate);
        }
    }

    fn record(&mut self, name: String, alternate: &str) {
        if !self.found.contains_key(&name) && self.found.len() >= MAX_INKS {
            self.dropped.insert(name);
            return;
        }
        let page = self.page;
        let f = self.found.entry(name).or_default();
        if f.alternates.len() < MAX_ALTERNATES {
            f.alternates.insert(alternate.to_owned());
        }
        f.pages.extend(page);
    }

    /// The family name of a colour space: its name, or its array's first entry.
    fn family(&self, cs: &Object) -> String {
        let cs = self.doc.resolve(cs);
        let name = match &*cs {
            Object::Array(a) => a.first().and_then(|v| self.doc.resolve(v).as_name().map(<[u8]>::to_vec)),
            o => o.as_name().map(<[u8]>::to_vec),
        };
        name.map_or_else(|| "unreadable".to_owned(), |n| String::from_utf8_lossy(&n).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdfcraft_cos::Document;

    use super::{InkList, MAX_INKS, inks};

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
            "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /A [/Separation /Gold /DeviceCMYK 7 0 R] >> /XObject << /F 5 0 R >> >> >>",
            // Page 2: the same spot defined differently, in an annotation's appearance, and a
            // link whose destination is page 1, which doesn't make page 1's inks page 2's.
            "<< /Type /Page /Parent 2 0 R /Annots [6 0 R 9 0 R] >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /ColorSpace << /N [/DeviceN [/Cyan /Silver /None] /DeviceRGB 7 0 R] >> >> /Length 0 >>\nstream\n\nendstream",
            "<< /Type /Annot /Subtype /Square /Rect [0 0 1 1] /P 4 0 R /AP << /N 8 0 R >> >>",
            "<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 0 1 0] /N 1 >>",
            "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /ColorSpace << /G [/Separation /Gold [/Lab << /WhitePoint [1 1 1] >>] 7 0 R] >> >> /Length 0 >>\nstream\n\nendstream",
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
        let names: String = (0..MAX_INKS + 10).map(|i| format!("/S{i} ")).collect();
        let devicen = format!("[/DeviceN [{names}] /DeviceCMYK 6 0 R]");
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
    }

    #[test]
    fn resources_nested_past_the_depth_limit_leave_the_list_incomplete() {
        let mut objs = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200] >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /C 4 0 R >> >> >>".to_owned(),
        ];
        // A chain of references deeper than the walk goes, a spot at its far end.
        let links = usize::from(super::MAX_DEPTH) + 10;
        objs.extend((0..links).map(|i| format!("[{} 0 R]", i + 5)));
        objs.push("[/Separation /Deep /DeviceGray << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >>]".to_owned());
        let doc = build(&objs);
        let found = inks(&doc, &pdfcraft_model::pages(&doc));
        assert!(!found.complete, "the page walk stopped before the end of the chain");
    }
}
