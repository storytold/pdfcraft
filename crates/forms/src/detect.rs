//! Prepare a form ▸ automatic field detection: find where a printed form expects answers —
//! underscore runs, rules (horizontal lines), empty boxes, rows of comb cells and small squares —
//! and name each new field from the label next to it (Acrobat's auto-naming).
//!
//! [`detect`] is a pure function over the page's words and drawn shapes; [`page_shapes`] reads
//! the shapes from a page's content, [`raster::shapes`] from a picture of the page (a scan).

use pdfcraft_content::{Matrix, Pieces};
use pdfcraft_cos::{Document, Object};

pub mod raster;

/// A piece of text on the page, in user space.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    /// [x0, y0, x1, y1], y up.
    pub rect: [f64; 4],
}

/// Shapes drawn on the page, in user space: axis-aligned rectangle paths and horizontal segments.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shapes {
    pub boxes: Vec<[f64; 4]>,
    pub rules: Vec<[f64; 4]>,
}

/// What detection proposes.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Text,
    CheckBox,
    /// A row of touching cells, one character each: a text field with the comb flag and
    /// `cells` as its character limit.
    Comb {
        cells: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub kind: Kind,
    pub rect: [f64; 4],
    /// From the nearby label, made a valid field name.
    pub name: String,
}

/// The most points a subpath may have and still be read as a rectangle: its four corners plus
/// points along its sides (#2942: a cell drawn with a midpoint on each vertical side).
const MAX_RECT_POINTS: usize = 16;

/// Maximum rectangle inputs examined for combs and boxes, and maximum combined drawn/existing
/// rectangle inputs examined for check-box labels. Count inputs before filtering or deduplication:
/// repeated or irrelevant rectangles must not defeat the bound on pairwise comparisons.
pub const MAX_DETECTION_BOXES: usize = 2000;

/// The rectangles and horizontal segments a page draws (stroked or filled), in user space.
#[derive(Default)]
struct DetectionPath {
    points: Vec<[f64; 2]>,
    rules: Vec<[f64; 4]>,
    complex: bool,
    closed: bool,
}

impl DetectionPath {
    fn finish(&mut self, paths: &mut Vec<Self>) {
        let path = std::mem::take(self);
        if !path.rules.is_empty() || (!path.complex && path.points.len() >= 4) {
            paths.push(path);
        }
    }

    fn rectangle(&self, implicitly_closed: bool) -> Option<[f64; 4]> {
        if self.complex {
            return None;
        }
        if self.points.iter().flatten().any(|v| !v.is_finite()) {
            return None;
        }
        let near = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6;
        let mut points = self.points.clone();
        let repeated_end = points.len() > 1 && near(points[0], *points.last()?);
        if repeated_end {
            points.pop();
        }
        if !(self.closed || implicitly_closed || repeated_end) {
            return None;
        }
        // A point on a straight side, between its neighbours, doesn't change the shape: drop it,
        // so a cell drawn with a midpoint on a side is the rectangle it looks like (#2942). A
        // point that turns back along the side is not between its neighbours and stays, so a
        // path that doubles back is still not a rectangle.
        let between = |a: f64, m: f64, b: f64| a.min(b) - 1e-6 <= m && m <= a.max(b) + 1e-6;
        let mut i = 0;
        while points.len() > 4 && i < points.len() {
            let n = points.len();
            let (Some(&a), Some(&m), Some(&b)) = (points.get((i + n - 1) % n), points.get(i), points.get((i + 1) % n)) else { break };
            let on_row = (a[1] - m[1]).abs() < 1e-6 && (m[1] - b[1]).abs() < 1e-6 && between(a[0], m[0], b[0]);
            let on_column = (a[0] - m[0]).abs() < 1e-6 && (m[0] - b[0]).abs() < 1e-6 && between(a[1], m[1], b[1]);
            if on_row || on_column {
                points.remove(i);
                i = 0;
            } else {
                i += 1;
            }
        }
        if points.len() != 4 {
            return None;
        }
        let mut horizontal = [false; 4];
        for i in 0..4 {
            let (a, b) = (points[i], points[(i + 1) % 4]);
            let h = (a[1] - b[1]).abs() < 1e-6;
            let v = (a[0] - b[0]).abs() < 1e-6;
            if h == v {
                return None; // Diagonal or zero-length edge.
            }
            horizontal[i] = h;
        }
        if (0..4).any(|i| horizontal[i] == horizontal[(i + 1) % 4]) {
            return None;
        }
        Some([
            points.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min),
            points.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min),
            points.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max),
            points.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max),
        ])
    }
}

pub fn page_shapes(doc: &Document, page: usize) -> Shapes {
    let mut out = Shapes::default();
    let Some(p) = pdfcraft_model::pages(doc).into_iter().nth(page) else { return out };
    let list: Vec<Object> = match p.dict.get(b"Contents").map(|c| doc.resolve(c)) {
        Some(c) => match &*c {
            Object::Array(a) => a.clone(),
            Object::Stream(_) => vec![p.dict.get(b"Contents").cloned().unwrap_or(Object::Null)],
            _ => Vec::new(),
        },
        None => Vec::new(),
    };
    let mut pieces = Vec::new();
    for o in list {
        if let Object::Stream(s) = &*doc.resolve(&o) {
            pieces.push(s.decoded().unwrap_or_default());
        }
    }
    let mut ctm = Matrix::IDENTITY;
    let mut stack = Vec::new();
    let mut path_boxes: Vec<[f64; 4]> = Vec::new();
    let mut subpaths: Vec<DetectionPath> = Vec::new();
    let mut path = DetectionPath::default();
    let mut cur: Option<[f64; 2]> = None;
    for op in Pieces::join(&pieces).parse().ops {
        match op.op.as_slice() {
            b"q" => stack.push(ctm),
            b"Q" => ctm = stack.pop().unwrap_or(ctm),
            b"cm" => {
                if let Some(m) = op.nums::<6>() {
                    ctm = Matrix(m).then(&ctm);
                }
            }
            b"re" => {
                path.finish(&mut subpaths);
                if let Some([x, y, w, h]) = op.nums::<4>() {
                    path_boxes.push(ctm.bbox([x.min(x + w), y.min(y + h), x.max(x + w), y.max(y + h)]));
                    let (x, y) = ctm.apply(x, y);
                    cur = Some([x, y]);
                    path.complex = true;
                    path.points.push([x, y]);
                }
            }
            b"m" => {
                path.finish(&mut subpaths);
                cur = op.nums::<2>().map(|[x, y]| {
                    let (a, b) = ctm.apply(x, y);
                    [a, b]
                });
                if let Some(p) = cur {
                    path.points.push(p);
                }
            }
            b"l" => {
                if let (Some(a), Some([x, y])) = (cur, op.nums::<2>()) {
                    // Continuing after closepath is not a single four-edge rectangle.
                    if path.closed {
                        path.complex = true;
                    }
                    let (bx, by) = ctm.apply(x, y);
                    let b = [bx, by];
                    if !path.complex {
                        if path.points.len() > MAX_RECT_POINTS {
                            path.complex = true;
                        } else {
                            if path.points.is_empty() {
                                path.points.push(a);
                            }
                            path.points.push(b);
                        }
                    }
                    if (a[1] - b[1]).abs() < 0.5 && (a[0] - b[0]).abs() > 1.0 {
                        path.rules.push([a[0].min(b[0]), a[1], a[0].max(b[0]), a[1]]);
                    }
                    cur = Some(b);
                }
            }
            b"c" | b"v" | b"y" => {
                path.complex = true;
                let end = if op.op.as_slice() == b"c" { op.nums::<6>().map(|v| [v[4], v[5]]) } else { op.nums::<4>().map(|v| [v[2], v[3]]) };
                if let Some([x, y]) = end {
                    let (x, y) = ctm.apply(x, y);
                    cur = Some([x, y]);
                }
            }
            b"h" => {
                path.closed = true;
                cur = path.points.first().copied().or(cur);
            }
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" => {
                if matches!(op.op.as_slice(), b"s" | b"b" | b"b*") {
                    path.closed = true;
                }
                path.finish(&mut subpaths);
                let filled = !matches!(op.op.as_slice(), b"S" | b"s");
                let mut path_rules = Vec::new();
                for subpath in subpaths.drain(..) {
                    if let Some(rect) = subpath.rectangle(filled) {
                        path_boxes.push(rect);
                    } else {
                        path_rules.extend(subpath.rules);
                    }
                }
                for r in path_boxes.drain(..) {
                    // A very flat filled rectangle is a rule.
                    if r[3] - r[1] < 1.5 && r[2] - r[0] > 1.0 {
                        out.rules.push([r[0], (r[1] + r[3]) / 2.0, r[2], (r[1] + r[3]) / 2.0]);
                    } else {
                        out.boxes.push(r);
                    }
                }
                out.rules.append(&mut path_rules);
                cur = None;
            }
            b"n" => {
                path_boxes.clear();
                subpaths.clear();
                path = DetectionPath::default();
                cur = None;
            }
            _ => {}
        }
    }
    out
}

fn mid(r: [f64; 4]) -> f64 {
    (r[1] + r[3]) / 2.0
}

/// A label's words made into a field name: letters, digits and spaces, no trailing colon.
fn clean(label: &str) -> String {
    let s: String = label.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '/' { c } else { ' ' }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s.chars().take(40).collect::<String>().trim().to_string()
}

/// The label for a text field at `rect`: the words to its left on the same line, else the words
/// just above it.
fn label_for(words: &[Word], rect: [f64; 4]) -> String {
    let h = (rect[3] - rect[1]).max(8.0);
    let same_line = |w: &Word| (mid(w.rect) - mid(rect)).abs() < h.max(10.0) * 0.7;
    let mut left: Vec<&Word> = words.iter().filter(|w| same_line(w) && w.rect[2] <= rect[0] + 1.0 && rect[0] - w.rect[2] < 250.0).collect();
    left.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
    // The words that end nearest the field, back to a wide gap.
    let mut picked: Vec<&str> = Vec::new();
    let mut x = rect[0];
    for w in left.iter().rev() {
        if x - w.rect[2] > 24.0 && !picked.is_empty() {
            break;
        }
        if w.text.chars().all(|c| c == '_') {
            break;
        }
        picked.insert(0, w.text.as_str());
        x = w.rect[0];
    }
    let l = clean(&picked.join(" "));
    if !l.is_empty() {
        return l;
    }
    // Above: the nearest line of words over the field's left part.
    let above: Vec<&Word> = words
        .iter()
        .filter(|w| w.rect[1] >= rect[3] - 1.0 && w.rect[1] - rect[3] < 24.0 && w.rect[0] < rect[2] && w.rect[2] > rect[0] - 4.0)
        .collect();
    let Some(line) = above.iter().map(|w| mid(w.rect)).min_by(f64::total_cmp) else { return String::new() };
    let mut on: Vec<&&Word> = above.iter().filter(|w| (mid(w.rect) - line).abs() < 3.0).collect();
    on.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
    clean(&on.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "))
}

/// Sort items into reading order: top to bottom (rows within 8 pt of the row's first), left to right.
fn sort_reading_order<T>(mut items: Vec<T>, rect_of: impl Fn(&T) -> [f64; 4]) -> Vec<T> {
    items.sort_by(|a, b| mid(rect_of(b)).total_cmp(&mid(rect_of(a))));
    let mut rows = Vec::with_capacity(items.len());
    let (mut row, mut start) = (0usize, f64::INFINITY);
    for item in &items {
        if start - mid(rect_of(item)) > 8.0 {
            row += 1;
            start = mid(rect_of(item));
        }
        rows.push(row);
    }
    let mut keyed: Vec<(usize, T)> = rows.into_iter().zip(items).collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then(rect_of(&a.1)[0].total_cmp(&rect_of(&b.1)[0])));
    keyed.into_iter().map(|(_, item)| item).collect()
}

/// The label for a check box at `rect`: the nearer unclaimed phrase on the same line (to its
/// right or to its left), without crossing or stealing a phrase that sits closer to an adjacent
/// check box on the line.
fn checkbox_label(words: &[Word], rect: [f64; 4], boxes: &[[f64; 4]], used: &mut std::collections::HashSet<usize>) -> String {
    let h = (rect[3] - rect[1]).max(8.0);
    let same_line_word = |r: [f64; 4]| (mid(r) - mid(rect)).abs() < h.max(10.0) * 0.7;
    let same_line_box = |b: [f64; 4]| {
        #[cfg(test)]
        work_bound_tests::record_comparison();
        (mid(b) - mid(rect)).abs() < h.max(b[3] - b[1]).max(10.0) * 0.7
    };
    let has_content = |w: &Word| !clean(&w.text).is_empty();
    let prev_box_x2 =
        boxes.iter().copied().filter(|b| *b != rect && same_line_box(*b) && b[2] <= rect[0] + 1.0).map(|b| b[2]).fold(f64::NEG_INFINITY, f64::max);
    let next_box_x0 =
        boxes.iter().copied().filter(|b| *b != rect && same_line_box(*b) && b[0] >= rect[2] - 1.0).map(|b| b[0]).fold(f64::INFINITY, f64::min);

    let mut left_words: Vec<(usize, &Word)> = words
        .iter()
        .enumerate()
        .filter(|(i, w)| {
            !used.contains(i) && same_line_word(w.rect) && w.rect[2] <= rect[0] + 1.0 && rect[0] - w.rect[2] < 200.0 && w.rect[0] >= prev_box_x2 - 1.0
        })
        .collect();
    left_words.sort_by(|a, b| a.1.rect[0].total_cmp(&b.1.rect[0]));
    let mut left_picked: Vec<(usize, &Word)> = Vec::new();
    let mut x = rect[0];
    let mut has_word = false;
    for (i, w) in left_words.into_iter().rev() {
        if x - w.rect[2] > 24.0 || w.text.chars().all(|c| c == '_') || (has_word && w.text.trim_end().ends_with(':')) {
            break;
        }
        has_word |= has_content(w);
        left_picked.push((i, w));
        x = w.rect[0];
    }
    left_picked.reverse();
    let left_candidate = match (left_picked.iter().find(|(_, w)| has_content(w)), left_picked.iter().rev().find(|(_, w)| has_content(w))) {
        (Some((_, first)), Some((_, last))) => {
            let text = clean(&left_picked.iter().map(|(_, w)| w.text.as_str()).collect::<Vec<_>>().join(" "));
            let gap = (rect[0] - last.rect[2]).max(0.0);
            let prev_gap = (first.rect[0] - prev_box_x2).max(0.0);
            let colon = left_picked.last().is_some_and(|(_, w)| w.text.trim_end().ends_with(':')) || last.text.trim_end().ends_with(':');
            (!text.is_empty() && gap <= prev_gap).then_some((text, gap, colon, left_picked))
        }
        _ => None,
    };

    let mut right_words: Vec<(usize, &Word)> = words
        .iter()
        .enumerate()
        .filter(|(i, w)| {
            !used.contains(i) && same_line_word(w.rect) && w.rect[0] >= rect[2] - 1.0 && w.rect[0] - rect[2] < 200.0 && w.rect[2] <= next_box_x0 + 1.0
        })
        .collect();
    right_words.sort_by(|a, b| a.1.rect[0].total_cmp(&b.1.rect[0]));
    let mut right_picked: Vec<(usize, &Word)> = Vec::new();
    let mut x = rect[2];
    let mut has_word = false;
    for (i, w) in right_words {
        if w.rect[0] - x > 24.0 || w.text.chars().all(|c| c == '_') || (has_word && (next_box_x0 - w.rect[2]).max(0.0) < w.rect[0] - x) {
            break;
        }
        has_word |= has_content(w);
        right_picked.push((i, w));
        x = w.rect[2];
    }
    let right_candidate = match (right_picked.iter().find(|(_, w)| has_content(w)), right_picked.iter().rev().find(|(_, w)| has_content(w))) {
        (Some((_, first)), Some((_, last))) => {
            let text = clean(&right_picked.iter().map(|(_, w)| w.text.as_str()).collect::<Vec<_>>().join(" "));
            let gap = (first.rect[0] - rect[2]).max(0.0);
            let next_gap = (next_box_x0 - last.rect[2]).max(0.0);
            (!text.is_empty() && gap <= next_gap).then_some((text, gap, next_gap, right_picked))
        }
        _ => None,
    };

    let chosen = match (left_candidate, right_candidate) {
        (Some((lt, l_gap, colon, l_words)), Some((rt, r_gap, next_gap, r_words))) => {
            let pick_left = !colon && (l_gap < r_gap || (next_gap <= 24.0 && l_gap <= r_gap));
            if pick_left { Some((lt, l_words)) } else { Some((rt, r_words)) }
        }
        (Some((lt, _, _, l_words)), None) => Some((lt, l_words)),
        (None, Some((rt, _, _, r_words))) => Some((rt, r_words)),
        (None, None) => None,
    };
    if let Some((text, picked)) = chosen {
        for (i, _) in picked {
            used.insert(i);
        }
        text
    } else {
        String::new()
    }
}

/// Rows of two or more touching cells of one size, as (the row's rect, cell count). Two is
/// enough: dates split into day and month groups of two cells. Cells are square or taller
/// than wide; an empty table of wider cells is not a comb.
fn combs(boxes: &[[f64; 4]]) -> Vec<([f64; 4], usize)> {
    let cells: Vec<[f64; 4]> = boxes
        .iter()
        .copied()
        .filter(|r| {
            let (w, h) = (r[2] - r[0], r[3] - r[1]);
            (5.0..=30.0).contains(&w) && (8.0..=40.0).contains(&h) && w <= h * 1.2
        })
        .collect();
    // `b` comes right after `a` in a row: same size, its left edge on a's right edge.
    let follows = |a: &[f64; 4], b: &[f64; 4]| {
        #[cfg(test)]
        work_bound_tests::record_comparison();
        (b[0] - a[2]).abs() <= 1.0 && (b[1] - a[1]).abs() <= 0.5 && (b[3] - a[3]).abs() <= 0.5 && ((b[2] - b[0]) - (a[2] - a[0])).abs() <= 0.5
    };
    let mut out = Vec::new();
    for first in &cells {
        if cells.iter().any(|a| follows(a, first)) {
            continue;
        }
        let (mut last, mut n) = (first, 1usize);
        // Each step moves at least 4 pt right, so this ends; the bound is for safety.
        while let Some(b) = cells.iter().find(|b| follows(last, b))
            && n < cells.len()
        {
            last = b;
            n += 1;
        }
        if n >= 2 {
            out.push(([first[0], first[1], last[2], first[3]], n));
        }
    }
    out
}

/// A balanced bounding-volume tree indexes *all* existing widgets, independently of the
/// proposal/label budget. Far-away widgets can be excluded together instead of rescanning
/// the entire page for each proposal. Construction is O(n log n), with logarithmic depth.
struct WidgetIndex {
    bounds: [f64; 4],
    contents: WidgetContents,
}

enum WidgetContents {
    Leaf(Box<[[f64; 4]]>),
    Branch(Box<WidgetIndex>, Box<WidgetIndex>),
}

fn rectangles_overlap(a: [f64; 4], b: [f64; 4]) -> bool {
    #[cfg(test)]
    work_bound_tests::record_comparison();
    a[0] < b[2] - 1.0 && b[0] < a[2] - 1.0 && a[1] < b[3] - 1.0 && b[1] < a[3] - 1.0
}

impl WidgetIndex {
    fn new(rects: &[[f64; 4]]) -> Option<Self> {
        // A NaN makes the exact overlap predicate false. Infinities and inverted boxes are
        // retained: pruning uses normalized bounds, but leaves preserve original geometry.
        let mut rects: Vec<_> = rects.iter().copied().filter(|r| !r.iter().any(|x| x.is_nan())).collect();
        Self::build(&mut rects, 0)
    }

    fn build(rects: &mut [[f64; 4]], depth: usize) -> Option<Self> {
        let first = rects.first()?;
        let mut bounds = [first[0].min(first[2]), first[1].min(first[3]), first[0].max(first[2]), first[1].max(first[3])];
        for r in rects.iter().skip(1) {
            bounds = [bounds[0].min(r[0]).min(r[2]), bounds[1].min(r[1]).min(r[3]), bounds[2].max(r[0]).max(r[2]), bounds[3].max(r[1]).max(r[3])];
        }
        let contents = if rects.len() <= 8 {
            WidgetContents::Leaf(rects.to_vec().into_boxed_slice())
        } else {
            let axis = depth % 2;
            let mid = rects.len() / 2;
            rects.select_nth_unstable_by(mid, |a, b| a[axis].total_cmp(&b[axis]));
            let (left, right) = rects.split_at_mut(mid);
            WidgetContents::Branch(Box::new(Self::build(left, depth + 1)?), Box::new(Self::build(right, depth + 1)?))
        };
        Some(Self { bounds, contents })
    }

    fn overlaps(&self, rect: [f64; 4], remaining: &mut usize) -> bool {
        // Spatial indexes can degenerate on adversarial geometry. Exhaustion suppresses
        // the unverified proposal, rather than overlooking any existing widget exclusion.
        if *remaining == 0 {
            return true;
        }
        *remaining -= 1;
        #[cfg(test)]
        work_bound_tests::record_index_visit();
        if !rectangles_overlap(self.bounds, rect) {
            return false;
        }
        match &self.contents {
            WidgetContents::Leaf(rects) => rects.iter().any(|r| {
                if *remaining == 0 {
                    return true;
                }
                *remaining -= 1;
                rectangles_overlap(*r, rect)
            }),
            WidgetContents::Branch(left, right) => left.overlaps(rect, remaining) || right.overlaps(rect, remaining),
        }
    }
}

/// Propose fields for a page. `existing` are the page's widget rectangles (no candidate
/// overlaps them); names are unique against `taken`.
///
/// Box detection considers only the first [`MAX_DETECTION_BOXES`] drawn rectangles in input
/// order, and check-box labelling considers at most that many drawn/existing rectangles combined.
/// Limits apply before pairwise comb/overlap/label work, not just to accepted candidates. This
/// heuristic can return a partial set on unusually complex pages, like raster shape detection;
/// the retained candidates are still returned in reading order. Every existing widget remains
/// an overlap exclusion, even when it falls outside the label-detection budget.
pub fn detect(words: &[Word], shapes: &Shapes, existing: &[[f64; 4]], taken: &[String]) -> Vec<Candidate> {
    // Bound the comb pass too: small squares qualify as comb cells, so limiting only the new
    // check-box loop would still leave quadratic work on all of a hostile page's rectangles.
    let boxes = shapes.boxes.get(..shapes.boxes.len().min(MAX_DETECTION_BOXES)).unwrap_or_default();
    let mut out: Vec<Candidate> = Vec::new();
    let overlaps = rectangles_overlap;
    let existing_index = WidgetIndex::new(existing);
    let mut existing_work = 64 * MAX_DETECTION_BOXES;
    let mut push = |kind: Kind, rect: [f64; 4], label: String, out: &mut Vec<Candidate>| {
        if existing_index.as_ref().is_some_and(|index| index.overlaps(rect, &mut existing_work)) || out.iter().any(|c| overlaps(c.rect, rect)) {
            return;
        }
        out.push(Candidate { kind, rect, name: label });
    };
    // Text that isn't underscores (labels), and underscore runs (blanks).
    let (blanks, labels): (Vec<&Word>, Vec<&Word>) = words.iter().partition(|w| w.text.len() >= 3 && w.text.chars().all(|c| c == '_'));
    let labels: Vec<Word> = labels.into_iter().cloned().collect();
    for b in blanks {
        let h = (b.rect[3] - b.rect[1]).max(10.0);
        let rect = [b.rect[0], b.rect[1], b.rect[2], b.rect[1] + (h * 1.4).clamp(12.0, 22.0)];
        let l = label_for(&labels, rect);
        push(Kind::Text, rect, l, &mut out);
    }
    // Comb boxes (postcodes, dates, IBANs): one text field per row of cells, found before
    // check boxes so the cells don't each become one.
    for (rect, cells) in combs(boxes) {
        if labels.iter().any(|x| overlaps(x.rect, rect)) {
            continue;
        }
        let l = label_for(&labels, rect);
        push(Kind::Comb { cells }, rect, l, &mut out);
    }
    // Small squares: check boxes, labelled by the nearer text to their right or left.
    // `existing` contains geometry only: a small square widget of any type reserves its label.
    // Cap examined inputs, not accepted boxes, so duplicates cannot keep this pass running.
    let mut checkboxes: Vec<[f64; 4]> = Vec::new();
    for r in boxes.iter().chain(existing.iter()).take(MAX_DETECTION_BOXES) {
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        if (5.0..=20.0).contains(&w)
            && (5.0..=20.0).contains(&h)
            && (w / h - 1.0).abs() < 0.25
            && !out.iter().map(|c| &c.rect).chain(checkboxes.iter()).any(|e| overlaps(*e, *r))
        {
            checkboxes.push(*r);
        }
    }
    let checkboxes = sort_reading_order(checkboxes, |r| *r);
    let mut used_words = std::collections::HashSet::new();
    for r in &checkboxes {
        let l = checkbox_label(&labels, *r, &checkboxes, &mut used_words);
        push(Kind::CheckBox, *r, l, &mut out);
    }
    // Empty boxes big enough to write in: text fields inside them.
    for r in boxes {
        let (w, h) = (r[2] - r[0], r[3] - r[1]);
        let has_text = labels.iter().any(|x| overlaps(x.rect, *r));
        if w >= 40.0 && (12.0..=60.0).contains(&h) && !has_text {
            let inner = [r[0] + 1.0, r[1] + 1.0, r[2] - 1.0, r[3] - 1.0];
            let l = label_for(&labels, *r);
            push(Kind::Text, inner, l, &mut out);
        }
    }
    // Rules: a text field sitting on the line.
    for r in &shapes.rules {
        let w = r[2] - r[0];
        if w < 40.0 {
            continue;
        }
        // A line that underlines text (or a table border with text on it) isn't a blank.
        let rect = [r[0], r[1] + 1.0, r[2], r[1] + 17.0];
        if labels.iter().any(|x| overlaps(x.rect, rect)) {
            continue;
        }
        let l = label_for(&labels, rect);
        push(Kind::Text, rect, l, &mut out);
    }
    let mut out = sort_reading_order(out, |c| c.rect);
    // Unique names (Acrobat numbers repeats); unlabelled fields get Text1, Check Box1, …
    // A linear membership scan inside each suffix retry makes a page of unnamed squares
    // cubic even after bounding the geometric passes.
    let mut used: std::collections::HashSet<String> = taken.iter().cloned().collect();
    for c in &mut out {
        let base = if c.name.is_empty() {
            match c.kind {
                Kind::Text | Kind::Comb { .. } => "Text".to_string(),
                Kind::CheckBox => "Check Box".to_string(),
            }
        } else {
            c.name.clone()
        };
        let mut name = if c.name.is_empty() { format!("{base}1") } else { base.clone() };
        let mut n = 1;
        while used.contains(&name) {
            n += 1;
            name = format!("{base}{n}");
        }
        used.insert(name.clone());
        c.name = name;
    }
    out
}

#[cfg(test)]
mod work_bound_tests {
    use super::*;
    use std::cell::Cell;

    // Per-thread so the other detection tests can run concurrently. Count work rather than
    // wall time: appending more hostile rectangles must not add pairwise comparisons.
    thread_local! {
        static COMPARISONS: Cell<usize> = const { Cell::new(0) };
        static INDEX_VISITS: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn record_comparison() {
        COMPARISONS.set(COMPARISONS.get() + 1);
    }

    pub(super) fn record_index_visit() {
        INDEX_VISITS.set(INDEX_VISITS.get() + 1);
    }

    #[test]
    fn rectangle_budget_bounds_comparisons_before_filtering() {
        for duplicates in [false, true] {
            for existing_only in [false, true] {
                let boxes = (0..MAX_DETECTION_BOXES)
                    .map(|i| {
                        let x = if duplicates { 0.0 } else { i as f64 * 30.0 };
                        [x, 0.0, x + 10.0, 10.0]
                    })
                    .collect();
                let mut shapes = Shapes { boxes, rules: Vec::new() };
                let run = |shapes: &Shapes| {
                    if existing_only { detect(&[], &Shapes::default(), &shapes.boxes, &[]) } else { detect(&[], shapes, &[], &[]) }
                };
                COMPARISONS.set(0);
                let expected = run(&shapes);
                let bounded = COMPARISONS.get();
                assert!(bounded > 0 && bounded <= 5 * MAX_DETECTION_BOXES * MAX_DETECTION_BOXES);
                shapes.boxes.extend_from_within(..);
                COMPARISONS.set(0);
                assert_eq!(run(&shapes), expected);
                if existing_only {
                    // Existing widgets are never discarded: doubling them may add tree levels,
                    // but must not restore the full pairwise scan.
                    assert!(COMPARISONS.get() <= bounded + 16 * MAX_DETECTION_BOXES);
                } else {
                    assert_eq!(COMPARISONS.get(), bounded, "discarded inputs add no pairwise work");
                }
            }
        }
    }

    #[test]
    fn many_existing_widgets_do_not_multiply_proposal_work() {
        let mut existing: Vec<_> = (0..100_000)
            .map(|i| {
                let x = i as f64 * 30.0;
                [x, 100.0, x + 10.0, 110.0]
            })
            .collect();
        let shapes = Shapes {
            boxes: (0..MAX_DETECTION_BOXES)
                .map(|i| {
                    let x = i as f64 * 30.0;
                    [x, 0.0, x + 10.0, 10.0]
                })
                .collect(),
            rules: Vec::new(),
        };
        // An exclusion beyond both budgets must still suppress the last drawn candidate.
        let exclusion = [59_970.0, 0.0, 59_980.0, 10.0];
        existing.push(exclusion);
        COMPARISONS.set(0);
        let expected = detect(&[], &shapes, &[exclusion], &[]);
        let baseline_work = COMPARISONS.get();
        COMPARISONS.set(0);
        INDEX_VISITS.set(0);
        let candidates = detect(&[], &shapes, &existing, &[]);
        assert_eq!(candidates.len(), MAX_DETECTION_BOXES - 1);
        assert_eq!(candidates, expected);
        assert!(!candidates.iter().any(|c| c.rect == [59_970.0, 0.0, 59_980.0, 10.0]));
        assert!(COMPARISONS.get() <= baseline_work + 64 * MAX_DETECTION_BOXES, "existing-widget comparisons must not be candidates times widgets");
        assert!(INDEX_VISITS.get() < 100 * MAX_DETECTION_BOXES);
    }

    #[test]
    fn widget_index_matches_linear_overlap_semantics() {
        let mut rects =
            vec![[0.0, 0.0, 10.0, 10.0], [30.0, 20.0, 20.0, 10.0], [f64::NEG_INFINITY, 50.0, f64::INFINITY, 60.0], [f64::NAN, 0.0, 10.0, 10.0]];
        rects.extend((0..500).map(|i| {
            let x = (i % 25) as f64 * 3.0;
            let y = (i / 25) as f64 * 4.0;
            [x, y, x + 2.0, y + 2.0]
        }));
        let index = WidgetIndex::new(&rects).expect("nonempty index");
        for x in -5..40 {
            for y in -5..70 {
                let query = [x as f64, y as f64, x as f64 + 12.0, y as f64 + 12.0];
                let mut remaining = usize::MAX;
                assert_eq!(index.overlaps(query, &mut remaining), rects.iter().any(|r| rectangles_overlap(*r, query)));
            }
        }
    }

    #[test]
    fn adversarial_widget_geometry_exhausts_budget_without_accepting_unchecked_fields() {
        let existing = vec![[1e9, 1e9, -1e9, -1e9]; 100_000];
        let shapes = Shapes {
            boxes: (0..MAX_DETECTION_BOXES)
                .map(|i| {
                    let x = i as f64 * 30.0;
                    [x, 0.0, x + 10.0, 10.0]
                })
                .collect(),
            rules: Vec::new(),
        };
        COMPARISONS.set(0);
        INDEX_VISITS.set(0);
        let candidates = detect(&[], &shapes, &existing, &[]);
        assert!(candidates.is_empty(), "an exhausted search cannot verify any proposal");
        assert!(INDEX_VISITS.get() <= 64 * MAX_DETECTION_BOXES);
        assert!(COMPARISONS.get() < 5 * MAX_DETECTION_BOXES * MAX_DETECTION_BOXES);
    }
}
