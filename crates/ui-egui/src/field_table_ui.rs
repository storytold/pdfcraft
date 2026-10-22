//! Prepare a form ▸ Text field table: a movable window that divides the area drawn on a page into
//! rows and columns of text fields, while the page shows the table as it changes. The table is
//! the engine's [`FieldTable`]; this is its window.
//!
//! The window is not modal, so that the page stays in view, but it belongs to one page of one
//! document as it was when the window opened: it closes when that document stops being the one
//! prepared or changes in any way (a page rotated, moved or deleted, an edit undone), and adds the
//! table there.

use std::ops::RangeInclusive;
use std::sync::Arc;

use egui::{Align, Color32, CornerRadius, Layout, Rect, RichText, Sense, Stroke, pos2, vec2};
use pdfcraft_engine::field_table::{self, Cell, FieldTable, MAX_COLUMNS, MAX_FIRST_ROW, MAX_ROWS, TableError};
use pdfcraft_engine::{DocId, Edit, FormField};
use pdfcraft_render::PageInfo;

use crate::canvas::PageXform;
use crate::{Dialog, PdfCraftApp, i18n, theme, widgets};

const SELECT_BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);
const ERROR_RED: Color32 = Color32::from_rgb(0xD1, 0x3B, 0x3B);
/// A divider can't be dragged to leave a column less than this share of the table, or less than a
/// quarter of what the two columns it divides have between them when that is smaller.
const MIN_SHARE: f64 = 0.04;

/// The table being set up in the window, and where it goes.
#[derive(Clone, Debug)]
pub struct FieldTableDraft {
    /// The document the table goes in.
    pub doc: DocId,
    /// The page it goes on, 0-based.
    pub page: usize,
    pub table: FieldTable,
    /// What added columns are named after: the first free "TableK" when the window opened.
    base: String,
    /// The document's edit generation when the window opened: any edit since closes it.
    generation: u64,
    /// The table as last laid out, kept until the table changes.
    laid: Option<Arc<Laid>>,
    /// A field in the window had the keyboard last frame, so Escape leaves the field.
    typing: bool,
}

/// Two drafts are the same table in the same place; what is kept for the next frame isn't part of it.
impl PartialEq for FieldTableDraft {
    fn eq(&self, other: &Self) -> bool {
        (self.doc, self.page, &self.table, &self.base, self.generation) == (other.doc, other.page, &other.table, &other.base, other.generation)
    }
}

/// A table laid out and checked once, for every frame it stays the same: the cells the page and the
/// window show, and the edit Add fields applies. Up to 100 × 50 cells, so not every frame.
#[derive(Debug)]
pub(crate) struct Laid {
    table: FieldTable,
    cells: Result<Vec<Cell>, TableError>,
    edit: Result<Edit, TableError>,
}

/// What the page shows of the table being set up: its cells, or why it can't be laid out.
pub(crate) struct Preview {
    page: usize,
    laid: Arc<Laid>,
}

/// How the window was left this frame.
enum Choice {
    Add,
    Cancel,
}

impl FieldTableDraft {
    /// A table filling `rect` (view space) on `page` of a document at edit `generation`, named after
    /// the first base `form` leaves free.
    pub(crate) fn new(doc: DocId, page: usize, rect: [f64; 4], form: &[FormField], generation: u64) -> Self {
        let base = field_table::free_base(form.iter().map(|f| f.name.as_str()));
        FieldTableDraft { doc, page, table: FieldTable::new(rect, &base), base, generation, laid: None, typing: false }
    }

    /// The table laid out on its page (`info`, in a form whose fields are `form`), worked out again
    /// only when the table has changed since. The form can't change under it: an edit closes the
    /// window.
    pub(crate) fn laid(&mut self, info: &PageInfo, form: &[FormField]) -> Arc<Laid> {
        if let Some(laid) = self.laid.as_ref().filter(|l| l.table == self.table) {
            return Arc::clone(laid);
        }
        let laid =
            Arc::new(Laid { table: self.table.clone(), cells: self.table.cells(), edit: field_table::edit(&self.table, self.page, info, form) });
        self.laid = Some(Arc::clone(&laid));
        laid
    }

    /// What the page shows of this table this frame.
    pub(crate) fn preview(&mut self, info: &PageInfo, form: &[FormField]) -> Preview {
        Preview { page: self.page, laid: self.laid(info, form) }
    }

    /// Each column's share of the table's width, together 1.
    fn shares(&self) -> Vec<f64> {
        let total: f64 = self.table.columns.iter().map(|c| c.width).sum();
        let n = self.table.columns.len().max(1) as f64;
        self.table.columns.iter().map(|c| if total > 0.0 && total.is_finite() { c.width / total } else { 1.0 / n }).collect()
    }

    /// Move the divider after column `i` (0-based) by `delta`, a share of the table's width. The
    /// two columns it divides keep [`MIN_SHARE`] each, or a quarter of what they have between them
    /// when that is less, so narrow columns can still be resized; the other columns keep theirs.
    fn move_divider(&mut self, i: usize, delta: f64) {
        let total: f64 = self.table.columns.iter().map(|c| c.width).sum();
        let (Some(a), Some(b)) = (self.table.columns.get(i).map(|c| c.width), self.table.columns.get(i + 1).map(|c| c.width)) else { return };
        if !(total > 0.0 && total.is_finite() && delta.is_finite()) {
            return;
        }
        let pair = (a + b) / total;
        // At most half the pair, so the bounds never cross; `max` then `min` all the same, never
        // `clamp`, which panics if they do.
        let least = MIN_SHARE.min(pair / 4.0);
        let left = (a / total + delta).max(least).min(pair - least);
        if let Some(c) = self.table.columns.get_mut(i) {
            c.width = left * total;
        }
        if let Some(c) = self.table.columns.get_mut(i + 1) {
            c.width = (pair - left) * total;
        }
    }

    /// Give every column the same width: their widths are shares, so the same number does it.
    fn distribute_evenly(&mut self) {
        for c in &mut self.table.columns {
            c.width = 1.0;
        }
    }
}

/// Show the window while [`Dialog::FieldTable`] is open, and add the table when asked to.
pub(crate) fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    // The window belongs to one page of one document, while that document is being prepared.
    let active = app.active_ids().map(|(_, id)| id);
    let preparing = app.is_preparing();
    let belongs = app.field_table_draft.as_ref().is_some_and(|d| {
        active == Some(d.doc)
            && preparing
            && app
                .session
                .get(d.doc)
                .is_some_and(|doc| doc.edit_generation() == d.generation && d.page < doc.info.pages.len() && doc.allows_modification())
    });
    if !belongs {
        close(app);
        return;
    }
    // Both there, as `belongs` says; the fields are borrowed apart, so the window can change the
    // table while it reads the document.
    let Some(draft) = app.field_table_draft.as_mut() else { return };
    let Some((info, form)) = app.session.get(draft.doc).and_then(|doc| Some((doc.info.pages.get(draft.page)?, &doc.form))) else { return };
    let t = theme::Tokens::get(ctx);
    let mut open = true;
    let mut choice = None;
    let mut laid = None;
    egui::Window::new(tl!("Text Field Table"))
        .id(egui::Id::new("field_table_window"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .min_width(440.0)
        .max_width(440.0)
        .default_pos(egui::pos2(120.0, 100.0))
        .show(ctx, |ui| {
            widgets::begin_dialog(ui);
            let (c, l) = body(ui, draft, info, form, &t);
            choice = c;
            laid = Some(l);
        });
    // Escape closes it, as it closes a modal dialog, unless a field had the keyboard: then Escape
    // only leaves the field, and what was set up stays. Enter is left to the field being typed in.
    let escape = !draft.typing && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    draft.typing = ctx.egui_wants_keyboard_input();
    match choice {
        _ if !open || escape => close(app),
        Some(Choice::Cancel) => close(app),
        // Added, the window closes; refused, it stays as it is, with the notice saying why.
        Some(Choice::Add) => {
            if let Some(Ok(edit)) = laid.as_deref().map(|l| l.edit.clone())
                && app.apply_edit(edit)
            {
                close(app);
            }
        }
        None => {}
    }
}

/// Drop the table being set up, and the dialog with it if it is still the open one.
fn close(app: &mut PdfCraftApp) {
    app.field_table_draft = None;
    if app.dialog == Some(Dialog::FieldTable) {
        app.dialog = None;
    }
}

/// The window's controls, the names the table will have, and why it can't be added if it can't.
/// Returns what was chosen and the table as it now stands, laid out and checked.
fn body(ui: &mut egui::Ui, d: &mut FieldTableDraft, info: &PageInfo, form: &[FormField], t: &theme::Tokens) -> (Option<Choice>, Arc<Laid>) {
    let columns = d.table.columns.len();
    egui::Grid::new("field_table_grid").num_columns(2).spacing([14.0, 8.0]).min_col_width(120.0).show(ui, |ui| {
        ui.label(tl!("Rows"));
        stepper(ui, &mut d.table.rows, 1..=MAX_ROWS, 1.0, "");
        ui.end_row();
        ui.label(tl!("Columns"));
        let mut n = columns;
        if stepper(ui, &mut n, 1..=MAX_COLUMNS, 1.0, "") {
            d.table.set_columns(n, &d.base);
        }
        ui.end_row();
        ui.label(tl!("Row spacing"));
        stepper(ui, &mut d.table.row_gap, 0.0..=100.0, 1.0, " pt");
        ui.end_row();
        ui.label(tl!("Column spacing"));
        ui.add_enabled_ui(columns > 1, |ui| stepper(ui, &mut d.table.column_gap, 0.0..=100.0, 1.0, " pt"));
        ui.end_row();
        ui.label(tl!("First row number"));
        ui.add(egui::DragValue::new(&mut d.table.first_row).range(0..=MAX_FIRST_ROW));
        ui.end_row();
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(tl!("Column widths")).strong());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled_ui(d.table.columns.len() > 1, |ui| {
                if widgets::pill_button(ui, tl!("Distribute evenly"), false).clicked() {
                    d.distribute_evenly();
                }
            });
        });
    });
    ui.add_space(4.0);
    column_bar(ui, d, t);

    ui.add_space(8.0);
    ui.label(RichText::new(tl!("Field names")).strong());
    // How the names are worked out. Its tokens are the same in every language.
    let hint = tl!("N or {row} is the row number, {col} the column number, {seq} the cell number. With none of them, the row number goes last.");
    ui.add(egui::Label::new(RichText::new(hint).color(t.text_muted).size(11.0)).wrap());
    ui.add_space(4.0);
    // The widths shown don't depend on the names being typed below.
    let layout = d.laid(info, form);
    let shares = d.shares();
    let scrolls = d.table.columns.len() > 3;
    let mut column_fields = |ui: &mut egui::Ui| {
        for (c, column) in d.table.columns.iter_mut().enumerate() {
            let width = layout.cells.as_ref().ok().and_then(|cells| cells.get(c)).map(|cell| cell.rect[2] - cell.rect[0]);
            let share = shares.get(c).copied().unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(RichText::new(column_label(c)).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let size = width.map_or_else(|| "—".to_owned(), |w| format!("{w:.1} pt · {:.0}%", share * 100.0));
                    ui.label(RichText::new(size).color(t.text_muted).size(11.0));
                });
            });
            let full = ui.available_width();
            ui.add(widgets::line(&mut column.pattern).desired_width(full).id_salt(("field table column", c)));
            ui.add_space(4.0);
        }
    };
    if scrolls {
        egui::ScrollArea::vertical().max_height(180.0).show(ui, column_fields);
    } else {
        column_fields(ui);
    }

    ui.add_space(8.0);
    // The names, and the edit Add fields applies, from the table as it now stands.
    let laid = d.laid(info, form);
    if let Ok(cells) = &laid.cells {
        let names: Vec<&str> = cells.iter().map(|c| c.name.as_str()).collect();
        let shown = match names.as_slice() {
            [] => String::new(),
            [one] => (*one).to_owned(),
            [first, second] => format!("{first}, {second}"),
            [first, second, .., last] => format!("{first}, {second} … {last}"),
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(i18n::trn(i18n::current(), names.len() as u64, "{n} field", "{n} fields")).color(t.text_muted));
            ui.add(egui::Label::new(RichText::new(shown).font(theme::medium(12.0))).truncate());
        });
    }
    if let Err(e) = &laid.edit {
        ui.add(egui::Label::new(RichText::new(e.said(&i18n::words)).color(ERROR_RED).size(11.0)).wrap());
    }

    ui.add_space(14.0);
    let mut choice = None;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui.add_enabled_ui(laid.edit.is_ok(), |ui| widgets::pill_button(ui, tl!("Add fields"), true)).inner.clicked() {
            choice = Some(Choice::Add);
        }
        if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
            choice = Some(Choice::Cancel);
        }
    });
    (choice, laid)
}

/// "Column 2" for the column at 0-based `index`.
fn column_label(index: usize) -> String {
    i18n::fmt(tl!("Column {n}"), &[("n", &index.saturating_add(1).to_string())])
}

/// A number with minus and plus buttons either side, held to `range`, moving by `step`. Whether it
/// changed.
fn stepper<N: egui::emath::Numeric>(ui: &mut egui::Ui, value: &mut N, range: RangeInclusive<N>, step: f64, suffix: &str) -> bool {
    let before = *value;
    let (low, high) = (range.start().to_f64(), range.end().to_f64());
    ui.horizontal(|ui| {
        if ui.button("-").clicked() {
            *value = N::from_f64((value.to_f64() - step).max(low));
        }
        ui.add(egui::DragValue::new(value).range(range).speed(step / 2.0).suffix(suffix));
        if ui.button("+").clicked() {
            *value = N::from_f64((value.to_f64() + step).min(high));
        }
    });
    *value != before
}

/// The columns as a bar with a divider between each two, dragged (or moved with the arrow keys once
/// focused) to share the width differently.
fn column_bar(ui: &mut egui::Ui, d: &mut FieldTableDraft, t: &theme::Tokens) {
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width().max(100.0), 28.0), Sense::hover());
    let edges = |shares: &[f64]| -> Vec<f32> {
        let mut x = bar.left();
        shares
            .iter()
            .map(|s| {
                x += *s as f32 * bar.width();
                x
            })
            .collect()
    };
    let n = d.table.columns.len();
    // Wide enough to grab, never reaching past the middle of a column.
    let reach = (bar.width() / n.max(1) as f32 / 2.0).clamp(2.0, 8.0);
    let mut active = None;
    let ends = edges(&d.shares());
    for (i, &x) in ends.iter().enumerate().take(n.saturating_sub(1)) {
        let hit = Rect::from_min_max(pos2(x - reach, bar.top()), pos2(x + reach, bar.bottom()));
        let resp = ui.interact(hit, ui.id().with(("field table divider", i)), Sense::click_and_drag());
        let share = d.shares().get(i).copied().unwrap_or_default();
        resp.widget_info(|| egui::WidgetInfo::slider(true, (share * 100.0).round(), column_label(i)));
        let mut delta = f64::from(resp.drag_delta().x) / f64::from(bar.width());
        if resp.has_focus() {
            ui.input_mut(|input| {
                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                    delta -= 0.01;
                }
                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                    delta += 0.01;
                }
            });
        }
        if delta != 0.0 {
            d.move_divider(i, delta);
        }
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if resp.hovered() || resp.dragged() || resp.has_focus() {
            active = Some(i);
        }
    }

    let painter = ui.painter_at(bar);
    let shares = d.shares();
    let mut left = bar.left();
    for (c, (&right, share)) in edges(&shares).iter().zip(&shares).enumerate() {
        // The last column ends at the bar's edge, whatever rounding says.
        let right = if c + 1 == n { bar.right() } else { right.min(bar.right()) };
        let column = Rect::from_min_max(pos2(left, bar.top()), pos2(right, bar.bottom()));
        painter.rect_filled(column, CornerRadius::ZERO, SELECT_BLUE.gamma_multiply(if c % 2 == 0 { 0.11 } else { 0.055 }));
        if column.width() > 32.0 {
            painter.text(column.center(), egui::Align2::CENTER_CENTER, format!("{:.0}%", share * 100.0), theme::medium(11.0), t.text);
        }
        left = right;
    }
    painter.rect_stroke(bar, CornerRadius::same(3), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
    for (i, &x) in edges(&shares).iter().enumerate().take(n.saturating_sub(1)) {
        let stroke = if active == Some(i) { Stroke::new(3.0, SELECT_BLUE) } else { Stroke::new(1.5, t.border) };
        painter.line_segment([pos2(x, bar.top() + 1.0), pos2(x, bar.bottom() - 1.0)], stroke);
    }
}

/// The table being set up, on its page: each cell with its name, or the table's outline in red
/// with the reason it can't be laid out. Only the cells in view are drawn, and only those with room
/// for it get their name: a table can have 5,000 cells.
pub(crate) fn paint_preview(painter: &egui::Painter, xf: &PageXform, page: usize, preview: &Preview) {
    if preview.page != page {
        return;
    }
    let zoom = (xf.rect.width() / xf.pw.max(1.0)).clamp(0.5, 3.0);
    let font = theme::regular((9.0 * zoom).clamp(7.0, 13.0));
    let on_screen = |r: [f64; 4]| xf.view_rect([r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32]);
    let outline = on_screen(preview.laid.table.rect);
    let visible = painter.clip_rect();
    // A tag of white `text` in the middle of `area`, kept inside it.
    let tag = |area: Rect, text: String, fill: Color32| {
        let galley = painter.layout_no_wrap(text, font.clone(), Color32::WHITE);
        let tag = Rect::from_center_size(area.center(), galley.size() + vec2(6.0, 2.0));
        let inside = painter.with_clip_rect(painter.clip_rect().intersect(area));
        inside.rect_filled(tag, CornerRadius::same(1), fill);
        inside.galley(tag.min + vec2(3.0, 1.0), galley, Color32::WHITE);
    };
    match &preview.laid.cells {
        Ok(cells) => {
            painter.rect_stroke(outline, CornerRadius::ZERO, Stroke::new(1.0, SELECT_BLUE.gamma_multiply(0.3)), egui::StrokeKind::Outside);
            for cell in cells {
                let r = on_screen(cell.rect);
                if !visible.intersects(r) {
                    continue;
                }
                painter.rect_filled(r, CornerRadius::ZERO, crate::prepare::FIELD_FILL);
                painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.5, SELECT_BLUE), egui::StrokeKind::Inside);
                if r.height() >= font.size + 2.0 && r.width() >= 24.0 {
                    tag(r, cell.name.clone(), SELECT_BLUE);
                }
            }
        }
        Err(e) => {
            painter.rect_stroke(outline, CornerRadius::ZERO, Stroke::new(1.5, ERROR_RED), egui::StrokeKind::Inside);
            tag(outline, e.said(&i18n::words), ERROR_RED);
        }
    }
}

#[cfg(test)]
mod tests {
    use pdfcraft_engine::field_table::TableColumn;

    use super::*;

    fn draft(columns: usize) -> FieldTableDraft {
        let mut d = FieldTableDraft::new(DocId(1), 0, [0.0, 0.0, 400.0, 200.0], &[], 0);
        d.table.set_columns(columns, &d.base);
        d
    }

    /// Thirty even columns leave each pair less than twice the least share a divider keeps: the
    /// two bounds crossed, and `f64::clamp` panicked on the first drag.
    #[test]
    fn dragging_a_divider_between_narrow_columns_never_panics() {
        for columns in [2, 25, 26, 30, MAX_COLUMNS] {
            for delta in [-1.0, -0.01, 0.0, 0.01, 1.0, f64::NAN, f64::INFINITY] {
                let mut d = draft(columns);
                let before: f64 = d.table.columns.iter().map(|c| c.width).sum();
                d.move_divider(0, delta);
                d.move_divider(columns - 2, delta);
                let after: f64 = d.table.columns.iter().map(|c| c.width).sum();
                assert!((after - before).abs() < 1e-9, "{columns} columns, {delta}: the total width stays");
                assert!(d.table.columns.iter().all(|c| c.width > 0.0), "{columns} columns, {delta}: no column vanishes");
            }
            // And the divider still moves.
            let mut d = draft(columns);
            d.move_divider(0, -0.01);
            assert!(d.table.columns[0].width < d.table.columns[1].width, "{columns} columns: {:?}", d.shares());
        }
    }

    #[test]
    fn a_divider_moves_only_the_two_columns_it_divides() {
        let mut d = draft(3);
        d.move_divider(0, 0.1);
        let shares = d.shares();
        assert!((shares[0] - (1.0 / 3.0 + 0.1)).abs() < 1e-9 && (shares[1] - (1.0 / 3.0 - 0.1)).abs() < 1e-9, "{shares:?}");
        assert!((shares[2] - 1.0 / 3.0).abs() < 1e-9);
        // Dragged past its neighbour, a column stops at the least share; past the end, nothing.
        d.move_divider(0, 1.0);
        assert!((d.shares()[1] - MIN_SHARE).abs() < 1e-9);
        let before = d.clone();
        d.move_divider(2, 0.5);
        assert_eq!(d, before);
        d.distribute_evenly();
        assert!(d.table.columns.iter().all(|c| c.width == 1.0));
    }

    /// The window and the page share one layout of the table, worked out again only when the table
    /// changes: at 100 × 50 cells, laying it out several times a frame was 25,000 names a frame.
    #[test]
    fn the_layout_is_worked_out_again_only_when_the_table_changes() {
        let info = PageInfo { width: 400.0, height: 300.0, label: "1".into(), crop: [0.0, 0.0, 400.0, 300.0], rotation: 0 };
        let mut d = draft(2);
        let first = d.laid(&info, &[]);
        assert!(Arc::ptr_eq(&first, &d.laid(&info, &[])), "the same table, the same layout");
        assert!(Arc::ptr_eq(&first, &d.preview(&info, &[]).laid), "the page shares it");
        d.table.rows = 5;
        let second = d.laid(&info, &[]);
        assert!(!Arc::ptr_eq(&first, &second), "a changed table is laid out again");
        assert_eq!(second.cells.as_ref().map(Vec::len), Ok(10));
        assert!(second.edit.is_ok());
    }

    #[test]
    fn added_columns_are_named_after_the_tables_base() {
        let mut d = FieldTableDraft::new(DocId(1), 0, [0.0, 0.0, 400.0, 200.0], &[], 0);
        d.table.set_columns(2, &d.base);
        let patterns: Vec<_> = d.table.columns.iter().map(|c: &TableColumn| c.pattern.as_str()).collect();
        assert_eq!(patterns, ["Table1_N", "Table1_c2_N"]);
    }
}
