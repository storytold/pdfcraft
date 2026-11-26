//! The in-place editor for text typed onto a page: the framework's text editor over a one-box host.

use std::ops::Range;

use craft::behavior::kurbo::Point;
use craft::pages::ui::PageBody;
use craft::text::ui::editor::{CaretLine, EditKind, EditorEnd, HostLine, HostRun, TextEditor, TextEditorHost};
use craft::ui::contract::{Bind, Props as _, Show as _};
use craft::ui::runtime::focus::FocusRequest;
use egui::{Color32, FontId, Pos2, Rect, Stroke, vec2};
use pdfcraft_engine::{BorderShape, Edit};
use pdfcraft_render::DocInfo;

use super::{TypedLook, typed_with};

use crate::canvas::PageSession;

/// Line height over font size, as the typewriter annotation lays its text out.
const LINE: f32 = 1.2;
const MARGIN_X: f32 = 4.0;
const MARGIN_Y: f32 = 2.0;

/// The text of a type box laid out in the page's proportional face. Geometry is in the screen
/// points of the pass, with the first line's top-left at `origin`.
struct TypeBoxHost<'a> {
    text: &'a mut String,
    ctx: &'a egui::Context,
    origin: Pos2,
    size: f32,
}

impl TypeBoxHost<'_> {
    fn width_of(&self, s: &str) -> f32 {
        let font = FontId::proportional(self.size);
        self.ctx.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font, Color32::BLACK).size().x)
    }

    fn line_height(&self) -> f32 {
        self.size * LINE
    }

    fn byte_of(&self, char_at: usize) -> usize {
        self.text.char_indices().nth(char_at).map_or(self.text.len(), |(b, _)| b)
    }

    /// The char ranges of the lines, without their breaks. At least one, possibly empty.
    fn line_ranges(&self) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        let mut start = 0;
        let mut at = 0;
        for c in self.text.chars() {
            if c == '\n' {
                ranges.push(start..at);
                start = at + 1;
            }
            at += 1;
        }
        ranges.push(start..at);
        ranges
    }

    fn slice(&self, chars: Range<usize>) -> &str {
        let (a, b) = (self.byte_of(chars.start), self.byte_of(chars.end));
        self.text.get(a..b).unwrap_or("")
    }

    /// Each char's start along its line, and the line's end: one more entry than chars.
    fn columns(&self, chars: Range<usize>) -> Vec<f32> {
        let line = self.slice(chars);
        let mut ends: Vec<usize> = line.char_indices().map(|(b, _)| b).collect();
        ends.push(line.len());
        ends.iter().map(|b| self.width_of(line.get(..*b).unwrap_or(""))).collect()
    }

    fn line_of(&self, offset: usize) -> usize {
        let ranges = self.line_ranges();
        ranges.iter().position(|r| offset <= r.end).unwrap_or(ranges.len().saturating_sub(1))
    }

    /// The offset on line `line` nearest `x` (screen points from the origin).
    fn nearest(&self, line: usize, x: f32) -> usize {
        let ranges = self.line_ranges();
        let Some(range) = ranges.get(line.min(ranges.len().saturating_sub(1))).cloned() else { return 0 };
        let columns = self.columns(range.clone());
        let at = columns.iter().enumerate().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map_or(0, |(i, _)| i);
        range.start + at
    }

    fn x_of(&self, offset: usize, line: usize) -> f32 {
        let ranges = self.line_ranges();
        let Some(range) = ranges.get(line).cloned() else { return 0.0 };
        let columns = self.columns(range.clone());
        columns.get(offset.saturating_sub(range.start)).copied().unwrap_or(0.0)
    }

    fn point(&self, x: f32, line: usize) -> Point {
        Point::new(f64::from(self.origin.x + x), f64::from(self.origin.y + line as f32 * self.line_height()))
    }

    /// The pieces of `range`, one per line it touches: (line, from x, to x).
    fn pieces(&self, range: Range<usize>) -> Vec<(usize, f32, f32)> {
        let mut out = Vec::new();
        for (line, r) in self.line_ranges().into_iter().enumerate() {
            let (from, to) = (range.start.max(r.start), range.end.min(r.end));
            if from <= to && (from < to || r.is_empty() && range.start <= r.start && range.end > r.start) {
                out.push((line, self.x_of(from, line), self.x_of(to, line)));
            }
        }
        out
    }
}

impl TextEditorHost for TypeBoxHost<'_> {
    fn text(&self) -> &str {
        self.text
    }

    fn replace(&mut self, range: Range<usize>, text: &str, _kind: EditKind) -> Option<usize> {
        let count = self.text.chars().count();
        let (start, end) = (range.start.min(count), range.end.clamp(range.start.min(count), count));
        let (a, b) = (self.byte_of(start), self.byte_of(end));
        self.text.replace_range(a..b, text);
        Some(text.chars().count())
    }

    fn hit(&mut self, at: Point) -> (usize, usize) {
        let last = self.line_ranges().len().saturating_sub(1);
        let row = ((at.y as f32 - self.origin.y) / self.line_height()).floor();
        let line = if row.is_finite() && row > 0.0 { (row as usize).min(last) } else { 0 };
        (self.nearest(line, at.x as f32 - self.origin.x), line)
    }

    fn caret(&mut self, offset: usize, line: Option<usize>) -> Option<CaretLine> {
        let line = line.unwrap_or_else(|| self.line_of(offset));
        let x = self.x_of(offset, line);
        let top = self.point(x, line);
        Some(CaretLine::new(top, Point::new(top.x, top.y + f64::from(self.line_height()))))
    }

    fn selection_quads(&mut self, range: Range<usize>) -> Vec<[Point; 4]> {
        let h = f64::from(self.line_height());
        self.pieces(range)
            .into_iter()
            .map(|(line, a, b)| {
                let (tl, tr) = (self.point(a, line), self.point(b, line));
                [tl, tr, Point::new(tr.x, tr.y + h), Point::new(tl.x, tl.y + h)]
            })
            .collect()
    }

    fn underline_segments(&mut self, range: Range<usize>) -> Vec<[Point; 2]> {
        let h = f64::from(self.line_height());
        self.pieces(range)
            .into_iter()
            .map(|(line, a, b)| {
                let (l, r) = (self.point(a, line), self.point(b, line));
                [Point::new(l.x, l.y + h), Point::new(r.x, r.y + h)]
            })
            .collect()
    }

    fn lines(&mut self) -> Vec<HostLine> {
        let (h, size) = (self.line_height(), self.size);
        self.line_ranges()
            .into_iter()
            .enumerate()
            .map(|(i, chars)| {
                let columns = self.columns(chars.clone());
                let widths: Vec<f32> = columns.windows(2).map(|w| w[1] - w[0]).collect();
                let positions: Vec<f32> = columns.iter().take(widths.len()).copied().collect();
                let text = self.slice(chars.clone()).to_string();
                let width = columns.last().copied().unwrap_or(0.0);
                let top = self.origin.y + i as f32 * h;
                let rect = Rect::from_min_size(Pos2::new(self.origin.x, top), vec2(width, h));
                let runs = vec![HostRun::new(0..text.len(), false, positions, widths)];
                HostLine::new(text, chars, rect, top + size, runs)
            })
            .collect()
    }

    fn line_range(&mut self, line: usize) -> Range<usize> {
        self.line_ranges().get(line).cloned().unwrap_or(0..0)
    }

    fn line_step(&mut self, offset: usize, lines: i32) -> usize {
        let from = self.line_of(offset);
        let last = self.line_ranges().len().saturating_sub(1) as i64;
        let to = (from as i64 + i64::from(lines)).clamp(0, last) as usize;
        let x = self.x_of(offset, from);
        self.nearest(to, x)
    }

    fn line_edge(&mut self, offset: usize, end: bool) -> usize {
        let range = self.line_range(self.line_of(offset));
        if end { range.end } else { range.start }
    }
}

/// The type box's editor over its page. Returns the edit once it is committed (Cmd+Enter, Tab,
/// or a press elsewhere on the canvas); Escape or an empty box drops it.
pub(crate) fn show(ui: &mut egui::Ui, body: &PageBody<'_>, session: &mut PageSession, info: &DocInfo, author: &str) -> Option<Edit> {
    let PageSession { fill_text, fill_selection, .. } = session;
    let tb = fill_text.as_mut()?;
    let placed = body.pages.iter().find(|p| p.page == tb.page)?;
    let page = info.pages.get(tb.page)?;
    let v = page.user_to_view(tb.at[0] as f32, tb.at[1] as f32);
    let corner = placed.rect.lerp_inside(vec2(v[0] / page.width.max(f32::EPSILON), v[1] / page.height.max(f32::EPSILON)));
    let zoom = placed.scale as f32;
    let size = (tb.size as f32 * zoom).max(8.0);
    let ctx = ui.ctx().clone();
    let mut host = TypeBoxHost { text: &mut tb.text, ctx: &ctx, origin: corner + vec2(MARGIN_X, MARGIN_Y), size };
    let natural = host.line_ranges().into_iter().map(|r| host.columns(r).last().copied().unwrap_or(0.0)).fold(0.0, f32::max);
    // Include the field padding and a pixel for layout rounding in the measured width.
    let width = (natural + 2.0 * MARGIN_X + 1.0).max(8.0 * tb.size as f32 * 0.6 * zoom).clamp(60.0, 600.0);
    let height = host.line_ranges().len() as f32 * host.line_height() + 2.0 * MARGIN_Y;
    let rect = Rect::from_min_size(corner, vec2(width, height));
    // A scrolled editor belongs to the page viewport, including its pointer hit area.
    let clip = ui.clip_rect();
    ui.set_clip_rect(clip.intersect(body.rect));
    // PLAN: the box and its border preview are document content on a page body; they go through the body's `Draw` as data colours once it hands one to the page code.
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(255, 255, 255, 230));
    let ink = crate::comments::color32(tb.color);
    for (i, r) in host.line_ranges().into_iter().enumerate() {
        let line = host.slice(r).to_string();
        painter.text(host.origin + vec2(0.0, i as f32 * host.line_height()), egui::Align2::LEFT_TOP, line, FontId::proportional(size), ink);
    }
    if host.text.is_empty() {
        painter.text(host.origin, egui::Align2::LEFT_TOP, tl!("Type text"), FontId::proportional(size), Color32::from_gray(150));
    }
    if tb.border.width > 0.0 {
        let w = (tb.border.width as f32 * zoom).max(1.0);
        let color = tb.border.color.map_or(Color32::BLACK, crate::comments::color32);
        match tb.border.shape {
            BorderShape::Rectangle => {
                painter.rect_stroke(rect, 0.0, Stroke::new(w, color), egui::StrokeKind::Outside);
            }
            // A circle around the text typed so far, as `typed` will draw it: the box sized to the
            // text, the border around that.
            BorderShape::Circle => {
                let (bw, size) = (tb.border.width, tb.size);
                let longest = pdfcraft_engine::annot_text::text_width(host.text.trim(), size);
                let (tw, th) = (longest + 8.0 + 2.0 * bw, size * 1.2 + 6.0 + 2.0 * bw);
                let o = BorderShape::Circle.around([0.0, 0.0, tw, th], bw, Some([longest, size * 1.2]));
                let centre = corner + vec2((tw / 2.0 - bw) as f32, (th / 2.0 - bw) as f32) * zoom;
                let radius = (o[2] - o[0]) as f32 / 2.0 * zoom - w / 2.0;
                painter.add(egui::Shape::circle_stroke(centre, radius, Stroke::new(w, color)));
            }
            // An oval around it, as it will be drawn (half the line outside).
            shape => {
                let b = rect.expand(w);
                let o = shape.around([b.min.x, b.min.y, b.max.x, b.max.y].map(f64::from), f64::from(w), None);
                let half = vec2((o[2] - o[0]) as f32 / 2.0 - w / 2.0, (o[3] - o[1]) as f32 / 2.0 - w / 2.0);
                painter.add(egui::Shape::ellipse_stroke(rect.center(), half, Stroke::new(w, color)));
            }
        }
    }
    // Escape cancels: the page's Escape belongs to the editor while a box is open.
    // A box still waiting for its focus was started in this pass by the press the editor also sees as a press on the
    // canvas background: that press is the one that opened it, not a click away from it.
    let fresh = std::mem::take(&mut tb.focus);
    let editor = TextEditor::new(tl!("Text to place"), &mut host, Bind::bound(fill_selection)).esc_commits(false);
    let out = if fresh { editor.request_focus(FocusRequest::Take) } else { editor }.show_at(ui, rect);
    ui.set_clip_rect(clip);
    match out.ended.filter(|end| !(fresh && *end == EditorEnd::Commit))? {
        EditorEnd::Cancel => {
            *fill_text = None;
            None
        }
        EditorEnd::Commit | EditorEnd::Next | EditorEnd::Previous => {
            let tb = fill_text.take()?;
            (!tb.text.trim().is_empty()).then(|| {
                let look = TypedLook { color: tb.color, border: tb.border };
                typed_with(tb.page, tb.at, &tb.text, tb.size, crate::comments::page_rotation(info, tb.page), author, look)
            })
        }
        // `EditorEnd` is non-exhaustive: an end this code does not know leaves the box open.
        _ => None,
    }
}
