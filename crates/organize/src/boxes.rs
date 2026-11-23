//! Page boxes (ISO 32000-2 §14.11.2): Acrobat's Set Page Boxes and Crop, execution plan M4.4.
//!
//! The crop, bleed, trim and art boxes default to their "parent" box (crop → media; the others
//! → crop) and are clipped to the media box. Margins are measured inward from the media box in
//! default user space (unrotated), as Acrobat's dialog does.

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

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
        let lower = s.trim().to_ascii_lowercase();
        match lower.strip_suffix("box").unwrap_or(&lower) {
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
    /// Margins inward from the media box: left, bottom, right, top (default user-space units of the
    /// unrotated page; points only on a page without `/UserUnit`).
    Margins([f64; 4]),
}

/// Parses a 4-number PDF rectangle `[x0, y0, x1, y1]` into `[xmin, ymin, xmax, ymax]` when it has
/// finite coordinates and positive width and height.
fn rect_of(o: Option<&Object>, doc: &Document) -> Option<[f64; 4]> {
    let o = doc.resolve(o?);
    let v: Vec<f64> = o.as_array()?.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
    if v.len() != 4 || !v.iter().all(|x| x.is_finite()) {
        return None;
    }
    let r = [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])];
    let (w, h) = (r[2] - r[0], r[3] - r[1]);
    (w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()).then_some(r)
}

/// Returns the coordinate-wise intersection of two normalized `[xmin, ymin, xmax, ymax]` rectangles.
fn intersect(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]
}

/// Clips rectangle `a` to `b`, returning `None` when their intersection has no positive area.
fn clip(a: [f64; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let r = intersect(a, b);
    (r[2] > r[0] && r[3] > r[1]).then_some(r)
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
            let crop = own(b"CropBox").and_then(|c| clip(c, media)).unwrap_or(media);
            let child = |k: &[u8]| rect_of(d.get(k), doc).and_then(|c| clip(c, media)).unwrap_or(crop);
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
        if let Some(r) = rect {
            let (w, h) = (r[2] - r[0], r[3] - r[1]);
            if !w.is_finite() || !h.is_finite() {
                return Err(bad("the rectangle is not a number"));
            }
            if w < 1.0 || h < 1.0 {
                return Err(bad(&format!("page {} would be smaller than a point", i + 1)));
            }
        }
        plan.push((i, all[i].0, rect, media));
    }
    for &(_, page, rect, _) in &plan {
        doc.update_dict(page, |d| match rect {
            Some(r) => d.set(which.key().to_vec(), real_array(&r)),
            None => {
                d.remove(which.key());
            }
        })?;
    }
    if spec == BoxSpec::Remove && which == PageBox::Crop {
        pin_inherited_crop(doc, plan.iter().map(|&(i, page, _, media)| (i, page, media)))?;
    }
    Ok(())
}

/// An inherited crop box from an ancestor `/Pages` node would come back after a page's own is
/// removed: pin each page's media box as its crop box only when an ancestor still supplies a
/// valid CropBox. `pages` are (0-based index, page, media box).
fn pin_inherited_crop(doc: &mut Document, pages: impl IntoIterator<Item = (usize, ObjRef, [f64; 4])>) -> Result<(), OrganizeError> {
    let after = walk(doc)?;
    for (i, page, media) in pages {
        if after.get(i).and_then(|(_, inh)| rect_of(inh.get(b"CropBox"), doc)).is_some() {
            doc.update_dict(page, |d| d.set(b"CropBox".to_vec(), real_array(&media)))?;
        }
    }
    Ok(())
}

fn real_array(r: &[f64; 4]) -> Object {
    Object::Array(r.iter().map(|v| Object::Real(*v)).collect())
}

/// The largest side of a page, in default user-space units (ISO 32000-2 Annex C.2).
pub const MAX_PAGE_SIDE: f64 = 14_400.0;

/// Where Change Page Size puts a page's visible area (its crop box) on the resized page, measured
/// on the page as shown, after `/Rotate`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// The share of the spare width to the left of the visible area: 0 left edge, 0.5 centred,
    /// 1 right edge.
    pub across: f64,
    /// The share of the spare height above it: 0 top edge, 0.5 centred, 1 bottom edge.
    pub down: f64,
    /// Then moved this many points right and down (negative: left and up).
    pub offset: [f64; 2],
}

impl Default for Placement {
    /// Centred, not moved.
    fn default() -> Self {
        Self { across: 0.5, down: 0.5, offset: [0.0; 2] }
    }
}

/// The media box (user space) that gives a page of `size` points (width, height as shown) with its
/// visible area `crop` (user space) where `placement` puts it, nothing scaled. `rotation` is the
/// page's clockwise `/Rotate` (0, 90, 180 or 270) and `user_unit` its points per user-space unit.
fn resized_media_box(crop: [f64; 4], rotation: i64, user_unit: f64, size: [f64; 2], placement: Placement) -> [f64; 4] {
    let u = if user_unit.is_finite() && user_unit > 0.0 { user_unit } else { 1.0 };
    let turned = rotation.rem_euclid(180) == 90;
    // Shown sizes in user-space units: the new page, and the visible area.
    let (w, h) = (size[0] / u, size[1] / u);
    let (cw, ch) = if turned { (crop[3] - crop[1], crop[2] - crop[0]) } else { (crop[2] - crop[0], crop[3] - crop[1]) };
    // The visible area's top-left corner on the shown page, from the new page's top-left.
    let x = placement.across * (w - cw) + placement.offset[0] / u;
    let y = placement.down * (h - ch) + placement.offset[1] / u;
    // Unturned, the shown page is the media box: its user-space width and height.
    let (mw, mh) = if turned { (h, w) } else { (w, h) };
    // Where the shown top-left corner of the new page lies relative to the crop box, per turn:
    // 90° shows the left edge on top, 180° the bottom edge, 270° the right edge.
    let (x0, y0) = match rotation.rem_euclid(360) {
        90 => (crop[0] - y, crop[1] - x),
        180 => (crop[2] + x - mw, crop[1] - y),
        270 => (crop[2] + y - mw, crop[3] + x - mh),
        _ => (crop[0] - x, crop[3] + y - mh),
    };
    [x0, y0, x0 + mw, y0 + mh]
}

/// Whether a page whose visible area was `visible` loses part of it on a media box `media` (both
/// user space, of a page with `user_unit` points per unit). Less than a hundredth of a point
/// doesn't count: a page given the size it already has, measured in another unit or rounded,
/// loses nothing anyone can see.
pub fn cuts_off(visible: [f64; 4], media: [f64; 4], user_unit: f64) -> bool {
    let slack = 0.01 / if user_unit.is_finite() && user_unit > 0.0 { user_unit } else { 1.0 };
    visible[0] < media[0] - slack || visible[1] < media[1] - slack || visible[2] > media[2] + slack || visible[3] > media[3] + slack
}

/// Change Page Size: give `pages` (0-based) a new size of `size` points (width, height as the page
/// is shown) without scaling anything on them. The media box moves, placed around each page's
/// visible area as `placement` says; the page's crop, bleed, trim and art boxes go back to their
/// defaults, the whole new page; where it reaches past the visible area, the content is clipped to
/// that area, so what the old boxes hid stays hidden. Returns the pages whose visible area no longer fits on them
/// (part of what showed is now cut off). Every page is checked before any is changed.
pub fn set_page_size(doc: &mut Document, pages: &[usize], size: [f64; 2], placement: Placement) -> Result<Vec<usize>, OrganizeError> {
    let all = walk(doc)?;
    check(pages, all.len())?;
    let bad = |m: &str| OrganizeError::InvalidBox(m.to_string());
    if pages.is_empty() {
        return Err(bad("no pages to resize"));
    }
    let [across, down] = [placement.across, placement.down];
    if !(0.0..=1.0).contains(&across) || !(0.0..=1.0).contains(&down) {
        return Err(bad("the placement must be between 0 and 1 across and down"));
    }
    let boxes = page_boxes(doc)?;
    let mut plan = Vec::with_capacity(pages.len());
    let mut cut = Vec::new();
    for &i in pages {
        let (Some((page, attrs)), Some(b)) = (all.get(i), boxes.get(i)) else { return Err(OrganizeError::NoSuchPage(i)) };
        let mut dict = attrs.clone();
        if let Some(u) = doc.get(*page).as_dict().and_then(|d| d.get(b"UserUnit")) {
            dict.set(b"UserUnit".to_vec(), u.clone());
        }
        let shown = pdfcraft_model::Page { obj: *page, dict };
        let unit = shown.user_unit(doc);
        let side = |v: f64| v.is_finite() && (1.0..=MAX_PAGE_SIDE).contains(&(v / unit));
        if !side(size[0]) || !side(size[1]) {
            return Err(bad(&format!("page {} can be {} to {} points a side, not {} × {}", i + 1, unit, MAX_PAGE_SIDE * unit, size[0], size[1])));
        }
        // Further than a page's largest side moves the old page off any new one.
        if !placement.offset.iter().all(|v| v.is_finite() && v.abs() / unit <= MAX_PAGE_SIDE) {
            return Err(bad(&format!("page {} can move at most {} points each way", i + 1, MAX_PAGE_SIDE * unit)));
        }
        let crop = b[1];
        let media = resized_media_box(crop, shown.rotation(doc), unit, size, placement);
        if !media.iter().all(|v| v.is_finite()) {
            return Err(bad("the page's boxes are out of range"));
        }
        if cuts_off(crop, media, unit) {
            cut.push(i);
        }
        plan.push((i, *page, media, crop, unit));
    }
    for &(_, page, media, crop, unit) in &plan {
        // Where the new page reaches past what showed, whatever the old boxes hid (bleed, marks,
        // anything cropped away) would show: the content is drawn clipped to what showed.
        if cuts_off(media, crop, unit) {
            clip_content(doc, page, crop)?;
        }
        doc.update_dict(page, |d| {
            d.set(b"MediaBox".to_vec(), real_array(&media));
            for b in [PageBox::Crop, PageBox::Bleed, PageBox::Trim, PageBox::Art] {
                d.remove(b.key());
            }
        })?;
    }
    pin_inherited_crop(doc, plan.into_iter().map(|(i, page, media, ..)| (i, page, media)))?;
    Ok(cut)
}

/// Draws the page's content inside a clip to `area` (user space): a stream opening the clip goes
/// before the page's own and one closing it after, which are kept as they are.
fn clip_content(doc: &mut Document, page: ObjRef, area: [f64; 4]) -> Result<(), OrganizeError> {
    let Some(contents) = doc.get(page).as_dict().and_then(|d| d.get(b"Contents").cloned()) else { return Ok(()) };
    let mut parts = match doc.resolve(&contents).as_array() {
        Some(list) => list.clone(),
        None => vec![contents],
    };
    let [x0, y0, x1, y1] = area;
    let open = format!("q {x0} {y0} {} {} re W n\n", x1 - x0, y1 - y0);
    parts.insert(0, Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), open.as_bytes())))));
    parts.push(Object::Ref(doc.add(Object::Stream(Stream::flate(Dict::new(), b"\nQ\n")))));
    doc.update_dict(page, |d| d.set(b"Contents".to_vec(), Object::Array(parts)))?;
    Ok(())
}
