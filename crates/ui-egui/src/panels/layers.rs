//! The Layers panel: the default configuration's `/Order` tree with an eye column, and the panel
//! menu's rows.
// PLAN: assumed `super::right::PanelResult` (pdf-chrome-w13).

use craft::behavior::{ColumnKey, ColumnWidth, DisplayOrder, IconId, ItemSource, SelectionChange};
use craft::keymap::CommandRef;
use craft::ui::contract::{CollectionEvent, ColumnSpec, SelectMode, ShowCollection as _};
use craft::ui::list::{List, ListRow, RowInk, ToggleCell};
use craft::ui::menu::PanelRow;
use craft::ui::runtime::env::Label;
use pdfcraft_render::{DocInfo, LayerItem};

use super::empty_state;
use super::right::PanelResult;
use crate::PdfCraftApp;

/// The eye column.
const EYE: ColumnKey = ColumnKey::new("eye");
/// The label column.
const NAME: ColumnKey = ColumnKey::new("name");
/// The deepest level of `/Order` shown indented; deeper rows stay at it.
const MAX_DEPTH: usize = 8;

/// The `/Order` rows, keyed by their position in `info.layer_order`.
struct Order<'a>(&'a DocInfo);

impl Order<'_> {
    /// The layer (an index into `info.layers`) row `at` shows; `None` for a label.
    fn layer(&self, at: usize) -> Option<usize> {
        match self.0.layer_order.get(at).map(|r| &r.item) {
            Some(LayerItem::Layer(li)) => Some(*li),
            _ => None,
        }
    }

    fn visible(&self, at: usize) -> bool {
        self.layer(at).and_then(|li| self.0.layers.get(li)).is_some_and(|l| l.visible)
    }
}

impl DisplayOrder<usize> for Order<'_> {
    fn len(&self) -> usize {
        self.0.layer_order.len()
    }

    fn key(&self, at: usize) -> usize {
        at
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        (*key < self.0.layer_order.len()).then_some(*key)
    }

    fn text(&self, at: usize) -> Option<std::borrow::Cow<'_, str>> {
        match self.0.layer_order.get(at).map(|r| &r.item)? {
            LayerItem::Layer(li) => self.0.layers.get(*li).map(|l| std::borrow::Cow::Borrowed(l.name.as_str())),
            LayerItem::Label(label) => Some(std::borrow::Cow::Borrowed(label.as_str())),
        }
    }
}

impl ItemSource<usize> for Order<'_> {
    fn toggled(&self, at: usize, column: &ColumnKey) -> bool {
        *column == EYE && self.visible(at)
    }
}

/// Show the Layers panel for the active document. A layer's eye, or a click on its row, turns it on
/// or off; a label of `/Order` heads the rows under it and does nothing.
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) -> PanelResult {
    let mut result = PanelResult::default();
    let Some((_, id)) = app.active_ids() else { return result };
    let Some(doc) = app.session.get(id) else { return result };
    let info = &doc.info;
    if info.layers.is_empty() {
        empty_state(ui, "layers", tl!("This document has no layers."));
        return result;
    }
    if info.layer_order.is_empty() {
        // ISO 32000-2 Table 99: only the groups /Order lists are presented.
        empty_state(ui, "layers", tl!("This document lists no layers to show."));
        return result;
    }
    let order = Order(info);
    let columns = vec![
        ColumnSpec::toggle(EYE, Label::Empty, craft::ui::contract::ToggleColumn::sweepable()).icon(IconId::new("eye")),
        ColumnSpec::new(NAME, Label::Empty, ColumnWidth::fill(1.0)),
    ];
    let out = List::<usize>::new(tl!("Layers").to_string())
        .header(false)
        .tree(true)
        .natural_height(true)
        .select(SelectMode::Single)
        .columns(columns)
        .show_items(ui, &order, &mut |key, icx| {
            let Some(row) = info.layer_order.get(*key) else { return ListRow::new("") };
            let depth = row.depth.min(MAX_DEPTH);
            match &row.item {
                LayerItem::Label(label) => ListRow::new(label.clone()).section_heading().depth(depth).cells(icx.cells()).state(icx.row.clone()),
                LayerItem::Layer(li) => {
                    let Some(layer) = info.layers.get(*li) else { return ListRow::new("") };
                    let list_row = ListRow::new(layer.name.clone())
                        .layer(true)
                        .depth(depth)
                        .cells(icx.cells())
                        .state(icx.row.clone())
                        .toggle(ToggleCell::eye(EYE, layer.visible).row_press());
                    if layer.visible { list_row } else { list_row.ink(RowInk::Dimmed) }
                }
            }
        });
    // The eye reports a toggle. A click on the row's own area is a selection: it flips what the eye shows.
    let mut toggled = false;
    for event in &out.events {
        if let CollectionEvent::Toggle(edit) = event
            && let Some(cell) = edit.landed()
            && let Some(layer) = order.layer(cell.key)
        {
            result.layer = Some((layer, cell.on));
            toggled = true;
        }
    }
    if !toggled {
        for event in &out.events {
            if let CollectionEvent::Selection(SelectionChange::Set(keys)) = event
                && let Some(key) = keys.last()
                && let Some(layer) = order.layer(*key)
            {
                result.layer = Some((layer, !order.visible(*key)));
            }
        }
    }
    result
}

/// The rows of the Layers panel's menu: Reset to Initial Visibility while the document has layers.
pub(crate) fn menu_rows(app: &PdfCraftApp) -> Vec<PanelRow> {
    let has_layers = app.active_ids().and_then(|(_, id)| app.session.get(id)).is_some_and(|d| !d.info.layers.is_empty());
    if has_layers { vec![PanelRow::Command(CommandRef::new("layers.reset_visibility"))] } else { Vec::new() }
}
