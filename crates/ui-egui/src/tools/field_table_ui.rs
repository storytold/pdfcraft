//! Prepare a form ▸ Text field table: a movable window that divides the area drawn on a page into
//! rows and columns of text fields, while the page shows the table as it changes. The table is
//! the engine's [`FieldTable`]; this is its window.
//!
//! The window is not modal, so that the page stays in view, but it belongs to one page of one
//! document as it was when the window opened: it closes when that document stops being the one
//! prepared or changes in any way (a page rotated, moved or deleted, an edit undone), and adds the
//! table there.

use std::sync::Arc;

use craft::behavior::{Input, Metrics};
use craft::pages::ui::ColumnBar;
use craft::theme::variant::Tone;
use craft::ui::buttons::Button;
use craft::ui::contract::{
    Access, Bind, Common, Component, Cx, MeasureCx, PaintUi, Props as _, Role, Scroll, Show as _, ShowContainer as _, SideLabel, clicked,
};
use craft::ui::dialog::{DialogHost, DialogId, DialogResult, DialogStack, HeldAnswer, HeldOk, StackDialog, dialog_id};
use craft::ui::display::Text;
use craft::ui::form::{Field, FieldGroup};
use craft::ui::layout_components::Stack;
use craft::ui::notices::{Notice, NoticeLayout};
use craft::ui::patterns::form_dialogs::FormDialog;
use craft::ui::primitives::{Draw, HAlign, PaintShape, StrokeWidth, TextRun};
use craft::ui::runtime::env::Label;
use craft::ui::runtime::overlay::CloseReason;
use craft::ui::style::{ComponentStyle, StyleColor, StyleCx};
use craft::ui::text::{NumberField, TextField};
use egui::{Context, CornerRadius, Rect, Response, Ui};
use pdfcraft_engine::field_table::{self, Cell, FieldTable, MAX_COLUMNS, MAX_FIRST_ROW, MAX_ROWS, TableError};
use pdfcraft_engine::{DocId, Edit, FormField};
use pdfcraft_render::PageInfo;
use serde_json::{Value as Json, json};

use crate::canvas::PageXform;
use crate::{PdfCraftApp, i18n};

/// The table being set up in the window, and where it goes.
#[derive(Clone, Debug)]
pub struct FieldTableDraft {
    /// The document the table goes in.
    pub doc: DocId,
    /// The page it goes on, 0-based.
    pub page: usize,
    pub table: FieldTable,
    /// The field and widget each cell copies (Copy into Grid), or none for new text fields.
    pub source: Option<(String, usize)>,
    /// What added columns are named after: the first free "TableK" (or "CopyK") when the window opened.
    base: String,
    /// The document's edit generation when the window opened: any edit since closes it.
    generation: u64,
    /// The table as last laid out, kept until the table changes.
    laid: Option<Arc<Laid>>,
}

/// Two drafts are the same table in the same place; what is kept for the next frame isn't part of it.
impl PartialEq for FieldTableDraft {
    fn eq(&self, other: &Self) -> bool {
        (self.doc, self.page, &self.table, &self.source, &self.base, self.generation)
            == (other.doc, other.page, &other.table, &other.source, &other.base, other.generation)
    }
}

/// The empty draft the form dialog's value type needs; a real one comes from [`FieldTableDraft::new`].
impl Default for FieldTableDraft {
    fn default() -> Self {
        let base = "Table1".to_owned();
        FieldTableDraft { doc: DocId(0), page: 0, table: FieldTable::new([0.0, 0.0, 1.0, 1.0], &base), source: None, base, generation: 0, laid: None }
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

impl Preview {
    /// The page the table is on, 0-based: the one page that shows it.
    pub(crate) fn page(&self) -> usize {
        self.page
    }
}

impl FieldTableDraft {
    /// A table filling `rect` (view space) on `page` of a document at edit `generation`, named after
    /// the first base `form` leaves free.
    pub(crate) fn new(doc: DocId, page: usize, rect: [f64; 4], form: &[FormField], generation: u64) -> Self {
        let base = field_table::free_base(form.iter().map(|f| f.name.as_str()));
        FieldTableDraft { doc, page, table: FieldTable::new(rect, &base), source: None, base, generation, laid: None }
    }

    /// A grid of copies of widget `widget` of the field `source`, starting as two copies beside it
    /// ([`copy_grid`]) on its page (`info`), every column named after the first "CopyK" `form`
    /// leaves free. `None` when the widget isn't on a page or there is no room beside it.
    pub(crate) fn copies(doc: DocId, info: &PageInfo, form: &[FormField], source: (String, usize), generation: u64) -> Option<Self> {
        let widget = form.iter().find(|f| f.name == source.0)?.widgets.get(source.1)?;
        let page = widget.page?;
        let (rect, rows, columns) = copy_grid(info, widget.rect)?;
        let base = free_copy_base(form);
        let mut table = FieldTable::new(rect, &base);
        table.rows = rows;
        table.set_columns(columns, &base);
        Some(FieldTableDraft { doc, page, table, source: Some(source), base, generation, laid: None })
    }

    /// The edit Add fields applies: new text fields, or copies of the source.
    fn edit(&self, info: &PageInfo, form: &[FormField]) -> Result<Edit, TableError> {
        match &self.source {
            Some((name, widget)) => field_table::copy_edit(&self.table, self.page, info, form, name, *widget),
            None => field_table::edit(&self.table, self.page, info, form),
        }
    }

    /// The window's title, and its group's.
    fn title(&self) -> &'static str {
        if self.source.is_some() { tl!("Copy into Grid") } else { tl!("Text Field Table") }
    }

    /// The table laid out on its page (`info`, in a form whose fields are `form`), worked out again
    /// only when the table has changed since. The form can't change under it: an edit closes the
    /// window.
    pub(crate) fn laid(&mut self, info: &PageInfo, form: &[FormField]) -> Arc<Laid> {
        if let Some(laid) = self.laid.as_ref().filter(|l| l.table == self.table) {
            return Arc::clone(laid);
        }
        let laid = Arc::new(Laid { table: self.table.clone(), cells: self.table.cells(), edit: self.edit(info, form) });
        self.laid = Some(Arc::clone(&laid));
        laid
    }

    /// What the page shows of this table this frame.
    pub(crate) fn preview(&mut self, info: &PageInfo, form: &[FormField]) -> Preview {
        Preview { page: self.page, laid: self.laid(info, form) }
    }

    /// Give every column the same width: their widths are shares, so the same number does it.
    fn distribute_evenly(&mut self) {
        for c in &mut self.table.columns {
            c.width = 1.0;
        }
    }
}

/// The text field table window, held as an open dialog of `app.dialogs`. It keeps the page's size and the form as they
/// were when it opened, which is all the table needs of the document: any edit closes the window.
pub struct FieldTableDialog {
    draft: FieldTableDraft,
    info: PageInfo,
    form: Vec<FormField>,
    /// The filed draft keeps beside a refusal the table it refused, so the reason shows only until the table changes.
    held: HeldAnswer<FieldTableDraft>,
}

impl StackDialog for FieldTableDialog {
    type Result = ();

    fn id(&self) -> DialogId {
        dialog_id("field-table")
    }

    fn show(&mut self, ui: &mut Ui, _: &DialogStack) -> Option<DialogResult<()>> {
        let measuring = craft::ui::runtime::measure::active(ui.ctx());
        let finish = self.held.is_accepted();
        let FieldTableDialog { draft, info, form, held } = self;
        let waiting = held.is_waiting();
        let refusal = held.refusal().filter(|(_, refused)| **refused == *draft).map(|(reason, _)| reason.clone());
        // The layout of a table that did not change this pass is kept in the draft, so the body asks for it again for free.
        draft.laid(info, form);
        let shown = FormDialog::new(draft.title().to_owned(), Bind::bound(&mut *draft))
            .ok_label(tl!("Add fields").to_owned())
            .enter_confirms(false)
            .accept_stays_open(true)
            .ok_enabled(!waiting)
            .shaped(|dialog| dialog.modeless())
            .show(ui, |ui, cx, values| {
                // A measuring run closes nothing: the real run of the pass makes the close.
                if finish && !measuring {
                    cx.request_close_as(CloseReason::Activated);
                }
                body(ui, values, info, form, refusal.as_ref())
            });
        if held.refusal().is_some_and(|(_, refused)| *refused != *draft) {
            held.clear_refusal();
        }
        // The first footer button is Add fields; a pass without the press files nothing, so a second pass of a frame files once.
        if shown.picked == Some(0) && !waiting {
            held.file(draft.clone());
        }
        match shown.closed {
            Some(CloseReason::Activated) if finish => Some(held.finish().map_or(DialogResult::Cancel, |_| DialogResult::Ok(()))),
            Some(_) => Some(DialogResult::Cancel),
            None => None,
        }
    }

    fn held_state(&self) -> Option<HeldOk> {
        self.held.state()
    }

    /// Files the table as Add fields does and answers none until the app accepted it; then it answers.
    fn confirm(&mut self, _: &Context) -> Option<()> {
        if self.held.is_accepted() {
            return self.held.finish().map(|_| ());
        }
        if !self.held.is_waiting() {
            let FieldTableDialog { draft, info, form, held } = self;
            if draft.laid(info, form).edit.is_ok() {
                held.file(draft.clone());
            }
        }
        None
    }

    /// The table's rows, columns, spacings, first row number, column widths and name patterns, and why it can't be added
    /// while it can't.
    fn json_fields(&self) -> Option<Json> {
        let t = &self.draft.table;
        let reason = self.draft.edit(&self.info, &self.form).err().map(|e| e.said(&i18n::words));
        let mut map = serde_json::Map::new();
        if let Some((name, widget)) = &self.draft.source {
            map.insert("source".into(), json!(name));
            map.insert("source_widget".into(), json!(widget));
            map.insert("rect".into(), json!(t.rect));
        }
        map.insert("rows".into(), json!(t.rows));
        map.insert("columns".into(), json!(t.columns.len()));
        map.insert("row_gap".into(), json!(t.row_gap));
        map.insert("column_gap".into(), json!(t.column_gap));
        map.insert("first".into(), json!(t.first_row));
        map.insert("widths".into(), json!(t.columns.iter().map(|c| c.width).collect::<Vec<_>>()));
        map.insert("patterns".into(), json!(t.columns.iter().map(|c| c.pattern.as_str()).collect::<Vec<_>>()));
        if let Some(reason) = reason {
            map.insert("reason".into(), json!(reason));
        }
        Some(Json::Object(map))
    }
}

/// Opens the window for a table filling `rect` (view space) on `page` of `doc`, at edit `generation`, over a form whose
/// fields are `form`. Nothing opens when the page is not there.
pub(crate) fn open(app: &mut PdfCraftApp, doc: DocId, page: usize, rect: [f64; 4], form: &[FormField], generation: u64) {
    let Some(info) = app.session.get(doc).and_then(|d| d.info.pages.get(page)).cloned() else { return };
    let draft = FieldTableDraft::new(doc, page, rect, form, generation);
    crate::dialogs::host::open(app, FieldTableDialog { draft, info, form: form.to_vec(), held: HeldAnswer::new() });
}

/// Opens the window for copies of widget `widget` of the field `name` of `doc` in a grid beside it (Copy into Grid), or
/// says there is no room for them. Nothing opens when the field or its page is gone. Whether it opened.
pub(crate) fn open_copies(app: &mut PdfCraftApp, doc: DocId, name: String, widget: usize, form: &[FormField], generation: u64) -> bool {
    let Some(page) = form.iter().find(|f| f.name == name).and_then(|f| f.widgets.get(widget)).and_then(|w| w.page) else { return false };
    let Some(info) = app.session.get(doc).and_then(|d| d.info.pages.get(page)).cloned() else { return false };
    let Some(draft) = FieldTableDraft::copies(doc, &info, form, (name, widget), generation) else {
        app.notify_tr("There is not enough space beside this field for the copies.");
        return false;
    };
    crate::dialogs::host::open(app, FieldTableDialog { draft, info, form: form.to_vec(), held: HeldAnswer::new() });
    true
}

/// Closes the window when its page or document is no longer what it was opened on, and adds the table when asked to; once per
/// pass from the dialogs' results.
pub(crate) fn pass(app: &mut PdfCraftApp, ctx: &Context) {
    let Some(dialog) = app.dialogs.get::<FieldTableDialog>() else { return };
    let (doc, page, generation) = (dialog.draft.doc, dialog.draft.page, dialog.draft.generation);
    // The window belongs to one page of one document, while that document is being prepared.
    let belongs = app.active_ids().map(|(_, id)| id) == Some(doc)
        && app.is_preparing()
        && app.session.get(doc).is_some_and(|d| d.edit_generation() == generation && page < d.info.pages.len() && d.allows_modification());
    if !belongs {
        app.dialogs.cancel::<FieldTableDialog>(ctx);
        return;
    }
    let Some(FieldTableDialog { draft, info, form, held }) = app.dialogs.get_mut::<FieldTableDialog>() else { return };
    if held.take().is_none() {
        return;
    }
    let edit = draft.laid(info, form).edit.as_ref().ok().cloned();
    // Added, the window closes; refused, it stays as it is, with the notice saying why.
    let added = edit.is_some_and(|edit| app.apply_edit(ctx, edit));
    if let Some(dialog) = app.dialogs.get_mut::<FieldTableDialog>() {
        dialog.held.answer(if added { Ok(()) } else { Err(tl!("The field table could not be added.").to_owned().into()) });
    }
}

/// What the page shows of the table being set up in the window of `doc`, if it is open.
pub(crate) fn preview(dialogs: &mut DialogHost, doc: DocId) -> Option<Preview> {
    let dialog = dialogs.get_mut::<FieldTableDialog>().filter(|d| d.draft.doc == doc)?;
    Some(dialog.draft.preview(&dialog.info, &dialog.form))
}

/// The window's controls, the names the table will have, and why it can't be added if it can't. Returns whether it can.
fn body(ui: &mut Ui, d: &mut FieldTableDraft, info: &PageInfo, form: &[FormField], refusal: Option<&Label>) -> bool {
    Stack::new().show(ui, |ui, _| {
        if d.source.is_some() {
            copy_area(ui, d, info);
        }
        FieldGroup::new(d.title().to_owned()).show(ui, |ui, _| {
            whole(ui, "Rows", &mut d.table.rows, 1, MAX_ROWS);
            let mut columns = d.table.columns.len();
            if whole(ui, "Columns", &mut columns, 1, MAX_COLUMNS) {
                d.table.set_columns(columns, &d.base);
            }
            points(ui, "Row spacing", &mut d.table.row_gap, true);
            points(ui, "Column spacing", &mut d.table.column_gap, d.table.columns.len() > 1);
            whole(ui, "First row number", &mut d.table.first_row, 0, MAX_FIRST_ROW);
        });
        widths(ui, d);
        names(ui, d, info, form);
    });
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
        let count = i18n::trn(i18n::current(), names.len() as u64, "{n} field", "{n} fields");
        Text::new(format!("{count}: {shown}")).show(ui);
    }
    if let Err(e) = &laid.edit {
        Notice::new(Tone::Bad, e.said(&i18n::words)).layout(NoticeLayout::Inline).show(ui);
    }
    if let Some(reason) = refusal {
        Notice::new(Tone::Bad, reason.clone()).layout(NoticeLayout::Inline).show(ui);
    }
    laid.edit.is_ok()
}

/// Where the copies go, in points from the displayed page's top-left. Each box is held so the area stays on the page.
fn copy_area(ui: &mut Ui, d: &mut FieldTableDraft, info: &PageInfo) {
    let [x0, y0, x1, y1] = d.table.rect;
    let (mut left, mut top) = (x0.min(x1), y0.min(y1));
    let (mut width, mut height) = ((x1 - x0).abs(), (y1 - y0).abs());
    let (page_width, page_height) = (f64::from(info.width), f64::from(info.height));
    if ![left, top, width, height, page_width, page_height].iter().all(|v| v.is_finite()) {
        return;
    }
    FieldGroup::new(tl!("Area").to_owned()).show(ui, |ui, _| {
        length(ui, "Left", &mut left, 0.0, page_width - width);
        length(ui, "Top", &mut top, 0.0, page_height - height);
        length(ui, "Width", &mut width, 1.0, page_width - left);
        length(ui, "Height", &mut height, 1.0, page_height - top);
    });
    d.table.rect = [left, top, left + width, top + height];
}

/// A length in points held to `least..=most` (to `least` when `most` is under it).
fn length(ui: &mut Ui, label: &str, value: &mut f64, least: f64, most: f64) {
    let most = most.max(least);
    let mut typed = Some(*value);
    Field::new(tl!(label).to_owned()).show(ui, |ui, _| {
        NumberField::new(tl!(label).to_owned(), Bind::bound(&mut typed)).range(least..=most).unit("pt").show(ui);
    });
    // An emptied field keeps the length; `max` then `min` rather than `clamp`, which panics if the bounds cross.
    if let Some(v) = typed.filter(|v| v.is_finite()) {
        *value = v.max(least).min(most);
    }
}

/// The first of "Copy", "Copy2", … that no field in `form` is named after (`{base}_…`), so a grid's names start free.
fn free_copy_base(form: &[FormField]) -> String {
    let taken: std::collections::HashSet<&str> = form.iter().filter_map(|f| f.name.split_once('_').map(|(head, _)| head)).collect();
    // One of the first `taken.len() + 1` is always free.
    (1..=taken.len().saturating_add(1))
        .map(|k| if k == 1 { "Copy".to_owned() } else { format!("Copy{k}") })
        .find(|base| !taken.contains(base.as_str()))
        .unwrap_or_else(|| "Copy".to_owned())
}

/// Points between a field and the grid of its copies.
const COPY_GAP: f64 = 8.0;

/// Where a grid of copies of a widget at `rect` (user space) starts: two copies below the widget, else above, else to its
/// right, else to its left, [`COPY_GAP`] from it and wholly on the displayed page. A view-space area with its rows and
/// columns; `None` when there is no room on any side.
pub(crate) fn copy_grid(info: &PageInfo, rect: [f64; 4]) -> Option<([f64; 4], usize, usize)> {
    let a = info.user_to_view_f64(rect[0], rect[1]);
    let b = info.user_to_view_f64(rect[2], rect[3]);
    let (left, right, top, bottom) = (a[0].min(b[0]), a[0].max(b[0]), a[1].min(b[1]), a[1].max(b[1]));
    let (width, height) = (right - left, bottom - top);
    let (page_width, page_height) = (f64::from(info.width), f64::from(info.height));
    if ![left, right, top, bottom, page_width, page_height].iter().all(|v| v.is_finite()) || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let (tall, wide) = (COPY_GAP + height * 2.0, COPY_GAP + width * 2.0);
    let (fits_across, fits_down) = (left >= 0.0 && right <= page_width, top >= 0.0 && bottom <= page_height);
    if fits_across && bottom + tall <= page_height {
        Some(([left, bottom + COPY_GAP, right, bottom + tall], 2, 1))
    } else if fits_across && top >= tall {
        Some(([left, top - tall, right, top - COPY_GAP], 2, 1))
    } else if fits_down && right + wide <= page_width {
        Some(([right + COPY_GAP, top, right + wide, bottom], 1, 2))
    } else if fits_down && left >= wide {
        Some(([left - wide, top, left - COPY_GAP, bottom], 1, 2))
    } else {
        None
    }
}

/// A whole number field held to `least..=most`. Whether the number changed.
fn whole(ui: &mut Ui, label: &str, value: &mut usize, least: usize, most: usize) -> bool {
    let mut typed = Some(*value as f64);
    Field::new(tl!(label).to_owned()).show(ui, |ui, _| {
        NumberField::new(tl!(label).to_owned(), Bind::bound(&mut typed)).range(least as f64..=most as f64).show(ui);
    });
    // An emptied field keeps the number; `max` then `min` rather than `clamp`, which panics if the bounds cross.
    let next = typed.map_or(*value, |v| (v.round().max(0.0) as usize).max(least).min(most));
    std::mem::replace(value, next) != next
}

/// A length in points from 0 to 100, shown only while `enabled`.
fn points(ui: &mut Ui, label: &str, value: &mut f64, enabled: bool) {
    let mut typed = Some(*value);
    Field::new(tl!(label).to_owned()).enabled(enabled).show(ui, |ui, _| {
        NumberField::new(tl!(label).to_owned(), Bind::bound(&mut typed)).range(0.0..=100.0).unit("pt").show(ui);
    });
    if let Some(v) = typed {
        *value = v;
    }
}

/// The columns as a bar with a divider between each two, and the button giving them one width.
fn widths(ui: &mut Ui, d: &mut FieldTableDraft) {
    let mut shares: Vec<f64> = d.table.columns.iter().map(|c| c.width).collect();
    let names: Vec<String> = (0..shares.len()).map(column_label).collect();
    Text::new(tl!("Column widths").to_owned()).show(ui);
    ColumnBar::new(tl!("Column widths").to_owned(), Bind::bound(&mut shares)).names(&names).show(ui);
    if shares.len() == d.table.columns.len() {
        for (column, share) in d.table.columns.iter_mut().zip(shares) {
            column.width = share;
        }
    }
    let more = d.table.columns.len() > 1;
    if clicked(&Button::new(tl!("Distribute evenly").to_owned()).enabled(more).show(ui)) {
        d.distribute_evenly();
    }
}

/// How each column names its cells, with the width it has.
fn names(ui: &mut Ui, d: &mut FieldTableDraft, info: &PageInfo, form: &[FormField]) {
    Text::new(tl!("Field names").to_owned()).show(ui);
    // How the names are worked out. Its tokens are the same in every language.
    Text::new(
        tl!("N or {row} is the row number, {col} the column number, {seq} the cell number. With none of them, the row number goes last.").to_owned(),
    )
    .show(ui);
    // The widths shown don't depend on the names being typed below.
    let layout = d.laid(info, form);
    let total: f64 = d.table.columns.iter().map(|c| c.width).sum();
    Stack::new().scroll(Scroll::vertical()).show(ui, |ui, _| {
        for (c, column) in d.table.columns.iter_mut().enumerate() {
            let width = layout.cells.as_ref().ok().and_then(|cells| cells.get(c)).map(|cell| cell.rect[2] - cell.rect[0]);
            let share = if total > 0.0 && total.is_finite() { column.width / total } else { 0.0 };
            let size = width.map_or_else(|| "—".to_owned(), |w| format!("{w:.1} pt · {:.0}%", share * 100.0));
            // The column names the box; its width is the label's detail.
            Field::new(column_label(c)).side(SideLabel::new().detail(size)).id_salt(("field table column", c)).show(ui, |ui, _| {
                TextField::new(column_label(c), Bind::bound(&mut column.pattern)).show(ui);
            });
        }
    });
}

/// "Column 2" for the column at 0-based `index`.
fn column_label(index: usize) -> String {
    i18n::fmt(tl!("Column {n}"), &[("n", &index.saturating_add(1).to_string())])
}

/// The look of the table preview on the page: the cells and the reason a table can't be laid out.
#[derive(Clone, Debug, PartialEq, ComponentStyle)]
#[non_exhaustive]
pub(crate) struct TablePreviewStyle {
    #[style(metrics)]
    metrics: Metrics,
    /// A cell's outline and name: the selection role.
    cell: StyleColor,
    /// The outline of a table that can't be laid out, and the reason.
    bad: StyleColor,
}

/// The table being set up, on its page: each cell with its name, or the table's outline with the reason it can't be laid
/// out. Only the cells in view are drawn, and only those with room for it get their name: a table can have 5,000 cells.
#[derive(craft::ui::contract::Component, craft::ui::contract::Props)]
#[component(canvas_control)]
pub(crate) struct TablePreview<'a> {
    #[common]
    common: Common,
    preview: &'a Preview,
    xf: PageXform,
}

impl<'a> TablePreview<'a> {
    /// The preview named `label`, on the page `xf` places on screen.
    pub(crate) fn new(label: impl Into<Label>, preview: &'a Preview, xf: PageXform) -> Self {
        TablePreview { common: Common::new(), preview, xf }.label(label)
    }
}

impl Component for TablePreview<'_> {
    const ROLE: Role = Role::GRAPHICS_OBJECT;
    const ALT_ROLES: &'static [Role] = &[Role::None];
    const INPUT: Input = Input::Display;
    const PARTS: bool = false;
    const STOPS: bool = false;
    const OVERLAYS: bool = false;
    type Style = TablePreviewStyle;
    type Output = Response;

    fn style(cx: &StyleCx<'_>, _: TablePreviewStyleKey) -> TablePreviewStyle {
        TablePreviewStyle { metrics: Metrics::new(), cell: cx.color(cx.tokens.color.accent.solid), bad: cx.tone(Tone::Bad) }
    }

    fn measure(&mut self, _: &MeasureCx<'_, Self>) -> egui::Vec2 {
        egui::Vec2::ZERO
    }

    fn paint(&mut self, ui: &mut PaintUi<'_>, cx: &Cx<'_, Self>) {
        let draw = ui.draw();
        paint_table(&draw, &self.xf, cx.visible, self.preview, cx.style);
    }

    fn access(&self, access: &mut Access, _: &Cx<'_, Self>) {
        access.role(Role::None);
    }

    fn finish(self, cx: Cx<'_, Self>) -> Response {
        cx.response.clone()
    }
}

/// The least a cell's name needs to be drawn in it, in points on screen.
const NAME_ROOM: egui::Vec2 = egui::vec2(24.0, 14.0);

fn paint_table(draw: &Draw<'_>, xf: &PageXform, visible: Rect, preview: &Preview, style: &TablePreviewStyle) {
    let on_screen = |r: [f64; 4]| xf.view_rect([r[0] as f32, r[1] as f32, r[2] as f32, r[3] as f32]);
    let outline = on_screen(preview.laid.table.rect);
    match &preview.laid.cells {
        Ok(cells) => {
            for cell in cells {
                let r = on_screen(cell.rect);
                if !visible.intersects(r) {
                    continue;
                }
                draw.stroke(&PaintShape::Rect(r, CornerRadius::ZERO), StrokeWidth::Medium, style.cell);
                if r.height() >= NAME_ROOM.y && r.width() >= NAME_ROOM.x {
                    draw.clipped(r, |inside| {
                        inside.text(r, &TextRun::new(cell.name.as_str()).color(style.cell).align(HAlign::Center));
                    });
                }
            }
        }
        Err(e) => {
            draw.stroke(&PaintShape::Rect(outline, CornerRadius::ZERO), StrokeWidth::Medium, style.bad);
            let reason = e.said(&i18n::words);
            draw.clipped(outline, |inside| {
                inside.text(outline, &TextRun::new(reason.as_str()).color(style.bad).align(HAlign::Center));
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The copies start beside their field, on the page, and say so when there is no room.
    #[test]
    fn a_copy_grid_starts_on_the_page_without_covering_the_source() {
        let info = PageInfo { width: 400.0, height: 300.0, label: "1".into(), crop: [0.0, 0.0, 400.0, 300.0], rotation: 0 };
        let inside = |r: [f64; 4]| r[0] >= 0.0 && r[1] >= 0.0 && r[2] <= 400.0 && r[3] <= 300.0;
        let apart = |a: [f64; 4], b: [f64; 4]| a[2] <= b[0] || b[2] <= a[0] || a[3] <= b[1] || b[3] <= a[1];
        // A field near the top of the page (user y up) gets its copies under it; one near the bottom, over it.
        for (user, view) in [([10.0, 270.0, 110.0, 290.0], [10.0, 10.0, 110.0, 30.0]), ([10.0, 0.0, 110.0, 20.0], [10.0, 280.0, 110.0, 300.0])] {
            let (grid, rows, columns) = copy_grid(&info, user).expect("room above or below");
            assert_eq!((rows, columns), (2, 1));
            assert!(inside(grid) && apart(grid, view), "{grid:?} is on the page and clear of {view:?}");
        }
        assert!(copy_grid(&info, [0.0, 0.0, 400.0, 300.0]).is_none(), "a field filling the page leaves no room");
    }

    fn draft(columns: usize) -> FieldTableDraft {
        let mut d = FieldTableDraft::new(DocId(1), 0, [0.0, 0.0, 400.0, 200.0], &[], 0);
        d.table.set_columns(columns, &d.base);
        d
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
}
