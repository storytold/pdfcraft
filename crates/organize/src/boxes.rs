//! Page boxes (ISO 32000-2 §14.11.2): Acrobat's Set Page Boxes and Crop, execution plan M4.4.
//!
//! The crop, bleed, trim and art boxes default to their "parent" box (crop → media; the others
//! → crop) and are clipped to the media box. Margins are measured inward from the media box in
//! default user space (unrotated), as Acrobat's dialog does.

use pdfcraft_cos::{Document, Object};

use crate::{OrganizeError, check, walk};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageBox {
    Media,
    Crop,
    Bleed,
    Trim,
    Art,
}

impl PageBox {
    pub fn key(self) -> &'static [u8] {
        match self {
            PageBox::Media => b"MediaBox",
            PageBox::Crop => b"CropBox",
            PageBox::Bleed => b"BleedBox",
            PageBox::Trim => b"TrimBox",
            PageBox::Art => b"ArtBox",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().trim_end_matches("box") {
            "media" => Some(Self::Media),
            "crop" => Some(Self::Crop),
            "bleed" => Some(Self::Bleed),
            "trim" => Some(Self::Trim),
            "art" => Some(Self::Art),
            _ => None,
        }
    }
}

/// What to set a box to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BoxSpec {
    /// Back to its default (the parent box).
    Remove,
    /// An absolute rectangle in default user space.
    Rect([f64; 4]),
    /// Margins inward from the media box: left, bottom, right, top (points).
    Margins([f64; 4]),
}

fn rect_of(o: Option<&Object>, doc: &Document) -> Option<[f64; 4]> {
    let o = doc.resolve(o?);
    let v: Vec<f64> = o.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    (v.len() == 4 && v.iter().all(|x| x.is_finite())).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
}

fn intersect(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]
}

/// The effective boxes of every page: media, crop, bleed, trim, art.
pub fn page_boxes(doc: &Document) -> Result<Vec<[[f64; 4]; 5]>, OrganizeError> {
    Ok(walk(doc)?
        .iter()
        .map(|(page, inherited)| {
            let obj = doc.get(*page);
            let d = obj.as_dict().cloned().unwrap_or_default();
            let own = |k: &[u8]| rect_of(d.get(k).or_else(|| inherited.get(k)), doc);
            let media = own(b"MediaBox").unwrap_or([0.0, 0.0, 612.0, 792.0]);
            let crop = own(b"CropBox").map(|c| intersect(c, media)).unwrap_or(media);
            let child = |k: &[u8]| rect_of(d.get(k), doc).map(|c| intersect(c, media)).unwrap_or(crop);
            [media, crop, child(b"BleedBox"), child(b"TrimBox"), child(b"ArtBox")]
        })
        .collect())
}

/// Set a box on `pages` (0-based). Margins apply to each page's own media box.
pub fn set_page_box(doc: &mut Document, pages: &[usize], which: PageBox, spec: BoxSpec) -> Result<(), OrganizeError> {
    let all = walk(doc)?;
    check(pages, all.len())?;
    let boxes = page_boxes(doc)?;
    let bad = |m: &str| OrganizeError::InvalidBox(m.to_string());
    if which == PageBox::Media && spec == BoxSpec::Remove {
        return Err(bad("the media box can't be removed"));
    }
    // Validate every page first so a failure changes nothing.
    let mut plan = Vec::new();
    for &i in pages {
        let media = boxes[i][0];
        let rect = match spec {
            BoxSpec::Remove => None,
            BoxSpec::Rect(r) => {
                if !r.iter().all(|x| x.is_finite()) {
                    return Err(bad("the rectangle is not a number"));
                }
                let r = [r[0].min(r[2]), r[1].min(r[3]), r[0].max(r[2]), r[1].max(r[3])];
                Some(if which == PageBox::Media { r } else { intersect(r, media) })
            }
            BoxSpec::Margins(m) => {
                if !m.iter().all(|x| x.is_finite() && *x >= 0.0) {
                    return Err(bad("margins must be zero or more"));
                }
                Some([media[0] + m[0], media[1] + m[1], media[2] - m[2], media[3] - m[3]])
            }
        };
        if let Some(r) = rect
            && (r[2] - r[0] < 1.0 || r[3] - r[1] < 1.0)
        {
            return Err(bad(&format!("page {} would be smaller than a point", i + 1)));
        }
        plan.push((all[i].0, all[i].1.contains(which.key()), rect, media));
    }
    for (page, inherited, rect, media) in plan {
        doc.update_dict(page, |d| match rect {
            Some(r) => d.set(which.key().to_vec(), Object::Array(r.iter().map(|v| Object::Real(*v)).collect())),
            // An inherited crop box would come back: pin the default explicitly instead.
            None if inherited => d.set(which.key().to_vec(), Object::Array(media.iter().map(|v| Object::Real(*v)).collect())),
            None => {
                d.remove(which.key());
            }
        })?;
    }
    Ok(())
}

/// Most columns, or rows, [`split_pages`] cuts a page into.
pub const MAX_SPLIT: u32 = 10;

/// Most pages one [`split_pages`] may add, so a request can't grow a document without bound.
pub const MAX_SPLIT_PAGES: usize = 10_000;

/// Split Pages (#517): each of `pages` (0-based) becomes `columns × rows` pages, one per cell of
/// what it shows (its crop box), for scanned book spreads or sheets holding several pages. The
/// parts follow in reading order as the page is shown: rows from the top, each from the left,
/// with `/Rotate` taken into account. Each part is a copy of the page (as Duplicate Pages makes
/// it, with its own comments and widgets) whose crop box is its cell, so nothing is resampled and
/// the content outside the cell stays in the file. Nothing changes when any page would split into
/// parts smaller than a point, or when one can't be copied.
pub fn split_pages(doc: &mut Document, pages: &[usize], columns: u32, rows: u32) -> Result<(), OrganizeError> {
    if !(1..=MAX_SPLIT).contains(&columns) || !(1..=MAX_SPLIT).contains(&rows) {
        return Err(OrganizeError::Invalid(format!("columns and rows must be from 1 to {MAX_SPLIT}")));
    }
    // At most MAX_SPLIT² = 100.
    let cells = (columns * rows) as usize;
    if cells < 2 {
        return Err(OrganizeError::Invalid("splitting a page needs at least two parts".into()));
    }
    let all = walk(doc)?;
    check(pages, all.len())?;
    let mut order = pages.to_vec();
    order.sort_unstable();
    order.dedup();
    if order.len().saturating_mul(cells - 1) > MAX_SPLIT_PAGES {
        return Err(OrganizeError::Invalid(format!("splitting would add more than {MAX_SPLIT_PAGES} pages")));
    }
    let boxes = page_boxes(doc)?;
    // Plan every page first so a failure changes nothing.
    let mut plan = Vec::with_capacity(order.len());
    for &i in &order {
        let (Some(b), Some((page, inherited))) = (boxes.get(i), all.get(i)) else { return Err(OrganizeError::NoSuchPage(i)) };
        let rotate = doc.get(*page).as_dict().and_then(|d| d.get(b"Rotate").cloned()).or_else(|| inherited.get(b"Rotate").cloned());
        let rotate = rotate.and_then(|r| doc.resolve(&r).as_f64()).filter(|r| r.is_finite()).map_or(0, |r| (r / 90.0).round().rem_euclid(4.0) as u8);
        let parts: Vec<[f64; 4]> =
            (0..rows).flat_map(|r| (0..columns).map(move |c| (c, r))).map(|(c, r)| cell(b[1], rotate, (c, r), (columns, rows))).collect();
        if parts.iter().any(|p| p[2] - p[0] < 1.0 || p[3] - p[1] < 1.0) {
            return Err(OrganizeError::InvalidBox(format!("page {} is too small to split into {columns} × {rows}", i + 1)));
        }
        plan.push((i, *page, parts));
    }
    let src = doc.clone();
    // Split a copy and keep it only when every page is done, so a page that can't be copied
    // (objects nested too deep, say) leaves the document as it was (CodeRabbit on #5587).
    let mut out = src.clone();
    // The last page first, so the earlier ones keep their places in `src`'s order.
    for (i, page, parts) in plan.into_iter().rev() {
        let copies = crate::import_pages(&mut out, &src, &vec![i; cells - 1], i + 1)?;
        for (r, part) in std::iter::once(page).chain(copies).zip(parts) {
            out.update_dict(r, |d| d.set(b"CropBox".to_vec(), Object::Array(part.iter().map(|v| Object::Real(*v)).collect())))?;
        }
    }
    *doc = out;
    Ok(())
}

/// Cell (`column`, `row`) of `crop` cut into `columns × rows` as the page is shown, turned
/// `rotate` quarter turns clockwise (`/Rotate`), as a rectangle in default user space.
fn cell(crop: [f64; 4], rotate: u8, (column, row): (u32, u32), (columns, rows): (u32, u32)) -> [f64; 4] {
    let (w, h) = (crop[2] - crop[0], crop[3] - crop[1]);
    // A point of the shown page, `u` across from the left and `v` down from the top (0 to 1).
    let user = |u: f64, v: f64| match rotate {
        1 => [crop[0] + v * w, crop[1] + u * h],
        2 => [crop[2] - u * w, crop[1] + v * h],
        3 => [crop[2] - v * w, crop[3] - u * h],
        _ => [crop[0] + u * w, crop[3] - v * h],
    };
    let (cols, rows) = (f64::from(columns), f64::from(rows));
    let a = user(f64::from(column) / cols, f64::from(row) / rows);
    let b = user(f64::from(column + 1) / cols, f64::from(row + 1) / rows);
    [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]
}
