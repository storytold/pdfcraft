//! The expanded All tools view: every tool, filed under Acrobat-style categories, with a search
//! field, filling the workspace instead of the sidebar. The sidebar's expand button opens it;
//! its collapse button, Esc or choosing a tool closes it again.

use std::hash::{Hash, Hasher};

use craft::behavior::{IconId, Selection};
use craft::i18n::matcher::Matcher;
use craft::keymap::CommandRef;
use craft::ui::contract::{ActivateOn, Bind, CollectionEvent, Props as _, SelectMode, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::primitives::IconSpec;
use craft::ui::runtime::focus::FocusRequest;
use craft::ui::sections::{HeaderCommand, Panel};
use craft::ui::style::DataColor;
use craft::ui::text::TextField;
use craft::ui::tile_grid::{BadgeKind, BadgeSlot, Tile, TileBadge, TileGrid, TileGroup};
use egui::Color32;
use pdfcraft_engine::catalog::{self, Availability, TOOL_CATEGORIES, ToolGroup};

use super::tools::Rows;
use crate::PdfCraftApp;

/// Item names a tile lists when the search found the tool by them.
const MATCHES_SHOWN: usize = 2;

/// Whether `group` matches the query `matcher` holds, in English or the UI language. `None` is no
/// match; otherwise the items that matched, empty when the tool's own name did (or the query is
/// empty). An entry with a menu (Watermark: Add, Update, Remove) also matches by its commands'
/// names, so "remove watermark" finds it.
fn matches(matcher: &Matcher, group: &ToolGroup) -> Option<Vec<&'static str>> {
    let hit = |label: &str| matcher.score(label).is_some() || matcher.score(tl!(label)).is_some();
    if hit(group.label) {
        return Some(Vec::new());
    }
    let action_hit = |id: &&str| {
        pdfcraft_engine::commands::command(id)
            .is_some_and(|spec| matcher.score(spec.label).is_some() || matcher.score(&crate::i18n::menu_label(spec.id, spec.label)).is_some())
    };
    let items: Vec<_> =
        group.sections.iter().flat_map(|s| s.items.iter()).filter(|i| hit(i.label) || i.actions.iter().any(action_hit)).map(|i| i.label).collect();
    (!items.is_empty()).then_some(items)
}

/// Identifies the set of tiles a query shows, so the grid's lead starts over when it changes.
fn query_stamp(query: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    query.hash(&mut hasher);
    hasher.finish()
}

/// One tool's tile: its icon and name, then the `items` the search found it by or, when there are
/// none, its availability (a planned tool is dimmed with its milestone).
fn tile(g: &'static ToolGroup, items: &[&'static str]) -> Tile<'static> {
    let mut tile = Tile::new(tl!(g.label).to_owned())
        .icon(IconSpec::svg(IconId::new(g.icon)))
        .hue(DataColor::new(Color32::from_rgb(g.hue[0], g.hue[1], g.hue[2])));
    // What found the tool, or else whether it works today.
    let note = if items.is_empty() {
        match g.availability {
            Availability::Ready => String::new(),
            Availability::Planned(m) => format!("{} · {m}", tl!("Planned")),
            Availability::Provider => g.badge.map(|b| tl!(b).to_owned()).unwrap_or_default(),
        }
    } else {
        items.iter().take(MATCHES_SHOWN).map(|l| tl!(l)).collect::<Vec<_>>().join(" · ")
    };
    if !note.is_empty() {
        tile = tile.detail(note);
    }
    if g.availability != Availability::Ready {
        return tile.dimmed();
    }
    match g.badge {
        Some(b) => {
            tile.badge(TileBadge::new(BadgeSlot::EndTop, BadgeKind::Text(tl!(b).to_owned().into()), craft::theme::variant::Tone::Accent, None))
        }
        None => tile,
    }
}

/// Draw the view over the workspace: the title with Collapse, the search field, and each
/// category's tiles that match the search. Choosing a tile opens that tool's panel.
pub fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    let mut chosen = None;
    let mut escape = false;
    Panel::new(tl!("All tools").to_owned())
        .titled()
        .commands([HeaderCommand::new(IconId::new("minimize-2"), CommandRef::new("tools.collapse"))])
        .show(ui, |ui, cx| {
            // The search field takes the keys whenever nothing else holds them, as it did when the view opened.
            TextField::search(tl!("Search tools and commands…").to_owned(), Bind::bound(&mut app.all_tools_query))
                .request_focus(FocusRequest::WhenNone)
                .show(ui);
            let query = app.all_tools_query.trim().to_owned();
            let matcher = Matcher::new(&craft::i18n::Folder::new(crate::i18n::current().code()), &query);

            let mut shown: Vec<(&'static ToolGroup, Vec<&'static str>)> = Vec::new();
            let mut spans = Vec::new();
            for category in TOOL_CATEGORIES {
                let start = shown.len();
                shown.extend(category.groups.iter().filter_map(|id| catalog::group(id)).filter_map(|g| matches(&matcher, g).map(|m| (g, m))));
                if shown.len() > start {
                    spans.push((start, shown.len() - start, tl!(category.label).to_owned()));
                }
            }
            // The header rows are keyed after the tiles, whose keys are their positions.
            let groups: Vec<TileGroup<usize>> =
                spans.into_iter().enumerate().map(|(at, (start, count, title))| TileGroup::new(shown.len() + at, start, count, title)).collect();
            // The panel's body scrolls as a whole, so the grid takes the height of its rows and owns no scroll.
            let grid = TileGrid::<usize>::new(tl!("All tools").to_owned())
                .natural_height(true)
                .select(SelectMode::Single)
                .activate(ActivateOn::Click)
                .card()
                .groups(&groups)
                .selection(Bind::given(Selection::default()))
                .rows_stamp(query_stamp(&query))
                .empty(tl!("No tools match your search.").to_owned())
                .show_items(ui, &Rows(shown.len()), &mut |at, _| match shown.get(*at) {
                    Some((g, items)) => tile(g, items),
                    None => Tile::new(String::new()),
                });
            for event in &grid.response.events {
                if let CollectionEvent::Activate(at) = event
                    && let Some((g, _)) = shown.get(*at)
                {
                    chosen = Some(g.id);
                }
            }
            if !app.modal_open() && !app.palette_open {
                escape = cx.take_cancel();
            }
        });
    if escape {
        app.all_tools_full = false;
    }
    if let Some(id) = chosen {
        app.set_left_open(true);
        app.select_tool(ui.ctx(), id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft::i18n::Folder;

    fn group(id: &str) -> &'static ToolGroup {
        catalog::group(id).expect("catalogue group")
    }

    fn find(query: &str, group: &ToolGroup) -> Option<Vec<&'static str>> {
        matches(&Matcher::new(&Folder::new(crate::i18n::current().code()), query), group)
    }

    #[test]
    fn search_matches_tool_names_and_their_items() {
        assert_eq!(find("", group("organize")), Some(Vec::new()), "an empty query shows everything");
        assert_eq!(find("organize", group("organize")), Some(Vec::new()), "the tool's own name");
        let rotate = find("rotate", group("organize")).expect("Organize pages lists Rotate");
        assert!(!rotate.is_empty() && rotate.iter().all(|l| l.to_lowercase().contains("rotate")), "{rotate:?}");
        assert_eq!(find("no such tool anywhere", group("organize")), None);
    }

    #[test]
    fn search_finds_an_entry_by_its_menus_commands() {
        let found = find("remove watermark", group("edit")).expect("Watermark's menu has Remove watermark");
        assert!(found.contains(&"Watermark"), "{found:?}");
    }

    #[test]
    fn a_tool_is_found_by_its_english_name_in_another_language() {
        if !crate::i18n::needs_catalogs() {
            return;
        }
        let french = crate::i18n::Lang::from_code("fr").expect("French is registered");
        let before = crate::i18n::current();
        crate::i18n::set_current(french);
        let organize = group("organize");
        let translated = crate::i18n::tr(french, organize.label);
        let by_english = find(organize.label, organize);
        let by_translation = find(translated, organize);
        let unrelated = find("zzzz", organize);
        crate::i18n::set_current(before);
        assert_eq!(by_english, Some(Vec::new()), "the English name still finds the tool");
        assert_eq!(by_translation, Some(Vec::new()), "and so does its translation");
        assert_eq!(unrelated, None);
    }
}
