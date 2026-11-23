//! A drop-down's option list under its field, as the framework's `PopupList` of document rows.

use craft::behavior::{DisplayOrder, ItemSource};
// PLAN: assumed `craft::ui::contract::CollectionEvent` re-export of the collection event enum.
use craft::ui::contract::{Bind, CollectionEvent, LeadOnOpen, Props as _, ShowCollection as _, ShowContainer as _, overlay::Anchor};
use craft::ui::fonts::FontRef;
use craft::ui::list::{DocumentRows, List, ListRow};
use craft::ui::popover::{PopupList, PopupListKind};
use craft::ui::primitives::HAlign;
use egui::Rect;
use pdfcraft_engine::FormField;

use super::field_text_size;

/// The option names a list shows, keyed by position: two options can share an export value.
struct Names<'a>(&'a [&'a str]);

impl DisplayOrder<usize> for Names<'_> {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn key(&self, at: usize) -> usize {
        at
    }

    fn index_of(&self, key: &usize) -> Option<usize> {
        (*key < self.0.len()).then_some(*key)
    }

    fn text(&self, at: usize) -> Option<std::borrow::Cow<'_, str>> {
        self.0.get(at).map(|name| std::borrow::Cow::Borrowed(*name))
    }
}

impl ItemSource<usize> for Names<'_> {}

/// The list of `field`'s options hung from `rect`, the widget's rectangle on screen, open while `open` is true. Its
/// rows are the options' shown names in the field's text size (`zoom` screen points per page point) and alignment.
/// Returns the position of the option activated this pass.
pub(super) fn choice_list(ui: &mut egui::Ui, field: &FormField, rect: Rect, zoom: f32, open: &mut bool) -> Option<usize> {
    // An auto size is fitted to the field's first widget: the twins of one field share its text size.
    let widget = field.widgets.first()?;
    // Auto-sized text is fitted to what the field shows (its chosen option's shown name), so the rows come out at the
    // size the field's value is drawn at.
    let value_text = field.value.first().map_or("", |v| field.options.iter().find(|(e, _)| e == v).map_or(v.as_str(), |(_, d)| d.as_str()));
    let size = field_text_size(ui.painter(), field, widget, value_text) * zoom;
    let align = match field.quadding {
        1 => HAlign::Center,
        2 => HAlign::End,
        _ => HAlign::Start,
    };
    let names: Vec<&str> = field.options.iter().map(|(_, shown)| shown.as_str()).collect();
    let source = Names(&names);
    // The popup names its list: the framework routes the field's list keys (arrows, End, Enter) to a named list only.
    let list = ui.scope_id().with("pdfcraft.choice-list");
    let shown = PopupList::new(tl!("Options"), Bind::bound(&mut *open))
        .kind(PopupListKind::Document)
        .overlay(|overlay| {
            let dismiss = overlay.dismiss.focus_left(false);
            overlay.anchor(Anchor::Rect(rect)).dismiss(dismiss)
        })
        .list(list)
        .show(ui, |ui, _| {
            List::<usize>::options(tl!("Options"))
                .id(list)
                .lead_on_open(LeadOnOpen::Selected)
                .document_rows(DocumentRows::new(FontRef::proportional(), size, align))
                .show_items(ui, &source, &mut |key, _| ListRow::new(names.get(*key).copied().unwrap_or_default().to_owned()).option())
                .events
        });
    let mut picked = None;
    for event in shown.inner.into_iter().flatten() {
        if let CollectionEvent::Activate(key) = event {
            picked = Some(key);
        }
    }
    picked
}
