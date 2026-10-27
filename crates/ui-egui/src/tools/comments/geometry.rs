use super::state::PageCx;
use super::*;

pub(super) fn is_markup(subtype: &str) -> bool {
    matches!(subtype, "Highlight" | "Underline" | "StrikeOut" | "Squiggly")
}

/// Rectangles, ovals, text boxes, stamps and drawings (drawn signatures among them). A callout's
/// `/Rect` also holds its leader line, so it only moves.
pub(super) fn resizable(a: &Annotation) -> bool {
    matches!(a.subtype.as_str(), "Square" | "Circle" | "FreeText" | "Stamp" | "Ink") && a.intent.as_deref() != Some("FreeTextCallout")
}

/// Lines, connected lines, polygons and clouds: their ends and corners have handles of their own
/// (measurements keep the points they measured, and a locked comment its shape).
pub(super) fn reshapable(a: &Annotation) -> bool {
    matches!(a.subtype.as_str(), "Line" | "PolyLine" | "Polygon")
        && !a.points.is_empty()
        && !a.locked
        && !a.intent.as_deref().is_some_and(|i| i.ends_with("Dimension"))
}

/// Whether `tool` picks up a line's end and corner handles: the Select and Fill & Sign tools.
/// The drawing tools draw from where they are pressed, so a new line starts at the end of the
/// one just drawn rather than dragging it.
pub(super) fn grabs_points(tool: QuickTool) -> bool {
    !matches!(tool, QuickTool::Comment(t) if t.draws() || t.clicks_points())
}

/// The point of `a` (an end or a corner) whose handle is at `p` on screen.
pub(super) fn point_handle(cx: &PageCx<'_>, a: &Annotation, p: Pos2) -> Option<usize> {
    a.points.iter().position(|q| cx.to_screen([f64::from(q[0]), f64::from(q[1])]).distance(p) <= 7.0)
}

/// `points` with point `i` moved to `to`; with Shift, held to the nearest 45° from the point
/// before it (a line's other end; the first corner turns about the second).
pub(crate) fn reshaped(points: &[[f64; 2]], i: usize, to: [f64; 2], shift: bool) -> Vec<[f64; 2]> {
    let mut out = points.to_vec();
    let anchor = if i == 0 { points.get(1) } else { points.get(i - 1) };
    let to = match anchor {
        Some(a) if shift => pdfcraft_engine::measure::snap::constrain(*a, to),
        _ => to,
    };
    if let Some(p) = out.get_mut(i) {
        *p = to;
    }
    out
}

pub(super) const HANDLES: [(i8, i8); 8] = [(-1, -1), (0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0)];

pub(super) fn handle_pos(r: Rect, (hx, hy): (i8, i8)) -> Pos2 {
    let x = match hx {
        -1 => r.left(),
        0 => r.center().x,
        _ => r.right(),
    };
    let y = match hy {
        -1 => r.top(),
        0 => r.center().y,
        _ => r.bottom(),
    };
    pos2(x, y)
}

/// The proportions a corner drag of `r` keeps: the box's own fixed ratio (an image signature's),
/// else `r`'s current one while Shift is held, else none.
pub(crate) fn corner_ratio(r: Rect, fixed: Option<f32>, shift: bool) -> Option<f32> {
    fixed.or_else(|| (shift && r.width() > 0.0 && r.height() > 0.0).then(|| r.width() / r.height()))
}

pub(crate) fn resized(r: Rect, (hx, hy): (i8, i8), d: egui::Vec2, aspect_ratio: Option<f32>) -> Rect {
    if hx != 0
        && hy != 0
        && let Some(ratio) = aspect_ratio.filter(|v| v.is_finite() && *v > 0.0)
    {
        // As in content_ui, the larger requested dimension drives proportional corner resizing.
        // Keep the opposite corner fixed and prevent crossing it from flipping the image.
        let width = (r.width() + f32::from(hx) * d.x).max((r.height() + f32::from(hy) * d.y) * ratio).max(4.0).max(4.0 * ratio);
        let anchor = handle_pos(r, (-hx, -hy));
        return Rect::from_two_pos(anchor, anchor + vec2(f32::from(hx) * width, f32::from(hy) * width / ratio));
    }
    let mut r = r;
    match hx {
        -1 => r.min.x += d.x,
        1 => r.max.x += d.x,
        _ => {}
    }
    match hy {
        -1 => r.min.y += d.y,
        1 => r.max.y += d.y,
        _ => {}
    }
    Rect::from_two_pos(r.min, r.max)
}

pub(super) fn clamp_to(r: Rect, p: Pos2) -> Pos2 {
    pos2(p.x.clamp(r.left(), r.right()), p.y.clamp(r.top(), r.bottom()))
}

/// The side of the caret's box, in user space units, away from text.
pub(super) const CARET_SIZE: f64 = 8.0;

/// [`caret_snap`] in the page's user space: the caret's tip and the side of its box.
///
/// The caret is drawn upright in user space, so it only snaps on an unrotated page, where up on
/// the page is up on screen; its size goes through the same mapping as the tip, so a page with
/// `/UserUnit` gets a caret of the same size relative to its text.
pub(crate) fn snapped_caret(page: &PageInfo, text: &PageText, vx: f32, vy: f32) -> Option<([f64; 2], f64)> {
    if !page.rotation.is_multiple_of(360) {
        return None;
    }
    let ([x, y], size) = caret_snap(text, vx, vy)?;
    let tip = page.view_to_user(x, y);
    let foot = page.view_to_user(x, y + size);
    let side = f64::from(tip[1] - foot[1]);
    (side.is_finite() && side > 0.0).then(|| ([f64::from(tip[0]), f64::from(tip[1])], side))
}

/// Insert text: where a click puts the caret's tip (view space) and how tall the caret is.
///
/// On upright, horizontal text the tip snaps to the nearer edge of the character under (or
/// next to) the click, on the line's baseline, and the caret's side is half the text's em size
/// (clamped to 4–36 view-space points; [`snapped_caret`] maps it to user space): the
/// proposal marks an insertion point in the text, as a proofreader's caret does. Elsewhere
/// (away from text, or on turned, sloped or vertical text) it returns `None` and the caret stays
/// at the click. A ligature is one glyph, so the caret goes before or after it, never inside.
pub(crate) fn caret_snap(text: &PageText, vx: f32, vy: f32) -> Option<([f32; 2], f32)> {
    let g = text.glyphs.get(text.nearest(vx, vy)?)?;
    if !(g.direction[0] > 0.0 && g.direction[1].abs() <= 0.02) {
        return None;
    }
    // Close enough to belong to this glyph: within half an em of its box.
    let [x0, y0, x1, y1] = g.rect;
    let slack = (y1 - y0) / 2.0;
    if !(slack.is_finite() && slack > 0.0) || vx < x0 - slack || vx > x1 + slack || vy < y0 - slack || vy > y1 + slack {
        return None;
    }
    // The em box runs from 0.2 em below the baseline to 0.8 em above it (`pdfcraft_render`'s
    // text extraction); `quad` is its bottom edge in the writing direction, then its top edge
    // backwards. Take the side of the box the click is nearer to: its foot and head.
    let [q0, q1, q2, q3] = g.quad;
    let (foot, head) = if vx < (q0[0] + q1[0]) / 2.0 { (q0, q3) } else { (q1, q2) };
    let em = (head[0] - foot[0]).hypot(head[1] - foot[1]);
    if !(em.is_finite() && em > 0.0) {
        return None;
    }
    let baseline = [foot[0] + 0.2 * (head[0] - foot[0]), foot[1] + 0.2 * (head[1] - foot[1])];
    Some((baseline, (em / 2.0).clamp(4.0, 36.0)))
}

/// A text box sized to its text (at most 300 pt wide), hanging from its top-left corner.
/// The box for `text` at `size` with its displayed top-left corner at `at`, on a page turned by
/// `rotation` (its `/Rotate`): on a quarter-turned page it is taller than wide in user space.
pub fn text_box_rect(at: [f64; 2], text: &str, size: f64, rotation: i64) -> [f64; 4] {
    use pdfcraft_engine::annot_text::{text_width, wrap};
    let pad = 2.0;
    let longest = text.lines().map(|l| text_width(l, size)).fold(0.0, f64::max);
    let w = (longest + 2.0 * pad + 4.0).clamp(40.0, 300.0);
    let lines = wrap(text, size, w - 2.0 * pad).len().max(1);
    let h = lines as f64 * size * 1.2 + 2.0 * pad + 2.0;
    pdfcraft_engine::text_box_at(rotation, at, w, h).unwrap_or([at[0], at[1] - h, at[0] + w, at[1]])
}

/// The box dragged between user-space points `a` and `b`: its displayed top-left corner (user
/// space) and its displayed width and height. Measured in the page's view space, so the view's
/// own rotation and zoom (View > Rotate View) do not turn or scale it.
pub(super) fn drawn_box(page: &PageInfo, a: [f64; 2], b: [f64; 2]) -> ([f64; 2], [f64; 2]) {
    let (a, b) = (page.user_to_view(a[0] as f32, a[1] as f32), page.user_to_view(b[0] as f32, b[1] as f32));
    let at = page.view_to_user(a[0].min(b[0]), a[1].min(b[1]));
    ([f64::from(at[0]), f64::from(at[1])], [f64::from((a[0] - b[0]).abs()), f64::from((a[1] - b[1]).abs())])
}

/// A text box dragged out `size` (displayed width and height) from its displayed top-left corner
/// `at`, with a border `border` wide: the text wraps to its width inside the border, as its
/// appearance lays it out. When the text needs more lines the box grows down, and moves up to stay
/// on the page; text that cannot fit even so keeps the drawn height and is clipped.
pub fn drawn_text_box_rect(page: &PageInfo, at: [f64; 2], size: [f64; 2], text: &str, font_size: f64, border: f64) -> [f64; 4] {
    use pdfcraft_engine::annot_text::wrap;
    let pad = 2.0 + border.max(0.0);
    let [w, drawn] = size;
    let lines = wrap(text, font_size, (w - 2.0 * pad).max(1.0)).len().max(1);
    let needed = lines as f64 * font_size * 1.2 + 2.0 * pad + 2.0;
    let [vx, vy] = page.user_to_view(at[0] as f32, at[1] as f32).map(f64::from);
    let bottom = f64::from(page.height);
    let (top, h) = if needed <= drawn || needed > bottom { (vy, drawn) } else { (vy.min(bottom - needed).max(0.0), needed) };
    let at = page.view_to_user(vx as f32, top as f32).map(f64::from);
    pdfcraft_engine::text_box_at(i64::from(page.rotation), at, w, h).unwrap_or([at[0], at[1] - h, at[0] + w, at[1]])
}

/// The `/Rotate` of `page` (0 when it is unknown).
pub(crate) fn page_rotation(info: &DocInfo, page: usize) -> i64 {
    info.pages.get(page).map_or(0, |p| i64::from(p.rotation))
}
