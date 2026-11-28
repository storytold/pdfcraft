//! Page boxes that reach past their page's media box (ISO 32000-2 §14.11.2.1). They are reported,
//! never repaired: a reader intersects each one with the media box, and a changed media box leaves
//! the other boxes exactly as stored.

use pdfcraft_cos::{Document, Object};

use crate::boxes::{PageBox, page_boxes, rect_of};
use crate::{OrganizeError, check, walk};

/// A crop, bleed, trim or art box its page's media box does not contain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxOutside {
    /// 0-based page index.
    pub page: usize,
    /// Never [`PageBox::Media`].
    pub which: PageBox,
    /// The box as stored, normalized to `[xmin, ymin, xmax, ymax]` in default user space.
    pub rect: [f64; 4],
}

/// For each of `pages` (0-based), the boxes its effective media box does not contain, in page
/// order and then crop, bleed, trim, art.
///
/// The crop box is the page's own or an inherited one. Bleed, trim and art are not inheritable
/// (§7.7.3.3, the page object's entries; §7.7.3.4), so only the page's own count; an absent one defaults to
/// the crop box and is covered by the crop entry. A box that is not four finite numbers with a
/// positive width and height frames nothing and is skipped, as is an array with any entry that
/// isn't a number.
///
/// An edge within [`EDGE_TOLERANCE`] of the media box counts as inside, so a box equal to a media
/// box written from rounded arithmetic (`383.0` against `382.99999999999994`) is not listed.
/// A page given twice is listed twice, next to itself.
pub fn boxes_outside_media(doc: &Document, pages: &[usize]) -> Result<Vec<BoxOutside>, OrganizeError> {
    let all = walk(doc)?;
    check(pages, all.len())?;
    let effective = page_boxes(doc)?;
    let mut order = pages.to_vec();
    order.sort_unstable();
    let mut out = Vec::new();
    for i in order {
        let (Some((page, inherited)), Some(boxes)) = (all.get(i), effective.get(i)) else {
            continue;
        };
        let media = boxes[0];
        let obj = doc.get(*page);
        let own = obj.as_dict();
        let stored = |which: PageBox| {
            let key = which.key();
            let value = own.and_then(|d| d.get(key));
            let value = if which == PageBox::Crop { value.or_else(|| inherited.get(key)) } else { value };
            value.filter(|v| four_numbers(v, doc)).and_then(|v| rect_of(Some(v), doc))
        };
        for which in [PageBox::Crop, PageBox::Bleed, PageBox::Trim, PageBox::Art] {
            if let Some(rect) = stored(which).filter(|r| !contains(media, *r)) {
                out.push(BoxOutside { page: i, which, rect });
            }
        }
    }
    Ok(out)
}

/// Whether `o` is an array of exactly four entries that are all numbers, given directly or by
/// reference. [`rect_of`] alone skips an entry that isn't a number.
fn four_numbers(o: &Object, doc: &Document) -> bool {
    doc.resolve(o).as_array().is_some_and(|a| a.len() == 4 && a.iter().all(|x| doc.resolve(x).as_f64().is_some()))
}

/// How far, in default user-space units, an edge may pass the media box and still count as inside.
/// Far below anything a page can show, and far above the error of `f64` arithmetic on page sizes.
const EDGE_TOLERANCE: f64 = 1e-6;

/// Whether normalized rectangle `outer` contains `inner`, edges included, within [`EDGE_TOLERANCE`].
fn contains(outer: [f64; 4], inner: [f64; 4]) -> bool {
    let t = EDGE_TOLERANCE;
    inner[0] >= outer[0] - t && inner[1] >= outer[1] - t && inner[2] <= outer[2] + t && inner[3] <= outer[3] + t
}
