//! The Fields panel: the document's form fields by page, each page's in tab order. While preparing
//! a form a field can move earlier or later in its page's tab order (Acrobat: Order Tabs Manually).
// PLAN: assumed `super::right::PanelResult` (pdf-chrome-w13).

use std::collections::HashMap;

use craft::behavior::{DisplayOrder, IconId, ItemSource, Selection, SelectionChange};
use craft::ui::buttons::Button;
use craft::ui::contract::{Bind, CollectionEvent, LeadSource, Props as _, SelectMode, Show as _, ShowCollection as _};
use craft::ui::display::Text;
use craft::ui::list::{List, ListRow, RowEvent, TrailingFace, TrailingPart};
use craft::ui::primitives::IconSpec;
use pdfcraft_render::{DocInfo, Field, FieldKind};

use super::right::PanelResult;
use super::{Nav, empty_state};
use crate::PdfCraftApp;

/// The part that moves a field earlier in its page's tab order.
const EARLIER: TrailingPart = TrailingPart::new(0);
/// The part that moves a field later in its page's tab order.
const LATER: TrailingPart = TrailingPart::new(1);

/// A row of the Fields list. A field's identity is its name and page, not its position: the form can
/// change under a selection. `dup` tells apart fields that share both.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum FieldRow {
    /// A page's heading; `None` is "Unplaced".
    Page(Option<usize>),
    /// A field on a page.
    Field { name: String, page: Option<usize>, dup: usize },
}

/// One row with what it stands for.
struct Row<'a> {
    key: FieldRow,
    /// The field; `None` for a heading.
    field: Option<&'a Field>,
    /// The form field and widget a selection of this row names; `None` when the form has no widget of it
    /// on its page.
    select: Option<(String, usize)>,
}

/// The rows in display order, with each key's position.
struct Rows<'a> {
    rows: Vec<Row<'a>>,
    at: HashMap<FieldRow, usize>,
}

impl<'a> Rows<'a> {
    fn new(fields: &'a [Field], form: &[pdfcraft_engine::FormField]) -> Self {
        let mut rows = Vec::new();
        for (page, page_rows) in field_rows(fields, form) {
            rows.push(Row { key: FieldRow::Page(page), field: None, select: None });
            let mut seen: HashMap<&str, usize> = HashMap::new();
            for (field, select) in page_rows {
                let dup = seen.entry(field.name.as_str()).or_insert(0);
                let key = FieldRow::Field { name: field.name.clone(), page, dup: *dup };
                *dup += 1;
                rows.push(Row { key, field: Some(field), select });
            }
        }
        let at = rows.iter().enumerate().map(|(i, r)| (r.key.clone(), i)).collect();
        Rows { rows, at }
    }

    fn get(&self, key: &FieldRow) -> Option<&Row<'a>> {
        self.at.get(key).and_then(|&i| self.rows.get(i))
    }
}

impl DisplayOrder<FieldRow> for Rows<'_> {
    fn len(&self) -> usize {
        self.rows.len()
    }

    fn key(&self, at: usize) -> FieldRow {
        self.rows.get(at).map(|r| r.key.clone()).unwrap_or(FieldRow::Page(None))
    }

    fn index_of(&self, key: &FieldRow) -> Option<usize> {
        self.at.get(key).copied()
    }

    fn text(&self, at: usize) -> Option<std::borrow::Cow<'_, str>> {
        self.rows.get(at).and_then(|r| r.field).map(|f| std::borrow::Cow::Borrowed(f.name.as_str()))
    }
}

impl ItemSource<FieldRow> for Rows<'_> {}

/// A heading's text: "Page {label}", or "Unplaced" for no page or one the document doesn't have (#1307).
fn heading(info: &DocInfo, page: Option<usize>) -> String {
    page.and_then(|p| info.pages.get(p))
        .map(|page| crate::i18n::fmt(tl!("Page {label}"), &[("label", &page.label)]))
        .unwrap_or_else(|| tl!("Unplaced").to_string())
}

/// The icon a field kind shows.
fn kind_icon(kind: &FieldKind) -> &'static str {
    match kind {
        FieldKind::Text => "text-cursor-input",
        FieldKind::CheckBox => "check-circle-2",
        FieldKind::Radio => "circle",
        FieldKind::PushButton | FieldKind::Unknown => "square",
        FieldKind::Combo => "chevron-down",
        FieldKind::List => "list",
        FieldKind::Signature => "signature",
    }
}

/// The rows a selection change leaves selected: `current` with `change` applied.
fn changed(current: Vec<FieldRow>, change: &SelectionChange<FieldRow>) -> Vec<FieldRow> {
    match change {
        SelectionChange::Set(keys) => keys.clone(),
        SelectionChange::Toggle(key) => {
            let mut keys = current;
            match keys.iter().position(|k| k == key) {
                Some(i) => {
                    keys.remove(i);
                }
                None => keys.push(key.clone()),
            }
            keys
        }
        SelectionChange::Clear => Vec::new(),
        _ => current,
    }
}

/// Show the Fields panel for the active document.
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) -> PanelResult {
    let mut result = PanelResult::default();
    let preparing = app.is_preparing();
    let Some((index, id)) = app.active_ids() else { return result };
    let Some(doc) = app.session.get(id) else { return result };
    let Some(view) = app.views.get_mut(index) else { return result };
    let info = &doc.info;
    if info.fields.is_empty() {
        empty_state(ui, "text-cursor-input", tl!("This document has no form fields."));
        return result;
    }
    let selection = &mut view.session.prepare;
    if preparing {
        Text::new(tl!("Tab order: move a field with its arrows.").to_string()).show(ui);
        Text::new(tl!("Shift- or Ctrl/Command-click to select several fields.").to_string()).show(ui);
        let names = selection.names();
        Text::new(crate::i18n::fmt(tl!("{n} selected"), &[("n", &names.len().to_string())])).show(ui);
        if Button::new(tl!("Properties…").to_string()).enabled(!names.is_empty()).show(ui).clicked() {
            result.field_properties = selection.selected.clone();
        }
    }
    let rows = Rows::new(&info.fields, doc.form.as_slice());
    // What the list shows is the selection as this pass found it; the events below change `selection`.
    let (lead, also) = (selection.selected.clone(), selection.also.clone());
    let is_selected = |row: &Row<'_>| row.select.as_ref().is_some_and(|key| lead.as_ref() == Some(key) || also.contains(key));
    let shown = Selection::new(
        rows.rows.iter().filter(|r| is_selected(r)).map(|r| r.key.clone()).collect(),
        rows.rows.iter().find(|r| r.select.is_some() && r.select.as_ref() == lead.as_ref()).map(|r| r.key.clone()),
        None,
    );
    let mode = if preparing { SelectMode::Multi } else { SelectMode::None };
    let out = List::<FieldRow>::new(tl!("Fields").to_string()).select(mode).selection(Bind::given(shown)).natural_height(true).show_items(
        ui,
        &rows,
        &mut |key, _| {
            let Some(row) = rows.get(key) else { return ListRow::new("") };
            let Some(field) = row.field else {
                let page = if let FieldRow::Page(p) = &row.key { *p } else { None };
                return ListRow::new(heading(info, page)).section_heading();
            };
            let mut list_row = ListRow::new(field.name.clone()).icon(IconSpec::svg(IconId::new(kind_icon(&field.kind)))).tooltip(field_tip(field));
            if let Some(value) = &field.value {
                list_row = list_row.optional(value.clone());
            }
            if preparing && field.page.is_some() {
                // Each arrow names the field it moves.
                let named = |tip: &str| format!("{}: {}", tl!(tip), field.name);
                list_row = list_row
                    .trailing_part(EARLIER, TrailingFace::Icon(IconSpec::svg(IconId::new("chevron-up"))), named("Earlier in tab order"))
                    .trailing_part(LATER, TrailingFace::Icon(IconSpec::svg(IconId::new("chevron-down"))), named("Later in tab order"));
            }
            list_row
        },
    );
    let mut flashed = false;
    let mut flash = |key: &FieldRow, result: &mut PanelResult| {
        if let Some(Row { field: Some(f), .. }) = rows.get(key)
            && let (Some(p), Some(r)) = (f.page, f.rect)
            && !flashed
        {
            result.nav = Some(Nav::Flash(p, r));
            flashed = true;
        }
    };
    for event in &out.events {
        match event {
            CollectionEvent::Selection(change) => {
                if preparing {
                    let current = rows.rows.iter().filter(|r| is_selected(r)).map(|r| r.key.clone()).collect();
                    let keys: Vec<(String, usize)> =
                        changed(current, change).iter().filter_map(|k| rows.get(k).and_then(|r| r.select.clone())).collect();
                    // The anchor of Align and Match Size stays the field selected first while it stays selected, as
                    // `PrepareView::select` keeps it for clicks on the page.
                    let lead = selection.selected.clone().filter(|lead| keys.contains(lead)).or_else(|| keys.last().cloned());
                    selection.also = keys.into_iter().filter(|key| Some(key) != lead.as_ref()).collect();
                    selection.selected = lead;
                }
                if let SelectionChange::Set(keys) = change
                    && let Some(key) = keys.last()
                {
                    flash(key, &mut result);
                }
            }
            CollectionEvent::LeadMoved { key, by: LeadSource::Pointer } => flash(key, &mut result),
            CollectionEvent::Activate(key) if preparing => {
                if let Some(select) = rows.get(key).and_then(|r| r.select.clone()) {
                    if !is_selected(rows.get(key).unwrap_or(&rows.rows[0])) {
                        selection.selected = Some(select.clone());
                        selection.also.clear();
                    }
                    result.field_properties = Some(select);
                }
            }
            CollectionEvent::Item(key, RowEvent::Trailing { part, .. }) => {
                if let Some(Row { field: Some(f), .. }) = rows.get(key) {
                    let earlier = *part == EARLIER;
                    if earlier || *part == LATER {
                        result.edit = Some(pdfcraft_engine::Edit::MoveInTabOrder { name: f.name.clone(), earlier });
                    }
                }
            }
            _ => {}
        }
    }
    result
}

/// A Fields panel row's tooltip: the field's kind, its own tooltip and whether it has scripts.
fn field_tip(f: &Field) -> String {
    let kind = format!("{:?}", f.kind);
    let mut tip = crate::i18n::fmt(tl!("{kind} field"), &[("kind", tl!(&kind))]);
    if let Some(tt) = &f.tooltip {
        tip.push_str(&format!(" — {tt}"));
    }
    if f.has_actions {
        tip.push_str(&format!("\n{}", tl!("Has JavaScript actions (run in M6)")));
    }
    tip
}

/// One page's rows of the Fields panel: each field, with the form field and widget a click on
/// it selects (`None` when the form has no widget of it on that page).
type FieldRows<'a> = Vec<(&'a Field, Option<(String, usize)>)>;

/// The Fields panel's rows: by page ("Unplaced" first), each page's in tab order. A row is paired
/// with the first form field of its name, as the engine pairs them: one map, built once a frame.
/// Searching the form for each row, and again inside the sort, was O(fields²) every frame.
fn field_rows<'a>(fields: &'a [Field], form: &[pdfcraft_engine::FormField]) -> Vec<(Option<usize>, FieldRows<'a>)> {
    let mut by_name: HashMap<&str, &pdfcraft_engine::FormField> = HashMap::with_capacity(form.len());
    for field in form {
        by_name.entry(field.name.as_str()).or_insert(field);
    }
    let rank = |name: &str| by_name.get(name).and_then(|f| f.widgets.iter().map(|w| w.tab).min()).unwrap_or(usize::MAX);
    let mut ordered: Vec<&Field> = fields.iter().collect();
    ordered.sort_by_cached_key(|f| (f.page, rank(&f.name)));
    let mut rows: Vec<(Option<usize>, FieldRows<'a>)> = Vec::new();
    for f in ordered {
        let key =
            by_name.get(f.name.as_str()).and_then(|field| field.widgets.iter().position(|w| w.page == f.page).map(|wi| (field.name.clone(), wi)));
        match rows.last_mut() {
            Some((page, page_rows)) if *page == f.page => page_rows.push((f, key)),
            _ => rows.push((f.page, vec![(f, key)])),
        }
    }
    rows
}

#[cfg(test)]
mod field_rows_tests {
    use super::field_rows;
    use std::sync::Arc;

    /// A PDF of `objects` (numbered from 1; object 1 the catalog) with a valid xref table.
    fn pdf(objects: &[String]) -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1).bytes());
        out
    }

    /// The fields the engine reads from `bytes`: the inspector's list and the form.
    fn open(bytes: Vec<u8>) -> (Vec<pdfcraft_render::Field>, Arc<Vec<pdfcraft_engine::FormField>>) {
        let mut session = pdfcraft_engine::Session::new();
        let id = session.open("form.pdf", None, Arc::new(bytes), None).expect("opens");
        let doc = session.get(id).expect("open");
        (doc.info.fields.clone(), doc.form.clone())
    }

    /// Rows go by page, the unplaced first, each page's in tab order (here its annotation order,
    /// not the `/Fields` order), and a row selects the first form field of its name.
    #[test]
    fn rows_follow_pages_and_tab_order() {
        let widget = |name: &str, page: Option<usize>| match page {
            Some(p) => format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /Rect [10 10 90 30] /P {} 0 R >>", 3 + p),
            None => format!("<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /Rect [10 10 90 30] >>"),
        };
        let bytes = pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R 7 0 R 8 0 R 9 0 R 10 0 R 6 0 R] >> >>".into(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [6 0 R 5 0 R] >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Annots [7 0 R 8 0 R 9 0 R] >>".into(),
            widget("a1", Some(0)),
            widget("a2", Some(0)),
            widget("b", Some(1)),
            widget("twin", Some(1)),
            widget("twin", Some(1)),
            widget("loose", None),
        ]);
        let (fields, form) = open(bytes);
        // A page's rows as (field name, the form field and widget a click selects).
        type Named<'a> = Vec<(&'a str, Option<(String, usize)>)>;
        let rows: Vec<(Option<usize>, Named)> = field_rows(&fields, &form)
            .into_iter()
            .map(|(page, rows)| (page, rows.into_iter().map(|(f, key)| (f.name.as_str(), key)).collect()))
            .collect();
        let key = |name: &str| Some((name.to_string(), 0));
        assert_eq!(
            rows,
            [
                (None, vec![("loose", key("loose"))]),
                (Some(0), vec![("a2", key("a2")), ("a1", key("a1"))]),
                (Some(1), vec![("b", key("b")), ("twin", key("twin")), ("twin", key("twin"))]),
            ]
        );
    }
}
