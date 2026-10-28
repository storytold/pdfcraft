//! Edit a PDF ▸ select and delete vector artwork painted directly by the page's content
//! streams. Chrome/Skia prints, for example, draw menu and toolbar icons as inline paths, not as
//! images or Form XObjects, so the image inventory cannot see them.
//!
//! A graphic is either a whole `q … Q` scope that paints only paths (its nested pure scopes
//! included), or, for paths painted in a scope shared with text, images or other content, one
//! painted path. Deleting a whole scope removes its saved graphics state with it; deleting a
//! path run removes only its construction and painting operators, never the colour, clip or
//! text state between them, which also belongs to the content that follows.

use std::collections::BTreeMap;

use pdfcraft_content::{Matrix, Pieces};
use pdfcraft_cos::{Document, Object};

use crate::EditError;
use crate::text::content_streams;

/// The most graphics one page's inventory lists, like [`crate::images::MAX_PAGE_IMAGES`].
pub const MAX_PAGE_GRAPHICS: usize = 100_000;
const MAX_OPERATIONS: usize = 100_000;
const MAX_DEPTH: usize = 1_024;
const MAX_COORDINATE: f64 = 1e12;
/// A shape covering this much of the page is background artwork, not an object to click.
const BACKGROUND_SHARE: f64 = 0.9;

/// A visible piece of vector artwork on a page.
#[derive(Clone, Debug, PartialEq)]
pub struct PageGraphic {
    /// Its visible box in user space, clipped to the page and the active clips.
    pub rect: [f64; 4],
    /// The operators it occupies in the page's joined streams (zero-based, end exclusive).
    start: usize,
    end: usize,
    /// A whole `q … Q` scope (rather than a path painted in a shared scope).
    whole_scope: bool,
}

fn page_of(doc: &Document, page: usize) -> Result<pdfcraft_model::Page, EditError> {
    pdfcraft_model::pages(doc).into_iter().nth(page).ok_or(EditError::NoSuchPage(page))
}

fn joined(streams: &[(Object, Vec<u8>)]) -> Pieces {
    Pieces::join(&streams.iter().map(|(_, data)| data.as_slice()).collect::<Vec<_>>())
}

fn intersection(a: [f64; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let r = [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])];
    (r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1]).then_some(r)
}

fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

/// A path's bounding box in user space, extended as its operators are read.
#[derive(Clone, Copy)]
struct Bounds {
    min: [f64; 2],
    max: [f64; 2],
    any: bool,
}

impl Bounds {
    fn new() -> Self {
        Self { min: [f64::INFINITY; 2], max: [f64::NEG_INFINITY; 2], any: false }
    }

    fn add(&mut self, x: f64, y: f64) {
        if x.is_finite() && y.is_finite() && x.abs() <= MAX_COORDINATE && y.abs() <= MAX_COORDINATE {
            self.any = true;
            self.min[0] = self.min[0].min(x);
            self.min[1] = self.min[1].min(y);
            self.max[0] = self.max[0].max(x);
            self.max[1] = self.max[1].max(y);
        }
    }

    fn rect(&self) -> Option<[f64; 4]> {
        let r = [self.min[0], self.min[1], self.max[0], self.max[1]];
        (self.any && r[2] > r[0] && r[3] > r[1]).then_some(r)
    }
}

/// A painted path (or, while parsing, the path being built).
struct Run {
    start: usize,
    end: usize,
    rect: [f64; 4],
    scope: usize,
}

/// A saved graphics state (`q`) and the purity of its contents.
struct Scope {
    start: usize,
    end: usize,
    parent: Option<usize>,
    /// A `q`, not the page's implicit outer scope; only these can be deleted as a whole.
    real: bool,
    /// It directly contains something other than path painting (text, an image, a Form, …).
    other: bool,
    /// A nested scope contains something other than path painting.
    impure_child: bool,
    /// It (or a nested pure scope) paints a visible path.
    painted: bool,
    closed: bool,
}

impl Scope {
    fn pure(&self) -> bool {
        self.real && self.closed && self.painted && !self.other && !self.impure_child
    }
}

struct Saved {
    matrix: Matrix,
    matrix_ok: bool,
    clip: [f64; 4],
}

/// Whether `op` only affects the graphics state or paints paths, so a scope holding just these
/// can be removed (or kept) as one piece of artwork.
fn graphics_operator(op: &[u8]) -> bool {
    matches!(
        op,
        b"q" | b"Q"
            | b"cm"
            | b"w"
            | b"J"
            | b"j"
            | b"M"
            | b"d"
            | b"ri"
            | b"i"
            | b"gs"
            | b"CS"
            | b"cs"
            | b"SC"
            | b"SCN"
            | b"sc"
            | b"scn"
            | b"G"
            | b"g"
            | b"RG"
            | b"rg"
            | b"K"
            | b"k"
            | b"W"
            | b"W*"
            | b"m"
            | b"l"
            | b"c"
            | b"v"
            | b"y"
            | b"h"
            | b"re"
            | b"S"
            | b"s"
            | b"f"
            | b"F"
            | b"f*"
            | b"B"
            | b"B*"
            | b"b"
            | b"b*"
            | b"n"
            | b"MP"
            | b"DP"
            | b"BMC"
            | b"BDC"
            | b"EMC"
            | b"BX"
            | b"EX"
    )
}

fn path_operator(op: &[u8]) -> bool {
    matches!(op, b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re")
}

fn paint_operator(op: &[u8]) -> bool {
    matches!(op, b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*")
}

fn marked_content_operator(op: &[u8]) -> bool {
    matches!(op, b"MP" | b"DP" | b"BMC" | b"BDC" | b"EMC")
}

/// The page's visible vector artwork, in drawing order. A whole pure-graphics `q … Q` scope is
/// one item, including the separate fill operators of a multi-part icon; paths painted in a
/// scope shared with text, images or Forms are one item each. Clip-only paths and page-sized
/// background shapes are left out.
pub fn page_graphics(doc: &Document, page: usize) -> Result<Vec<PageGraphic>, EditError> {
    let p = page_of(doc, page)?;
    let crop = p.crop(doc);
    let streams = content_streams(doc, &p.dict)?;
    let pieces = joined(&streams);
    let ops = pieces.parse().ops;
    if ops.len() > MAX_OPERATIONS {
        return Err(EditError::Invalid("this page has too many content operations for object selection".into()));
    }
    let mut scopes =
        vec![Scope { start: 0, end: ops.len(), parent: None, real: false, other: false, impure_child: false, painted: false, closed: false }];
    let mut stack = vec![0usize];
    let mut saved: Vec<Saved> = Vec::new();
    let mut matrix = Matrix::IDENTITY;
    let mut matrix_ok = true;
    let mut clip = crop;
    let mut path = Bounds::new();
    let mut path_start: Option<usize> = None;
    let mut path_bad = false;
    let mut pending_clip = false;
    let mut in_text = false;
    let mut inline_image = false;
    let mut runs: Vec<Run> = Vec::new();
    let current = |stack: &[usize]| stack.last().copied().unwrap_or(0);

    for (index, op) in ops.iter().enumerate() {
        let keyword = op.op.as_slice();
        if inline_image {
            if keyword == b"EI" {
                inline_image = false;
            }
            continue;
        }
        match keyword {
            b"q" => {
                if saved.len() >= MAX_DEPTH {
                    return Err(EditError::Invalid("the page graphics state is nested too deeply".into()));
                }
                let parent = current(&stack);
                scopes.push(Scope {
                    start: index,
                    end: ops.len(),
                    parent: Some(parent),
                    real: true,
                    other: false,
                    impure_child: false,
                    painted: false,
                    closed: false,
                });
                let id = scopes.len() - 1;
                if path_start.is_some() {
                    // Do not remove a `q` with the path it happens to contain.
                    path_bad = true;
                }
                saved.push(Saved { matrix, matrix_ok, clip });
                stack.push(id);
                continue;
            }
            b"Q" => {
                let state = saved.pop();
                if let Some(id) = stack.pop().filter(|id| *id != 0) {
                    let (painted, pure, parent) = {
                        let scope = &mut scopes[id];
                        scope.end = index + 1;
                        scope.closed = true;
                        (scope.painted, scope.pure(), scope.parent)
                    };
                    if let Some(parent) = parent {
                        if painted {
                            scopes[parent].painted = true;
                        }
                        if !pure {
                            scopes[parent].impure_child = true;
                        }
                    }
                }
                if let Some(s) = state {
                    matrix = s.matrix;
                    matrix_ok = s.matrix_ok;
                    clip = s.clip;
                }
                if path_start.is_some() {
                    path_bad = true;
                }
                continue;
            }
            _ => {}
        }
        if keyword == b"BT" {
            in_text = true;
            scopes[current(&stack)].other = true;
            if path_start.is_some() {
                path_bad = true;
            }
        } else if keyword == b"ET" {
            in_text = false;
            scopes[current(&stack)].other = true;
        } else if keyword == b"BI" {
            inline_image = true;
            scopes[current(&stack)].other = true;
            if path_start.is_some() {
                path_bad = true;
            }
        } else if path_operator(keyword) {
            if path_start.is_none() {
                path_start = Some(index);
            }
            let mut add = |x: f64, y: f64| {
                if matrix_ok {
                    let (x, y) = matrix.apply(x, y);
                    path.add(x, y);
                }
            };
            match keyword {
                b"m" | b"l" => match op.nums::<2>() {
                    Some([x, y]) => add(x, y),
                    None => path_bad = true,
                },
                b"c" => match op.nums::<6>() {
                    Some([x1, y1, x2, y2, x3, y3]) => {
                        for (x, y) in [(x1, y1), (x2, y2), (x3, y3)] {
                            add(x, y);
                        }
                    }
                    None => path_bad = true,
                },
                b"v" | b"y" => match op.nums::<4>() {
                    Some([x1, y1, x2, y2]) => {
                        for (x, y) in [(x1, y1), (x2, y2)] {
                            add(x, y);
                        }
                    }
                    None => path_bad = true,
                },
                b"re" => match op.nums::<4>() {
                    Some([x, y, w, h]) => {
                        for (x, y) in [(x, y), (x + w, y), (x + w, y + h), (x, y + h)] {
                            add(x, y);
                        }
                    }
                    None => path_bad = true,
                },
                _ => {}
            }
        } else if keyword == b"W" || keyword == b"W*" {
            pending_clip = true;
        } else if keyword == b"cm" {
            match op.nums::<6>() {
                Some(m) if m.iter().all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE) => {
                    matrix = Matrix(m).then(&matrix);
                }
                _ => matrix_ok = false,
            }
        } else if keyword == b"n" || paint_operator(keyword) {
            let painted = paint_operator(keyword) && !in_text && !path_bad;
            if let Some(bounds) = path.rect() {
                if painted
                    && !pending_clip
                    && let Some(rect) = intersection(bounds, clip)
                {
                    if runs.len() >= MAX_PAGE_GRAPHICS {
                        return Err(EditError::Invalid(format!("the page paints more than {MAX_PAGE_GRAPHICS} graphics")));
                    }
                    let scope = current(&stack);
                    scopes[scope].painted = true;
                    runs.push(Run { start: path_start.unwrap_or(index), end: index + 1, rect, scope });
                }
                if pending_clip {
                    clip = intersection(clip, bounds).unwrap_or([0.0; 4]);
                }
            } else if pending_clip {
                clip = [0.0; 4];
            }
            pending_clip = false;
            path = Bounds::new();
            path_start = None;
            path_bad = false;
        } else if keyword == b"Do" || keyword == b"sh" {
            scopes[current(&stack)].other = true;
            pending_clip = false;
            path = Bounds::new();
            path_start = None;
            path_bad = false;
        } else if !graphics_operator(keyword) {
            scopes[current(&stack)].other = true;
            if path_start.is_some() {
                path_bad = true;
            }
        }
    }

    // A run belongs to the outermost pure scope around it; its siblings then stay one icon or
    // logo rather than one object per filled bar. Runs outside any pure scope stay separate.
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut singles = Vec::new();
    for (index, run) in runs.iter().enumerate() {
        let mut id = Some(run.scope);
        let mut outer = None;
        while let Some(scope) = id {
            let s = &scopes[scope];
            if s.pure() {
                outer = Some(scope);
            }
            id = s.parent;
        }
        match outer {
            Some(scope) => groups.entry(scope).or_default().push(index),
            None => singles.push(index),
        }
    }
    let mut out = Vec::new();
    for (scope, members) in groups {
        let rect = members.iter().map(|i| runs[*i].rect).reduce(union);
        let Some(rect) = rect else { continue };
        if background(rect, crop) {
            // Artwork spanning the page is split back into its paths, so the icons printed into
            // a full-page graphic can still be selected; the background path itself is skipped.
            for i in members {
                if !background(runs[i].rect, crop) {
                    out.push(PageGraphic { rect: runs[i].rect, start: runs[i].start, end: runs[i].end, whole_scope: false });
                }
            }
        } else {
            let s = &scopes[scope];
            out.push(PageGraphic { rect, start: s.start, end: s.end, whole_scope: true });
        }
    }
    for i in singles {
        if !background(runs[i].rect, crop) {
            out.push(PageGraphic { rect: runs[i].rect, start: runs[i].start, end: runs[i].end, whole_scope: false });
        }
    }
    out.sort_by_key(|g| (g.start, g.end));
    out.dedup_by(|a, b| (a.start, a.end) == (b.start, b.end));
    Ok(out)
}

fn background(rect: [f64; 4], crop: [f64; 4]) -> bool {
    let Some(r) = intersection(rect, crop) else { return false };
    let page = (crop[2] - crop[0]) * (crop[3] - crop[1]);
    let area = (r[2] - r[0]) * (r[3] - r[1]);
    page > 0.0 && area >= page * BACKGROUND_SHARE
}

/// Delete graphic `index` (from [`page_graphics`]) from `page`. A whole pure scope goes with its
/// saved graphics state; a path painted in a shared scope loses only its path operators and
/// paint, so the colour, clip and text state around it stays as it was. Marked-content
/// operators stay too, keeping the page's tags balanced.
pub fn delete_graphic(doc: &mut Document, page: usize, index: usize) -> Result<(), EditError> {
    let graphics = page_graphics(doc, page)?;
    let graphic = graphics.get(index).ok_or_else(|| EditError::Invalid(format!("page {} has no graphic {}", page + 1, index + 1)))?.clone();
    let p = page_of(doc, page)?;
    let all = content_streams(doc, &p.dict)?;
    let pieces = joined(&all);
    let ops = pieces.parse().ops;
    if graphic.end > ops.len() || graphic.start >= graphic.end {
        return Err(EditError::Invalid("the graphic operation could not be located".into()));
    }
    let removes = |op: &[u8]| {
        if marked_content_operator(op) {
            return false;
        }
        if graphic.whole_scope { true } else { path_operator(op) || paint_operator(op) }
    };
    let data = crate::text::splice(&pieces, &ops, |i| {
        if i < graphic.start || i >= graphic.end {
            return (Vec::new(), true);
        }
        let keep = ops.get(i).is_none_or(|op| !removes(op.op.as_slice()));
        (Vec::new(), keep)
    });
    let contents = crate::text::rewritten_contents(doc, &all, data);
    doc.update_dict(p.obj, |d| d.set(b"Contents".to_vec(), contents))?;
    Ok(())
}
