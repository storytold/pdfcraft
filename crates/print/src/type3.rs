//! Which pages draw with Type 3 fonts, so they can be printed as pictures (#2684).
//!
//! On Windows a job is drawn by `Windows.Data.Pdf` into Direct2D's print control (craft-print),
//! and that component can bring the whole process down on some Type 3 text, such as the Type 3
//! fonts Chrome writes for web fonts. The crash is native, so nothing in PdfCraft can catch it;
//! the pages found here are printed as pictures instead ([`crate::Settings::type3_raster`]).
//!
//! The check reads resources, not content streams: the fonts listed for the page's content and
//! for everything it draws, through form XObjects, tiling patterns and soft masks, and through
//! the appearances of the annotations that print. A page whose resources name a Type 3 font it
//! never uses is printed as a picture too, as is a page whose resources nest too deep or are too
//! large to finish reading: the picture is the safe side.

use std::collections::HashSet;
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::Content;
use crate::imposition::{appearance, prints};

/// How deep resources are followed (each form, pattern or soft mask inside another counts two:
/// the object and its resources); deeper, the page counts as drawing with a Type 3 font.
const MAX_DEPTH: usize = 64;
/// Objects and resource entries one page's check reads; past it, the page counts as drawing with
/// a Type 3 font.
const BUDGET: usize = 20_000;
/// What one call reads for all its pages together (the Print dialog asks about every page of the
/// document as it opens); past it, the pages left count as drawing with a Type 3 font.
const DOCUMENT_BUDGET: usize = 200_000;

/// The pages among `pages` (0-based; each once, in the order given) whose print under `content`
/// draws with a Type 3 font. Pages past the end are left out.
pub fn type3_pages(doc: &Document, pages: &[usize], content: Content) -> Vec<usize> {
    pages_within(doc, pages, content, DOCUMENT_BUDGET)
}

/// [`type3_pages`], reading at most `budget` objects and entries for all the pages together.
/// What a page reads in full without meeting a Type 3 font is not read again for a later page:
/// resources pages share cost their first reading only.
pub(crate) fn pages_within(doc: &Document, pages: &[usize], content: Content, budget: usize) -> Vec<usize> {
    let all = pdfcraft_model::pages(doc);
    let mut checked = HashSet::new();
    let mut found = Vec::new();
    let mut clean = HashSet::new();
    let mut left = budget;
    for &index in pages {
        if !checked.insert(index) {
            continue;
        }
        let Some(page) = all.get(index) else { continue };
        let mut walk = Walk { doc, seen: HashSet::new(), clean: &clean, left: BUDGET.min(left) };
        let type3 = walk.page(&page.dict, content);
        left = left.saturating_sub(BUDGET.min(left) - walk.left);
        let seen = std::mem::take(&mut walk.seen);
        if type3 {
            found.push(index);
        } else {
            // Everything this page reached was read through without a Type 3 font.
            clean.extend(seen);
        }
    }
    found
}

/// One page's reading: the objects already read (so a cycle or a shared resource is read once),
/// those earlier pages read in full without a Type 3 font, and what is left of the budget.
struct Walk<'a> {
    doc: &'a Document,
    seen: HashSet<ObjRef>,
    clean: &'a HashSet<ObjRef>,
    left: usize,
}

impl<'a> Walk<'a> {
    /// One more read; `false` once the budget is spent.
    fn spend(&mut self) -> bool {
        match self.left.checked_sub(1) {
            Some(left) => {
                self.left = left;
                true
            }
            None => false,
        }
    }

    /// `o` resolved, or `None` for a reference this page (or an earlier one, without meeting a
    /// Type 3 font) has already read.
    fn resolve(&mut self, o: &Object) -> Option<Arc<Object>> {
        if let Object::Ref(r) = o
            && (self.clean.contains(r) || !self.seen.insert(*r))
        {
            return None;
        }
        Some(self.doc.resolve(o))
    }

    fn named(&self, d: &Dict, key: &[u8], name: &[u8]) -> bool {
        d.get(key).is_some_and(|v| self.doc.resolve(v).as_name() == Some(name))
    }

    /// Does what `page` prints under `content` draw with a Type 3 font?
    fn page(&mut self, page: &Dict, content: Content) -> bool {
        // Form fields only leaves the page's own content out of the print.
        if content != Content::FormFieldsOnly
            && let Some(res) = page.get(b"Resources")
            && self.resources(res, 0)
        {
            return true;
        }
        let Some(annots) = page.get(b"Annots").map(|a| self.doc.resolve(a)) else { return false };
        for a in annots.as_array().into_iter().flatten() {
            if !self.spend() {
                return true;
            }
            let a = self.doc.resolve(a);
            let Some(d) = a.as_dict() else { continue };
            if prints(self.doc, d, content)
                && let Some(ap) = appearance(self.doc, d)
                && self.xobject(&Object::Ref(ap), 1)
            {
                return true;
            }
        }
        false
    }

    /// Does a resource dictionary, or anything it reaches, hold a Type 3 font?
    fn resources(&mut self, res: &Object, depth: usize) -> bool {
        if depth > MAX_DEPTH || !self.spend() {
            return true;
        }
        let Some(res) = self.resolve(res) else { return false };
        let Some(res) = res.as_dict() else { return false };
        let category = |walk: &mut Self, key: &[u8]| res.get(key).and_then(|c| walk.resolve(c));
        if let Some(fonts) = category(self, b"Font") {
            for (_, font) in fonts.as_dict().into_iter().flat_map(Dict::iter) {
                if !self.spend() {
                    return true;
                }
                let Some(font) = self.resolve(font) else { continue };
                if font.as_dict().is_some_and(|f| self.named(f, b"Subtype", b"Type3")) {
                    return true;
                }
            }
        }
        if let Some(xobjects) = category(self, b"XObject") {
            for (_, x) in xobjects.as_dict().into_iter().flat_map(Dict::iter) {
                if !self.spend() || self.xobject(x, depth + 1) {
                    return true;
                }
            }
        }
        if let Some(patterns) = category(self, b"Pattern") {
            for (_, p) in patterns.as_dict().into_iter().flat_map(Dict::iter) {
                if !self.spend() {
                    return true;
                }
                let Some(p) = self.resolve(p) else { continue };
                let found = match &*p {
                    // A tiling pattern draws with its own resources.
                    Object::Stream(s) => s.dict.get(b"Resources").is_some_and(|r| self.resources(r, depth + 1)),
                    // A shading pattern may carry a graphics state, and so a soft mask.
                    Object::Dict(d) => d.get(b"ExtGState").is_some_and(|g| self.graphics_state(g, depth + 1)),
                    _ => false,
                };
                if found {
                    return true;
                }
            }
        }
        if let Some(states) = category(self, b"ExtGState") {
            for (_, g) in states.as_dict().into_iter().flat_map(Dict::iter) {
                if !self.spend() || self.graphics_state(g, depth + 1) {
                    return true;
                }
            }
        }
        false
    }

    /// A form XObject (or an annotation appearance) and what its resources reach. Images hold no
    /// fonts.
    fn xobject(&mut self, x: &Object, depth: usize) -> bool {
        if depth > MAX_DEPTH {
            return true;
        }
        let Some(x) = self.resolve(x) else { return false };
        let Object::Stream(s) = &*x else { return false };
        if self.named(&s.dict, b"Subtype", b"Image") {
            return false;
        }
        s.dict.get(b"Resources").is_some_and(|r| self.resources(r, depth + 1))
    }

    /// A graphics state's soft mask draws its group form.
    fn graphics_state(&mut self, g: &Object, depth: usize) -> bool {
        if depth > MAX_DEPTH {
            return true;
        }
        let Some(g) = self.resolve(g) else { return false };
        let Some(mask) = g.as_dict().and_then(|g| g.get(b"SMask")) else { return false };
        let Some(mask) = self.resolve(mask) else { return false };
        mask.as_dict().and_then(|m| m.get(b"G")).is_some_and(|group| self.xobject(group, depth + 1))
    }
}
