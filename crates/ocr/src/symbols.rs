//! Tolerance and dimension symbols the ocrs models cannot name (#8300).
//!
//! The recognition model's alphabet ([`super::ALPHABET`]) is ASCII and the euro sign: it has no
//! `±`, `Ø` or `°`, the three symbols every dimensioned drawing's title block and callouts
//! carry. The CTC decoder names the nearest class instead — `+`, `-`, `t` or `:` for `±`; `0`,
//! `8`, `@` or `?` for `Ø`; `?` or nothing for `°` — and a tolerance of ±.005 silently becomes
//! +.005. After a line is read, the characters that can stand in for a symbol are checked
//! against the ink they were read from, and renamed only when that ink has the symbol's shape:
//!
//! - `±`: a centred cross with a detached bar under it, as wide as the crossbar.
//! - `Ø`: a ring with one straight stroke across it from lower left to upper right that reaches
//!   past the ring at both ends (a slashed zero keeps its stroke inside the ring, and stays).
//! - `°`: a small ring in the top half of the line, right after a digit, with nothing under it.
//!
//! Only a character next to a digit is checked (`± .005`, `Ø1.25`, `1/2°`), so a plus between
//! words is never touched, and the checks are conservative: too small to judge, too faint, or no
//! clear shape is a no-op. Tesseract's alphabet has these characters, so its words are not
//! checked. The shapes are the ones ISO 129-1 and ASME Y14.5 draw and every sans-serif face
//! follows; the thresholds were chosen on glyphs drawn in code (the tests), on a title block
//! rendered at 150 and 300 dpi, and on a drawing scanned at 600 dpi.

/// A character the engine read and the image columns it was read from (pixels, left and right).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ReadChar {
    pub ch: char,
    pub left: f32,
    pub right: f32,
}

/// A word's text after the check, and its new right edge when a dropped `°` was found past it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Repair {
    pub text: String,
    pub right: Option<f32>,
}

/// The grey value (0 ink … 255 paper) at an image pixel; 255 outside the image.
pub(crate) type Gray<'a> = &'a dyn Fn(i64, i64) -> u8;

/// How far past a character's columns its glyph may lie, as a fraction of the line's height:
/// the columns come from the recogniser's coarse steps, not from the ink.
const SLACK: f32 = 0.2;
/// How far past the last character a dropped `°` is looked for, in line heights.
const TAIL: f32 = 0.8;
/// The least contrast (darkest to lightest pixel) a region needs to be judged at all.
const MIN_CONTRAST: u32 = 64;
/// The most pixels a word's region may hold; a larger one is left alone.
const MAX_REGION: usize = 2_000_000;
/// Characters the decoder names for a `±`, a `Ø`, and a `°`.
const STANDS_FOR_PLUS_MINUS: &str = "+-t:=";
const STANDS_FOR_DIAMETER: &str = "0Oo8@?Q";
const STANDS_FOR_DEGREE: &str = "?oO0'\"*^";

/// Check one word. `line_top..line_bottom` is the line's box as found (image rows), `next_left`
/// where the next word on the line starts, `next_digit` whether that word starts with a digit
/// or a decimal point. `None` when nothing changes.
pub(crate) fn repair_word(
    chars: &[ReadChar],
    line_top: f32,
    line_bottom: f32,
    next_left: Option<f32>,
    next_digit: bool,
    gray: Gray,
) -> Option<Repair> {
    let text: Vec<char> = chars.iter().map(|c| c.ch).collect();
    if !has_candidate(&text, next_digit) {
        return None;
    }
    let (first, last) = (chars.first()?, chars.last()?);
    let height = line_bottom - line_top;
    if !(height.is_finite() && height >= 6.0) {
        return None;
    }
    let slack = SLACK * height;
    // The region reaches [`TAIL`] past the last character for a dropped `°`, but not past the
    // next word, and not on along a last character whose columns run to the line's end.
    let mut right = (last.right + TAIL * height).min(last.left + (1.0 + 2.0 * TAIL) * height);
    if let Some(next) = next_left {
        right = right.min(next - 1.0);
    }
    let region = Ink::sample(gray, [first.left - slack, line_top - slack, right + slack, line_bottom + slack])?;
    let shapes = Shapes::of(&region)?;
    let mut out = text.clone();
    let mut consumed = Vec::new();
    let mut i = 0;
    while i < out.len() {
        let c = out[i];
        if STANDS_FOR_PLUS_MINUS.contains(c) && digit_follows(&out, i, next_digit) {
            if let Some((ids, glyph)) = shapes.stacked_glyph(chars[i], slack)
                && is_plus_minus(&glyph)
            {
                out[i] = '±';
                consumed.extend(ids);
            }
        } else if STANDS_FOR_DIAMETER.contains(c)
            && starts_a_number(&out, i, next_digit)
            && let Some((_, glyph)) = shapes.main_glyph(chars[i], slack)
            && is_diameter(&glyph)
        {
            out[i] = 'Ø';
        }
        i += 1;
    }
    // The `°` first, while `out` still lines up with `chars`: dropping a character would put
    // the digit's columns off by one.
    let new_right = degree(&mut out, chars, &shapes, slack, next_left);
    // A second character the decoder named for the bar of a `±` goes with it.
    drop_consumed(&mut out, chars, &shapes, slack, &consumed);
    (out != text).then(|| Repair { text: out.into_iter().collect(), right: new_right })
}

/// Whether any character of `text` could stand for a symbol in a numeric context: the quick
/// test that keeps ordinary words away from the pixels.
fn has_candidate(text: &[char], next_digit: bool) -> bool {
    text.iter().enumerate().any(|(i, &c)| {
        (STANDS_FOR_PLUS_MINUS.contains(c) && digit_follows(text, i, next_digit))
            || (STANDS_FOR_DIAMETER.contains(c) && starts_a_number(text, i, next_digit))
    }) || !degree_candidates(text).is_empty()
}

/// A digit, or a decimal point and a digit, follows position `i` (in this word, or at the start
/// of the next one when `i` is the last character).
fn digit_follows(text: &[char], i: usize, next_digit: bool) -> bool {
    match text.get(i + 1) {
        Some(c) if c.is_ascii_digit() => true,
        Some('.') => text.get(i + 2).is_some_and(char::is_ascii_digit),
        Some(_) => false,
        None => next_digit,
    }
}

/// Position `i` begins a number: nothing alphanumeric before it, a digit after it.
fn starts_a_number(text: &[char], i: usize, next_digit: bool) -> bool {
    let after_separator = i == 0 || text.get(i - 1).is_some_and(|c| !c.is_alphanumeric());
    after_separator && digit_follows(text, i, next_digit)
}

/// The indices of the digits a `°` could follow, nearest the end first: the word's last
/// character when it is a digit, and the one before a last character the decoder may have named
/// for the ring. A last `0` is both: a digit, or the name for a ring after the digit before it.
fn degree_candidates(text: &[char]) -> Vec<usize> {
    let Some(last) = text.len().checked_sub(1) else {
        return Vec::new();
    };
    let mut digits = Vec::new();
    if text[last].is_ascii_digit() {
        digits.push(last);
    }
    if STANDS_FOR_DEGREE.contains(text[last]) && last >= 1 && text[last - 1].is_ascii_digit() {
        digits.push(last - 1);
    }
    digits
}

/// Drop a character beside a `±` when the decoder named it for part of the same ink, a cross
/// and bar read as two characters: its glyph is one of the components the `±` consumed.
fn drop_consumed(out: &mut Vec<char>, chars: &[ReadChar], shapes: &Shapes, slack: f32, consumed: &[usize]) {
    if consumed.is_empty() {
        return;
    }
    let dropped: Vec<usize> = (0..out.len())
        .filter(|&i| {
            let beside_symbol = out[i] != '±' && ((i >= 1 && out[i - 1] == '±') || out.get(i + 1) == Some(&'±'));
            let named_for_part = STANDS_FOR_PLUS_MINUS.contains(out[i]) || out[i] == '_';
            beside_symbol && named_for_part && chars.get(i).and_then(|c| shapes.main_glyph(*c, slack)).is_some_and(|(id, _)| consumed.contains(&id))
        })
        .collect();
    for i in dropped.into_iter().rev() {
        out.remove(i);
    }
}

/// Rename or append the `°` after the word's last digit when a small raised ring follows that
/// digit's glyph; returns the word's new right edge when the ring lay past the word.
fn degree(out: &mut Vec<char>, chars: &[ReadChar], shapes: &Shapes, slack: f32, next_left: Option<f32>) -> Option<f32> {
    for digit in degree_candidates(out) {
        let Some((_, glyph)) = chars.get(digit).and_then(|c| shapes.main_glyph(*c, slack)) else {
            continue;
        };
        let Some(ring) = shapes.ring_after(&glyph, next_left) else {
            continue;
        };
        if !shapes.is_degree(&ring) {
            continue;
        }
        let ring_right = ring.right as f32 + 1.0;
        if digit + 1 < out.len() {
            out.truncate(digit + 1);
            out.push('°');
            return None;
        }
        out.push('°');
        return (ring_right > chars.last().map_or(ring_right, |c| c.right)).then_some(ring_right);
    }
    None
}

/// The ink of a region: `on` where a pixel is darker than the midpoint between the region's
/// darkest and lightest pixel. Image coordinates throughout.
struct Ink {
    left: i64,
    top: i64,
    w: usize,
    h: usize,
    on: Vec<bool>,
}

impl Ink {
    /// Sample `[left, top, right, bottom]` (image pixels, floats from the engine). `None` when
    /// the region is empty, too large or too faint to judge.
    fn sample(gray: Gray, rect: [f32; 4]) -> Option<Ink> {
        if rect.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let (left, top) = (rect[0].floor() as i64, rect[1].floor() as i64);
        let (w, h) = (usize::try_from(rect[2].ceil() as i64 - left).ok()?, usize::try_from(rect[3].ceil() as i64 - top).ok()?);
        if w == 0 || h == 0 || w.checked_mul(h)? > MAX_REGION {
            return None;
        }
        let values: Vec<u8> = (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| gray(left + x as i64, top + y as i64)).collect();
        let (lo, hi) = (u32::from(*values.iter().min()?), u32::from(*values.iter().max()?));
        if hi - lo < MIN_CONTRAST {
            return None;
        }
        let threshold = (lo + hi) / 2;
        Some(Ink { left, top, w, h, on: values.iter().map(|&g| u32::from(g) < threshold).collect() })
    }

    fn at(&self, x: i64, y: i64) -> bool {
        let (lx, ly) = (x - self.left, y - self.top);
        if lx < 0 || ly < 0 {
            return false;
        }
        let (lx, ly) = (lx as usize, ly as usize);
        lx < self.w && ly < self.h && self.on.get(ly * self.w + lx).copied().unwrap_or(false)
    }

    /// Ink within `r` pixels of (`x`, `y`).
    fn near(&self, x: i64, y: i64, r: i64) -> bool {
        (-r..=r).any(|dy| (-r..=r).any(|dx| self.at(x + dx, y + dy)))
    }

    /// The region's ink rows: the first and last row holding at least two ink pixels, so a
    /// speck does not set the line's height.
    fn ink_rows(&self) -> Option<(i64, i64)> {
        let mut rows = (0..self.h).filter(|&y| self.on.get(y * self.w..(y + 1) * self.w).is_some_and(|row| row.iter().filter(|&&p| p).count() >= 2));
        let first = rows.next()?;
        let last = rows.next_back().unwrap_or(first);
        Some((self.top + first as i64, self.top + last as i64))
    }
}

/// One connected piece of ink (8-connected), by its box (inclusive, image coordinates).
#[derive(Clone, Debug)]
struct Component {
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
    pixels: usize,
}

impl Component {
    fn centre_x(&self) -> f32 {
        (self.left + self.right) as f32 / 2.0
    }

    fn width(&self) -> i64 {
        self.right - self.left + 1
    }

    fn height(&self) -> i64 {
        self.bottom - self.top + 1
    }
}

/// A glyph's ink: the union of some components, as a mask over their box.
#[derive(Debug)]
struct Glyph {
    left: i64,
    w: usize,
    h: usize,
    on: Vec<bool>,
}

impl Glyph {
    fn at(&self, x: usize, y: usize) -> bool {
        x < self.w && y < self.h && self.on.get(y * self.w + x).copied().unwrap_or(false)
    }

    fn near(&self, x: usize, y: usize, r: usize) -> bool {
        let (x0, y0) = (x.saturating_sub(r), y.saturating_sub(r));
        (y0..=y + r).any(|yy| (x0..=x + r).any(|xx| self.at(xx, yy)))
    }

    /// Ink pixels per row.
    fn row_counts(&self) -> Vec<usize> {
        (0..self.h).map(|y| (0..self.w).filter(|&x| self.at(x, y)).count()).collect()
    }

    /// Ink pixels per column, over rows `rows`.
    fn column_counts(&self, rows: std::ops::Range<usize>) -> Vec<usize> {
        (0..self.w).map(|x| rows.clone().filter(|&y| self.at(x, y)).count()).collect()
    }

    /// The leftmost and rightmost ink column over rows `rows`.
    fn extent(&self, rows: std::ops::Range<usize>) -> Option<(usize, usize)> {
        let columns = self.column_counts(rows);
        let first = columns.iter().position(|&n| n > 0)?;
        let last = columns.iter().rposition(|&n| n > 0)?;
        Some((first, last))
    }
}

/// A region's components, with every pixel labelled by the component it belongs to.
struct Shapes<'a> {
    ink: &'a Ink,
    labels: Vec<u32>,
    components: Vec<Component>,
    /// The line's ink rows within the region.
    line: (i64, i64),
}

impl<'a> Shapes<'a> {
    fn of(ink: &'a Ink) -> Option<Shapes<'a>> {
        let line = ink.ink_rows()?;
        let (w, h) = (ink.w, ink.h);
        let mut labels = vec![0u32; w * h];
        let mut components = Vec::new();
        let mut stack = Vec::new();
        for start in 0..w * h {
            if !ink.on.get(start).copied().unwrap_or(false) || labels.get(start).copied().unwrap_or(1) != 0 {
                continue;
            }
            let id = u32::try_from(components.len() + 1).ok()?;
            let mut c = Component { left: i64::MAX, top: i64::MAX, right: i64::MIN, bottom: i64::MIN, pixels: 0 };
            stack.push(start);
            if let Some(l) = labels.get_mut(start) {
                *l = id;
            }
            while let Some(p) = stack.pop() {
                let (x, y) = ((p % w) as i64, (p / w) as i64);
                c.left = c.left.min(x);
                c.right = c.right.max(x);
                c.top = c.top.min(y);
                c.bottom = c.bottom.max(y);
                c.pixels += 1;
                for (nx, ny) in neighbours(x, y) {
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let q = ny as usize * w + nx as usize;
                    if ink.on.get(q).copied().unwrap_or(false) && labels.get(q).copied().unwrap_or(1) == 0 {
                        if let Some(l) = labels.get_mut(q) {
                            *l = id;
                        }
                        stack.push(q);
                    }
                }
            }
            c.left += ink.left;
            c.right += ink.left;
            c.top += ink.top;
            c.bottom += ink.top;
            components.push(c);
        }
        Some(Shapes { ink, labels, components, line })
    }

    /// The largest component whose centre lies within `slack` of the character's columns:
    /// the glyph it was read from. Returns its index too.
    fn main_glyph(&self, c: ReadChar, slack: f32) -> Option<(usize, Glyph)> {
        let (lo, hi) = (c.left - slack, c.right + slack);
        let (id, _) = self.components.iter().enumerate().filter(|(_, k)| (lo..=hi).contains(&k.centre_x())).max_by_key(|(_, k)| k.pixels)?;
        Some((id, self.glyph(&[id])?))
    }

    /// The glyph read for the character together with every component stacked over or under
    /// it: the bar of a `±` is detached from its cross.
    fn stacked_glyph(&self, c: ReadChar, slack: f32) -> Option<(Vec<usize>, Glyph)> {
        let (main_id, _) = self.main_glyph(c, slack)?;
        let main = self.components.get(main_id)?;
        let reach = (self.line.1 - self.line.0) / 2;
        let ids: Vec<usize> = self
            .components
            .iter()
            .enumerate()
            .filter(|(id, k)| {
                let overlap = k.right.min(main.right) - k.left.max(main.left) + 1;
                let gap = (k.top - main.bottom).max(main.top - k.bottom);
                *id == main_id || (overlap * 2 >= k.width().min(main.width()) && gap <= reach)
            })
            .map(|(id, _)| id)
            .collect();
        Some((ids.clone(), self.glyph(&ids)?))
    }

    /// The nearest component that lies wholly to the right of `glyph`, within [`TAIL`] line
    /// heights of it and before the next word.
    fn ring_after(&self, glyph: &Glyph, next_left: Option<f32>) -> Option<Component> {
        let right = glyph.left + glyph.w as i64 - 1;
        let limit = right + ((self.line.1 - self.line.0) as f32 * TAIL) as i64;
        self.components
            .iter()
            .filter(|k| k.left > right && k.left <= limit && next_left.is_none_or(|n| (k.right as f32) < n))
            .min_by_key(|k| k.left)
            .cloned()
    }

    /// A small ring in the top half of the line with nothing under it: `°`.
    fn is_degree(&self, c: &Component) -> bool {
        let line_h = (self.line.1 - self.line.0 + 1) as f32;
        let (cw, ch) = (c.width() as f32, c.height() as f32);
        if ch < 4.0 || line_h < 8.0 || !(0.18..=0.55).contains(&(ch / line_h)) || !(0.6..=1.5).contains(&(cw / ch)) {
            return false;
        }
        if (c.top - self.line.0) as f32 > 0.3 * line_h || (c.bottom - self.line.0) as f32 > 0.65 * line_h {
            return false;
        }
        let (cx, cy) = ((c.left + c.right) / 2, (c.top + c.bottom) / 2);
        let ring = !self.ink.at(cx, cy)
            && self.ink.near(c.left, cy, 1)
            && self.ink.near(c.right, cy, 1)
            && self.ink.near(cx, c.top, 1)
            && self.ink.near(cx, c.bottom, 1);
        let under = (c.bottom + 1..=self.line.1).flat_map(|y| (c.left..=c.right).map(move |x| (x, y))).filter(|&(x, y)| self.ink.at(x, y)).count();
        ring && under * 20 <= ((self.line.1 - c.bottom).max(0) as usize + 1) * c.width() as usize
    }

    /// The mask of the components `ids`, over their joint box.
    fn glyph(&self, ids: &[usize]) -> Option<Glyph> {
        let boxes: Vec<&Component> = ids.iter().filter_map(|&id| self.components.get(id)).collect();
        let left = boxes.iter().map(|c| c.left).min()?;
        let top = boxes.iter().map(|c| c.top).min()?;
        let right = boxes.iter().map(|c| c.right).max()?;
        let bottom = boxes.iter().map(|c| c.bottom).max()?;
        let (w, h) = (usize::try_from(right - left + 1).ok()?, usize::try_from(bottom - top + 1).ok()?);
        let wanted: Vec<u32> = ids.iter().filter_map(|&id| u32::try_from(id + 1).ok()).collect();
        let on = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let (ix, iy) = (left + x as i64 - self.ink.left, top + y as i64 - self.ink.top);
                if ix < 0 || iy < 0 || ix >= self.ink.w as i64 {
                    return false;
                }
                self.labels.get(iy as usize * self.ink.w + ix as usize).is_some_and(|l| wanted.contains(l))
            })
            .collect();
        Some(Glyph { left, w, h, on })
    }
}

fn neighbours(x: i64, y: i64) -> [(i64, i64); 8] {
    [(x - 1, y - 1), (x, y - 1), (x + 1, y - 1), (x - 1, y), (x + 1, y), (x - 1, y + 1), (x, y + 1), (x + 1, y + 1)]
}

/// A centred cross with a detached bar under it, as wide as the crossbar: `±`.
fn is_plus_minus(g: &Glyph) -> bool {
    let (w, h) = (g.w, g.h);
    if w < 5 || h < 7 {
        return false;
    }
    let rows = g.row_counts();
    let wide = |n: usize| n * 2 >= w;
    // A gap row holds an anti-aliased edge at most, never a stem.
    let sparse = |n: usize| n * 10 <= w;
    // The bar: wide rows at the bottom; then a gap; then the cross.
    let bar = rows.iter().rev().take_while(|&&n| wide(n)).count();
    if bar == 0 || bar * 100 > h * 35 {
        return false;
    }
    let gap = rows[..h - bar].iter().rev().take_while(|&&n| sparse(n)).count();
    if gap == 0 || gap * 2 > h {
        return false;
    }
    let cross = h - bar - gap;
    if cross * 5 < h * 2 {
        return false;
    }
    let Some(crossbar) = centred_band(&rows[..cross], wide, cross) else { return false };
    let columns = g.column_counts(0..cross);
    let Some(stem) = centred_band(&columns, |n| n * 10 >= cross * 6, w) else { return false };
    // Little ink off the cross's two strokes (an anti-aliased edge, not a curl or a serif).
    let stray = (0..cross)
        .filter(|y| !crossbar.contains(y))
        .flat_map(|y| (0..w).filter(|x| !stem.contains(x)).map(move |x| (x, y)))
        .filter(|&(x, y)| g.at(x, y))
        .count();
    if stray * 12 > w * cross {
        return false;
    }
    match (g.extent(crossbar), g.extent(h - bar..h)) {
        (Some((l0, r0)), Some((l1, r1))) => {
            let (crossbar_w, bar_w) = ((r0 - l0 + 1) as f32, (r1 - l1 + 1) as f32);
            (0.6..=1.5).contains(&(bar_w / crossbar_w))
        }
        _ => false,
    }
}

/// The one contiguous run of `profile` entries that `is` selects, when it sits in the middle
/// (its centre in the middle 40 % of `len`) and takes at most half of `len`.
fn centred_band(profile: &[usize], is: impl Fn(usize) -> bool, len: usize) -> Option<std::ops::Range<usize>> {
    let first = profile.iter().position(|&n| is(n))?;
    let last = profile.iter().rposition(|&n| is(n))?;
    let contiguous = profile[first..=last].iter().all(|&n| is(n));
    let size = last - first + 1;
    let centre = (first + last) as f32 / 2.0;
    (contiguous && size * 2 <= len && (0.3 * len as f32..=0.7 * len as f32).contains(&centre)).then_some(first..last + 1)
}

/// A ring with one straight stroke across it from lower left to upper right, reaching past the
/// ring at both ends: `Ø`. A zero, a slashed zero (its stroke stays inside the ring, so the
/// box's corners are blank) and an eight (round corners) all fail the stroke; an at sign fails
/// the ring-hole-stroke-hole-ring order along the other diagonal.
fn is_diameter(g: &Glyph) -> bool {
    let (w, h) = (g.w, g.h);
    // Under 14 pixels wide, a ring's blank corners are too small to tell from a stroke's tips.
    if w < 14 || h < 14 || h * 10 < w * 9 || h > w * 5 / 2 {
        return false;
    }
    const N: usize = 40;
    // The stroke is followed with some tolerance; the holes beside it are narrow, so the other
    // diagonal is read almost pixel by pixel.
    let r = (w.min(h) / 12).max(1);
    let fine = (w.min(h) / 24).max(1);
    let along = |t: usize| (t * (w - 1) / N, (h - 1) - t * (h - 1) / N);
    let stroke = (2..=N - 2).filter(|&t| g.near(along(t).0, along(t).1, r)).count();
    if stroke * 100 < (N - 3) * 85 {
        return false;
    }
    // The stroke's tips reach the box's corners, where a ring (a zero, slashed or not) leaves
    // them blank: the inscribed ellipse starts about 15 % of the way in.
    let reaches = |ts: &mut dyn Iterator<Item = usize>| ts.take(N * 8 / 100 + 1).any(|t| g.near(along(t).0, along(t).1, fine));
    if !reaches(&mut (0..=N)) || !reaches(&mut (0..=N).rev()) {
        return false;
    }
    let samples = (0..=N).map(|t| g.near(t * (w - 1) / N, t * (h - 1) / N, fine));
    let mut runs = runs(samples);
    while runs.first() == Some(&false) {
        runs.remove(0);
    }
    while runs.last() == Some(&false) {
        runs.pop();
    }
    runs == [true, false, true, false, true]
}

/// The values of `samples` as runs, with runs shorter than two samples ignored.
fn runs(samples: impl Iterator<Item = bool>) -> Vec<bool> {
    let mut raw: Vec<(bool, usize)> = Vec::new();
    for s in samples {
        match raw.last_mut() {
            Some((v, n)) if *v == s => *n += 1,
            _ => raw.push((s, 1)),
        }
    }
    let mut out: Vec<bool> = Vec::new();
    for (v, _) in raw.into_iter().filter(|&(_, n)| n >= 2) {
        if out.last() != Some(&v) {
            out.push(v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A white page the tests draw glyphs on, in code: our own fixtures.
    struct Canvas {
        w: usize,
        h: usize,
        px: Vec<u8>,
    }

    impl Canvas {
        fn new(w: usize, h: usize) -> Canvas {
            Canvas { w, h, px: vec![255; w * h] }
        }

        fn gray(&self) -> impl Fn(i64, i64) -> u8 + '_ {
            move |x, y| {
                if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 { 255 } else { self.px[y as usize * self.w + x as usize] }
            }
        }

        fn rect(&mut self, x0: usize, y0: usize, x1: usize, y1: usize) {
            for y in y0..=y1 {
                for x in x0..=x1 {
                    self.px[y * self.w + x] = 0;
                }
            }
        }

        /// An elliptical ring: centre, radii, stroke thickness.
        fn ring(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, stroke: f32) {
            for y in 0..self.h {
                for x in 0..self.w {
                    let (dx, dy) = ((x as f32 + 0.5 - cx) / rx, (y as f32 + 0.5 - cy) / ry);
                    let d = (dx * dx + dy * dy).sqrt();
                    // A ring of the given thickness, scaled by the smaller radius.
                    if d <= 1.0 && d >= 1.0 - stroke / rx.min(ry) {
                        self.px[y * self.w + x] = 0;
                    }
                }
            }
        }

        /// A thick line segment.
        fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, thickness: f32) {
            let (vx, vy) = (x1 - x0, y1 - y0);
            let len2 = vx * vx + vy * vy;
            for y in 0..self.h {
                for x in 0..self.w {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let t = (((px - x0) * vx + (py - y0) * vy) / len2).clamp(0.0, 1.0);
                    let (qx, qy) = (x0 + t * vx - px, y0 + t * vy - py);
                    if (qx * qx + qy * qy).sqrt() <= thickness / 2.0 {
                        self.px[y * self.w + x] = 0;
                    }
                }
            }
        }
    }

    /// The glyphs, drawn in a 40-pixel cap height (a 10 pt face at 300 dpi) at `x`, baseline
    /// at `BASE`. Each returns the columns it covers.
    const BASE: usize = 50;
    const CAP: usize = 40;

    fn digit(c: &mut Canvas, x: usize) -> (f32, f32) {
        c.rect(x + 8, BASE - CAP, x + 13, BASE);
        c.rect(x + 2, BASE - 5, x + 20, BASE);
        (x as f32, (x + 22) as f32)
    }

    fn plus(c: &mut Canvas, x: usize, top: usize, bottom: usize) {
        let mid = (top + bottom) / 2;
        c.rect(x + 11, top, x + 15, bottom);
        c.rect(x + 1, mid - 2, x + 25, mid + 2);
    }

    fn plain_plus(c: &mut Canvas, x: usize) -> (f32, f32) {
        plus(c, x, BASE - 32, BASE - 6);
        (x as f32, (x + 27) as f32)
    }

    fn plus_minus(c: &mut Canvas, x: usize) -> (f32, f32) {
        plus(c, x, BASE - 36, BASE - 12);
        c.rect(x + 1, BASE - 4, x + 25, BASE);
        (x as f32, (x + 27) as f32)
    }

    /// A `t`: a stem left of centre with a short crossbar and a curl at the foot.
    fn letter_t(c: &mut Canvas, x: usize) -> (f32, f32) {
        c.rect(x + 6, BASE - 36, x + 10, BASE - 2);
        c.rect(x + 1, BASE - 26, x + 16, BASE - 23);
        c.rect(x + 9, BASE - 3, x + 17, BASE);
        (x as f32, (x + 19) as f32)
    }

    fn zero(c: &mut Canvas, x: usize) -> (f32, f32) {
        c.ring(x as f32 + 15.0, BASE as f32 - 20.0, 13.0, 20.0, 4.5);
        (x as f32, (x + 31) as f32)
    }

    fn slashed_zero(c: &mut Canvas, x: usize) -> (f32, f32) {
        let cols = zero(c, x);
        c.line(x as f32 + 9.0, BASE as f32 - 8.0, x as f32 + 21.0, BASE as f32 - 32.0, 3.5);
        cols
    }

    fn diameter(c: &mut Canvas, x: usize) -> (f32, f32) {
        let cols = zero(c, x);
        c.line(x as f32 - 1.0, BASE as f32 + 3.0, x as f32 + 31.0, BASE as f32 - 43.0, 3.5);
        cols
    }

    fn eight(c: &mut Canvas, x: usize) -> (f32, f32) {
        c.ring(x as f32 + 14.0, BASE as f32 - 30.0, 11.0, 10.5, 4.0);
        c.ring(x as f32 + 14.0, BASE as f32 - 10.0, 12.0, 10.5, 4.0);
        (x as f32, (x + 29) as f32)
    }

    fn degree(c: &mut Canvas, x: usize) -> (f32, f32) {
        c.ring(x as f32 + 8.0, BASE as f32 - 33.0, 7.0, 7.0, 3.0);
        (x as f32, (x + 17) as f32)
    }

    /// An `o` on the baseline: the same ring, where a letter sits.
    fn letter_o(c: &mut Canvas, x: usize) -> (f32, f32) {
        c.ring(x as f32 + 12.0, BASE as f32 - 13.0, 11.0, 13.0, 4.0);
        (x as f32, (x + 25) as f32)
    }

    fn read(ch: char, cols: (f32, f32)) -> ReadChar {
        ReadChar { ch, left: cols.0, right: cols.1 }
    }

    fn word(c: &Canvas, chars: &[ReadChar], next: Option<(f32, bool)>) -> Option<Repair> {
        let top = (BASE - CAP) as f32;
        let bottom = (BASE + 1) as f32;
        repair_word(chars, top, bottom, next.map(|n| n.0), next.is_some_and(|n| n.1), &c.gray())
    }

    #[test]
    fn a_cross_over_a_detached_bar_is_plus_minus_but_a_plain_plus_or_a_t_is_not() {
        let mut c = Canvas::new(200, 70);
        let sign = plus_minus(&mut c, 10);
        let one = digit(&mut c, 50);
        let fixed = word(&c, &[read('+', sign), read('1', one)], None).expect("the ± is repaired");
        assert_eq!(fixed.text, "±1");
        // The decoder's other names for the same ink repair the same way.
        assert_eq!(word(&c, &[read('-', sign), read('1', one)], None).map(|r| r.text).as_deref(), Some("±1"));
        assert_eq!(word(&c, &[read('t', sign), read('1', one)], None).map(|r| r.text).as_deref(), Some("±1"));
        // The sign as a word of its own, before a word that starts with a digit.
        assert_eq!(word(&c, &[read('+', sign)], Some((50.0, true))).map(|r| r.text).as_deref(), Some("±"));
        // The decoder may split the one glyph over two steps, a `+` and a `-` side by side: the
        // leftover name goes with the symbol, whichever side it is on.
        let (left_half, right_half) = ((sign.0, sign.0 + 13.0), (sign.0 + 13.0, sign.1));
        let split = [read('+', left_half), read('-', right_half), read('1', one)];
        assert_eq!(word(&c, &split, None).map(|r| r.text).as_deref(), Some("±1"));
        let split = [read('-', left_half), read('+', right_half), read('1', one)];
        assert_eq!(word(&c, &split, None).map(|r| r.text).as_deref(), Some("±1"));

        let mut plain = Canvas::new(200, 70);
        let sign = plain_plus(&mut plain, 10);
        let one = digit(&mut plain, 50);
        assert_eq!(word(&plain, &[read('+', sign), read('1', one)], None), None, "a plus has no bar under it");

        let mut tee = Canvas::new(200, 70);
        let sign = letter_t(&mut tee, 10);
        let one = digit(&mut tee, 50);
        assert_eq!(word(&tee, &[read('t', sign), read('1', one)], None), None, "a t's stem is off centre and its foot is no bar");
    }

    #[test]
    fn a_ring_with_a_stroke_past_its_edges_is_a_diameter_sign_but_a_zero_slashed_zero_or_eight_is_not() {
        for (draw, expect) in [(diameter as fn(&mut Canvas, usize) -> (f32, f32), Some("Ø1")), (zero, None), (slashed_zero, None), (eight, None)] {
            let mut c = Canvas::new(200, 70);
            let ring = draw(&mut c, 10);
            let one = digit(&mut c, 60);
            for named in ['0', '8', '?'] {
                let got = word(&c, &[read(named, ring), read('1', one)], None).map(|r| r.text);
                assert_eq!(got.as_deref(), expect, "{named} read over this glyph");
            }
        }
    }

    #[test]
    fn a_diameter_sign_is_only_read_at_the_start_of_a_number() {
        let mut c = Canvas::new(200, 70);
        let two = digit(&mut c, 10);
        let ring = diameter(&mut c, 40);
        let one = digit(&mut c, 90);
        // Inside a number the ring is a digit the decoder named, whatever its ink.
        assert_eq!(word(&c, &[read('2', two), read('0', ring), read('1', one)], None), None);
    }

    #[test]
    fn a_small_raised_ring_after_a_digit_is_a_degree_sign_whether_read_as_a_mark_or_dropped() {
        let mut c = Canvas::new(200, 70);
        let two = digit(&mut c, 10);
        let ring = degree(&mut c, 36);
        // Read as a question mark over the ring's columns: renamed. Read as a zero, the same:
        // a digit at the end of a word may be the name for the ring after the digit before it.
        assert_eq!(word(&c, &[read('2', two), read('?', ring)], None).map(|r| r.text).as_deref(), Some("2°"));
        assert_eq!(word(&c, &[read('2', two), read('0', ring)], None).map(|r| r.text).as_deref(), Some("2°"));
        // A full-height zero after the digit is the digit the decoder named.
        let mut twenty = Canvas::new(200, 70);
        let tens = digit(&mut twenty, 10);
        let nought = zero(&mut twenty, 36);
        assert_eq!(word(&twenty, &[read('2', tens), read('0', nought)], None), None);
        // Dropped, the digit's columns running on to the line's end: appended, and the word
        // grows to the ring.
        let fixed = word(&c, &[read('2', (two.0, 120.0))], None).expect("the dropped ° is appended");
        assert_eq!(fixed.text, "2°");
        assert!(fixed.right.is_none(), "the word's columns already reached past the ring");
        let fixed = word(&c, &[read('2', two)], None).expect("the dropped ° is appended");
        assert!(fixed.right.is_some_and(|r| r > two.1 && r <= 60.0), "{fixed:?}: the word now ends at the ring");

        // The same ring on the baseline is a letter, and a ring past the next word's start is
        // that word's.
        let mut o = Canvas::new(200, 70);
        let two = digit(&mut o, 10);
        let ring = letter_o(&mut o, 36);
        assert_eq!(word(&o, &[read('2', two), read('o', ring)], None), None);
        assert_eq!(word(&c, &[read('2', two)], Some((36.0, false))), None);
    }

    /// `±1°` read as `+-1?`: the sign's leftover name goes, and the ring is still found after
    /// the digit, not after whichever character is now where the digit was.
    #[test]
    fn a_split_sign_and_a_degree_ring_in_one_word_are_both_repaired() {
        let mut c = Canvas::new(200, 70);
        let sign = plus_minus(&mut c, 10);
        let one = digit(&mut c, 50);
        let ring = degree(&mut c, 76);
        let (left_half, right_half) = ((sign.0, sign.0 + 13.0), (sign.0 + 13.0, sign.1));
        let split = [read('+', left_half), read('-', right_half), read('1', one), read('?', ring)];
        assert_eq!(word(&c, &split, None).map(|r| r.text).as_deref(), Some("±1°"));
        // Dropped altogether, the digit's columns running on: appended, and the word grows.
        let split = [read('+', left_half), read('-', right_half), read('1', one)];
        let fixed = word(&c, &split, None).expect("both repaired");
        assert_eq!(fixed.text, "±1°");
        assert!(fixed.right.is_some_and(|r| r > one.1), "{fixed:?}: the word now ends at the ring");
    }

    #[test]
    fn symbols_are_only_checked_next_to_digits() {
        let mut c = Canvas::new(200, 70);
        let sign = plus_minus(&mut c, 10);
        let ring = diameter(&mut c, 50);
        // A plus between words and a ring before a letter keep what the decoder named: no
        // pixel is read for them.
        assert_eq!(word(&c, &[read('+', sign)], Some((50.0, false))), None);
        assert_eq!(word(&c, &[read('0', ring), read('x', (90.0, 100.0))], None), None);
    }

    #[test]
    fn faint_ink_is_not_judged() {
        let mut c = Canvas::new(200, 70);
        let sign = plus_minus(&mut c, 10);
        let one = digit(&mut c, 50);
        for p in &mut c.px {
            *p = 255 - (255 - *p) / 8;
        }
        assert_eq!(word(&c, &[read('+', sign), read('1', one)], None), None);
    }
}
