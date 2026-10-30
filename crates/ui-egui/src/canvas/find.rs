use pdfcraft_render::DocInfo;

use super::find_comments;
pub(super) use super::find_comments::FindComment;
pub use super::find_comments::FindHit;
use super::{Align2, Color32, CornerRadius, DocView, Rect, Stroke, Tokens, icons, theme, vec2, widgets};

/// The find bar (⌘F): query, matches across the document, current match.
#[derive(Default)]
pub struct Find {
    pub query: String,
    /// Text and comment hits in document order: a page's text first, then its comments.
    pub matches: Vec<FindHit>,
    pub current: Option<usize>,
    /// The page a search started from while its match is picked automatically: a text layer
    /// that arrives out of order can still supply an earlier match, and the automatic choice
    /// follows this page. Cleared when the user picks a match or moves the view themselves.
    pub auto_from: Option<usize>,
    /// The page the automatic choice last put the view on (the search's starting page until it
    /// chose anything). Compared with the viewed page, it tells an automatic scroll from one
    /// the reader made themselves; `None` with `auto_from` once the choice is the user's.
    pub auto_page: Option<usize>,
    /// The reader ended automatic following (by navigating, scrolling, or picking a match).
    /// This is distinct from a search that never followed: a late match must be listed without
    /// being revealed by scrolling, even when the reader moved before the first match arrived.
    pub follow_ended: bool,
    pub focus: bool,
    pub case_query: String,
    /// Find options (Acrobat's: case-sensitive, whole words only, include comments).
    pub case_sensitive: bool,
    pub whole_words: bool,
    /// Also search comments. Off until the reader turns it on, as in Acrobat's Find options.
    pub include_comments: bool,
    /// Shown in the Search panel (Advanced Search) instead of the find bar.
    pub in_panel: bool,
}

/// What a text layer arriving after a search began may do with the automatic choice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Follow {
    /// The reader has not moved the view since the automatic choice: it may follow `from`.
    Active(usize),
    /// The reader navigated, scrolled or picked a match, so no automatic choice may reveal.
    Ended,
    /// No automatic choice is being followed (the search has not started, or the reader chose).
    Never,
}

impl Find {
    /// The first match at or after `page`, or the first match when none follows (the search
    /// wraps round the document).
    pub(super) fn first_at_or_after(&self, page: usize) -> Option<usize> {
        self.matches.iter().position(|hit| hit.page() >= page).or_else(|| (!self.matches.is_empty()).then_some(0))
    }

    /// How a text layer arriving now relates to the automatic choice. Once the reader has moved
    /// the view (page box, Pages panel, outline, view history or any scrolling), automatic
    /// following ends: a later layer must not pull them back to an anchor-derived match.
    /// `Ended` is deliberately distinct from `Never`, so a first match arriving after that move
    /// is not revealed by a flash meant only for a search that is still following.
    pub(super) fn follow_page(&mut self, viewed: usize) -> Follow {
        if self.follow_ended {
            return Follow::Ended;
        }
        match (self.auto_from, self.auto_page) {
            (Some(from), Some(page)) if page == viewed => Follow::Active(from),
            (Some(_), _) | (_, Some(_)) => {
                self.stop_following();
                Follow::Ended
            }
            (None, None) => Follow::Never,
        }
    }

    /// Ends automatic following: the selected match is now the user's choice to keep.
    fn stop_following(&mut self) {
        self.auto_from = None;
        self.auto_page = None;
        self.follow_ended = true;
    }
}

impl DocView {
    /// Open the find bar (or focus it if already open).
    pub fn open_find(&mut self) {
        let f = self.find.get_or_insert_with(Find::default);
        f.focus = true;
    }

    /// Re-run the search on all pages whose text is known (after the query changed).
    pub fn rerun_find(&mut self) {
        let Some(f) = self.find.as_mut() else { return };
        f.case_query = f.query.clone();
        f.matches.clear();
        f.current = None;
        // Refining the query keeps the page the reader started searching from, even when an
        // automatic choice has already scrolled them to a match. Only a view the reader moved
        // themselves re-anchors the search on the page now shown.
        let from = match (f.auto_from, f.auto_page) {
            (Some(from), Some(page)) if !f.follow_ended && page == self.current => from,
            _ => self.current,
        };
        f.auto_from = Some(from);
        f.auto_page = Some(self.current);
        f.follow_ended = false;
        if f.case_query.trim().is_empty() {
            return;
        }
        let mut pages: Vec<usize> = self.texts.keys().copied().collect();
        pages.sort();
        for &p in &pages {
            if let Some(text) = self.texts.get(&p) {
                f.matches.extend(text_hits(p, text, &f.case_query, f.case_sensitive, f.whole_words));
            }
        }
        if f.include_comments {
            f.matches.extend(find_comments::comment_hits(&self.find_comments, &f.case_query, f.case_sensitive, f.whole_words));
        }
        f.matches.sort_by_key(FindHit::sort_key);
        // First match at or after the page the search started from.
        if let Some(i) = f.first_at_or_after(from)
            && let Some(p) = f.matches.get(i).map(FindHit::page)
        {
            f.current = Some(i);
            f.auto_page = Some(p);
            self.flash_match(p);
        }
    }

    pub(super) fn refresh_find_page(&mut self, page: usize) {
        let (Some(f), Some(text)) = (self.find.as_mut(), self.texts.get(&page)) else { return };
        if f.case_query.trim().is_empty() {
            return;
        }
        let previous = f.current.and_then(|c| f.matches.get(c).cloned());
        // An automatic choice follows the page it was anchored on, so a page whose text
        // arrives late can still supply an earlier match. Once the reader has moved the view,
        // this arrival must not pull them back; the choice then becomes theirs to keep.
        let follow = f.follow_page(self.current);
        // Comment hits do not come from the text layer, so a late or failed page keeps them.
        f.matches.retain(|hit| !hit.text_on(page));
        f.matches.extend(text_hits(page, text, &f.case_query, f.case_sensitive, f.whole_words));
        f.matches.sort_by_key(FindHit::sort_key);
        if let Follow::Active(from) = follow {
            f.current = f.first_at_or_after(from);
            let selected = f.current.and_then(|i| f.matches.get(i).cloned());
            // Follow a changed automatic choice; the view is still where the last one left it.
            if selected != previous
                && let Some(p) = selected.as_ref().map(FindHit::page)
            {
                f.auto_page = Some(p);
                self.flash_match(p);
            }
            return;
        }
        // A match the user picked keeps its identity, exactly as before automatic following.
        // When it is gone (or none was picked), the first match at or after the viewed page is
        // selected for the panel, but the view stays put: `Ended` and `Never` have no automatic
        // choice to reveal, and a reader who moved before the first layer arrived must not be
        // pulled back by that late match.
        let preserved = previous.as_ref().and_then(|k| f.matches.iter().position(|m| m == k));
        f.current = preserved.or_else(|| f.first_at_or_after(self.current));
    }

    /// Go to match `i` (the Search panel's results).
    pub fn go_to_match(&mut self, i: usize) {
        let Some(f) = self.find.as_mut() else { return };
        let Some(p) = f.matches.get(i).map(FindHit::page) else { return };
        f.current = Some(i);
        f.stop_following();
        self.go_to_page(p);
        self.goto_point = None;
        self.flash_match(p);
    }

    /// Move to the next/previous match.
    pub fn find_step(&mut self, forward: bool) {
        let Some(f) = self.find.as_mut() else { return };
        if f.matches.is_empty() {
            return;
        }
        let n = f.matches.len();
        let c = f.current.unwrap_or(0);
        let next = if forward { (c + 1) % n } else { (c + n - 1) % n };
        f.current = Some(next);
        f.stop_following();
        let Some(p) = f.matches.get(next).map(FindHit::page) else { return };
        // Only explicit search navigation joins history; background refresh uses flash_match alone.
        self.go_to_page(p);
        self.goto_point = None;
        self.flash_match(p);
    }

    fn flash_match(&mut self, page: usize) {
        let y = self.find.as_ref().and_then(|f| f.current.and_then(|c| f.matches.get(c))).and_then(|hit| match hit {
            FindHit::Text { page: p, range } => self.texts.get(p).and_then(|t| t.glyphs.get(range.start)).map(|g| g.rect[1]),
            FindHit::Comment { top, .. } => Some(*top),
        });
        let frac = y.zip(self.page_heights.get(page).copied()).map(|(y, h)| ((y - 60.0) / h).clamp(0.0, 1.0)).unwrap_or(0.0);
        self.goto = Some((page, frac));
        self.current = page;
        self.reset_coarse_state();
        self.show_comment_hit();
    }

    /// A comment hit selects that thread and outlines it. A text hit, while comments are included,
    /// leaves the previous comment so the highlight on the page is the match.
    fn show_comment_hit(&mut self) {
        if !self.find.as_ref().is_some_and(|f| f.include_comments) {
            return;
        }
        let hit = self.find.as_ref().and_then(|f| f.current.and_then(|c| f.matches.get(c))).cloned();
        match hit {
            Some(FindHit::Comment { page, index, rect, .. }) => {
                self.comments.selected = Some((page, index));
                self.comments.reveal = true;
                self.flash = Some((page, rect, 0.0));
            }
            Some(FindHit::Text { .. }) => {
                self.comments.selected = None;
                self.flash = None;
            }
            None => {}
        }
    }

    /// Replace the comment index Find searches. A real change, with Include comments on and a
    /// query typed, searches again on the next find-bar pass.
    pub(super) fn set_find_comments(&mut self, next: Vec<FindComment>) {
        if self.find_comments == next {
            return;
        }
        self.find_comments = next;
        if self.find.as_ref().is_some_and(|f| f.include_comments && !f.query.trim().is_empty())
            && let Some(f) = self.find.as_mut()
        {
            f.case_query.clear();
        }
    }

    /// Rebuild the comment index from the open document.
    pub(crate) fn sync_find_comments(&mut self, info: &DocInfo) {
        self.set_find_comments(find_comments::comment_find_index(info));
    }

    /// Ends automatic search following after the reader navigated or scrolled the view. A
    /// programmatic search flash never comes through here; it sets `goto` and `current` directly
    /// so that its own scroll cannot be mistaken for the reader's.
    pub(super) fn end_find_following(&mut self) {
        if let Some(f) = self.find.as_mut() {
            f.stop_following();
        }
    }
}

/// Acrobat-style floating find bar at the top-right of the document area.
pub(super) fn find_bar(view: &mut DocView, pages: usize, area: Rect, ui: &mut egui::Ui, t: &Tokens) {
    let Some(find) = view.find.as_mut() else { return };
    // The Search panel shows this search.
    if find.in_panel {
        return;
    }
    let mut close = false;
    let mut step: Option<bool> = None;
    let searched = view.texts.len() + view.text_failed.len();
    egui::Area::new(egui::Id::unique("find-bar"))
        .order(egui::Order::Middle)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(area.right_top() + vec2(-18.0, 12.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::NONE
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(10))
                .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(if t.dark() { 90 } else { 30 }) })
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(icons::image("search", 16.0, t.text_muted));
                        let edit = egui::TextEdit::singleline(&mut find.query)
                            .id(egui::Id::unique("find-input"))
                            .hint_text(tl!("Find text"))
                            .desired_width(220.0)
                            .frame(egui::Frame::NONE);
                        let r = ui.add(edit).accessible_name(tl!("Find text"));
                        if find.focus {
                            r.request_focus();
                            find.focus = false;
                        }
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            step = Some(!ui.input(|i| i.modifiers.shift));
                            r.request_focus();
                        }
                        if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            close = true;
                        }
                        let status = if find.query.trim().is_empty() {
                            String::new()
                        } else if find.matches.is_empty() {
                            if searched < pages {
                                crate::i18n::fmt(tl!("Searching… {s}/{p}"), &[("s", &searched.to_string()), ("p", &pages.to_string())])
                            } else {
                                tl!("No matches").to_string()
                            }
                        } else {
                            let more = if searched < pages { "+" } else { "" };
                            crate::i18n::fmt(
                                tl!("{c} of {n}{more}"),
                                &[
                                    ("c", &find.current.map(|c| c + 1).unwrap_or(0).to_string()),
                                    ("n", &find.matches.len().to_string()),
                                    ("more", more),
                                ],
                            )
                        };
                        ui.label(egui::RichText::new(status).font(theme::regular(12.0)).color(t.text_muted));
                        let tip = crate::commands::key_tip(ui.ctx(), tl!("Previous ({key})"), crate::commands::FIND_PREV);
                        if icons::button(ui, "chevron-up", 26.0, false, &tip).clicked() {
                            step = Some(false);
                        }
                        let tip = crate::commands::key_tip(ui.ctx(), tl!("Next ({key})"), crate::commands::FIND_NEXT);
                        if icons::button(ui, "chevron-down", 26.0, false, &tip).clicked() {
                            step = Some(true);
                        }
                        let opts_on = find.case_sensitive || find.whole_words || find.include_comments;
                        let opts = icons::button(ui, "settings-2", 26.0, opts_on, tl!("Find options"));
                        egui::Popup::menu(&opts).show(|ui| {
                            let a = ui.add(widgets::checkbox(&mut find.whole_words, tl!("Whole words only"))).changed();
                            let b = ui.add(widgets::checkbox(&mut find.case_sensitive, tl!("Case-sensitive"))).changed();
                            let c = ui.add(widgets::checkbox(&mut find.include_comments, tl!("Include comments"))).changed();
                            if a || b || c {
                                // Search again with the new options.
                                find.case_query.clear();
                            }
                        });
                        if icons::button(ui, "x", 26.0, false, tl!("Close (Esc)")).clicked() {
                            close = true;
                        }
                    });
                });
        });
    if close {
        view.find = None;
        return;
    }
    if find.query != find.case_query {
        view.rerun_find();
    }
    if let Some(forward) = step {
        view.find_step(forward);
    }
}

fn text_hits(page: usize, text: &pdfcraft_render::PageText, query: &str, case_sensitive: bool, whole_words: bool) -> Vec<FindHit> {
    text.find_opts(query, case_sensitive, whole_words).into_iter().map(|range| FindHit::text(page, range)).collect()
}
