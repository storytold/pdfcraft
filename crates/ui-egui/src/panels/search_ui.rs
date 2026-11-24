//! Edit ▸ Advanced Search (⇧⌘F): the search in a side panel that lists every match with its
//! context, page by page; click a result to go to it. It shares the find bar's search, and with
//! Include comments it also lists the comment threads that match.
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
use crate::canvas::find_comments::{CommentHit, excerpt_match};

/// Glyphs of context shown on each side of a match.
const CONTEXT: usize = 36;
/// The most results listed.
const MAX_RESULTS: usize = 2000;

/// A row of the results: a page's heading, a text match by its index in the search's matches, or a
/// comment thread by its index in the comment results.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Hit {
    Page(usize),
    Match(usize),
    Comment(usize),
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
    let Some(doc) = app.session.get(id) else { return result };
    let (info, hidden) = (&doc.info, doc.comments_hidden());
    let pages = info.pages.len();
    let Some(view) = app.views.get_mut(index) else { return result };
    let searched = view.session.texts.len() + view.session.text_failed.len();
    let Some(find) = view.find.as_mut() else { return result };
    find.in_panel = true;
    Text::new(tl!("What word or phrase would you like to search for?").to_string()).show(ui);
    let field = TextField::search(tl!("Search").to_string(), Bind::bound(&mut find.query)).placeholder(tl!("Search")).select_on_focus().show(ui);
    // The bar was asked for (⇧⌘F, also with the panel open): the field takes the focus once.
    if find.bar.taken() {
        focus::request(ui.ctx(), FocusTarget::Id(field.response.id));
    }
    let before = (find.options.whole_word, find.options.match_case);
    Row::new().show(ui, |ui, _| {
        Checkbox::new(Bind::bound(&mut find.options.whole_word)).label(tl!("Whole words only").to_string()).show(ui);
        Checkbox::new(Bind::bound(&mut find.options.match_case)).label(tl!("Case-sensitive").to_string()).show(ui);
    });
    Checkbox::new(Bind::bound(&mut find.include_comments)).label(tl!("Include comments").to_string()).show(ui);
    if before != (find.options.whole_word, find.options.match_case) {
        find.case_query.clear();
    }
    if find.query != find.case_query {
        view.rerun_find();
    }
    view.sync_comment_hits(info);
    let current_comment = view.current_comment();
    let Some(find) = view.find.as_ref() else { return result };
    let matches = find.matches.matches();
    let comments = &find.comment_hits;
    let status = if find.query.trim().is_empty() {
        String::new()
    } else if matches.is_empty() && comments.is_empty() && searched < pages {
        crate::i18n::fmt(tl!("Searching… {s} of {p} pages"), &[("s", &searched.to_string()), ("p", &pages.to_string())])
    } else {
        let n = matches.len().saturating_add(comments.len());
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
    let text = matches.iter().enumerate().filter(|(_, m)| view.session.texts.contains_key(&m.page)).map(|(i, m)| (m.page, Hit::Match(i)));
    let merged = in_page_order(text, comments.iter().enumerate().map(|(i, c)| (c.page, Hit::Comment(i))));
    let mut rows = Vec::new();
    let mut last_page = None;
    for (page, hit) in merged.take(MAX_RESULTS) {
        if last_page != Some(page) {
            rows.push(Hit::Page(page));
            last_page = Some(page);
        }
        rows.push(hit);
    }
    let at = rows.iter().cloned().enumerate().map(|(i, h)| (h, i)).collect();
    let results = Results { rows, at };
    let current = current_comment.map(Hit::Comment).or_else(|| find.matches.current().map(Hit::Match)).map(Selection::single).unwrap_or_default();
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
            Hit::Comment(i) => match comments.get(*i) {
                Some(hit) => comment_row(hit, &find.case_query, find.options.match_case, find.options.whole_word),
                None => ListRow::new(Label::Empty),
            },
        });
    let mut go = None;
    for event in &out.events {
        match event {
            CollectionEvent::Selection(SelectionChange::Set(keys)) => {
                if let Some(hit @ (Hit::Match(_) | Hit::Comment(_))) = keys.last() {
                    go = Some(hit.clone());
                }
            }
            CollectionEvent::Activate(hit @ (Hit::Match(_) | Hit::Comment(_))) => go = Some(hit.clone()),
            _ => {}
        }
    }
    match go {
        Some(Hit::Match(i)) => view.go_to_match(i),
        Some(Hit::Comment(i)) => view.go_to_comment(i, info, hidden),
        _ => {}
    }
    result
}

/// Text results and comment results, each by page, as one list: page text first on each page,
/// then its comments. It merges as it goes, so the caller's limit stops it however many there are.
fn in_page_order<T>(text: impl Iterator<Item = (usize, T)>, comments: impl Iterator<Item = (usize, T)>) -> impl Iterator<Item = (usize, T)> {
    let (mut text, mut comments) = (text.peekable(), comments.peekable());
    std::iter::from_fn(move || match (text.peek(), comments.peek()) {
        (Some((text_page, _)), Some((comment_page, _))) if comment_page < text_page => comments.next(),
        (Some(_), _) => text.next(),
        (None, _) => comments.next(),
    })
}

/// A comment result: its type, then the matched text with the match highlighted.
fn comment_row(hit: &CommentHit, query: &str, case_sensitive: bool, whole_words: bool) -> ListRow<'static> {
    let kind = format!("{}: ", tl!(hit.kind.as_str()));
    let name = format!("{kind}{}", hit.excerpt);
    let mut lines = vec![Span::new(kind).muted()];
    let parts = excerpt_match(&hit.excerpt, query, case_sensitive, whole_words).and_then(|range| {
        let (before, rest) = hit.excerpt.split_at_checked(range.start)?;
        let (found, after) = rest.split_at_checked(range.len())?;
        Some((before, found, after))
    });
    match parts {
        Some((before, found, after)) => {
            lines.extend([Span::new(before.to_owned()), Span::new(found.to_owned()).highlight().strong(), Span::new(after.to_owned())]);
        }
        None => lines.push(Span::new(hit.excerpt.clone())),
    }
    ListRow::new(Label::Empty).lines(lines).accessible_name(name).tooltip(tl!("Go to this comment").to_string())
}

#[cfg(test)]
mod tests {
    use super::in_page_order;

    #[test]
    fn page_text_comes_before_the_comments_on_its_page() {
        let text = [(0, "t0"), (2, "t2a"), (2, "t2b")];
        let comments = [(1, "c1"), (2, "c2"), (3, "c3")];
        let order: Vec<_> = in_page_order(text.into_iter(), comments.into_iter()).map(|(_, hit)| hit).collect();
        assert_eq!(order, ["t0", "c1", "t2a", "t2b", "c2", "c3"]);
    }

    #[test]
    fn the_merge_stops_at_the_limit() {
        let mut pulled = 0;
        let text = (0..usize::MAX).map(|page| {
            pulled += 1;
            (page, "t")
        });
        assert_eq!(in_page_order(text, std::iter::empty()).take(3).count(), 3);
        assert!(pulled <= 4, "{pulled} pulled");
    }
}
