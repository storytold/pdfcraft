//! Split pages into multiple sub-pages (e.g. 50% vertical split for 2-up spreads, horizontal, grid).
//!
//! Each sub-page inherits the original page's contents and resources, with its MediaBox and CropBox
//! set to the corresponding slice of the original page's visible area. Content streams are shared,
//! so document file size is preserved.

use pdfcraft_cos::{Dict, Document, ObjRef, Object};

use crate::{OrganizeError, check, page_boxes, rebuild, resolved_rotation, walk};

/// How a physical page is sliced into sub-pages.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PageSplit {
    /// Vertical cut into N equal columns (default 2 for 50% left & right).
    Vertical { parts: usize },
    /// Horizontal cut into N equal rows (default 2 for 50% top & bottom).
    Horizontal { parts: usize },
    /// Grid cut into rows x columns (e.g. 2x2 = 4 parts).
    Grid { rows: usize, columns: usize },
    /// Custom vertical ratio: cut line at `ratio` (0.0 < ratio < 1.0) of visible width from visual left.
    VerticalRatio(f64),
    /// Custom horizontal ratio: cut line at `ratio` (0.0 < ratio < 1.0) of visible height from visual top.
    HorizontalRatio(f64),
}

/// Maps a visual rectangle `[vx0, vy0, vx1, vy1]` (where 0.0..=1.0 represents the fraction
/// of the displayed page from visual top-left, y down) into unrotated user-space bounds `[xmin, ymin, xmax, ymax]`.
fn visual_fraction_to_user_rect(crop: [f64; 4], rot: u16, vx0: f64, vy0: f64, vx1: f64, vy1: f64) -> [f64; 4] {
    let [cx0, cy0, cx1, cy1] = crop;
    let cw = (cx1 - cx0).max(1e-6);
    let ch = (cy1 - cy0).max(1e-6);

    let corners = [(vx0, vy0), (vx1, vy0), (vx0, vy1), (vx1, vy1)];
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for (vx, vy) in corners {
        // Map visual fraction (vx, vy) to unrotated user fractions (u, v)
        // u: fraction across from left (0..1)
        // v: fraction down from top (0..1)
        let (u, v) = match rot % 360 {
            90 => (vy, 1.0 - vx),
            180 => (1.0 - vx, 1.0 - vy),
            270 => (1.0 - vy, vx),
            _ => (vx, vy),
        };
        let x = cx0 + u * cw;
        let y = cy1 - v * ch;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }

    [min_x, min_y, max_x, max_y]
}

/// Calculates the slice rectangles in user space coordinates.
pub fn calculate_slices(crop: [f64; 4], rot: u16, split: PageSplit) -> Result<Vec<[f64; 4]>, OrganizeError> {
    let mut visual_rects = Vec::new();
    match split {
        PageSplit::Vertical { parts } => {
            if !(2..=100).contains(&parts) {
                return Err(OrganizeError::Invalid("vertical split requires between 2 and 100 parts".into()));
            }
            let n = parts as f64;
            for i in 0..parts {
                let x0 = (i as f64) / n;
                let x1 = ((i + 1) as f64) / n;
                visual_rects.push((x0, 0.0, x1, 1.0));
            }
        }
        PageSplit::Horizontal { parts } => {
            if !(2..=100).contains(&parts) {
                return Err(OrganizeError::Invalid("horizontal split requires between 2 and 100 parts".into()));
            }
            let n = parts as f64;
            for i in 0..parts {
                let y0 = (i as f64) / n;
                let y1 = ((i + 1) as f64) / n;
                visual_rects.push((0.0, y0, 1.0, y1));
            }
        }
        PageSplit::Grid { rows, columns } => {
            let total = rows.checked_mul(columns).ok_or_else(|| {
                OrganizeError::Invalid("grid dimensions overflow".into())
            })?;
            if rows < 1 || columns < 1 || rows > 50 || columns > 50 || total < 2 {
                return Err(OrganizeError::Invalid(
                    "grid split requires between 1 and 50 rows/columns and at least 2 total cells".into(),
                ));
            }
            let r_f = rows as f64;
            let c_f = columns as f64;
            for r in 0..rows {
                let y0 = (r as f64) / r_f;
                let y1 = ((r + 1) as f64) / r_f;
                for c in 0..columns {
                    let x0 = (c as f64) / c_f;
                    let x1 = ((c + 1) as f64) / c_f;
                    visual_rects.push((x0, y0, x1, y1));
                }
            }
        }
        PageSplit::VerticalRatio(ratio) => {
            if !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0 {
                return Err(OrganizeError::Invalid("vertical split ratio must be between 0.0 and 1.0".into()));
            }
            visual_rects.push((0.0, 0.0, ratio, 1.0));
            visual_rects.push((ratio, 0.0, 1.0, 1.0));
        }
        PageSplit::HorizontalRatio(ratio) => {
            if !ratio.is_finite() || ratio <= 0.0 || ratio >= 1.0 {
                return Err(OrganizeError::Invalid("horizontal split ratio must be between 0.0 and 1.0".into()));
            }
            visual_rects.push((0.0, 0.0, 1.0, ratio));
            visual_rects.push((0.0, ratio, 1.0, 1.0));
        }
    }

    let mut slices = Vec::with_capacity(visual_rects.len());
    for (vx0, vy0, vx1, vy1) in visual_rects {
        let r = visual_fraction_to_user_rect(crop, rot, vx0, vy0, vx1, vy1);
        let w = r[2] - r[0];
        let h = r[3] - r[1];
        if !w.is_finite() || !h.is_finite() || w < 1.0 || h < 1.0 {
            return Err(OrganizeError::Invalid("a split part would be smaller than 1 point".into()));
        }
        slices.push(r);
    }

    Ok(slices)
}

/// Slices the specified pages into multiple sub-pages in place.
///
/// Returns the 0-based indices of all newly produced pages in the resulting document.
pub fn split_pages(doc: &mut Document, pages: &[usize], split: PageSplit) -> Result<Vec<usize>, OrganizeError> {
    let all = walk(doc)?;
    check(pages, all.len())?;
    if pages.is_empty() {
        return Ok(Vec::new());
    }

    let mut targets = pages.to_vec();
    targets.sort_unstable();
    targets.dedup();

    let boxes = page_boxes(doc)?;

    // Calculate plans first so any validation error returns before modifying doc.
    let mut page_plans = Vec::with_capacity(targets.len());
    for &page_idx in &targets {
        let (page_ref, inherited) = &all[page_idx];
        let current_page = doc.get(*page_ref);
        let current_attrs = current_page.as_dict().filter(|attrs| attrs.get(b"Rotate").is_some()).unwrap_or(inherited);
        let rot = (resolved_rotation(doc, current_attrs).rem_euclid(360)) as u16;
        let crop = boxes[page_idx][1];
        let slices = calculate_slices(crop, rot, split)?;
        if slices.len() < 2 {
            return Err(OrganizeError::Invalid("splitting must produce at least 2 parts".into()));
        }
        page_plans.push((page_idx, *page_ref, inherited.clone(), slices, rot));
    }

    let mut new_page_refs: Vec<(ObjRef, Dict)> = Vec::with_capacity(all.len() + targets.len() * 2);
    let mut affected_indices = Vec::new();

    for (i, (orig_ref, inherited)) in all.iter().enumerate() {
        if let Some((_, _, inh, slices, _)) = page_plans.iter().find(|(idx, ..)| *idx == i) {
            // Read annotations from original page to partition them across sub-pages
            let orig_annots: Vec<ObjRef> = if let Some(annots_obj) = doc.get(*orig_ref).as_dict().and_then(|d| d.get(b"Annots")) {
                let resolved = doc.resolve(annots_obj);
                if let Some(arr) = resolved.as_array() {
                    arr.iter()
                        .filter_map(|o| match o {
                            Object::Ref(r) => Some(*r),
                            _ => None,
                        })
                        .collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };

            let mut slice_annots: Vec<Vec<ObjRef>> = vec![Vec::new(); slices.len()];
            for annot_ref in orig_annots {
                let annot_obj = doc.get(annot_ref);
                if let Some(rect_obj) = annot_obj.as_dict().and_then(|d| d.get(b"Rect")) {
                    let resolved_rect = doc.resolve(rect_obj);
                    if let Some(rect_arr) = resolved_rect.as_array() {
                        let coords: Vec<f64> = rect_arr.iter().filter_map(|x| doc.resolve(x).as_f64()).collect();
                        if coords.len() == 4 {
                            let mid_x = (coords[0] + coords[2]) / 2.0;
                            let mid_y = (coords[1] + coords[3]) / 2.0;
                            let mut assigned = false;
                            for (s_idx, s_box) in slices.iter().enumerate() {
                                if mid_x >= s_box[0] && mid_x <= s_box[2] && mid_y >= s_box[1] && mid_y <= s_box[3] {
                                    slice_annots[s_idx].push(annot_ref);
                                    assigned = true;
                                    break;
                                }
                            }
                            if !assigned {
                                slice_annots[0].push(annot_ref);
                            }
                        }
                    }
                }
            }

            // Sub-page 0 reuses orig_ref
            let first_slice = slices[0];
            let first_annots = &slice_annots[0];
            doc.update_dict(*orig_ref, |d| {
                d.set(b"MediaBox".to_vec(), Object::Array(first_slice.iter().map(|v| Object::Real(*v)).collect()));
                d.set(b"CropBox".to_vec(), Object::Array(first_slice.iter().map(|v| Object::Real(*v)).collect()));
                d.remove(b"BleedBox");
                d.remove(b"TrimBox");
                d.remove(b"ArtBox");
                if first_annots.is_empty() {
                    d.remove(b"Annots");
                } else {
                    d.set(b"Annots".to_vec(), Object::Array(first_annots.iter().map(|r| Object::Ref(*r)).collect()));
                }
            })?;

            affected_indices.push(new_page_refs.len());
            new_page_refs.push((*orig_ref, inh.clone()));

            // Additional slices create new page dictionaries sharing contents and resources
            let orig_dict = doc.get(*orig_ref).as_dict().cloned().unwrap_or_default();
            for (sub_idx, slice) in slices.iter().enumerate().skip(1) {
                let mut new_dict = orig_dict.clone();
                new_dict.set(b"MediaBox".to_vec(), Object::Array(slice.iter().map(|v| Object::Real(*v)).collect()));
                new_dict.set(b"CropBox".to_vec(), Object::Array(slice.iter().map(|v| Object::Real(*v)).collect()));
                new_dict.remove(b"BleedBox");
                new_dict.remove(b"TrimBox");
                new_dict.remove(b"ArtBox");

                let sub_annots = &slice_annots[sub_idx];
                if sub_annots.is_empty() {
                    new_dict.remove(b"Annots");
                } else {
                    new_dict.set(b"Annots".to_vec(), Object::Array(sub_annots.iter().map(|r| Object::Ref(*r)).collect()));
                }

                let new_ref = doc.add(new_dict);
                for &annot_ref in sub_annots {
                    doc.update_dict(annot_ref, |ad| {
                        ad.set(b"P".to_vec(), Object::Ref(new_ref));
                    })?;
                }

                affected_indices.push(new_page_refs.len());
                new_page_refs.push((new_ref, inh.clone()));
            }
        } else {
            new_page_refs.push((*orig_ref, inherited.clone()));
        }
    }

    rebuild(doc, &new_page_refs)?;
    Ok(affected_indices)
}
