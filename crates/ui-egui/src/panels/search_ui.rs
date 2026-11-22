//! Edit ▸ Advanced Search (⇧⌘F): the search in a side panel that lists every match with its
//! context, page by page; click a result to go to it. It shares the find bar's search.
// PLAN: assumed `super::right::PanelResult` (pdf-chrome-w13).

use std::collections::HashMap;

use craft::behavior::{DisplayOrder, ItemSource, Selection, SelectionChange};
use craft::ui::choice::Checkbox;
use craft::ui::contract::{Bind, CollectionEvent, Props as _, SelectMode, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::display::Text;
use craft::ui::layout_components::Row;
use craft::ui::list::{List, ListRow};
use craft::ui::runtime::env::Label;
use craft::ui::runtime::focus::{self, FocusTarget};
use craft::ui::style::Span;
use craft::ui::text::TextField;

use super::right::PanelResult;
use crate::PdfCraftApp;

/// Glyphs of context shown on each side of a match.
const CONTEXT: usize = 36;
/// The most results listed.
const MAX_RESULTS: usize = 2000;

/// A row of the results: a page's heading or a match by its index in the search's matches.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Hit {
    Page(usize),
    Match(usize),
}

/// The results in display order.
struct Results {
    rows: Vec<Hit>,
    at: HashMap<Hit, usize>,
}

impl DisplayOrder<Hit> for Results {
    fn len(&self) -> usize {
        self.rows.len()
    }

    fn key(&self, at: usize) -> Hit {
        self.rows.get(at).cloned().unwrap_or(Hit::Page(0))
    }

    fn index_of(&self, key: &Hit) -> Option<usize> {
        self.at.get(key).copied()
    }
}

impl ItemSource<Hit> for Results {}

/// Draw the Search panel with its query, options, progress and clickable result excerpts.
pub(crate) fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) -> PanelResult {
    let result = PanelResult::default();
    let Some((index, id)) = app.active_ids() else { return result };
    let pages = app.session.get(id).map_or(0, |d| d.info.pages.len());
    let Some(view) = app.views.get_mut(index) else { return result };
    let searched = view.session.texts.len() + view.session.text_failed.len();
    let Some(find) = view.find.as_mut() else { return result };
    find.in_panel = true;
    Text::new(tl!("What word or phrase would you like to search for?").to_string()).show(ui);
    let field = TextField::search(tl!("Search").to_string(), Bind::bound(&mut find.query))
        .placeholder(tl!("Search"))
        .select_on_focus()
        .show(ui);
    // The bar was asked for (⇧⌘F, also with the panel open): the field takes the focus once.
    if find.bar.taken() {
        focus::request(ui.ctx(), FocusTarget::Id(field.response.id));
    }
    let before = (find.options.whole_word, find.options.match_case);
    Row::new().show(ui, |ui, _| {
        Checkbox::new(Bind::bound(&mut find.options.whole_word)).label(tl!("Whole words only").to_string()).show(ui);
        Checkbox::new(Bind::bound(&mut find.options.match_case)).label(tl!("Case-sensitive").to_string()).show(ui);
    });
    if before != (find.options.whole_word, find.options.match_case) {
        find.case_query.clear();
    }
    if find.query != find.case_query {
        view.rerun_find();
    }
    let Some(find) = view.find.as_ref() else { return result };
    let matches = find.matches.matches();
    let status = if find.query.trim().is_empty() {
        String::new()
    } else if matches.is_empty() && searched < pages {
        crate::i18n::fmt(tl!("Searching… {s} of {p} pages"), &[("s", &searched.to_string()), ("p", &pages.to_string())])
    } else {
        let n = matches.len();
        let more = if searched < pages {
            crate::i18n::fmt(tl!(" (searching… {s} of {p} pages)"), &[("s", &searched.to_string()), ("p", &pages.to_string())])
        } else {
            String::new()
        };
        // English keeps its singular; Chinese has no plural.
        let count = if n == 1 { crate::i18n::fmt(tl!("1 instance"), &[]) } else { crate::i18n::fmt(tl!("{n} instances"), &[("n", &n.to_string())]) };
        format!("{count}{more}")
    };
    if !status.is_empty() {
        Text::new(status).show(ui);
    }
    let mut rows = Vec::new();
    let mut last_page = None;
    for (i, m) in matches.iter().enumerate().take(MAX_RESULTS) {
        if !view.session.texts.contains_key(&m.page) {
            continue;
        }
        if last_page != Some(m.page) {
            rows.push(Hit::Page(m.page));
            last_page = Some(m.page);
        }
        rows.push(Hit::Match(i));
    }
    let at = rows.iter().cloned().enumerate().map(|(i, h)| (h, i)).collect();
    let results = Results { rows, at };
    let current = find.matches.current().map(|i| Selection::single(Hit::Match(i))).unwrap_or_default();
    let flat = |s: String| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let out = List::<Hit>::new(tl!("Search results").to_string())
        .select(SelectMode::Single)
        .selection(Bind::given(current))
        .natural_height(true)
        .show_items(ui, &results, &mut |key, _| match key {
            Hit::Page(p) => ListRow::new(crate::i18n::fmt(tl!("Page {label}"), &[("label", &(p + 1).to_string())])).section_heading(),
            Hit::Match(i) => {
                let Some(m) = matches.get(*i) else { return ListRow::new(Label::Empty) };
                let Some(text) = view.session.texts.get(&m.page) else { return ListRow::new(Label::Empty) };
                let n = text.glyphs.len();
                let range = &m.range;
                let before = flat(text.text_of(range.start.saturating_sub(CONTEXT)..range.start.min(n)));
                let hit = flat(text.text_of(range.start.min(n)..range.end.min(n)));
                let after = flat(text.text_of(range.end.min(n)..range.end.saturating_add(CONTEXT).min(n)));
                let name = format!("…{before} {hit} {after}…");
                ListRow::new(Label::Empty)
                    .lines(vec![
                        Span::new(format!("…{before} ")).muted(),
                        Span::new(hit).highlight().strong(),
                        Span::new(format!(" {after}…")).muted(),
                    ])
                    .accessible_name(name)
                    .tooltip(tl!("Go to this result").to_string())
            }
        });
    let mut go = None;
    for event in &out.events {
        match event {
            CollectionEvent::Selection(SelectionChange::Set(keys)) => {
                if let Some(Hit::Match(i)) = keys.last() {
                    go = Some(*i);
                }
            }
            CollectionEvent::Activate(Hit::Match(i)) => go = Some(*i),
            _ => {}
        }
    }
    if let Some(i) = go {
        view.go_to_match(i);
    }
    result
}
