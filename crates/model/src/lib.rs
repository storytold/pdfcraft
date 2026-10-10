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
    let v: Vec<f64> = o.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    (v.len() == 4 && v.iter().all(|x| x.is_finite())).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
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

    /// Clockwise rotation when displayed: 0, 90, 180 or 270.
    pub fn rotation(&self, doc: &Document) -> i64 {
        self.dict.get(b"Rotate").and_then(|o| doc.resolve(o).as_int()).unwrap_or(0).rem_euclid(360) / 90 * 90
    }

    /// The size of the page's user-space unit in points (`/UserUnit`; it isn't inherited), read
    /// as Acrobat reads it: a number from 1 up, at most [`MAX_USER_UNIT`]; anything else is 1.
    pub fn user_unit(&self, doc: &Document) -> f64 {
        self.dict.get(b"UserUnit").and_then(|o| doc.resolve(o).as_f64()).filter(|u| *u >= 1.0).map_or(1.0, |u| u.min(MAX_USER_UNIT))
    }

    /// The displayed size (width, height) in points, after `/UserUnit` and `/Rotate`.
    pub fn display_size(&self, doc: &Document) -> (f64, f64) {
        let c = self.crop(doc);
        let u = self.user_unit(doc);
        let (w, h) = ((c[2] - c[0]) * u, (c[3] - c[1]) * u);
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
    let Some(root) = doc.root() else { return Vec::new() };
    let Some(top) = doc.get(root).as_dict().and_then(|d| d.reference(b"Pages")) else { return Vec::new() };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut stack: Vec<(ObjRef, Dict)> = vec![(top, Dict::new())];
    while let Some((node, inherited)) = stack.pop() {
        if !seen.insert(node) || seen.len() > 1_000_000 {
            continue;
        }
        let obj = doc.get(node);
        let Some(d) = obj.as_dict() else { continue };
        let mut attrs = inherited.clone();
        for k in INHERITABLE {
            if let Some(v) = d.get(k) {
                attrs.set(k.to_vec(), v.clone());
            }
        }
        let is_pages = d.name(b"Type") == Some(b"Pages") || (d.contains(b"Kids") && d.name(b"Type") != Some(b"Page"));
        if is_pages {
            let kids = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()).unwrap_or_default();
            for k in kids.iter().rev() {
                if let Some(r) = k.as_ref() {
                    stack.push((r, attrs.clone()));
                }
            }
        } else {
            let mut dict = d.clone();
            for k in INHERITABLE {
                if !dict.contains(k)
                    && let Some(v) = attrs.get(k)
                {
                    dict.set(k.to_vec(), v.clone());
                }
            }
            out.push(Page { obj: node, dict });
        }
    }
    out
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
        ] {
            let d = doc_with(0, pages_attrs, page_attrs);
            assert_eq!(pages(&d)[0].user_unit(&d), unit, "{pages_attrs} {page_attrs}");
        }
    }
}
