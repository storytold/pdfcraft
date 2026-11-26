// PLAN: the `craft` dependency in crates/ui-egui/Cargo.toml needs the `text` feature for `craft::text` (owner: pdf-hub-w1).
use craft::behavior::kurbo::Point;
use craft::behavior::overlay::Align;
use craft::pages::ui::TextMark;
use craft::text::find::{FindMatches, FindOptions};
use craft::text::ui::find::{FindBar, FindBarState, FindRequest};
use craft::ui::canvas_controls::TaskBar;

use super::{DocView, Rect, ViewAction};

/// The find bar (⌘F): query, matches across the document, current match.
#[derive(Default)]
pub struct Find {
    pub query: String,
    /// Find options (Acrobat's: case-sensitive, whole words only).
    pub options: FindOptions,
    /// The matches in document order and the current one; it owns following the search's start page.
    pub matches: FindMatches,
    /// Whether the bar was just asked for (⌘F, Advanced Search), so its field takes the keyboard.
    pub bar: FindBarState,
    /// The query the matches belong to.
    pub case_query: String,
    /// Shown in the Search panel (Advanced Search) instead of the find bar.
    pub in_panel: bool,
    /// Find and replace (in Edit text mode): the Replace row shows, with what replaces a match.
    pub replacing: bool,
    pub replacement: String,
}

impl Find {
    /// The matches as highlights for the page view's text marks, the current one stronger.
    pub fn marks(&self) -> Vec<TextMark> {
        let current = self.matches.current();
        self.matches.matches().iter().enumerate().map(|(i, m)| TextMark::new(m.page, m.range.clone(), current == Some(i))).collect()
    }
}

impl DocView {
    /// Open the find bar (or focus it if already open).
    pub fn open_find(&mut self) {
        let f = self.find.get_or_insert_with(Find::default);
        f.bar.open();
    }

    /// Open the find bar with its Replace row (Edit text mode shows it): the Find field takes the
    /// keyboard while it is empty, else the Replace field.
    pub fn open_replace(&mut self) {
        let f = self.find.get_or_insert_with(Find::default);
        f.in_panel = false;
        f.replacing = true;
        f.bar.open_replace();
    }

    /// Re-run the search on all pages whose text is known (after the query changed).
    pub fn rerun_find(&mut self) {
        let current = self.view.current;
        let Some(f) = self.find.as_mut() else { return };
        f.case_query = f.query.clone();
        // Refining the query keeps the page the reader started searching from, even when an
        // automatic choice has already scrolled them to a match.
        let from = f.matches.from().unwrap_or(current);
        f.matches = FindMatches::new(from);
        if f.case_query.trim().is_empty() {
            return;
        }
        let mut pages: Vec<usize> = self.session.texts.keys().copied().collect();
        pages.sort();
        let mut reveal = false;
        for &p in &pages {
            if let Some(text) = self.session.texts.get(&p) {
                reveal |= f.matches.set_page(p, text.find_opts(&f.case_query, f.options.match_case, f.options.whole_word));
            }
        }
        let target = f.matches.current_match().map(|m| m.page).filter(|_| reveal);
        if let Some(p) = target {
            self.flash_match(p);
        }
    }

    pub(super) fn refresh_find_page(&mut self, page: usize) {
        let (Some(f), Some(text)) = (self.find.as_mut(), self.session.texts.get(&page)) else { return };
        if f.case_query.trim().is_empty() {
            return;
        }
        let reveal = f.matches.set_page(page, text.find_opts(&f.case_query, f.options.match_case, f.options.whole_word));
        let target = f.matches.current_match().map(|m| m.page).filter(|_| reveal);
        if let Some(p) = target {
            self.flash_match(p);
        }
    }

    /// Go to match `i` (the Search panel's results).
    pub fn go_to_match(&mut self, i: usize) {
        let Some(f) = self.find.as_mut() else { return };
        if i >= f.matches.len() {
            return;
        }
        f.matches.pick(i);
        self.show_current_match();
    }

    /// Move to the next/previous match.
    pub fn find_step(&mut self, forward: bool) {
        let Some(f) = self.find.as_mut() else { return };
        if f.matches.step(!forward).is_none() {
            return;
        }
        self.show_current_match();
    }

    /// Show the current match, which the matches already chose (the find bar and the Search panel
    /// step or pick it). Only explicit search navigation joins history; background refresh uses
    /// `flash_match` alone.
    pub(super) fn show_current_match(&mut self) {
        let Some(p) = self.find.as_ref().and_then(|f| f.matches.current_match()).map(|m| m.page) else { return };
        self.go_to_page(p);
        self.flash_match(p);
    }

    /// The centre of the current match in page points on its displayed page (from the top-left
    /// corner, after the view rotation), where the page view's go-to point takes it.
    fn match_centre(&self) -> Option<Point> {
        let m = self.find.as_ref()?.matches.current_match()?;
        let glyphs = self.session.texts.get(&m.page)?.glyphs.get(m.range.clone())?;
        let [x0, y0, x1, y1] = glyphs.iter().map(|g| g.rect).reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])])?;
        let &(w, h) = self.page_sizes.get(m.page).filter(|(w, h)| *w > 0.0 && *h > 0.0)?;
        let (w, h) = (f64::from(w), f64::from(h));
        let cx = (f64::from(x0 + x1) / 2.0).clamp(0.0, w);
        let cy = (f64::from(y0 + y1) / 2.0).clamp(0.0, h);
        // Turned clockwise by the view rotation.
        Some(match self.view.rotation % 360 {
            90 => Point::new(h - cy, cx),
            180 => Point::new(w - cx, h - cy),
            270 => Point::new(cy, w - cx),
            _ => Point::new(cx, cy),
        })
    }

    fn flash_match(&mut self, page: usize) {
        // Put the match's centre at the window's centre, as Acrobat does (#3530) and as a /FitR
        // destination's rectangle is: a page wider or taller than the window no longer leaves it
        // off to one side or near an edge.
        // Finite only: a hostile page's glyphs can span -inf..inf, and NaN survives `clamp`.
        if let Some(centre) = self.match_centre().filter(|c| c.x.is_finite() && c.y.is_finite()) {
            self.view.go_to_point(page, centre, (0.5, 0.5));
        } else {
            // The match line's top, centred on that axis alone (a NaN axis keeps its offset).
            let top = self
                .find
                .as_ref()
                .and_then(|f| f.matches.current_match())
                .and_then(|m| self.session.texts.get(&m.page).and_then(|t| t.glyphs.get(m.range.start)))
                .map(|g| f64::from(g.rect[1]))
                .zip(self.page_sizes.get(page).map(|&(_, h)| f64::from(h)))
                .filter(|(y, h)| y.is_finite() && h.is_finite() && *h > 0.0);
            match top {
                Some((y, h)) => self.view.go_to_point(page, Point::new(f64::NAN, y.clamp(0.0, h)), (0.5, 0.5)),
                None => self.view.go_to(page),
            }
        }
    }

    /// Ends automatic search following after the reader navigated or scrolled the view. A
    /// programmatic search flash never comes through here, so that its own scroll cannot be
    /// mistaken for the reader's.
    pub(super) fn end_find_following(&mut self) {
        if let Some(f) = self.find.as_mut() {
            f.matches.stop_following();
        }
    }
}

/// The find bar at the page view's top edge, end-aligned. In Edit text mode (`editing`) it offers a
/// Replace row: Replace changes the highlighted match, Replace all every match, each inside its own
/// paragraph.
pub(super) fn find_bar(view: &mut DocView, pages: usize, area: Rect, editing: bool, ui: &mut egui::Ui) {
    let searched = view.session.texts.len() + view.session.text_failed.len();
    let key = view.pane.key;
    let Some(find) = view.find.as_mut() else { return };
    // The Search panel shows this search.
    if find.in_panel {
        return;
    }
    // One id per pane, so two panes' bars do not share a field id (and so the focus).
    let shown = ui
        .push_id(("find-bar", key), |ui| {
            TaskBar::at_edge(tl!("Find"), Align::End).show(ui, area.shrink2(egui::vec2(6.0, 12.0)), |ui, _cx| {
                let Find { query, options, matches, bar, replacement, replacing, .. } = &mut *find;
                let mut find_bar = FindBar::new(query, options, matches).state(bar).progress(searched, pages);
                if editing {
                    find_bar = find_bar.replace(replacement, replacing);
                }
                find_bar.show(ui)
            })
        })
        .inner;
    let Some(out) = shown.inner else { return };
    if out.closed {
        view.find = None;
        return;
    }
    match out.request {
        Some(FindRequest::Replace { .. }) => view.pending_action = Some(ViewAction::Replace { all: false }),
        Some(FindRequest::ReplaceAll { .. }) => view.pending_action = Some(ViewAction::Replace { all: true }),
        _ => {}
    }
    if out.search {
        view.rerun_find();
    }
    if out.stepped {
        // The bar already stepped the matches.
        view.show_current_match();
    }
}
