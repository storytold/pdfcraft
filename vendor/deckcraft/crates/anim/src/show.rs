//! Slide show navigation: click steps, slide order (hidden slides, custom shows, ranges), loop
//! and automatic advance.

use std::collections::{BTreeMap, HashMap};

use deckcraft_model::text::{Action, Hyperlink};
use deckcraft_model::{Presentation, SlideId};

use crate::Timeline;
use crate::interact::SlideRun;

/// What the UI should do after a navigation call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowAction {
    /// Nothing changed (e.g. back on the first slide).
    None,
    /// Start playing main step `n` of the current slide (clock from 0).
    PlayStep(usize),
    /// Step `n` jumped to its end (it was playing); nothing plays now.
    FinishStep(usize),
    /// Went back before step `n` (showing the state before it).
    StepBack(usize),
    /// Moved to slide index `n` (play its transition; `ShowState::playing` tells whether step 0
    /// starts by itself).
    Slide(usize),
    /// Past the last slide: show the end screen.
    End,
    /// Leave the show (next on the end screen).
    Exit,
}

// Which links the show opens on a click; PDF export writes the same ones.
pub use deckcraft_model::text::safe_url;

/// What a clicked link does in the show (`ShowState::follow`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkJump {
    /// Navigation inside the show (already applied to the state).
    Show(ShowAction),
    /// Open this URL (`http`, `https` or `mailto`, see [`safe_url`]) outside the show.
    Open(String),
    /// End the show.
    Exit,
    /// Nothing to follow.
    None,
}

/// Where a slide show is. Besides the current position it remembers what was played on every slide
/// the show has visited (main steps, triggered sequences), so a slide reached again by a link or
/// by going back looks the way it was left, as in PowerPoint. Going forward to a slide starts it
/// over, so its animations play again (#2090).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ShowState {
    /// Index into `presentation.slides`.
    pub slide: usize,
    /// Main step that plays next or is playing; `== steps` when the slide is fully built.
    pub step: usize,
    /// Step `step` is playing (its clock runs). With `step`, gives `Timeline::state(.., step, t)`.
    pub playing: bool,
    /// On the end screen.
    pub ended: bool,
    /// The current slide was reached by going back (← / Backspace), so the show plays the
    /// transition of the slide it left, in reverse, as PowerPoint does (#1531).
    pub backward: bool,
    /// The show has entered a slide (so `slide` and `step` are meaningful and can be saved).
    active: bool,
    /// The time of the last [`ShowState::tick`] (seconds), for the triggered animations' clocks.
    pub(crate) last_now: Option<f64>,
    /// What was played on each visited slide (index into `presentation.slides`).
    pub(crate) runs: BTreeMap<usize, SlideRun>,
    /// One-shot visit order (Custom Show from the dialog or a click). `None` uses [`show_order`].
    order: Option<Vec<usize>>,
    /// Name of the custom show being played, when there is one.
    custom: Option<String>,
    /// Return to Zoom: (zoom slide, last zoomed slide). Advancing past the last zoomed slide goes
    /// back to the zoom slide, fully built.
    pub zoom: Option<(usize, usize)>,
}

/// Index of each slide by id (the first slide wins a duplicate id), so resolving a custom show
/// costs the sum of its two lists, not their product.
fn slide_indices(pres: &Presentation) -> HashMap<SlideId, usize> {
    let mut map = HashMap::with_capacity(pres.slides.len());
    for (i, s) in pres.slides.iter().enumerate() {
        map.entry(s.id).or_insert(i);
    }
    map
}

/// Slide indices the show visits in order: the custom show or range from Set Up Slide Show, minus
/// hidden slides.
pub fn show_order(pres: &Presentation) -> Vec<usize> {
    let n = pres.slides.len();
    let base: Vec<usize> = match pres.show.custom_show.as_ref().and_then(|name| pres.custom_shows.iter().find(|c| &c.name == name)) {
        Some(cs) => {
            let index = slide_indices(pres);
            cs.slides.iter().filter_map(|id| index.get(id).copied()).collect()
        }
        None => match pres.show.range {
            Some((a, b)) => {
                let a = (a.max(1) as usize).saturating_sub(1);
                let b = (b as usize).min(n);
                (a..b).collect()
            }
            None => (0..n).collect(),
        },
    };
    base.into_iter().filter(|i| pres.slides.get(*i).is_some_and(|s| !s.hidden)).collect()
}

/// Visible slides of custom show `name`, in that show's order.
fn custom_indices(pres: &Presentation, name: &str) -> Option<Vec<usize>> {
    let cs = pres.custom_shows.iter().find(|c| c.name == name)?;
    let index = slide_indices(pres);
    let order: Vec<usize> =
        cs.slides.iter().filter_map(|id| index.get(id).copied()).filter(|&i| pres.slides.get(i).is_some_and(|s| !s.hidden)).collect();
    if order.is_empty() { None } else { Some(order) }
}

fn looping(pres: &Presentation) -> bool {
    pres.show.loop_until_esc || pres.show.show_type == "kiosk"
}

impl ShowState {
    /// Start at the first slide of the show order.
    pub fn new(pres: &Presentation) -> ShowState {
        match show_order(pres).first() {
            Some(&i) => ShowState::start_at(pres, i),
            None => ShowState { ended: true, ..Default::default() },
        }
    }

    /// Start at slide `i` (hidden slides allowed: "from current slide").
    pub fn start_at(pres: &Presentation, i: usize) -> ShowState {
        let mut s = ShowState::default();
        if i < pres.slides.len() {
            s.enter(pres, i);
        } else {
            s.ended = true;
        }
        if let Some(name) = pres.show.custom_show.clone() {
            s.custom = Some(name);
        }
        s
    }

    /// Play custom show `name` from its first slide, without changing Set Up Slide Show.
    /// `None` when that show is missing or has no visible slides.
    pub fn play_custom(pres: &Presentation, name: &str) -> Option<ShowState> {
        let order = custom_indices(pres, name)?;
        let &first = order.first()?;
        let mut s = ShowState::start_at(pres, first);
        s.order = Some(order);
        s.custom = Some(name.to_string());
        Some(s)
    }

    /// Name of the custom show this playback is limited to.
    pub fn custom_name(&self) -> Option<&str> {
        self.custom.as_deref().filter(|s| !s.is_empty())
    }

    /// The slide that follows the current one in this show.
    pub fn peek_next(&self, pres: &Presentation) -> Option<usize> {
        self.next_index(pres)
    }

    pub fn first_index(&self, pres: &Presentation) -> Option<usize> {
        self.visit(pres).first().copied()
    }

    pub fn last_index(&self, pres: &Presentation) -> Option<usize> {
        self.visit(pres).last().copied()
    }

    fn visit(&self, pres: &Presentation) -> Vec<usize> {
        match &self.order {
            Some(order) => order.iter().copied().filter(|&i| pres.slides.get(i).is_some_and(|s| !s.hidden)).collect(),
            None => show_order(pres),
        }
    }

    /// The timeline of slide `i`.
    pub fn timeline(pres: &Presentation, i: usize) -> Timeline {
        match pres.slides.get(i) {
            Some(s) => Timeline::new(s, pres.slide_size.width, pres.slide_size.height),
            None => Timeline::default(),
        }
    }

    /// Main steps of slide `i` (0 with Show without animation).
    pub fn steps(pres: &Presentation, i: usize) -> usize {
        if pres.show.without_animation { 0 } else { ShowState::timeline(pres, i).steps() }
    }

    /// Save what was played on the current slide before the show moves away from it. A main step
    /// that is playing is taken to its end. Triggered animations keep their place: their clock only
    /// runs while the slide is shown, and they go on from there when the show comes back.
    fn leave(&mut self, pres: &Presentation) {
        if !self.active {
            return;
        }
        let done = if self.playing { self.step + 1 } else { self.step };
        let step = done.min(ShowState::steps(pres, self.slide));
        self.runs.entry(self.slide).or_default().step = step;
        self.playing = false;
    }

    /// Show slide `i`: as it was left when the show has been there before, otherwise from the start.
    fn enter(&mut self, pres: &Presentation, i: usize) {
        self.leave(pres);
        self.active = true;
        self.slide = i;
        self.ended = false;
        self.backward = false;
        // Time spent elsewhere never counts for this slide's triggered animations.
        self.last_now = None;
        let steps = ShowState::steps(pres, i);
        let run = self.runs.entry(i).or_default();
        if run.visited {
            self.step = run.step.min(steps);
            self.playing = false;
        } else {
            run.visited = true;
            self.step = 0;
            self.playing = !pres.show.without_animation && ShowState::timeline(pres, i).step_is_auto(0);
        }
    }

    /// Advance to slide `i`: it starts over, as if the show had not been there, so its
    /// animations play again (#2090). Links and Back keep a slide as it was left.
    fn enter_forward(&mut self, pres: &Presentation, i: usize) {
        self.leave(pres);
        self.runs.remove(&i);
        self.enter(pres, i);
    }

    fn next_index(&self, pres: &Presentation) -> Option<usize> {
        let order = self.visit(pres);
        match order.iter().position(|&i| i == self.slide) {
            Some(p) => order.get(p + 1).copied(),
            None => order.iter().copied().find(|&i| i > self.slide),
        }
    }

    fn prev_index(&self, pres: &Presentation) -> Option<usize> {
        let order = self.visit(pres);
        match order.iter().position(|&i| i == self.slide) {
            Some(p) => p.checked_sub(1).and_then(|q| order.get(q).copied()),
            None => order.iter().copied().rev().find(|&i| i < self.slide),
        }
    }

    /// Click / → / Space: finish the playing step, else play the next step, else go to the next slide.
    pub fn next(&mut self, pres: &Presentation) -> ShowAction {
        if pres.slides.is_empty() {
            self.ended = true;
            return ShowAction::Exit;
        }
        if self.ended {
            return ShowAction::Exit;
        }
        if self.playing {
            return self.step_done(pres);
        }
        let steps = ShowState::steps(pres, self.slide);
        if self.step < steps {
            self.playing = true;
            return ShowAction::PlayStep(self.step);
        }
        if let Some((back, until)) = self.zoom
            && until == self.slide
            && back < pres.slides.len()
        {
            self.zoom = None;
            self.enter(pres, back);
            self.step = ShowState::steps(pres, back);
            self.playing = false;
            return ShowAction::Slide(back);
        }
        match self.next_index(pres) {
            Some(i) => {
                self.enter_forward(pres, i);
                ShowAction::Slide(i)
            }
            None if looping(pres) => match self.visit(pres).first() {
                Some(&i) => {
                    self.enter_forward(pres, i);
                    ShowAction::Slide(i)
                }
                None => {
                    self.ended = true;
                    ShowAction::End
                }
            },
            None => {
                self.ended = true;
                self.playing = false;
                ShowAction::End
            }
        }
    }

    /// The playing step reached its end (the UI's clock passed `step_duration`).
    pub fn step_done(&mut self, pres: &Presentation) -> ShowAction {
        if !self.playing {
            return ShowAction::None;
        }
        let n = self.step;
        self.playing = false;
        self.step = (self.step + 1).min(ShowState::steps(pres, self.slide));
        ShowAction::FinishStep(n)
    }

    /// ← / Backspace: undo the last step, else go to the previous slide fully built. An automatic
    /// first step (With/After Previous) is part of the slide appearing, so it is never undone.
    pub fn prev(&mut self, pres: &Presentation) -> ShowAction {
        if self.ended {
            self.ended = false;
            if self.slide >= pres.slides.len() {
                return ShowAction::None;
            }
            self.step = ShowState::steps(pres, self.slide);
            self.playing = false;
            self.backward = false;
            return ShowAction::Slide(self.slide);
        }
        // #844: undoing the automatic first step left the slide as before it appeared, and the show
        // played the step again at once, so Back never got past the slide.
        let undo = if self.playing { Some(self.step) } else { self.step.checked_sub(1) };
        let auto_first = undo == Some(0) && !pres.show.without_animation && ShowState::timeline(pres, self.slide).step_is_auto(0);
        if let Some(k) = undo
            && !auto_first
        {
            self.playing = false;
            self.step = k;
            return ShowAction::StepBack(k);
        }
        let target = match self.prev_index(pres) {
            Some(i) => Some(i),
            None if looping(pres) => self.visit(pres).last().copied().filter(|&i| i != self.slide),
            None => None,
        };
        match target {
            Some(i) => {
                // A slide the show has been to looks as it was left; one it has not been to is
                // shown fully built.
                let seen = self.runs.get(&i).is_some_and(|r| r.visited);
                self.enter(pres, i);
                if !seen {
                    self.step = ShowState::steps(pres, i);
                    self.playing = false;
                }
                self.backward = true;
                ShowAction::Slide(i)
            }
            None => ShowAction::None,
        }
    }

    /// Jump to slide `i` (number + Enter, See All Slides); hidden slides are shown when jumped to.
    pub fn goto(&mut self, pres: &Presentation, i: usize) -> ShowAction {
        if i >= pres.slides.len() {
            return ShowAction::None;
        }
        self.zoom = None;
        self.enter(pres, i);
        ShowAction::Slide(i)
    }

    /// Follow a clicked hyperlink or shape action. `last_viewed` is the slide shown before the
    /// current one (Last Slide Viewed). Moves to slide targets; everything else is for the UI.
    pub fn follow(&mut self, pres: &Presentation, action: &Action, last_viewed: Option<usize>) -> LinkJump {
        let target = match action {
            Action::Url { url } => return if safe_url(url) { LinkJump::Open(url.trim().to_string()) } else { LinkJump::None },
            Action::EndShow => return LinkJump::Exit,
            Action::Slide { slide } => pres.slides.iter().position(|s| s.id == *slide),
            Action::NextSlide => self.next_index(pres),
            Action::PreviousSlide => self.prev_index(pres),
            Action::FirstSlide => self.visit(pres).first().copied(),
            Action::LastSlide => self.visit(pres).last().copied(),
            Action::LastViewed => last_viewed,
            Action::CustomShow { name, .. } => {
                let Some(order) = custom_indices(pres, name) else { return LinkJump::None };
                let Some(&i) = order.first() else { return LinkJump::None };
                self.order = Some(order);
                self.custom = Some(name.clone());
                return LinkJump::Show(self.goto(pres, i));
            }
            // Programs and media actions aren't followed.
            Action::Program { .. } | Action::PlayMedia => None,
        };
        match target {
            Some(i) => LinkJump::Show(self.goto(pres, i)),
            None => LinkJump::None,
        }
    }

    /// Follow a clicked link, honouring a zoom's Return to Zoom (see [`Hyperlink::zoom_return`]).
    pub fn follow_link(&mut self, pres: &Presentation, link: &Hyperlink, last_viewed: Option<usize>) -> LinkJump {
        let from = self.slide;
        let jump = self.follow(pres, &link.action, last_viewed);
        if let Some(until) = link.zoom_return.and_then(|id| pres.slides.iter().position(|s| s.id == id))
            && matches!(jump, LinkJump::Show(ShowAction::Slide(_)))
        {
            self.zoom = Some((from, until));
        }
        jump
    }

    /// Seconds after which the current slide advances by itself (Advance Slide ▸ After), when the
    /// show uses timings.
    pub fn advance_after(&self, pres: &Presentation) -> Option<f64> {
        if !pres.show.use_timings || self.ended {
            return None;
        }
        pres.slides.get(self.slide).and_then(|s| s.transition.as_ref()).and_then(|t| t.advance_after_ms).map(|ms| ms as f64 / 1000.0)
    }

    /// Should the show advance now, `elapsed` seconds after the slide appeared? Waits for the
    /// slide's animations: auto-advance happens once nothing plays and the time is up; remaining
    /// click steps play automatically with timings.
    pub fn auto_advance_due(&self, pres: &Presentation, elapsed: f64) -> bool {
        match self.advance_after(pres) {
            Some(after) => !self.playing && elapsed.is_finite() && elapsed >= after,
            None => false,
        }
    }
}
