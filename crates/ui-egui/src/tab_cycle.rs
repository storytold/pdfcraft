//! The order Next tab and Previous tab (Ctrl+Tab, Ctrl+Shift+Tab) go through the tabs (#2063):
//! their order on screen, as browsers do, or the order they were last shown in, as Windows'
//! Alt+Tab does.
//!
//! [`RecentTabs`] keeps the tabs most recently shown first. While Ctrl is held the order of a
//! cycle stays as it was at the first press, so pressing Tab again goes on to the tab shown
//! before that one; letting go of Ctrl puts the tab the cycle ended on first.

use pdfcraft_engine::DocId;

/// Preferences ▸ Documents and view: which order Next tab and Previous tab follow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabCycle {
    /// The tabs' order on screen, wrapping around.
    #[default]
    Tabs,
    /// The tab shown most recently first.
    Recent,
}

impl TabCycle {
    /// Every order, as Preferences lists them.
    pub const ALL: [Self; 2] = [Self::Tabs, Self::Recent];

    /// The name in the settings file and for `set tab_cycle` (the control channel).
    pub fn token(self) -> &'static str {
        match self {
            Self::Tabs => "tabs",
            Self::Recent => "recent",
        }
    }

    /// The order a settings value or `set tab_cycle` names ([`Self::token`], any case); `None`
    /// for anything else, which leaves the setting as it was.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.token().eq_ignore_ascii_case(value))
    }
}

/// A tab: an open document, or the Combine files tab. The Home tab takes no part in the cycle,
/// as it takes none in the tab-order one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKey {
    Doc(DocId),
    Combine,
}

/// The tabs, most recently shown first (see the module comment).
#[derive(Debug, Default)]
pub struct RecentTabs {
    /// Every open tab, most recently shown first.
    order: Vec<TabKey>,
    /// The order of the cycle in progress, fixed at its first step while Ctrl is held.
    cycle: Option<Vec<TabKey>>,
    /// Whether Ctrl was held at the last [`Self::note`]: without it every step is a cycle of its
    /// own, even two in one frame.
    held: bool,
}

impl RecentTabs {
    /// Called every frame with the tab showing (`None`: Home), the open tabs in their order on
    /// screen, and whether Ctrl is held. Outside a cycle, the tab showing becomes the most
    /// recent; a cycle ends when Ctrl is let go, and the tab it ended on becomes the most recent.
    pub fn note(&mut self, showing: Option<TabKey>, open: &[TabKey], held: bool) {
        self.held = held;
        if held && self.cycle.is_some() {
            return;
        }
        self.cycle = None;
        settle(&mut self.order, open);
        if let Some(showing) = showing.filter(|s| open.contains(s)) {
            self.order.retain(|t| *t != showing);
            self.order.insert(0, showing);
        }
    }

    /// The tab Next tab (`forward`) or Previous tab goes to, most recently shown first: the
    /// first step starts a cycle (from Home, Next tab goes to the most recent tab). `None` when
    /// no tab is open.
    pub fn step(&mut self, showing: Option<TabKey>, open: &[TabKey], forward: bool) -> Option<TabKey> {
        // Without Ctrl the tab showing is the most recent now, though no frame has noted it yet.
        if !self.held {
            self.note(showing, open, false);
        }
        let order = &self.order;
        let tabs = self.cycle.get_or_insert_with(|| order.clone());
        // A tab closed during the cycle drops out of it; one opened during it comes last.
        settle(tabs, open);
        let last = tabs.len().checked_sub(1)?;
        let next = match (showing.and_then(|s| tabs.iter().position(|t| *t == s)), forward) {
            (None, true) => 0,
            (None, false) => last,
            (Some(i), true) if i >= last => 0,
            (Some(i), true) => i + 1,
            (Some(i), false) => i.checked_sub(1).unwrap_or(last),
        };
        tabs.get(next).copied()
    }
}

/// Keep only the tabs in `open`, and add the open ones `tabs` lacks (opened but never shown, a
/// file opened in the background) at the end, in their order on screen.
fn settle(tabs: &mut Vec<TabKey>, open: &[TabKey]) {
    tabs.retain(|t| open.contains(t));
    for t in open {
        if !tabs.contains(t) {
            tabs.push(*t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(n: u64) -> TabKey {
        TabKey::Doc(DocId(n))
    }

    /// Shows `tabs` one after another, each on a frame of its own with Ctrl up.
    fn show(r: &mut RecentTabs, open: &[TabKey], tabs: &[TabKey]) {
        for t in tabs {
            r.note(Some(*t), open, false);
        }
    }

    /// The example of #2063: five documents, switching between the first and the fourth. One
    /// press goes back to the other of the two, whichever way.
    #[test]
    fn one_press_goes_back_to_the_tab_shown_before() {
        let open = [doc(1), doc(2), doc(3), doc(4), doc(5)];
        let mut r = RecentTabs::default();
        show(&mut r, &open, &[doc(5), doc(1), doc(4)]);
        r.note(Some(doc(4)), &open, true);
        assert_eq!(r.step(Some(doc(4)), &open, true), Some(doc(1)));
        // Ctrl let go: the next press goes back to the fourth.
        r.note(Some(doc(1)), &open, false);
        assert_eq!(r.step(Some(doc(1)), &open, true), Some(doc(4)));
    }

    /// While Ctrl is held the cycle's order stays as it was at the first press: Tab again goes
    /// on to the tab shown before that, and Shift+Tab back. Letting go makes the tab landed on
    /// the most recent, ahead of the one the cycle started from.
    #[test]
    fn the_order_holds_while_ctrl_is_held_and_settles_when_it_is_let_go() {
        let open = [doc(1), doc(2), doc(3), doc(4)];
        let mut r = RecentTabs::default();
        show(&mut r, &open, &[doc(2), doc(4), doc(1), doc(3)]);
        // Most recent first: 3, 1, 4, 2.
        let mut showing = doc(3);
        let mut visited = vec![];
        for _ in 0..4 {
            r.note(Some(showing), &open, true);
            showing = r.step(Some(showing), &open, true).unwrap();
            visited.push(showing);
        }
        assert_eq!(visited, [doc(1), doc(4), doc(2), doc(3)], "goes down the history and wraps around");
        r.note(Some(showing), &open, true);
        showing = r.step(Some(showing), &open, true).unwrap();
        r.note(Some(showing), &open, true);
        showing = r.step(Some(showing), &open, true).unwrap();
        assert_eq!(showing, doc(4));
        r.note(Some(showing), &open, true);
        showing = r.step(Some(showing), &open, false).unwrap();
        assert_eq!(showing, doc(1), "Shift+Tab steps back in the same order");
        // Let go on 1: the history is now 1, 3, 4, 2.
        r.note(Some(showing), &open, false);
        assert_eq!(r.step(Some(doc(1)), &open, true), Some(doc(3)));
        r.note(Some(doc(3)), &open, true);
        assert_eq!(r.step(Some(doc(3)), &open, true), Some(doc(4)));
    }

    /// Previous tab from the most recent tab goes to the least recent, and both directions wrap.
    #[test]
    fn previous_tab_goes_to_the_least_recent() {
        let open = [doc(1), doc(2), doc(3)];
        let mut r = RecentTabs::default();
        show(&mut r, &open, &[doc(1), doc(2), doc(3)]);
        assert_eq!(r.step(Some(doc(3)), &open, false), Some(doc(1)));
    }

    /// A tab closed during a cycle drops out of it, one opened during it comes last, and a tab
    /// opened in the background (never shown) is behind every tab that was shown.
    #[test]
    fn closed_and_newly_opened_tabs_keep_the_cycle_whole() {
        let mut open = vec![doc(1), doc(2), doc(3)];
        let mut r = RecentTabs::default();
        show(&mut r, &open, &[doc(3), doc(1)]);
        // 2 was never shown: it comes after 1 and 3.
        r.note(Some(doc(1)), &open, true);
        assert_eq!(r.step(Some(doc(1)), &open, true), Some(doc(3)));
        open.retain(|t| *t != doc(3));
        open.push(doc(9));
        // 3 closed while Ctrl was held: from 1 the cycle goes on to 2, then the new 9.
        r.note(Some(doc(1)), &open, true);
        assert_eq!(r.step(Some(doc(1)), &open, true), Some(doc(2)));
        r.note(Some(doc(2)), &open, true);
        assert_eq!(r.step(Some(doc(2)), &open, true), Some(doc(9)));
        r.note(Some(doc(9)), &open, false);
        assert_eq!(r.step(Some(doc(9)), &open, true), Some(doc(1)));
    }

    /// From Home, Next tab goes to the most recent tab and Previous tab to the least recent;
    /// with no tab open there is nowhere to go. The Combine files tab takes part like a document.
    #[test]
    fn from_home_and_with_the_combine_tab() {
        let open = [doc(1), TabKey::Combine, doc(2)];
        let mut r = RecentTabs::default();
        show(&mut r, &open, &[doc(2), TabKey::Combine, doc(1)]);
        r.note(None, &open, false);
        assert_eq!(r.step(None, &open, true), Some(doc(1)));
        r.note(None, &open, false);
        assert_eq!(r.step(None, &open, false), Some(doc(2)));
        r.note(Some(doc(1)), &open, false);
        assert_eq!(r.step(Some(doc(1)), &open, true), Some(TabKey::Combine));
        let mut empty = RecentTabs::default();
        assert_eq!(empty.step(None, &[], true), None);
        empty.note(None, &[], false);
        assert_eq!(empty.step(Some(doc(7)), &[], false), None, "a tab that isn't open is no way in");
    }

    /// Without Ctrl (the command palette, the automation tools) every press is a cycle of its
    /// own, so Next tab goes back and forth between the two most recent tabs.
    #[test]
    fn without_ctrl_each_press_goes_back_to_the_previous_tab() {
        let open = [doc(1), doc(2), doc(3)];
        let mut r = RecentTabs::default();
        show(&mut r, &open, &[doc(1), doc(2), doc(3)]);
        let mut showing = doc(3);
        let mut visited = vec![];
        for _ in 0..3 {
            r.note(Some(showing), &open, false);
            showing = r.step(Some(showing), &open, true).unwrap();
            visited.push(showing);
        }
        assert_eq!(visited, [doc(2), doc(3), doc(2)]);
    }

    /// CodeRabbit on #4553: two presses without Ctrl before the next frame (the automation tools
    /// run commands back to back) are two cycles too, so the second goes back.
    #[test]
    fn without_ctrl_two_presses_in_one_frame_go_back_and_forth() {
        let open = [doc(1), doc(2), doc(3)];
        let mut r = RecentTabs::default();
        show(&mut r, &open, &[doc(1), doc(2), doc(3)]);
        let first = r.step(Some(doc(3)), &open, true).unwrap();
        assert_eq!(first, doc(2));
        assert_eq!(r.step(Some(first), &open, true), Some(doc(3)), "back to the tab shown before");
    }

    #[test]
    fn settings_names_round_trip() {
        for c in TabCycle::ALL {
            assert_eq!(TabCycle::parse(c.token()), Some(c));
            assert_eq!(serde_json::to_value(c).unwrap(), serde_json::Value::String(c.token().to_string()));
            assert_eq!(serde_json::from_value::<TabCycle>(serde_json::json!(c.token())).unwrap(), c);
        }
        assert_eq!(TabCycle::parse("RECENT"), Some(TabCycle::Recent));
        assert_eq!(TabCycle::parse("mru"), None);
        assert_eq!(TabCycle::default(), TabCycle::Tabs, "tab order stays the default");
    }
}
