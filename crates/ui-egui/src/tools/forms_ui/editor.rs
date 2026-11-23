//! The text session of a focused form field: a Text field or an editable combo box edited in place.
//!
//! The field's text is laid out here, upright in the field's box and turned onto the page by
//! [`TextFrame`], so the caret, the selection and the pointer follow the field's `/MK /R`, the
//! page's rotation and the view's. The framework's text editor takes the keys, the input method and
//! the selection through [`FieldHost`]; this module paints the paper and the text, and acts on how
//! the session ended.

use std::ops::Range;
use std::sync::Arc;

use craft::behavior::CanvasView;
use craft::behavior::kurbo::Point;
use craft::pages::ui::view::PageBody;
use craft::text::ui::editor::{CaretLine, EditKind, EditorEnd, EditorMode, HostLine, HostRun, TextEditor, TextEditorHost};
use craft::ui::contract::{Bind, Props as _, Show as _};
use egui::text::{CCursor, CharIndex, LayoutJob};
use egui::{Align, Color32, CornerRadius, FontId, Pos2, Rect, pos2, vec2};
use pdfcraft_engine::{Document, FormField, FormFieldKind, FormWidget, field_flags, form_appearance};
use pdfcraft_render::DocInfo;

use super::{FIELD_FILL, FOCUS_BLUE, Focus, TextFrame, commit, editable_combo, field_screen_font, focused_field, list_pick_edit, widget_rect};
use crate::canvas::{PageGeometry, PageSession};

/// Where a field's text is laid out: its frame on the page and the screen points per page point.
#[derive(Clone, Copy)]
struct Geometry {
    frame: TextFrame,
    zoom: f32,
}

/// One laid-out line of a field's text, in the galley's coordinates.
struct Row {
    /// The chars on the line, without a line break.
    chars: Range<usize>,
    /// The line's top.
    top: f32,
}

/// A field's text laid out upright in its box, for the pass.
struct FieldLayout {
    frame: TextFrame,
    galley: Arc<egui::Galley>,
    /// The laid-out lines; never empty.
    rows: Vec<Row>,
    row_height: f32,
    /// Where the galley's top-left corner is in the upright box.
    origin: Pos2,
    /// The width of a comb cell, for a comb field.
    cell: Option<f32>,
    font: f32,
    /// The text as drawn: bullets for a password.
    shown: String,
}

impl FieldLayout {
    /// Lay `text` out as the field's page appearance does: at the field's text size, aligned by `/Q`,
    /// wrapped at the box less its margins when the field has several lines, in cells for a comb.
    /// A line wider than the box, and lines below it, scroll to keep the char at `caret` in view.
    fn build(ctx: &egui::Context, f: &FormField, w: &FormWidget, geo: Geometry, text: &str, caret: usize) -> Self {
        let painter = egui::Painter::new(ctx.clone(), egui::LayerId::background(), Rect::EVERYTHING);
        let font = field_screen_font(&painter, f, w, text, geo.zoom);
        let multiline = f.has(field_flags::MULTILINE);
        let count = text.chars().count();
        let shown = if f.has(field_flags::PASSWORD) { "•".repeat(count) } else { text.to_owned() };
        let upright = geo.frame.upright;
        let inner_w = (upright.width() - 6.0 * geo.zoom).max(1.0);
        let inner_h = (upright.height() - 2.0).max(1.0);
        let font_id = FontId::proportional(font);
        let mut job = if multiline {
            LayoutJob::simple(shown.clone(), font_id, Color32::BLACK, inner_w)
        } else {
            LayoutJob::simple_singleline(shown.clone(), font_id, Color32::BLACK)
        };
        job.keep_trailing_whitespace = true;
        let (align, factor) = match f.quadding {
            1 => (Align::Center, 0.5),
            2 => (Align::Max, 1.0),
            _ => (Align::Min, 0.0),
        };
        if multiline {
            job.halign = align;
        }
        let galley = ctx.fonts_mut(|fonts| fonts.layout_job(job));

        // A line starts where the cursor placed on the next line is lower than the one before it.
        let chars: Vec<char> = text.chars().collect();
        let mut rows: Vec<Row> = Vec::new();
        for i in 0..=count {
            let top = galley.pos_from_cursor(CCursor { index: CharIndex(i), prefer_next_row: true }).min.y;
            match rows.last_mut() {
                Some(last) if (top - last.top).abs() < 0.5 => last.chars.end = i,
                _ => rows.push(Row { chars: i..i, top }),
            }
        }
        let starts: Vec<usize> = rows.iter().map(|r| r.chars.start).collect();
        for (row, next) in rows.iter_mut().zip(starts.iter().skip(1)) {
            // A hard break ends its line before the break; a soft wrap ends it where the next begins.
            row.chars.end = if next.checked_sub(1).and_then(|at| chars.get(at)) == Some(&'\n') { next - 1 } else { *next };
        }
        if let Some(last) = rows.last_mut() {
            last.chars.end = count;
        }
        let row_height = galley.pos_from_cursor(CCursor::new(0)).height().max(1.0);

        let cell = (f.is_comb()).then_some(f.max_len).flatten().filter(|n| *n > 0).map(|n| inner_w / n as f32);
        let mut origin = upright.min + vec2(3.0, 1.0);
        if cell.is_none() {
            let width = galley.rect.width();
            let caret_at = galley.pos_from_cursor(CCursor::new(caret.min(count)));
            if multiline {
                origin.y -= (caret_at.max.y - inner_h).max(0.0);
            } else {
                origin.x += ((inner_w - width) * factor).max(0.0) - (caret_at.max.x - inner_w).max(0.0);
            }
        }
        Self { frame: geo.frame, galley, rows, row_height, origin, cell, font, shown }
    }

    fn last_row(&self) -> usize {
        self.rows.len().saturating_sub(1)
    }

    /// The line `offset` starts (the later one at a soft wrap).
    fn row_of(&self, offset: usize) -> usize {
        self.rows.iter().rposition(|r| r.chars.start <= offset).unwrap_or(0)
    }

    /// The caret at `offset` on `row`, from its top to its bottom, in the upright box.
    fn caret_box(&self, offset: usize, row: usize) -> [Pos2; 2] {
        let row = row.min(self.last_row());
        if let Some(cell) = self.cell {
            let x = self.origin.x + offset as f32 * cell;
            return [pos2(x, self.origin.y), pos2(x, self.origin.y + self.row_height.max(self.font))];
        }
        let at = self.galley.pos_from_cursor(CCursor { index: CharIndex(offset), prefer_next_row: row == self.row_of(offset) });
        let bottom = at.max.y.max(at.min.y + self.font);
        let x = self.origin.x + at.min.x;
        [pos2(x, self.origin.y + at.min.y), pos2(x, self.origin.y + bottom)]
    }

    /// The x of `offset` on `row`, in the upright box.
    fn x_of(&self, offset: usize, row: usize) -> f32 {
        self.caret_box(offset, row)[0].x
    }

    /// The char offset nearest a point of the upright box, and its line.
    fn hit_box(&self, p: Pos2) -> (usize, usize) {
        let v = p - self.origin;
        let row = self.rows.iter().rposition(|r| r.top <= v.y).unwrap_or(0);
        let Some(line) = self.rows.get(row) else { return (0, 0) };
        if let Some(cell) = self.cell {
            let at = (v.x / cell).round().max(0.0) as usize;
            return (at.min(line.chars.end), row);
        }
        let at = self.galley.cursor_from_pos(vec2(v.x, line.top + self.row_height / 2.0)).index.0;
        (at.clamp(line.chars.start, line.chars.end), row)
    }

    /// The quadrilaterals covering `range`, one per line piece, in the upright box.
    fn quads(&self, range: Range<usize>) -> Vec<[Pos2; 4]> {
        let mut quads = Vec::new();
        for (index, row) in self.rows.iter().enumerate() {
            let (from, to) = (range.start.max(row.chars.start), range.end.min(row.chars.end));
            if from >= to {
                continue;
            }
            let (x0, x1) = (self.x_of(from, index), self.x_of(to, index));
            let (y0, y1) = (self.origin.y + row.top, self.origin.y + row.top + self.row_height);
            quads.push([pos2(x0, y0), pos2(x1, y0), pos2(x1, y1), pos2(x0, y1)]);
        }
        quads
    }

    /// Paint the text, upright in the box and turned onto the field.
    fn paint(&self, painter: &egui::Painter) {
        let Some(cell) = self.cell else {
            self.frame.paint_galley(painter, self.origin, self.galley.clone(), Color32::BLACK);
            return;
        };
        for (i, c) in self.shown.chars().enumerate() {
            let glyph = painter.layout_no_wrap(c.to_string(), FontId::proportional(self.font), Color32::BLACK);
            let x = self.origin.x + i as f32 * cell + (cell - glyph.size().x) / 2.0;
            self.frame.paint_galley(painter, pos2(x, self.origin.y), glyph, Color32::BLACK);
        }
    }
}

/// A focused field's draft as the framework's text editor sees it.
struct FieldHost<'a> {
    ctx: egui::Context,
    /// The canvas view that maps the box's screen points to the document units the editor works in.
    view: CanvasView,
    field: &'a FormField,
    widget: &'a FormWidget,
    geo: Geometry,
    /// How a field that does not scroll lays its text out: an edit keeps no more than fits.
    room: Option<form_appearance::TextBox>,
    /// The real draft, which a password field's layout shows as bullets.
    text: &'a mut String,
    /// The caret as the editor last had it, which the layout keeps in view.
    caret: usize,
    laid: Option<FieldLayout>,
}

impl FieldHost<'_> {
    /// Run `read` on the layout of the draft as it is and the canvas view.
    fn with<R>(&mut self, read: impl FnOnce(&FieldLayout, &CanvasView) -> R) -> R {
        let laid = self.laid.get_or_insert_with(|| FieldLayout::build(&self.ctx, self.field, self.widget, self.geo, self.text.as_str(), self.caret));
        read(laid, &self.view)
    }

    /// A point of the upright box, in document units.
    fn doc(laid: &FieldLayout, view: &CanvasView, p: Pos2) -> Point {
        view.to_doc(laid.frame.on_screen(p))
    }
}

impl TextEditorHost for FieldHost<'_> {
    fn text(&self) -> &str {
        self.text
    }

    fn replace(&mut self, range: Range<usize>, text: &str, _kind: EditKind) -> Option<usize> {
        let len = self.text.chars().count();
        if range.start > range.end || range.end > len {
            return None;
        }
        let byte = |s: &str, at: usize| s.char_indices().nth(at).map_or(s.len(), |(b, _)| b);
        // The framework drops a refused replace whole, so a paste keeps the part that fits here.
        let room = self.field.max_len.map_or(usize::MAX, |max| max.saturating_sub(len - range.len()));
        let insert: String = text.chars().take(room).collect();
        let before = self.text.clone();
        let mut after = before.clone();
        after.replace_range(byte(&before, range.start)..byte(&before, range.end), &insert);
        if after != before
            && let Some(room) = &self.room
        {
            after = room.fitted_edit(&before, &after).0;
        }
        if after == before && insert.chars().count() < text.chars().count() {
            return None;
        }
        // The editor puts the caret after the chars kept, not after the whole `text` it gave.
        let kept = after.chars().count().saturating_sub(len - range.len()).min(text.chars().count());
        *self.text = after;
        self.laid = None;
        Some(kept)
    }

    fn hit(&mut self, at: Point) -> (usize, usize) {
        self.with(|laid, view| laid.hit_box(laid.frame.in_box(view.to_screen(at))))
    }

    fn caret(&mut self, offset: usize, line: Option<usize>) -> Option<CaretLine> {
        self.with(|laid, view| {
            let [top, bottom] = laid.caret_box(offset, line.unwrap_or_else(|| laid.row_of(offset)));
            Some(CaretLine::new(Self::doc(laid, view, top), Self::doc(laid, view, bottom)))
        })
    }

    fn selection_quads(&mut self, range: Range<usize>) -> Vec<[Point; 4]> {
        self.with(|laid, view| laid.quads(range).into_iter().map(|q| q.map(|p| Self::doc(laid, view, p))).collect())
    }

    fn underline_segments(&mut self, range: Range<usize>) -> Vec<[Point; 2]> {
        self.with(|laid, view| {
            laid.quads(range).into_iter().map(|[_, _, right, left]| [Self::doc(laid, view, left), Self::doc(laid, view, right)]).collect()
        })
    }

    fn lines(&mut self) -> Vec<HostLine> {
        let text: Vec<char> = self.text.chars().collect();
        self.with(|laid, view| {
            laid.rows
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    let line: String = text.get(row.chars.clone()).unwrap_or_default().iter().collect();
                    let start = laid.x_of(row.chars.start, index);
                    let xs: Vec<f32> = row.chars.clone().chain([row.chars.end]).map(|at| laid.x_of(at, index)).collect();
                    let positions = xs.iter().take(row.chars.len()).map(|x| view.doc_len(x - start) as f32).collect();
                    let widths = xs.windows(2).map(|pair| view.doc_len(pair[1] - pair[0]) as f32).collect();
                    let (top, bottom) = (laid.origin.y + row.top, laid.origin.y + row.top + laid.row_height);
                    let corners = [pos2(start, top), pos2(laid.x_of(row.chars.end, index), bottom)].map(|p| Self::doc(laid, view, p));
                    let rect = Rect::from_two_pos(pos2(corners[0].x as f32, corners[0].y as f32), pos2(corners[1].x as f32, corners[1].y as f32));
                    let baseline = Self::doc(laid, view, pos2(start, top + laid.row_height * 0.8)).y as f32;
                    let run = HostRun::new(0..line.len(), false, positions, widths);
                    HostLine::new(line, row.chars.clone(), rect, baseline, vec![run])
                })
                .collect()
        })
    }

    fn line_range(&mut self, line: usize) -> Range<usize> {
        self.with(|laid, _| laid.rows.get(line.min(laid.last_row())).map_or(0..0, |row| row.chars.clone()))
    }

    fn line_step(&mut self, offset: usize, lines: i32) -> usize {
        self.with(|laid, _| {
            let row = laid.row_of(offset);
            let target = (row as i64 + i64::from(lines)).clamp(0, laid.last_row() as i64) as usize;
            let Some(line) = laid.rows.get(target) else { return offset };
            laid.hit_box(pos2(laid.x_of(offset, row), laid.origin.y + line.top + laid.row_height / 2.0)).0
        })
    }

    fn line_edge(&mut self, offset: usize, end: bool) -> usize {
        self.with(|laid, _| laid.rows.get(laid.row_of(offset)).map_or(offset, |row| if end { row.chars.end } else { row.chars.start }))
    }
}

/// The char offset in the focused field's draft under the screen point `at`: where a press on the
/// field puts the caret, and a drag from it moves it to. The field part captures the press, so the
/// editor never sees it; this is the one mapping from a point to the draft.
///
/// `at` is in the page's view space, as `PageXform::screen_to_view` gives it.
///
/// PLAN: takes the context as a leading argument because the layout needs the fonts; pdf-forms-w7's `press_offset` (`forms_ui.rs:384`) calls it without one and must pass its `ui.ctx()`.
pub(super) fn offset_at(
    ctx: &egui::Context,
    session: &PageSession,
    form: &[FormField],
    info: &DocInfo,
    field: usize,
    widget: usize,
    at: Pos2,
    geo: PageGeometry<'_>,
) -> usize {
    let Some(focus) = session.forms.focus.as_ref() else { return 0 };
    let Some(f) = form.get(field) else { return 0 };
    let Some(w) = f.widgets.get(widget) else { return 0 };
    let Some(page) = w.page else { return 0 };
    let Some(xf) = session.page_xform(page, geo) else { return 0 };
    let rect = widget_rect(&xf, info, page, w.rect);
    let frame = TextFrame::of(&xf, info, page, w, rect);
    let laid = FieldLayout::build(ctx, f, w, Geometry { frame, zoom: xf.zoom() }, &focus.text, focus.selection.caret);
    laid.hit_box(frame.in_box(at)).0
}

/// Show the session of the focused field for this pass: the editor over a Text field or an editable
/// combo box, the drop-down list of a combo box and the calendar of a date field. The session ends
/// by queuing its edit in `session.pending_edit` (a commit), by moving to another field, or by
/// dropping the draft.
///
/// Returns the editor's id when one was shown: it takes the key focus over when the focus arrives on
/// the field's page part (Tab, a click or a focus request), so the keys and typing reach the text.
pub(crate) fn show(
    ui: &mut egui::Ui,
    body: &PageBody<'_>,
    session: &mut PageSession,
    doc: &Document,
    form: &[FormField],
    info: &DocInfo,
    today: (i64, u32, u32),
    pages: PageGeometry<'_>,
) -> Option<egui::Id> {
    let focus = session.forms.focus.as_ref()?;
    let Some(f) = focused_field(focus, form) else {
        session.forms.focus = None;
        return None;
    };
    let widget = focus.widget;
    let w = f.widgets.get(widget)?;
    let page = w.page?;
    let xf = session.page_xform(page, pages)?;
    let rect = widget_rect(&xf, info, page, w.rect);
    let zoom = xf.zoom();
    let geo = Geometry { frame: TextFrame::of(&xf, info, page, w, rect), zoom };
    if f.read_only() || w.read_only {
        return None;
    }
    let combo = f.kind == FormFieldKind::Combo;
    let types = f.kind == FormFieldKind::Text || (combo && editable_combo(f));
    let mut ended = None;
    let mut shown = None;
    if types {
        let room = (f.kind == FormFieldKind::Text && f.has(field_flags::DO_NOT_SCROLL)).then(|| doc.field_text_box(&focus.name, widget)).flatten();
        let focus = session.forms.focus.as_mut()?;
        let Focus { text, selection, .. } = focus;
        let mut host = FieldHost { ctx: ui.ctx().clone(), view: body.view, field: f, widget: w, geo, room, text, caret: selection.caret, laid: None };
        // PLAN: paints through an egui `Painter` with black text, as pdf-forms-w7's `paint_page` does; both move to the `Draw` tokens and the field's `/DA` colour together.
        let painter = ui.painter_at(rect.intersect(body.rect));
        painter.rect_filled(rect, CornerRadius::ZERO, FIELD_FILL);
        // The field being typed in carries a blue border, as production draws it.
        painter.rect_stroke(rect, CornerRadius::ZERO, egui::Stroke::new(1.5, FOCUS_BLUE), egui::StrokeKind::Inside);
        host.with(|laid, _| laid.paint(&painter));
        let multiline = f.kind == FormFieldKind::Text && f.has(field_flags::MULTILINE);
        let editor = TextEditor::new(f.name.clone(), &mut host, Bind::bound(selection))
            .id_salt((f.obj, widget))
            .mode(if multiline { EditorMode::FieldLines } else { EditorMode::FieldLine })
            .esc_commits(false);
        let editor = if f.has(field_flags::PASSWORD) { editor.masked() } else { editor };
        let out = ui.push_id((&f.name, f.obj, widget), |ui| editor.show_at(ui, rect)).inner;
        ended = out.ended;
        shown = Some(out.response.id);
    }
    match ended {
        Some(EditorEnd::Commit) => commit(session, form),
        Some(EditorEnd::Next) => {
            session.pending_edit = super::tab_to_next_field(session, info, form, f, widget, true, pages);
            session.forms.refocus = true;
        }
        Some(EditorEnd::Previous) => {
            session.pending_edit = super::tab_to_next_field(session, info, form, f, widget, false, pages);
            session.forms.refocus = true;
        }
        Some(EditorEnd::Cancel) => session.forms.focus = None,
        // `EditorEnd` is non-exhaustive: an end this code does not know leaves the session open.
        Some(_) | None => {}
    }
    let Some(focus) = session.forms.focus.as_mut() else { return shown };
    let was_open = focus.list_open;
    // A list box is picked from in place on the page, so only a combo box has a drop-down list.
    let picked = if combo { super::choice_list(ui, f, rect, zoom, &mut focus.list_open) } else { None };
    let list_closed = combo && !types && was_open && !focus.list_open;
    let date = match &f.actions.format {
        pdfcraft_engine::form_scripts::Format::Date(fmt) if types => super::calendar(ui, rect, &focus.text, fmt, today, &mut focus.calendar_open),
        _ => None,
    };
    if let Some(i) = picked {
        session.pending_edit = Some(list_pick_edit(form, f, vec![i]));
        session.forms.focus = None;
    } else if list_closed {
        session.forms.focus = None;
    } else if let Some(text) = date {
        focus.text = text;
        commit(session, form);
    }
    shown
}
