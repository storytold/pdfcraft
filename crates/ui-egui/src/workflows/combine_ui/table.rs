use std::cell::RefCell;
use std::time::SystemTime;

use craft::behavior::{ColumnKey, ColumnWidth, DisplayOrder, EditEvent, IconId, ItemSource, ReorderKeys};
use craft::theme::variant::Tone;
use craft::ui::contract::{
    Bind, CollectionEvent, ColumnEvent, ColumnLayout, ColumnSort, ColumnSpec, Entry, Props as _, ReorderMove, ReorderRule, ShowCollection as _,
    SpaceAction,
};
use craft::ui::list::{CellStatus, ListRow, RowEvent};
use craft::ui::menu::Menu;
use craft::ui::table::Table;
use craft::ui::text::TextField;

// PLAN: assumed `CombineKey::{File(u64), Page(u64, usize)}` in `files` (pdf-workflows-w5)
use super::files::{CombineFile, CombineKey, Lock, RowAction, SortKey};
use super::parts::utc_stamp;
use crate::PdfCraftApp;

const NAME: ColumnKey = ColumnKey::new("name");
const PAGES: ColumnKey = ColumnKey::new("pages");
const SIZE: ColumnKey = ColumnKey::new("size");
const MODIFIED: ColumnKey = ColumnKey::new("modified");
const WARNINGS: ColumnKey = ColumnKey::new("warnings");

/// The column of a sort key; the one place the two vocabularies meet.
fn column_of(key: SortKey) -> ColumnKey {
    match key {
        SortKey::Name => NAME,
        SortKey::Pages => PAGES,
        SortKey::Size => SIZE,
        SortKey::Modified => MODIFIED,
        SortKey::Warnings => WARNINGS,
    }
}

fn sort_key_of(column: &ColumnKey) -> Option<SortKey> {
    SortKey::ALL.into_iter().find(|key| column_of(*key) == *column)
}

/// The layout the last release kept in its settings: `order`, every sort key once by its name, and
/// optionally `widths`, one number per key in the keys' own order. `None` for anything else (a
/// repeated key, a wrong count, a width that is not a positive number), which leaves the default.
pub(crate) fn columns_from_legacy(value: &serde_json::Value) -> Option<ColumnLayout> {
    let object = value.as_object()?;
    let order =
        object.get("order")?.as_array()?.iter().map(|name| serde_json::from_value::<SortKey>(name.clone()).ok()).collect::<Option<Vec<_>>>()?;
    let complete = order.len() == SortKey::ALL.len() && SortKey::ALL.iter().all(|key| order.contains(key));
    if !complete {
        return None;
    }
    let widths = match object.get("widths") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(widths) => {
            let widths = widths.as_array()?;
            if widths.len() != SortKey::ALL.len() {
                return None;
            }
            let mut pairs = Vec::new();
            for (key, width) in SortKey::ALL.into_iter().zip(widths) {
                let width = width.as_f64().filter(|w| w.is_finite() && *w > 0.0)?;
                pairs.push((column_of(key), width as f32));
            }
            pairs
        }
    };
    Some(ColumnLayout { order: order.into_iter().map(column_of).collect(), widths, hidden: Vec::new() })
}

/// The files as a collection source, keyed by id, in the order of the list.
struct Files<'a> {
    ids: &'a [u64],
    names: &'a [String],
}

impl DisplayOrder<CombineKey> for Files<'_> {
    fn len(&self) -> usize {
        self.ids.len()
    }

    fn key(&self, at: usize) -> CombineKey {
        CombineKey::File(self.ids.get(at).copied().unwrap_or_default())
    }

    fn index_of(&self, key: &CombineKey) -> Option<usize> {
        match key {
            CombineKey::File(id) => self.ids.iter().position(|held| held == id),
            _ => None,
        }
    }

    fn text(&self, at: usize) -> Option<std::borrow::Cow<'_, str>> {
        self.names.get(at).map(|name| std::borrow::Cow::Borrowed(name.as_str()))
    }
}

impl ItemSource<CombineKey> for Files<'_> {}

/// What a row shows, worked out before the table runs so its row closure borrows nothing mutable.
struct RowData {
    name: String,
    range: String,
    editable: bool,
    range_tip: String,
    range_count: String,
    size: String,
    modified: Option<(u64, String)>,
    status: Option<(Tone, String, String)>,
    locked: Option<Lock>,
}

fn row_data(app: &mut PdfCraftApp, checks: &[Result<usize, String>]) -> Vec<RowData> {
    let conflicts = app.combine_conflicts.per_file.clone();
    let twice = app.combine_tab.duplicate_warnings(&app.combine_draft);
    let now = SystemTime::now();
    app.combine_draft
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let check = checks.get(i).cloned().unwrap_or(Ok(f.pages));
            let mut errors: Vec<String> = f.problem.iter().cloned().collect();
            if let Err(e) = &check {
                errors.push(e.clone());
            }
            let mut warnings = f.notes.clone();
            if twice.get(i).copied().unwrap_or(false) {
                warnings.push(tl!("Added more than once").to_string());
            }
            if let Some(renames) = conflicts.get(i).filter(|r| !r.is_empty()) {
                let list = renames.iter().take(3).map(|(old, new)| format!("{old} → {new}")).collect::<Vec<_>>().join(", ");
                warnings.push(crate::i18n::fmt(
                    &crate::i18n::trn(
                        crate::i18n::current(),
                        renames.len() as u64,
                        "{n} field name collides with earlier files and will be renamed: {list}",
                        "{n} field names collide with earlier files and will be renamed: {list}",
                    ),
                    &[("list", &list)],
                ));
            }
            let all = errors.iter().chain(&warnings).cloned().collect::<Vec<_>>().join("\n");
            let status = match (errors.first(), warnings.first()) {
                (Some(e), _) => Some((Tone::Bad, e.clone(), all)),
                (None, Some(w)) => Some((Tone::Warn, w.clone(), all)),
                (None, None) => None,
            };
            let range_tip = match &check {
                Err(e) => e.clone(),
                Ok(k) => {
                    let of = crate::i18n::fmt(tl!("{k} of {n}"), &[("k", &k.to_string()), ("n", &f.pages.to_string())]);
                    let what = crate::i18n::fmt(tl!("Pages of {name} to combine, e.g. 1-3, 6"), &[("name", &f.name)]);
                    format!("{what} ({of})")
                }
            };
            let range_count = if f.problem.is_some() {
                "—".to_string()
            } else {
                let k = check.as_ref().map_or_else(|_| "?".to_string(), usize::to_string);
                crate::i18n::fmt(tl!("{k} of {n}"), &[("k", &k), ("n", &f.pages.to_string())])
            };
            RowData {
                name: f.name.clone(),
                range: f.range.clone(),
                editable: f.problem.is_none(),
                range_tip,
                range_count,
                size: crate::panels::human_size(f.bytes.len()),
                modified: f.modified.map(|m| (now.duration_since(m).map_or(0, |d| d.as_secs()), utc_stamp(m))),
                status,
                locked: f.lock,
            }
        })
        .collect()
}

fn columns() -> Vec<ColumnSpec> {
    vec![
        ColumnSpec::new(NAME, SortKey::Name.label(), ColumnWidth::fill(1.0)).sortable().movable().resizable(),
        ColumnSpec::new(PAGES, SortKey::Pages.label(), ColumnWidth::Content).sortable().movable().resizable(),
        ColumnSpec::new(SIZE, SortKey::Size.label(), ColumnWidth::Content).numeric().first_descending().sortable().movable().resizable(),
        ColumnSpec::new(MODIFIED, SortKey::Modified.label(), ColumnWidth::Content).first_descending().sortable().movable().resizable(),
        ColumnSpec::new(WARNINGS, SortKey::Warnings.label(), ColumnWidth::fill(1.0)).sortable().movable().resizable(),
    ]
}

/// Where a reorder lands, as the `DropAt` the list takes: the first moved file's id and the gap in the list as it
/// is. `ReorderMove::to` counts in the list without the moved files, so the gap is the position of the file now at
/// `to` (or the end).
fn drop_at(ids: &[u64], moved: &ReorderMove<CombineKey>) -> Option<(u64, usize)> {
    let ids_of: Vec<u64> = moved
        .keys
        .iter()
        .filter_map(|key| match key {
            CombineKey::File(id) => Some(*id),
            _ => None,
        })
        .collect();
    let first = *ids_of.first()?;
    let rest: Vec<u64> = ids.iter().copied().filter(|id| !ids_of.contains(id)).collect();
    let gap = match rest.get(moved.to) {
        Some(target) => ids.iter().position(|id| id == target)?,
        None => ids.len(),
    };
    Some((first, gap))
}

/// The file table; what it reports as a row action.
pub(super) fn table(app: &mut PdfCraftApp, ui: &mut egui::Ui) -> Option<RowAction> {
    let checks: Vec<Result<usize, String>> = app.combine_draft.iter_mut().map(CombineFile::selection).collect();
    let rows = row_data(app, &checks);
    let ids: Vec<u64> = app.combine_draft.iter().map(|f| f.id).collect();
    let names: Vec<String> = rows.iter().map(|r| r.name.clone()).collect();
    let sort = app.combine_tab.sort.map(|(key, ascending)| ColumnSort::new(column_of(key), !ascending));
    let revision = app.combine_tab.revision;
    let edits: RefCell<Vec<(usize, EditEvent<String>)>> = RefCell::new(Vec::new());
    let source = Files { ids: &ids, names: &names };

    let out = Table::<CombineKey>::new(tl!("Files"), columns())
        .id(super::collection_id())
        .request_focus(craft::ui::runtime::focus::FocusRequest::WhenNone)
        .sort(sort)
        .selection(Bind::bound(&mut app.combine_tab.selection))
        .column_layout(Bind::bound(&mut app.combine_columns))
        .plain_header_menu(true)
        .reorder(ReorderRule::new().keys(ReorderKeys::AltArrows))
        .deletable(true)
        .space(SpaceAction::Preview)
        .show_items(ui, &source, &mut |_, icx| {
            let at = icx.at;
            let Some(row) = rows.get(at) else { return ListRow::new("") };
            let mut list = ListRow::new(row.name.clone()).cells(icx.cells()).striped(icx.row.striped);
            // Keep every draft, including invalid ranges, so the row can show its validation error as the user types.
            let field = TextField::new(tl!("Pages"), Bind::given(row.range.clone())).placeholder(tl!("All pages")).enabled(row.editable);
            let edits = &edits;
            list = list.cell_field(PAGES, field, move |output| {
                edits.borrow_mut().extend(output.events.into_iter().map(|event| (at, event)));
            });
            list = list
                .cell_field_suffix(PAGES, row.range_count.clone())
                .cell_muted(PAGES)
                .cell_tip(PAGES, row.range_tip.clone())
                .cell(SIZE, row.size.clone());
            list = match &row.modified {
                Some((age, stamp)) => list.cell(MODIFIED, icx.env.relative_age(*age, craft::i18n::AgeStyle::Short)).cell_tip(MODIFIED, stamp.clone()),
                None => list.cell(MODIFIED, "—"),
            };
            if let Some((tone, first, all)) = &row.status {
                let mut status = CellStatus::new(*tone, first.clone()).tip(all.clone());
                if row.locked.is_some() {
                    status = status.action(tl!("Unlock…"));
                }
                list = list.cell_status(WARNINGS, status);
            }
            list
        });

    let mut action = None;
    for (at, event) in edits.into_inner() {
        let Some(f) = app.combine_draft.get_mut(at) else { continue };
        if let EditEvent::Update { value, .. } | EditEvent::Commit { after: value, .. } | EditEvent::Set { after: value, .. } = &event {
            f.range.clone_from(value);
        }
        app.combine_range_edit(&event);
    }
    for event in out.events {
        match event {
            CollectionEvent::Selection(_) => app.combine_keep_one_kind(),
            CollectionEvent::Column(ColumnEvent::Sort { key, .. }) => {
                if let Some(key) = sort_key_of(&key) {
                    action = Some(RowAction::Sort(key));
                }
            }
            CollectionEvent::Reorder(EditEvent::Commit { after: moved, .. } | EditEvent::Set { after: moved, .. }) => {
                if let Some((file, gap)) = drop_at(&ids, &moved) {
                    action = Some(RowAction::DropAt { file, revision, gap });
                }
            }
            CollectionEvent::Delete(_) => action = Some(RowAction::Remove),
            CollectionEvent::Preview(_) => action = Some(RowAction::PreviewSelected),
            CollectionEvent::Item(CombineKey::File(id), RowEvent::CellAction { column, .. }) if column == WARNINGS => {
                action = Some(RowAction::Unlock(Some(id)));
            }
            _ => {}
        }
    }
    // A locked file's row offers Unlock… on a right-click, for the row it was opened on.
    let locked = out.menu.as_ref().and_then(|request| match request.key {
        CombineKey::File(id) => ids.iter().position(|i| *i == id).and_then(|at| rows.get(at)).filter(|row| row.locked.is_some()).map(|_| id),
        CombineKey::Page(..) => None,
    });
    if let Some(id) = locked {
        let entries = [Entry::new(id, tl!("Unlock…")).icon(IconId::new("lock-open"))];
        if let Some(id) = Menu::context(tl!("File"), &out.response).pick(ui, &entries).picked {
            action = Some(RowAction::Unlock(Some(id)));
        }
    }
    action
}
