//! pdfcraft-model — typed views over the PDF object graph (L2). See the README.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

/// Attributes a page inherits from its ancestors (ISO 32000-2 §7.7.3.4).
pub const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// The largest `/UserUnit` honoured, as in Acrobat (a larger value counts as this one).
pub const MAX_USER_UNIT: f64 = 75_000.0;

/// A leaf page and its effective (inherited) attributes.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub obj: ObjRef,
    /// The page dictionary with inherited attributes filled in.
    pub dict: Dict,
}

fn rect(doc: &Document, o: Option<&Object>) -> Option<[f64; 4]> {
    let o = doc.resolve(o?);
    let v: Vec<f64> =
        o.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).filter(|x| !x.is_nan()).map(|x| x.clamp(f64::MIN, f64::MAX)).collect();
    (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

impl Page {
    /// The visible region (crop box clipped to the media box), in user space.
    pub fn crop(&self, doc: &Document) -> [f64; 4] {
        let media = rect(doc, self.dict.get(b"MediaBox")).unwrap_or([0.0, 0.0, 612.0, 792.0]);
        match rect(doc, self.dict.get(b"CropBox")) {
            Some(c) => {
                let r = [c[0].max(media[0]), c[1].max(media[1]), c[2].min(media[2]), c[3].min(media[3])];
                if r[2] > r[0] && r[3] > r[1] { r } else { media }
            }
            None => media,
        }
    }

    /// Clockwise rotation when displayed: 0, 90, 180 or 270. A `/Rotate` that isn't a multiple
    /// of 90 (ISO 32000-2 §7.7.3.3 requires one) counts as 0, as the renderer draws that page.
    pub fn rotation(&self, doc: &Document) -> i64 {
        let r = self.dict.get(b"Rotate").and_then(|o| doc.resolve(o).as_int()).unwrap_or(0).rem_euclid(360);
        if r % 90 == 0 { r } else { 0 }
    }

    /// The size of the page's user-space unit in points (`/UserUnit`; it isn't inherited), read
    /// as Acrobat reads it: a number from 1 up, at most [`MAX_USER_UNIT`]; anything else is 1.
    pub fn user_unit(&self, doc: &Document) -> f64 {
        self.dict.get(b"UserUnit").and_then(|o| doc.resolve(o).as_f64()).filter(|u| *u >= 1.0).map_or(1.0, |u| u.min(MAX_USER_UNIT))
    }

    /// The displayed size (width, height) in points, after `/UserUnit` and `/Rotate`.
    pub fn display_size(&self, doc: &Document) -> (f64, f64) {
        let c = self.crop(doc);
        let raw_w = (c[2] - c[0]).min(f64::MAX);
        let raw_h = (c[3] - c[1]).min(f64::MAX);
        let max_raw = raw_w.max(raw_h);
        let u = self.user_unit(doc);
        let u = if max_raw > 0.0 && (max_raw * u).is_infinite() { f64::MAX / max_raw } else { u };
        let (w, h) = (raw_w * u, raw_h * u);
        if self.rotation(doc) % 180 == 0 { (w, h) } else { (h, w) }
    }

    /// The matrix `[a b c d e f]` from display space (points, origin at the bottom-left of the
    /// page as shown, y up, after `/UserUnit` and `/Rotate`) to user space.
    pub fn view_matrix(&self, doc: &Document) -> [f64; 6] {
        let m = view_matrix_for(self.rotation(doc), self.crop(doc));
        let u = self.user_unit(doc);
        if u == 1.0 { m } else { [m[0] / u, m[1] / u, m[2] / u, m[3] / u, m[4], m[5]] }
    }
}

/// [`Page::view_matrix`] for a region `crop` (`[x0, y0, x1, y1]`) shown turned clockwise by
/// `rotation` (0, 90, 180 or 270): the matrix from the turned region's own display space
/// (origin at its bottom-left, y up) to the region's space. For a rectangle in user space it
/// puts an upright picture into that rectangle as the page displays it.
pub fn view_matrix_for(rotation: i64, crop: [f64; 4]) -> [f64; 6] {
    let [x0, y0, x1, y1] = crop;
    match rotation {
        90 => [0.0, 1.0, -1.0, 0.0, x1, y0],
        180 => [-1.0, 0.0, 0.0, -1.0, x1, y1],
        270 => [0.0, -1.0, 1.0, 0.0, x0, y1],
        _ => [1.0, 0.0, 0.0, 1.0, x0, y0],
    }
}

/// Leaf pages in document order, with inherited attributes resolved. Cycles and absurdly deep
/// trees are cut off rather than followed.
pub fn pages(doc: &Document) -> Vec<Page> {
    PageTree::new(doc).collect()
}

// Retain the owner of each inherited value rather than deep-cloning direct Resources on
// every edge. Only the final Page dictionaries need owned copies of those values.
type Inherited = [Option<std::sync::Arc<Object>>; 4];

struct Children {
    owner: std::sync::Arc<Object>,
    next: usize,
    inherited: Inherited,
}

impl Children {
    fn kids(&self) -> Option<&Vec<Object>> {
        match &*self.owner {
            Object::Dict(d) => d.get(b"Kids")?.as_array(),
            other => other.as_array(),
        }
    }
}

struct PageTree<'a> {
    doc: &'a Document,
    seen: std::collections::HashSet<ObjRef>,
    // One cursor per ancestor, never one entry per child. This also preserves the first
    // depth-first occurrence's attributes when malformed trees share a child.
    pending: Vec<Children>,
}

impl<'a> PageTree<'a> {
    fn new(doc: &'a Document) -> Self {
        let top = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|d| d.reference(b"Pages")));
        let pending = top
            .into_iter()
            .map(|r| Children { owner: std::sync::Arc::new(Object::Array(vec![Object::Ref(r)])), next: 0, inherited: Default::default() })
            .collect();
        Self { doc, seen: Default::default(), pending }
    }
}

impl Iterator for PageTree<'_> {
    type Item = Page;

    fn next(&mut self) -> Option<Page> {
        while let Some(parent) = self.pending.last_mut() {
            let Some(child) = parent.kids().and_then(|kids| kids.get(parent.next)) else {
                self.pending.pop();
                continue;
            };
            let node = child.as_ref();
            // A successful get means next is strictly below the allocated array length.
            parent.next += 1;
            let Some(node) = node else { continue };
            if self.seen.len() >= 1_000_000 {
                self.pending.clear();
                return None;
            }
            if !self.seen.insert(node) {
                continue;
            }
            let obj = self.doc.get(node);
            let Some(d) = obj.as_dict() else { continue };
            let mut inherited = parent.inherited.clone();
            for (key, owner) in INHERITABLE.iter().zip(&mut inherited) {
                if d.contains(key) {
                    *owner = Some(obj.clone());
                }
            }
            let is_pages = d.name(b"Type") == Some(b"Pages") || (d.contains(b"Kids") && d.name(b"Type") != Some(b"Page"));
            if is_pages {
                let owner = match d.get(b"Kids") {
                    Some(Object::Ref(r)) => self.doc.get(*r),
                    Some(Object::Array(_)) => obj.clone(),
                    _ => continue,
                };
                self.pending.push(Children { owner, next: 0, inherited });
            } else {
                let mut dict = d.clone();
                for (key, owner) in INHERITABLE.iter().zip(&inherited) {
                    if !dict.contains(key)
                        && let Some(value) = owner.as_ref().and_then(|o| o.as_dict()).and_then(|d| d.get(key))
                    {
                        dict.set(key.to_vec(), value.clone());
                    }
                }
                return Some(Page { obj: node, dict });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn doc(rotate: i64) -> Document {
        doc_with(rotate, "", "")
    }

    /// `pages` and `page` add entries to the page tree node and to the page.
    fn doc_with(rotate: i64, pages: &str, page: &str) -> Document {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            format!("<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 600 800] /Rotate {rotate} /Resources << /Font << >> >> {pages} >>"),
            format!("<< /Type /Page /Parent 2 0 R /CropBox [10 20 210 320] {page} >>"),
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offs = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offs.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let x = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offs {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
        Document::open(Arc::new(out)).unwrap()
    }

    #[test]
    fn repeated_children_do_not_queue_resource_copies() {
        let mut d = doc(0);
        let root = ObjRef { num: 2, generation: 0 };
        let leaf = ObjRef { num: 3, generation: 0 };
        d.update_dict(root, |dict| {
            // Modest fixture: the assertion measures retained work, not an OOM or timing.
            dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(leaf); 128]));
            let mut resources = Dict::new();
            resources.set(b"Payload".to_vec(), Object::Array(vec![Object::Int(42); 1024]));
            dict.set(b"Resources".to_vec(), Object::Dict(resources));
        })
        .unwrap();
        let mut walk = PageTree::new(&d);
        let p = walk.next().unwrap();
        assert_eq!(p.obj, leaf);
        assert_eq!(p.dict.get(b"Resources").unwrap().as_dict().unwrap().get(b"Payload").unwrap().as_array().unwrap().len(), 1024);
        assert!(walk.pending.len() <= 2, "duplicate edges retained {} pending entries", walk.pending.len());
        assert!(walk.next().is_none());
    }

    #[test]
    fn shared_children_keep_depth_first_order_and_nearest_attributes() {
        let mut d = doc(0);
        let root = ObjRef { num: 2, generation: 0 };
        let leaf = ObjRef { num: 3, generation: 0 };
        let mut second = d.get(leaf).as_dict().unwrap().clone();
        second.set(b"Rotate".to_vec(), Object::Int(180));
        let second = d.add(second);
        let mut branch = Dict::new();
        branch.set(b"Type".to_vec(), Object::name("Pages"));
        branch.set(b"Rotate".to_vec(), Object::Int(90));
        // A cycle, a shared child, an invalid entry, and an indirect Kids array.
        let kids = d.add(Object::Array(vec![Object::Ref(root), Object::Null, Object::Ref(leaf), Object::Ref(second)]));
        branch.set(b"Kids".to_vec(), Object::Ref(kids));
        let branch = d.add(branch);
        d.update_dict(root, |dict| {
            dict.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(branch), Object::Ref(leaf), Object::Ref(second)]));
        })
        .unwrap();
        let pages = pages(&d);
        assert_eq!(pages.iter().map(|p| p.obj).collect::<Vec<_>>(), vec![leaf, second]);
        assert_eq!(pages[0].rotation(&d), 90, "first depth-first occurrence supplies inheritance");
        assert_eq!(pages[1].rotation(&d), 180, "a leaf's own attribute wins");
        assert!(pages.iter().all(|p| p.dict.contains(b"Resources")));
    }

    #[test]
    fn wide_page_tree_retains_only_ancestor_cursors() {
        let mut d = doc(0);
        let root = ObjRef { num: 2, generation: 0 };
        let leaf = d.get(ObjRef { num: 3, generation: 0 }).as_dict().unwrap().clone();
        let kids: Vec<Object> = (0..64).map(|_| Object::Ref(d.add(leaf.clone()))).collect();
        d.update_dict(root, |dict| dict.set(b"Kids".to_vec(), Object::Array(kids.clone()))).unwrap();
        let mut walk = PageTree::new(&d);
        for kid in kids {
            assert_eq!(walk.next().unwrap().obj, kid.as_ref().unwrap());
            assert!(walk.pending.len() <= 2);
        }
        assert!(walk.next().is_none());
    }

    fn apply(m: [f64; 6], x: f64, y: f64) -> [f64; 2] {
        [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]
    }

    #[test]
    fn inherited_attributes_and_display_geometry() {
        for rotate in [0, 90, 180, 270, 450] {
            let d = doc(rotate);
            let p = &pages(&d)[0];
            assert!(p.dict.contains(b"Resources") && p.dict.contains(b"MediaBox"));
            assert_eq!(p.crop(&d), [10.0, 20.0, 210.0, 320.0]);
            let (w, h) = p.display_size(&d);
            let m = p.view_matrix(&d);
            // The displayed page's corners map onto the crop box's corners.
            let corners: Vec<[f64; 2]> = [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)].iter().map(|(x, y)| apply(m, *x, *y)).collect();
            for c in &corners {
                assert!([10.0, 210.0].contains(&c[0]) && [20.0, 320.0].contains(&c[1]), "{rotate}: {c:?}");
            }
            // The displayed top-left corner is where a reader sees it.
            let tl = apply(m, 0.0, h);
            let expect = match rotate % 360 {
                0 => [10.0, 320.0],
                90 => [10.0, 20.0],
                180 => [210.0, 20.0],
                _ => [210.0, 320.0],
            };
            assert_eq!(tl, expect, "rotation {rotate}");
        }
    }

    /// A `/Rotate` that isn't a quarter turn counts as 0, as the renderer draws the page
    /// upright, so display space matches what's on screen.
    #[test]
    fn rotate_off_a_quarter_turn_counts_as_upright() {
        for rotate in [135, 30, -45, 359] {
            let d = doc(rotate);
            let p = &pages(&d)[0];
            assert_eq!(p.rotation(&d), 0, "{rotate}");
            assert_eq!(p.display_size(&d), (200.0, 300.0), "{rotate}");
            assert_eq!(p.view_matrix(&d), [1.0, 0.0, 0.0, 1.0, 10.0, 20.0], "{rotate}");
        }
        let d = doc(-90);
        assert_eq!(pages(&d)[0].rotation(&d), 270);
    }

    #[test]
    fn overflowing_page_box_is_preserved_or_save_is_refused() {
        use pdfcraft_cos::{SaveOptions, write_full, write_incremental};
        for bounds in ["0 0 1e400 1e400", "-1e400 -1e400 0 0"] {
            let mut d = doc_with(0, "", &format!("/MediaBox [{bounds}] /CropBox [{bounds}]"));
            let source = d.bytes().clone();
            let before = pages(&d)[0].display_size(&d);
            assert_eq!(before, (f64::MAX, f64::MAX));
            // Unmodified page objects stay byte-for-byte in the original revision.
            let bytes = write_incremental(&d, &Default::default()).unwrap();
            assert!(bytes.starts_with(&source));
            let reopened = Document::open(Arc::new(bytes)).unwrap();
            assert_eq!(pages(&reopened)[0].display_size(&reopened), before);
            for object_streams in [false, true] {
                assert!(write_full(&d, &SaveOptions { object_streams, ..Default::default() }).is_err());
            }
            let page = pages(&d)[0].obj;
            d.update_dict(page, |dict| dict.set(b"Rotate".to_vec(), Object::Int(90))).unwrap();
            assert!(write_incremental(&d, &Default::default()).is_err());
            assert_eq!(d.bytes(), &source, "failed save must leave source bytes alone");
            assert_eq!(pages(&d)[0].rotation(&d), 90, "failed save must keep edits");
            assert_eq!(pages(&d)[0].display_size(&d), before);
        }
        // Representable huge boxes still round-trip through both full-save formats.
        let d = doc_with(0, "", "/MediaBox [0 0 1e30 2e30] /CropBox [0 0 1e30 2e30] /UserUnit 75000");
        for object_streams in [false, true] {
            let bytes = write_full(&d, &SaveOptions { object_streams, ..Default::default() }).unwrap();
            let reopened = Document::open(Arc::new(bytes)).unwrap();
            assert_eq!(pages(&reopened)[0].display_size(&reopened), pages(&d)[0].display_size(&d));
        }
    }

    /// Display space is in points: `/UserUnit 2` doubles it. As in Acrobat, the value isn't
    /// inherited, values below 1 or not numbers count as 1, and values over 75,000 as 75,000.
    #[test]
    fn user_unit_scales_display_space() {
        for rotate in [0, 90, 180, 270] {
            let d = doc_with(rotate, "", "/UserUnit 2");
            let p = &pages(&d)[0];
            let (w, h) = p.display_size(&d);
            assert_eq!((w, h), if rotate % 180 == 0 { (400.0, 600.0) } else { (600.0, 400.0) }, "{rotate}");
            let plain = doc(rotate);
            let unscaled = pages(&plain)[0].view_matrix(&plain);
            for (x, y) in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h), (37.0, 81.0)] {
                assert_eq!(apply(p.view_matrix(&d), x, y), apply(unscaled, x / 2.0, y / 2.0), "{rotate}: ({x}, {y})");
            }
        }
        for (pages_attrs, page_attrs, unit) in [
            ("/UserUnit 2", "", 1.0),
            ("", "/UserUnit 1.5", 1.5),
            ("", "/UserUnit 0.5", 1.0),
            ("", "/UserUnit 0", 1.0),
            ("", "/UserUnit -2", 1.0),
            ("", "/UserUnit /Two", 1.0),
            ("", "/UserUnit 100000", MAX_USER_UNIT),
            ("", "/UserUnit 1e20", MAX_USER_UNIT),
            ("", "/UserUnit 1e400", MAX_USER_UNIT),
        ] {
            let d = doc_with(0, pages_attrs, page_attrs);
            assert_eq!(pages(&d)[0].user_unit(&d), unit, "{pages_attrs} {page_attrs}");
        }
        for page_attrs in
            ["/MediaBox [0 0 1e30 1e30] /CropBox [0 0 1e30 1e30]", "/MediaBox [0 0 1e400 1e400] /CropBox [0 0 1e400 1e400] /UserUnit 1e20"]
        {
            let d = doc_with(0, "", page_attrs);
            let p = &pages(&d)[0];
            let (w, h) = p.display_size(&d);
            assert!(w.is_finite() && w >= 1e30 && h.is_finite() && h >= 1e30, "{page_attrs}: ({w}, {h})");
        }
        let d = doc_with(0, "", "/MediaBox [0 0 1e305 2e305] /CropBox [0 0 1e305 2e305] /UserUnit 75000");
        let (w, h) = pages(&d)[0].display_size(&d);
        assert!(w.is_finite() && h.is_finite() && (h / w - 2.0).abs() < 1e-12, "({w}, {h})");
    }
}
